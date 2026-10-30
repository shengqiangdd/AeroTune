export interface ProcessHistoryMetric {
  name: string;
  /** Null marks a legacy sample whose old per-core CPU value cannot be converted safely. */
  cpu: number | null;
  memMb: number;
}

export interface ProcessHistorySample {
  sampledAt: number;
  processes: ProcessHistoryMetric[];
}

export interface ProcessHistoryTrend {
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
}

export function processNameKey(name: string): string;
export function clearLegacyProcessHistoryCpu(samples: ProcessHistorySample[]): ProcessHistorySample[];
export function mergeProcessHistoryMetrics(metrics: ProcessHistoryMetric[]): ProcessHistoryMetric[];
export function buildTrends(samples: ProcessHistorySample[]): ProcessHistoryTrend[];
export function formatProcessHistorySpan(spanMs: number): string;
