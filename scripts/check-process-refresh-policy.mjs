import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  cpuLoadBandFromUsage,
  interactionLatencyBandFromP95,
  memoryLoadBandFromUsage,
  processRefreshIntervalMs,
} from "../src/lib/processRefreshPolicy.js";

const rustStats = readFileSync(new URL("../src-tauri/src/stats.rs", import.meta.url), "utf8");

const cpuCases = [
  ["normal CPU", 59.99, "unknown", "normal"],
  ["elevated CPU enters at 60", 60, "unknown", "elevated"],
  ["elevated CPU resists downward jitter", 58, "elevated", "elevated"],
  ["elevated CPU exits below 56", 55.99, "elevated", "normal"],
  ["high CPU enters at 80", 80, "unknown", "high"],
  ["high CPU resists downward jitter", 78, "high", "high"],
  ["high CPU exits below 76", 75.99, "high", "elevated"],
  ["elevated CPU exits after high band", 55.99, "elevated", "normal"],
  ["invalid CPU clears stale band", Number.NaN, "high", "unknown"],
  ["out-of-range CPU is unknown", 101, "normal", "unknown"],
];

for (const [name, cpuPct, previous, expected] of cpuCases) {
  assert.equal(cpuLoadBandFromUsage(cpuPct, previous), expected, name);
}

const memoryCases = [
  ["normal usage", 8, 16, "unknown", "normal"],
  ["below elevated boundary", 84.99, 100, "unknown", "normal"],
  ["elevated boundary", 85, 100, "unknown", "elevated"],
  ["elevated RAM resists downward jitter", 84, 100, "elevated", "elevated"],
  ["elevated RAM exits below 83", 82.99, 100, "elevated", "normal"],
  ["below high boundary", 89.99, 100, "unknown", "elevated"],
  ["high boundary", 90, 100, "unknown", "high"],
  ["high RAM resists downward jitter", 89, 100, "high", "high"],
  ["high RAM exits below 88", 87.99, 100, "high", "elevated"],
  ["elevated RAM holds after high exit", 84, 100, "elevated", "elevated"],
  ["elevated RAM returns to normal below 83", 82.99, 100, "elevated", "normal"],
  ["zero usage", 0, 16, "unknown", "normal"],
  ["missing usage", undefined, 16, "high", "unknown"],
  ["non-numeric usage", "8", 16, "normal", "unknown"],
  ["negative usage", -1, 16, "elevated", "unknown"],
  ["empty capacity", 8, 0, "high", "unknown"],
  ["used exceeds total", 17, 16, "normal", "unknown"],
  ["non-finite usage", Number.POSITIVE_INFINITY, 16, "high", "unknown"],
  ["non-finite total", 8, Number.NaN, "high", "unknown"],
];

for (const [name, usedGb, totalGb, previous, expected] of memoryCases) {
  assert.equal(memoryLoadBandFromUsage(usedGb, totalGb, previous), expected, name);
}

const latencyCases = [
  ["not yet measured", null, 0, "unknown", "unknown"],
  ["high P95 is ignored before 20 samples", 250, 19, "unknown", "unknown"],
  ["low confidence clears a stale high band", 250, 19, "high", "unknown"],
  ["fractional sample count is invalid", 250, 20.5, "high", "unknown"],
  ["normal interaction latency after confidence gate", 49.99, 20, "unknown", "normal"],
  ["elevated latency enters at 50ms", 50, 20, "unknown", "elevated"],
  ["elevated latency resists downward jitter", 45, 20, "elevated", "elevated"],
  ["elevated latency holds at exact 40ms exit boundary", 40, 20, "elevated", "elevated"],
  ["elevated latency exits below 40ms", 39.99, 20, "elevated", "normal"],
  ["high latency enters at 100ms", 100, 20, "elevated", "high"],
  ["high latency resists downward jitter", 90, 20, "high", "high"],
  ["high latency holds at exact 80ms exit boundary", 80, 20, "high", "high"],
  ["high latency exits below 80ms", 79.99, 20, "high", "elevated"],
  ["latency can exceed 100ms", 250, 20, "unknown", "high"],
  ["negative latency is invalid", -1, 20, "high", "unknown"],
  ["non-finite latency is invalid", Number.POSITIVE_INFINITY, 20, "normal", "unknown"],
];

for (const [name, p95Ms, sampleCount, previous, expected] of latencyCases) {
  assert.equal(interactionLatencyBandFromP95(p95Ms, sampleCount, previous), expected, name);
}

const intervalCases = [
  ["normal pressure, unmeasured latency", "normal", "normal", "unknown", 5_000],
  ["elevated CPU", "elevated", "normal", "unknown", 8_000],
  ["high CPU", "high", "normal", "unknown", 15_000],
  ["elevated RAM", "normal", "elevated", "unknown", 8_000],
  ["high RAM", "normal", "high", "unknown", 15_000],
  ["elevated interaction P95", "normal", "normal", "elevated", 8_000],
  ["high interaction P95", "normal", "normal", "high", 15_000],
  ["combined pressure takes slowest interval", "elevated", "elevated", "high", 15_000],
  ["unknown CPU remains conservative", "unknown", "normal", "unknown", 10_000],
  ["critical RAM overrides unknown CPU", "unknown", "high", "unknown", 15_000],
  ["interaction lag overrides unknown CPU", "unknown", "normal", "elevated", 10_000],
  ["unknown RAM and latency leave CPU interval intact", "elevated", "unknown", "unknown", 8_000],
  ["unknown CPU label uses fallback", "other", "normal", "unknown", 10_000],
  ["unknown pressure labels are ignored", "normal", "other", "other", 5_000],
];

for (const [name, cpuBand, memoryBand, latencyBand, expected] of intervalCases) {
  assert.equal(processRefreshIntervalMs(cpuBand, memoryBand, latencyBand), expected, name);
}

const ioBaselineUpdate = rustStats.match(/fn update_process_io_start_baseline\([\s\S]*?\n}/)?.[0];
assert.ok(ioBaselineUpdate, "process-I/O identity baseline helper must remain present");
assert.match(ioBaselineUpdate, /sampled_this_tick && delta_valid[\s\S]*?baselines\.get\(&pid\)\.copied\(\)/);
assert.match(ioBaselineUpdate, /if sampled_this_tick \{\s*baselines\.insert/);
assert.match(rustStats, /let mut current_pids = proc_io_sampled_this_tick\s*\.then\(\|\| std::collections::HashSet::with_capacity/);
assert.match(rustStats, /update_process_io_start_baseline\([\s\S]*?current_pids\.as_mut\(\)[\s\S]*?proc_io_sampled_this_tick[\s\S]*?proc_io_delta_valid/);
assert.match(rustStats, /prune_process_io_start_baselines\([\s\S]*?current_pids\.as_ref\(\)/);
assert.doesNotMatch(rustStats, /current_pids\.insert\(pid\)|last_io_process_starts\.retain/);

console.log(`Process refresh pressure policy passed: ${cpuCases.length + memoryCases.length + latencyCases.length + intervalCases.length} cases plus I/O baseline cadence contract`);
