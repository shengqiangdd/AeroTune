// 服务管理：非微软服务优先展示，启动类型四档切换（自动延迟/自动/手动/禁用）
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Server, RefreshCw, Search, ShieldAlert, Download } from "lucide-react";
import { api, SvcInfo } from "../lib/api";
import { Btn, PageHead, Chip } from "../components/ui";
import { toastOk, toastErr } from "../lib/toast";

const MODE_LABEL: Record<string, string> = {
  auto: "自动",
  delayed: "自动（延迟）",
  manual: "手动",
  disabled: "禁用",
};

function currentMode(s: SvcInfo): string {
  const start = s.start_reg;
  if (start === 4) return "disabled";
  if (start === 3) return "manual";
  if (start === 2) return s.delayed === 1 ? "delayed" : "auto";
  return s.start_mode.toLowerCase().includes("auto") ? "auto" : "manual";
}

export function Services() {
  const [list, setList] = useState<SvcInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [query, setQuery] = useState("");
  const [modifiableOnly, setModifiableOnly] = useState(true);
  const [stateFilter, setStateFilter] = useState<"all" | "running" | "stopped">("all");
  const [startupFilter, setStartupFilter] = useState("all");
  const [exporting, setExporting] = useState(false);
  const [busyService, setBusyService] = useState<string | null>(null);
  const loadingRef = useRef(false);
  const busyRef = useRef(false);
  const loadFailedRef = useRef(false);
  const mountedRef = useRef(true);

  const load = useCallback(async (notifyFailure = true) => {
    if (!mountedRef.current || loadingRef.current || busyRef.current) return;
    loadingRef.current = true;
    setLoading(true);
    try {
      const freshList = await api.listServices();
      if (mountedRef.current) {
        setList(freshList);
        loadFailedRef.current = false;
        setLoadFailed(false);
      }
    } catch (e) {
      if (mountedRef.current) {
        loadFailedRef.current = true;
        setLoadFailed(true);
        if (notifyFailure) toastErr(e);
      }
    } finally {
      loadingRef.current = false;
      if (mountedRef.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    void load();
    return () => {
      mountedRef.current = false;
    };
  }, [load]);

  const filtered = useMemo(
    () =>
      list
        .filter((s) => (modifiableOnly ? s.modifiable : true))
        .filter((s) => stateFilter === "all" || (stateFilter === "running" ? s.state === "Running" : s.state !== "Running"))
        .filter((s) => startupFilter === "all" || currentMode(s) === startupFilter)
        .filter(
          (s) =>
            !query.trim() ||
            s.name.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()) ||
            s.display.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()) ||
            s.account.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase())
        ),
    [list, modifiableOnly, query, stateFilter, startupFilter]
  );

  const startupModes = useMemo(() => [...new Set(list.map(currentMode))].sort(), [list]);

  const exportCsv = async () => {
    if (exporting || loadingRef.current || loadFailedRef.current || filtered.length === 0) return;
    setExporting(true);
    try {
      const cell = (value: string) => {
        const formulaText = /^[\s\u0000-\u001F]*[=+@\-]/.test(value) ? `'${value}` : value;
        const safe = formulaText.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "");
        return `"${safe.replace(/"/g, '""')}"`;
      };
      const rows = [
        ["显示名称", "服务名", "状态", "启动类型", "可调整", "运行账户", "可执行文件"],
        ...filtered.map((s) => [s.display, s.name, s.state === "Running" ? "运行中" : "已停止", MODE_LABEL[currentMode(s)] ?? currentMode(s), s.modifiable ? "是" : "否", s.account, s.path]),
      ];
      const csv = "\uFEFF" + rows.map((row) => row.map(cell).join(",")).join("\r\n");
      const filename = `AeroTune-services-${new Date().toISOString().replace(/[:.]/g, "-")}.csv`;
      const saved = await api.saveTextExport(filename, csv);
      if (!saved) return;
      toastOk(`已导出 ${filtered.length} 个筛选服务到所选文件`);
    } catch (e) {
      toastErr(e);
    } finally {
      setExporting(false);
    }
  };

  const change = async (s: SvcInfo, mode: string) => {
    if (busyRef.current || loadingRef.current || loadFailedRef.current) return;
    busyRef.current = true;
    setBusyService(s.name);
    let mutationFailed = false;
    try {
      await api.setServiceStartup(s.name, mode);
      toastOk(`「${s.name}」启动类型 → ${MODE_LABEL[mode] ?? mode}（可在变更日志还原）`);
    } catch (e) {
      mutationFailed = true;
      toastErr(e);
    } finally {
      busyRef.current = false;
      if (mountedRef.current) setBusyService(null);
      await load(!mutationFailed);
    }
  };

  const modifiableCount = list.filter((s) => s.modifiable).length;

  return (
    <div>
      <PageHead
        title="服务管理"
        desc={loading
          ? list.length > 0 ? `正在刷新服务列表；现有 ${list.length} 项暂只读` : "正在读取服务列表…"
          : loadFailed
            ? list.length > 0
              ? `上次读取快照 ${list.length} 个服务（只读） · 刷新成功后恢复操作`
              : "服务列表读取失败 · 刷新成功前不可修改"
            : `可调整服务 ${modifiableCount} 个 · 停用闲置服务可减少开机初始化与后台资源占用`}
        right={
          <>
            <Chip tone={modifiableOnly ? "cyan" : "slate"}>
              <button onClick={() => setModifiableOnly(!modifiableOnly)}>
                {modifiableOnly ? "仅可调整" : "全部服务"}
              </button>
            </Chip>
            <Btn variant="ghost" onClick={() => void load()} disabled={loading || busyService !== null}>
              <RefreshCw size={14} className={loading ? "animate-spin" : ""} /> {loading ? "刷新中…" : "刷新"}
            </Btn>
          </>
        }
      />

      {loadFailed && (
        <div role="alert" className="mb-3 flex flex-wrap items-start justify-between gap-3 rounded-lg border border-amber-400/20 bg-amber-400/5 p-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">
            服务列表读取失败；{list.length > 0 ? `仍显示上次读取的 ${list.length} 项快照。` : "目前没有可用列表。"}
            服务修改与 CSV 导出已暂停，请重试读取。
          </span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()} disabled={loading || busyService !== null}>
            {loading ? "重试中…" : "重试"}
          </Btn>
        </div>
      )}
      {loading && list.length > 0 && (
        <div role="status" className="mb-3 text-xs text-slate-400">正在刷新服务列表；当前快照暂不可修改。</div>
      )}

      <div className="mb-3 flex min-w-0 flex-wrap items-center gap-2">
        <div className="glass flex min-w-0 flex-1 basis-full items-center gap-2 px-4 py-2.5 focus-within:ring-2 focus-within:ring-cyan-300/70 sm:basis-64">
          <Search size={15} className="text-slate-500" />
          <input
            aria-label="搜索服务名称"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="搜索服务名称…"
            className="w-full bg-transparent text-sm text-slate-200 outline-none placeholder:text-slate-600"
          />
        </div>
        <select aria-label="按服务状态筛选" value={stateFilter} onChange={(e) => setStateFilter(e.target.value as "all" | "running" | "stopped")} className="rounded-lg border border-white/10 bg-[#171923] px-2.5 py-2 text-xs text-slate-300 outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70">
          <option value="all">全部状态</option><option value="running">仅运行中</option><option value="stopped">仅已停止</option>
        </select>
        <select aria-label="按服务启动类型筛选" value={startupFilter} onChange={(e) => setStartupFilter(e.target.value)} className="rounded-lg border border-white/10 bg-[#171923] px-2.5 py-2 text-xs text-slate-300 outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70">
          <option value="all">全部启动类型</option>{startupModes.map((mode) => <option key={mode} value={mode}>{MODE_LABEL[mode] ?? mode}</option>)}
        </select>
        <Btn variant="ghost" onClick={exportCsv} disabled={exporting || loading || loadFailed || filtered.length === 0}><Download size={14} /> 导出 CSV</Btn>
        <Chip tone="amber">
          <ShieldAlert size={10} /> 修改需管理员
        </Chip>
      </div>

      <div className="glass overflow-hidden">
        <div className="max-h-[calc(100vh-300px)] overflow-auto overscroll-x-contain">
          <table aria-label="服务列表" className="w-full min-w-[680px] table-fixed text-left text-[13px] sm:min-w-[900px] sm:table-auto">
            <thead className="sticky top-0 bg-[#111827]/95 backdrop-blur">
              <tr className="border-b border-white/8">
                <th scope="col" className="w-[48%] px-2 py-2.5 text-[11px] text-slate-400 sm:w-auto sm:px-4">服务</th>
                <th scope="col" className="hidden px-4 py-2.5 text-[11px] text-slate-400 sm:table-cell">可执行文件</th>
                <th scope="col" className="w-[20%] px-2 py-2.5 text-[11px] text-slate-400 sm:w-24 sm:px-4">状态</th>
                <th scope="col" className="w-[32%] px-2 py-2.5 text-right text-[11px] text-slate-400 sm:w-44 sm:px-4">启动类型</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((s) => (
                <tr key={s.name} className="border-b border-white/4 hover:bg-white/4">
                  <td className="max-w-72 truncate px-2 py-2.5 sm:px-4">
                    <div className="truncate font-medium text-slate-200" title={s.name}>
                      {s.display}
                    </div>
                    <div className="truncate text-[11px] text-slate-500" title={`${s.name} · ${s.account || "未知账户"}`}>
                      {s.name} · 账户 {s.account || "未知"}
                      {!s.modifiable && <span className="ml-1.5 text-slate-600">· 只读</span>}
                    </div>
                  </td>
                  <td className="hidden max-w-80 truncate px-4 py-2.5 text-[11px] text-slate-500 sm:table-cell" title={s.path}>
                    {s.path || "—"}
                  </td>
                  <td className="px-2 py-2.5 sm:px-4">
                    <Chip className="whitespace-nowrap" tone={s.state === "Running" ? "green" : "slate"}>
                      {s.state === "Running" ? "运行中" : "已停止"}
                    </Chip>
                  </td>
                  <td className="px-2 py-2.5 text-right sm:px-4">
                    <select
                      aria-label={`${s.display} 启动类型`}
                      value={currentMode(s)}
                      disabled={loading || loadFailed || busyService !== null || !s.modifiable}
                      title={loadFailed ? "服务列表读取失败，当前数据仅供查看；刷新成功后可修改" : loading ? "服务列表刷新中，当前数据暂不可修改" : s.modifiable ? undefined : "服务路径或 ACL 未通过验证、运行账户未审查，或属于系统关键服务"}
                      onChange={(e) => change(s, e.target.value)}
                      className="rounded-md border border-white/10 bg-white/5 px-2 py-1.5 text-[12px] text-slate-200 outline-none hover:bg-white/10 focus-visible:ring-2 focus-visible:ring-cyan-300/70"
                    >
                      {Object.entries(MODE_LABEL).map(([k, v]) => (
                        <option key={k} value={k}>
                          {v}
                        </option>
                      ))}
                    </select>
                  </td>
                </tr>
              ))}
              {filtered.length === 0 && (
                <tr>
                  <td colSpan={4} role={loading && list.length === 0 ? "status" : undefined} className="px-4 py-12 text-center text-sm text-slate-500">
                    <Server className="mx-auto mb-2 opacity-40" />
                    {loadFailed
                      ? list.length > 0 ? "上次读取的服务快照中没有匹配项。" : "服务列表读取失败；请使用上方按钮重试。"
                      : loading && list.length === 0 ? "正在读取服务列表…" : "没有匹配的服务"}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
      <div className="mt-3 text-[11px] text-slate-600">
        {loadFailed
          ? list.length > 0
            ? `旧快照：${list.length} 个服务 · 显示 ${filtered.length} 个（只读）`
            : "服务列表读取失败（无可用快照）"
          : `共 ${list.length} 个服务 · 显示 ${filtered.length} 个`}
        {" · "}仅允许受审查的内置/虚拟账户，修改/还原时由后端再次核验 SCM 账户；可执行文件与父目录 ACL 也须通过检查；系统关键服务拒绝修改 ·
        发布者签名尚未核验，ACL 检查与服务配置修改也非原子操作；每次修改均记录在「设置 → 变更日志」可还原
      </div>
    </div>
  );
}
