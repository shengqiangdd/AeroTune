export const MAX_INTERACTION_LATENCY_SAMPLES: 31;
export const INTERACTION_LATENCY_P95_UPDATE_INTERVAL: 5;

export interface InteractionLatencySampleUpdate {
  totalSampleCount: number;
  rollingSampleCount: number;
  shouldUpdateP95: boolean;
}

export function recordInteractionLatencySample(
  samples: number[],
  value: number,
  totalSampleCount: number,
  maxSamples?: number,
  updateEvery?: number,
): InteractionLatencySampleUpdate | null;
