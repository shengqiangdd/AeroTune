import assert from "node:assert/strict";
import fs from "node:fs";
import {
  appendDiskSamples,
  diskHistoryNeedsRefresh,
  DISK_HISTORY_MAX_SAMPLES_PER_VOLUME,
  DISK_HISTORY_RETENTION_MS,
  DISK_HISTORY_SAMPLE_INTERVAL_MS,
  indexDiskHistory,
  normalizeDriveMount,
  sanitizeDiskHistory,
  validatedDiskFreeGb,
} from "../src/lib/diskHistory.js";

const now = 2_000_000_000_000;
assert.equal(
  DISK_HISTORY_MAX_SAMPLES_PER_VOLUME,
  Math.floor(DISK_HISTORY_RETENTION_MS / DISK_HISTORY_SAMPLE_INTERVAL_MS) + 1,
  "the cap must hold every sample from both inclusive retention-window endpoints",
);
const fullRetentionWindow = Array.from({ length: DISK_HISTORY_MAX_SAMPLES_PER_VOLUME }, (_, index) => ({
  ts: now - DISK_HISTORY_RETENTION_MS + index * DISK_HISTORY_SAMPLE_INTERVAL_MS,
  mount: "C:", totalGb: 100, freeGb: 50,
}));
const retainedWindow = sanitizeDiskHistory(fullRetentionWindow, now);
assert.equal(retainedWindow.length, DISK_HISTORY_MAX_SAMPLES_PER_VOLUME);
assert.equal(retainedWindow[0].ts, now - DISK_HISTORY_RETENTION_MS, "retain the oldest inclusive boundary sample");
assert.equal(retainedWindow.at(-1).ts, now, "retain the newest inclusive boundary sample");

assert.equal(normalizeDriveMount("c:\\"), "C:");
assert.equal(normalizeDriveMount("D:/"), "D:");
assert.equal(normalizeDriveMount("C:\\Users\\me"), null, "a user path must never be persisted as a volume key");
assert.equal(normalizeDriveMount("\\\\server\\share"), null);

assert.equal(validatedDiskFreeGb(500, 300), 200);
assert.equal(validatedDiskFreeGb(500, 500), 0, "a full disk is a valid zero-free-space reading");
assert.equal(validatedDiskFreeGb(0, 0), null, "zero-capacity telemetry is unknown");
assert.equal(validatedDiskFreeGb(500, -1), null, "negative used space is invalid");
assert.equal(validatedDiskFreeGb(500, 501), null, "used space above capacity is invalid");
assert.equal(validatedDiskFreeGb(Number.NaN, 10), null, "non-finite capacity is invalid");

const first = appendDiskSamples([], [{ mount: "C:\\", total_gb: 500, used_gb: 300 }], now);
assert.equal(first.changed, true);
assert.equal(first.samples.length, 1);
assert.deepEqual(Object.keys(first.samples[0]).sort(), ["freeGb", "mount", "totalGb", "ts"]);
assert.equal(first.samples[0].freeGb, 200);

const historyIndex = indexDiskHistory(first.samples, now);
const disks = [{ mount: "C:", total_gb: 500, used_gb: 305 }];
assert.equal(diskHistoryNeedsRefresh(historyIndex, disks, now + DISK_HISTORY_SAMPLE_INTERVAL_MS - 1), false);
assert.equal(diskHistoryNeedsRefresh(historyIndex, disks, now + DISK_HISTORY_SAMPLE_INTERVAL_MS), true);
assert.equal(diskHistoryNeedsRefresh(historyIndex, [{ mount: "C:", total_gb: 250, used_gb: 100 }], now + 1000), true, "capacity change triggers a new series immediately");
assert.equal(diskHistoryNeedsRefresh(historyIndex, [{ mount: "D:", total_gb: 250, used_gb: 100 }], now + 1000), true, "new drive is sampled immediately");
const tooSoon = appendDiskSamples(first.samples, disks, now + DISK_HISTORY_SAMPLE_INTERVAL_MS - 1);
assert.equal(tooSoon.changed, false, "samples must respect the minimum interval");
const due = appendDiskSamples(first.samples, [{ mount: "C:", total_gb: 500, used_gb: 310 }], now + DISK_HISTORY_SAMPLE_INTERVAL_MS);
assert.equal(due.samples.length, 2);
assert.equal(due.samples.at(-1).freeGb, 190);

const replacedVolume = appendDiskSamples(first.samples, [{ mount: "C:", total_gb: 250, used_gb: 100 }], now + DISK_HISTORY_SAMPLE_INTERVAL_MS);
assert.equal(replacedVolume.samples.length, 1, "a large capacity change starts a new series for that drive letter");
assert.equal(replacedVolume.samples[0].totalGb, 250);

const sanitized = sanitizeDiskHistory({ version: 1, samples: [
  { ts: now, mount: "E:", totalGb: 100, freeGb: 30 },
  { ts: now - 1, mount: "C:", totalGb: 100, freeGb: 20 },
  { ts: now, mount: "C:\\Users\\me", totalGb: 100, freeGb: 10 },
  { ts: now + 2, mount: "F:", totalGb: 100, freeGb: 101 },
  { ts: now - DISK_HISTORY_RETENTION_MS - 1, mount: "G:", totalGb: 100, freeGb: 10 },
  { ts: now + 5, mount: "H:", totalGb: 100, freeGb: 10 },
] }, now);
assert.deepEqual(new Set(sanitized.map((sample) => sample.mount)), new Set(["C:", "E:"]));
assert.equal(sanitized.find((sample) => sample.mount === "C:").ts, now - 1);

const dense = Array.from({ length: DISK_HISTORY_MAX_SAMPLES_PER_VOLUME + 5 }, (_, index) => ({
  ts: now - (DISK_HISTORY_MAX_SAMPLES_PER_VOLUME - index) * 1000,
  mount: "C:", totalGb: 100, freeGb: 50,
}));
assert.equal(sanitizeDiskHistory(dense, now).length, DISK_HISTORY_MAX_SAMPLES_PER_VOLUME);

const card = fs.readFileSync("src/components/DiskSpaceHistoryCard.tsx", "utf8");
assert.match(card, /validatedDiskFreeGb\(currentDisk\.total_gb, currentDisk\.used_gb\)/);
assert.doesNotMatch(card, /samples\.at\(-1\)\?\.freeGb \?\? null/, "historical values must not be labeled as current");
assert.match(card, /当前数据不可用/);
assert.match(card, /当前未检测到/);
assert.match(card, /const samples = useMemo\([\s\S]*?\[activeMount, history\]/, "selected history sample identity stays stable across live stats updates");
assert.match(card, /const DiskHistoryChart = memo\(function DiskHistoryChart/);
assert.match(card, /<DiskHistoryChart mount=\{activeMount\} samples=\{samples\} \/>/);

console.log("PASS disk history normalization, freshness labels, telemetry bounds, interval, volume reset, retention, and cap");
