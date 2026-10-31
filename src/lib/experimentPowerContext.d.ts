import type { EffectivePowerMode, Stats } from "./api";

export type PowerSource = "ac" | "battery" | "unknown";
export interface PowerContextSample {
  powerSource: PowerSource;
  effectivePowerMode: EffectivePowerMode | null;
}
export interface PowerModeCount {
  mode: EffectivePowerMode;
  count: number;
}
export interface PowerContextSummary {
  sampleCount: number;
  powerSourceCounts: Record<PowerSource, number>;
  effectivePowerModeCounts: PowerModeCount[];
  effectivePowerModeUnknownCount: number;
}

export const MIN_STABLE_POWER_CONTEXT_SAMPLES: number;
export function capturePowerContext(stats: Stats | null | undefined): PowerContextSample;
export function summarizePowerContext(samples: readonly (PowerContextSample | null | undefined)[]): PowerContextSummary;
export function isPowerContextSummary(value: unknown): value is PowerContextSummary;
export function stablePowerSource(summary: PowerContextSummary | null | undefined): PowerSource | null;
export function comparablePowerSource(
  baseline: PowerContextSummary | null | undefined,
  after: PowerContextSummary | null | undefined,
): boolean;
export function powerSourceChanged(
  baseline: PowerContextSummary | null | undefined,
  after: PowerContextSummary | null | undefined,
): boolean;
