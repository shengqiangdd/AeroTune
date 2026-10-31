import { batteryAcState, batteryQueryFailed } from "./powerEstimate.js";

export const MIN_STABLE_POWER_CONTEXT_SAMPLES = 3;

const EFFECTIVE_POWER_MODES = [
  "battery_saver",
  "better_battery",
  "balanced",
  "high_performance",
  "max_performance",
  "game_mode",
  "mixed_reality",
];
const POWER_SOURCES = ["ac", "battery", "unknown"];
const isKnownPowerMode = (mode) => typeof mode === "string" && EFFECTIVE_POWER_MODES.includes(mode);
const isCount = (value) => Number.isInteger(value) && value >= 0;

export function capturePowerContext(stats) {
  const battery = stats?.battery_status_ok === true && !batteryQueryFailed(stats) ? stats?.battery ?? null : null;
  const acConnected = batteryAcState(battery);
  const effectivePowerMode = stats?.effective_power_mode;
  return {
    powerSource: acConnected === true ? "ac" : acConnected === false ? "battery" : "unknown",
    effectivePowerMode: isKnownPowerMode(effectivePowerMode) ? effectivePowerMode : null,
  };
}

export function summarizePowerContext(samples) {
  const powerSourceCounts = { ac: 0, battery: 0, unknown: 0 };
  const modeCounts = new Map();
  let effectivePowerModeUnknownCount = 0;
  const sampleList = Array.isArray(samples) ? samples : [];

  for (const sample of sampleList) {
    const source = POWER_SOURCES.includes(sample?.powerSource) ? sample.powerSource : "unknown";
    powerSourceCounts[source] += 1;
    if (isKnownPowerMode(sample?.effectivePowerMode)) {
      modeCounts.set(sample.effectivePowerMode, (modeCounts.get(sample.effectivePowerMode) ?? 0) + 1);
    } else {
      effectivePowerModeUnknownCount += 1;
    }
  }

  return {
    sampleCount: sampleList.length,
    powerSourceCounts,
    effectivePowerModeCounts: EFFECTIVE_POWER_MODES
      .filter((mode) => modeCounts.has(mode))
      .map((mode) => ({ mode, count: modeCounts.get(mode) })),
    effectivePowerModeUnknownCount,
  };
}

export function isPowerContextSummary(value) {
  if (!value || typeof value !== "object") return false;
  const summary = value;
  if (!isCount(summary.sampleCount) || !isCount(summary.effectivePowerModeUnknownCount)) return false;
  const sources = summary.powerSourceCounts;
  if (!sources || !POWER_SOURCES.every((source) => isCount(sources[source]))) return false;
  if (POWER_SOURCES.reduce((sum, source) => sum + sources[source], 0) !== summary.sampleCount) return false;
  if (!Array.isArray(summary.effectivePowerModeCounts)) return false;

  const seenModes = new Set();
  let knownModeCount = 0;
  for (const item of summary.effectivePowerModeCounts) {
    if (!item || typeof item !== "object" || !isKnownPowerMode(item.mode) || !Number.isInteger(item.count) || item.count <= 0) {
      return false;
    }
    if (seenModes.has(item.mode)) return false;
    seenModes.add(item.mode);
    knownModeCount += item.count;
  }
  return knownModeCount + summary.effectivePowerModeUnknownCount === summary.sampleCount;
}

export function stablePowerSource(summary) {
  if (!isPowerContextSummary(summary) || summary.sampleCount < MIN_STABLE_POWER_CONTEXT_SAMPLES) return null;
  const counts = summary.powerSourceCounts;
  if (counts.unknown !== 0) return null;
  if (counts.ac === summary.sampleCount) return "ac";
  if (counts.battery === summary.sampleCount) return "battery";
  return null;
}

// Cross-experiment analysis requires a known, stable and matching supply in both windows.
export function comparablePowerSource(baseline, after) {
  const beforeSource = stablePowerSource(baseline);
  return beforeSource !== null && beforeSource === stablePowerSource(after);
}

export function powerSourceChanged(baseline, after) {
  const beforeSource = stablePowerSource(baseline);
  const afterSource = stablePowerSource(after);
  return beforeSource !== null && afterSource !== null && beforeSource !== afterSource;
}
