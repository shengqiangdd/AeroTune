export const BATTERY_SAMPLE_INTERVAL_MS = 60_000;
export const BATTERY_MAX_SAMPLE_GAP_MS = BATTERY_SAMPLE_INTERVAL_MS * 2.5;
export const AC_DRAIN_MAX_SAMPLE_GAP_MS = BATTERY_SAMPLE_INTERVAL_MS * 1.5;
export const AC_DRAIN_MIN_COVERAGE_MS = 15 * 60_000;
export const AC_DRAIN_MIN_DROP_PERCENT = 2;
export const AC_DRAIN_MIN_SAMPLES = 12;
export const AC_DRAIN_TREND_WINDOW_MS = 30 * 60_000;
const AC_DRAIN_MIN_HALF_DROP_PERCENT = 0.5;
const MIN_COVERAGE_MS = 10 * 60_000;
const MIN_DROP_PERCENT = 2;

export function batteryAcState(battery) {
  return typeof battery?.ac_connected === "boolean" ? battery.ac_connected : null;
}

export function batteryQueryFailed(stats) {
  return stats?.battery_status_ok === false;
}

function percentile(sortedValues, fraction) {
  if (sortedValues.length === 1) return sortedValues[0];
  const index = (sortedValues.length - 1) * fraction;
  const lower = Math.floor(index);
  const upper = Math.ceil(index);
  const blend = index - lower;
  return sortedValues[lower] * (1 - blend) + sortedValues[upper] * blend;
}

function medianValue(values) {
  if (!values.length) return null;
  const sorted = values.slice().sort((a, b) => a - b);
  const middle = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[middle] : (sorted[middle - 1] + sorted[middle]) / 2;
}

export function estimateDischarge(samples, currentPercent) {
  if (
    currentPercent == null ||
    !Number.isFinite(currentPercent) ||
    currentPercent < 0 ||
    currentPercent > 100
  ) return null;

  // Only use the latest uninterrupted run of known battery-powered samples.
  // A stale gap can span sleep, an app pause, or unseen charging and must not
  // be averaged as if the battery drained continuously while observed.
  const run = [];
  for (let i = samples.length - 1; i >= 0; i -= 1) {
    const sample = samples[i];
    if (sample.acConnected !== false) break;
    const newer = run[run.length - 1];
    if (newer && newer.timestamp - sample.timestamp > BATTERY_MAX_SAMPLE_GAP_MS) break;
    run.push(sample);
  }
  run.reverse();
  if (run.length < 2) return null;

  const first = run[0];
  const last = run[run.length - 1];
  const spanMs = last.timestamp - first.timestamp;
  const drop = first.percent - last.percent;
  if (spanMs < MIN_COVERAGE_MS || drop < MIN_DROP_PERCENT) return null;

  const observedRate = drop / (spanMs / (60 * 60 * 1000));
  if (!Number.isFinite(observedRate) || observedRate <= 0) return null;

  // Sample representative rolling windows to describe range without quadratic work.
  const windowRates = [];
  const windowLengths = [10, 30, 60, 120, 240, 480, 960].map((minutes) => minutes * 60_000);
  for (let end = 1; end < run.length; end += 1) {
    for (const windowMs of windowLengths) {
      const target = run[end].timestamp - windowMs;
      if (target < run[0].timestamp) continue;
      let low = 0;
      let high = end - 1;
      let start = -1;
      while (low <= high) {
        const middle = Math.floor((low + high) / 2);
        if (run[middle].timestamp <= target) {
          start = middle;
          low = middle + 1;
        } else {
          high = middle - 1;
        }
      }
      if (start < 0) continue;
      const elapsed = run[end].timestamp - run[start].timestamp;
      const windowDrop = run[start].percent - run[end].percent;
      if (elapsed >= MIN_COVERAGE_MS && windowDrop >= MIN_DROP_PERCENT) {
        windowRates.push(windowDrop / (elapsed / (60 * 60 * 1000)));
      }
    }
  }
  windowRates.push(observedRate);

  const sortedRates = windowRates.slice().sort((a, b) => a - b);
  const lowRate = percentile(sortedRates, 0.1);
  const highRate = percentile(sortedRates, 0.9);
  const hours = currentPercent / observedRate;
  const rangeHours = [currentPercent / highRate, currentPercent / lowRate];
  const spanMinutes = spanMs / 60_000;

  const quality =
    spanMinutes >= 240 && run.length >= 8 && drop >= 8 && highRate / lowRate <= 1.6
      ? "高"
      : spanMinutes >= 60 && run.length >= 4 && drop >= 4 && highRate / lowRate <= 2.5
        ? "中"
        : "低";

  return { hours, rangeHours, percentPerHour: observedRate, quality, sampleCount: run.length, spanMinutes };
}

/**
 * Observe a recent run whose sampled points report AC and show a distributed decline.
 * Unobserved instants between samples are unknowable; this does not diagnose hardware.
 */
export function estimateAcDrain(samples) {
  if (!Array.isArray(samples)) return null;
  const run = [];
  for (let i = samples.length - 1; i >= 0; i -= 1) {
    const sample = samples[i];
    if (
      sample?.acConnected !== true ||
      !Number.isFinite(sample.percent) ||
      sample.percent < 0 ||
      sample.percent > 100 ||
      !Number.isFinite(sample.timestamp)
    ) break;
    const newer = run[run.length - 1];
    if (newer && (
      newer.timestamp - sample.timestamp > AC_DRAIN_MAX_SAMPLE_GAP_MS ||
      newer.timestamp - sample.timestamp > AC_DRAIN_TREND_WINDOW_MS
    )) break;
    run.push(sample);
  }
  run.reverse();
  if (run.length < AC_DRAIN_MIN_SAMPLES) return null;

  const first = run[0];
  const last = run[run.length - 1];
  const spanMs = last.timestamp - first.timestamp;
  const dropPercent = first.percent - last.percent;
  if (spanMs < AC_DRAIN_MIN_COVERAGE_MS || dropPercent < AC_DRAIN_MIN_DROP_PERCENT) return null;

  // Require decline in both halves so a single late gauge step cannot look sustained.
  const midpointTimestamp = first.timestamp + spanMs / 2;
  let midpointIndex = 1;
  while (midpointIndex < run.length - 1 && run[midpointIndex].timestamp < midpointTimestamp) midpointIndex += 1;
  const firstHalfDrop = first.percent - run[midpointIndex].percent;
  const secondHalfDrop = run[midpointIndex].percent - last.percent;
  if (firstHalfDrop < AC_DRAIN_MIN_HALF_DROP_PERCENT || secondHalfDrop < AC_DRAIN_MIN_HALF_DROP_PERCENT) return null;

  // Theil–Sen-style median pairwise slope resists isolated percentage jumps.
  const slopes = [];
  for (let start = 0; start < run.length - 1; start += 1) {
    for (let end = start + 1; end < run.length; end += 1) {
      const elapsedHours = (run[end].timestamp - run[start].timestamp) / (60 * 60 * 1000);
      if (elapsedHours > 0) slopes.push((run[end].percent - run[start].percent) / elapsedHours);
    }
  }
  const medianSlope = medianValue(slopes);
  const percentPerHour = medianSlope == null ? null : -medianSlope;
  if (percentPerHour == null || !Number.isFinite(percentPerHour) || percentPerHour <= 0) return null;
  return {
    dropPercent,
    percentPerHour,
    sampleCount: run.length,
    spanMinutes: spanMs / 60_000,
  };
}
