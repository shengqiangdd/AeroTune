import { memo, useEffect, useMemo, useRef, useState } from "react";
import { Activity, Cpu, HardDrive, Trash2 } from "lucide-react";
import { Card, Chip, Toggle, Btn } from "./ui";
import type { ProcInfo } from "../lib/api";
import { buildTrends, clearLegacyProcessHistoryCpu, formatProcessHistorySpan, mergeProcessHistoryMetrics } from "../lib/processHistoryTrends";

// Keep the stable storage slot; the payload version controls in-place migration without duplicating quota usage.
const STORAGE_KEY = "aerotune.process-history.v1";
const LEGACY_SCHEMA_VERSION = 1;
const SCHEMA_VERSION = 2;
const MAX_CPU_PERCENT = 100;
const MIN_SAMPLE_INTERVAL_MS = 5 * 60_000;
const MAX_HISTORY_AGE_MS = 7 * 24 * 60 * 60 * 1_000;
const MAX_SAMPLES = 2_017; // Seven-day 5-minute sampling includes both endpoints.
const MAX_NAMES_PER_SAMPLE = 20;
const NAMES_PER_METRIC = 10;
const MAX_NAME_LENGTH = 128;

type ProcessMetric = {
  name: string;
  /** Null marks a legacy CPU sample whose former scale cannot be converted safely. */
  cpu: number | null;
  memMb: number;
};

type ProcessSample = {
  sampledAt: number;
  processes: ProcessMetric[];
};

type HistoryState = {
  version: typeof SCHEMA_VERSION;
  enabled: boolean;
  samples: ProcessSample[];
};

type Trend = {
  name: string;
  cpuObservations: number;
  memoryObservations: number;
  averageCpu: number | null;
  averageMemMb: number;
  firstCpu: number | null;
  lastCpu: number | null;
  firstMemMb: number;
  lastMemMb: number;
  firstCpuSampledAt: number | null;
  lastCpuSampledAt: number | null;
  firstMemorySampledAt: number;
  lastMemorySampledAt: number;
};

const EMPTY_HISTORY: HistoryState = {
  version: SCHEMA_VERSION,
  enabled: false,
  samples: [],
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Keep only an executable-like basename; never persist paths or arguments. */
function executableName(value: unknown): string | null {
  if (typeof value !== "string") return null;
  let name = value.replace(/[\u0000-\u001f\u007f]/g, " ").trim();
  if (!name) return null;
  name = name.split(/[\\/]/).pop()?.trim() ?? "";
  const exeEnd = name.toLowerCase().indexOf(".exe");
  if (exeEnd >= 0) name = name.slice(0, exeEnd + 4);
  else name = name.split(/\s+/)[0] ?? "";
  name = name.replace(/[<>:"|?*]/g, "").trim().slice(0, MAX_NAME_LENGTH);
  return name || null;
}

function validMetricNumber(value: unknown, max: number): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= max;
}

function pruneSamples(samples: ProcessSample[], now: number): ProcessSample[] {
  return samples
    .filter((sample) => Number.isFinite(sample.sampledAt)
      && sample.sampledAt <= now
      && now - sample.sampledAt <= MAX_HISTORY_AGE_MS)
    .sort((a, b) => a.sampledAt - b.sampledAt)
    .slice(-MAX_SAMPLES);
}

function parseStoredHistory(raw: string, expectedVersion: number): HistoryState | null {
  const parsed: unknown = JSON.parse(raw);
  if (!isRecord(parsed) || parsed.version !== expectedVersion || !Array.isArray(parsed.samples)) {
    return null;
  }

  const now = Date.now();
  const samples: ProcessSample[] = [];
  for (const rawSample of parsed.samples.slice(-MAX_SAMPLES)) {
    if (!isRecord(rawSample) || !validMetricNumber(rawSample.sampledAt, Number.MAX_SAFE_INTEGER)) continue;
    const sampledAt = Math.floor(rawSample.sampledAt);
    if (sampledAt > now || now - sampledAt > MAX_HISTORY_AGE_MS || !Array.isArray(rawSample.processes)) continue;

    const rawProcesses: ProcessMetric[] = [];
    for (const rawMetric of rawSample.processes.slice(0, MAX_NAMES_PER_SAMPLE)) {
      if (!isRecord(rawMetric)) continue;
      const name = executableName(rawMetric.name);
      if (!name || !validMetricNumber(rawMetric.memMb, 10_000_000)) continue;
      const cpu = expectedVersion === LEGACY_SCHEMA_VERSION
        ? (validMetricNumber(rawMetric.cpu, 10_000) ? rawMetric.cpu : null)
        : (validMetricNumber(rawMetric.cpu, MAX_CPU_PERCENT) ? rawMetric.cpu : null);
      rawProcesses.push({ name, cpu, memMb: rawMetric.memMb });
    }
    const processes = mergeProcessHistoryMetrics(rawProcesses);
    if (processes.length) samples.push({ sampledAt, processes });
  }

  const prunedSamples = pruneSamples(samples, now);
  return {
    version: SCHEMA_VERSION,
    enabled: parsed.enabled === true,
    samples: expectedVersion === LEGACY_SCHEMA_VERSION
      ? clearLegacyProcessHistoryCpu(prunedSamples)
      : prunedSamples,
  };
}

function readStoredHistory(): HistoryState {
  if (typeof window === "undefined") return EMPTY_HISTORY;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) return EMPTY_HISTORY;
    return parseStoredHistory(raw, SCHEMA_VERSION)
      ?? parseStoredHistory(raw, LEGACY_SCHEMA_VERSION)
      ?? EMPTY_HISTORY;
  } catch {
    // Storage can be disabled by the browser or contain malformed JSON.
    return EMPTY_HISTORY;
  }
}

function aggregateProcesses(processes: ProcInfo[]): ProcessMetric[] {
  const rawMetrics: Array<{ name: string; cpu: number; memMb: number }> = [];
  for (const process of Array.isArray(processes) ? processes : []) {
    const name = executableName(process?.name);
    if (!name
      || !validMetricNumber(process?.cpu, MAX_CPU_PERCENT)
      || !validMetricNumber(process?.mem_mb, 10_000_000)) continue;
    rawMetrics.push({ name, cpu: process.cpu, memMb: process.mem_mb });
  }

  const all = mergeProcessHistoryMetrics(rawMetrics)
    .filter((metric): metric is ProcessMetric & { cpu: number } => metric.cpu != null);
  if (all.length <= MAX_NAMES_PER_SAMPLE) return all;

  // Reserve slots for both CPU and memory leaders, then fill by normalized rank.
  const selected = new Map<string, ProcessMetric>();
  const byCpu = [...all].sort((a, b) => b.cpu - a.cpu || a.name.localeCompare(b.name));
  const byMemory = [...all].sort((a, b) => b.memMb - a.memMb || a.name.localeCompare(b.name));
  for (const metric of byCpu.slice(0, NAMES_PER_METRIC)) selected.set(metric.name, metric);
  for (const metric of byMemory.slice(0, NAMES_PER_METRIC)) selected.set(metric.name, metric);

  const maxCpu = all.reduce((max, metric) => Math.max(max, metric.cpu), 1);
  const maxMemory = all.reduce((max, metric) => Math.max(max, metric.memMb), 1);
  const combinedRank = [...all].sort((a, b) => (
    (b.cpu / maxCpu + b.memMb / maxMemory) - (a.cpu / maxCpu + a.memMb / maxMemory)
  ) || a.name.localeCompare(b.name));
  for (const metric of combinedRank) {
    if (selected.size >= MAX_NAMES_PER_SAMPLE) break;
    selected.set(metric.name, metric);
  }

  return [...selected.values()];
}

function saveHistory(history: HistoryState): boolean {
  if (typeof window === "undefined") return false;
  try {
    // The persisted schema deliberately contains no ProcInfo object, PID, path, or command line.
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify({
      version: SCHEMA_VERSION,
      enabled: history.enabled,
      samples: history.samples,
    }));
    return true;
  } catch {
    return false;
  }
}

function formatDelta(delta: number, digits = 1): string {
  if (Math.abs(delta) < 0.05) return "持平";
  return `${delta > 0 ? "+" : "−"}${Math.abs(delta).toFixed(digits)}`;
}

const TrendList = memo(function TrendList({
  title,
  trends,
  metric,
}: {
  title: string;
  trends: Trend[];
  metric: "cpu" | "memory";
}) {
  const ranked = useMemo(
    () => [...trends]
      .filter((trend) => metric === "cpu" ? trend.cpuObservations > 0 : trend.memoryObservations > 0)
      .sort((a, b) => metric === "cpu"
        ? (b.averageCpu ?? 0) - (a.averageCpu ?? 0)
        : b.averageMemMb - a.averageMemMb)
      .slice(0, 5),
    [trends, metric],
  );

  return (
    <section aria-label={title} className="min-w-0 rounded-xl border border-white/8 bg-slate-950/20 p-3">
      <h3 className="mb-2 text-xs font-medium text-slate-300">{title}</h3>
      {ranked.length ? (
        <ol className="space-y-2">
          {ranked.map((trend) => {
            const observations = metric === "cpu" ? trend.cpuObservations : trend.memoryObservations;
            const average = metric === "cpu" ? trend.averageCpu : trend.averageMemMb;
            const first = metric === "cpu" ? trend.firstCpu : trend.firstMemMb;
            const last = metric === "cpu" ? trend.lastCpu : trend.lastMemMb;
            const firstSampledAt = metric === "cpu" ? trend.firstCpuSampledAt : trend.firstMemorySampledAt;
            const lastSampledAt = metric === "cpu" ? trend.lastCpuSampledAt : trend.lastMemorySampledAt;
            if (average == null || first == null || last == null || firstSampledAt == null || lastSampledAt == null) return null;
            const delta = last - first;
            const unit = metric === "cpu" ? "%" : " MB";
            return (
              <li key={trend.name} className="flex min-w-0 items-start justify-between gap-3 text-xs">
                <span className="min-w-0 flex-1 truncate font-medium text-slate-200" title={trend.name}>{trend.name}</span>
                <span className="min-w-0 flex-1 text-right tabular-nums text-slate-400">
                  <span className="block text-slate-200">{average.toFixed(1)}{unit}</span>
                  {observations > 1 ? (
                    <>
                      <span className="block">首末差 {formatDelta(delta)}{unit}</span>
                      <span className="block text-[10px]">{observations} 次采样 · 跨度 {formatProcessHistorySpan(lastSampledAt - firstSampledAt)}</span>
                    </>
                  ) : (
                    <span className="block">1 次采样</span>
                  )}
                </span>
              </li>
            );
          })}
        </ol>
      ) : (
        <p className="text-xs text-slate-500">开启采样后，这里会显示实际记录到的趋势。</p>
      )}
    </section>
  );
});

export function ProcessHistoryCard({ processes }: { processes: ProcInfo[] }) {
  const [history, setHistory] = useState<HistoryState>(readStoredHistory);
  const [storageAvailable, setStorageAvailable] = useState(true);
  const lastSampledAt = useRef(history.samples[history.samples.length - 1]?.sampledAt ?? 0);

  const commit = (next: HistoryState) => {
    setHistory(next);
    setStorageAvailable(saveHistory(next));
  };

  useEffect(() => {
    // Rewrite the validated/pruned schema so expired or malformed legacy data is not retained.
    setStorageAvailable(saveHistory(history));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    const oldest = history.samples[0];
    if (!oldest) return;
    const expiresAt = oldest.sampledAt + MAX_HISTORY_AGE_MS;
    const timer = window.setTimeout(() => {
      const now = Date.now();
      const samples = pruneSamples(history.samples, now);
      if (samples.length !== history.samples.length) {
        commit({ ...history, samples });
      }
    }, Math.max(0, expiresAt - Date.now() + 1));
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [history]);

  useEffect(() => {
    if (!history.enabled) return;
    const now = Date.now();
    const clockMovedBackwards = now < lastSampledAt.current;
    const previousSamples = clockMovedBackwards ? pruneSamples(history.samples, now) : history.samples;
    if (clockMovedBackwards) lastSampledAt.current = now - MIN_SAMPLE_INTERVAL_MS;
    if (!clockMovedBackwards && now - lastSampledAt.current < MIN_SAMPLE_INTERVAL_MS) return;

    const captured = aggregateProcesses(processes);
    if (!captured.length) {
      if (clockMovedBackwards && previousSamples.length !== history.samples.length) {
        commit({ ...history, samples: previousSamples });
      }
      return;
    }

    const sample: ProcessSample = { sampledAt: now, processes: captured };
    lastSampledAt.current = now;
    commit({
      version: SCHEMA_VERSION,
      enabled: true,
      samples: pruneSamples([...previousSamples, sample], now),
    });
    // Sampling is intentionally driven by process-list updates, with a hard five-minute floor.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [processes, history.enabled]);

  const trends = useMemo(() => buildTrends(history.samples), [history.samples]);
  const hasUnscaledCpuHistory = useMemo(
    () => history.samples.some((sample) => sample.processes.some((process) => process.cpu == null)),
    [history.samples],
  );
  const latestSample = history.samples[history.samples.length - 1];

  const setOptIn = (enabled: boolean) => {
    commit({ ...history, enabled });
  };

  const deleteHistory = () => {
    lastSampledAt.current = 0;
    commit({ version: SCHEMA_VERSION, enabled: history.enabled, samples: [] });
  };

  return (
    <Card
      title="进程资源历史"
      icon={<Activity size={15} className="text-cyan-300" aria-hidden="true" />}
      right={<Chip tone={history.enabled ? "green" : "slate"}>{history.enabled ? "本地采样已开启" : "默认关闭"}</Chip>}
    >
      <div className="space-y-4">
        <div className="flex flex-wrap items-center justify-between gap-4 rounded-xl border border-white/8 bg-slate-950/20 px-3 py-2.5">
          <div className="min-w-0">
            <p className="text-sm font-medium text-slate-200">允许记录本机进程资源采样</p>
            <p className="mt-0.5 text-xs leading-5 text-slate-400">
              仅保存在此设备的应用本地存储中；关闭后停止新增采样，保留的历史可随时删除。
            </p>
          </div>
          <Toggle
            label="允许记录本机进程资源采样"
            on={history.enabled}
            onChange={setOptIn}
          />
        </div>

        <p className="text-xs leading-5 text-slate-400">
          本机进程定时样本，不代表生命周期总量；CPU 按全机逻辑处理器总能力归一为 0–100%；每 5 分钟最多一次，保留 7 天、每样本最多 20 个文件名及 CPU/内存值。同名合并，首末差不代表单一实例；不保存路径、PID 或命令行。
        </p>

        {hasUnscaledCpuHistory && (
          <p role="status" className="text-xs leading-5 text-slate-400">
            旧 CPU 单位无法可靠换算，已从 CPU 趋势中排除；历史内存值仍保留。
          </p>
        )}

        {!storageAvailable && (
          <p role="status" className="text-xs text-amber-300">
            本地存储当前不可用；本次运行中的记录仍可查看，但重新打开应用后不会保留。
          </p>
        )}

        <div className="grid gap-3 sm:grid-cols-2">
          <TrendList title="CPU 全机口径趋势 · Top 5" trends={trends} metric="cpu" />
          <TrendList title="内存平均采样趋势 · Top 5" trends={trends} metric="memory" />
        </div>

        <div className="flex flex-wrap items-center justify-between gap-3 border-t border-white/8 pt-3">
          <div className="flex min-w-0 items-center gap-2 text-xs text-slate-400" aria-live="polite">
            <Cpu size={14} className="shrink-0 text-cyan-300" aria-hidden="true" />
            <span>{history.samples.length} 个实际采样点</span>
            {latestSample && (
              <>
                <span aria-hidden="true">·</span>
                <time dateTime={new Date(latestSample.sampledAt).toISOString()}>
                  最近 {new Date(latestSample.sampledAt).toLocaleString()}
                </time>
              </>
            )}
          </div>
          <div className="flex items-center gap-2">
            <HardDrive size={14} className="text-slate-500" aria-hidden="true" />
            <Btn variant="danger" onClick={deleteHistory}>
              <Trash2 size={14} aria-hidden="true" />
              删除历史
            </Btn>
          </div>
        </div>
      </div>
    </Card>
  );
}
