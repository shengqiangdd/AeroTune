import type { Diagnostics } from "./api";

export const MEMORY_SLOPE_MIN_SAMPLES: number;
export const MEMORY_SLOPE_WINDOW: number;
export const MEMORY_RISING_SLOPE_THRESHOLD: number;
export const MEMORY_FORECAST_MIN_SAMPLES: number;
export const MEMORY_PRESSURE_ELEVATED_PCT: number;
export const MEMORY_CRITICAL_PCT: number;
export const MEMORY_FORECAST_MAX_SECONDS: number;

export type MemoryPressureTone = "muted" | "warning" | "good";

export interface MemoryPressureStatus {
  label: string;
  tone: MemoryPressureTone;
  percent: number;
  percentValid: boolean;
  etaSeconds: number | null;
}

export function memoryPressureStatus(
  diagnostics: Pick<Diagnostics, "score" | "mem_pct" | "mem_slope" | "mem_slope_ready" | "mem_slope_sample_count" | "mem_critical_eta_secs"> | null | undefined,
): MemoryPressureStatus;
