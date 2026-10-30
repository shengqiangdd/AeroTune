export type CpuForecastGateState =
  | "warming"
  | "monitor"
  | "held"
  | "idle"
  | "invalid"
  | "unstable"
  | "slope"
  | "above"
  | "below";

export interface CpuForecastGateInput {
  ready: boolean;
  adaptive: boolean;
  holdSeconds: number;
  idleSeconds: number;
  awayThreshold: number;
  stability: number;
  slopePctPerSec: number;
  forecastPct: number;
  triggerPct: number;
}

export const CPU_FORECAST_WARMUP_SAMPLE_TARGET: number;
export const CPU_FORECAST_WARMUP_SPAN_SECS: number;
export function cpuForecastWarmupProgress(sampleCount: number, observedSpanSecs: number): number;
export function cpuForecastPollDelayMs(ready: boolean, hidden?: boolean, failureCount?: number): number;
export function cpuForecastTriggerPct(heavyThreshold: number | undefined): number;
export function cpuForecastGateState(input: CpuForecastGateInput): CpuForecastGateState;
export function cpuForecastGateMessage(state: CpuForecastGateState, triggerPct: number): string;
