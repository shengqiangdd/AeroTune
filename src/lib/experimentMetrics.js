import { capturePowerContext } from "./experimentPowerContext.js";
import { validCapacityPair } from "./resourceEvidence.js";

export const EXPERIMENT_SAMPLE_INTERVAL_MS = 1_000;
export const MIN_EXPERIMENT_WINDOW_COVERAGE = 0.7;
export const MIN_EXPERIMENT_WINDOW_DURATION_MS = 60_000;

const finiteOrNull = (value) => typeof value === "number" && Number.isFinite(value) ? value : null;
const percentOrNull = (value) => {
  const number = finiteOrNull(value);
  return number != null && number >= 0 && number <= 100 ? number : null;
};
const nonNegativeOrNull = (value) => {
  const number = finiteOrNull(value);
  return number != null && number >= 0 ? number : null;
};

export function captureExperimentMetrics(stats) {
  const memoryValid = validCapacityPair(stats.mem_used_gb, stats.mem_total_gb);
  const gpuMemoryValid = validCapacityPair(stats.gpu?.mem_used_mb, stats.gpu?.mem_total_mb);
  return {
    cpuPct: percentOrNull(stats.cpu),
    memoryUsedGb: memoryValid ? stats.mem_used_gb : null,
    memoryPct: memoryValid ? (stats.mem_used_gb / stats.mem_total_gb) * 100 : null,
    gpuPct: percentOrNull(stats.gpu?.utilization),
    gpuMemoryMb: gpuMemoryValid ? stats.gpu.mem_used_mb : null,
    rxKbps: nonNegativeOrNull(stats.net_rx_kbps),
    txKbps: nonNegativeOrNull(stats.net_tx_kbps),
    powerContext: capturePowerContext(stats),
  };
}

/** Convert valid-sample counts into elapsed-window coverage fractions for Rust analysis. */
export function analyzerCoverage(summary) {
  const durationMs = typeof summary?.durationMs === "number" && Number.isFinite(summary.durationMs)
    ? Math.max(0, summary.durationMs)
    : 0;
  const completeWindow = durationMs >= MIN_EXPERIMENT_WINDOW_DURATION_MS;
  const expectedSamples = Math.max(1, Math.ceil(durationMs / EXPERIMENT_SAMPLE_INTERVAL_MS));
  const normalized = (count) => {
    if (!completeWindow) return 0;
    const validCount = typeof count === "number" && Number.isFinite(count) ? Math.max(0, count) : 0;
    return Math.min(1, validCount / expectedSamples);
  };
  const counts = summary?.coverage ?? {};
  return {
    cpu: normalized(counts.cpu),
    memory: normalized(counts.memory),
    memoryPct: normalized(counts.memoryPct),
    gpu: normalized(counts.gpu),
    gpuMemory: normalized(counts.gpuMemory),
    rx: normalized(counts.rx),
    tx: normalized(counts.tx),
  };
}

export function hasTrustworthyExperimentCoverage(summary) {
  if (
    !summary ||
    typeof summary.durationMs !== "number" || !Number.isFinite(summary.durationMs) || summary.durationMs < MIN_EXPERIMENT_WINDOW_DURATION_MS ||
    !Number.isSafeInteger(summary.sampleCount) || summary.sampleCount < 3
  ) return false;
  const coverage = analyzerCoverage(summary);
  return coverage.cpu >= MIN_EXPERIMENT_WINDOW_COVERAGE && coverage.memory >= MIN_EXPERIMENT_WINDOW_COVERAGE;
}
