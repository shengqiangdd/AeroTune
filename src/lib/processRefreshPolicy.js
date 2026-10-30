import { validCapacityPair } from "./resourceEvidence.js";

const MEMORY_INTERVALS_MS = {
  high: 15_000,
  elevated: 8_000,
  normal: 5_000,
  unknown: 0,
};

const CPU_INTERVALS_MS = {
  high: 15_000,
  elevated: 8_000,
  normal: 5_000,
  unknown: 10_000,
};

const INTERACTION_LATENCY_INTERVALS_MS = {
  high: 15_000,
  elevated: 8_000,
  normal: 0,
  unknown: 0,
};

const CPU_ELEVATED_ENTER_PCT = 60;
const CPU_ELEVATED_EXIT_PCT = 56;
const CPU_HIGH_ENTER_PCT = 80;
const CPU_HIGH_EXIT_PCT = 76;
const MEMORY_ELEVATED_ENTER_PCT = 85;
const MEMORY_ELEVATED_EXIT_PCT = 83;
const MEMORY_HIGH_ENTER_PCT = 90;
const MEMORY_HIGH_EXIT_PCT = 88;
const LATENCY_ELEVATED_ENTER_MS = 50;
const LATENCY_ELEVATED_EXIT_MS = 40;
const LATENCY_HIGH_ENTER_MS = 100;
const LATENCY_HIGH_EXIT_MS = 80;
const LATENCY_MIN_SAMPLES = 20;

function classifyPressureBand(value, previousBand, thresholds, maximum = 100) {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0 || value > maximum) {
    return "unknown";
  }

  if (value >= thresholds.highEnter || (previousBand === "high" && value >= thresholds.highExit)) {
    return "high";
  }
  if (value >= thresholds.elevatedEnter || (previousBand === "elevated" && value >= thresholds.elevatedExit)) {
    return "elevated";
  }
  return "normal";
}

export function cpuLoadBandFromUsage(cpuPct, previousBand = "unknown") {
  return classifyPressureBand(cpuPct, previousBand, {
    elevatedEnter: CPU_ELEVATED_ENTER_PCT,
    elevatedExit: CPU_ELEVATED_EXIT_PCT,
    highEnter: CPU_HIGH_ENTER_PCT,
    highExit: CPU_HIGH_EXIT_PCT,
  });
}

export function memoryLoadBandFromUsage(usedGb, totalGb, previousBand = "unknown") {
  if (!validCapacityPair(usedGb, totalGb)) return "unknown";
  const usedPct = (usedGb / totalGb) * 100;
  return classifyPressureBand(usedPct, previousBand, {
    elevatedEnter: MEMORY_ELEVATED_ENTER_PCT,
    elevatedExit: MEMORY_ELEVATED_EXIT_PCT,
    highEnter: MEMORY_HIGH_ENTER_PCT,
    highExit: MEMORY_HIGH_EXIT_PCT,
  });
}

export function interactionLatencyBandFromP95(p95Ms, sampleCount, previousBand = "unknown") {
  if (!Number.isInteger(sampleCount) || sampleCount < LATENCY_MIN_SAMPLES) return "unknown";
  return classifyPressureBand(p95Ms, previousBand, {
    elevatedEnter: LATENCY_ELEVATED_ENTER_MS,
    elevatedExit: LATENCY_ELEVATED_EXIT_MS,
    highEnter: LATENCY_HIGH_ENTER_MS,
    highExit: LATENCY_HIGH_EXIT_MS,
  }, Number.POSITIVE_INFINITY);
}

export function processRefreshIntervalMs(cpuLoadBand, memoryLoadBand, interactionLatencyBand = "unknown") {
  const cpuIntervalMs = CPU_INTERVALS_MS[cpuLoadBand] ?? CPU_INTERVALS_MS.unknown;
  const memoryIntervalMs = MEMORY_INTERVALS_MS[memoryLoadBand] ?? 0;
  const interactionLatencyIntervalMs = INTERACTION_LATENCY_INTERVALS_MS[interactionLatencyBand] ?? 0;
  return Math.max(cpuIntervalMs, memoryIntervalMs, interactionLatencyIntervalMs);
}
