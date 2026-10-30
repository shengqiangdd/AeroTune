const APP_ISSUE_KINDS = new Set(["app_crash", "app_hang"]);
const EVENT_FILTER_KINDS = {
  all: null,
  issues: new Set(["app_crash", "app_hang", "unexpected_shutdown"]),
  system: new Set(["unexpected_shutdown", "planned_shutdown"]),
  updates: new Set(["update_installed", "update_failed"]),
};
const DAY_MS = 24 * 60 * 60 * 1000;

export const canonicalAppKey = (label) =>
  typeof label === "string" ? label.trim().replace(/\s+/g, " ").toLowerCase() : "";

const compareText = (left, right) => (left < right ? -1 : left > right ? 1 : 0);

export function buildRecurringAppGroups(events) {
  const groups = new Map();
  for (const event of Array.isArray(events) ? events : []) {
    if (!APP_ISSUE_KINDS.has(event?.kind)) continue;
    const label = typeof event.title === "string" ? event.title.trim().replace(/\s+/g, " ") : "";
    const key = canonicalAppKey(label);
    if (!key) continue;
    const group = groups.get(key) ?? { key, label, count: 0, crashes: 0, hangs: 0 };
    if (compareText(label, group.label) < 0) group.label = label;
    group.count += 1;
    if (event.kind === "app_crash") group.crashes += 1;
    else group.hangs += 1;
    groups.set(key, group);
  }
  return [...groups.values()]
    .filter((group) => group.count > 1)
    .sort((left, right) => right.count - left.count || compareText(left.key, right.key))
    .slice(0, 3);
}

export function filterReliabilityEvents(events, eventFilter, appKey = null) {
  const kinds = EVENT_FILTER_KINDS[eventFilter] ?? EVENT_FILTER_KINDS.all;
  return (Array.isArray(events) ? events : []).filter((event) => {
    if (kinds && !kinds.has(event.kind)) return false;
    if (appKey !== null && (!APP_ISSUE_KINDS.has(event.kind) || canonicalAppKey(event.title) !== appKey)) return false;
    return true;
  });
}

export function updateReliabilityFilters(current, changes, pageSize = 12) {
  const safePageSize = Number.isSafeInteger(pageSize) && pageSize > 0 ? pageSize : 12;
  return { ...current, ...changes, visibleCount: safePageSize };
}

function dayKeyAt(timestampMs, timeZone) {
  const parts = new Intl.DateTimeFormat("en-US", {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
  }).formatToParts(new Date(timestampMs));
  const values = Object.fromEntries(parts.filter((part) => part.type !== "literal").map((part) => [part.type, part.value]));
  return `${values.year.padStart(4, "0")}-${values.month}-${values.day}`;
}

function enumerateDayKeys(firstKey, lastKey) {
  const [firstYear, firstMonth, firstDay] = firstKey.split("-").map(Number);
  const [lastYear, lastMonth, lastDay] = lastKey.split("-").map(Number);
  const cursor = new Date(Date.UTC(firstYear, firstMonth - 1, firstDay));
  const last = Date.UTC(lastYear, lastMonth - 1, lastDay);
  const keys = [];
  while (cursor.getTime() <= last && keys.length < 93) {
    keys.push(`${cursor.getUTCFullYear().toString().padStart(4, "0")}-${String(cursor.getUTCMonth() + 1).padStart(2, "0")}-${String(cursor.getUTCDate()).padStart(2, "0")}`);
    cursor.setUTCDate(cursor.getUTCDate() + 1);
  }
  return keys;
}

/** Daily local-calendar buckets for one app, restricted to the loaded query window. */
export function buildAppIssueDayBuckets(events, appKey, rangeDays, rangeEndMs, timeZone) {
  if (!appKey || !Number.isSafeInteger(rangeDays) || rangeDays < 1 || rangeDays > 90 || !Number.isFinite(rangeEndMs)) return [];
  const endMs = Math.floor(rangeEndMs);
  const startMs = endMs - rangeDays * DAY_MS;
  const firstKey = dayKeyAt(startMs, timeZone);
  const lastKey = dayKeyAt(endMs, timeZone);
  const buckets = enumerateDayKeys(firstKey, lastKey).map((dayKey) => ({ dayKey, crashes: 0, hangs: 0 }));
  const byDay = new Map(buckets.map((bucket) => [bucket.dayKey, bucket]));
  for (const event of Array.isArray(events) ? events : []) {
    if (!APP_ISSUE_KINDS.has(event?.kind) || canonicalAppKey(event.title) !== appKey) continue;
    if (!Number.isFinite(event.ts) || event.ts <= 0) continue;
    const timestampMs = event.ts * 1000;
    if (timestampMs < startMs || timestampMs > endMs) continue;
    const bucket = byDay.get(dayKeyAt(timestampMs, timeZone));
    if (!bucket) continue;
    if (event.kind === "app_crash") bucket.crashes += 1;
    else bucket.hangs += 1;
  }
  return buckets;
}

export function assessReliabilitySnapshot(snapshot) {
  const sources = Array.isArray(snapshot?.sources) ? snapshot.sources : [];
  const unavailable = sources.filter((source) => !source.available);
  const capped = sources.filter((source) => source.limit_reached);
  const omitted = Number.isFinite(snapshot?.omitted_estimate) && snapshot.omitted_estimate > 0;
  const invalid = Number.isFinite(snapshot?.invalid_rows) && snapshot.invalid_rows > 0;
  const truncated = omitted || capped.length > 0;
  const incomplete = unavailable.length > 0 || truncated || invalid;
  return {
    noEvents: !Array.isArray(snapshot?.events) || snapshot.events.length === 0,
    incomplete,
    truncated,
    unavailable: unavailable.length > 0,
    invalidRows: invalid,
  };
}
