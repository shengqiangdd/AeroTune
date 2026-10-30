import assert from "node:assert/strict";
import fs from "node:fs";
import {
  NETWORK_QUALITY_HISTORY_MAX_AGE_MS,
  NETWORK_QUALITY_HISTORY_MAX_ENTRIES,
  NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS,
  normalizeNetworkQualityEntry,
  parseNetworkQualityHistoryEntry,
} from "../src/lib/networkQualityHistory.js";

const maxSamplesInRetentionWindow =
  Math.floor(NETWORK_QUALITY_HISTORY_MAX_AGE_MS / NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS) + 1;
assert.equal(NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS, 5 * 60 * 1000);
assert.equal(maxSamplesInRetentionWindow, 289);
assert.ok(
  maxSamplesInRetentionWindow <= NETWORK_QUALITY_HISTORY_MAX_ENTRIES,
  "the sample cap must cover a full 24 hours at the minimum cadence",
);

const now = 1_800_000_000_000;
const dns = { ok: 4, avg: 12.5, sd: 1.2, loss_pct: 0 };
const qualityWithoutSecondDns = {
  gateway: null,
  dns,
  second_dns: null,
  summary: { label: "网关探测不可用", detail: "test", tone: "amber" },
};

const liveEntry = normalizeNetworkQualityEntry(qualityWithoutSecondDns, now);
assert.ok(liveEntry, "a valid result with nullable second DNS should be recordable");
assert.equal(liveEntry.gateway, null);
assert.deepEqual(liveEntry.dns, dns);
assert.equal(Object.hasOwn(liveEntry, "second_dns"), false, "absent DNS 2 is normalized by omission");

// Stored legacy/backend entries can carry explicit JSON null for Option<PingStat>.
const persistedEntry = { ...liveEntry, second_dns: null };
const hydrated = parseNetworkQualityHistoryEntry(persistedEntry, now);
assert.ok(hydrated, "a persisted null second DNS must not discard the whole sample");
assert.equal(hydrated.gateway, null);
assert.equal(hydrated.dns.avg, 12.5);
assert.equal(Object.hasOwn(hydrated, "second_dns"), false);

const legacyEntryWithoutSecondDns = parseNetworkQualityHistoryEntry(liveEntry, now);
assert.ok(legacyEntryWithoutSecondDns, "an omitted optional second DNS remains supported");
assert.equal(parseNetworkQualityHistoryEntry({ ...persistedEntry, second_dns: "bad" }, now), null);
assert.equal(
  normalizeNetworkQualityEntry({ ...qualityWithoutSecondDns, second_dns: "bad" }, now),
  null,
  "malformed non-null DNS 2 still invalidates the sample",
);
assert.equal(
  parseNetworkQualityHistoryEntry({ ...persistedEntry, dns: { ...dns, ok: 5 } }, now),
  null,
  "impossible probe counts are rejected",
);

const component = fs.readFileSync("src/components/NetworkQualityHistory.tsx", "utf8");
const api = fs.readFileSync("src/lib/api.ts", "utf8");
assert.match(api, /second_dns\?: PingStat \| null;/);
assert.match(component, /parseNetworkQualityHistoryEntry\(entry, now\)/);
assert.match(component, /normalizeNetworkQualityEntry\(quality, ts\)/);
assert.match(component, /MIN_SAMPLE_INTERVAL_MS = NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS/);
assert.match(component, /最短采样间隔为 5 分钟，300 条上限可覆盖 24 小时连续观察/);
assert.match(component, /至少 5 分钟的间隔记录网关与 DNS/);

console.log("Network history passed: nullable DNS, hydration, and 24-hour sampling-capacity boundaries");
