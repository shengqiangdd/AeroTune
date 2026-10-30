import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Activity, CheckCircle2, Clock, Cpu, Info, RotateCcw, Trash2, TrendingUp } from "lucide-react";
import { api, ExperimentConsistencyReport, ExperimentRecordInput } from "../lib/api";
import type { Stats } from "../lib/api";
import type { StatsStatus } from "../hooks/useStats";
import { effectivePowerModePresentation } from "../lib/effectivePowerMode.js";
import {
  isPowerContextSummary,
  powerSourceChanged,
  stablePowerSource,
  summarizePowerContext,
} from "../lib/experimentPowerContext.js";
import type { PowerContextSummary, PowerSource } from "../lib/experimentPowerContext.js";
import {
  analyzerCoverage,
  captureExperimentMetrics,
  hasTrustworthyExperimentCoverage,
} from "../lib/experimentMetrics.js";
import type { ExperimentMetricSample } from "../lib/experimentMetrics.js";
import { Btn, Card, Chip, PageHead } from "./ui";

const WINDOW_MS = 60_000;
const HISTORY_LIMIT = 8;
const HISTORY_KEY = "aerotune.optimization-lab.history.v1";
const DRAFT_KEY = "aerotune.optimization-lab.draft.v1";
const DRAFT_TTL_MS = 2 * 60 * 60 * 1000;

type ModeKey = "game" | "power" | "dev";
type CapturePhase = "baseline" | "apply" | "after";
type RecordStatus = "complete" | "interrupted";

type MetricSample = ExperimentMetricSample;

type ActiveSession = {
  mode: ModeKey;
  phase: CapturePhase;
  baselineStartedAt: number;
  baselineEndsAt: number;
  baselineDurationMs: number;
  afterStartedAt: number | null;
  afterEndsAt: number | null;
  afterDurationMs: number;
  baselineSamples: MetricSample[];
  afterSamples: MetricSample[];
  lastSampleId: number | null;
  draftUpdatedAt?: number;
};

type WindowSummary = {
  durationMs: number;
  sampleCount: number;
  cpuPct: number | null;
  memoryUsedGb: number | null;
  memoryPct: number | null;
  gpuPct: number | null;
  gpuMemoryMb: number | null;
  rxKbps: number | null;
  txKbps: number | null;
  coverage: {
    cpu: number;
    memory: number;
    memoryPct: number;
    gpu: number;
    gpuMemory: number;
    rx: number;
    tx: number;
  };
  /** Missing on records created before power-context summaries existed. */
  powerContext?: PowerContextSummary;
};

type ExperimentRecord = {
  id: string;
  ts: number;
  mode: ModeKey;
  status: RecordStatus;
  interruptionReason?: "page-hidden" | "page-hide" | "unmount" | "manual" | "telemetry-unavailable" | "insufficient-coverage";
  baseline: WindowSummary;
  after: WindowSummary | null;
};

const MODE_LABEL: Record<ModeKey, string> = {
  game: "游戏模式",
  power: "省电模式",
  dev: "编码模式",
};

const isFiniteNumber = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value);
const isMetricInRange = (value: unknown, minimum: number, maximum = Number.POSITIVE_INFINITY) =>
  value === null || (isFiniteNumber(value) && value >= minimum && value <= maximum);
const METRIC_BOUNDS: Partial<Record<keyof MetricSample, [number, number]>> = {
  cpuPct: [0, 100],
  memoryUsedGb: [0, Number.POSITIVE_INFINITY],
  memoryPct: [0, 100],
  gpuPct: [0, 100],
  gpuMemoryMb: [0, Number.POSITIVE_INFINITY],
  rxKbps: [0, Number.POSITIVE_INFINITY],
  txKbps: [0, Number.POSITIVE_INFINITY],
};
const isValidSampleMetric = (key: keyof MetricSample, value: unknown): value is number => {
  const bounds = METRIC_BOUNDS[key];
  return bounds != null && isFiniteNumber(value) && value >= bounds[0] && value <= bounds[1];
};

function isWindowSummary(value: unknown): value is WindowSummary {
  if (!value || typeof value !== "object") return false;
  const summary = value as Partial<WindowSummary>;
  const coverage = summary.coverage;
  const sampleCount = summary.sampleCount;
  const coverageCounts = coverage ? [coverage.cpu, coverage.memory, coverage.memoryPct, coverage.gpu, coverage.gpuMemory, coverage.rx, coverage.tx] : [];
  const coverageCountsValid = isFiniteNumber(sampleCount) && Number.isSafeInteger(sampleCount) && sampleCount >= 0
    ? coverageCounts.every((count) => Number.isSafeInteger(count) && count >= 0 && count <= sampleCount)
    : false;
  const powerContext = summary.powerContext;
  const powerContextIsValid = powerContext == null || (
    isPowerContextSummary(powerContext) && powerContext.sampleCount === summary.sampleCount
  );
  return isFiniteNumber(summary.durationMs) && summary.durationMs >= 0 &&
    isFiniteNumber(sampleCount) && Number.isSafeInteger(sampleCount) && sampleCount >= 0 &&
    isMetricInRange(summary.cpuPct, 0, 100) && isMetricInRange(summary.memoryUsedGb, 0) &&
    isMetricInRange(summary.memoryPct, 0, 100) && isMetricInRange(summary.gpuPct, 0, 100) &&
    isMetricInRange(summary.gpuMemoryMb, 0) && isMetricInRange(summary.rxKbps, 0) && isMetricInRange(summary.txKbps, 0) &&
    !!coverage && coverageCountsValid &&
    powerContextIsValid;
}

function readLocalHistory(): ExperimentRecord[] {
  if (typeof window === "undefined") return [];
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(HISTORY_KEY) ?? "[]");
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter((value): value is ExperimentRecord => {
        if (!value || typeof value !== "object") return false;
        const item = value as Partial<ExperimentRecord>;
        return typeof item.id === "string" && isFiniteNumber(item.ts) &&
          (item.mode === "game" || item.mode === "power" || item.mode === "dev") &&
          (item.status === "complete" || item.status === "interrupted") &&
          isWindowSummary(item.baseline) && (item.after == null || isWindowSummary(item.after));
      })
      .slice(0, HISTORY_LIMIT);
  } catch {
    return [];
  }
}

function readDraft(): ActiveSession | null {
  if (typeof window === "undefined") return null;
  try {
    const parsed: unknown = JSON.parse(window.sessionStorage.getItem(DRAFT_KEY) ?? "null");
    if (!parsed || typeof parsed !== "object") return null;
    const draft = parsed as ActiveSession;
    if (
      draft.phase !== "apply" ||
      (draft.mode !== "game" && draft.mode !== "power" && draft.mode !== "dev") ||
      !Array.isArray(draft.baselineSamples) ||
      !isFiniteNumber(draft.draftUpdatedAt) ||
      Date.now() - draft.draftUpdatedAt > DRAFT_TTL_MS
    ) {
      window.sessionStorage.removeItem(DRAFT_KEY);
      return null;
    }
    return draft;
  } catch {
    return null;
  }
}

function clearDraft(): void {
  try {
    window.sessionStorage.removeItem(DRAFT_KEY);
  } catch {
    // Session storage may be disabled; the in-memory workflow still works.
  }
}

function persistDraft(session: ActiveSession): void {
  if (session.phase !== "apply") return;
  try {
    window.sessionStorage.setItem(
      DRAFT_KEY,
      JSON.stringify({ ...session, draftUpdatedAt: Date.now() }),
    );
  } catch {
    // Keep the live experiment usable if storage is unavailable.
  }
}

function average(samples: MetricSample[], key: keyof MetricSample): { value: number | null; count: number } {
  let total = 0;
  let count = 0;
  for (const sample of samples) {
    const value = sample[key];
    if (isValidSampleMetric(key, value)) {
      total += value;
      count += 1;
    }
  }
  return { value: count ? total / count : null, count };
}

function summarize(samples: MetricSample[], durationMs: number): WindowSummary {
  const cpu = average(samples, "cpuPct");
  const memory = average(samples, "memoryUsedGb");
  const memoryPct = average(samples, "memoryPct");
  const gpu = average(samples, "gpuPct");
  const gpuMemory = average(samples, "gpuMemoryMb");
  const rx = average(samples, "rxKbps");
  const tx = average(samples, "txKbps");
  return {
    durationMs: Math.max(0, durationMs),
    sampleCount: samples.length,
    cpuPct: cpu.value,
    memoryUsedGb: memory.value,
    memoryPct: memoryPct.value,
    gpuPct: gpu.value,
    gpuMemoryMb: gpuMemory.value,
    rxKbps: rx.value,
    txKbps: tx.value,
    coverage: {
      cpu: cpu.count,
      memory: memory.count,
      memoryPct: memoryPct.count,
      gpu: gpu.count,
      gpuMemory: gpuMemory.count,
      rx: rx.count,
      tx: tx.count,
    },
    powerContext: summarizePowerContext(samples.map((sample) => sample.powerContext)),
  };
}

function createRecord(
  session: ActiveSession,
  now: number,
  status: RecordStatus,
  interruptionReason?: ExperimentRecord["interruptionReason"],
): ExperimentRecord {
  const baselineDuration = session.phase === "baseline"
    ? Math.min(WINDOW_MS, Math.max(0, now - session.baselineStartedAt))
    : session.baselineDurationMs;
  const afterDuration = session.phase === "after" && session.afterStartedAt != null
    ? Math.min(WINDOW_MS, Math.max(0, now - session.afterStartedAt))
    : session.afterDurationMs;
  return {
    id: `${now}-${Math.random().toString(36).slice(2, 8)}`,
    ts: now,
    mode: session.mode,
    status,
    interruptionReason,
    baseline: summarize(session.baselineSamples, baselineDuration),
    after: session.afterSamples.length
      ? summarize(session.afterSamples, afterDuration)
      : null,
  };
}

function formatNumber(value: number | null, digits = 1): string {
  return value == null || !Number.isFinite(value) ? "—" : value.toFixed(digits);
}

function formatSigned(value: number | null, digits = 1): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return `${value > 0 ? "+" : value < 0 ? "−" : ""}${Math.abs(value).toFixed(digits)}`;
}

function formatDuration(ms: number): string {
  const seconds = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

function sensorNotes(summary: WindowSummary): string[] {
  const n = summary.sampleCount;
  if (!n) return ["此窗口没有收到有效遥测样本。"];
  const notes: string[] = [];
  const note = (label: string, count: number) => {
    if (count < n) notes.push(`${label}：${count}/${n} 个样本可用`);
  };
  note("CPU", summary.coverage.cpu);
  note("内存用量", summary.coverage.memory);
  note("内存百分比", summary.coverage.memoryPct);
  note("GPU 利用率", summary.coverage.gpu);
  note("GPU 显存", summary.coverage.gpuMemory);
  note("网络接收", summary.coverage.rx);
  note("网络发送", summary.coverage.tx);
  return notes;
}

function modeName(mode: ModeKey): string {
  return MODE_LABEL[mode];
}

function ComparisonTable({ baseline, after }: { baseline: WindowSummary; after: WindowSummary | null }) {
  const rows = [
    { label: "CPU 平均利用率", before: baseline.cpuPct, current: after?.cpuPct ?? null, unit: "%", digits: 1 },
    {
      label: "内存平均用量",
      before: baseline.memoryUsedGb,
      current: after?.memoryUsedGb ?? null,
      unit: "GB",
      digits: 2,
      beforeDetail: baseline.memoryPct,
      currentDetail: after?.memoryPct ?? null,
    },
    { label: "GPU 平均利用率", before: baseline.gpuPct, current: after?.gpuPct ?? null, unit: "%", digits: 1 },
    { label: "GPU 显存平均用量", before: baseline.gpuMemoryMb, current: after?.gpuMemoryMb ?? null, unit: "MB", digits: 0 },
    { label: "网络平均接收速率", before: baseline.rxKbps, current: after?.rxKbps ?? null, unit: "KB/s", digits: 1 },
    { label: "网络平均发送速率", before: baseline.txKbps, current: after?.txKbps ?? null, unit: "KB/s", digits: 1 },
  ];

  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[540px] text-left text-[12px]" aria-label="基线与调优后观测值对比">
        <thead>
          <tr className="border-b border-white/8 text-[11px] text-slate-500">
            <th scope="col" className="py-2 pr-3 font-medium">指标</th>
            <th scope="col" className="py-2 px-2 font-medium">基线均值</th>
            <th scope="col" className="py-2 px-2 font-medium">调优后均值</th>
            <th scope="col" className="py-2 pl-2 font-medium">观测差值</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => {
            const delta = row.before != null && row.current != null ? row.current - row.before : null;
            const before = row.before == null
              ? "—"
              : `${formatNumber(row.before, row.digits)} ${row.unit}${"beforeDetail" in row && row.beforeDetail != null ? ` · ${formatNumber(row.beforeDetail)}%` : ""}`;
            const current = row.current == null
              ? "—"
              : `${formatNumber(row.current, row.digits)} ${row.unit}${"currentDetail" in row && row.currentDetail != null ? ` · ${formatNumber(row.currentDetail)}%` : ""}`;
            const deltaText = delta == null
              ? "—"
              : `${formatSigned(delta, row.digits)} ${row.unit}${"beforeDetail" in row && row.beforeDetail != null && row.currentDetail != null ? ` · ${formatSigned(row.currentDetail - row.beforeDetail)}%` : ""}`;
            return (
              <tr key={row.label} className="border-b border-white/5 last:border-0">
                <th scope="row" className="py-2.5 pr-3 font-medium text-slate-300">{row.label}</th>
                <td className="py-2.5 px-2 tabular-nums text-slate-400">{before}</td>
                <td className="py-2.5 px-2 tabular-nums text-slate-200">{current}</td>
                <td className="py-2.5 pl-2 tabular-nums text-cyan-200">{deltaText}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function WindowNotes({ label, summary }: { label: string; summary: WindowSummary }) {
  const notes = sensorNotes(summary);
  return (
    <div className="mt-2 text-[11px] text-slate-500">
      <span className="font-medium text-slate-400">{label}：</span>
      {notes.length ? notes.join("；") : "传感器均有数据"}
    </div>
  );
}

const POWER_SOURCE_LABEL: Record<PowerSource, string> = {
  ac: "AC 供电",
  battery: "电池供电",
  unknown: "未知供电",
};

function powerModeDistributionText(context: PowerContextSummary): string {
  const distribution = context.effectivePowerModeCounts.map(({ mode, count }) =>
    `${effectivePowerModePresentation(mode).label} ${count}`,
  );
  if (context.effectivePowerModeUnknownCount > 0) {
    distribution.push(`未知/暂不可用 ${context.effectivePowerModeUnknownCount}`);
  }
  return distribution.length ? distribution.join(" · ") : "未知/暂不可用";
}

function formatPowerContext(context: PowerContextSummary | null | undefined): string {
  if (!context) return "旧记录无电源上下文";
  if (context.sampleCount === 0) return "无上下文样本";
  const counts = context.powerSourceCounts;
  return `AC ${counts.ac} / 电池 ${counts.battery} / 未知 ${counts.unknown} · ${powerModeDistributionText(context)}`;
}

function WindowPowerContext({
  label,
  context,
}: {
  label: string;
  context: PowerContextSummary | null | undefined;
}) {
  if (!context) {
    return <div className="text-[10px] leading-4 text-slate-600">{label}：旧记录无该上下文</div>;
  }
  if (context.sampleCount === 0) {
    return <div className="text-[10px] leading-4 text-slate-600">{label}：没有电源上下文样本</div>;
  }
  return (
    <div className="rounded-lg border border-white/6 bg-white/3 p-2 text-[10px] leading-4 text-slate-500">
      <div className="font-medium text-slate-400">{label} · {context.sampleCount} 个上下文样本</div>
      <div>
        供电：AC {context.powerSourceCounts.ac} · 电池 {context.powerSourceCounts.battery} · 未知 {context.powerSourceCounts.unknown}
      </div>
      <div>Windows 能效态：{powerModeDistributionText(context)}</div>
    </div>
  );
}

function PowerSourceComparison({
  baseline,
  after,
}: {
  baseline: PowerContextSummary | null | undefined;
  after: PowerContextSummary | null | undefined;
}) {
  if (!powerSourceChanged(baseline, after)) return null;
  const beforeSource = stablePowerSource(baseline);
  const afterSource = stablePowerSource(after);
  if (!beforeSource || !afterSource) return null;
  return (
    <p role="note" className="mt-2 rounded-lg border border-amber-300/12 bg-amber-300/5 px-3 py-2 text-[11px] leading-relaxed text-amber-100/80">
      基线稳定为{POWER_SOURCE_LABEL[beforeSource]}，调优后稳定为{POWER_SOURCE_LABEL[afterSource]}；供电上下文不同，结果仅作观察差异，不应据此归因于调优模式。
    </p>
  );
}

const ExperimentHistoryView = memo(function ExperimentHistoryView({
  history,
  consistency,
  storageWarning,
  onDeleteRecord,
  onResetHistory,
}: {
  history: ExperimentRecord[];
  consistency: ExperimentConsistencyReport | null;
  storageWarning: boolean;
  onDeleteRecord: (id: string) => void;
  onResetHistory: () => void;
}) {
  const latest = history[0];
  return (
    <>
      {latest && (
        <Card className="mb-4" title="最近一次实验" icon={<Activity size={15} className="text-emerald-300" />} right={<Chip tone={latest.status === "complete" ? "green" : "amber"}>{latest.status === "complete" ? "已完成" : "未完成"}</Chip>}>
          <div className="mb-3 flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-slate-500">
            <span>{new Date(latest.ts).toLocaleString()}</span>
            <span>比较模式：{modeName(latest.mode)}</span>
            <span>基线 {latest.baseline.sampleCount} 样本 / {formatDuration(latest.baseline.durationMs)}</span>
            {latest.after && <span>调优后 {latest.after.sampleCount} 样本 / {formatDuration(latest.after.durationMs)}</span>}
          </div>
          <ComparisonTable baseline={latest.baseline} after={latest.after} />
          <div className="mt-3 grid gap-1 sm:grid-cols-2">
            <WindowNotes label="基线传感器备注" summary={latest.baseline} />
            {latest.after && <WindowNotes label="调优后传感器备注" summary={latest.after} />}
          </div>
          <div className="mt-3 grid gap-2 sm:grid-cols-2">
            <WindowPowerContext label="基线" context={latest.baseline.powerContext} />
            {latest.after && <WindowPowerContext label="调优后" context={latest.after.powerContext} />}
          </div>
          {latest.after && (
            <PowerSourceComparison baseline={latest.baseline.powerContext} after={latest.after.powerContext} />
          )}
          <p className="mt-3 border-t border-white/6 pt-3 text-[11px] leading-relaxed text-slate-500">
            这是两个采样窗口的观察差异，不表示模式导致了变化；工作负载、温度、后台活动与传感器可用性都会影响结果。此测量不承诺 FPS 或功耗收益。
          </p>
        </Card>
      )}

      <Card
        title={`本地历史（最多 ${HISTORY_LIMIT} 条）`}
        icon={<Clock size={15} className="text-slate-300" />}
        right={history.length > 0 && <Btn variant="ghost" className="px-2.5 py-1.5 text-xs" onClick={onResetHistory}><RotateCcw size={13} /> 清空历史</Btn>}
      >
        {storageWarning && <p role="status" className="mb-3 text-[11px] text-amber-300">浏览器存储不可用；本次数据仅保留在当前页面内存中。</p>}
        {history.length === 0 ? (
          <p className="text-[12px] text-slate-500">尚无实验记录。完成两段 60 秒窗口后，汇总会保存在此设备的本地存储中。</p>
        ) : (
          <ul className="space-y-2">
            {history.map((record) => (
              <li key={record.id} className="flex flex-wrap items-center gap-x-3 gap-y-2 rounded-lg border border-white/6 bg-white/3 px-3 py-2.5">
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2 text-[12px] text-slate-200">
                    <span>{modeName(record.mode)}</span>
                    <Chip tone={record.status === "complete" ? "green" : "amber"}>{record.status === "complete" ? "已完成" : "中断"}</Chip>
                  </div>
                  <div className="mt-1 text-[11px] text-slate-500">
                    {new Date(record.ts).toLocaleString()} · 基线 {record.baseline.sampleCount} 样本 / {formatDuration(record.baseline.durationMs)}
                    {record.after ? ` · 调优后 ${record.after.sampleCount} 样本 / ${formatDuration(record.after.durationMs)}` : " · 调优后窗口无完整记录"}
                  </div>
                  <div className="mt-1 break-words text-[10px] leading-4 text-slate-600">
                    基线电源上下文：{formatPowerContext(record.baseline.powerContext)}
                    {record.after ? ` · 调优后：${formatPowerContext(record.after.powerContext)}` : " · 调优后上下文无完整记录"}
                  </div>
                </div>
                <button
                  type="button"
                  aria-label={`删除 ${new Date(record.ts).toLocaleString()} 的实验记录`}
                  onClick={() => onDeleteRecord(record.id)}
                  className="rounded-md p-2 text-slate-500 transition-colors hover:bg-rose-400/10 hover:text-rose-300 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70"
                >
                  <Trash2 size={14} />
                </button>
              </li>
            ))}
          </ul>
        )}
        <p className="mt-3 text-[10px] leading-4 text-slate-600">本地历史仅保存聚合指标、电源来源与 Windows 能效态计数、窗口时长、样本数和模式名称；上限 {HISTORY_LIMIT} 条，可逐条删除或清空。待续草稿临时保存在当前会话存储，自动过期且不含进程名称、路径或设备标识。</p>
      </Card>

      {consistency && consistency.modes.length > 0 && (
        <Card
          className="mt-4"
          title="实验一致性观察 · 跨多次实验的方向分析"
          icon={<TrendingUp size={15} className="text-emerald-300" />}
          right={<Chip tone={consistency.verdict === "ready" ? "green" : consistency.verdict === "inconsistent" ? "amber" : "slate"}>
            {consistency.verdict === "ready" ? "存在一致方向" : consistency.verdict === "inconsistent" ? "方向不一致" : "样本不足"}
          </Chip>}
        >
          <div className="space-y-1.5">
            {consistency.modes.map((mode) => (
              <div key={mode.mode} className="flex flex-wrap items-center gap-3 rounded-lg border border-white/8 bg-white/3 px-3.5 py-2 text-[12px]">
                <span className="min-w-0 flex-1 text-slate-200">{modeName(mode.mode as ModeKey)}</span>
                <span className="tabular-nums text-slate-500">{mode.analyzed} 次可信实验</span>
                <span className="tabular-nums text-slate-400">
                  CPU {mode.cpu_direction === "lower" ? "一致更低" : mode.cpu_direction === "higher" ? "一致更高" : mode.cpu_direction === "flat" ? "变化不明显" : "方向不一致"}
                  {mode.cpu_median_delta != null ? `（中位 ${mode.cpu_median_delta > 0 ? "+" : ""}${mode.cpu_median_delta.toFixed(1)} pct）` : ""}
                </span>
                <span className="tabular-nums text-slate-400">
                  内存 {mode.mem_direction === "lower" ? "一致更低" : mode.mem_direction === "higher" ? "一致更高" : mode.mem_direction === "flat" ? "变化不明显" : "方向不一致"}
                  {mode.mem_median_delta != null ? `（中位 ${mode.mem_median_delta > 0 ? "+" : ""}${mode.mem_median_delta.toFixed(2)} GB）` : ""}
                </span>
              </div>
            ))}
          </div>
          <p className="mt-3 border-t border-white/6 pt-3 text-[11px] leading-relaxed text-slate-500">
            {consistency.summary} 方向与幅度是多次本地实验的聚合观察，不构成因果结论，也不作为任何自动调参的依据。
          </p>
        </Card>
      )}
    </>
  );
});

export function OptimizationLab({ stats, statsStatus }: { stats: Stats | null; statsStatus: StatsStatus }) {
  const initialSession = useMemo(() => readDraft(), []);
  const [session, setSession] = useState<ActiveSession | null>(initialSession);
  const sessionRef = useRef<ActiveSession | null>(initialSession);
  const [history, setHistory] = useState<ExperimentRecord[]>(readLocalHistory);
  const historyRef = useRef(history);
  const [mode, setMode] = useState<ModeKey>("game");
  const [secondsLeft, setSecondsLeft] = useState(60);
  const [notice, setNotice] = useState("");
  const [storageWarning, setStorageWarning] = useState(false);
  const [consistency, setConsistency] = useState<ExperimentConsistencyReport | null>(null);

  const saveRecord = useCallback((record: ExperimentRecord, updateUi = true) => {
    const next = [record, ...historyRef.current].slice(0, HISTORY_LIMIT);
    historyRef.current = next;
    try {
      window.localStorage.setItem(HISTORY_KEY, JSON.stringify(next));
      if (updateUi) setStorageWarning(false);
    } catch {
      if (updateUi) setStorageWarning(true);
    }
    if (updateUi) setHistory(next);
  }, []);

  const setActiveSession = useCallback((next: ActiveSession | null) => {
    sessionRef.current = next;
    setSession(next);
  }, []);

  // 实验一致性影子分析：历史变化时把本地记录交给后端纯函数分析。
  // 无状态、不落盘；结论仅供观察，AeroTune 不据此调整任何参数。
  useEffect(() => {
    let disposed = false;
    const payload: ExperimentRecordInput[] = history.map((record) => ({
      mode: record.mode,
      status: record.status,
      baseline: {
        duration_ms: record.baseline.durationMs,
        sample_count: record.baseline.sampleCount,
        cpu_pct: record.baseline.cpuPct,
        memory_used_gb: record.baseline.memoryUsedGb,
        coverage: analyzerCoverage(record.baseline),
      },
      after: record.after
        ? {
            duration_ms: record.after.durationMs,
            sample_count: record.after.sampleCount,
            cpu_pct: record.after.cpuPct,
            memory_used_gb: record.after.memoryUsedGb,
            coverage: analyzerCoverage(record.after),
          }
        : null,
    }));
    api.analyzeExperimentConsistency(payload)
      .then((report) => { if (!disposed) setConsistency(report); })
      .catch(() => { if (!disposed) setConsistency(null); });
    return () => { disposed = true; };
  }, [history]);

  const interruptSession = useCallback((reason: NonNullable<ExperimentRecord["interruptionReason"]>, updateUi = true) => {
    const current = sessionRef.current;
    if (!current) return;
    if (current.phase === "apply" && reason === "unmount") {
      persistDraft(current);
      return;
    }
    sessionRef.current = null;
    clearDraft();
    saveRecord(createRecord(current, Date.now(), "interrupted", reason), updateUi);
    if (updateUi) {
      setSession(null);
      const message = reason === "telemetry-unavailable"
        ? "实时遥测未保持在线；已将窗口标记为中断，不会纳入一致性分析。"
        : reason === "insufficient-coverage"
          ? "窗口有效样本覆盖不足；已标记为中断，不会把稀疏样本当作完整测量。"
          : "实验已中断；已将已有窗口与样本数保存在本地历史中。可重新开始完整测量。";
      setNotice(message);
    }
  }, [saveRecord]);

  useEffect(() => {
    const current = sessionRef.current;
    if (statsStatus !== "live" && current && (current.phase === "baseline" || current.phase === "after")) {
      interruptSession("telemetry-unavailable");
    }
  }, [statsStatus, interruptSession]);

  useEffect(() => {
    if (!stats || statsStatus !== "live") return;
    const current = sessionRef.current;
    if (!current || (current.phase !== "baseline" && current.phase !== "after")) return;
    if (typeof document !== "undefined" && document.hidden) return;
    if (current.lastSampleId === stats.sample_id) return;
    const now = Date.now();
    const endAt = current.phase === "baseline" ? current.baselineEndsAt : current.afterEndsAt;
    if (endAt == null || now > endAt) return;

    const next: ActiveSession = current.phase === "baseline"
      ? { ...current, baselineSamples: [...current.baselineSamples, captureExperimentMetrics(stats)], lastSampleId: stats.sample_id }
      : { ...current, afterSamples: [...current.afterSamples, captureExperimentMetrics(stats)], lastSampleId: stats.sample_id };
    setActiveSession(next);
  }, [stats, statsStatus, setActiveSession]);

  useEffect(() => {
    if (!session || (session.phase !== "baseline" && session.phase !== "after")) return;
    const timer = window.setInterval(() => {
      const current = sessionRef.current;
      if (!current || (current.phase !== "baseline" && current.phase !== "after")) return;
      const now = Date.now();
      const endAt = current.phase === "baseline" ? current.baselineEndsAt : current.afterEndsAt;
      if (endAt == null) return;
      const remaining = Math.max(0, Math.ceil((endAt - now) / 1000));
      setSecondsLeft((previous) => previous === remaining ? previous : remaining);
      if (now < endAt) return;

      if (current.phase === "baseline") {
        const baselineSummary = summarize(current.baselineSamples, WINDOW_MS);
        if (!hasTrustworthyExperimentCoverage(baselineSummary)) {
          interruptSession("insufficient-coverage");
          return;
        }
        const next: ActiveSession = { ...current, phase: "apply", baselineDurationMs: WINDOW_MS };
        setActiveSession(next);
        setSecondsLeft(60);
        setNotice(`基线窗口完成（${current.baselineSamples.length} 个样本）。现在请手动应用「${modeName(current.mode)}」，再返回此处开始调优后窗口。`);
        return;
      }

      const afterSummary = summarize(current.afterSamples, WINDOW_MS);
      if (!hasTrustworthyExperimentCoverage(afterSummary)) {
        interruptSession("insufficient-coverage");
        return;
      }
      clearDraft();
      saveRecord(createRecord(current, now, "complete"));
      setActiveSession(null);
      setSecondsLeft(60);
      setNotice("两段窗口采集完成；历史中显示的是观测差异，不代表该模式造成了变化。");
    }, 250);
    return () => window.clearInterval(timer);
  }, [session?.phase, saveRecord, setActiveSession, interruptSession]);

  useEffect(() => {
    const onVisibilityChange = () => {
      if (document.visibilityState === "hidden") {
        const current = sessionRef.current;
        if (current?.phase === "apply") persistDraft(current);
        else interruptSession("page-hidden");
      }
    };
    const onPageHide = () => {
      const current = sessionRef.current;
      if (current?.phase === "apply") persistDraft(current);
      else interruptSession("page-hide");
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    window.addEventListener("pagehide", onPageHide);
    return () => {
      document.removeEventListener("visibilitychange", onVisibilityChange);
      window.removeEventListener("pagehide", onPageHide);
    };
  }, [interruptSession]);

  useEffect(() => () => {
    const current = sessionRef.current;
    if (!current) return;
    if (current.phase === "apply") {
      persistDraft(current);
      return;
    }
    interruptSession("unmount", false);
  }, [interruptSession]);

  const startBaseline = () => {
    if (!stats || statsStatus !== "live") return;
    clearDraft();
    const now = Date.now();
    const next: ActiveSession = {
      mode,
      phase: "baseline",
      baselineStartedAt: now,
      baselineEndsAt: now + WINDOW_MS,
      baselineDurationMs: 0,
      afterStartedAt: null,
      afterEndsAt: null,
      afterDurationMs: 0,
      baselineSamples: [],
      afterSamples: [],
      lastSampleId: null,
    };
    setNotice("基线窗口已开始；请保持相同任务与使用场景，采集期间不要切换调优模式。");
    setSecondsLeft(60);
    setActiveSession(next);
  };

  const startAfter = () => {
    const current = sessionRef.current;
    if (!current || current.phase !== "apply" || !stats || statsStatus !== "live") return;
    clearDraft();
    const now = Date.now();
    const next: ActiveSession = {
      ...current,
      phase: "after",
      afterStartedAt: now,
      afterEndsAt: now + WINDOW_MS,
      afterDurationMs: 0,
    };
    setNotice("调优后窗口已开始；继续保持相近任务与使用场景，避免把场景变化误读为模式效果。");
    setSecondsLeft(60);
    setActiveSession(next);
  };

  const deleteRecord = useCallback((id: string) => {
    const next = historyRef.current.filter((record) => record.id !== id);
    historyRef.current = next;
    setHistory(next);
    try {
      window.localStorage.setItem(HISTORY_KEY, JSON.stringify(next));
      setStorageWarning(false);
    } catch {
      setStorageWarning(true);
    }
  }, []);

  const resetHistory = useCallback(() => {
    historyRef.current = [];
    setHistory([]);
    try {
      window.localStorage.removeItem(HISTORY_KEY);
      setStorageWarning(false);
    } catch {
      setStorageWarning(true);
    }
  }, []);

  const currentSummary = session
    ? session.phase === "baseline"
      ? summarize(session.baselineSamples, Math.min(WINDOW_MS, Date.now() - session.baselineStartedAt))
      : summarize(session.baselineSamples, session.baselineDurationMs)
    : null;
  const afterSummary = session?.phase === "after" && session.afterStartedAt != null
    ? summarize(session.afterSamples, Math.min(WINDOW_MS, Date.now() - session.afterStartedAt))
    : null;
  const activeEndAt = session?.phase === "baseline"
    ? session.baselineEndsAt
    : session?.phase === "after" ? session.afterEndsAt : null;
  const progress = activeEndAt == null
    ? 0
    : Math.min(100, Math.max(0, ((WINDOW_MS - Math.max(0, activeEndAt - Date.now())) / WINDOW_MS) * 100));

  return (
    <div>
      <PageHead
        title="优化对比实验"
        desc="本地采集两段 60 秒遥测窗口：先记录基线，再手动应用一个现有模式；同时对照供电来源与 Windows 有效能效态。"
        right={<Chip tone="cyan"><Activity size={12} /> 本地实验</Chip>}
      />

      <Card className="mb-4" title="实验设置" icon={<Cpu size={15} className="text-cyan-300" />}>
        <div className="flex flex-col gap-4 sm:flex-row sm:items-end">
          <label className="flex min-w-0 flex-1 flex-col gap-1.5 text-[12px] text-slate-400">
            调优后要手动应用的现有模式
            <select
              value={mode}
              disabled={!!session}
              onChange={(event) => setMode(event.target.value as ModeKey)}
              className="w-full rounded-lg border border-white/10 bg-slate-900/70 px-3 py-2 text-sm text-slate-200 outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70 disabled:opacity-50"
            >
              <option value="game">游戏模式</option>
              <option value="power">省电模式</option>
              <option value="dev">编码模式</option>
            </select>
          </label>
          {!session && (
            <Btn onClick={startBaseline} disabled={!stats || statsStatus !== "live"}>
              <Clock size={14} /> 开始 60 秒基线
            </Btn>
          )}
          {session && (
            <Btn variant="danger" onClick={() => interruptSession("manual")}>
              取消并记录未完成
            </Btn>
          )}
        </div>
        <div className="mt-3 flex items-start gap-2 border-t border-white/6 pt-3 text-[12px] leading-relaxed text-slate-500">
          <Info size={14} className="mt-0.5 shrink-0 text-cyan-300/80" />
          <span>这里只读取当前 Stats 快照，不会调用调优接口；记录聚合指标、电源来源与 Windows 有效能效态样本，不收集进程名称、路径或设备标识。建议两段窗口运行相近任务。</span>
        </div>
      </Card>

      {session && (
        <Card
          className="mb-4"
          title={session.phase === "baseline" ? "正在采集基线" : session.phase === "apply" ? "等待手动应用模式" : "正在采集调优后数据"}
          icon={<Clock size={15} className="text-violet-300" />}
          right={<Chip tone={session.phase === "apply" ? "amber" : "violet"}>{session.phase === "apply" ? "待操作" : `${secondsLeft}s`}</Chip>}
        >
          {session.phase === "baseline" && (
            <p className="mb-3 text-[13px] text-slate-400">保持日常任务与当前设置，基线完成后再应用「{modeName(session.mode)}」。</p>
          )}
          {session.phase === "apply" && (
            <div className="mb-3 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
              <p className="text-[13px] leading-relaxed text-slate-400">
                基线已采集 {session.baselineSamples.length} 个样本。请在 AeroTune 对应页面手动开启 <b className="text-slate-200">{modeName(session.mode)}</b>；完成后返回此处开始第二段窗口。切换操作由你手动完成，本组件不会应用模式。
              </p>
              <Btn onClick={startAfter} disabled={!stats || statsStatus !== "live"}>
                <CheckCircle2 size={14} /> 已手动应用，开始调优后
              </Btn>
            </div>
          )}
          {session.phase === "apply" && statsStatus !== "live" && (
            <p className="mb-3 text-xs text-amber-300" role="status">实时遥测暂不可用；恢复后才能开始调优后窗口。</p>
          )}
          {session.phase !== "apply" && (
            <div className="mb-3 h-1.5 overflow-hidden rounded-full bg-white/8" role="progressbar" aria-label="窗口采集进度" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(progress)}>
              <div className="h-full rounded-full grad-btn transition-[width] duration-200" style={{ width: `${progress}%` }} />
            </div>
          )}
          <div className="grid grid-cols-2 gap-2 text-[12px] sm:grid-cols-4">
            <div className="rounded-lg border border-white/6 bg-white/3 px-3 py-2">
              <div className="text-slate-500">窗口时长</div>
              <div className="mt-0.5 tabular-nums text-slate-200">{session.phase === "baseline" ? formatDuration(Math.min(WINDOW_MS, Date.now() - session.baselineStartedAt)) : session.phase === "after" && session.afterStartedAt != null ? formatDuration(Math.min(WINDOW_MS, Date.now() - session.afterStartedAt)) : formatDuration(session.baselineDurationMs)}</div>
            </div>
            <div className="rounded-lg border border-white/6 bg-white/3 px-3 py-2">
              <div className="text-slate-500">基线样本</div>
              <div className="mt-0.5 tabular-nums text-slate-200">{session.baselineSamples.length}</div>
            </div>
            <div className="rounded-lg border border-white/6 bg-white/3 px-3 py-2">
              <div className="text-slate-500">调优后样本</div>
              <div className="mt-0.5 tabular-nums text-slate-200">{session.afterSamples.length}</div>
            </div>
            <div className="rounded-lg border border-white/6 bg-white/3 px-3 py-2">
              <div className="text-slate-500">记录模式</div>
              <div className="mt-0.5 text-slate-200">{modeName(session.mode)}</div>
            </div>
          </div>
          {currentSummary && (
            <div className="mt-3 grid gap-1 text-[11px] text-slate-500 sm:grid-cols-2">
              <span>基线：{formatNumber(currentSummary.cpuPct)}% CPU · {formatNumber(currentSummary.memoryUsedGb, 2)} GB 内存</span>
              {afterSummary && <span>调优后：{formatNumber(afterSummary.cpuPct)}% CPU · {formatNumber(afterSummary.memoryUsedGb, 2)} GB 内存</span>}
            </div>
          )}
        </Card>
      )}

      {notice && <div role="status" aria-live="polite" className="mb-4 rounded-lg border border-cyan-400/15 bg-cyan-400/5 px-3 py-2 text-[12px] text-cyan-100/80">{notice}</div>}

      <ExperimentHistoryView
        history={history}
        consistency={consistency}
        storageWarning={storageWarning}
        onDeleteRecord={deleteRecord}
        onResetHistory={resetHistory}
      />
    </div>
  );
}
