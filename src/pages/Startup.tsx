// 开机全景：开机耗时 + 启动项 + 计划任务 + 延迟服务，一张画像看懂开机
import { useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { Rocket, RefreshCw, FolderOpen, Timer, Server, CalendarClock, Search, Download } from "lucide-react";
import { api, StartupItem, BootRecord, StartupImpact, TaskInfo } from "../lib/api";
import { Btn, PageHead, Chip, Toggle } from "../components/ui";
import { toastOk, toastErr } from "../lib/toast";
import { fmtTime } from "../lib/format";
import { matchStartupImpactPathToItem } from "../lib/startupImpactMatch.js";
import { buildStartupEvidenceByItem, startupEvidenceExportLabel } from "../lib/startupEvidence.js";
import { filterStartupTasks, paginateStartupTasks, STARTUP_TASK_PAGE_SIZE } from "../lib/startupTaskPagination.js";
import { startupDelayRecoveryViewState } from "../lib/startupDelayRecoveryView.js";
import { startupTrustLabel } from "../lib/startupTrust.js";

const LVL: Record<number, { label: string; tone: "rose" | "amber" | "cyan" | "green" }> = {
  1: { label: "严重拖慢", tone: "rose" },
  2: { label: "异常", tone: "rose" },
  3: { label: "偏慢", tone: "amber" },
  4: { label: "正常", tone: "green" },
};

type ImpactLookupNotice = {
  impactKey: string;
  kind: "exact" | "ambiguous" | "fallback";
  message: string;
  candidates: StartupItem[];
};

export function Startup({ onNav }: { onNav: (p: "services") => void }) {
  const [items, setItems] = useState<StartupItem[]>([]);
  const [loading, setLoading] = useState(false);
  const [startupStatus, setStartupStatus] = useState<"loading" | "ready" | "error">("loading");
  const [startupLoadError, setStartupLoadError] = useState<string | null>(null);
  const startupRequestId = useRef(0);
  const [delays, setDelays] = useState<{ name: string; task: string }[] | null>(null);
  const [delaysStatus, setDelaysStatus] = useState<"loading" | "ready" | "error">("loading");
  const [delaysError, setDelaysError] = useState<string | null>(null);
  const delayedStartupRequestId = useRef(0);
  const [restoringDelayTask, setRestoringDelayTask] = useState<string | null>(null);
  const delayRecoveryView = startupDelayRecoveryViewState(delaysStatus, delays);
  const [boots, setBoots] = useState<BootRecord[] | null>(null);
  const [bootLoading, setBootLoading] = useState(false);
  const [impacts, setImpacts] = useState<StartupImpact[] | null>(null);
  const [impactLoading, setImpactLoading] = useState(false);
  const [tasks, setTasks] = useState<TaskInfo[] | null>(null);
  const [taskPage, setTaskPage] = useState(0);
  const [taskQuery, setTaskQuery] = useState("");
  const taskSearchRef = useRef<HTMLInputElement>(null);
  const [tasksStatus, setTasksStatus] = useState<"loading" | "ready" | "error">("loading");
  const tasksRequestId = useRef(0);
  const [busyTaskId, setBusyTaskId] = useState<string | null>(null);
  const [delayedSvcs, setDelayedSvcs] = useState<number | null>(null);
  const [startupQuery, setStartupQuery] = useState("");
  const [startupEnabledFilter, setStartupEnabledFilter] = useState<"all" | "enabled" | "disabled">("all");
  const [startupSourceFilter, setStartupSourceFilter] = useState("all");
  const [focusedStartupItemId, setFocusedStartupItemId] = useState<string | null>(null);
  const [impactLookupNotice, setImpactLookupNotice] = useState<ImpactLookupNotice | null>(null);
  const [exportingStartup, setExportingStartup] = useState(false);

  const loadDelayedStartups = useCallback(async () => {
    const requestId = ++delayedStartupRequestId.current;
    setDelaysStatus("loading");
    setDelaysError(null);
    try {
      const result = await api.listDelayedStartup();
      if (requestId === delayedStartupRequestId.current) {
        setDelays(result);
        setDelaysStatus("ready");
      }
    } catch (error) {
      if (requestId === delayedStartupRequestId.current) {
        setDelaysStatus("error");
        setDelaysError(error instanceof Error ? error.message : String(error));
      }
    }
  }, []);

  const load = useCallback(async () => {
    const requestId = ++startupRequestId.current;
    setLoading(true);
    setStartupStatus("loading");
    setStartupLoadError(null);
    try {
      const [startupResult] = await Promise.allSettled([
        api.listStartup(),
        loadDelayedStartups(),
      ] as const);
      if (requestId !== startupRequestId.current) return;
      if (startupResult.status === "fulfilled") {
        setItems(startupResult.value);
        setStartupStatus("ready");
      } else {
        const error = startupResult.reason instanceof Error ? startupResult.reason.message : String(startupResult.reason);
        setStartupStatus("error");
        setStartupLoadError(error);
        toastErr(startupResult.reason);
      }
    } catch (error) {
      if (requestId === startupRequestId.current) {
        setStartupStatus("error");
        setStartupLoadError(error instanceof Error ? error.message : String(error));
        toastErr(error);
      }
    } finally {
      if (requestId === startupRequestId.current) setLoading(false);
    }
  }, [loadDelayedStartups]);

  useEffect(() => {
    load();
  }, [load]);

  const loadBoots = useCallback(async () => {
    setBootLoading(true);
    try {
      setBoots(await api.bootHistory());
    } catch {
      setBoots([]);
    }
    setBootLoading(false);
  }, []);

  const loadImpacts = useCallback(async () => {
    setImpactLoading(true);
    try { setImpacts(await api.startupImpacts()); }
    catch { setImpacts([]); }
    finally { setImpactLoading(false); }
  }, []);

  const loadTasks = useCallback(async () => {
    const requestId = ++tasksRequestId.current;
    setTasksStatus("loading");
    try {
      const result = await api.listLogonTasks();
      if (requestId === tasksRequestId.current) { setTasks(result); setTasksStatus("ready"); }
    } catch {
      if (requestId === tasksRequestId.current) setTasksStatus("error");
    }
  }, []);

  const restoreDelayedStartup = async (delay: { name: string; task: string }) => {
    if (delaysStatus !== "ready" || restoringDelayTask !== null) return;
    setRestoringDelayTask(delay.task);
    try {
      await api.undelayStartupItem(delay.name);
      toastOk(`「${delay.name}」已恢复直接启动`);
    } catch (error) {
      toastErr(error);
    } finally {
      try {
        await load();
        await loadTasks();
      } finally {
        setRestoringDelayTask(null);
      }
    }
  };

  const loadDelayed = useCallback(async () => {
    try {
      const svcs = await api.listServices();
      setDelayedSvcs(svcs.filter((s) => s.start_reg === 2).length);
    } catch {
      setDelayedSvcs(null);
    }
  }, []);

  useEffect(() => {
    loadBoots();
    loadImpacts();
    loadTasks();
    loadDelayed();
  }, [loadBoots, loadImpacts, loadTasks, loadDelayed]);

  const toggleTask = async (t: TaskInfo) => {
    if (busyTaskId || tasksStatus !== "ready") return;
    const taskId = `${t.path}${t.name}`;
    setBusyTaskId(taskId);
    const enabling = t.state === "Disabled";
    try {
      await api.setTaskState(t.path, t.name, enabling);
      setTasks((current) => current?.map((item) =>
        item.path === t.path && item.name === t.name
          ? { ...item, state: enabling ? "Ready" : "Disabled" }
          : item,
      ) ?? null);
      toastOk(enabling ? `已启用计划任务「${t.name}」` : `已禁用计划任务「${t.name}」`);
    } catch (e) {
      toastErr(e);
    } finally {
      setBusyTaskId(null);
    }
  };

  const enabledItems = useMemo(
    () => items.reduce((count, item) => count + (item.enabled ? 1 : 0), 0),
    [items],
  );
  const logonTasks = useMemo(
    () => (tasks ?? []).reduce((count, task) => count + (task.state !== "Disabled" && (task.logon || task.boot) ? 1 : 0), 0),
    [tasks],
  );
  const sortedTasks = useMemo(
    () => tasks?.slice().sort((a, b) => Number(b.logon || b.boot) - Number(a.logon || a.boot) || a.name.localeCompare(b.name)) ?? null,
    [tasks],
  );
  const deferredTaskQuery = useDeferredValue(taskQuery);
  const matchingTasks = useMemo(
    () => filterStartupTasks(sortedTasks ?? [], deferredTaskQuery),
    [sortedTasks, deferredTaskQuery],
  );
  const taskPageResult = useMemo(
    () => paginateStartupTasks(matchingTasks, taskPage, STARTUP_TASK_PAGE_SIZE),
    [matchingTasks, taskPage],
  );

  const toggle = async (item: StartupItem) => {
    if (startupStatus !== "ready" || loading) return;
    try {
      await api.toggleStartup(item.id, !item.enabled);
      toastOk(item.enabled ? `已禁用「${item.name}」` : `已启用「${item.name}」`);
      load();
    } catch (e) {
      toastErr(e);
    }
  };

  const startupSources = useMemo(() => [...new Set(items.map((item) => item.source))].sort((a, b) => a.localeCompare(b, "zh-CN")), [items]);
  const filteredStartupItems = useMemo(() => {
    if (focusedStartupItemId) return items.filter((item) => item.id === focusedStartupItemId);
    const q = startupQuery.trim().toLocaleLowerCase();
    return items.filter((item) => {
      if (startupEnabledFilter === "enabled" && !item.enabled) return false;
      if (startupEnabledFilter === "disabled" && item.enabled) return false;
      if (startupSourceFilter !== "all" && item.source !== startupSourceFilter) return false;
      return !q || `${item.name} ${item.cmd} ${item.source}`.toLocaleLowerCase().includes(q);
    }).sort((a, b) => Number(b.enabled) - Number(a.enabled) || a.name.localeCompare(b.name, "zh-CN"));
  }, [items, startupQuery, startupEnabledFilter, startupSourceFilter, focusedStartupItemId]);
  const startupEvidenceById = useMemo(
    () => buildStartupEvidenceByItem(items, impacts ?? []),
    [items, impacts],
  );

  useEffect(() => {
    if (!focusedStartupItemId) return;
    const timer = window.setTimeout(() => {
      document.getElementById(`startup-item-${encodeURIComponent(focusedStartupItemId)}`)?.scrollIntoView({ behavior: "smooth", block: "center" });
    }, 60);
    return () => window.clearTimeout(timer);
  }, [focusedStartupItemId, filteredStartupItems]);

  const focusStartupItem = (item: StartupItem, impactKey: string) => {
    setStartupQuery("");
    setStartupEnabledFilter("all");
    setStartupSourceFilter("all");
    setFocusedStartupItemId(item.id);
    setImpactLookupNotice({ impactKey, kind: "exact", message: `已按完整路径定位到「${item.name}」`, candidates: [] });
  };

  const locateFromImpact = (impact: StartupImpact, impactKey: string) => {
    const match = matchStartupImpactPathToItem(impact.path, items);
    setStartupEnabledFilter("all");
    setStartupSourceFilter("all");
    if (match.kind === "exact") {
      focusStartupItem(match.item, impactKey);
      return;
    }
    setFocusedStartupItemId(null);
    setStartupQuery(impact.name);
    setImpactLookupNotice(match.kind === "ambiguous"
      ? { impactKey, kind: "ambiguous", message: "完整路径匹配到多个启动项，请选择对应条目；未按名称自动猜测。", candidates: match.items }
      : { impactKey, kind: "fallback", message: "没有找到完整路径匹配，已按应用名称搜索。", candidates: [] });
  };

  const exportStartupCsv = async () => {
    if (exportingStartup || loading || startupStatus !== "ready" || filteredStartupItems.length === 0) return;
    setExportingStartup(true);
    try {
      const cell = (value: string | number) => {
        const text = String(value);
        // Neutralize formulas even when a cell starts with whitespace/control characters.
        const formulaText = /^[\s\u0000-\u001F]*[=+@\-]/.test(text) ? `'${text}` : text;
        const safe = formulaText.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "");
        return `"${safe.replace(/"/g, '""')}"`;
      };
      const rows = [
        ["名称", "状态", "来源", "命令 / 位置", "签名上下文", "证据提示"],
        ...filteredStartupItems.map((item) => [
          item.name,
          item.enabled ? "已启用" : "已禁用",
          item.source,
          item.cmd,
          startupTrustLabel(item.signature_status, item.signer_subject),
          startupEvidenceExportLabel(startupEvidenceById.get(item.id)),
        ]),
      ];
      const csv = "\uFEFF" + rows.map((row) => row.map(cell).join(",")).join("\r\n");
      const filename = `AeroTune-startup-${new Date().toISOString().replace(/[:.]/g, "-")}.csv`;
      const saved = await api.saveTextExport(filename, csv);
      if (!saved) return;
      toastOk(`已导出 ${filteredStartupItems.length} 条筛选后的启动项到所选文件`);
    } catch (e) {
      toastErr(e);
    } finally {
      setExportingStartup(false);
    }
  };

  return (
    <div>
      <PageHead
        title="开机全景"
        desc="启动项 × 计划任务 × 延迟服务 × 实测开机耗时 —— 区分系统记录的影响与名称启发式线索，不把线索当作实测结论"
        right={
          <Btn variant="ghost" onClick={load} disabled={loading}>
            <RefreshCw size={14} className={loading ? "animate-spin" : ""} /> 刷新
          </Btn>
        }
      />

      {/* 开机画像汇总 */}
      <div className="mb-5 grid grid-cols-1 gap-3 sm:grid-cols-2 2xl:grid-cols-4">
        {(
          [
            { label: "登录启动项", val: `${enabledItems}/${items.length}`, sub: "已启用/总数", to: undefined as undefined },
            { label: "登录计划任务", val: tasksStatus === "ready" ? `${logonTasks}` : "—", sub: tasksStatus === "error" ? "状态读取失败" : "第三方就绪中", to: undefined as undefined },
            { label: "自动/延迟服务", val: delayedSvcs == null ? "…" : `${delayedSvcs}`, sub: "系统全部服务", to: ("services" as const) },
            { label: "最近开机", val: boots?.[0] ? `${boots[0].total_secs.toFixed(1)}s` : "…", sub: "总耗时", to: undefined as undefined },
          ] as const
        ).map((c) => (
            <button
              key={c.label}
              onClick={c.to ? () => onNav(c.to) : undefined}
              className="glass p-4 text-left transition-all hover:bg-white/8"
            >
            <div className="text-[11px] text-slate-500">{c.label}</div>
            <div className="mt-1 text-xl font-semibold tabular-nums text-slate-100">{c.val}</div>
            <div className="text-[10px] text-slate-600">{c.sub}</div>
          </button>
        ))}
      </div>

      {/* 开机耗时分析 */}
      <div className="mb-5">
        <div className="glass p-5">
          <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
            <div className="flex min-w-0 items-center gap-2 text-sm font-medium text-slate-300">
              <Timer size={15} className="shrink-0 text-cyan-300" />
              <span>开机耗时分析（Windows 诊断事件，最近 5 次）</span>
            </div>
            <Btn variant="ghost" onClick={loadBoots} disabled={bootLoading}>
              <RefreshCw size={14} className={bootLoading ? "animate-spin" : ""} /> 重新分析
            </Btn>
          </div>
          {bootLoading && <div className="h-20 animate-pulse rounded-lg bg-white/4" />}
          {!bootLoading && boots !== null && boots.length === 0 && (
            <div className="py-3 text-[12px] text-slate-500">
              暂无开机诊断事件（事件日志可能被清理，或系统刚装好还没记录足够次数）
            </div>
          )}
          {!bootLoading && boots !== null && boots.length > 0 && (
            <div className="space-y-2">
              {boots.map((b, i) => {
                const meta = LVL[b.level] ?? LVL[4];
                const pct = Math.min(100, (b.total_secs / 90) * 100);
                return (
                  <div key={i} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-[13px] sm:flex-nowrap">
                    <span className="w-24 shrink-0 tabular-nums text-slate-500">{fmtTime(b.ts)}</span>
                    <div className="flex min-w-0 flex-1 items-center gap-2">
                      <div className="flex h-3.5 flex-1 overflow-hidden rounded-full bg-white/8">
                        <div
                          className="h-full bg-gradient-to-r from-cyan-400 to-violet-500"
                          style={{ width: `${(b.main_secs / Math.max(b.total_secs, 1)) * pct}%` }}
                          title={`主路径 ${b.main_secs}s`}
                        />
                        <div
                          className="h-full bg-gradient-to-r from-violet-500 to-rose-400"
                          style={{ width: `${(b.post_secs / Math.max(b.total_secs, 1)) * pct}%` }}
                          title={`后开机 ${b.post_secs}s（登录后就绪）`}
                        />
                      </div>
                      <span className="w-16 shrink-0 text-right font-medium tabular-nums text-slate-200">
                        {b.total_secs.toFixed(1)}s
                      </span>
                    </div>
                    <Chip tone={meta.tone}>{meta.label}</Chip>
                  </div>
                );
              })}
              <div className="pt-1 text-[11px] text-slate-600">
                主路径 = 到登录界面；后开机 = 登录到系统就绪（含启动项初始化）— 禁用启动项主要压缩后者
              </div>
            </div>
          )}
        </div>
      </div>

      <div className="mb-5">
        <div className="glass p-5">
          <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2 text-sm font-medium text-slate-300"><Timer size={15} className="text-amber-300" />Windows 已记录的应用启动影响（事件 101）</div>
            <Btn variant="ghost" onClick={loadImpacts} disabled={impactLoading}><RefreshCw size={14} className={impactLoading ? "animate-spin" : ""} />重新分析</Btn>
          </div>
          {impactLoading && <div className="py-4 text-xs text-slate-500">正在读取 Windows 诊断事件…</div>}
          {!impactLoading && impacts?.length === 0 && <div className="py-3 text-xs text-slate-500">暂无可关联的应用影响事件；日志可能未记录、已清理，或该应用启动未达到系统诊断条件。</div>}
          {!impactLoading && impacts && impacts.length > 0 && <div className="space-y-2">
            {impacts.slice(0, 8).map((impact, index) => {
              const impactKey = `${impact.ts}-${impact.name}-${index}`;
              const notice = impactLookupNotice?.impactKey === impactKey ? impactLookupNotice : null;
              return <div key={impactKey} className="rounded-xl border border-white/6 bg-black/10 px-3.5 py-3">
                <div className="flex flex-wrap items-center justify-between gap-2"><div className="min-w-0 truncate text-sm font-medium text-slate-200" title={impact.path || impact.name}>{impact.name}</div><div className="text-xs tabular-nums text-amber-200">系统记录影响 {impact.degradation_ms.toFixed(0)} ms · 启动耗时 {impact.total_ms.toFixed(0)} ms</div></div>
                <div className="mt-1 flex flex-wrap items-center justify-between gap-2 text-[10px] text-slate-500"><span className="max-w-full truncate" title={impact.path}>{impact.path || "未提供应用路径"}</span><span>{impact.ts ? new Date(impact.ts * 1000).toLocaleString() : "时间未知"}</span></div>
                <button onClick={() => locateFromImpact(impact, impactKey)} className="mt-2 text-[11px] text-cyan-300 hover:text-cyan-200">{impact.path ? "按完整路径定位启动项" : "按名称查找启动项"}</button>
                {notice && <div className={`mt-2 rounded-md border px-2.5 py-2 text-[11px] ${notice.kind === "ambiguous" ? "border-amber-400/20 bg-amber-400/5 text-amber-200" : "border-cyan-400/15 bg-cyan-400/5 text-cyan-200"}`}>
                  {notice.message}
                  {notice.candidates.length > 0 && <div className="mt-2 space-y-1">{notice.candidates.map((candidate) => <button key={candidate.id} onClick={() => focusStartupItem(candidate, impactKey)} className="block w-full truncate rounded px-2 py-1 text-left text-slate-300 hover:bg-white/8" title={candidate.cmd}>{candidate.name} · {candidate.source} · {candidate.cmd}</button>)}</div>}
                </div>}
              </div>;
            })}
            <div className="text-[11px] leading-5 text-slate-500">这些数值来自 Windows Diagnostics‑Performance 事件，仅显示系统实际记录的样本，不等于每次开机的完整归因；未出现事件时不推断启动项影响。</div>
          </div>}
        </div>
      </div>

      {/* 计划任务（第三方 登录/开机 触发） */}
      <div className="mb-5">
        <div className="glass p-5">
          <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
            <div className="flex items-center gap-2 text-sm font-medium text-slate-300">
              <CalendarClock size={15} className="text-violet-300" />
              计划任务（第三方 · 登录/开机触发优先）
            </div>
            <Btn variant="ghost" onClick={() => void loadTasks()} disabled={tasksStatus === "loading"}>
              <RefreshCw size={13} className={tasksStatus === "loading" ? "animate-spin" : ""} /> {tasksStatus === "loading" ? "读取中…" : "刷新"}
            </Btn>
          </div>
          {tasksStatus === "loading" && tasks === null && <div role="status" className="h-16 animate-pulse rounded-lg bg-white/4" />}
          {tasksStatus === "loading" && tasks !== null && <div role="status" className="mb-2 text-[11px] text-slate-500">正在更新列表，暂示上次成功读取结果…</div>}
          {tasksStatus === "error" && (
            <div role="alert" className="mb-2 flex flex-wrap items-start justify-between gap-3 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-xs text-amber-100">
              <span className="min-w-0 flex-1 break-words">{tasks ? "读取失败，当前显示上次成功结果。" : "计划任务读取失败，无法确认列表是否为空。"}</span>
              <Btn variant="ghost" className="shrink-0" onClick={() => void loadTasks()}>重试</Btn>
            </div>
          )}
          {sortedTasks !== null && sortedTasks.length > 0 && (
            <div className="mb-3 flex flex-wrap items-center gap-2">
              <div className="flex min-w-0 flex-1 basis-full items-center gap-2 rounded-lg border border-white/8 bg-white/3 px-3 py-2 sm:basis-72">
                <Search size={14} className="shrink-0 text-slate-500" />
                <input
                  ref={taskSearchRef}
                  type="search"
                  aria-label="搜索计划任务名称、路径、执行命令或发布者"
                  value={taskQuery}
                  onChange={(event) => { setTaskQuery(event.target.value); setTaskPage(0); }}
                  placeholder="搜索名称、路径、命令或发布者…"
                  className="min-w-0 flex-1 bg-transparent text-xs text-slate-200 outline-none placeholder:text-slate-600"
                />
                {taskQuery && (
                  <button
                    type="button"
                    aria-label="清除计划任务搜索"
                    onClick={() => { setTaskQuery(""); setTaskPage(0); taskSearchRef.current?.focus(); }}
                    className="shrink-0 rounded px-1 text-[11px] text-cyan-300 hover:text-cyan-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-cyan-300"
                  >
                    清除
                  </button>
                )}
              </div>
              <span className="text-[11px] text-slate-500">匹配 {matchingTasks.length} / {sortedTasks.length} 个计划任务 · 空格可组合关键词</span>
            </div>
          )}
          {tasksStatus === "ready" && tasks?.length === 0 && (
            <div className="py-3 text-center text-[12px] text-slate-500">没有发现第三方计划任务（Microsoft 自带任务已过滤）</div>
          )}
          {sortedTasks !== null && sortedTasks.length > 0 && matchingTasks.length === 0 && (
            <div role="status" aria-live="polite" className="py-3 text-center text-[12px] text-slate-500">没有匹配的计划任务；可尝试名称、路径、执行命令或发布者，或清除搜索。</div>
          )}
          {matchingTasks.length > 0 && (
            <div key={taskPageResult.page} className="max-h-72 space-y-1.5 overflow-y-auto pr-1">
              {taskPageResult.items.map((t) => (
                  <div key={t.path + t.name} className="flex min-w-0 flex-wrap items-start gap-3 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2.5 sm:flex-nowrap">
                    <div className="min-w-0 flex-1">
                      <div className="flex min-w-0 flex-wrap items-center gap-2">
                        <span className="max-w-full truncate text-[13px] font-medium text-slate-200">{t.name}</span>
                        {(t.logon || t.boot) && <Chip tone="cyan">开机触发</Chip>}
                        <Chip tone={t.state === "Disabled" ? "rose" : t.state === "Running" ? "green" : "slate"}>
                          {t.state === "Ready" ? "就绪" : t.state === "Disabled" ? "已禁用" : t.state === "Running" ? "运行中" : t.state}
                        </Chip>
                      </div>
                      <div className="truncate text-[11px] text-slate-500" title={t.path + t.action}>
                        {t.path}{t.name} {t.action && `· ${t.action}`}
                      </div>
                    </div>
                    {busyTaskId === `${t.path}${t.name}` && <span role="status" className="text-[11px] text-cyan-200">处理中…</span>}
                    <Toggle label={`计划任务：${t.name}`} on={t.state !== "Disabled"} disabled={tasksStatus !== "ready" || busyTaskId !== null} onChange={() => void toggleTask(t)} />
                  </div>
                ))}
            </div>
          )}
          {matchingTasks.length > STARTUP_TASK_PAGE_SIZE && (
            <nav aria-label="计划任务分页" className="mt-3 flex flex-wrap items-center justify-center gap-3">
              <Btn variant="ghost" onClick={() => setTaskPage(taskPageResult.page - 1)} disabled={taskPageResult.page === 0 || tasksStatus === "loading"}>
                上一页
              </Btn>
              <span role="status" aria-live="polite" className="text-[11px] text-slate-400">
                第 {taskPageResult.page + 1} / {taskPageResult.pageCount} 页 · 显示 {taskPageResult.items.length} / {matchingTasks.length} 个匹配项（总计 {sortedTasks?.length ?? 0}）
              </span>
              <Btn variant="ghost" onClick={() => setTaskPage(taskPageResult.page + 1)} disabled={taskPageResult.page + 1 >= taskPageResult.pageCount || tasksStatus === "loading"}>
                下一页
              </Btn>
            </nav>
          )}
          {sortedTasks !== null && sortedTasks.length > 0 && (
            <div className="mt-2 text-[11px] text-slate-600">
              触发器类型用 CIM 类名判定，不受系统语言影响；禁用不影响任务定义，可随时在此恢复
            </div>
          )}
        </div>
      </div>

      <div className="glass mb-5 p-4">
        <div className="mb-2 flex flex-wrap items-start justify-between gap-3">
          <div>
            <div className="text-sm text-slate-300">延迟启动恢复（持久备份，不支持设置页日志撤销）</div>
            <div className="mt-1 text-[12px] text-slate-500">仅转换绝对本地 exe、无参数的 REG_SZ；其他命令明确拒绝。失败记录也显示在此，可重试恢复。旧版无备份任务需人工核对。</div>
          </div>
          <Btn variant="ghost" onClick={() => void loadDelayedStartups()} disabled={delayRecoveryView.loading || restoringDelayTask !== null}>
            <RefreshCw size={13} className={delayRecoveryView.loading ? "animate-spin" : ""} />
            {delayRecoveryView.loading ? "读取中…" : delayRecoveryView.error ? "重试读取" : "刷新恢复记录"}
          </Btn>
        </div>
        {delayRecoveryView.loading && !delayRecoveryView.hasSnapshot && (
          <div role="status" className="py-2 text-[12px] text-slate-500">正在读取延迟启动恢复记录…</div>
        )}
        {delayRecoveryView.refreshingSnapshot && (
          <div role="status" className="mb-2 text-[11px] text-slate-500">正在刷新恢复记录；当前仅显示上次读取结果，恢复操作暂不可用。</div>
        )}
        {delayRecoveryView.error && (
          <div role="alert" className="mb-2 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-xs leading-5 text-amber-100">
            {delays === null
              ? "无法读取恢复记录，因此不能确认是否为空；不会显示“暂无恢复记录”。"
              : delays.length === 0
                ? "本次读取失败；上次成功结果为空，但当前恢复记录状态无法确认。"
                : "本次读取失败；下方记录仅为上次成功读取的快照。"}
            {` 恢复操作已禁用。${delaysError ? ` 读取错误：${delaysError}` : ""}`}
          </div>
        )}
        {delayRecoveryView.showRecords && delays?.map((delay) => (
          <div key={delay.task} className="flex items-center justify-between py-2">
            <span className="text-[13px] text-slate-300">{delay.name}</span>
            <Btn
              variant="ghost"
              disabled={!delayRecoveryView.canRestore || restoringDelayTask !== null}
              onClick={() => void restoreDelayedStartup(delay)}
            >
              {restoringDelayTask === delay.task ? "恢复中…" : "恢复直接启动"}
            </Btn>
          </div>
        ))}
        {delayRecoveryView.showEmpty && <div role="status" className="text-[12px] text-slate-500">暂无延迟启动恢复记录</div>}
      </div>

      <div className="glass overflow-hidden">
        <div className="sticky top-0 z-10 flex flex-wrap items-center gap-2 border-b border-white/8 bg-[#11131c]/95 p-3 backdrop-blur">
          <div className="flex min-w-0 flex-1 basis-full items-center gap-2 rounded-lg border border-white/8 bg-white/3 px-3 py-2 sm:basis-48">
            <Search size={14} className="text-slate-500" />
            <input aria-label="搜索启动项名称、命令或来源" value={startupQuery} onChange={(e) => { setFocusedStartupItemId(null); setImpactLookupNotice(null); setStartupQuery(e.target.value); }} placeholder="搜索名称、命令或来源…" className="min-w-0 flex-1 bg-transparent text-xs text-slate-200 outline-none placeholder:text-slate-600" />
          </div>
          <select aria-label="按启动项状态筛选" value={startupEnabledFilter} onChange={(e) => { setFocusedStartupItemId(null); setImpactLookupNotice(null); setStartupEnabledFilter(e.target.value as "all" | "enabled" | "disabled"); }} className="rounded-lg border border-white/10 bg-[#171923] px-2.5 py-2 text-xs text-slate-300 outline-none">
            <option value="all">全部状态</option><option value="enabled">仅已启用</option><option value="disabled">仅已禁用</option>
          </select>
          <select aria-label="按启动项来源筛选" value={startupSourceFilter} onChange={(e) => { setFocusedStartupItemId(null); setImpactLookupNotice(null); setStartupSourceFilter(e.target.value); }} className="max-w-44 rounded-lg border border-white/10 bg-[#171923] px-2.5 py-2 text-xs text-slate-300 outline-none">
            <option value="all">全部来源</option>{startupSources.map((source) => <option key={source} value={source}>{source}</option>)}
          </select>
          <Btn variant="ghost" onClick={exportStartupCsv} disabled={exportingStartup || loading || startupStatus !== "ready" || filteredStartupItems.length === 0}>
            <Download size={14} /> 导出 CSV
          </Btn>
          <span className="text-[11px] text-slate-500">{filteredStartupItems.length}/{items.length}</span>
          {focusedStartupItemId && <button type="button" onClick={() => { setFocusedStartupItemId(null); setImpactLookupNotice(null); }} className="text-[11px] text-cyan-300 hover:text-cyan-200">清除定位</button>}
        </div>
        {startupStatus === "loading" && (
          <div role="status" className="mx-3 mb-2 rounded-lg border border-white/8 bg-white/3 px-3 py-2 text-[11px] text-slate-400">
            {items.length === 0 ? "正在读取启动项与签名上下文…" : "正在刷新；当前列表是上次快照，启动项开关、延迟和导出暂不可用。"}
          </div>
        )}
        {startupStatus === "error" && (
          <div role="alert" className="mx-3 mb-2 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-[11px] leading-5 text-amber-100">
            <span className="min-w-0 flex-1 break-words">
              {items.length === 0 ? "无法读取启动项清单，不能确认当前是否为空。" : "当前仅显示上次成功读取的启动项快照。"}
              {` 开关、延迟和导出已禁用。${startupLoadError ? ` 读取错误：${startupLoadError}` : ""}`}
            </span>
            <Btn variant="ghost" onClick={() => void load()} disabled={loading}>重试读取</Btn>
          </div>
        )}
        <div className="mb-2 space-y-1 text-[11px] leading-5 text-slate-500" role="note">
          <p>本表仅覆盖当前注册表视图的 HKCU/HKLM Run、用户/所有用户启动文件夹中的 .lnk/.exe/.bat 项及 AeroTune 自己禁用的项目；计划任务在下方单列（其“作者”字段不是已验证发布者），服务、Winlogon 等不在本表内，因此不是完整启动清单。</p>
          <p>信任检查仅对无参数、直接解析的绝对本地 EXE 执行；每次最多筛选 32 个候选、对 8 个新目标执行 WinVerifyTrust，结果缓存 60 秒。快捷方式、脚本、环境变量、带参数或歧义路径、不可访问目标及超出限额均显示“未验证”；预算耗尽或临时验证失败的项目可点“刷新”重试。检查使用本机 WinVerifyTrust，不做证书撤销在线检查；只查内嵌签名，目录签名可能显示“无内嵌签名”。有效签名不代表软件安全，未知也不代表安全。</p>
          <p>“Windows 实测”仅在事件 101 完整路径唯一匹配时显示；名称启发式只是线索，非实测；没有事件记录不代表没有影响。</p>
        </div>
        <div className="overflow-x-auto">
        <table aria-label="启动项列表" className="w-full min-w-[760px] text-left text-[13px]">
          <thead>
            <tr className="border-b border-white/8">
              <th scope="col" className="px-4 py-2.5 text-[11px] text-slate-400">名称</th>
              <th scope="col" className="px-4 py-2.5 text-[11px] text-slate-400">命令 / 位置</th>
              <th scope="col" className="px-4 py-2.5 text-[11px] text-slate-400">来源</th>
              <th scope="col" className="px-4 py-2.5 text-right text-[11px] text-slate-400">开机启动</th>
            </tr>
          </thead>
          <tbody>
            {filteredStartupItems.map((it) => {
              const evidence = startupEvidenceById.get(it.id);
              const latestImpact = evidence?.measuredImpacts[0];
              const signatureLabel = startupTrustLabel(it.signature_status, it.signer_subject);
              const signatureTone = it.signature_status === "signed" ? "cyan" : it.signature_status === "unsigned" ? "amber" : "slate";
              return <tr key={it.id} id={`startup-item-${encodeURIComponent(it.id)}`} className={`border-b border-white/4 ${focusedStartupItemId === it.id ? "bg-cyan-400/8 ring-1 ring-inset ring-cyan-300/25" : "hover:bg-white/4"}`} aria-current={focusedStartupItemId === it.id ? "location" : undefined}>
                <td className="px-4 py-2.5">
                  <span className="font-medium text-slate-200">{it.name}</span>
                  {!it.enabled && (
                    <span className="ml-2 text-[10px] text-slate-500">(已禁用)</span>
                  )}
                  {(latestImpact || (it.enabled && evidence?.heuristic)) && (
                    <div className="mt-1 flex flex-wrap gap-1">
                      {latestImpact && <Chip tone="amber">Windows 事件 101 实测 · 影响 {latestImpact.degradation_ms.toFixed(0)} ms</Chip>}
                      {it.enabled && evidence?.heuristic && <Chip tone="slate">名称启发式（非实测）· {evidence.heuristic}</Chip>}
                    </div>
                  )}
                </td>
                <td className="max-w-105 truncate px-4 py-2.5 text-[12px] text-slate-500" title={it.cmd}>
                  {it.cmd || "—"}
                </td>
                <td className="px-4 py-2.5">
                  <div className="flex flex-col items-start gap-1">
                    <Chip tone={it.enabled ? (it.source.includes("HKLM") ? "amber" : "slate") : "rose"}>
                      {it.source}
                    </Chip>
                    <div className={`max-w-64 break-words text-[10px] leading-4 ${signatureTone === "cyan" ? "text-cyan-200" : signatureTone === "amber" ? "text-amber-200" : "text-slate-500"}`} title={signatureLabel}>
                      {signatureLabel}
                    </div>
                  </div>
                </td>
                <td className="px-4 py-2.5 text-right">
                  <div className="flex items-center justify-end gap-2">
                    {it.enabled && it.id.startsWith("reg|HKCU|") && (
                      <button
                        className="rounded-lg border border-white/10 bg-white/4 px-2.5 py-1.5 text-[12px] text-slate-300 transition-all hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-50"
                        title="创建登录后 60 秒延迟启动的计划任务（可逆）"
                        disabled={startupStatus !== "ready" || loading}
                        onClick={async () => {
                          if (startupStatus !== "ready" || loading) return;
                          try {
                            await api.delayStartupItem(it.name, 60);
                            toastOk(`「${it.name}」已转为登录后 60 秒延迟启动`);
                          } catch (e) {
                            toastErr(e);
                          } finally {
                            await load();
                            await loadTasks();
                          }
                        }}
                      >
                        延迟 60s
                      </button>
                    )}
                    {it.enabled && (
                      <button
                        className="rounded-md p-1.5 text-slate-500 hover:bg-white/10 hover:text-cyan-300 disabled:cursor-not-allowed disabled:opacity-50"
                        title="在文件夹中显示"
                        disabled={startupStatus !== "ready" || loading}
                        onClick={() => {
                          const path = it.id.startsWith("file|") ? it.cmd : it.cmd.split('"')[1] || it.cmd;
                          void api.openPath(path).catch(toastErr);
                        }}
                      >
                        <FolderOpen size={14} />
                      </button>
                    )}
                    <Toggle label={`启动项：${it.name}`} on={it.enabled} onChange={() => toggle(it)} disabled={startupStatus !== "ready" || loading} />
                  </div>
                </td>
              </tr>;
            })}
            {filteredStartupItems.length === 0 && (
              <tr>
                <td colSpan={4} className="px-4 py-12 text-center text-sm text-slate-500">
                  <Rocket className="mx-auto mb-2 opacity-40" />
                  {startupStatus === "ready" ? (items.length === 0 ? "没有发现启动项" : "没有符合条件的启动项") : startupStatus === "error" ? "无法确认启动项或筛选结果；请重试读取" : "正在刷新启动项清单…"}
                </td>
              </tr>
            )}
          </tbody>
        </table>
        </div>
      </div>
      <div className="mt-3 text-[11px] text-slate-600">
        HKLM / 所有用户来源的启动项需要管理员权限；禁用的项保存在 AeroTune 备份中，可随时重新启用。
      </div>
    </div>
  );
}
