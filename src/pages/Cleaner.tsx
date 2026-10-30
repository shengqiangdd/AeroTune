// 空间与存储中心：垃圾清理 / 磁盘分析 / 系统维护（SMART·休眠·更新·预装应用）
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Trash2, ScanSearch, Sparkles, ShieldAlert, HardDrive, Wrench, ScanLine,
  BatteryCharging, MonitorCog, PackageX, Search, FolderOpen, RefreshCw, Server, History as HistoryIcon, Gauge, ChevronDown, Copy,
} from "lucide-react";
import { api, JunkItem, JunkCleanupPreview, DiskScanResult, DiskScanPreset, ScanProgress, DiskHealth, DiskTrendReport, AppxApp, HibernateInfo, ResidualItem, DefragStatus } from "../lib/api";
import { Btn, PageHead, Card, Chip, Toggle } from "../components/ui";
import { toastOk, toastErr, toast } from "../lib/toast";
import { fmtFileSize } from "../lib/format";
import { DuplicateFinderTab } from "../components/DuplicateFinderTab";

const CONF: Record<string, { label: string; tone: "green" | "amber" | "slate" }> = {
  high: { label: "高置信", tone: "green" },
  mid: { label: "中置信", tone: "amber" },
  low: { label: "较新", tone: "slate" },
};

type Tab = "junk" | "disk" | "duplicates" | "system";
const STORAGE_TABS: { id: Tab; label: string; icon: React.ReactNode }[] = [
  { id: "junk", label: "垃圾清理", icon: <Trash2 size={14} /> },
  { id: "disk", label: "磁盘分析", icon: <HardDrive size={14} /> },
  { id: "duplicates", label: "重复文件", icon: <Copy size={14} /> },
  { id: "system", label: "系统维护", icon: <Wrench size={14} /> },
];

export function Cleaner() {
  const [tab, setTab] = useState<Tab>("junk");
  const moveTab = (current: Tab, direction: -1 | 1) => {
    const index = STORAGE_TABS.findIndex((item) => item.id === current);
    const next = STORAGE_TABS[(index + direction + STORAGE_TABS.length) % STORAGE_TABS.length].id;
    setTab(next);
    requestAnimationFrame(() => document.getElementById(`cleaner-${next}-tab`)?.focus());
  };
  const panel = (id: Tab) => id === "junk" ? <JunkTab />
    : id === "disk" ? <DiskTab />
      : id === "duplicates" ? <DuplicateFinderTab /> : <SystemTab />;

  return (
    <div>
      <PageHead title="空间与存储" desc="垃圾清理 · 磁盘分析 · 重复文件查找 · 系统维护" />
      <div role="tablist" aria-label="空间与存储功能" className="mb-5 grid grid-cols-1 gap-2 sm:flex sm:flex-wrap">
        {STORAGE_TABS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            id={`cleaner-${item.id}-tab`}
            aria-controls={`cleaner-${item.id}-panel`}
            aria-selected={tab === item.id}
            tabIndex={tab === item.id ? 0 : -1}
            onClick={() => setTab(item.id)}
            onKeyDown={(event) => {
              if (event.key === "ArrowRight") { event.preventDefault(); moveTab(item.id, 1); }
              else if (event.key === "ArrowLeft") { event.preventDefault(); moveTab(item.id, -1); }
              else if (event.key === "Home") { event.preventDefault(); const first = STORAGE_TABS[0].id; setTab(first); requestAnimationFrame(() => document.getElementById(`cleaner-${first}-tab`)?.focus()); }
              else if (event.key === "End") { event.preventDefault(); const last = STORAGE_TABS[STORAGE_TABS.length - 1].id; setTab(last); requestAnimationFrame(() => document.getElementById(`cleaner-${last}-tab`)?.focus()); }
            }}
            className={`inline-flex items-center gap-1.5 rounded-lg border px-4 py-2 text-[13px] transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70 ${
              tab === item.id
                ? "border-cyan-400/40 bg-cyan-400/12 text-cyan-200"
                : "border-white/10 bg-white/4 text-slate-300 hover:bg-white/10"
            }`}
          >
            {item.icon} {item.label}
          </button>
        ))}
      </div>
      {STORAGE_TABS.map((item) => (
        <div
          key={item.id}
          id={`cleaner-${item.id}-panel`}
          role="tabpanel"
          aria-labelledby={`cleaner-${item.id}-tab`}
          tabIndex={0}
          hidden={tab !== item.id}
          className={tab === item.id ? "rise focus-visible:outline-none" : ""}
        >
          {item.id === "duplicates" ? <DuplicateFinderTab /> : tab === item.id ? panel(item.id) : null}
        </div>
      ))}
    </div>
  );
}

/* ================= 垃圾清理（原功能） ================= */

function JunkTab() {
  const [items, setItems] = useState<JunkItem[] | null>(null);
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(false);
  const [cleaning, setCleaning] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [preview, setPreview] = useState<JunkCleanupPreview | null>(null);
  const [olderOnly, setOlderOnly] = useState(false);

  const scan = async () => {
    setScanning(true);
    setItems(null);
    setPreview(null);
    try {
      const r = await api.scanJunk();
      setItems(r);
      setChecked(new Set(r.filter((i) => !i.admin_required && i.confidence !== "low").map((i) => i.id)));
    } catch (e) {
      toastErr(e);
    } finally {
      setScanning(false);
    }
  };

  const preparePreview = async () => {
    if (checked.size === 0 || previewing) return;
    setPreviewing(true);
    setPreview(null);
    try {
      const nextPreview = await api.previewJunk([...checked], olderOnly ? 7 : 0);
      setPreview(nextPreview);
      if (nextPreview.over_limit) toast("info", "所选项目超过预览上限；请缩小选择后重新预览。 ");
      else toastOk("已生成短时有效的文件清理预览");
    } catch (e) {
      toastErr(e);
    } finally {
      setPreviewing(false);
    }
  };

  const clean = async () => {
    if (!preview?.can_execute || !preview.plan_id) return;
    const regularCategories = preview.categories.filter((category) => !category.permanent_action);
    const candidateFiles = regularCategories.reduce((sum, category) => sum + category.candidate_files, 0);
    if (candidateFiles === 0) return;
    setCleaning(true);
    try {
      const result = await api.cleanJunk(preview.plan_id);
      if (result.cancelled) {
        toast("info", "已取消本次清理，未删除预览文件；旧预览已失效，请重新预览");
        await scan();
        return;
      }
      const details = [`删除 ${result.deleted_files} 个文件（${fmtFileSize(result.deleted_bytes)}）`];
      if (result.skipped_changed) details.push(`变化/已消失 ${result.skipped_changed} 个`);
      if (result.skipped_unsafe) details.push(`不安全路径跳过 ${result.skipped_unsafe} 个`);
      if (result.failed) details.push(`失败 ${result.failed} 个`);
      toastOk(`清理完成：${details.join("；")}`);
      if (result.errors.length) toast("info", `部分文件未能删除：${result.errors.join("；")}`);
      await scan();
    } catch (e) {
      toastErr(e);
      await scan();
    } finally {
      setCleaning(false);
    }
  };

  const emptyRecycleBin = async () => {
    if (!preview?.plan_id || !preview.categories.some((category) => category.permanent_action)) return;
    setCleaning(true);
    try {
      const cleared = await api.emptyRecycleBin(preview.plan_id);
      if (!cleared) {
        toast("info", "已取消清空，未执行删除；旧预览已失效，请重新预览");
        await scan();
        return;
      }
      toastOk("回收站已清空");
      await scan();
    } catch (e) {
      toastErr(e);
      await scan();
    } finally {
      setCleaning(false);
    }
  };

  const setCategorySelected = (id: string, selected: boolean) => {
    setPreview(null);
    setChecked((current) => {
      const next = new Set(current);
      if (selected) next.add(id);
      else next.delete(id);
      return next;
    });
  };

  const totalMb = (items ?? []).filter((i) => checked.has(i.id)).reduce((s, i) => s + i.size_mb, 0);
  const adminMissing = (items ?? []).some((i) => i.admin_required && i.size_mb > 50);
  const previewRegularFiles = preview?.categories.filter((category) => !category.permanent_action).reduce((sum, category) => sum + category.candidate_files, 0) ?? 0;
  const previewHasRecycle = preview?.categories.some((category) => category.permanent_action) ?? false;

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
        <div className="text-[13px] text-slate-400">
          临时文件 · 浏览器缓存 · 更新缓存 · 回收站
        </div>
        <div className="flex w-full flex-wrap items-center gap-2 sm:w-auto">
          <Chip tone={olderOnly ? "green" : "slate"}>
            <button disabled={cleaning || previewing} onClick={() => { setPreview(null); setOlderOnly(!olderOnly); }}>
              {olderOnly ? "仅清理 7 天以上旧文件" : "全部文件"}
            </button>
          </Chip>
          <Btn variant="ghost" onClick={scan} disabled={scanning || cleaning || previewing}>
            <ScanSearch size={14} className={scanning ? "animate-pulse" : ""} />
            {scanning ? "扫描中…" : "扫描"}
          </Btn>
          <Btn onClick={preparePreview} disabled={previewing || scanning || checked.size === 0}>
            <Sparkles size={14} />
            {previewing ? "生成预览…" : `预览 ${checked.size} 项（扫描估算 ~${totalMb.toFixed(0)} MB）`}
          </Btn>
        </div>
      </div>

      {preview && <Card className="mb-4" title="清理预览 · 计划 5 分钟后过期" icon={<HistoryIcon size={15} className="text-cyan-300" />}>
        <div className={`mb-3 rounded-md border px-3 py-2 text-[11px] ${preview.over_limit ? "border-amber-300/20 bg-amber-300/5 text-amber-200" : "border-cyan-300/12 bg-cyan-300/4 text-slate-300"}`}>
          {preview.over_limit
            ? "候选扫描达到安全上限，当前结果不完整且不会生成可执行计划；请缩小类别或年龄范围后重新预览。"
            : "以下清单由后端按固定清理目录生成；执行时将复核路径范围、文件类型、大小和修改时间。永久删除前会出现 Windows 系统确认框；取消会刷新预览。筛选、扫描或重新预览会替换此计划。"}
        </div>
        <div className="space-y-2">
          {preview.categories.map((category) => <div key={category.id} className="rounded-lg border border-white/7 bg-black/10 px-3 py-2.5">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs font-medium text-slate-200">{category.label}{category.permanent_action ? " · 单独永久操作" : ""}</span>
              <span className="text-[11px] tabular-nums text-slate-300">{category.permanent_action ? `估算 ${fmtFileSize(category.candidate_bytes)}` : `${category.candidate_files.toLocaleString()} 个文件 · ${fmtFileSize(category.candidate_bytes)}`}</span>
            </div>
            <div className="mt-1 text-[10px] text-slate-500">
              {category.permanent_action
                ? "Windows 未提供可逐项预览的回收站文件列表；单独清空会永久删除当前用户回收站中的全部项目，执行前会出现 Windows 系统确认框。"
                : `按年龄条件筛除 ${category.age_filtered_files.toLocaleString()} 个 · 链接/不可读/越界跳过 ${category.skipped_files.toLocaleString()} 个${category.exact ? "" : " · 计数可能不完整"}`}
            </div>
            {category.sample_paths.length > 0 && <details className="mt-2 text-[10px] text-slate-500">
              <summary className="cursor-pointer select-none hover:text-slate-300">展开最多 {category.sample_paths.length} 个代表路径</summary>
              <ul className="mt-1 max-h-24 space-y-1 overflow-y-auto pl-4">
                {category.sample_paths.map((path) => <li key={path} className="break-all">{path}</li>)}
              </ul>
            </details>}
          </div>)}
        </div>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Btn onClick={clean} disabled={cleaning || !preview.can_execute || previewRegularFiles === 0}>
            <Sparkles size={13} />{cleaning ? "处理中…" : `执行文件清理 · ${previewRegularFiles.toLocaleString()} 个`}
          </Btn>
          {previewHasRecycle && <Btn variant="danger" onClick={emptyRecycleBin} disabled={cleaning || !preview.can_execute}>
            <Trash2 size={13} />单独清空回收站（永久）
          </Btn>}
          <Btn variant="ghost" onClick={() => setPreview(null)} disabled={cleaning}>取消预览</Btn>
        </div>
      </Card>}

      {items === null && !scanning && (
        <Card className="flex flex-col items-center py-14 text-center">
          <Trash2 size={38} className="mb-3 text-slate-600" />
          <div className="mb-1 text-sm text-slate-300">点击「扫描」开始</div>
          <div className="text-xs text-slate-500">要找真正吃空间的大文件？切到「磁盘分析」标签</div>
        </Card>
      )}

      {scanning && (
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="glass h-24 animate-pulse" />
          ))}
        </div>
      )}

      {items !== null && (
        <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
          {items.map((it) => {
            const on = checked.has(it.id);
            const conf = CONF[it.confidence] ?? CONF.low;
            return (
              <button
                key={it.id}
                type="button"
                aria-pressed={on}
                disabled={cleaning || previewing}
                onClick={() => setCategorySelected(it.id, !on)}
                className={`glass flex items-start gap-3 p-4 text-left transition-all hover:bg-white/8 ${
                  on ? "!border-cyan-400/35 bg-cyan-400/6" : "opacity-80"
                }`}
              >
                <div
                  className={`mt-0.5 flex h-4.5 w-4.5 shrink-0 items-center justify-center rounded border transition-colors ${
                    on ? "grad-btn border-transparent" : "border-white/20"
                  }`}
                >
                  {on && <span className="text-[10px] leading-none text-white">✓</span>}
                </div>
                <div className="min-w-0 flex-1">
                  <div className="mb-0.5 flex items-center gap-2">
                    <span className="truncate text-[13px] font-medium text-slate-200">{it.label}</span>
                    <Chip tone={conf.tone}>{conf.label}</Chip>
                    {it.admin_required && (
                      <Chip tone="amber">
                        <ShieldAlert size={10} /> 管理员
                      </Chip>
                    )}
                  </div>
                  <div className="truncate text-[11px] text-slate-500" title={it.path}>
                    {it.path}
                    {it.files > 0 && ` · ${it.files} 个文件`}
                    {it.median_age_days > 0 && ` · 中位年龄 ${it.median_age_days} 天`}
                  </div>
                </div>
                <div className="shrink-0 text-right">
                  <span className={`text-lg font-semibold tabular-nums ${on ? "grad-text" : "text-slate-400"}`}>
                    {it.size_mb >= 1024 ? (it.size_mb / 1024).toFixed(2) + " GB" : it.size_mb.toFixed(1) + " MB"}
                  </span>
                </div>
              </button>
            );
          })}
          {items.length === 0 && (
            <Card className="col-span-2 py-12 text-center text-sm text-slate-400">
              太干净了！没有发现可清理的项目
            </Card>
          )}
        </div>
      )}

      {adminMissing && (
        <div className="mt-4 flex items-center gap-2 rounded-xl border border-amber-400/25 bg-amber-400/8 px-4 py-3 text-[12px] text-amber-200">
          <ShieldAlert size={15} />
          存在需要管理员权限的大体积系统目录，当前未勾选 — 可在「设置」页提权后重新扫描
        </div>
      )}
    </div>
  );
}

/* ================= 磁盘分析 ================= */

function fmtSize(bytes: number): string {
  if (bytes >= 1024 ** 3) return (bytes / 1024 ** 3).toFixed(2) + " GB";
  if (bytes >= 1024 ** 2) return (bytes / 1024 ** 2).toFixed(1) + " MB";
  return (bytes / 1024).toFixed(0) + " KB";
}

function DiskTab() {
  const [scanning, setScanning] = useState(false);
  const [progress, setProgress] = useState<ScanProgress | null>(null);
  const [result, setResult] = useState<DiskScanResult | null>(null);
  const [scanPreset, setScanPreset] = useState<DiskScanPreset | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [canceling, setCanceling] = useState(false);
  const scanPending = useRef(false);
  const cancelPending = useRef(false);

  useEffect(() => {
    let un: (() => void) | undefined;
    let disposed = false;
    api.onDiskScanProgress((p) => { if (!disposed) setProgress(p); })
      .then((u) => { if (disposed) u(); else un = u; }).catch(() => {});
    return () => { disposed = true; un?.(); };
  }, []);

  const startScan = async (preset: DiskScanPreset) => {
    if (!preset || scanPending.current) return;
    scanPending.current = true;
    cancelPending.current = false;
    setScanPreset(preset);
    setScanning(true);
    setResult(null);
    setSelected(new Set());
    try {
      const r = await api.scanDisk(preset);
      setResult(r);
      if (r.cancelled) toast("info", `扫描已停止：已检查 ${r.total_files.toLocaleString()} 个文件 / ${fmtSize(r.total_bytes)}`);
      else toastOk(`扫描完成：${r.total_files.toLocaleString()} 个文件 / ${fmtSize(r.total_bytes)} / ${(r.duration_ms / 1000).toFixed(1)}s`);
    } catch (e) {
      toastErr(e);
    } finally {
      scanPending.current = false;
      setScanning(false);
      setProgress(null);
      cancelPending.current = false;
      setCanceling(false);
    }
  };

  const stop = async () => {
    if (!scanPending.current || cancelPending.current) return;
    cancelPending.current = true;
    setCanceling(true);
    try {
      await api.cancelDiskScan();
      toast("info", "正在停止扫描…");
    } catch (e) {
      cancelPending.current = false;
      setCanceling(false);
      toastErr(e);
    }
  };

  const delSelected = async () => {
    if (selected.size === 0) return;
    if (!result?.delete_token || !scanPreset) {
      toastErr("扫描授权已失效，请重新扫描后再试");
      setSelected(new Set());
      setResult(null);
      if (scanPreset) await startScan(scanPreset);
      return;
    }
    if (!confirm(`把选中的 ${selected.size} 个大文件移入回收站？`)) return;
    const deleteToken = result.delete_token;
    const rootToRescan = scanPreset;
    try {
      await api.recycleDelete(deleteToken, [...selected]);
      toastOk(`已移入回收站 ${selected.size} 个文件`);
      setSelected(new Set());
      await startScan(rootToRescan);
    } catch (e) {
      toastErr(e);
      // The shell recycle operation can partially succeed even when it reports failure.
      setSelected(new Set());
      setResult(null);
      await startScan(rootToRescan);
    }
  };

  return (
    <div>
      <div className="mb-4 flex flex-wrap items-center gap-2">
        <span className="text-[13px] text-slate-400">扫描范围：</span>
        <Btn variant="ghost" onClick={() => startScan("user_profile")} disabled={scanning}>
          用户目录
        </Btn>
        <Btn variant="ghost" onClick={() => startScan("system_drive")} disabled={scanning}>
          整个系统盘
        </Btn>
        {scanning && (
          <Btn variant="danger" onClick={stop} disabled={canceling}>
            停止
          </Btn>
        )}
        {selected.size > 0 && (
          <Btn variant="danger" onClick={delSelected}>
            回收站删除 {selected.size} 项
          </Btn>
        )}
      </div>

      {scanning && progress && (
        <Card className="mb-4">
          <div className="mb-2 flex items-center justify-between text-[12px] text-slate-400">
            <span className="truncate" title={progress.current}>正在扫描：{progress.current}</span>
            <span className="tabular-nums">{fmtSize(progress.bytes)}</span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-white/8">
            <div className="h-full w-1/3 rounded-full grad-btn glow" />
          </div>
        </Card>
      )}
      {scanning && !progress && (
        <div className="mb-4 flex items-center gap-2 text-[13px] text-slate-400">
          <ScanLine size={15} className="animate-pulse text-cyan-300" /> 正在枚举文件…
        </div>
      )}

      {result && (
        <>
          <div className="mb-4 grid grid-cols-2 gap-3 xl:grid-cols-4">
            {[
              { l: "文件总数", v: result.total_files.toLocaleString() },
              { l: "总大小", v: fmtSize(result.total_bytes) },
              { l: "耗时", v: (result.duration_ms / 1000).toFixed(1) + "s" },
              { l: "范围", v: result.root },
            ].map((c) => (
              <div key={c.l} className="glass p-3.5">
                <div className="text-[11px] text-slate-500">{c.l}</div>
                <div className="mt-0.5 truncate text-base font-semibold text-slate-100">{c.v}</div>
              </div>
            ))}
          </div>

          <div className="mb-4">
            <Card title="空间占用排行（二级目录）" icon={<HardDrive size={15} className="text-cyan-300" />}>
              <div className="space-y-1.5">
                {result.dirs.slice(0, 20).map((d) => {
                  const pct = result.total_bytes > 0 ? (d.bytes / result.total_bytes) * 100 : 0;
                  return (
                    <div key={d.path} className="flex items-center gap-3 text-[13px]">
                      <span className="w-40 truncate text-slate-300" title={d.path}>
                        {d.path.replace(result.root, "") || result.root}
                      </span>
                      <div className="h-2.5 flex-1 overflow-hidden rounded-full bg-white/8">
                        <div className="h-full rounded-full grad-btn" style={{ width: `${pct}%` }} />
                      </div>
                      <span className="w-20 text-right tabular-nums text-slate-200">{fmtSize(d.bytes)}</span>
                      <span className="w-16 text-right text-[11px] tabular-nums text-slate-500">{pct.toFixed(1)}%</span>
                    </div>
                  );
                })}
              </div>
            </Card>
          </div>

          <Card
            title="大文件排行（>32MB，勾选可送回收站）"
            icon={<ScanLine size={15} className="text-violet-300" />}
            right={result.truncated ? <Chip tone="slate">仅显示前 60 个</Chip> : undefined}
          >
            <div className="max-h-96 space-y-1 overflow-y-auto pr-1">
              {result.files.map((f) => {
                const on = selected.has(f.path);
                return (
                  <div key={f.path} className="flex items-center gap-1">
                    <button
                      type="button"
                      role="checkbox"
                      aria-checked={on}
                      aria-label={`选择文件 ${f.path}，${fmtSize(f.bytes)}`}
                      onClick={() => {
                        const next = new Set(selected);
                        if (on) next.delete(f.path);
                        else next.add(f.path);
                        setSelected(next);
                      }}
                      className={`flex min-w-0 flex-1 cursor-pointer items-center gap-3 rounded-lg border px-3 py-2 text-left text-[13px] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/60 ${
                        on ? "border-rose-400/30 bg-rose-400/8" : "border-white/6 bg-white/3 hover:bg-white/6"
                      }`}
                    >
                      <span
                        aria-hidden="true"
                        className={`flex h-4 w-4 shrink-0 items-center justify-center rounded border ${
                          on ? "grad-btn border-transparent" : "border-white/20"
                        }`}
                      >
                        {on && <span className="text-[10px] leading-none text-white">✓</span>}
                      </span>
                      <span className="min-w-0 flex-1 truncate text-slate-300" title={f.path}>
                        {f.path}
                      </span>
                      <span className="shrink-0 tabular-nums text-slate-200">{fmtSize(f.bytes)}</span>
                    </button>
                    <button
                      type="button"
                      aria-label={`将 ${f.path} 送回回收站`}
                      onClick={() => {
                        api.recycleDelete(result.delete_token, [f.path]).catch(async (error) => {
                           setSelected(new Set());
                           setResult(null);
                           if (scanPreset) await startScan(scanPreset);
                           throw error;
                         }).then(() => {
                          toastOk("已移入回收站");
                          if (scanPreset) void startScan(scanPreset);
                        }).catch(toastErr);
                      }}
                      className="shrink-0 rounded-md p-1 text-slate-500 hover:bg-rose-500/20 hover:text-rose-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-rose-300/60"
                      title="送回收站"
                    >
                      <PackageX size={14} />
                    </button>
                  </div>
                );
              })}
            </div>
          </Card>
        </>
      )}

      {!result && !scanning && (
        <Card className="flex flex-col items-center py-14 text-center">
          <HardDrive size={38} className="mb-3 text-slate-600" />
          <div className="mb-1 text-sm text-slate-300">选择范围开始扫描</div>
          <div className="text-xs text-slate-500">
            深度二级目录聚合 + 32MB 以上大文件排行 · 扫描整盘可能需要一到几分钟，可随时停止
          </div>
        </Card>
      )}
    </div>
  );
}

/* ================= 系统维护 ================= */

function SystemTab() {
  return (
    <div className="space-y-4">
      <DefragCard />
      <ResidualCard />
      <WinSxsCard />
      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <SmartCard />
        <HibernateCard />
        <UpdateCard />
        <CompactOsCard />
        <MemCompCard />
        <AppxCard />
      </div>
    </div>
  );
}

function DefragCard() {
  const [st, setSt] = useState<DefragStatus | null>(null);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  const statusRequestId = useRef(0);
  const [busyDrive, setBusyDrive] = useState<string | null>(null);
  const [analyzing, setAnalyzing] = useState<string | null>(null);
  const [scheduleBusy, setScheduleBusy] = useState(false);
  const [frag, setFrag] = useState<Record<string, number | null>>({});
  const [schedDay, setSchedDay] = useState("Sunday");
  const [schedTime, setSchedTime] = useState("03:00");

  const load = useCallback(async () => {
    const requestId = ++statusRequestId.current;
    setStatus("loading");
    try {
      const value = await api.defragStatus();
      if (requestId === statusRequestId.current) { setSt(value); setStatus("ready"); }
    } catch {
      if (requestId === statusRequestId.current) setStatus("error");
    }
  }, []);
  useEffect(() => {
    void load();
    return () => { statusRequestId.current += 1; };
  }, [load]);

  const analyze = async (drive: string) => {
    if (status !== "ready") return;
    setAnalyzing(drive);
    try {
      const r = await api.defragAnalyze(drive);
      setFrag((f) => ({ ...f, [drive]: r }));
    } catch (e) {
      toastErr(e);
    }
    setAnalyzing(null);
  };

  const optimize = async (drive: string) => {
    if (status !== "ready") return;
    if (!confirm(`优化 ${drive} ？HDD 将进行碎片整理（可能数十分钟），SSD 执行 TRIM。期间请勿关机。`)) return;
    setBusyDrive(drive);
    try {
      await api.defragOptimize(drive);
      toastOk(`${drive} 优化完成`);
    } catch (e) {
      toastErr(e);
    }
    setBusyDrive(null);
  };

  const setSched = async (enable: boolean) => {
    if (status !== "ready" || scheduleBusy) return;
    setScheduleBusy(true);
    try {
      await api.setDefragSchedule(enable, schedDay, schedTime);
      setSt((current) => current ? { ...current, task_enabled: enable, next_run: "" } : current);
      toastOk(enable ? `每周${schedDay} ${schedTime} 自动优化` : "自动优化已关闭");
    } catch (e) {
      toastErr(e);
    } finally {
      setScheduleBusy(false);
    }
  };

  const DAYS: Record<string, string> = {
    Monday: "周一", Tuesday: "周二", Wednesday: "周三", Thursday: "周四",
    Friday: "周五", Saturday: "周六", Sunday: "周日",
  };

  return (
    <Card
      title="磁盘优化（HDD 碎片整理 / SSD TRIM）"
      icon={<HardDrive size={15} className="text-cyan-300" />}
      right={
        <Btn variant="ghost" onClick={() => void load()} disabled={status === "loading"}>
          <RefreshCw size={13} className={status === "loading" ? "animate-spin" : ""} /> {status === "loading" ? "读取中…" : "刷新"}
        </Btn>
      }
    >
      {status === "loading" && st === null && <div role="status" className="h-20 animate-pulse rounded-lg bg-white/4" />}
      {status === "loading" && st !== null && <div role="status" className="mb-2 text-[11px] text-slate-500">正在刷新磁盘计划，暂示上次成功快照…</div>}
      {status === "error" && (
        <div role="alert" className="mb-3 flex flex-wrap items-start justify-between gap-3 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-xs text-amber-100">
          <span className="min-w-0 flex-1 break-words">{st ? "刷新失败，磁盘信息为上次成功快照。" : "磁盘状态读取失败，优化操作已停用。"}</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
        </div>
      )}
      {st !== null && (
        <>
          <div className="mb-3 space-y-1.5">
            {st.volumes.map((v) => {
              const busy = status !== "ready" || busyDrive === v.drive || analyzing === v.drive;
              const f = frag[v.drive];
              return (
                <div key={v.drive} className="flex items-center gap-3 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2.5 text-[13px]">
                  <HardDrive size={14} className="shrink-0 text-slate-400" />
                  <span className="w-12 font-medium text-slate-200">{v.drive}</span>
                  <Chip tone={v.media === "SSD" ? "cyan" : v.media === "HDD" ? "amber" : "slate"}>
                    {v.media === "SSD" ? "SSD（TRIM）" : v.media === "HDD" ? "HDD（碎片整理）" : v.media}
                  </Chip>
                  <span className="min-w-0 flex-1 truncate text-[11px] text-slate-500">
                    {f == null ? "" : f > 10 ? `碎片化 ${f}% — 建议优化` : f > 0 ? `碎片化 ${f}% — 良好` : "无需整理"}
                  </span>
                  <Btn variant="ghost" disabled={busy} onClick={() => analyze(v.drive)}>
                    {analyzing === v.drive ? "分析中…" : "分析"}
                  </Btn>
                  <Btn disabled={busy} onClick={() => optimize(v.drive)}>
                    {busyDrive === v.drive ? "优化中…" : "立即优化"}
                  </Btn>
                </div>
              );
            })}
          </div>
          <div className="flex flex-wrap items-center gap-3 border-t border-white/6 pt-3 text-[13px]">
            <span className="text-slate-400">定期优化（系统内置 Defrag 计划）：</span>
            <Chip tone={st.task_enabled ? "green" : "slate"}>{st.task_enabled ? "已启用" : "已禁用"}</Chip>
            {st.task_enabled && st.next_run && (
              <span className="text-[11px] text-slate-500">下次：{st.next_run.slice(0, 16).replace("T", " ")}</span>
            )}
            <Toggle
              label="定期优化计划"
              on={st.task_enabled}
              disabled={status !== "ready" || scheduleBusy}
              onChange={(v) => void setSched(v)}
            />
            {!st.task_enabled && (
              <select
                aria-label="定期优化计划星期"
                value={schedDay}
                disabled={status !== "ready" || scheduleBusy}
                onChange={(e) => setSchedDay(e.target.value)}
                className="rounded-md border border-white/10 bg-white/5 px-2 py-1 text-[12px] text-slate-200 outline-none"
              >
                {Object.entries(DAYS).map(([k, cn]) => (
                  <option key={k} value={k}>{cn}</option>
                ))}
              </select>
            )}
            {!st.task_enabled && (
              <input
                aria-label="定期优化计划时间"
                type="time"
                value={schedTime}
                disabled={status !== "ready" || scheduleBusy}
                onChange={(e) => setSchedTime(e.target.value)}
                className="rounded-md border border-white/10 bg-white/5 px-2 py-1 text-[12px] tabular-nums text-slate-200 outline-none"
              />
            )}
            {!st.task_enabled && (
              <Btn onClick={() => void setSched(true)} disabled={status !== "ready" || scheduleBusy}>{scheduleBusy ? "保存中…" : "启用每周计划"}</Btn>
            )}
          </div>
          {st.task_enabled && st.last_run && (
            <div className="mt-1 text-[11px] text-slate-600">上次运行：{st.last_run.slice(0, 16).replace("T", " ")}</div>
          )}
        </>
      )}
    </Card>
  );
}

function ResidualCard() {
  const [items, setItems] = useState<ResidualItem[] | null>(null);
  const [deletingPath, setDeletingPath] = useState<string | null>(null);
  const [scanError, setScanError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const load = useCallback(async () => {
    setScanning(true);
    setScanError(null);
    try {
      setItems(await api.scanResiduals());
      setScanError(null);
    } catch (error) {
      setItems([]);
      setScanError(error instanceof Error ? error.message : String(error));
    } finally {
      setScanning(false);
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);
  const totalGb = (items ?? []).reduce((s, i) => s + (i.size_mb ?? 0), 0) / 1024;
  return (
    <Card
      title="系统升级残留（C 盘大头盲区）"
      icon={<HistoryIcon size={15} className="text-amber-300" />}
      right={
        <div className="flex items-center gap-2">
          {totalGb > 0.1 && <Chip tone="amber">共 {totalGb.toFixed(1)} GB</Chip>}
          <Btn variant="ghost" onClick={load} disabled={scanning || deletingPath !== null}>
            {scanning ? "检测中…" : "重新检测"}
          </Btn>
        </div>
      }
    >
      {items === null && <div className="h-16 animate-pulse rounded-lg bg-white/4" />}
      {items !== null && items.length === 0 && (
        <div className="py-2 text-center text-[12px] text-slate-500" role={scanError ? "alert" : undefined}>
          {scanError ? `扫描失败：${scanError}。可重新检测。` : "未发现升级残留（Windows.old / ESD / 转储文件等）"}
        </div>
      )}
      {items !== null &&
        items.map((r) => (
          <div key={r.path} className="mb-2 flex items-center gap-3 rounded-lg border border-amber-400/20 bg-amber-400/6 px-3.5 py-2.5">
            <div className="min-w-0 flex-1">
              <div className="text-[13px] text-slate-200">
                {r.label} <span className="text-[11px] text-slate-500">· {r.path}</span>
              </div>
              <div className="text-[11px] text-slate-500">{r.desc}</div>
              {r.scan_error && (
                <div className="mt-1 text-[11px] text-rose-300" role="status">
                  安全检查未完成，删除已禁用：{r.scan_error}
                </div>
              )}
            </div>
            <span className="shrink-0 text-base font-semibold tabular-nums text-amber-200">
              {r.size_mb === null
                ? "无法安全估算"
                : r.size_mb >= 1024
                  ? (r.size_mb / 1024).toFixed(1) + " GB"
                  : r.size_mb.toFixed(0) + " MB"}
            </span>
            <Btn
              variant="danger"
              disabled={deletingPath !== null || !r.safe_to_delete}
              title={r.safe_to_delete ? undefined : "未能完整安全检查此目录，删除操作已禁用"}
              onClick={async () => {
                if (deletingPath !== null) return;
                setDeletingPath(r.path);
                try {
                  const removed = await api.removeResidual(r.path);
                  if (!removed) return;
                  toastOk("已删除");
                  await load();
                } catch (error) {
                  toastErr(error);
                } finally {
                  setDeletingPath(null);
                }
              }}
            >
              {deletingPath === r.path
                ? "安全检查/确认/处理中…"
                : r.safe_to_delete
                  ? "删除"
                  : "无法安全检查"}
            </Btn>
          </div>
        ))}
      <div className="mt-2 text-[11px] text-slate-600">
        仅允许删除白名单路径；完整安全扫描失败、发现重解析点或硬链接时会禁用删除。Windows.old 通过 takeown 所有权链移除，删除后无法回到旧系统
      </div>
    </Card>
  );
}

function WinSxsCard() {
  const [busy, setBusy] = useState<"none" | "std" | "deep">("none");
  const run = async (kind: "std" | "deep") => {
    const msg = kind === "deep"
      ? "深度清理将移除所有更新备份（已安装的更新将无法卸载），耗时可能 10-20 分钟。继续？"
      : "标准清理移除被取代的组件版本，耗时数分钟，期间请勿关机。继续？";
    if (!confirm(msg)) return;
    setBusy(kind);
    try {
      if (kind === "deep") await api.dismComponentCleanupReset();
      else await api.dismComponentCleanup();
      toastOk("组件存储清理完成");
    } catch (e) {
      toastErr(e);
    }
    setBusy("none");
  };
  return (
    <Card title="组件存储清理（WinSxS）" icon={<Gauge size={15} className="text-cyan-300" />}>
      <div className="mb-3 text-[13px] text-slate-400">
        Windows 组件存储会随更新不断膨胀（可达 10GB+）。DISM 官方清理可回收数 GB——清理期间请勿关机。
      </div>
      <div className="flex gap-2">
        <Btn variant="ghost" onClick={() => run("std")} disabled={busy !== "none"}>
          {busy === "std" ? "清理中…" : "标准清理"}
        </Btn>
        <Btn variant="danger" onClick={() => run("deep")} disabled={busy !== "none"}>
          {busy === "deep" ? "清理中…" : "深度清理（ResetBase）"}
        </Btn>
      </div>
      <div className="mt-2 text-[11px] text-slate-600">深度清理额外移除更新备份：省空间最大化，但已安装的更新将无法卸载</div>
    </Card>
  );
}

function CompactOsCard() {
  const [on, setOn] = useState<boolean | null>(null);
  const [loadStatus, setLoadStatus] = useState<"loading" | "ready" | "error">("loading");
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    setLoadStatus("loading");
    try { setOn(await api.compactosStatus()); setLoadStatus("ready"); }
    catch { setLoadStatus("error"); }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  return (
    <Card title="CompactOS 系统文件压缩" icon={<HardDrive size={15} className="text-violet-300" />}>
      {loadStatus === "loading" ? (
        <div role="status" className="py-3 text-xs text-slate-500">正在读取 CompactOS 状态…</div>
      ) : loadStatus === "error" ? (
        <div role="alert" className="flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">CompactOS 状态读取失败。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
        </div>
      ) : on === null ? (
        <div role="alert" className="flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">CompactOS 状态未知。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
        </div>
      ) : (
        <>
          <div className="mb-2 text-[13px] text-slate-400">
            {on
              ? "已压缩 — 系统文件占盘更小，读取时有轻微 CPU 解压开销"
              : "未压缩 — 可省约 2GB 系统盘空间"}
          </div>
          <div className="flex items-center justify-between">
            <Chip tone={on ? "cyan" : "slate"}>{on ? "已压缩" : "未压缩"}</Chip>
            <Toggle
              label="CompactOS 系统文件压缩"
              on={on}
              disabled={busy}
              onChange={async (v) => {
                if (busy || !confirm(v ? "开启系统文件压缩？首次执行需要几分钟。" : "关闭系统文件压缩？")) return;
                setBusy(true);
                try {
                  await api.setCompactos(v);
                  setOn(v);
                  toastOk("已切换（首次需几分钟后台处理）");
                } catch (error) {
                  toastErr(error);
                } finally {
                  setBusy(false);
                }
              }}
            />
          </div>
        </>
      )}
    </Card>
  );
}

function MemCompCard() {
  const [on, setOn] = useState<boolean | null>(null);
  const [loadStatus, setLoadStatus] = useState<"loading" | "ready" | "error">("loading");
  const [busy, setBusy] = useState(false);
  const load = useCallback(async () => {
    setLoadStatus("loading");
    try { setOn(await api.memCompressionStatus()); setLoadStatus("ready"); }
    catch { setLoadStatus("error"); }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  return (
    <Card title="内存压缩" icon={<Sparkles size={15} className="text-cyan-300" />}>
      {loadStatus === "loading" ? (
        <div role="status" className="py-3 text-xs text-slate-500">正在读取内存压缩状态…</div>
      ) : loadStatus === "error" ? (
        <div role="alert" className="flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">内存压缩状态读取失败。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
        </div>
      ) : on === null ? (
        <div role="alert" className="flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">内存压缩状态未知。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
        </div>
      ) : (
        <>
          <div className="mb-2 text-[13px] text-slate-400">
            {on
              ? "已开启 — 适合小内存机器（切换需重启生效）"
              : "已关闭 — 大内存机器（32GB+）关闭可省少量 CPU"}
          </div>
          <div className="flex items-center justify-between">
            <Chip tone={on ? "green" : "slate"}>{on ? "开启" : "关闭"}</Chip>
            <Toggle
              label="内存压缩"
              on={on}
              disabled={busy}
              onChange={async (v) => {
                if (busy) return;
                setBusy(true);
                try {
                  await api.setMemCompression(v);
                  setOn(v);
                  toastOk("已切换（重启系统后生效）");
                } catch (error) {
                  toastErr(error);
                } finally {
                  setBusy(false);
                }
              }}
            />
          </div>
        </>
      )}
    </Card>
  );
}

function SmartCard() {
  const [disks, setDisks] = useState<DiskHealth[] | null>(null);
  const [trends, setTrends] = useState<DiskTrendReport | null>(null);
  const load = useCallback(() => {
    api.diskHealth().then(setDisks).catch(() => setDisks([]));
    api.getDiskHealthTrends().then(setTrends).catch(() => setTrends(null));
  }, []);
  useEffect(() => {
    load();
  }, [load]);
  const trendFor = (name: string) => trends?.trends.find((t) => t.name === name);
  const trendTone = (tone: string): "rose" | "amber" | "green" | "slate" =>
    tone === "error-rise" ? "rose" : tone === "wear-moving" ? "amber" : tone === "stable" ? "green" : "slate";
  return (
    <Card
      title="磁盘健康（SMART）"
      icon={<Server size={15} className="text-emerald-300" />}
      right={
        <Btn variant="ghost" onClick={load}>
          刷新
        </Btn>
      }
    >
      {disks === null && <div className="h-16 animate-pulse rounded-lg bg-white/4" />}
      {disks !== null && disks.length === 0 && (
        <div className="text-[12px] text-slate-500">未读取到物理磁盘信息；设备或驱动也可能未公开 SMART 数据。</div>
      )}
      {disks !== null &&
        disks.map((d) => (
          <div key={d.name}>
            <div className="mb-2 flex items-center gap-3 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2.5 text-[13px]">
              <BatteryCharging size={15} className={d.health.toLowerCase() === "healthy" ? "text-emerald-300" : "text-rose-300"} />
              <span className="min-w-0 flex-1 truncate text-slate-200">
                {d.name} <span className="text-[11px] text-slate-500">{d.media} · {d.size_gb} GB</span>
              </span>
              {d.temp != null && d.temp > 0 && <span className="tabular-nums text-slate-400">{d.temp}°C</span>}
              <Chip tone={d.health.toLowerCase() === "healthy" ? "green" : "rose"}>
                {d.health.toLowerCase() === "healthy" ? "健康" : d.health}
              </Chip>
            </div>
            {(d.wear != null || d.power_on_hours != null || d.read_errors != null || d.write_errors != null) && (
              <div className="mb-2 mt-1 flex flex-wrap gap-x-4 gap-y-0.5 pb-1 pl-6 text-[11px] tabular-nums text-slate-500">
                {d.wear != null && <span>磨损 {d.wear.toFixed(0)}%</span>}
                {d.power_on_hours != null && d.power_on_hours > 0 && <span>通电 {(d.power_on_hours / 24).toFixed(0)} 天</span>}
                {d.read_errors != null && d.read_errors > 0 && <span>未纠正读错误 {d.read_errors.toFixed(0)}</span>}
                {d.write_errors != null && d.write_errors > 0 && <span>未纠正写错误 {d.write_errors.toFixed(0)}</span>}
              </div>
            )}
            {(() => {
              const trend = trendFor(d.name);
              if (!trend) return null;
              return (
                <div className="mb-2 ml-6 flex flex-wrap items-center gap-2 rounded-lg border border-white/6 bg-black/15 px-3 py-1.5 text-[11px]">
                  <Chip tone={trendTone(trend.tone)}>
                    {trend.tone === "error-rise" ? "出现新错误" : trend.tone === "wear-moving" ? "磨损上升" : trend.tone === "stable" ? "趋势平稳" : "样本不足"}
                  </Chip>
                  <span className="min-w-0 flex-1 text-slate-400">{trend.summary}</span>
                  {trend.wear_rate_per_1000h != null && (
                    <span className="tabular-nums text-slate-500">磨损速率 {trend.wear_rate_per_1000h.toFixed(2)}%/千小时</span>
                  )}
                  <span className="tabular-nums text-slate-500">
                    {trend.samples} 个样本{trend.span_days != null ? ` · 跨 ${trend.span_days.toFixed(1)} 天` : ""}
                  </span>
                </div>
              );
            })()}
          </div>
        ))}
      {trends && trends.trends.length > 0 && (
        <p className="mt-2 border-t border-white/6 pt-2 text-[10px] leading-relaxed text-slate-600">
          趋势基于本机 AeroTune 运行期间的有限样本（至少 1 小时间隔），不构成寿命预测；SMART 字段可用性取决于硬件与驱动
          {trends.skipped_unchanged > 0 ? `；本轮 ${trends.skipped_unchanged} 块磁盘因距上次记录不足 1 小时未追加样本` : ""}。
          观察提示不自动处置，出现新错误请优先备份重要数据。
        </p>
      )}
    </Card>
  );
}

function HibernateCard() {
  const [info, setInfo] = useState<HibernateInfo | null>(null);
  const load = useCallback(() => api.hibernateInfo().then(setInfo).catch(() => {}), []);
  useEffect(() => {
    load();
  }, [load]);
  return (
    <Card title="休眠文件（hiberfil.sys）" icon={<BatteryCharging size={15} className="text-cyan-300" />}>
      {info ? (
        <>
          <div className="mb-2 text-[13px] text-slate-400">
            {info.enabled
              ? <>当前占用 <b className="text-slate-100">{info.size_gb.toFixed(1)} GB</b> — 关闭休眠立即释放（代价：失去休眠/快速启动）</>
              : "休眠已关闭，未占用空间"}
          </div>
          <div className="flex items-center justify-between">
            <span className="text-[12px] text-slate-500">{info.supported ? "" : "此平台可能不支持休眠"}</span>
            <Toggle label="休眠功能" on={info.enabled} onChange={(v) => {
              if (!confirm(v ? "开启休眠？将重新生成 hiberfil.sys。" : `关闭休眠？将释放 ${info.size_gb.toFixed(1)} GB 空间，且无法使用休眠/快速启动。`)) return;
              api.setHibernate(v).then(() => { toastOk("已切换"); load(); }).catch(toastErr);
            }} />
          </div>
        </>
      ) : (
        <div className="h-10 animate-pulse rounded-lg bg-white/4" />
      )}
    </Card>
  );
}

function UpdateCard() {
  const [paused, setPaused] = useState(false);
  useEffect(() => {
    api.getUpdatePause().then(setPaused).catch(() => {});
  }, []);
  return (
    <Card title="Windows 更新控制" icon={<MonitorCog size={15} className="text-violet-300" />}>
      <div className="flex items-center justify-between">
        <div className="pr-4 text-[13px] text-slate-400">
          暂停自动更新（策略键，可还原）— 防止后台下载抢占带宽与磁盘；手动检查更新不受影响
        </div>
        <Toggle
          label="暂停 Windows 自动更新"
          on={paused}
          onChange={(v) => {
            api.setUpdatePause(v)
              .then(() => {
                setPaused(v);
                toastOk(v ? "已暂停自动更新" : "已恢复自动更新");
              })
              .catch(toastErr);
          }}
        />
      </div>
    </Card>
  );
}

function AppxCard() {
  const [apps, setApps] = useState<AppxApp[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [loading, setLoading] = useState(false);
  const [listLoaded, setListLoaded] = useState(false);
  const [busyPackage, setBusyPackage] = useState<string | null>(null);
  const load = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      setApps(await api.listAppx());
      setListLoaded(true);
    } catch (error) {
      setApps((current) => current ?? []);
      setListLoaded(true);
      setLoadError(String(error));
    } finally {
      setLoading(false);
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  const search = query.trim().toLocaleLowerCase();
  const filtered = useMemo(
    () => (apps ?? []).filter((a) => !search || [a.name, a.package_name, a.full_name, a.publisher, a.install_location].some((value) => value.toLocaleLowerCase().includes(search))),
    [apps, search]
  );
  const uninstall = async (app: AppxApp) => {
    if (busyPackage) return;
    if (!confirm(`卸载「${app.name}」？此操作针对当前 Windows 用户；若应用支持，可从 Microsoft Store 重新安装。\n\n包：${app.full_name}`)) return;
    setBusyPackage(app.full_name);
    try {
      await api.removeAppx(app.full_name);
      toastOk(`已卸载 ${app.name}`);
      await load();
    } catch (error) {
      toastErr(error);
    } finally {
      setBusyPackage(null);
    }
  };
  return (
    <Card
      className="xl:col-span-2 min-w-0"
      title="预装应用管理（UWP）"
      icon={<PackageX size={15} className="text-rose-300" />}
      right={
        <Btn variant="ghost" onClick={() => void load()} disabled={loading}>
          <RefreshCw size={14} className={loading ? "animate-spin" : ""} /> {loading ? "读取中" : "刷新列表"}
        </Btn>
      }
    >
      <div className="mb-3 text-[12px] leading-5 text-slate-400">
        显示名称优先取开始菜单名称或应用清单；系统资源名不可读时以安装目录推断。卸载仅影响当前用户，应用是否可从商店恢复取决于发行方。系统签名组件保留展示并禁止卸载。
      </div>
      <label className="mb-3 flex min-w-0 items-center gap-2 rounded-lg border border-white/10 bg-white/5 px-3 py-2 focus-within:border-cyan-400/40">
        <Search size={14} className="shrink-0 text-slate-500" />
        <input
          aria-label="搜索已安装应用名称、包名、发布者或路径"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="搜索显示名称、包名、发布者或安装路径…"
          className="min-w-0 flex-1 bg-transparent text-[13px] text-slate-200 outline-none placeholder:text-slate-600"
        />
        {apps !== null && <span className="shrink-0 text-[11px] tabular-nums text-slate-500">{filtered.length}/{apps.length}</span>}
      </label>
      {loadError && <div role="alert" className="mb-3 rounded-lg border border-rose-400/20 bg-rose-400/5 px-3 py-2 text-[12px] text-rose-200">读取失败：{loadError}</div>}
      {(!listLoaded || loading) && <div className="h-24 animate-pulse rounded-lg bg-white/4" />}
      {listLoaded && !loading && apps !== null && !loadError && apps.length === 0 && (
        <div className="rounded-lg border border-white/6 bg-white/3 px-4 py-5 text-center">
          <PackageX size={20} className="mx-auto mb-2 text-slate-600" />
          <div className="text-[13px] text-slate-300">当前用户没有可管理的预装应用</div>
          <div className="mt-1 text-[11px] text-slate-500">框架包已隐藏；受保护的系统应用会保留在列表中并标记</div>
        </div>
      )}
      {listLoaded && !loading && !loadError && apps !== null && filtered.length > 0 && (
        <div className="max-h-[min(32rem,55vh)] space-y-2 overflow-y-auto overscroll-contain pr-1">
          {filtered.map((app) => {
            const identityNameDiffers = app.package_name.toLocaleLowerCase() !== app.name.toLocaleLowerCase();
            return (
              <div key={app.full_name} className="flex min-w-0 flex-wrap items-start gap-3 rounded-xl border border-white/6 bg-white/3 px-3.5 py-3 sm:flex-nowrap">
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px] font-medium text-slate-100" title={app.name}>{app.name}</div>
                  {identityNameDiffers && <div className="mt-0.5 break-all text-[11px] text-slate-400">包名：{app.package_name}</div>}
                  {app.protected && <div className="mt-1 inline-flex rounded bg-amber-400/10 px-1.5 py-0.5 text-[10px] text-amber-200">系统组件 · 仅查看</div>}
                  <div className="mt-1 flex min-w-0 items-start gap-1.5 text-[11px] text-slate-500">
                    <FolderOpen size={12} className="mt-0.5 shrink-0" />
                    <span className="min-w-0 break-all" title={app.install_location}>{app.install_location || "Windows 未返回安装位置"}</span>
                  </div>
                  {app.install_location && <div className="mt-1 pl-[18px] text-[10px] text-slate-600">目录：{app.install_location.split(/[\\/]/).filter(Boolean).at(-1)}</div>}
                  <div className="mt-1 flex flex-wrap gap-x-3 gap-y-0.5 text-[10px] text-slate-600">
                    {app.publisher && <span className="break-all">{app.publisher}</span>}
                    {app.version && <span>版本 {app.version}</span>}
                  </div>
                  <details className="mt-1.5 text-[10px] text-slate-600">
                    <summary className="flex w-fit cursor-pointer select-none items-center gap-1 hover:text-slate-400"><ChevronDown size={11} /> 包标识</summary>
                    <div className="mt-1 break-all font-mono">{app.full_name}</div>
                  </details>
                </div>
                <Btn
                  variant="danger"
                  disabled={app.protected || busyPackage !== null}
                  onClick={() => void uninstall(app)}
                  className="ml-auto shrink-0"
                >
                  {app.protected ? "系统保护" : busyPackage === app.full_name ? "卸载中…" : "卸载"}
                </Btn>
              </div>
            );
          })}
        </div>
      )}
      {listLoaded && !loading && apps !== null && !loadError && apps.length > 0 && filtered.length === 0 && (
        <div className="py-5 text-center text-[12px] text-slate-500">没有匹配的应用；试试包名、发布者或路径关键词。</div>
      )}
    </Card>
  );
}
