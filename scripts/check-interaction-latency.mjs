import assert from "node:assert/strict";
import { recordInteractionLatencySample } from "../src/lib/interactionLatencySampling.js";
import { nearestRankPercentile } from "../src/lib/nearestRankPercentile.js";

const cases = [
  ["empty samples", [], 0.95, null],
  ["single sample", [12.5], 0.95, 12.5],
  ["nearest-rank P95 uses the maximum for five samples", [9, 1, 7, 3, 5], 0.95, 9],
  ["median uses one-based nearest rank", [4, 1, 3, 2], 0.5, 2],
  ["zero percentile selects the minimum", [8, 3, 5], 0, 3],
  ["100th percentile selects the maximum", [8, 3, 5], 1, 8],
  ["non-finite samples are ignored", [4, Number.NaN, 10, Number.POSITIVE_INFINITY, 7], 0.95, 10],
  ["all-invalid samples return null", [Number.NaN, Number.NEGATIVE_INFINITY], 0.95, null],
  ["invalid percentile below zero returns null", [1, 2], -0.01, null],
  ["invalid percentile above one returns null", [1, 2], 1.01, null],
  ["non-finite percentile returns null", [1, 2], Number.NaN, null],
  ["non-array input returns null", null, 0.95, null],
];

for (const [name, values, percentile, expected] of cases) {
  assert.equal(nearestRankPercentile(values, percentile), expected, name);
}

const unsorted = [15, 2, 11, 4, 7];
assert.equal(nearestRankPercentile(unsorted, 0.95), 15, "P95 remains correct on unsorted interaction samples");
assert.deepEqual(unsorted, [15, 2, 11, 4, 7], "percentile calculation does not mutate the rolling sample buffer");

const rollingSamples = [];
let totalSampleCount = 0;
const p95UpdateCounts = [];
for (let value = 1; value <= 35; value += 1) {
  const update = recordInteractionLatencySample(rollingSamples, value, totalSampleCount);
  assert.ok(update, `sample ${value} is accepted`);
  totalSampleCount = update.totalSampleCount;
  if (update.shouldUpdateP95) p95UpdateCounts.push(totalSampleCount);
}
assert.deepEqual(p95UpdateCounts, [5, 10, 15, 20, 25, 30, 31, 35], "P95 updates every five samples and at sample 31");
assert.equal(rollingSamples.length, 31, "rolling sample window is capped at 31");
assert.deepEqual(rollingSamples, Array.from({ length: 31 }, (_, index) => index + 5), "oldest samples are evicted from the rolling window");
assert.equal(recordInteractionLatencySample(rollingSamples, Number.NaN, totalSampleCount), null, "invalid measurements are ignored");
assert.deepEqual(rollingSamples, Array.from({ length: 31 }, (_, index) => index + 5), "invalid measurements leave the window unchanged");

console.log(`Interaction-latency nearest-rank P95 passed: ${cases.length + 7} checks`);
