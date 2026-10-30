import assert from "node:assert/strict";
import fs from "node:fs";
import {
  buildTrends,
  clearLegacyProcessHistoryCpu,
  formatProcessHistorySpan,
  mergeProcessHistoryMetrics,
  processNameKey,
} from "../src/lib/processHistoryTrends.js";

const start = 1_700_000_000_000;
const appSamples = [
  { sampledAt: start, processes: [{ name: "app.exe", cpu: 10, memMb: 100 }] },
  { sampledAt: start + 2 * 60 * 60_000, processes: [{ name: "app.exe", cpu: 30, memMb: 200 }] },
  { sampledAt: start + 3 * 60 * 60_000, processes: [{ name: "other.exe", cpu: 5, memMb: 50 }] },
  { sampledAt: start + 5 * 60 * 60_000, processes: [{ name: "app.exe", cpu: 0, memMb: 20 }] },
];

const trends = buildTrends(appSamples);
const appTrend = trends.find((trend) => trend.name === "app.exe");
assert.ok(appTrend);
assert.equal(appTrend.cpuObservations, 3, "count only valid CPU samples containing this executable name");
assert.equal(appTrend.memoryObservations, 3, "count only memory samples containing this executable name");
assert.equal(appTrend.firstCpuSampledAt, start, "CPU trend time span begins at its first valid CPU observation");
assert.equal(appTrend.lastCpuSampledAt, start + 5 * 60 * 60_000);
assert.equal(appTrend.firstMemorySampledAt, start);
assert.equal(appTrend.lastMemorySampledAt, start + 5 * 60 * 60_000);
assert.ok(Math.abs(appTrend.averageCpu - (40 / 3)) < 1e-9);
assert.ok(Math.abs(appTrend.averageMemMb - (320 / 3)) < 1e-9);
assert.equal(appTrend.firstCpu, 10);
assert.equal(appTrend.lastCpu, 0);
assert.equal(formatProcessHistorySpan(appTrend.lastCpuSampledAt - appTrend.firstCpuSampledAt), "5.0 小时");
assert.equal(formatProcessHistorySpan(12 * 60_000), "12 分钟");
assert.equal(formatProcessHistorySpan(5 * 60_000), "5 分钟");
assert.equal(formatProcessHistorySpan(2 * 24 * 60 * 60_000), "2.0 天");
const fiveMinutes = 5 * 60_000;
const processSampleDue = (now, last, interval) => now < last || now - last >= interval;
assert.equal(processSampleDue(10_000 + fiveMinutes - 1, 10_000, fiveMinutes), false);
assert.equal(processSampleDue(10_000 + fiveMinutes, 10_000, fiveMinutes), true);
assert.equal(processSampleDue(9_999, 10_000, fiveMinutes), true, "wall-clock rollback must re-arm sampling");
assert.equal(processNameKey("App.EXE"), "app.exe");
const sameSampleVariants = mergeProcessHistoryMetrics([
  { name: "App.EXE", cpu: 10, memMb: 100 },
  { name: "app.exe", cpu: 5, memMb: 50 },
]);
assert.equal(sameSampleVariants.length, 1, "same-sample variants merge into one executable record");
assert.deepEqual(sameSampleVariants[0], { name: "App.EXE", cpu: 15, memMb: 150 });
const cappedVariants = mergeProcessHistoryMetrics([
  { name: "App.EXE", cpu: 8_000, memMb: 8_000_000 },
  { name: "app.exe", cpu: 8_000, memMb: 8_000_000 },
]);
assert.equal(cappedVariants[0].cpu, 100);
assert.equal(cappedVariants[0].memMb, 10_000_000);

const caseVariantTrends = buildTrends([
  { sampledAt: start, processes: [{ name: "App.EXE", cpu: 10, memMb: 100 }] },
  { sampledAt: start + 5 * 60_000, processes: [{ name: "app.exe", cpu: 20, memMb: 200 }] },
]);
assert.equal(caseVariantTrends.length, 1, "Windows case variants must not split one executable trend");
assert.equal(caseVariantTrends[0].name, "App.EXE", "the earliest observed spelling remains the display name");
assert.equal(caseVariantTrends[0].averageCpu, 15);
assert.equal(caseVariantTrends[0].cpuObservations, 2);
assert.equal(caseVariantTrends[0].memoryObservations, 2);

const legacyHistory = clearLegacyProcessHistoryCpu([
  { sampledAt: start, processes: [{ name: "app.exe", cpu: 300, memMb: 200 }] },
  { sampledAt: start + fiveMinutes, processes: [{ name: "APP.EXE", cpu: 180, memMb: 220 }] },
]);
assert.deepEqual(legacyHistory.map((sample) => sample.processes[0]), [
  { name: "app.exe", cpu: null, memMb: 200 },
  { name: "APP.EXE", cpu: null, memMb: 220 },
], "legacy CPU values cannot be converted without their original CPU count, but memory remains valid");
const legacyOnlyTrend = buildTrends(legacyHistory)[0];
assert.equal(legacyOnlyTrend.cpuObservations, 0);
assert.equal(legacyOnlyTrend.averageCpu, null, "CPU trend remains explicitly unknown until a new-scale sample arrives");
assert.equal(legacyOnlyTrend.memoryObservations, 2);
const mixedScaleTrend = buildTrends([
  ...legacyHistory,
  { sampledAt: start + 2 * fiveMinutes, processes: [{ name: "App.exe", cpu: 6.25, memMb: 300 }] },
])[0];
assert.equal(mixedScaleTrend.cpuObservations, 1, "old single-core-scale values must not enter the new CPU average");
assert.equal(mixedScaleTrend.averageCpu, 6.25);
assert.equal(mixedScaleTrend.firstCpu, 6.25);
assert.equal(mixedScaleTrend.lastCpu, 6.25);
assert.equal(mixedScaleTrend.memoryObservations, 3, "valid old memory observations remain usable");
assert.equal(mixedScaleTrend.averageMemMb, 240);
assert.equal(mixedScaleTrend.firstCpuSampledAt, start + 2 * fiveMinutes, "CPU trend timing starts at the first sample in its new scale");
assert.equal(mixedScaleTrend.lastCpuSampledAt, start + 2 * fiveMinutes);
assert.equal(mixedScaleTrend.firstMemorySampledAt, start, "memory trend keeps the original retained history span");
assert.equal(mixedScaleTrend.lastMemorySampledAt, start + 2 * fiveMinutes);

const component = fs.readFileSync("src/components/ProcessHistoryCard.tsx", "utf8");
const processSampleIntervalMs = 5 * 60_000;
const processRetentionMs = 7 * 24 * 60 * 60 * 1_000;
const processMaxSamples = Math.floor(processRetentionMs / processSampleIntervalMs) + 1;
assert.equal(processMaxSamples, 2_017, "a 7-day 5-minute window needs 2,017 inclusive samples");
assert.match(component, /const MIN_SAMPLE_INTERVAL_MS = 5 \* 60_000/);
assert.match(component, /const MAX_HISTORY_AGE_MS = 7 \* 24 \* 60 \* 60 \* 1_000/);
assert.match(component, /const MAX_SAMPLES = 2_017/, "the cap includes both endpoints of the seven-day window");
assert.match(component, /const clockMovedBackwards = now < lastSampledAt\.current/);
assert.match(component, /clockMovedBackwards \? pruneSamples\(history\.samples, now\) : history\.samples/);
assert.match(component, /lastSampledAt\.current = now - MIN_SAMPLE_INTERVAL_MS/);
assert.match(component, /if \(!clockMovedBackwards && now - lastSampledAt\.current < MIN_SAMPLE_INTERVAL_MS\) return/);
assert.match(component, /\.sort\(\(a, b\) => a\.sampledAt - b\.sampledAt\)/, "trend input remains chronological");
assert.match(component, /formatProcessHistorySpan\(lastSampledAt - firstSampledAt\)/);
assert.match(component, /hasUnscaledCpuHistory &&[\s\S]*?旧 CPU 单位无法可靠换算/);
assert.match(component, /首末差不代表单一实例/);
assert.match(component, /同名合并/);
assert.match(component, /mergeProcessHistoryMetrics\(rawProcesses\)/, "hydration merges Windows filename case variants");
assert.match(component, /mergeProcessHistoryMetrics\(rawMetrics\)/, "live aggregation uses the same case-insensitive key");
assert.match(component, /const STORAGE_KEY = "aerotune\.process-history\.v1"/);
assert.match(component, /const LEGACY_SCHEMA_VERSION = 1/);
assert.match(component, /const SCHEMA_VERSION = 2/);
assert.match(component, /parseStoredHistory\(raw, SCHEMA_VERSION\)[\s\S]*?parseStoredHistory\(raw, LEGACY_SCHEMA_VERSION\)/, "v1 payload migrates in-place to avoid duplicate localStorage quota usage");
assert.match(component, /expectedVersion === LEGACY_SCHEMA_VERSION[\s\S]*?clearLegacyProcessHistoryCpu\(prunedSamples\)/, "v1 migration invalidates only CPU values whose old scale cannot be recovered");
assert.match(component, /setItem\(STORAGE_KEY,[\s\S]*?version: SCHEMA_VERSION/);
assert.match(component, /const MAX_CPU_PERCENT = 100/);
assert.match(component, /CPU 全机口径趋势/);
assert.match(component, /旧 CPU 单位无法可靠换算[\s\S]*历史内存值仍保留/);

console.log("Process history trends passed: scale-safe v1 migration, time coverage, grouping, and clock rollback recovery");
