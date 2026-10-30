import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import {
  appendDiskSamples,
  diskHistoryNeedsRefresh,
  DISK_HISTORY_MAX_SAMPLES_PER_VOLUME,
  indexDiskHistory,
} from "../src/lib/diskHistory.js";

const volumeCount = Number(process.argv[2] ?? 16);
const iterations = Number(process.argv[3] ?? 300);
const repeats = Number(process.argv[4] ?? 5);
if (!Number.isInteger(volumeCount) || volumeCount < 1 || volumeCount > 16) throw new Error("volumeCount must be 1..16");
if (!Number.isInteger(iterations) || iterations < 1) throw new Error("iterations must be positive");
if (!Number.isInteger(repeats) || repeats < 3) throw new Error("repeats must be at least 3");

const now = Date.now();
const mounts = Array.from({ length: volumeCount }, (_, i) => `${String.fromCharCode(67 + i)}:`);
const history = mounts.flatMap((mount, drive) => Array.from({ length: DISK_HISTORY_MAX_SAMPLES_PER_VOLUME }, (_, sample) => ({
  ts: now - (DISK_HISTORY_MAX_SAMPLES_PER_VOLUME - sample) * 1000,
  mount,
  totalGb: 500 + drive,
  freeGb: 250 + drive,
})));
const disks = mounts.map((mount, drive) => ({ mount, total_gb: 500 + drive, used_gb: 250 }));
const index = indexDiskHistory(history, now);
assert.equal(diskHistoryNeedsRefresh(index, disks, now + 100), false);

function measure(fn) {
  for (let i = 0; i < 20; i++) fn();
  const started = performance.now();
  for (let i = 0; i < iterations; i++) fn();
  return performance.now() - started;
}

const fullSanitizeMs = [];
const refreshGateMs = [];
for (let repeat = 0; repeat < repeats; repeat++) {
  fullSanitizeMs.push(measure(() => appendDiskSamples(history, disks, now + 100)));
  refreshGateMs.push(measure(() => diskHistoryNeedsRefresh(index, disks, now + 100)));
}
const median = (values) => [...values].sort((a, b) => a - b)[Math.floor(values.length / 2)];
const sanitizeMedianMs = median(fullSanitizeMs);
const gateMedianMs = median(refreshGateMs);
console.log(JSON.stringify({
  node: process.version,
  platform: process.platform,
  volumes: volumeCount,
  samples: history.length,
  iterationsPerRepeat: iterations,
  repeats,
  fullSanitizeMedianMs: +sanitizeMedianMs.toFixed(2),
  refreshGateMedianMs: +gateMedianMs.toFixed(2),
  isolatedSpeedup: +(sanitizeMedianMs / Math.max(gateMedianMs, 0.001)).toFixed(1),
  gateInvocationsThatNeededRefresh: 0,
}, null, 2));
