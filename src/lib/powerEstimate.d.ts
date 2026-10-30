export interface BatteryPowerSample {
  timestamp: number;
  acConnected: boolean | null;
  percent: number;
}

export interface BatteryDischargeEstimate {
  hours: number;
  rangeHours: [number, number];
  percentPerHour: number;
  quality: "低" | "中" | "高";
  sampleCount: number;
  spanMinutes: number;
}
export interface AcDrainEstimate {
  dropPercent: number;
  percentPerHour: number;
  sampleCount: number;
  spanMinutes: number;
}

export function batteryAcState(battery: { ac_connected?: boolean | null } | null | undefined): boolean | null;
export function batteryQueryFailed(stats: { battery_status_ok?: boolean } | null | undefined): boolean;
export const BATTERY_SAMPLE_INTERVAL_MS: number;
export const BATTERY_MAX_SAMPLE_GAP_MS: number;
export const AC_DRAIN_MAX_SAMPLE_GAP_MS: number;
export const AC_DRAIN_MIN_COVERAGE_MS: number;
export const AC_DRAIN_MIN_DROP_PERCENT: number;
export const AC_DRAIN_MIN_SAMPLES: number;
export const AC_DRAIN_TREND_WINDOW_MS: number;
export function estimateAcDrain(samples: readonly BatteryPowerSample[]): AcDrainEstimate | null;
export function estimateDischarge(
  samples: BatteryPowerSample[],
  currentPercent: number | null,
): BatteryDischargeEstimate | null;
