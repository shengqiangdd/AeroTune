export const MAX_INTERACTION_LATENCY_SAMPLES = 31;
export const INTERACTION_LATENCY_P95_UPDATE_INTERVAL = 5;

export function recordInteractionLatencySample(
  samples,
  value,
  totalSampleCount,
  maxSamples = MAX_INTERACTION_LATENCY_SAMPLES,
  updateEvery = INTERACTION_LATENCY_P95_UPDATE_INTERVAL,
) {
  if (
    !Array.isArray(samples)
    || typeof value !== "number"
    || !Number.isFinite(value)
    || value < 0
    || !Number.isInteger(totalSampleCount)
    || totalSampleCount < 0
    || !Number.isInteger(maxSamples)
    || maxSamples < 1
    || !Number.isInteger(updateEvery)
    || updateEvery < 1
  ) {
    return null;
  }

  samples.push(value);
  if (samples.length > maxSamples) samples.splice(0, samples.length - maxSamples);

  const nextSampleCount = totalSampleCount + 1;
  return {
    totalSampleCount: nextSampleCount,
    rollingSampleCount: samples.length,
    shouldUpdateP95: nextSampleCount % updateEvery === 0 || nextSampleCount === maxSamples,
  };
}
