import assert from "node:assert/strict";
import fs from "node:fs";
import {
  analyzerCoverage,
  captureExperimentMetrics,
  hasTrustworthyExperimentCoverage,
  MIN_EXPERIMENT_WINDOW_COVERAGE,
} from "../src/lib/experimentMetrics.js";

const validStats = {
  cpu: 42,
  mem_used_gb: 6,
  mem_total_gb: 16,
  gpu: { utilization: 55, mem_used_mb: 1536, mem_total_mb: 8192 },
  net_rx_kbps: 125,
  net_tx_kbps: 14,
  battery_status_ok: true,
  battery: { ac_connected: true, percent: 80 },
  effective_power_mode: "balanced",
};
const valid = captureExperimentMetrics(validStats);
assert.equal(valid.cpuPct, 42);
assert.equal(valid.memoryUsedGb, 6);
assert.equal(valid.gpuPct, 55);
assert.equal(valid.gpuMemoryMb, 1536);
assert.equal(valid.rxKbps, 125);
assert.equal(valid.txKbps, 14);

const invalid = captureExperimentMetrics({
  ...validStats,
  cpu: 101,
  mem_used_gb: 17,
  gpu: { utilization: -1, mem_used_mb: 9000, mem_total_mb: 8192 },
  net_rx_kbps: -1,
  net_tx_kbps: Number.POSITIVE_INFINITY,
});
assert.equal(invalid.cpuPct, null, "CPU utilization outside 0–100 is excluded");
assert.equal(invalid.memoryUsedGb, null, "memory use above capacity is excluded");
assert.equal(invalid.memoryPct, null);
assert.equal(invalid.gpuPct, null, "GPU utilization outside 0–100 is excluded");
assert.equal(invalid.gpuMemoryMb, null, "GPU memory use above capacity is excluded");
assert.equal(invalid.rxKbps, null, "negative network rate is excluded");
assert.equal(invalid.txKbps, null, "non-finite network rate is excluded");

const sparse = {
  durationMs: 60_000,
  sampleCount: 3,
  coverage: { cpu: 3, memory: 3, memoryPct: 3, gpu: 3, gpuMemory: 3, rx: 3, tx: 3 },
};
assert.equal(analyzerCoverage(sparse).cpu, 0.05, "three early samples represent only 5% of a 60-second window");
assert.equal(hasTrustworthyExperimentCoverage(sparse), false, "sparse windows cannot be marked complete");

const boundaryTrusted = {
  durationMs: 60_000,
  sampleCount: 42,
  coverage: { cpu: 42, memory: 42, memoryPct: 42, gpu: 0, gpuMemory: 0, rx: 42, tx: 42 },
};
assert.equal(analyzerCoverage(boundaryTrusted).cpu, MIN_EXPERIMENT_WINDOW_COVERAGE);
assert.equal(hasTrustworthyExperimentCoverage(boundaryTrusted), true, "70% valid CPU and memory coverage meets the gate");
assert.equal(hasTrustworthyExperimentCoverage({
  ...boundaryTrusted,
  coverage: { ...boundaryTrusted.coverage, memory: 41 },
}), false, "either primary metric below 70% rejects the window");
const partialWindow = {
  ...boundaryTrusted,
  durationMs: 30_000,
  sampleCount: 30,
  coverage: { cpu: 30, memory: 30, memoryPct: 30, gpu: 0, gpuMemory: 0, rx: 30, tx: 30 },
};
assert.equal(analyzerCoverage(partialWindow).cpu, 0, "partial windows send zero coverage to the backend analyzer");
assert.equal(hasTrustworthyExperimentCoverage(partialWindow), false, "a partial window cannot pass by having complete coverage of its shorter duration");

const lab = fs.readFileSync("src/components/OptimizationLab.tsx", "utf8");
const rust = fs.readFileSync("src-tauri/src/experiment_insight.rs", "utf8");
assert.match(lab, /coverage: analyzerCoverage\(record\.baseline\)/);
assert.match(lab, /coverage: analyzerCoverage\(record\.after\)/);
assert.match(lab, /hasTrustworthyExperimentCoverage\(baselineSummary\)/);
assert.match(lab, /hasTrustworthyExperimentCoverage\(afterSummary\)/);
assert.match(lab, /statsStatus !== "live"/);
assert.match(rust, /MIN_WINDOW_COVERAGE:\s*f64\s*=\s*0\.7/);

console.log("Experiment metrics passed: bounded telemetry, fractional window coverage, and sparse-window rejection");
