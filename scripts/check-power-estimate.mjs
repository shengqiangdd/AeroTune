import assert from "node:assert/strict";
import fs from "node:fs";
import {
  AC_DRAIN_MAX_SAMPLE_GAP_MS,
  AC_DRAIN_MIN_COVERAGE_MS,
  AC_DRAIN_MIN_DROP_PERCENT,
  AC_DRAIN_MIN_SAMPLES,
  AC_DRAIN_TREND_WINDOW_MS,
  BATTERY_MAX_SAMPLE_GAP_MS,
  BATTERY_SAMPLE_INTERVAL_MS,
  batteryAcState,
  batteryQueryFailed,
  estimateAcDrain,
  estimateDischarge,
} from "../src/lib/powerEstimate.js";

function sampleRun(start, count, startingPercent = 100, dropPerSample = 0.5) {
  return Array.from({ length: count }, (_, index) => ({
    timestamp: start + index * BATTERY_SAMPLE_INTERVAL_MS,
    acConnected: false,
    percent: startingPercent - index * dropPerSample,
  }));
}

function acSampleRun(start, count, startingPercent = 100, dropPerSample = 0.125, intervalMs = BATTERY_SAMPLE_INTERVAL_MS) {
  return Array.from({ length: count }, (_, index) => ({
    timestamp: start + index * intervalMs,
    acConnected: true,
    percent: startingPercent - index * dropPerSample,
  }));
}

function acSampleValues(start, values, intervalMs = BATTERY_SAMPLE_INTERVAL_MS) {
  return values.map((percent, index) => ({
    timestamp: start + index * intervalMs,
    acConnected: true,
    percent,
  }));
}

assert.equal(BATTERY_MAX_SAMPLE_GAP_MS, BATTERY_SAMPLE_INTERVAL_MS * 2.5);
assert.equal(batteryAcState({ ac_connected: false, ac_online: true }), false);
assert.equal(batteryAcState({ ac_connected: true, ac_online: false }), true);
assert.equal(batteryAcState({ ac_connected: null, ac_online: true }), null);
assert.equal(batteryAcState({ ac_online: true }), null);
assert.equal(
  batteryQueryFailed({ battery_status_ok: false, battery: { percent: 75, ac_connected: false } }),
  true,
  "an explicit query failure invalidates even a populated battery snapshot",
);
assert.equal(batteryQueryFailed({ battery_status_ok: true }), false);
assert.equal(batteryQueryFailed({}), false, "older snapshots without status remain compatible");

const powerInsights = fs.readFileSync("src/components/PowerInsights.tsx", "utf8");
assert.match(powerInsights, /function sampleFromStats[\s\S]*?statsStatus !== "live" \|\| stats\?\.battery_status_ok !== true \|\| batteryQueryFailed\(stats\)[\s\S]*?const acConnected = batteryAcState\(battery\)/);
assert.match(powerInsights, /const currentAcState = batteryAcState\(battery\)/);
assert.match(powerInsights, /const battery = !telemetryLive \|\| stats\?\.battery_status_ok !== true \|\| batteryQueryFailed\(stats\) \? null : stats\?\.battery \?\? null/);
assert.doesNotMatch(powerInsights, /battery\.ac_online/, "legacy conservative AC flag must not override unknown connection state");
assert.match(powerInsights, /const collectionStatus = statsStatus === "error"[\s\S]*?batteryQueryFailed\(stats\)[\s\S]*?电池接口查询失败 · 暂不估算[\s\S]*?电池接口状态未知 · 暂不估算[\s\S]*?!enabled/, "telemetry and query failures are visible even when local recording is disabled");
assert.match(powerInsights, /currentAcState === true[\s\S]*?观察接电电量变化/, "the collection status makes AC-powered observation explicit");
assert.match(powerInsights, /estimateAcDrain\(runSamples\)/);
assert.match(powerInsights, /!enabled \|\| statsStatus !== "live" \|\| stats\?\.battery_status_ok !== true \|\| batteryQueryFailed\(stats\) \|\| batteryAcState\(battery\) !== true/);
assert.match(powerInsights, /acDrainInvalidatedAtRef\.current = Date\.now\(\)/);
assert.match(powerInsights, /sample\.timestamp > \(acDrainInvalidatedAtRef\.current \?\? Number\.NEGATIVE_INFINITY\)/);
assert.match(powerInsights, /clockNow - latestSample\.timestamp > AC_DRAIN_MAX_SAMPLE_GAP_MS/);
assert.match(powerInsights, /setClockNow\(now\)/, "plugged-in estimate freshness has a reactive clock tick");
assert.match(powerInsights, /Prune expired opt-in data on page open even when recording is currently off/);
assert.match(powerInsights, /window\.localStorage\.removeItem\(STORAGE_KEY\)[\s\S]*?storageReadyRef\.current = true/);
assert.match(powerInsights, /最近 \$\{Math\.round\(pluggedDrain\.spanMinutes\)\} 分钟的采样点均报告 AC/);
assert.match(powerInsights, /点间瞬时断连不可见/);
assert.match(powerInsights, /AC 在线不代表适配器功率足够，也不能据此诊断硬件故障/);
assert.match(powerInsights, /right=\{<Chip role="status" tone=\{estimate \|\| pluggedDrain \?/ , "battery collection status uses an accessible status chip");
const sharedUi = fs.readFileSync("src/components/ui.tsx", "utf8");
assert.match(sharedUi, /export function Chip[\s\S]*?role\?: "status"[\s\S]*?role=\{role\}/, "status chips use the polite ARIA status role");

const continuous = sampleRun(0, 11);
const continuousEstimate = estimateDischarge(continuous, 94);
assert.ok(continuousEstimate, "a sufficiently long continuous discharge run should estimate");
assert.equal(continuousEstimate.sampleCount, 11);
assert.equal(continuousEstimate.spanMinutes, 10);

const lastObservedAt = continuous.at(-1).timestamp;
const exactGapBoundary = [
  ...continuous,
  ...sampleRun(lastObservedAt + BATTERY_MAX_SAMPLE_GAP_MS, 6, 90, 1),
];
const boundaryEstimate = estimateDischarge(exactGapBoundary, 85);
assert.ok(boundaryEstimate, "a gap exactly at the configured limit remains continuous");
assert.equal(boundaryEstimate.sampleCount, 17);

const interruptedShortRun = [
  ...continuous,
  ...sampleRun(lastObservedAt + BATTERY_MAX_SAMPLE_GAP_MS + 1, 6, 90, 1),
];
assert.equal(
  estimateDischarge(interruptedShortRun, 85),
  null,
  "a gap just above the limit must not combine a fresh short run with stale history",
);

const freshQualifyingRun = [
  ...continuous,
  ...sampleRun(lastObservedAt + BATTERY_MAX_SAMPLE_GAP_MS + 1, 11, 90, 0.5),
];
const freshEstimate = estimateDischarge(freshQualifyingRun, 84);
assert.ok(freshEstimate, "a fresh run may estimate after independently meeting coverage");
assert.equal(freshEstimate.sampleCount, 11, "older samples must not leak across the gap");
assert.equal(freshEstimate.spanMinutes, 10);

const acInterrupted = continuous.map((sample, index) => ({
  ...sample,
  acConnected: index < 5,
}));
assert.equal(estimateDischarge(acInterrupted, 94), null, "AC-connected samples still end a discharge run");
const unknownAcState = batteryAcState({ ac_connected: null, ac_online: true });
assert.equal(
  estimateDischarge([...continuous, { timestamp: lastObservedAt + BATTERY_SAMPLE_INTERVAL_MS, acConnected: unknownAcState, percent: 94 }], 94),
  null,
  "legacy conservative AC=true must not let a run cross an unknown connection state",
);
assert.equal(estimateDischarge(continuous, 101), null, "out-of-range current battery levels remain unknown");

assert.equal(AC_DRAIN_MAX_SAMPLE_GAP_MS, BATTERY_SAMPLE_INTERVAL_MS * 1.5);
assert.equal(AC_DRAIN_MIN_COVERAGE_MS, 15 * 60_000);
assert.equal(AC_DRAIN_MIN_DROP_PERCENT, 2);
assert.equal(AC_DRAIN_MIN_SAMPLES, 12);
assert.equal(AC_DRAIN_TREND_WINDOW_MS, 30 * 60_000);
const pluggedDrainRun = acSampleValues(0, [
  ...Array(6).fill(100),
  ...Array(5).fill(99),
  ...Array(6).fill(98),
]);
const pluggedDrain = estimateAcDrain(pluggedDrainRun);
assert.ok(pluggedDrain, "a distributed decline across both halves should be reported");
assert.equal(pluggedDrain.dropPercent, 2);
assert.equal(pluggedDrain.sampleCount, 17);
assert.equal(pluggedDrain.spanMinutes, 16);
assert.ok(pluggedDrain.percentPerHour > 0, "the median pairwise decline rate is reported");
const flatThenStep = acSampleValues(0, [...Array(9).fill(100), ...Array(8).fill(98)]);
assert.equal(estimateAcDrain(flatThenStep), null, "a flat window followed by one persistent gauge step is not a sustained trend");
const endpointStep = acSampleValues(0, [...Array(16).fill(100), 98]);
assert.equal(estimateAcDrain(endpointStep), null, "a single final gauge jump cannot trigger the report");
assert.equal(estimateAcDrain(acSampleRun(0, 16, 100, 0.1)), null, "a small gauge change below the 2-point threshold is ignored");
assert.equal(estimateAcDrain(acSampleRun(0, 17, 90, -0.2)), null, "charging or rising percentage is not a drain");
const elevenSampleBoundary = acSampleValues(0, [100, 100, 100, 100, 100, 99, 99, 99, 98, 98, 98], AC_DRAIN_MAX_SAMPLE_GAP_MS);
assert.equal(elevenSampleBoundary.at(-1).timestamp - elevenSampleBoundary[0].timestamp, AC_DRAIN_MIN_COVERAGE_MS);
assert.equal(estimateAcDrain(elevenSampleBoundary), null, "11 samples spanning 15 minutes still fail the independent 12-sample gate");
const exactGapRun = acSampleValues(0, [...Array(6).fill(100), ...Array(5).fill(99), 98], AC_DRAIN_MAX_SAMPLE_GAP_MS);
assert.equal(
  estimateAcDrain(exactGapRun)?.sampleCount,
  12,
  "the exact AC sample-gap boundary remains continuous when coverage and the sample-count gate are met",
);
const interruptedAcDrain = [
  ...acSampleRun(0, 6),
  ...acSampleRun(AC_DRAIN_MAX_SAMPLE_GAP_MS + 6 * BATTERY_SAMPLE_INTERVAL_MS, 6, 99),
];
assert.equal(estimateAcDrain(interruptedAcDrain), null, "an over-gap cannot combine two short AC runs");
assert.equal(
  estimateAcDrain([...pluggedDrainRun, { timestamp: pluggedDrainRun.at(-1).timestamp + BATTERY_SAMPLE_INTERVAL_MS, acConnected: false, percent: 98 }]),
  null,
  "disconnecting from AC terminates the AC-only run",
);
assert.equal(
  estimateAcDrain([...pluggedDrainRun, { timestamp: pluggedDrainRun.at(-1).timestamp + BATTERY_SAMPLE_INTERVAL_MS, acConnected: null, percent: 98 }]),
  null,
  "unknown AC status terminates the AC-only run",
);

console.log("Battery estimate passed: discharge boundaries and conservative plugged-in drain observation");
