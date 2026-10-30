import type { Stats } from "./api";
import type { PowerContextSample } from "./experimentPowerContext";

export interface ExperimentMetricSample {
  cpuPct: number | null;
  memoryUsedGb: number | null;
  memoryPct: number | null;
  gpuPct: number | null;
  gpuMemoryMb: number | null;
  rxKbps: number | null;
  txKbps: number | null;
  powerContext: PowerContextSample;
}
export interface ExperimentMetricCoverageCounts {
  cpu: number;
  memory: number;
  memoryPct: number;
  gpu: number;
  gpuMemory: number;
  rx: number;
  tx: number;
}
export interface ExperimentMetricSummary {
  durationMs: number;
  sampleCount: number;
  coverage: ExperimentMetricCoverageCounts;
}

export const EXPERIMENT_SAMPLE_INTERVAL_MS: number;
export const MIN_EXPERIMENT_WINDOW_COVERAGE: number;
export const MIN_EXPERIMENT_WINDOW_DURATION_MS: number;
export function captureExperimentMetrics(stats: Stats): ExperimentMetricSample;
export function analyzerCoverage(summary: Pick<ExperimentMetricSummary, "durationMs" | "coverage">): Record<string, number>;
export function hasTrustworthyExperimentCoverage(summary: ExperimentMetricSummary): boolean;
