// 进程管理：搜索 / 排序 / CSV 导出 / 结束 / 优先级 / 挂起恢复
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Search, RefreshCw, XCircle, Pause, Play, Gauge, MemoryStick, Download, Zap, ListTree, Radar } from "lucide-react";
import { api, MemoryCandidate, MemoryCandidateIdentity, MemoryOptimizationReport, ProcInfo, ProcessInsights, ResidencySuggestion } from "../lib/api";
import { formatWindowsFiletimeUtc } from "../lib/processCreationTime.js";
import { canMutateProcessRows, nextProcessSnapshotStatus } from "../lib/processSnapshotStatus.js";
import { mergeMemoryCandidateResults } from "../lib/memoryCandidateResults.js";
import { aggregateMemoryOptimizationBatches } from "../lib/memoryOptimizationBatch.js";
import { interactionLatencyBandFromP95, processRefreshIntervalMs } from "../lib/processRefreshPolicy";
import { buildProcessTreeRows } from "../lib/processTreeRows";
import { recordInteractionLatencySample } from "../lib/interactionLatencySampling";
import { nearestRankPercentile } from "../lib/nearestRankPercentile";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Btn, PageHead, Chip, Card } from "../components/ui";
import { toastOk, toastErr } from "../lib/toast";
import { ProcessHistoryCard } from "../components/ProcessHistoryCard";

type SortKey = "cpu" | "mem_mb" | "io_mb_s" | "name" | "pid";
type CpuLoadBand = "unknown" | "normal" | "elevated" | "high";
type InteractionLatencyBand = "unknown" | "normal" | "elevated" | "high";

const PRIO_LABELS = ["空闲", "低于正常", "正常", "高于正常", "高", "实时"];
const MAX_PROCESS_ROWS = 300;
const PROCESS_ROW_OVERSCAN = 5;
const PROCESS_HEADER_HEIGHT_PX = 36;

function formatAvailableMemory(value: number | null): string {
  return value == null || !Number.isFinite(value) ? "未测量" : `${value.toFixed(0)} MB`;
}

/** 画像角色中值得在进程表里一眼识别的子集；系统组件与普通用户应用不加徽章避免噪音。 */
const NOTABLE_ROLE_META: Record<string, { label: string; tone: "rose" | "violet" | "amber" }> = {
  "updater-suspect": { label: "更新器嫌疑", tone: "rose" },
  autostart: { label: "自启动", tone: "violet" },
  unknown: { label: "未知来源", tone: "amber" },
};

function insightRoleTone(roleId: string): "slate" | "cyan" | "violet" | "amber" | "rose" | "green" {
  if (roleId === "updater-suspect") return "rose";
  if (roleId === "autostart") return "violet";
  if (roleId === "unknown") return "amber";
  if (roleId === "user-app") return "cyan";
  return "slate";
}

function insightSignatureMeta(signature: string): { label: string; tone: "green" | "amber" | "slate" } {
  if (signature === "signed") return { label: "已签名", tone: "green" };
  if (signature === "unsigned") return { label: "无内嵌签名", tone: "amber" };
  return { label: "未校验", tone: "slate" };
}

function insightAgeLabel(secs: number): string {
  if (secs < 60) return `${secs} 秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} 小时 ${Math.floor((secs % 3600) / 60)} 分`;
  return `${Math.floor(secs / 86400)} 天`;
}

export function Processes({ focusPid = null, cpuLoadBand = "unknown", memoryLoadBand = "unknown" }: { focusPid?: number | null; cpuLoadBand?: CpuLoadBand; memoryLoadBand?: CpuLoadBand }) {
  const [list, setList] = useState<ProcInfo[]>([]);
  const [query, setQuery] = useState(focusPid == null ? "" : String(focusPid));
  const [sortKey, setSortKey] = useState<SortKey>("cpu");
  const [desc, setDesc] = useState(true);
  const [treeView, setTreeView] = useState(false);
  const [loading, setLoading] = useState(false);
  const [snapshotStatus, setSnapshotStatus] = useState({ hasSnapshot: false, stale: false });
  const [auto, setAuto] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [memoryReport, setMemoryReport] = useState<MemoryOptimizationReport | null>(null);
  const [memoryOperationWarning, setMemoryOperationWarning] = useState<string | null>(null);
  const [memoryBusy, setMemoryBusy] = useState(false);
  const [selectedMemoryPids, setSelectedMemoryPids] = useState<number[]>([]);
  const [measureSearchLatency, setMeasureSearchLatency] = useState(false);
  const [searchLatencyP95Ms, setSearchLatencyP95Ms] = useState<number | null>(null);
  const [searchLatencySampleCount, setSearchLatencySampleCount] = useState(0);
  const [interactionLatencyBand, setInteractionLatencyBand] = useState<InteractionLatencyBand>("unknown");
  const inputInteractionStartedAt = useRef<number | null>(null);
  const inputLatencySamples = useRef<number[]>([]);
  const inputLatencySampleCount = useRef(0);
  const processTableScrollRef = useRef<HTMLDivElement | null>(null);
  const firstVirtualRowRef = useRef<HTMLTableRowElement | null>(null);
  const [tableScrollTop, setTableScrollTop] = useState(0);
  const [tableViewportHeight, setTableViewportHeight] = useState(480);
  const [processRowHeight, setProcessRowHeight] = useState(40);
  const [insights, setInsights] = useState<ProcessInsights | null>(null);
  const insightsPending = useRef(false);
  const initialProcessListLoaded = useRef(false);
  const requestInitialInsights = useRef<(() => void) | null>(null);
  const [residency, setResidency] = useState<ResidencySuggestion[]>([]);
  const residencyPending = useRef(false);
  const memoryBusyRef = useRef(false);
  const timer = useRef<ReturnType<typeof setInterval> | null>(null);
  const mounted = useRef(false);
  const pending = useRef(false);
  const loadAgain = useRef(false);
  const snapshotStaleRef = useRef(false);
  const processMutationsDisabled = !canMutateProcessRows(snapshotStatus, loading);
  const visibilityPending = useRef(false);
  const markProcessInteraction = useCallback(() => {
    if (measureSearchLatency) inputInteractionStartedAt.current = performance.now();
    const table = processTableScrollRef.current;
    if (table && table.scrollTop !== 0) {
      table.scrollTop = 0;
      setTableScrollTop(0);
    }
  }, [measureSearchLatency]);

  const load = useCallback(async () => {
    // Coalesce manual/automatic refreshes while one process snapshot is in flight.
    if (pending.current) {
      loadAgain.current = true;
      return;
    }
    pending.current = true;
    if (mounted.current) setLoading(true);
    try {
      const processes = await api.listProcesses();
      if (mounted.current) {
        const recovered = snapshotStaleRef.current;
        setList(processes);
        setSnapshotStatus((current) => nextProcessSnapshotStatus(current, true));
        if (recovered) toastOk("进程列表已刷新，进程操作已重新启用。");
        snapshotStaleRef.current = false;
      }
      if (!initialProcessListLoaded.current) {
        initialProcessListLoaded.current = true;
        requestInitialInsights.current?.();
      }
    } catch (e) {
      if (mounted.current) {
        setSnapshotStatus((current) => nextProcessSnapshotStatus(current, false));
        if (!snapshotStaleRef.current) toastErr(e);
        snapshotStaleRef.current = true;
      }
    } finally {
      pending.current = false;
      if (mounted.current) setLoading(false);
      if (loadAgain.current && mounted.current) {
        loadAgain.current = false;
        queueMicrotask(() => { if (mounted.current) void load(); });
      } else {
        loadAgain.current = false;
      }
    }
  }, []);

  const pollIfVisible = useCallback(async () => {
    if (!mounted.current || document.hidden || visibilityPending.current) return;
    visibilityPending.current = true;
    try {
      const visible = await getCurrentWindow().isVisible();
      if (visible && mounted.current && !document.hidden) await load();
    } catch {
      // Native visibility lookup can fail during shutdown; browser visibility is a fallback.
      if (mounted.current && !document.hidden) await load();
    } finally {
      visibilityPending.current = false;
    }
  }, [load]);

  useEffect(() => {
    mounted.current = true;
    load();
    return () => {
      mounted.current = false;
      if (timer.current) clearInterval(timer.current);
    };
  }, [load]);

  // 后台常驻洞察：首份进程列表成功后才启动；之后独立只读轮询，隐藏时暂停、防重入。
  useEffect(() => {
    let disposed = false;
    const refreshInsights = async () => {
      if (!initialProcessListLoaded.current || insightsPending.current || disposed || document.hidden) return;
      insightsPending.current = true;
      try {
        const value = await api.getProcessInsights();
        if (!disposed) setInsights(value);
      } catch {
        // 洞察失败保持上次快照，不干扰主表格。
      } finally {
        insightsPending.current = false;
      }
    };
    const requestRefresh = () => { void refreshInsights(); };
    requestInitialInsights.current = requestRefresh;
    void refreshInsights();
    const insightsTimer = setInterval(() => { if (!document.hidden) void refreshInsights(); }, 30000);
    // 常驻模式学习建议：低频拉取（数据本身 30 分钟才累积一次），失败保持静默。
    const refreshResidency = async () => {
      if (residencyPending.current || disposed || document.hidden) return;
      residencyPending.current = true;
      try {
        setResidency(await api.getResidencySuggestions());
      } catch {
        // 建议是锦上添花，失败保持上次快照
      } finally {
        residencyPending.current = false;
      }
    };
    void refreshResidency();
    const residencyTimer = setInterval(() => { if (!document.hidden) void refreshResidency(); }, 120000);
    return () => {
      disposed = true;
      if (requestInitialInsights.current === requestRefresh) requestInitialInsights.current = null;
      clearInterval(insightsTimer);
      clearInterval(residencyTimer);
    };
  }, []);

  useEffect(() => {
    setInteractionLatencyBand((current) => {
      const next = measureSearchLatency
        ? interactionLatencyBandFromP95(searchLatencyP95Ms, searchLatencySampleCount, current)
        : "unknown";
      return next === current ? current : next;
    });
  }, [measureSearchLatency, searchLatencyP95Ms, searchLatencySampleCount]);

  const autoRefreshMs = processRefreshIntervalMs(cpuLoadBand, memoryLoadBand, interactionLatencyBand);
  useEffect(() => {
    if (timer.current) clearInterval(timer.current);
    timer.current = auto ? setInterval(() => { void pollIfVisible(); }, autoRefreshMs) : null;
    return () => {
      if (timer.current) clearInterval(timer.current);
      timer.current = null;
    };
  }, [auto, autoRefreshMs, pollIfVisible]);

  const optimizeMemoryPids = async (pids: number[], seed: MemoryOptimizationReport | null) => {
    setMemoryOperationWarning(null);
    const identities: MemoryCandidateIdentity[] = pids.map((pid) => {
      const candidate = seed?.candidates.find((entry) => entry.pid === pid);
      if (!candidate?.creation_token) throw new Error("候选进程身份信息缺失或已过期，请重新分析");
      return { pid, creation_token: candidate.creation_token };
    });
    const batchReports: MemoryOptimizationReport[] = [];
    let interrupted = false;
    let operationError: unknown;
    const attemptedPids: number[] = [];
    const confirmedPids: number[] = [];
    const candidateResults = new Map<number, MemoryCandidate>();
    for (let offset = 0; offset < identities.length; offset += 10) {
      const batch = identities.slice(offset, offset + 10);
      attemptedPids.push(...batch.map((candidate) => candidate.pid));
      let report: MemoryOptimizationReport;
      try {
        report = await api.optimizeProcessMemory(batch);
      } catch (error) {
        interrupted = true;
        operationError = error;
        break;
      }
      confirmedPids.push(...batch.map((candidate) => candidate.pid));
      batchReports.push(report);
      for (const candidate of report.candidates) candidateResults.set(candidate.pid, candidate);
    }
    const combinedReport = aggregateMemoryOptimizationBatches(batchReports, seed, interrupted);
    if (!combinedReport) throw new Error("没有可处理的进程");
    const report: MemoryOptimizationReport = {
      ...combinedReport,
      candidates: mergeMemoryCandidateResults(
        seed?.candidates ?? combinedReport.candidates,
        attemptedPids,
        [...candidateResults.values()],
      ),
    };
    const interruption = interrupted
      ? `分批执行中断：${confirmedPids.length} 个 PID 已收到结果；${attemptedPids.length - confirmedPids.length} 个当前批次请求结果未知（已锁定并需重新分析）；${pids.length - attemptedPids.length} 个 PID 尚未发送。失败批次后的可用内存未重新测量。${String(operationError)}`
      : null;
    return { report, interruption };
  };

  const loadMemoryPreview = async () => {
    if (memoryBusyRef.current) return;
    memoryBusyRef.current = true;
    setMemoryBusy(true);
    try {
      const report = await api.previewProcessMemoryOptimization();
      setMemoryReport(report);
      setMemoryOperationWarning(null);
      setSelectedMemoryPids([]);
    } catch (e) {
      toastErr(e);
    } finally {
      memoryBusyRef.current = false;
      setMemoryBusy(false);
    }
  };

  const optimizeMemory = async () => {
    if (memoryBusyRef.current || selectedMemoryPids.length === 0) return;
    memoryBusyRef.current = true;
    setMemoryBusy(true);
    try {
      const outcome = await optimizeMemoryPids(selectedMemoryPids, memoryReport);
      const { report, interruption } = outcome;
      setMemoryReport(report);
      setMemoryOperationWarning(interruption);
      setSelectedMemoryPids([]);
      if (interruption) {
        toastErr(interruption);
      } else {
        toastOk(`已处理 ${report.processed} 个第三方进程，${report.skipped} 个跳过；工作集估算变化 ${report.estimated_reclaimed_mb.toFixed(0)} MB，可用内存 ${formatAvailableMemory(report.available_before_mb)} → ${formatAvailableMemory(report.available_after_mb)}`);
      }
      load();
    } catch (e) {
      toastErr(e);
    } finally {
      memoryBusyRef.current = false;
      setMemoryBusy(false);
    }
  };

  const toggleMemoryCandidate = (pid: number) => {
    setSelectedMemoryPids((current) => current.includes(pid) ? current.filter((value) => value !== pid) : [...current, pid]);
  };

  const optimizeMemoryAll = async () => {
    if (memoryBusyRef.current) return;
    const eligible = memoryReport?.candidates.filter((candidate) => candidate.eligible).map((candidate) => candidate.pid) ?? [];
    if (eligible.length === 0) return;
    const confirmed = confirm(`将对 ${eligible.length} 个大体量、低活跃度进程分批回收工作集，每批最多 10 个。仅处理预览中的同一进程实例；执行时重新核验映像、前台/核心保护、工作集与 CPU。继续？`);
    if (!confirmed) return;
    memoryBusyRef.current = true;
    setMemoryBusy(true);
    try {
      const outcome = await optimizeMemoryPids(eligible, memoryReport);
      const { report, interruption } = outcome;
      setMemoryReport(report);
      setMemoryOperationWarning(interruption);
      setSelectedMemoryPids([]);
      if (interruption) {
        toastErr(interruption);
      } else {
        toastOk(`已处理 ${report.processed} 个进程，${report.skipped} 个跳过；工作集估算变化 ${report.estimated_reclaimed_mb.toFixed(0)} MB，可用内存 ${formatAvailableMemory(report.available_before_mb)} → ${formatAvailableMemory(report.available_after_mb)}`);
      }
      load();
    } catch (e) {
      toastErr(e);
    } finally {
      memoryBusyRef.current = false;
      setMemoryBusy(false);
    }
  };

  const search = query.trim().toLocaleLowerCase();
  const filtered = useMemo(() => list
    .filter((p) => !search || (focusPid != null && search === String(focusPid)
      ? p.pid === focusPid : p.name.toLocaleLowerCase().includes(search) || String(p.pid).includes(search)))
    .sort((a, b) => {
      const comparison = sortKey === "name"
        ? a.name.localeCompare(b.name, "zh-CN")
        : a[sortKey] - b[sortKey];
      return (desc ? -comparison : comparison) || a.pid - b.pid;
    }), [list, search, focusPid, sortKey, desc]);
  const displayRows = useMemo(() => {
    if (!treeView) return filtered.slice(0, MAX_PROCESS_ROWS).map((proc) => ({ proc, depth: 0 }));
    return buildProcessTreeRows(filtered);
  }, [filtered, treeView]);
  const cappedRows = useMemo(() => displayRows.slice(0, MAX_PROCESS_ROWS), [displayRows]);
  const visibleBodyOffset = Math.max(0, tableScrollTop - PROCESS_HEADER_HEIGHT_PX);
  const firstVisibleRow = Math.floor(visibleBodyOffset / processRowHeight);
  const virtualStart = Math.max(0, firstVisibleRow - PROCESS_ROW_OVERSCAN);
  const virtualCount = Math.ceil(tableViewportHeight / processRowHeight) + PROCESS_ROW_OVERSCAN * 2;
  const virtualEnd = Math.min(cappedRows.length, virtualStart + virtualCount);
  const virtualRows = useMemo(() => cappedRows.slice(virtualStart, virtualEnd), [cappedRows, virtualStart, virtualEnd]);
  const topSpacerHeight = virtualStart * processRowHeight;
  const bottomSpacerHeight = (cappedRows.length - virtualEnd) * processRowHeight;

  useEffect(() => {
    const table = processTableScrollRef.current;
    if (!table) return;
    const updateHeight = () => setTableViewportHeight(table.clientHeight);
    updateHeight();
    const observer = new ResizeObserver(updateHeight);
    observer.observe(table);
    return () => observer.disconnect();
  }, []);

  const firstVirtualRowPid = virtualRows[0]?.proc.pid;
  useLayoutEffect(() => {
    const row = firstVirtualRowRef.current;
    if (!row) return;
    const measured = row.getBoundingClientRect().height;
    if (measured > 0 && Math.abs(measured - processRowHeight) > 0.5) {
      setProcessRowHeight(measured);
    }
  }, [firstVirtualRowPid, processRowHeight, virtualStart, virtualRows.length]);

  // Optional user-driven response sampling: input event to the next frame after rows commit.
  useEffect(() => {
    const startedAt = inputInteractionStartedAt.current;
    if (!measureSearchLatency || startedAt == null) return;
    const frame = requestAnimationFrame(() => {
      if (inputInteractionStartedAt.current !== startedAt) return;
      inputInteractionStartedAt.current = null;
      const samples = inputLatencySamples.current;
      const sampleUpdate = recordInteractionLatencySample(
        samples,
        Math.max(0, performance.now() - startedAt),
        inputLatencySampleCount.current,
      );
      if (!sampleUpdate) return;
      inputLatencySampleCount.current = sampleUpdate.totalSampleCount;
      if (!sampleUpdate.shouldUpdateP95) return;
      setSearchLatencyP95Ms(nearestRankPercentile(samples, 0.95));
       setSearchLatencySampleCount(sampleUpdate.rollingSampleCount);
    });
    return () => cancelAnimationFrame(frame);
  }, [displayRows, measureSearchLatency, query]);

  const requireVerifiedActionIdentity = (p: ProcInfo) => {
    if (processMutationsDisabled) {
      toastErr("进程列表正在刷新或已过期，未执行操作；请刷新成功后重试。");
      return null;
    }
    const creationToken = p.creation_token;
    const creationTime = formatWindowsFiletimeUtc(creationToken);
    if (!creationToken || !creationTime) {
      toastErr("无法核验当前进程实例，未执行操作；请刷新进程列表后重试。");
      return null;
    }
    return { creationToken, creationTime };
  };

  const kill = async (p: ProcInfo) => {
    const identity = requireVerifiedActionIdentity(p);
    if (!identity) return;
    if (!confirm(`确定结束「${p.name}」(PID ${p.pid}，创建时间 ${identity.creationTime})？未保存的数据将丢失。`)) return;
    try {
      await api.killProcess(p.pid, identity.creationToken);
      toastOk(`已向 ${p.name}（PID ${p.pid}，${identity.creationTime}）发送结束请求`);
      load();
    } catch (e) {
      toastErr(e);
    }
  };

  const prio = async (p: ProcInfo, level: number) => {
    const identity = requireVerifiedActionIdentity(p);
    if (!identity) return;
    if (level === 5 && !confirm(`将「${p.name}」(PID ${p.pid}，创建时间 ${identity.creationTime}) 设为实时优先级，可能让其他进程或 Windows 界面得不到 CPU，造成系统卡顿或暂时无响应。通常不建议使用。\n\n仍要继续？`)) return;
    try {
      await api.setPriority(p.pid, identity.creationToken, level, level === 5);
      toastOk(`${p.name}（PID ${p.pid}）优先级 → ${PRIO_LABELS[level]}`);
    } catch (e) {
      toastErr(e);
    }
  };

  const resume = async (p: ProcInfo) => {
    const identity = requireVerifiedActionIdentity(p);
    if (!identity) return;
    if (!confirm(`确定尝试恢复「${p.name}」(PID ${p.pid}，创建时间 ${identity.creationTime})？后端会用列表捕获的创建时间令牌核对当前进程；核对失败时不会执行。如果该进程由其他程序暂停，此操作也可能改变其状态。`)) return;
    try {
      await api.resumeProcess(p.pid, identity.creationToken);
      toastOk(`已向 ${p.name}（PID ${p.pid}，${identity.creationTime}）发送恢复请求`);
      load();
    } catch (e) {
      toastErr(e);
      load();
    }
  };

  const exportCsv = async () => {
    if (exporting || filtered.length === 0) return;
    setExporting(true);
    try {
      const csvCell = (value: string | number) => {
        const text = String(value);
        // Neutralize formulas even when a cell starts with whitespace/control characters.
        const formulaText = /^[\s\u0000-\u001F]*[=+@\-]/.test(text) ? `'${text}` : text;
        const safe = formulaText.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "");
        return `"${safe.replace(/"/g, '""')}"`;
      };
      const rows = [
        ["进程", "PID", "角色", "CPU % (全机)", "内存 MB", "磁盘 IO MB/s", "路径"],
        ...filtered.map((p) => [p.name, p.pid, p.role_label, p.cpu.toFixed(2), p.mem_mb.toFixed(2), p.io_mb_s.toFixed(3), p.path]),
      ];
      const csv = "\uFEFF" + rows.map((row) => row.map(csvCell).join(",")).join("\r\n");
      const filename = `AeroTune-processes-${new Date().toISOString().replace(/[:.]/g, "-")}.csv`;
      const saved = await api.saveTextExport(filename, csv);
      if (!saved) return;
      toastOk(`已导出 ${filtered.length} 个当前筛选进程到所选文件`);
    } catch (e) {
      toastErr(e);
    } finally {
      setExporting(false);
    }
  };

  const affinity = async (p: ProcInfo, kind: string) => {
    const identity = requireVerifiedActionIdentity(p);
    if (!identity) return;
    try {
      await api.setProcessAffinity(p.pid, identity.creationToken, kind);
      toastOk(
        `${p.name}（PID ${p.pid}）亲和性 → ${
          kind === "all" ? "全部核心" : kind === "low" ? "低编号逻辑 CPU" : "高编号逻辑 CPU"
        }（会话级，进程重启后失效）`
      );
    } catch (e) {
      toastErr(e);
    }
  };

  const Th = ({ k, children, className = "", title }: { k: SortKey; children: string; className?: string; title?: string }) => {
    const alignment = className.includes("text-right") ? "justify-end text-right" : "justify-start text-left";
    const direction = sortKey === k ? (desc ? "降序" : "升序") : "默认降序";
    return (
      <th
        scope="col"
        aria-sort={sortKey === k ? (desc ? "descending" : "ascending") : undefined}
        className={`px-3 py-2 text-[11px] text-slate-400 ${className}`}
      >
        <button
          type="button"
          aria-label={`按${children}排序，当前为${direction}`}
          title={title}
          className={`flex w-full items-center gap-1 rounded-sm hover:text-slate-200 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70 ${alignment}`}
          onClick={() => {
            markProcessInteraction();
            if (sortKey === k) setDesc(!desc);
            else {
              setSortKey(k);
              setDesc(true);
            }
          }}
        >
          {children} {sortKey === k && <span aria-hidden="true">{desc ? "▾" : "▴"}</span>}
        </button>
      </th>
    );
  };

  return (
    <div>
      <PageHead
        title="进程管理"
        desc="CPU 按全机逻辑处理器总能力归一到 0–100%；CPU 与磁盘速率按采样区间平均（首次或长时间未刷新会短时预热），内存为当前工作集；可结束进程或调整优先级。为防止退出后遗留冻结进程，新挂起操作暂时关闭，恢复入口保留。"
        right={
          <>
            <Chip tone={auto ? "green" : "slate"}>
              <button
                aria-pressed={auto}
                aria-label={auto ? `关闭自动刷新；当前间隔 ${Math.round(autoRefreshMs / 1000)} 秒` : "开启自动刷新，按 CPU、内存压力和至少 20 个交互样本调整扫描间隔"}
                onClick={() => setAuto((value) => !value)}
                title={auto ? `当前间隔 ${Math.round(autoRefreshMs / 1000)} 秒；CPU、内存压力或基于至少 20 个样本的交互 P95 偏高时自动放缓` : "启动后按 CPU、内存压力及可选的交互 P95（至少 20 个样本）调整扫描间隔"}
              >
                {auto ? `自动刷新 · 开 · ${Math.round(autoRefreshMs / 1000)}s` : "自动刷新 · 关"}
              </button>
            </Chip>
            <Btn variant="ghost" onClick={() => setTreeView((value) => !value)}>
              <ListTree size={14} /> {treeView ? "平铺进程" : "进程树"}
            </Btn>
            <Btn variant="ghost" onClick={exportCsv} disabled={exporting || filtered.length === 0}>
              <Download size={14} /> 导出 CSV
            </Btn>
            <Btn variant="ghost" onClick={load} disabled={loading}>
              <RefreshCw size={14} className={loading ? "animate-spin" : ""} /> 刷新
            </Btn>
          </>
        }
      />

      <div role="status" className="mb-3 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3.5 py-2.5 text-xs leading-5 text-amber-100">
        为防止退出或崩溃后遗留冻结进程，新挂起操作暂时关闭；恢复入口仍保留。恢复前请确认目标进程。
      </div>

      <div className="glass mb-4 flex flex-wrap items-center gap-2 px-4 py-2.5">
        <Search size={15} className="text-slate-500" />
        <input
          aria-label="搜索进程名称或 PID"
          value={query}
          onChange={(e) => {
            markProcessInteraction();
            setQuery(e.target.value);
          }}
          placeholder="搜索进程名称或 PID…"
          className="min-w-40 flex-1 bg-transparent text-sm text-slate-200 outline-none placeholder:text-slate-600"
        />
        {query && (
          <button onClick={() => { markProcessInteraction(); setQuery(""); }} aria-label="清除搜索" className="text-slate-500 hover:text-slate-300">
            <XCircle size={15} />
          </button>
        )}
        <Chip tone={measureSearchLatency ? "green" : "slate"}>
          <button
            aria-pressed={measureSearchLatency}
            aria-label={measureSearchLatency ? "关闭搜索与排序响应测量" : "开启搜索与排序响应测量"}
            aria-describedby="process-response-measurement-status"
            title="启用后记录搜索或排序到下一帧的 P95（最多保留 31 个样本）；至少有 20 个样本后，P95 偏高时自动延长进程列表刷新间隔。"
            onClick={() => {
              inputInteractionStartedAt.current = null;
              inputLatencySamples.current = [];
              inputLatencySampleCount.current = 0;
              setSearchLatencyP95Ms(null);
               setSearchLatencySampleCount(0);
              setMeasureSearchLatency((enabled) => !enabled);
            }}
          >
            <span id="process-response-measurement-status">
              {measureSearchLatency
                ? <>响应 P95 {searchLatencyP95Ms == null ? "—" : `${searchLatencyP95Ms.toFixed(1)} ms`} · {inputLatencySamples.current.length < 20 ? `${inputLatencySamples.current.length}/20 样本` : `${inputLatencySamples.current.length} 样本 · 已就绪`}</>
                : "测搜索/排序"}
            </span>
          </button>
        </Chip>
        <Btn variant="ghost" onClick={loadMemoryPreview} disabled={memoryBusy}>
          <MemoryStick size={14} /> {memoryBusy ? "分析中…" : "分析内存候选"}
        </Btn>
      </div>

      {memoryReport && (
        <div className="glass mb-4 p-4">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div>
              <div className="text-sm font-medium text-slate-200">进程内存优化 · 预览</div>
              <div className="mt-1 text-xs text-slate-400">
                可用内存 {memoryReport.available_before_mb.toFixed(0)} MB；选中 {selectedMemoryPids.length} 个候选。候选每次重新分析后默认不勾选；仅可选低 CPU、工作集 ≥128 MiB 的第三方进程，系统与前台进程自动保护。
              </div>
            </div>
            <div className="flex flex-wrap gap-2">
              <Btn variant="ghost" onClick={loadMemoryPreview} disabled={memoryBusy}>重新分析</Btn>
              <Btn variant="ghost" onClick={optimizeMemoryAll} disabled={memoryBusy || !memoryReport.candidates.some((candidate) => candidate.eligible)}>
                优化全部候选（分批确认）
              </Btn>
              <Btn variant="ghost" onClick={() => setSelectedMemoryPids(memoryReport.candidates.filter((candidate) => candidate.eligible).map((candidate) => candidate.pid))} disabled={memoryBusy || !memoryReport.candidates.some((candidate) => candidate.eligible)}>
                全选可优化项
              </Btn>
              <Btn onClick={optimizeMemory} disabled={memoryBusy || selectedMemoryPids.length === 0}>
                <Zap size={14} /> {memoryBusy ? "处理中…" : "优化选中进程"}
              </Btn>
            </div>
          </div>
          {memoryOperationWarning && (
            <div role="alert" className="mt-3 rounded-lg border border-amber-400/20 bg-amber-500/10 p-3 text-xs text-amber-200">
              {memoryOperationWarning}
            </div>
          )}
          <div className="mt-3 max-h-64 overflow-auto rounded-lg border border-white/8">
            {memoryReport.candidates.length === 0 ? (
              <div className="p-4 text-sm text-slate-400">当前没有达到回收门槛的进程。</div>
            ) : memoryReport.candidates.map((candidate) => (
              <label key={candidate.pid} className="flex items-center gap-3 border-b border-white/5 px-3 py-2 text-xs last:border-0">
                <input
                  type="checkbox"
                  checked={candidate.eligible && selectedMemoryPids.includes(candidate.pid)}
                  disabled={!candidate.eligible || memoryBusy}
                  onChange={() => toggleMemoryCandidate(candidate.pid)}
                  className="accent-cyan-400"
                />
                <span className="min-w-0 flex-1 truncate text-slate-200" title={candidate.path || candidate.name}>{candidate.name} <span className="text-slate-500">PID {candidate.pid}</span></span>
                <span className="tabular-nums text-slate-300">工作集 {candidate.working_set_mb.toFixed(0)} MB</span>
                <span className="w-36 text-right text-slate-500">{candidate.eligible ? `CPU ${candidate.cpu.toFixed(1)}%` : candidate.reason}</span>
              </label>
            ))}
          </div>
          <div className="mt-2 text-[11px] text-slate-500">
            工作集回收会在进程后续访问数据时重新载入；它不等同于结束进程，实际可用内存变化取决于 Windows 内存管理器。
            {memoryReport.processed > 0 && ` 本次：${memoryReport.processed} 个进程，估算工作集变化 ${memoryReport.estimated_reclaimed_mb.toFixed(0)} MB，可用内存 ${formatAvailableMemory(memoryReport.available_before_mb)} → ${formatAvailableMemory(memoryReport.available_after_mb)}。`}
          </div>
        </div>
      )}

      {insights && insights.entries.length > 0 && (
        <div className="mb-4">
          <Card
            title="后台常驻洞察 · 非系统进程（按内存排序）"
            icon={<Radar size={15} className="text-cyan-300" />}
            right={<Chip tone="slate">{insights.entries.length} 项</Chip>}
          >
            <div className="space-y-1.5">
              {insights.entries.map((entry) => {
                const signature = insightSignatureMeta(entry.signature);
                return (
                  <div
                    key={entry.pid}
                    className="flex flex-wrap items-center gap-3 rounded-lg border border-white/8 bg-white/3 px-3.5 py-2"
                  >
                    <span className="min-w-0 flex-1 truncate text-[13px] text-slate-200" title={`${entry.path}\n${entry.role_detail}`}>
                      {entry.name} <span className="text-slate-500">· PID {entry.pid}</span>
                    </span>
                    <Chip tone={insightRoleTone(entry.role_id)}>{entry.role_label}</Chip>
                    <span className="w-20 text-right text-[12px] tabular-nums text-slate-300">{entry.mem_mb.toFixed(0)} MB</span>
                    <span className="w-16 text-right text-[12px] tabular-nums text-slate-400">{entry.cpu.toFixed(1)}%</span>
                    <span className="w-28 text-right text-[11px] text-slate-500">
                      {entry.start_age_secs != null ? `运行 ${insightAgeLabel(entry.start_age_secs)}` : "启动时间未知"}
                    </span>
                    <Chip tone={signature.tone}>{signature.label}</Chip>
                  </div>
                );
              })}
            </div>
            <div role="status" className="mt-3 rounded-xl border border-white/6 bg-black/15 px-3.5 py-3 text-xs leading-5 text-slate-400">
              画像与签名状态来自本地静态证据（路径、自启来源、内嵌 Authenticode 签名），不联网查询；角色分类是启发式，可能误判
              {insights.signature_deferred > 0 ? `；签名校验每轮最多 8 个文件，本轮 ${insights.signature_deferred} 个待下轮补验` : ""}
              。处置动作始终由你决定，此处不自动更改任何进程。
            </div>
          </Card>
        </div>
      )}
      {residency.length > 0 && (
        <div className="mb-4">
          <Card
            title="常驻模式观察 · 长期驻留的低活跃进程"
            icon={<Radar size={15} className="text-amber-300" />}
            right={<Chip tone="slate">{residency.length} 项</Chip>}
          >
            <div className="space-y-1.5">
              {residency.map((entry) => (
                <div key={entry.name} className="flex flex-wrap items-center gap-3 rounded-lg border border-white/8 bg-white/3 px-3.5 py-2">
                  <span className="min-w-0 flex-1 truncate text-[13px] text-slate-200" title={entry.name}>{entry.name}</span>
                  {entry.role_hint === "updater-suspect" && <Chip tone="rose">更新器嫌疑</Chip>}
                  <span className="text-[12px] tabular-nums text-slate-400">
                    跨 {entry.resident_days} 天观测 · 平均 {entry.avg_mem_mb.toFixed(0)} MB · {Math.round(entry.low_cpu_ratio * 100)}% 时间低活跃
                  </span>
                  <Btn
                    variant="ghost"
                    aria-label={`忽略 ${entry.name} 的常驻观察建议`}
                    onClick={async () => {
                      try {
                        if (await api.dismissResidencySuggestion(entry.name)) {
                          setResidency((current) => current.filter((item) => item.name !== entry.name));
                        }
                      } catch (e) {
                        toastErr(e);
                      }
                    }}
                  >
                    忽略
                  </Btn>
                </div>
              ))}
            </div>
            <div role="status" className="mt-3 rounded-xl border border-white/6 bg-black/15 px-3.5 py-3 text-xs leading-5 text-slate-400">
              由本机低频观测累积（仅 AeroTune 运行期间、每 30 分钟一个样本），不代表全天行为；"低活跃"按全机 CPU ≤2% 判定。
              这里只做观察提示、不会自动处置任何进程；可在进程列表结束进程，暂停功能暂时关闭。
            </div>
          </Card>
        </div>
      )}
      <div className="mb-4"><ProcessHistoryCard processes={list} /></div>

      {snapshotStatus.stale && (
        <div role="alert" className="mb-3 flex flex-wrap items-start justify-between gap-3 rounded-lg border border-amber-400/20 bg-amber-500/10 p-3 text-xs text-amber-200">
          <div className="min-w-0 flex-1 break-words">
            {snapshotStatus.hasSnapshot
              ? "进程列表刷新失败；当前保留的是上次成功快照。为避免基于过期 PID 操作，优先级、CPU 亲和性、恢复和结束均已禁用。"
              : "无法读取进程列表，目前没有可用快照；进程列表中的 PID 操作已禁用。"}
          </div>
          <Btn variant="ghost" onClick={load} disabled={loading} className="shrink-0">
            {loading ? "刷新中…" : "重试刷新"}
          </Btn>
        </div>
      )}

      <div className="glass overflow-hidden">
        <div
          ref={processTableScrollRef}
          onScroll={(event) => setTableScrollTop(event.currentTarget.scrollTop)}
          className="max-h-[calc(100vh-260px)] overflow-auto overscroll-x-contain"
        >
          <table aria-label="进程列表" className="w-full min-w-[900px] text-left text-[13px]">
            <thead className="sticky top-0 bg-[#111827]/95 backdrop-blur">
              <tr className="border-b border-white/8">
                <Th k="name">进程</Th>
                <Th k="pid" className="w-48" title="按 PID 排序；启动时间用于区分进程实例">PID / 启动时间 (UTC)</Th>
                <Th k="cpu" className="w-24 text-right" title="按全机逻辑处理器总能力归一，CPU 使用率范围为 0–100%">CPU</Th>
                <Th k="mem_mb" className="w-28 text-right">内存</Th>
                <Th k="io_mb_s" className="w-28 text-right">磁盘 IO</Th>
                <th scope="col" className="w-56 px-3 py-2 text-[11px] text-slate-400">路径</th>
                <th scope="col" className="w-64 px-3 py-2 text-right text-[11px] text-slate-400">操作</th>
              </tr>
            </thead>
            <tbody>
              {topSpacerHeight > 0 && (
                <tr key="virtual-top" aria-hidden="true">
                  <td colSpan={7} style={{ height: `${topSpacerHeight}px`, padding: 0, border: 0 }} />
                </tr>
              )}
              {virtualRows.map(({ proc: p, depth }, index) => {
                const creationTime = formatWindowsFiletimeUtc(p.creation_token);
                const visibleCreationTime = creationTime ? `${creationTime.slice(5, 27)} UTC` : "无法核验";
                const identityVerified = creationTime !== null;
                const rowMutationsDisabled = processMutationsDisabled || !identityVerified;
                const rowMutationDisabledReason = processMutationsDisabled ? "进程列表需刷新" : "无法核验创建时间，请刷新进程列表";
                return (
                <tr
                  key={p.pid}
                  ref={index === 0 ? firstVirtualRowRef : undefined}
                  style={{ height: `${processRowHeight}px` }}
                  className="whitespace-nowrap border-b border-white/4 transition-colors hover:bg-white/4"
                >
                  <td className="max-w-52 truncate px-3 py-2 font-medium text-slate-200" title={`${p.name} · PID ${p.pid} · 创建时间 ${creationTime ?? "无法核验"}${p.parent_pid != null ? ` · 父 PID ${p.parent_pid}` : ""}`} style={{ paddingLeft: `${12 + depth * 14}px` }}>
                    {treeView && depth > 0 ? <span className="mr-1 text-slate-600">└</span> : null}{p.name}
                    {NOTABLE_ROLE_META[p.role] && (
                      <span className={`ml-1.5 inline-block rounded px-1 py-px align-middle text-[10px] ${NOTABLE_ROLE_META[p.role].tone === "rose" ? "bg-rose-400/15 text-rose-300" : NOTABLE_ROLE_META[p.role].tone === "violet" ? "bg-violet-400/15 text-violet-300" : "bg-amber-400/15 text-amber-300"}`} title={`本地画像：${p.role_label}`}>
                        {NOTABLE_ROLE_META[p.role].label}
                      </span>
                    )}
                  </td>
                  <td className="w-48 max-w-48 px-3 py-1 tabular-nums text-slate-500" title={creationTime ?? "无法核验创建时间"}>
                    <span className="block leading-4 text-slate-400">{p.pid}</span>
                    <span className="block truncate font-mono text-[10px] leading-3 text-slate-400">{visibleCreationTime}</span>
                  </td>
                  <td className={`px-3 py-2 text-right tabular-nums ${p.cpu > 30 ? "text-rose-300" : p.cpu > 8 ? "text-amber-300" : "text-slate-400"}`}>
                    {p.cpu.toFixed(1)}%
                  </td>
                  <td className="px-3 py-2 text-right tabular-nums text-slate-400">{p.mem_mb.toFixed(1)} MB</td>
                  <td className={`px-3 py-2 text-right tabular-nums ${p.io_mb_s > 8 ? "text-violet-300" : p.io_mb_s > 1 ? "text-slate-300" : "text-slate-500"}`}>
                    {p.io_mb_s >= 1 ? p.io_mb_s.toFixed(1) + " MB/s" : (p.io_mb_s * 1024).toFixed(0) + " KB/s"}
                  </td>
                  <td className="max-w-56 truncate px-3 py-2 text-[11px] text-slate-500" title={p.path}>
                    {p.path || "—"}
                  </td>
                  <td className="px-3 py-2">
                    <div className="flex items-center justify-end gap-1">
                      <select
                        aria-label={rowMutationsDisabled ? `${p.name}，PID ${p.pid}，优先级暂不可用，${rowMutationDisabledReason}` : `${p.name}，PID ${p.pid}，优先级`}
                        defaultValue=""
                        disabled={rowMutationsDisabled}
                        onChange={(e) => {
                          if (e.target.value !== "") prio(p, Number(e.target.value));
                          e.target.value = "";
                        }}
                        className="rounded-md border border-white/10 bg-white/5 px-1.5 py-1 text-[11px] text-slate-300 outline-none hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-50"
                        title={rowMutationsDisabled ? `${rowMutationDisabledReason}，优先级修改已禁用` : "优先级"}
                      >
                        <option value="" disabled>
                          <Gauge size={10} /> 优先级
                        </option>
                        {PRIO_LABELS.map((l, i) => (
                          <option key={i} value={i}>
                            {l}
                          </option>
                        ))}
                      </select>
                      <select
                        aria-label={rowMutationsDisabled ? `${p.name}，PID ${p.pid}，CPU 亲和性暂不可用，${rowMutationDisabledReason}` : `${p.name}，PID ${p.pid}，CPU 亲和性（会话级）`}
                        defaultValue=""
                        disabled={rowMutationsDisabled}
                        onChange={(e) => {
                          if (e.target.value !== "") affinity(p, e.target.value);
                          e.target.value = "";
                        }}
                        className="rounded-md border border-white/10 bg-white/5 px-1.5 py-1 text-[11px] text-slate-300 outline-none hover:bg-white/10 disabled:cursor-not-allowed disabled:opacity-50"
                        title={rowMutationsDisabled ? `${rowMutationDisabledReason}，CPU 亲和性修改已禁用` : "CPU 亲和性（会话级）"}
                      >
                        <option value="" disabled>
                          亲和性
                        </option>
                        <option value="all">全部核心</option>
                        <option value="low">低编号半区（非 P/E）</option>
                        <option value="high">高编号半区（非 P/E）</option>
                      </select>
                      <button
                        type="button"
                        disabled
                        aria-label={`挂起进程暂不可用，${p.name}，PID ${p.pid}`}
                        className="cursor-not-allowed rounded-md p-1.5 text-slate-600"
                        title="为防止退出后遗留冻结进程，新挂起操作暂时关闭"
                      >
                        <Pause size={14} />
                      </button>
                      <button
                        type="button"
                        disabled={rowMutationsDisabled}
                        aria-label={rowMutationsDisabled
                          ? `恢复进程暂不可用，${p.name}，PID ${p.pid}，${rowMutationDisabledReason}`
                          : `恢复进程 ${p.name}，PID ${p.pid}，创建时间 ${creationTime}`}
                        onClick={() => resume(p)}
                        className={!rowMutationsDisabled
                          ? "rounded-md p-1.5 text-slate-400 hover:bg-white/10 hover:text-emerald-300"
                          : "cursor-not-allowed rounded-md p-1.5 text-slate-600"}
                        title={rowMutationsDisabled
                          ? `${rowMutationDisabledReason}，恢复已禁用`
                          : `创建时间 ${creationTime}；确认后将用完整令牌核验进程身份`}
                      >
                        <Play size={14} />
                      </button>
                      <button
                        type="button"
                        disabled={rowMutationsDisabled}
                        aria-label={rowMutationsDisabled
                          ? `结束进程暂不可用，${p.name}，PID ${p.pid}，${rowMutationDisabledReason}`
                          : `结束进程 ${p.name}，PID ${p.pid}`}
                        onClick={() => kill(p)}
                        className={rowMutationsDisabled
                          ? "cursor-not-allowed rounded-md p-1.5 text-slate-600"
                          : "rounded-md p-1.5 text-slate-400 hover:bg-rose-500/20 hover:text-rose-300"}
                        title={rowMutationsDisabled
                          ? `${rowMutationDisabledReason}，结束已禁用`
                          : "结束进程"}
                      >
                        <XCircle size={14} />
                      </button>
                    </div>
                  </td>
                </tr>
                );
              })}
              {bottomSpacerHeight > 0 && (
                <tr key="virtual-bottom" aria-hidden="true">
                  <td colSpan={7} style={{ height: `${bottomSpacerHeight}px`, padding: 0, border: 0 }} />
                </tr>
              )}
              {filtered.length === 0 && (
                <tr>
                  <td colSpan={7} className="px-4 py-10 text-center text-sm text-slate-500">
                    <MemoryStick className="mx-auto mb-2 opacity-40" />
                    {focusPid != null && search === String(focusPid)
                      ? `PID ${focusPid} 当前未出现在列表中（进程可能已退出，或不在前 500 个采样进程中）。`
                      : "没有匹配的进程"}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>
      <div className="mt-2 text-right text-[11px] text-slate-600">
        共 {list.length} 个进程 · 当前筛选 {filtered.length} 个 · 显示前 {Math.min(filtered.length, MAX_PROCESS_ROWS)} 个 · 新挂起暂时关闭，恢复操作需确认
      </div>
    </div>
  );
}
