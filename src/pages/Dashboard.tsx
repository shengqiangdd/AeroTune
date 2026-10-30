// 仪表盘：实时环形仪表 + 网络走势 + 快捷操作 + 系统信息 + 智能诊断
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, MonitorSmartphone, Zap, BatteryCharging, Clock, Cpu, HardDrive, ShieldAlert, Lightbulb, ArrowRight, RefreshCw, Download } from "lucide-react";
import { api, Stats, Diagnostics, SmartRecommendation, AnomalyContext, TimelineEvent } from "../lib/api";
import { parseDiagnosticReport, redactDiagnosticReport, type DiagnosticReport } from "../lib/diagnosticReport.js";
import type { Hist, StatsStatus } from "../hooks/useStats";
import { Card, Ring, Spark, Btn, Chip, PageHead } from "../components/ui";
import { fmtBytes, fmtGb, fmtUptime } from "../lib/format";
import { validCapacityPair } from "../lib/resourceEvidence";
import { batteryAcState, batteryQueryFailed } from "../lib/powerEstimate";
import { effectivePowerModePresentation } from "../lib/effectivePowerMode.js";
import { toastOk, toastErr, toast } from "../lib/toast";
import { PageId } from "../components/Sidebar";
import { SymptomGuide } from "../components/SymptomGuide";
import { OptimizationLab } from "../components/OptimizationLab";
import { CpuTopologyCard } from "../components/CpuTopologyCard";
import { ReliabilityTimeline } from "../components/ReliabilityTimeline";
import { DiskSpaceHistoryCard } from "../components/DiskSpaceHistoryCard";

const KIND_META: Record<string, { label: string; tone: "rose" | "amber" | "violet" }> = {
  cpu: { label: "CPU 异常", tone: "rose" },
  mem: { label: "疑似内存泄漏", tone: "amber" },
  io: { label: "磁盘 IO 密集", tone: "violet" },
};

function onsetAgeLabel(secs: number): string {
  if (secs < 60) return `${secs} 秒`;
  if (secs < 3600) return `${Math.floor(secs / 60)} 分钟`;
  return `${Math.floor(secs / 3600)} 小时 ${Math.floor((secs % 3600) / 60)} 分`;
}

const TIMELINE_KIND_META: Record<string, { color: string; label: string }> = {
  onset: { color: "#22d3ee", label: "异常起点" },
  "process-start": { color: "#a78bfa", label: "进程启动" },
  journal: { color: "#fbbf24", label: "变更日志" },
  decision: { color: "#34d399", label: "引擎切换" },
};

/** 事件关联时间线：手绘 SVG，横轴锚定异常起点，展示同窗口的本地事件。
 *  事件按时间均匀定位；仅为相关提示，非因果结论。 */
const AnomalyTimeline = memo(function AnomalyTimeline({ context }: { context: AnomalyContext }) {
  const events = context.timeline;
  const onset = events.find((event) => event.kind === "onset");
  const others = events.filter((event) => event.kind !== "onset");
  if (!onset || others.length === 0) return null;
  const times = events.map((event) => event.at_unix);
  const minT = Math.min(...times) - 15;
  const maxT = Math.max(onset.at_unix + 15, minT + 60);
  const span = Math.max(1, maxT - minT);
  const width = 560;
  const height = 46;
  const x = (t: number) => 8 + ((t - minT) / span) * (width - 16);
  const summary = `${others
    .map((event) => `${TIMELINE_KIND_META[event.kind]?.label ?? event.kind}「${event.label}」`)
    .join("、")}，均在异常起点附近`;
  return (
    <div className="mt-1" role="img" aria-label={`事件关联时间线：${summary}`}>
      <svg viewBox={`0 0 ${width} ${height}`} className="h-11 w-full" aria-hidden="true">
        <line x1={8} y1={32} x2={width - 8} y2={32} stroke="rgba(148,163,184,0.25)" strokeWidth={1} />
        {(() => {
          // 分钟刻度：从窗口起点到终点，最多 6 条
          const stepSecs = Math.max(30, Math.ceil(span / 300) * 60);
          const ticks: number[] = [];
          for (let t = minT + stepSecs; t < maxT && ticks.length < 6; t += stepSecs) ticks.push(t);
          return ticks.map((t) => (
            <g key={t}>
              <line x1={x(t)} y1={30} x2={x(t)} y2={34} stroke="rgba(148,163,184,0.3)" strokeWidth={1} />
              <text x={x(t)} y={44} textAnchor="middle" fontSize={8} fill="rgba(148,163,184,0.55)">
                -{Math.max(1, Math.round((onset.at_unix - t) / 60))}分
              </text>
            </g>
          ));
        })()}
        {others.map((event, index) => {
          const color = TIMELINE_KIND_META[event.kind]?.color ?? "#94a3b8";
          const cx = x(event.at_unix);
          const above = index % 2 === 0;
          return (
            <g key={`${event.kind}-${event.at_unix}-${index}`}>
              <line x1={cx} y1={above ? 20 : 32} x2={cx} y2={above ? 26 : 38} stroke={color} strokeOpacity={0.5} strokeWidth={1} />
              <circle cx={cx} cy={32} r={3.2} fill={color} fillOpacity={0.9} />
            </g>
          );
        })}
        <g>
          <line x1={x(onset.at_unix)} y1={12} x2={x(onset.at_unix)} y2={40} stroke={TIMELINE_KIND_META.onset.color} strokeWidth={1.5} />
          <circle cx={x(onset.at_unix)} cy={32} r={4.5} fill={TIMELINE_KIND_META.onset.color} />
          <text x={x(onset.at_unix)} y={9} textAnchor="middle" fontSize={8.5} fill={TIMELINE_KIND_META.onset.color}>
            起点
          </text>
        </g>
      </svg>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[10px] text-slate-500">
        {Object.entries(TIMELINE_KIND_META).map(([kind, meta]) => (
          <span key={kind} className="inline-flex items-center gap-1">
            <span className="inline-block size-1.5 rounded-full" style={{ backgroundColor: meta.color }} />
            {meta.label}
          </span>
        ))}
        <span>相关事件时间线，非因果结论</span>
      </div>
    </div>
  );
});

const RECOMMENDATION_PAGES: PageId[] = ["processes", "cleaner", "power", "network", "auto", "startup", "settings", "game", "dev"];
function isRecommendationPage(value: string): value is PageId {
  return RECOMMENDATION_PAGES.some((page) => page === value);
}

function fmtAnomaly(a: { kind: string; value: number; baseline: number; robust_z: number }): string {
  const deviation = ` · 偏离 ${a.robust_z.toFixed(1)} 个稳健尺度`;
  if (a.kind === "mem") return `${a.value.toFixed(0)} MB · 中位数 ${a.baseline.toFixed(0)} MB${deviation}`;
  if (a.kind === "io") return `${(a.value / 1024).toFixed(1)} MB/s · 中位数 ${(a.baseline / 1024).toFixed(1)} MB/s${deviation}`;
  return `CPU ${a.value.toFixed(1)}%（全机） · 中位数 ${a.baseline.toFixed(1)}%（全机）${deviation}`;
}

function recommendationIcon(id: string) {
  if (id.startsWith("gpu-temperature")) return "🌡";
  if (id.startsWith("gpu-memory")) return "🎮";
  if (id === "pagefile-pressure") return "🧠";
  return "💡";
}

export function Dashboard({
  stats,
  statsStatus,
  hist,
  onNav,
}: {
  stats: Stats | null;
  statsStatus: StatsStatus;
  hist: Hist;
  onNav: (p: PageId, pid?: number) => void;
}) {
  const [info, setInfo] = useState<Awaited<ReturnType<typeof api.getSystemInfo>> | null>(null);
  const [infoStatus, setInfoStatus] = useState<"loading" | "ready" | "error">("loading");
  const [headerRetrying, setHeaderRetrying] = useState(false);
  const headerRequestId = useRef(0);
  const [busy, setBusy] = useState(false);
  const [exportingReport, setExportingReport] = useState(false);
  const reportPending = useRef(false);
  const reportDialogRef = useRef<HTMLDialogElement>(null);
  const [reportPreview, setReportPreview] = useState<DiagnosticReport | null>(null);
  const [includeDeviceIdentifiers, setIncludeDeviceIdentifiers] = useState(false);
  const [includeDetailedTelemetry, setIncludeDetailedTelemetry] = useState(false);
  const [mode, setMode] = useState<string | null>(null);
  const [modeStatus, setModeStatus] = useState<"loading" | "ready" | "error">("loading");
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [diagStatus, setDiagStatus] = useState<"loading" | "ready" | "stale" | "error">("loading");
  const diagRef = useRef<Diagnostics | null>(null);
  const [recommendations, setRecommendations] = useState<SmartRecommendation[]>([]);
  const [recommendationsLoading, setRecommendationsLoading] = useState(true);
  const [recommendationsError, setRecommendationsError] = useState(false);
  const tipsPending = useRef(false);
  const tipsActive = useRef(false);
  const diagPending = useRef(false);
  const [anomalyContext, setAnomalyContext] = useState<AnomalyContext[] | null>(null);
  const contextPending = useRef(false);

  const closeReportPreview = () => {
    setReportPreview(null);
    setIncludeDeviceIdentifiers(false);
    setIncludeDetailedTelemetry(false);
  };

  useEffect(() => {
    const dialog = reportDialogRef.current;
    if (!dialog) return;
    if (reportPreview && !dialog.open) {
      try {
        dialog.showModal();
      } catch {
        toastErr("无法打开诊断报告预览，报告未保存。");
        setReportPreview(null);
      }
    } else if (!reportPreview && dialog.open) {
      dialog.close();
    }
  }, [reportPreview]);

  const loadHeaderState = useCallback(async (retry = false) => {
    const requestId = ++headerRequestId.current;
    if (retry) setHeaderRetrying(true);
    setInfoStatus("loading");
    setModeStatus("loading");
    try {
      const [infoResult, modeResult] = await Promise.allSettled([api.getSystemInfo(), api.getModeState()]);
      if (requestId !== headerRequestId.current) return;
      if (infoResult.status === "fulfilled") { setInfo(infoResult.value); setInfoStatus("ready"); }
      else setInfoStatus("error");
      if (modeResult.status === "fulfilled") { setMode(modeResult.value); setModeStatus("ready"); }
      else setModeStatus("error");
    } finally {
      if (retry && requestId === headerRequestId.current) setHeaderRetrying(false);
    }
  }, []);

  const refreshRecommendations = async (notify = true) => {
    if (tipsPending.current) return;
    tipsPending.current = true;
    setRecommendationsLoading(true);
    setRecommendationsError(false);
    try {
      const items = await api.getSmartRecommendations();
      if (tipsActive.current) {
        setRecommendations(items);
        setRecommendationsError(false);
      }
    } catch (e) {
      if (tipsActive.current) {
        setRecommendationsError(true);
        if (notify) toastErr(e);
      }
    } finally {
      tipsPending.current = false;
      if (tipsActive.current) setRecommendationsLoading(false);
    }
  };

  useEffect(() => {
    let disposed = false;
    tipsActive.current = true;
    void loadHeaderState();
    const refreshDiag = async () => {
      if (diagPending.current || disposed) return;
      diagPending.current = true;
      try {
        const value = await api.getDiagnostics();
        if (!disposed) {
          diagRef.current = value;
          setDiag(value);
          setDiagStatus("ready");
          // 根因提示只在存在确认异常时拉取；异常消失即清空，避免无谓轮询。
          if (value.anomalies.length === 0) {
            setAnomalyContext(null);
          } else if (!contextPending.current) {
            contextPending.current = true;
            api.getAnomalyContext()
              .then((contexts) => { if (!disposed) setAnomalyContext(contexts); })
              .catch(() => {})
              .finally(() => { contextPending.current = false; });
          }
        }
      } catch {
        if (!disposed) setDiagStatus(diagRef.current ? "stale" : "error");
      } finally {
        diagPending.current = false;
      }
    };
    refreshDiag();
    refreshRecommendations(false);
    const t = setInterval(() => { if (!document.hidden) refreshDiag(); }, 10000);
    const tipsTimer = setInterval(() => { if (!document.hidden) refreshRecommendations(false); }, 30000);
    return () => { disposed = true; headerRequestId.current += 1; tipsActive.current = false; clearInterval(t); clearInterval(tipsTimer); };
  }, [loadHeaderState]);

  const exportReport = async () => {
    if (reportPending.current) return;
    reportPending.current = true;
    setExportingReport(true);
    try {
      const report = parseDiagnosticReport(await api.exportDiagnosticReport());
      setIncludeDeviceIdentifiers(false);
      setIncludeDetailedTelemetry(false);
      setReportPreview(report);
    } catch (e) {
      toastErr(e);
    } finally {
      reportPending.current = false;
      setExportingReport(false);
    }
  };

  const saveDiagnosticReport = async () => {
    if (!reportPreview || reportPending.current) return;
    reportPending.current = true;
    setExportingReport(true);
    try {
      const report = redactDiagnosticReport(reportPreview, {
        includeDeviceIdentifiers,
        includeDetailedTelemetry,
      });
      const localDate = new Date().toLocaleDateString("sv-SE");
      const filename = `AeroTune-report-${localDate}.json`;
      const saved = await api.saveTextExport(filename, JSON.stringify(report, null, 2));
      if (!saved) return;
      closeReportPreview();
      toastOk("诊断报告已按预览选项保存到所选本机路径；AeroTune 不会上传报告。");
    } catch (e) {
      toastErr(e);
    } finally {
      reportPending.current = false;
      setExportingReport(false);
    }
  };

  const smartTrim = async () => {
    setBusy(true);
    try {
      const report = await api.previewProcessMemoryOptimization();
      const eligibleCount = report.candidates.filter((candidate) => candidate.eligible).length;
      const availableAtAnalysis = `候选分析时 Windows 可用 ${report.available_before_mb.toFixed(0)} MB`;
      if (eligibleCount > 0) {
        toast("info", `找到 ${eligibleCount} 个可优化进程；${availableAtAnalysis}。请到「进程管理」预览并选择。`);
      } else {
        toast("info", `当前没有符合门槛的低活跃大内存进程；${availableAtAnalysis}。`);
      }
    } catch (e) {
      toastErr(e);
    } finally {
      setBusy(false);
    }
  };

  const flush = async () => {
    try {
      await api.flushDns();
      toastOk("DNS 缓存已刷新");
    } catch (e) {
      toastErr(e);
    }
  };

  const memPct = stats && validCapacityPair(stats.mem_used_gb, stats.mem_total_gb)
    ? (stats.mem_used_gb / stats.mem_total_gb) * 100
    : null;
  const batteryInfo = batteryQueryFailed(stats) ? null : stats?.battery ?? null;
  const batteryAc = batteryAcState(batteryInfo);
  const batteryFill = batteryAc === false ? "bg-amber-400" : batteryAc === true ? "grad-btn" : "bg-slate-500";
  const effectivePowerMode = effectivePowerModePresentation(stats?.effective_power_mode, statsStatus === "loading", statsStatus === "stale" || statsStatus === "error");
  const cDisk = stats?.disks.find((d) => d.mount.startsWith("C"));
  const cPct = cDisk && cDisk.total_gb > 0 ? (cDisk.used_gb / cDisk.total_gb) * 100 : null;
  const gpuUtil = stats?.gpu?.utilization ?? null;
  const recommendationPid = (id: string): number | undefined => {
    const match = /^process-(\d+)-(cpu|mem|io)$/.exec(id);
    const pid = match ? Number(match[1]) : NaN;
    return Number.isSafeInteger(pid) && pid > 0 ? pid : undefined;
  };
  const retryHeaderState = () => { void loadHeaderState(true); };
  const modeName = mode === "game" ? "游戏" : mode === "power" ? "省电" : mode === "dev" ? "编码" : mode === "away" ? "离开" : mode === "media" ? "影音" : mode === "heavy" ? "重载" : mode === "none" ? "常规" : "状态未知";
  const modeTone = modeStatus !== "ready" ? (modeStatus === "error" ? "amber" : "slate") : mode === "game" || mode === "heavy" ? "rose" : mode === "power" ? "green" : mode === "dev" ? "cyan" : mode === "away" ? "violet" : mode === "media" ? "amber" : "slate";
  const diagReady = diag?.process_sample_ready === true;
  const diagBaselineStable = diagReady
    && diag?.cpu_baseline_warming_processes === 0
    && diag?.mem_baseline_warming_processes === 0
    && diag?.io_baseline_warming_processes === 0;
  const diagCount = diagReady ? diag?.anomalies.length ?? 0 : 0;
  const reportPreviewJson = useMemo(
    () => reportPreview
      ? JSON.stringify(redactDiagnosticReport(reportPreview, { includeDeviceIdentifiers, includeDetailedTelemetry }), null, 2)
      : "",
    [reportPreview, includeDeviceIdentifiers, includeDetailedTelemetry],
  );

  return (
    <div>
      <dialog
        ref={reportDialogRef}
        aria-labelledby="diagnostic-report-title"
        aria-describedby="diagnostic-report-description"
        onCancel={(event) => {
          event.preventDefault();
          if (!exportingReport) closeReportPreview();
        }}
        className="m-auto max-h-[90vh] w-[min(96vw,760px)] max-w-none overflow-hidden rounded-2xl border border-white/12 bg-slate-950 p-0 text-slate-100 shadow-2xl backdrop:bg-black/75 backdrop:backdrop-blur-sm"
      >
        <div className="max-h-[90vh] overflow-y-auto p-5 sm:p-6">
          <div>
            <h2 id="diagnostic-report-title" className="text-lg font-semibold text-slate-100">诊断报告预览与脱敏</h2>
            <p id="diagnostic-report-description" className="mt-2 text-sm leading-6 text-slate-300">
              报告只在本机生成；只有点击保存并选择路径后才会写入文件，AeroTune 不会上传。下方 JSON 是按当前选项处理后的最终内容。
            </p>
          </div>
          <div className="mt-4 space-y-3">
            <label className="flex cursor-pointer items-start gap-3 rounded-lg border border-white/8 bg-white/3 p-3">
              <input
                type="checkbox"
                checked={includeDeviceIdentifiers}
                disabled={exportingReport}
                onChange={(event) => setIncludeDeviceIdentifiers(event.target.checked)}
                className="mt-1 accent-cyan-400"
              />
              <span>
                <span className="block text-sm font-medium text-slate-200">包含设备标识（默认关闭）</span>
                <span className="mt-1 block text-xs leading-5 text-slate-400">主机名、CPU/GPU 型号及核心数；关闭时这些字段从文件中移除。</span>
              </span>
            </label>
            <label className="flex cursor-pointer items-start gap-3 rounded-lg border border-white/8 bg-white/3 p-3">
              <input
                type="checkbox"
                checked={includeDetailedTelemetry}
                disabled={exportingReport}
                onChange={(event) => setIncludeDetailedTelemetry(event.target.checked)}
                className="mt-1 accent-cyan-400"
              />
              <span>
                <span className="block text-sm font-medium text-slate-200">包含详细遥测（默认关闭）</span>
                <span className="mt-1 block text-xs leading-5 text-slate-400">精确时间、CPU/内存/进程数据、磁盘盘符与用量、网络流量、电池及 GPU 指标。</span>
              </span>
            </label>
          </div>
          <pre aria-label="脱敏后的诊断报告 JSON 预览" className="mt-4 max-h-72 overflow-auto whitespace-pre-wrap break-words rounded-lg border border-white/8 bg-black/25 p-3 text-[11px] leading-5 text-slate-300">{reportPreviewJson}</pre>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <Btn variant="ghost" onClick={closeReportPreview} disabled={exportingReport}>取消</Btn>
            <Btn onClick={saveDiagnosticReport} disabled={exportingReport}>
              <Download size={14} /> {exportingReport ? "保存中…" : "保存所选报告"}
            </Btn>
          </div>
        </div>
      </dialog>
      <PageHead
        title="仪表盘"
        desc={info ? `${info.host} · ${info.os} ${info.version} · ${info.cpu_model}` : infoStatus === "loading" ? "正在读取系统信息…" : "系统信息暂不可用"}
        right={
          <>
            <Chip tone={modeTone}>
              {modeStatus === "loading" ? "当前模式：读取中…" : modeStatus === "error" ? "当前模式：状态未知" : `当前模式：${modeName}`}
            </Chip>
            {(infoStatus === "error" || modeStatus === "error" || headerRetrying) && (
              <Btn variant="ghost" onClick={retryHeaderState} disabled={headerRetrying}>
                <RefreshCw size={14} className={headerRetrying ? "animate-spin" : ""} />
                {headerRetrying ? "读取中…" : "重试状态"}
              </Btn>
            )}
            <Btn variant="ghost" onClick={exportReport} disabled={exportingReport}>
              <Download size={14} /> {exportingReport ? "生成预览…" : "诊断报告预览"}
            </Btn>
            <Btn variant="ghost" onClick={flush}>
              刷新 DNS
            </Btn>
            <Btn onClick={smartTrim} disabled={busy}>
              <Zap size={14} /> 分析内存候选
            </Btn>
          </>
        }
      />

      <div className="mb-5">
        <SymptomGuide stats={stats} diagnostics={diag} recommendations={recommendations} recommendationsLoading={recommendationsLoading} recommendationsError={recommendationsError} onNav={onNav} />
      </div>

      {/* 健康评分 + 四个核心仪表 */}
      <div className="dashboard-metrics mb-5">
        <div className="dashboard-metrics-grid">
        <Card className="flex items-center justify-center py-6">
          <Ring
            value={diagReady ? diag?.score ?? null : null}
            label="系统健康评分"
            sub={
              !diagReady
                ? "诊断样本尚未就绪"
                : !diagBaselineStable
                  ? "进程基线仍在建立"
                  : diag && diag.deductions.length > 0
                    ? `-${diag.deductions.reduce((s, d) => s + d.cost, 0)} 扣分`
                    : "状态良好"
            }
            color={!diagReady ? "#94a3b8" : diag && diag.score >= 85 ? "#34d399" : diag && diag.score >= 65 ? "#fbbf24" : "#f87171"}
            color2={!diagReady ? "#64748b" : diag && diag.score >= 85 ? "#22d3ee" : diag && diag.score >= 65 ? "#f97316" : "#ef4444"}
          />
        </Card>
        <Card className="flex items-center justify-center py-6">
          <Ring value={stats ? stats.cpu : null} label="处理器" sub={stats ? (stats.cpu_freq_mhz > 0 ? `有效核心均频 ${stats.cpu_freq_mhz.toLocaleString()} MHz` : "频率暂不可用") : "等待遥测数据…"} />
        </Card>
        <Card className="flex items-center justify-center py-6">
          <Ring
            value={memPct}
            label="内存"
            color="#8b5cf6"
            color2="#22d3ee"
            sub={stats ? (validCapacityPair(stats.mem_used_gb, stats.mem_total_gb) ? `${fmtGb(stats.mem_used_gb)} 已用 / 总量 ${fmtGb(stats.mem_total_gb)}` : "内存数据暂不可用") : "等待遥测数据…"}
          />
        </Card>
        <Card className="flex items-center justify-center py-6">
          <Ring
            value={gpuUtil}
            label="显卡"
            color="#34d399"
            color2="#22d3ee"
            sub={gpuUtil == null ? (stats ? "未检测到实时数据" : "等待遥测数据…") : stats?.gpu?.temperature != null ? `${stats.gpu.temperature.toFixed(0)}°C · 3D 引擎` : "3D 引擎"}
          />
        </Card>
        <Card className="flex items-center justify-center py-6">
          <Ring
            value={cPct}
            label="C 盘占用"
            color="#fbbf24"
            color2="#f87171"
            sub={cDisk ? `${fmtGb(cDisk.used_gb)} / ${fmtGb(cDisk.total_gb)}` : stats ? "未检测到 C 盘数据" : "等待遥测数据…"}
          />
        </Card>
        </div>
      </div>

      <div className="mb-5">
        <CpuTopologyCard stats={stats} physicalCores={info?.cores ?? null} />
      </div>

      <div className="mb-5">
        <OptimizationLab stats={stats} statsStatus={statsStatus} />
      </div>

      <div className="mb-5 grid grid-cols-1 gap-4 2xl:grid-cols-3">
        {/* 网络走势 */}
        <Card
          className="min-w-0 2xl:col-span-2"
          title="网络实时吞吐"
          icon={<Activity size={15} className="text-cyan-300" />}
          right={
            <div className="flex items-center gap-3 text-xs tabular-nums">
              {stats ? <><span className="text-cyan-300">↓ {fmtBytes(stats.net_rx_kbps)}</span><span className="text-violet-300">↑ {fmtBytes(stats.net_tx_kbps)}</span></> : <span className="text-slate-500">等待遥测数据…</span>}
            </div>
          }
        >
          <div className="grid grid-cols-2 gap-3">
            <div className="h-24 rounded-lg bg-white/3 p-1">
              <Spark data={hist.rx} color="#22d3ee" />
            </div>
            <div className="h-24 rounded-lg bg-white/3 p-1">
              <Spark data={hist.tx} color="#8b5cf6" />
            </div>
          </div>
          <div className="mt-2 flex items-center justify-between text-[11px] text-slate-500">
            <span title="跨 AeroTune 重启保留，按本地自然日清零；仅统计程序运行期间观测到的网卡流量。">
              今日观测 ↓ {(stats?.net_today_rx_gb ?? 0).toFixed(2)} GB · ↑ {(stats?.net_today_tx_gb ?? 0).toFixed(2)} GB
            </span>
            <span>进程数 {stats?.process_count ?? "-"}</span>
            <span className="flex items-center gap-1">
              <Clock size={11} /> 开机 {stats ? fmtUptime(stats.uptime_secs) : "-"}
            </span>
          </div>
        </Card>

        {/* 电池 / 磁盘 */}
        <Card title="电源与磁盘" icon={<BatteryCharging size={15} className="text-emerald-300" />}>
          {batteryInfo ? (
            <div className="mb-4">
              <div className="mb-1.5 flex items-center justify-between text-sm">
                <span className="flex items-center gap-1.5 text-slate-300">
                  <BatteryCharging size={15} className={batteryAc === true ? "text-emerald-300" : batteryAc === false ? "text-amber-300" : "text-slate-500"} />
                  电池
                </span>
                <span className="tabular-nums text-slate-200">
                  {batteryInfo.percent != null ? `${batteryInfo.percent}%` : "未知"}
                  <span className="ml-1 text-[11px] text-slate-500">
                    {batteryAc === true ? "· 电源供电" : batteryAc === false ? "· 电池供电" : "· 电源状态未知"}
                  </span>
                </span>
              </div>
              <div className="h-2 w-full overflow-hidden rounded-full bg-white/8">
                <div
                  className={`h-full rounded-full ${batteryFill}`}
                  style={{ width: `${batteryInfo.percent ?? 0}%` }}
                />
              </div>
            </div>
          ) : (
            <div className="mb-4 text-xs text-slate-500">{!stats ? "等待电池遥测数据…" : stats.battery_status_ok === false ? "电池接口查询失败" : "未检测到电池"}</div>
          )}
          <div className="mb-4">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-xs text-slate-400">Windows 能效态</span>
              <Chip tone={effectivePowerMode.tone} role="status">{effectivePowerMode.label}</Chip>
            </div>
            <p className="mt-1 text-[10px] leading-4 text-slate-500">系统级策略信号，不代表 CPU 频率或性能实测。</p>
          </div>
          <div className="space-y-2">
            {(stats?.disks ?? []).slice(0, 3).map((d) => (
              <div key={d.mount}>
                <div className="mb-1 flex items-center justify-between text-xs text-slate-400">
                  <span className="flex items-center gap-1">
                    <HardDrive size={12} /> {d.mount}（{d.fs}）
                  </span>
                  <span className="tabular-nums">
                    {d.used_gb.toFixed(0)} / {d.total_gb.toFixed(0)} GB
                  </span>
                </div>
                <div className="h-1.5 overflow-hidden rounded-full bg-white/8">
                  <div
                    className="h-full rounded-full bg-gradient-to-r from-amber-300 to-rose-400"
                    style={{ width: `${d.total_gb > 0 ? (d.used_gb / d.total_gb) * 100 : 0}%` }}
                  />
                </div>
              </div>
            ))}
          </div>
        </Card>
      </div>

      <div className="mb-5">
        <DiskSpaceHistoryCard stats={stats} />
      </div>

      {/* 只读决策建议：点击只跳转到对应页面，不自动改变系统设置 */}
      <div className="mb-5">
        <Card
          title="智能建议 · 根据当前状态生成"
          icon={<Lightbulb size={15} className="text-cyan-300" />}
          right={
            <Btn variant="ghost" onClick={() => refreshRecommendations()} disabled={recommendationsLoading}>
              <RefreshCw size={14} className={recommendationsLoading ? "animate-spin" : ""} /> 重新评估
            </Btn>
          }
        >
          {recommendationsError && recommendations.length === 0 ? (
            <div className="py-3 text-sm text-amber-300">建议暂时未获取，点击“重新评估”重试。</div>
          ) : recommendations.length === 0 ? (
            <div className="py-3 text-sm text-slate-400">
              {recommendationsLoading ? "正在分析系统状态…" : "当前没有需要优先处理的问题。建议会随系统状态自动更新。"}
            </div>
          ) : (
            <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
              {recommendations.map((item) => (
                <div key={item.id} className="flex items-start gap-3 rounded-lg border border-white/8 bg-white/3 p-3.5">
                  <span aria-hidden="true" className="mt-0.5 text-base">{recommendationIcon(item.id)}</span>
                  <Chip tone={item.priority >= 80 ? "rose" : item.priority >= 50 ? "amber" : "cyan"}>
                    {item.priority >= 80 ? "优先" : item.priority >= 50 ? "关注" : "建议"}
                  </Chip>
                  <div className="min-w-0 flex-1">
                    <div className="text-sm font-medium text-slate-200">{item.title}</div>
                    <div className="mt-1 text-xs leading-5 text-slate-400">{item.reason}</div>
                  </div>
                  {isRecommendationPage(item.action) && (
                    <button type="button" onClick={() => onNav(item.action as PageId, item.action === "processes" ? recommendationPid(item.id) : undefined)}
                      className="shrink-0 rounded-md p-1.5 text-cyan-300 hover:bg-white/8"
                      title="查看相关页面" aria-label={`查看${item.title}`}>
                      <ArrowRight size={16} />
                    </button>
                  )}
                </div>
              ))}
            </div>
          )}
          <div className="mt-3 text-[11px] text-slate-500">建议只提供诊断和导航，是否执行操作由你决定。</div>
        </Card>
      </div>

      <div className="mb-5">
        <ReliabilityTimeline />
      </div>

      {/* 智能诊断：异常进程 */}
      <div className="mb-5">
        <Card
          title="智能诊断 · 异常进程（滚动 median/MAD 基线）"
          icon={<ShieldAlert size={15} className="text-amber-300" />}
          right={
            <Chip tone={diagStatus === "error" || diagStatus === "stale" ? "amber" : diagStatus === "loading" || !diagReady || (diagCount === 0 && !diagBaselineStable) ? "slate" : diagCount > 0 ? "rose" : "green"}>
              {diagStatus === "loading" ? "正在分析…" : diagStatus === "error" ? "诊断不可用" : diagStatus === "stale" ? "更新失败 · 上次结果" : !diagReady ? "等待诊断样本" : diagCount > 0 ? `${diagCount} 个异常` : diagBaselineStable ? "运行平稳" : "基线建立中"}
            </Chip>
          }
        >
          {diagStatus === "loading" && !diag && (
            <div role="status" className="py-4 text-center text-[13px] text-slate-400">正在读取诊断数据…</div>
          )}
          {diagStatus === "error" && !diag && (
            <div role="alert" className="py-4 text-center text-[13px] text-amber-200">诊断暂不可用，系统将稍后重试。</div>
          )}
          {diagStatus === "stale" && diag && (
            <div role="status" className="mb-3 rounded-lg border border-amber-300/15 bg-amber-300/5 px-3 py-2 text-xs text-amber-100">诊断刷新失败，当前显示上次成功快照。</div>
          )}
          {diag && !diagReady && (
            <div role="status" className="py-4 text-center text-[13px] text-slate-400">诊断样本尚未就绪，暂不判定为运行平稳。</div>
          )}
          {diagReady && diagCount === 0 && (
            <div className="py-4 text-center text-[13px] text-slate-500">
              {diagBaselineStable ? "CPU / 内存 / 磁盘 IO 三维基线均平稳，未检测到异常进程" : "部分进程基线仍在建立或不可用，暂不判定为运行平稳"}
            </div>
          )}
          {diagReady && diagCount > 0 && (
            <div className="space-y-1.5">
              {diag.anomalies.map((a) => {
                const meta = KIND_META[a.kind] ?? KIND_META.cpu;
                const context = anomalyContext?.find((entry) => entry.pid === a.pid && entry.kind === a.kind);
                const hintText = context?.hints.map((hint) => hint.text).join("；") ?? null;
                return (
                  <div key={a.kind + a.pid} className="rounded-lg border border-white/8 bg-white/3">
                    <div className="flex items-center gap-3 px-3.5 py-2">
                      <ShieldAlert size={15} className="shrink-0 text-slate-400" />
                      <span className="min-w-0 flex-1 truncate text-[13px] text-slate-200">
                        {a.name} <span className="text-slate-500">· PID {a.pid}</span>
                      </span>
                      <Chip tone={meta.tone}>{meta.label}</Chip>
                      <span className="w-56 text-right text-[12px] tabular-nums text-slate-300" title="偏离值以 1.4826 × MAD 估算尺度归一化，不表示概率">
                        {fmtAnomaly(a)}
                      </span>
                      <button
                        type="button"
                        aria-label={`查看进程 ${a.name}，PID ${a.pid}，前往列表核验当前实例`}
                        className="rounded-md p-1.5 text-slate-400 hover:bg-white/10 hover:text-cyan-300"
                        title="进程诊断可能滞后；前往进程列表核验当前实例后再操作"
                        onClick={() => onNav("processes", a.pid)}
                      >
                        <ArrowRight size={14} />
                      </button>
                    </div>
                    {hintText && (
                      <div className="border-t border-white/6 px-3.5 py-1.5 text-[11px] leading-5 text-slate-400">
                        {context?.age_secs != null && (
                          <span className="mr-1 tabular-nums text-slate-500">起于 {onsetAgeLabel(context.age_secs)}前 ·</span>
                        )}
                        {hintText}
                        {context && <AnomalyTimeline context={context} />}
                      </div>
                    )}
                  </div>
                );
              })}
              {anomalyContext && anomalyContext.length > 0 && (
                <div className="px-1 text-[11px] text-slate-500">
                  根因提示由本地事件时间关联生成（进程启动、变更日志、引擎切换），仅提示可能相关，不构成因果结论。
                </div>
              )}
            </div>
          )}
        </Card>
      </div>

      {/* 场景快捷入口 */}
      <div className="grid grid-cols-1 gap-4 lg:grid-cols-3">
        {(
          [
            { id: "game", title: "游戏模式", desc: "高性能计划 · 关 GameDVR · 释放内存", cls: "hover:border-rose-400/40" },
            { id: "power", title: "省电续航", desc: "节能计划 · 降低后台功耗", cls: "hover:border-emerald-400/40" },
            { id: "dev", title: "编码模式", desc: "平衡计划 · 前台响应优先", cls: "hover:border-cyan-400/40" },
          ] as const
        ).map((s) => (
          <button
            key={s.id}
            onClick={() => onNav(s.id)}
            className={`glass group p-4 text-left transition-all hover:bg-white/8 ${s.cls}`}
          >
            <div className="mb-1 flex items-center gap-2 text-sm font-medium text-slate-200">
              {s.id === "game" ? (
                <MonitorSmartphone size={16} className="text-rose-300" />
              ) : s.id === "power" ? (
                <BatteryCharging size={16} className="text-emerald-300" />
              ) : (
                <Cpu size={16} className="text-cyan-300" />
              )}
              {s.title}
            </div>
            <div className="text-xs text-slate-400">{s.desc}</div>
          </button>
        ))}
      </div>
    </div>
  );
}
