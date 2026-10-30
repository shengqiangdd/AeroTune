import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { performance } from "node:perf_hooks";
import ts from "typescript";

const helperSource = await readFile("src/lib/processTreeRows.ts", "utf8");
const processPageSource = await readFile("src/pages/Processes.tsx", "utf8");
const transpiled = ts.transpileModule(helperSource, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
  reportDiagnostics: true,
});
const errors = (transpiled.diagnostics ?? []).filter(
  (diagnostic) => diagnostic.category === ts.DiagnosticCategory.Error,
);
assert.equal(errors.length, 0, ts.formatDiagnostics(errors, {
  getCurrentDirectory: () => process.cwd(),
  getCanonicalFileName: (fileName) => fileName,
  getNewLine: () => "\n",
}));
const moduleUrl = `data:text/javascript;base64,${Buffer.from(transpiled.outputText).toString("base64")}`;
const { buildProcessTreeRows } = await import(moduleUrl);

assert.match(processPageSource, /import \{ buildProcessTreeRows \} from "\.\.\/lib\/processTreeRows"/);
assert.match(processPageSource, /return buildProcessTreeRows\(filtered\)/);
assert.doesNotMatch(processPageSource, /children\.set\(parent\.pid,\s*\[\.\.\./);
assert.match(helperSource, /siblings\.push\(proc\)/);
assert.doesNotMatch(helperSource, /children\.set\(parent\.pid,\s*\[\.\.\./);

const node = (pid, parent_pid) => Object.freeze({ pid, parent_pid, name: `process-${pid}.exe` });
const highFanout = Array.from({ length: 500 }, (_, index) =>
  index === 0 ? node(1, null) : node(index + 1, 1),
);
const balanced = Array.from({ length: 500 }, (_, index) => {
  const pid = index + 1;
  return node(pid, pid === 1 ? null : Math.floor(pid / 2));
});
const realisticMixed = Array.from({ length: 500 }, (_, index) => {
  const pid = index + 1;
  const firstPidInGroup = Math.floor(index / 10) * 10 + 1;
  return node(pid, index % 10 === 0 ? null : firstPidInGroup);
});
const mixed = [
  node(10, null), node(11, 10), node(12, 10), node(13, 11),
  node(20, 999), node(21, 21), node(30, 31), node(31, 30),
  node(40, null), node(41, 40), node(42, 40), node(43, 40),
];
const filteredSubset = [node(52, 51), node(53, 52), node(54, 999), node(55, 55)];
const fixtures = [
  ["high-fanout-500", highFanout],
  ["balanced-500", balanced],
  ["realistic-fanout-50-groups-500", realisticMixed],
  ["mixed-roots-missing-self-cycles", mixed],
  ["filtered-parent-absent", filteredSubset],
];

function referenceBuildProcessTreeRows(filtered) {
  const byPid = new Map(filtered.map((proc) => [proc.pid, proc]));
  const children = new Map();
  const roots = [];
  for (const proc of filtered) {
    const parent = proc.parent_pid == null ? null : byPid.get(proc.parent_pid);
    if (!parent || parent.pid === proc.pid) roots.push(proc);
    else children.set(parent.pid, [...(children.get(parent.pid) ?? []), proc]);
  }
  const rows = [];
  const seen = new Set();
  const visit = (proc, depth) => {
    if (seen.has(proc.pid)) return;
    seen.add(proc.pid);
    rows.push({ proc, depth });
    for (const child of children.get(proc.pid) ?? []) visit(child, depth + 1);
  };
  roots.forEach((root) => visit(root, 0));
  filtered.forEach((proc) => { if (!seen.has(proc.pid)) visit(proc, 0); });
  return rows;
}

const orderedPidsAndDepths = (rows) => rows.map(({ proc, depth }) => [proc.pid, depth]);
for (const [label, input] of fixtures) {
  const before = input.map(({ pid, parent_pid }) => [pid, parent_pid]);
  const expected = referenceBuildProcessTreeRows(input);
  const actual = buildProcessTreeRows(input);
  assert.deepEqual(orderedPidsAndDepths(actual), orderedPidsAndDepths(expected), label);
  assert.equal(actual.length, input.length, label);
  assert.deepEqual(input.map(({ pid, parent_pid }) => [pid, parent_pid]), before, `${label}: input unchanged`);
}
console.log("Process-tree builder equivalence passed: fanout, balanced, roots, missing parents, self-parent, cycles, filtered parent, and input immutability");

if (process.argv.includes("--bench")) {
  const BATCHES = 101;
  const checksum = (rows) => rows.reduce((sum, row) => (sum + row.proc.pid * 17 + row.depth) >>> 0, 0);
  const percentile95 = (values) => [...values].sort((a, b) => a - b)[Math.ceil(values.length * 0.95) - 1];
  for (const [label, input] of fixtures.slice(0, 3)) {
    const expected = orderedPidsAndDepths(referenceBuildProcessTreeRows(input));
    const actual = orderedPidsAndDepths(buildProcessTreeRows(input));
    assert.deepEqual(actual, expected, `${label}: benchmark parity`);
    let sink = 0;
    const measure = (builder) => {
      const started = performance.now();
      const rows = builder(input);
      const elapsed = performance.now() - started;
      sink = (sink ^ checksum(rows)) >>> 0;
      return elapsed * 1000;
    };
    for (let warmup = 0; warmup < 7; warmup += 1) {
      if (warmup % 2 === 0) {
        measure(referenceBuildProcessTreeRows);
        measure(buildProcessTreeRows);
      } else {
        measure(buildProcessTreeRows);
        measure(referenceBuildProcessTreeRows);
      }
    }
    const referenceUs = [];
    const candidateUs = [];
    for (let batch = 0; batch < BATCHES; batch += 1) {
      if (batch % 2 === 0) {
        referenceUs.push(measure(referenceBuildProcessTreeRows));
        candidateUs.push(measure(buildProcessTreeRows));
      } else {
        candidateUs.push(measure(buildProcessTreeRows));
        referenceUs.push(measure(referenceBuildProcessTreeRows));
      }
    }
    const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
    console.log(
      `Process tree ${label} (${input.length} rows, one build/keystroke): old median/p95=${median(referenceUs).toFixed(1)}/${percentile95(referenceUs).toFixed(1)} us; push median/p95=${median(candidateUs).toFixed(1)}/${percentile95(candidateUs).toFixed(1)} us`,
    );
    assert.ok(Number.isFinite(sink), `${label}: checksum consumed`);
  }
  console.log(`Node/V8 isolated builder benchmark complete (${BATCHES} alternating batches; no React/DOM/IPC)`);
}
