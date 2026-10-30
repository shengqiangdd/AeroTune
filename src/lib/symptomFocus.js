const FIXED_TARGETS = new Map([
  ["gpu-temperature-high", "gpu"],
  ["gpu-memory-pressure", "gpu"],
  ["pagefile-pressure", "memory"],
  ["memory-pressure", "memory"],
  ["startup-burden", "startup"],
  ["battery-low", "battery"],
  ["system-disk-low", "disk"],
]);

export function symptomForRecommendation(id) {
  const fixed = FIXED_TARGETS.get(id);
  if (fixed) return fixed;
  if (/^process-\d+-cpu$/.test(id)) return "lag";
  if (/^process-\d+-mem$/.test(id)) return "memory";
  if (/^process-\d+-io$/.test(id)) return "io";
  return null;
}

/** Selects the highest-priority actionable symptom; equal priorities preserve backend order. */
export function rankSymptomFocus(recommendations) {
  let best = null;
  for (const recommendation of recommendations) {
    const symptom = symptomForRecommendation(recommendation.id);
    if (!symptom || !Number.isFinite(recommendation.priority)) continue;
    if (!best || recommendation.priority > best.recommendation.priority) {
      best = { symptom, recommendation };
    }
  }
  return best;
}
