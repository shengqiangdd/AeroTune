import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  MAX_CPU_TOPOLOGY_LOGICAL_PROCESSORS,
  summarizeCpuTopology,
} from "../src/lib/cpuTopology.js";

function reference(values) {
  const logical = (values ?? []).slice(0, 256).map((value) =>
    Number.isFinite(value) ? Math.max(0, Math.min(100, value)) : 0,
  );
  return {
    logical,
    mean: logical.length ? logical.reduce((sum, value) => sum + value, 0) / logical.length : null,
    peak: logical.length ? Math.max(...logical) : null,
  };
}

const cases = [
  undefined,
  null,
  [],
  [18.25],
  [Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, -20, 0, 34.56, 100, 150],
  Array.from({ length: MAX_CPU_TOPOLOGY_LOGICAL_PROCESSORS }, (_, index) => (index * 17) % 121 - 10),
  Array.from({ length: MAX_CPU_TOPOLOGY_LOGICAL_PROCESSORS + 44 }, (_, index) => (index * 29) % 181 - 30),
];

for (const input of cases) {
  assert.deepEqual(summarizeCpuTopology(input), reference(input));
}

const source = Array.from({ length: 300 }, (_, index) => index % 101);
const before = source.slice();
const result = summarizeCpuTopology(source);
assert.equal(result.logical.length, MAX_CPU_TOPOLOGY_LOGICAL_PROCESSORS);
assert.deepEqual(source, before, "normalization does not mutate the telemetry array");

const helper = readFileSync(new URL("../src/lib/cpuTopology.js", import.meta.url), "utf8");
const component = readFileSync(new URL("../src/components/CpuTopologyCard.tsx", import.meta.url), "utf8");
assert.match(helper, /for \(let index = 0; index < count; index \+= 1\)/);
assert.doesNotMatch(helper, /\.slice\(|\.map\(|\.reduce\(|Math\.max\(\.\.\./, "normalization, mean, and peak stay in one pass");
assert.match(component, /summarizeCpuTopology\(stats\?\.per_core\)/);
assert.match(component, /logical\.map\(\(usage, index\) =>/);

console.log("CPU topology passed: exact reference output, 256-core cap, invalid-value handling, and one-pass aggregation");
