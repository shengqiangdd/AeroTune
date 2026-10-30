import assert from "node:assert/strict";
import { summarizeCpuTopology } from "../src/lib/cpuTopology.js";

const cores = Array.from({ length: 256 }, (_, index) => ((index * 37) % 101) + 0.25);

function previousImplementation(values) {
  const logical = values.slice(0, 256).map((value) =>
    Number.isFinite(value) ? Math.max(0, Math.min(100, value)) : 0,
  );
  const mean = logical.length ? logical.reduce((sum, value) => sum + value, 0) / logical.length : null;
  const peak = logical.length ? Math.max(...logical) : null;
  return { logical, mean, peak };
}

assert.deepEqual(summarizeCpuTopology(cores), previousImplementation(cores));

const rounds = 101;
const iterationsPerBatch = 1_000;
let sink = 0;
function batch(fn) {
  const started = process.hrtime.bigint();
  let checksum = 0;
  for (let index = 0; index < iterationsPerBatch; index += 1) {
    const result = fn(cores);
    checksum += result.mean ?? 0;
    checksum += result.peak ?? 0;
    checksum += result.logical[index % cores.length] ?? 0;
  }
  sink += checksum;
  return Number(process.hrtime.bigint() - started) / iterationsPerBatch / 1_000;
}

for (let warmup = 0; warmup < 5; warmup += 1) {
  batch(previousImplementation);
  batch(summarizeCpuTopology);
}

const previousUs = [];
const onePassUs = [];
for (let round = 0; round < rounds; round += 1) {
  if (round % 2 === 0) {
    previousUs.push(batch(previousImplementation));
    onePassUs.push(batch(summarizeCpuTopology));
  } else {
    onePassUs.push(batch(summarizeCpuTopology));
    previousUs.push(batch(previousImplementation));
  }
}

function medianAndP95(values) {
  values.sort((left, right) => left - right);
  return {
    median: values[Math.floor(values.length / 2)],
    p95: values[Math.ceil(values.length * 0.95) - 1],
  };
}

const previous = medianAndP95(previousUs);
const onePass = medianAndP95(onePassUs);
console.log(`CPU topology 256-core microbenchmark (${iterationsPerBatch} calls/batch, ${rounds} alternating batches):`);
console.log(`  previous: median ${previous.median.toFixed(3)} us/call, p95 ${previous.p95.toFixed(3)} us/call`);
console.log(`  one-pass: median ${onePass.median.toFixed(3)} us/call, p95 ${onePass.p95.toFixed(3)} us/call`);
console.log(`  reduction: median ${((1 - onePass.median / previous.median) * 100).toFixed(1)}%, p95 ${((1 - onePass.p95 / previous.p95) * 100).toFixed(1)}% (checksum ${sink.toFixed(1)})`);
