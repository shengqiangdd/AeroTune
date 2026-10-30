export function nearestRankPercentile(values, percentile = 0.95) {
  if (
    !Array.isArray(values)
    || typeof percentile !== "number"
    || !Number.isFinite(percentile)
    || percentile < 0
    || percentile > 1
  ) {
    return null;
  }

  const sorted = values.filter((value) => typeof value === "number" && Number.isFinite(value)).sort((a, b) => a - b);
  if (sorted.length === 0) return null;

  const rank = Math.max(1, Math.ceil(sorted.length * percentile));
  return sorted[rank - 1] ?? null;
}
