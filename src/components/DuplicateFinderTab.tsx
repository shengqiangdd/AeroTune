import { useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { Check, ChevronDown, ChevronRight, Copy, Download, FolderOpen, ScanSearch, Search, StopCircle, Trash2 } from "lucide-react";
import { api, DuplicateGroup, DuplicateScanProgress, DuplicateScanResult } from "../lib/api";
import { buildDuplicateCsv } from "../lib/duplicateCsv.js";
import { Btn, Card, Chip } from "./ui";
import { fmtBytes } from "../lib/format";
import { toast, toastErr, toastOk } from "../lib/toast";

const GROUP_PAGE_SIZE = 20;

function fmtModified(unixSeconds: number): string {
  if (!Number.isFinite(unixSeconds) || unixSeconds <= 0) return "修改时间未知";
  return new Date(unixSeconds * 1000).toLocaleString();
}

export function DuplicateFinderTab() {
  const [rootToken, setRootToken] = useState("");
  const [result, setResult] = useState<DuplicateScanResult | null>(null);
  const [progress, setProgress] = useState<DuplicateScanProgress | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(false);
  const [choosingRoot, setChoosingRoot] = useState(false);
  const [canceling, setCanceling] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [query, setQuery] = useState("");
  const [page, setPage] = useState(0);
  const [expandedGroupIds, setExpandedGroupIds] = useState<Set<string>>(new Set());
  const scanPending = useRef(false);
  const rootPickerPending = useRef(false);
  const cancelPending = useRef(false);
  const deletePending = useRef(false);

  useEffect(() => {
    let unsubscribe: (() => void) | undefined;
    let disposed = false;
    api.onDuplicateScanProgress((next) => {
      if (!disposed) setProgress(next);
    }).then((stop) => {
      if (disposed) stop();
      else unsubscribe = stop;
    }).catch(() => {});
    return () => {
      disposed = true;
      unsubscribe?.();
      if (scanPending.current) void api.cancelDuplicateScan().catch(() => {});
    };
  }, []);

  const scan = async (selectionToken: string) => {
    if (!selectionToken || scanPending.current || deletePending.current) return;
    scanPending.current = true;
    cancelPending.current = false;
    setRootToken(selectionToken);
    setScanning(true);
    setCanceling(false);
    setProgress(null);
    setResult(null);
    setSelected(new Set());
    try {
      const next = await api.scanDuplicateFiles(selectionToken);
      setRootToken(next.root_token);
      setResult(next);
      setSelected(new Set());
      setQuery("");
      setPage(0);
      setExpandedGroupIds(new Set(next.groups.slice(0, 1).map((group) => group.id)));
      if (next.cancelled) {
        toast("info", `扫描已取消；保留 ${next.groups.length} 组已核实结果`);
      } else {
        toastOk(`扫描完成：${next.groups.length} 组完全相同的文件，逻辑重复容量 ${fmtBytes(next.logical_duplicate_bytes)}`);
      }
    } catch (error) {
      toastErr(error);
    } finally {
      scanPending.current = false;
      cancelPending.current = false;
      setScanning(false);
      setCanceling(false);
      setProgress(null);
    }
  };

  const chooseFolder = async () => {
    if (scanPending.current || deletePending.current || rootPickerPending.current) return;
    rootPickerPending.current = true;
    setChoosingRoot(true);
    try {
      const selectionToken = await api.pickDuplicateScanRoot();
      if (selectionToken) await scan(selectionToken);
    } catch (error) {
      toastErr(error);
    } finally {
      rootPickerPending.current = false;
      setChoosingRoot(false);
    }
  };

  const cancelScan = async () => {
    if (!scanPending.current || cancelPending.current) return;
    cancelPending.current = true;
    setCanceling(true);
    try {
      await api.cancelDuplicateScan();
      toast("info", "正在停止；已完成的匹配结果会保留");
    } catch (error) {
      cancelPending.current = false;
      setCanceling(false);
      toastErr(error);
    }
  };

  const maxSelectableFor = (group: DuplicateGroup) =>
    Math.min(Math.max(0, group.files.length - 1), Math.max(0, group.files_total - 1));
  const selectedCountFor = (group: DuplicateGroup, values = selected) =>
    group.files.reduce((count, file) => count + Number(values.has(file.path)), 0);

  const toggleFile = (group: DuplicateGroup, path: string) => {
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(path)) {
        next.delete(path);
      } else if (selectedCountFor(group, current) < maxSelectableFor(group)) {
        next.add(path);
      }
      return next;
    });
  };

  const selectedPaths = useMemo(() => [...selected], [selected]);
  const estimatedSelectedBytes = useMemo(
    () => result?.groups.reduce((sum, group) =>
      sum + group.files.reduce((groupSum, file) => groupSum + (selected.has(file.path) ? file.bytes : 0), 0), 0) ?? 0,
    [result, selected],
  );
  const deferredQuery = useDeferredValue(query.trim().toLocaleLowerCase());
  const filteredGroups = useMemo(() => {
    if (!result) return [];
    if (!deferredQuery) return result.groups;
    return result.groups.filter((group) =>
      group.id.toLocaleLowerCase().includes(deferredQuery)
      || String(group.bytes).includes(deferredQuery)
      || fmtBytes(group.bytes).toLocaleLowerCase().includes(deferredQuery)
      || group.files.some((file) => file.path.toLocaleLowerCase().includes(deferredQuery)),
    );
  }, [result, deferredQuery]);
  const pageCount = Math.max(1, Math.ceil(filteredGroups.length / GROUP_PAGE_SIZE));
  const pageGroups = filteredGroups.slice(page * GROUP_PAGE_SIZE, (page + 1) * GROUP_PAGE_SIZE);

  const toggleGroup = (groupId: string) => {
    setExpandedGroupIds((current) => {
      const next = new Set(current);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  };

  const exportCsv = async () => {
    if (!result || result.groups.length === 0 || exporting) return;
    setExporting(true);
    try {
      const filename = `AeroTune-duplicates-${new Date().toISOString().replace(/[:.]/g, "-")}.csv`;
      const saved = await api.saveTextExport(filename, buildDuplicateCsv(result));
      if (!saved) return;
      const rows = result.groups.reduce((count, group) => count + group.files.length, 0);
      toastOk(`已导出 ${result.groups.length} 组、${rows.toLocaleString()} 条路径到所选文件`);
    } catch (error) {
      toastErr(error);
    } finally {
      setExporting(false);
    }
  };

  const moveSelectedToRecycleBin = async () => {
    if (selectedPaths.length === 0 || deletePending.current || scanPending.current) return;
    if (!result?.delete_token) {
      toastErr("扫描授权已失效，请重新扫描后再试");
      setSelected(new Set());
      setResult(null);
      if (rootToken) await scan(rootToken);
      return;
    }
    if (!confirm(`将 ${selectedPaths.length} 个已选副本移入回收站？每组至少会保留一个副本；估算逻辑容量 ${fmtBytes(estimatedSelectedBytes)}，不等同于实际释放空间。`)) return;
    deletePending.current = true;
    setDeleting(true);
    let moved = 0;
    const rootToRescan = result.root_token;
    const deleteToken = result.delete_token;
    try {
      for (let offset = 0; offset < selectedPaths.length; offset += 20) {
        const batch = selectedPaths.slice(offset, offset + 20);
        await api.recycleDelete(deleteToken, batch);
        moved += batch.length;
      }
      setSelected(new Set());
      toastOk(`已将 ${moved} 个副本送入回收站；正在重新扫描`);
      deletePending.current = false;
      setDeleting(false);
      setResult(null);
      await scan(rootToRescan);
    } catch (error) {
      toastErr(moved > 0 ? `已处理 ${moved} 个副本，其余操作失败：${String(error)}` : error);
      // A failed Shell recycle batch may have partially succeeded; discard the stale token/result.
      setSelected(new Set());
      deletePending.current = false;
      setDeleting(false);
      setResult(null);
      await scan(rootToRescan);
    } finally {
      deletePending.current = false;
      setDeleting(false);
    }
  };

  const statusText = progress
    ? `${progress.phase === "enumerating" ? "正在枚举文件" : progress.phase === "hashing" ? "正在完整读取并校验内容" : progress.phase === "cancelled" ? "已取消" : "扫描完成"} · 已检查 ${progress.files_scanned.toLocaleString()} 个文件 · 候选 ${progress.candidate_files.toLocaleString()} 个 · 已读取 ${fmtBytes(progress.bytes_hashed)}${progress.current ? ` · ${progress.current}` : ""}`
    : choosingRoot ? "正在打开原生文件夹选择器…" : scanning ? "正在准备扫描所选目录…" : result ? `上次扫描目录：${result.root}` : rootToken ? "上次选择已保留；若授权过期，请重新选择目录" : "尚未选择目录";

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <Btn onClick={chooseFolder} disabled={scanning || deleting || choosingRoot}>
          <FolderOpen size={14} /> {choosingRoot ? "正在打开选择器…" : "选择文件夹并扫描"}
        </Btn>
        {scanning && (
          <Btn variant="danger" onClick={cancelScan} disabled={canceling}>
            <StopCircle size={14} /> {canceling ? "正在停止…" : "取消扫描"}
          </Btn>
        )}
        {result && result.groups.length > 0 && (
          <Btn variant="ghost" onClick={exportCsv} disabled={exporting || scanning || deleting}>
            <Download size={14} /> {exporting ? "导出中…" : "导出全部 CSV"}
          </Btn>
        )}
        {selectedPaths.length > 0 && (
          <Btn variant="danger" onClick={moveSelectedToRecycleBin} disabled={deleting || scanning}>
            <Trash2 size={14} /> {deleting ? "移入回收站…" : `回收站删除 ${selectedPaths.length} 个副本`}
          </Btn>
        )}
      </div>

      <Card className="mb-4">
        <div className="flex items-start gap-3">
          <Copy size={17} className="mt-0.5 shrink-0 text-cyan-300" />
          <div className="min-w-0 flex-1">
            <div className="text-[13px] text-slate-200">只扫描你选定的文件夹；仅将 SHA-256 完整内容一致的非空文件归为重复项。</div>
            <div className="mt-1 text-[11px] leading-relaxed text-slate-500">按文件大小预筛后再分块完整读取，扫描可取消。结果按每组至少保留一份约束；选中项会送入回收站，不会直接永久删除。逻辑重复容量不等同于可实际释放的磁盘空间。</div>
            <div role="status" aria-live="polite" className="mt-2 break-all text-[11px] text-slate-400">{statusText}</div>
          </div>
        </div>
      </Card>

      {scanning && (
        <Card className="mb-4">
          <div className="mb-2 flex items-center gap-2 text-[12px] text-cyan-200">
            <ScanSearch size={14} className="animate-pulse" />
            <span>{progress?.phase === "hashing" ? "完整内容校验中" : "文件枚举中"}</span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-white/8">
            <div className="h-full w-1/3 animate-pulse rounded-full grad-btn" />
          </div>
        </Card>
      )}

      {result && (
        <>
          <div className="mb-4 grid grid-cols-2 gap-3 xl:grid-cols-4">
            {[
              { label: "检查文件", value: result.files_scanned.toLocaleString() },
              { label: "候选文件", value: result.candidate_files.toLocaleString() },
              { label: "完整读取", value: fmtBytes(result.bytes_hashed) },
              { label: "逻辑重复容量", value: fmtBytes(result.logical_duplicate_bytes) },
            ].map((item) => (
              <div key={item.label} className="glass min-w-0 p-3.5">
                <div className="text-[11px] text-slate-500">{item.label}</div>
                <div className="mt-0.5 truncate text-base font-semibold text-slate-100">{item.value}</div>
              </div>
            ))}
          </div>

          {(result.cancelled || result.truncated || result.skipped_unreadable > 0 || result.changed_during_scan > 0) && (
            <div className="mb-4 flex flex-wrap gap-2" role="status" aria-live="polite">
              {result.cancelled && <Chip tone="amber">扫描已取消，展示已核实结果</Chip>}
              {result.truncated && <Chip tone="amber">达到扫描/结果上限，结果可能不完整</Chip>}
              {result.skipped_unreadable > 0 && <Chip tone="amber">跳过不可读项 {result.skipped_unreadable}</Chip>}
              {result.changed_during_scan > 0 && <Chip tone="amber">扫描期间变更 {result.changed_during_scan} 项</Chip>}
            </div>
          )}

          {result.groups.length > 0 && (
            <div className="mb-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <label htmlFor="duplicate-group-search" className="sr-only">搜索重复文件路径、大小或组 ID</label>
              <div className="relative min-w-0 flex-1 sm:max-w-xl">
                <Search size={14} className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-slate-500" />
                <input
                  id="duplicate-group-search"
                  type="search"
                  value={query}
                  onChange={(event) => { setQuery(event.target.value); setPage(0); }}
                  placeholder="搜索文件路径、大小或组 ID"
                  aria-label="搜索重复文件路径、大小或组 ID"
                  className="w-full rounded-lg border border-white/10 bg-white/4 py-2 pl-9 pr-3 text-[12px] text-slate-200 outline-none placeholder:text-slate-500 focus:border-cyan-400/40 focus:ring-2 focus:ring-cyan-300/30"
                />
              </div>
              <span className="text-[11px] text-slate-500">匹配 {filteredGroups.length} / {result.groups.length} 组</span>
            </div>
          )}

          {result.groups.length === 0 ? (
            <Card className="py-12 text-center">
              <Check size={28} className="mx-auto mb-2 text-emerald-300" />
              <div className="text-sm text-slate-300">{result.cancelled ? "已停止；目前没有完整核实的重复组" : "没有发现内容完全相同的非空文件"}</div>
              <div className="mt-1 text-xs text-slate-500">扫描了 {result.files_scanned.toLocaleString()} 个文件，耗时 {(result.duration_ms / 1000).toFixed(1)} 秒</div>
            </Card>
          ) : filteredGroups.length === 0 ? (
            <Card className="py-10 text-center text-sm text-slate-400">没有匹配的重复组；可尝试更短的路径片段或文件大小。</Card>
          ) : (
            <>
              <div className="space-y-3">
                {pageGroups.map((group) => {
                  const selectedInGroup = selectedCountFor(group);
                  const expanded = expandedGroupIds.has(group.id);
                  const panelId = `duplicate-group-${group.id.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
                  return (
                    <Card
                      key={group.id}
                      title={`${group.files_total.toLocaleString()} 个相同文件 · 每个 ${fmtBytes(group.bytes)}`}
                      icon={<Copy size={15} className="text-violet-300" />}
                      right={(
                        <div className="flex flex-wrap items-center justify-end gap-2">
                          <Chip tone="cyan">额外逻辑容量 {fmtBytes(group.logical_duplicate_bytes)}</Chip>
                          <button
                            type="button"
                            aria-expanded={expanded}
                            aria-controls={panelId}
                            onClick={() => toggleGroup(group.id)}
                            className="inline-flex items-center gap-1 rounded-md border border-white/10 px-2 py-1 text-[11px] text-slate-300 hover:bg-white/8 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70"
                          >
                            {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                            {expanded ? "收起路径" : "展开路径"}
                          </button>
                        </div>
                      )}
                    >
                      <div className="mb-2 flex flex-wrap items-center justify-between gap-2 text-[11px] text-slate-500">
                        <span>已选 {selectedInGroup} / {maxSelectableFor(group)} 个可见可移除副本</span>
                        {group.files_truncated && <Chip tone="amber">仅展示此组前 {group.files.length} 个路径；可重扫继续处理</Chip>}
                      </div>
                      <div id={panelId} hidden={!expanded} aria-label={`${group.files_total} 个重复文件路径`}>
                        {expanded ? (
                          <div className="max-h-80 space-y-1 overflow-y-auto pr-1">
                            {group.files.map((file) => {
                              const checked = selected.has(file.path);
                              const lastCopySelected = checked && !group.files_truncated && selectedInGroup >= maxSelectableFor(group);
                              return (
                                <label
                                  key={file.path}
                                  className={`flex min-w-0 items-start gap-3 rounded-lg border px-3 py-2 text-[12px] transition-colors ${checked ? "border-rose-400/30 bg-rose-400/8" : "border-white/6 bg-white/3 hover:bg-white/6"}`}
                                >
                                  <input
                                    type="checkbox"
                                    checked={checked}
                                    disabled={(!checked && selectedInGroup >= maxSelectableFor(group)) || deleting || scanning}
                                    onChange={() => toggleFile(group, file.path)}
                                    aria-label={`移入回收站副本：${file.path}；每组至少保留一份`}
                                    className="mt-0.5 h-4 w-4 shrink-0 accent-cyan-400"
                                  />
                                  <span className="min-w-0 flex-1">
                                    <span className="block break-all text-slate-300" title={file.path}>{file.path}</span>
                                    <span className="mt-1 block text-[10px] text-slate-500">{fmtModified(file.modified_unix_s)}{lastCopySelected ? " · 最后一份不能删除" : ""}</span>
                                  </span>
                                </label>
                              );
                            })}
                          </div>
                        ) : null}
                      </div>
                    </Card>
                  );
                })}
                {result.truncated && <div className="text-center text-[11px] text-slate-500">已按潜在重复容量优先展示；达到上限的扫描可缩小目录后再次运行。</div>}
              </div>
              {filteredGroups.length > GROUP_PAGE_SIZE && (
                <div className="mt-4 flex flex-wrap items-center justify-center gap-3">
                  <Btn variant="ghost" onClick={() => setPage((value) => Math.max(0, value - 1))} disabled={page === 0}>上一页</Btn>
                  <span role="status" aria-live="polite" className="text-[11px] text-slate-400">第 {page + 1} / {pageCount} 页 · 每页 {GROUP_PAGE_SIZE} 组</span>
                  <Btn variant="ghost" onClick={() => setPage((value) => Math.min(pageCount - 1, value + 1))} disabled={page >= pageCount - 1}>下一页</Btn>
                </div>
              )}
            </>
          )}
        </>
      )}

      {!result && !scanning && (
        <Card className="flex flex-col items-center py-12 text-center">
          <FolderOpen size={32} className="mb-3 text-slate-600" />
          <div className="mb-1 text-sm text-slate-300">选择一个文件夹开始查找</div>
          <div className="max-w-xl text-xs leading-relaxed text-slate-500">扫描耗时和磁盘读取量取决于目录大小。软链接不会跟随；每组的删除复选框默认不勾选，并始终保留至少一个副本。</div>
        </Card>
      )}
    </div>
  );
}
