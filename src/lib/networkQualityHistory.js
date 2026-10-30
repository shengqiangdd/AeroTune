export const NETWORK_QUALITY_HISTORY_MAX_AGE_MS = 24 * 60 * 60 * 1000;
export const NETWORK_QUALITY_HISTORY_MAX_ENTRIES = 300;
export const NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS = 5 * 60 * 1000;

function isRecord(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parsePingStat(value) {
  if (!isRecord(value)) return null;

  const { ok, avg, sd, loss_pct: lossPct } = value;
  const nullableNonNegative = (number) => {
    if (number === null) return null;
    return typeof number === "number" && Number.isFinite(number) && number >= 0 ? number : undefined;
  };
  const parsedAvg = nullableNonNegative(avg);
  const parsedSd = nullableNonNegative(sd);

  if (
    typeof ok !== "number" || !Number.isSafeInteger(ok) || ok < 0 || ok > 4 ||
    parsedAvg === undefined || parsedSd === undefined ||
    typeof lossPct !== "number" || !Number.isFinite(lossPct) || lossPct < 0 || lossPct > 100 ||
    (ok === 0 && (parsedAvg !== null || parsedSd !== null)) ||
    (ok > 0 && (parsedAvg === null || parsedSd === null))
  ) {
    return null;
  }

  return { ok, avg: parsedAvg, sd: parsedSd, loss_pct: lossPct };
}

export function parseNetworkQualityHistoryEntry(value, now) {
  if (!isRecord(value)) return null;

  const ts = value.ts;
  if (
    typeof ts !== "number" || !Number.isSafeInteger(ts) ||
    ts < now - NETWORK_QUALITY_HISTORY_MAX_AGE_MS || ts > now
  ) {
    return null;
  }

  const gateway = value.gateway === null ? null : parsePingStat(value.gateway);
  const dns = parsePingStat(value.dns);
  const secondDnsMissing = value.second_dns === undefined || value.second_dns === null;
  const secondDns = secondDnsMissing ? undefined : parsePingStat(value.second_dns);

  if (
    (value.gateway !== null && gateway === null) || dns === null ||
    (!secondDnsMissing && secondDns === null)
  ) {
    return null;
  }

  return {
    ts,
    gateway,
    dns,
    ...(secondDns ? { second_dns: secondDns } : {}),
  };
}

export function trimNetworkQualityHistory(entries, now) {
  return entries
    .filter((entry) => entry.ts >= now - NETWORK_QUALITY_HISTORY_MAX_AGE_MS && entry.ts <= now)
    .sort((a, b) => a.ts - b.ts)
    .slice(-NETWORK_QUALITY_HISTORY_MAX_ENTRIES);
}

export function normalizeNetworkQualityEntry(quality, ts) {
  if (!Number.isSafeInteger(ts) || ts < 0) return null;

  const gateway = quality.gateway === null ? null : parsePingStat(quality.gateway);
  const dns = parsePingStat(quality.dns);
  const secondDnsMissing = quality.second_dns === undefined || quality.second_dns === null;
  const secondDns = secondDnsMissing ? undefined : parsePingStat(quality.second_dns);

  if (
    (quality.gateway !== null && gateway === null) || dns === null ||
    (!secondDnsMissing && secondDns === null)
  ) {
    return null;
  }

  return {
    ts,
    gateway,
    dns,
    ...(secondDns ? { second_dns: secondDns } : {}),
  };
}
