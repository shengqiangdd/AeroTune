import assert from "node:assert/strict";
import {
  assessReliabilitySnapshot,
  buildAppIssueDayBuckets,
  buildRecurringAppGroups,
  filterReliabilityEvents,
  updateReliabilityFilters,
} from "../src/lib/reliabilityTimeline.js";

const events = [
  { kind: "app_crash", title: "Example App", ts: 1704110400 },
  { kind: "app_hang", title: " example   app ", ts: 1704283200 },
  { kind: "app_crash", title: "EXAMPLE APP", ts: 1704672000 },
  { kind: "update_failed", title: "Example App", ts: 1704283200 },
  { kind: "app_crash", title: "Other App", ts: 1704283200 },
];

const recurring = buildRecurringAppGroups(events);
assert.equal(recurring.length, 1, "case and whitespace variants should aggregate");
assert.equal(recurring[0].count, 3);
assert.equal(recurring[0].crashes, 2);
assert.equal(recurring[0].hangs, 1);

const appKey = recurring[0].key;
assert.deepEqual(filterReliabilityEvents(events, "all", appKey).map((event) => event.kind), ["app_crash", "app_hang", "app_crash"]);
assert.equal(filterReliabilityEvents(events, "updates", appKey).length, 0);

const existing = { eventFilter: "all", appKey: null, visibleCount: 36 };
assert.deepEqual(updateReliabilityFilters(existing, { appKey }, 12), { eventFilter: "all", appKey, visibleCount: 12 });
assert.deepEqual(updateReliabilityFilters(existing, { eventFilter: "issues", appKey: null }, 12), { eventFilter: "issues", appKey: null, visibleCount: 12 });

const start = Date.UTC(2024, 0, 1, 0, 0, 0);
const end = Date.UTC(2024, 0, 8, 0, 0, 0);
const bucketEvents = [
  { kind: "app_crash", title: "Tool", ts: start / 1000 + 12 * 3600 },
  { kind: "app_hang", title: "tool", ts: Date.UTC(2024, 0, 3, 12) / 1000 },
  { kind: "app_crash", title: "Tool", ts: end / 1000 },
  { kind: "planned_shutdown", title: "Tool", ts: Date.UTC(2024, 0, 4) / 1000 },
];
const buckets = buildAppIssueDayBuckets(bucketEvents, "tool", 7, end, "UTC");
assert.equal(buckets.length, 8, "inclusive local-calendar endpoints produce each represented day");
assert.equal(buckets.find((bucket) => bucket.dayKey === "2024-01-01").crashes, 1);
assert.equal(buckets.find((bucket) => bucket.dayKey === "2024-01-03").hangs, 1);
assert.equal(buckets.find((bucket) => bucket.dayKey === "2024-01-08").crashes, 1);
assert.equal(buckets.reduce((sum, bucket) => sum + bucket.crashes + bucket.hangs, 0), 3, "system events are excluded from app issue trends");

const complete = { events: [], sources: [{ available: true, limit_reached: false }], omitted_estimate: 0, invalid_rows: 0 };
assert.equal(assessReliabilitySnapshot(complete).incomplete, false);
assert.equal(assessReliabilitySnapshot({ ...complete, sources: [{ available: false, limit_reached: false }] }).unavailable, true);
assert.equal(assessReliabilitySnapshot({ ...complete, omitted_estimate: 2 }).truncated, true);
assert.equal(assessReliabilitySnapshot({ ...complete, invalid_rows: 1 }).incomplete, true);

console.log("PASS recurring-app aggregation, filters, pagination reset, local-day buckets, and incomplete-source indicators");
