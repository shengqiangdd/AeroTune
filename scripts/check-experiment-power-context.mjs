import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  capturePowerContext,
  comparablePowerSource,
  isPowerContextSummary,
  powerSourceChanged,
  stablePowerSource,
  summarizePowerContext,
} from "../src/lib/experimentPowerContext.js";

const ac = capturePowerContext({
  battery_status_ok: true,
  battery: { ac_connected: true },
  effective_power_mode: "balanced",
});
assert.deepEqual(ac, { powerSource: "ac", effectivePowerMode: "balanced" });
assert.deepEqual(capturePowerContext({
  battery_status_ok: true,
  battery: { ac_connected: false },
  effective_power_mode: "battery_saver",
}), { powerSource: "battery", effectivePowerMode: "battery_saver" });
assert.equal(capturePowerContext({
  battery_status_ok: false,
  battery: { ac_connected: true },
  effective_power_mode: "balanced",
}).powerSource, "unknown", "a failed battery query must not trust stale fields");
assert.equal(capturePowerContext({ battery: { ac_connected: true }, effective_power_mode: "balanced" }).powerSource, "unknown", "missing query-health status does not trust populated battery fields");
assert.equal(capturePowerContext({ battery: null, effective_power_mode: "future_mode" }).powerSource, "unknown");
assert.equal(capturePowerContext({ battery: null, effective_power_mode: "future_mode" }).effectivePowerMode, null);

const context = summarizePowerContext([
  ac,
  { powerSource: "battery", effectivePowerMode: "battery_saver" },
  { powerSource: "unknown", effectivePowerMode: null },
  { powerSource: "other", effectivePowerMode: "__proto__" },
]);
assert.equal(context.sampleCount, 4);
assert.deepEqual(context.powerSourceCounts, { ac: 1, battery: 1, unknown: 2 });
assert.deepEqual(context.effectivePowerModeCounts, [
  { mode: "battery_saver", count: 1 },
  { mode: "balanced", count: 1 },
]);
assert.equal(context.effectivePowerModeUnknownCount, 2);
assert.equal(
  Object.values(context.powerSourceCounts).reduce((sum, count) => sum + count, 0),
  context.sampleCount,
);
assert.equal(
  context.effectivePowerModeCounts.reduce((sum, entry) => sum + entry.count, context.effectivePowerModeUnknownCount),
  context.sampleCount,
);
assert.equal(isPowerContextSummary(context), true);
assert.equal(isPowerContextSummary({ ...context, powerSourceCounts: { ...context.powerSourceCounts, unknown: 0 } }), false);
assert.equal(isPowerContextSummary({ ...context, sampleCount: 2 }), false);

const stableAc = summarizePowerContext(Array.from({ length: 3 }, () => ac));
const stableBattery = summarizePowerContext(Array.from({ length: 3 }, () => ({ powerSource: "battery", effectivePowerMode: null })));
const mixed = summarizePowerContext([ac, { powerSource: "battery", effectivePowerMode: null }, ac]);
const unknown = summarizePowerContext(Array.from({ length: 3 }, () => ({ powerSource: "unknown", effectivePowerMode: null })));
const tooShort = summarizePowerContext([ac, ac]);
const empty = summarizePowerContext([]);
assert.equal(stablePowerSource(stableAc), "ac");
assert.equal(stablePowerSource(stableBattery), "battery");
assert.equal(stablePowerSource(mixed), null, "mixed windows are descriptive, not classified as stable");
assert.equal(stablePowerSource(unknown), null);
assert.equal(stablePowerSource(tooShort), null);
assert.equal(stablePowerSource(empty), null);
assert.equal(comparablePowerSource(stableAc, stableAc), true);
assert.equal(comparablePowerSource(stableBattery, stableBattery), true);
assert.equal(comparablePowerSource(stableAc, stableBattery), false);
assert.equal(comparablePowerSource(stableAc, mixed), false);
assert.equal(comparablePowerSource(stableAc, unknown), false);
assert.equal(comparablePowerSource(stableAc, tooShort), false);
assert.equal(comparablePowerSource(undefined, stableAc), false);
assert.equal(comparablePowerSource(stableAc, undefined), false);
assert.equal(powerSourceChanged(stableAc, stableBattery), true);
assert.equal(powerSourceChanged(stableAc, stableAc), false);
assert.equal(powerSourceChanged(stableAc, mixed), false);
assert.equal(powerSourceChanged(stableAc, unknown), false);
assert.equal(powerSourceChanged(stableAc, empty), false);

const lab = readFileSync("src/components/OptimizationLab.tsx", "utf8");
assert.match(lab, /powerContext\?: PowerContextSummary/);
assert.match(lab, /powerContext == null/);
assert.match(lab, /旧记录无该上下文/);
assert.match(lab, /供电上下文不同，结果仅作观察差异/);
assert.match(lab, /Windows 有效能效态样本/);
assert.match(lab, /Windows 能效态：/);
assert.match(lab, /grid gap-2 sm:grid-cols-2/);
assert.match(lab, /break-words text-\[10px\]/);
assert.match(lab, /role="note" className/);
assert.match(lab, /history\.filter\(\(record\) =>\s*comparablePowerSource\(record\.baseline\.powerContext, record\.after\?\.powerContext\)/);
assert.match(lab, /来源不同、混合、未知或旧记录缺失上下文时排除/);
assert.match(lab, /analyzeExperimentConsistency\(payload\)/);
assert.match(lab, /power_context: record\.baseline\.powerContext \?\? null/);
assert.match(lab, /power_context: record\.after\.powerContext \?\? null/);
const api = readFileSync("src/lib/api.ts", "utf8");
assert.equal((api.match(/power_context: import\("\.\/experimentPowerContext"\)\.PowerContextSummary \| null/g) ?? []).length, 2);
const backend = readFileSync("src-tauri/src/experiment_insight.rs", "utf8");
assert.match(backend, /pub power_context: Option<PowerContextInput>/);
assert.match(backend, /source\.is_none\(\) \|\| source != stable_power_source\(after\)/);
assert.match(backend, /context\.sample_count != window\.sample_count/);

console.log("Experiment power context passed: AC/DC capture, unknown handling, stable comparison, legacy history, and read-only wording");
