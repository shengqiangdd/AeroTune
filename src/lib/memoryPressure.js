export const MEMORY_SLOPE_MIN_SAMPLES = 10;
export const MEMORY_SLOPE_WINDOW = 45;
export const MEMORY_RISING_SLOPE_THRESHOLD = 0.05;
export const MEMORY_FORECAST_MIN_SAMPLES = 20;
export const MEMORY_PRESSURE_ELEVATED_PCT = 78;
export const MEMORY_CRITICAL_PCT = 90;
export const MEMORY_FORECAST_MAX_SECONDS = 180;

const memoryStatus = (label, tone, percent = Number.NaN, percentValid = false, etaSeconds = null) => ({
  label,
  tone,
  percent,
  percentValid,
  etaSeconds,
});

/** Summarize RAM occupancy and trend without treating missing telemetry as flat. */
export function memoryPressureStatus(diag) {
  if (!diag || diag.score === 0) {
    return memoryStatus("正在建立基线", "muted");
  }

  const percent = diag.mem_pct;
  if (!Number.isFinite(percent) || percent < 0 || percent > 100) {
    return memoryStatus("RAM 占用率数据不可用", "muted");
  }
  if (percent >= MEMORY_CRITICAL_PCT) {
    return memoryStatus("RAM 占用率高；核对可用量与提交量", "warning", percent, true);
  }
  if (percent >= MEMORY_PRESSURE_ELEVATED_PCT) {
    if (diag.mem_slope_ready !== true) {
      const rawSampleCount = diag.mem_slope_sample_count;
      const sampleCount = Number.isFinite(rawSampleCount)
        ? Math.max(0, Math.min(MEMORY_SLOPE_WINDOW, Math.trunc(rawSampleCount)))
        : 0;
      const sampleStatus = sampleCount >= MEMORY_SLOPE_MIN_SAMPLES
        ? `RAM 占用率偏高；趋势状态待同步（${sampleCount} 个有效实测读数）`
        : `RAM 占用率偏高；趋势采样中（${sampleCount}/${MEMORY_SLOPE_MIN_SAMPLES}）`;
      return memoryStatus(
        sampleStatus,
        "warning",
        percent,
        true,
      );
    }
    if (!Number.isFinite(diag.mem_slope)) {
      return memoryStatus("RAM 占用趋势数据不可用", "muted", percent, true);
    }
    if (diag.mem_slope > MEMORY_RISING_SLOPE_THRESHOLD) {
      const rawEtaSeconds = diag.mem_critical_eta_secs;
      const rawEtaSampleCount = diag.mem_slope_sample_count;
      const etaSeconds = Number.isInteger(rawEtaSeconds)
        && rawEtaSeconds >= 1
        && rawEtaSeconds <= MEMORY_FORECAST_MAX_SECONDS
        && Number.isInteger(rawEtaSampleCount)
        && rawEtaSampleCount >= MEMORY_FORECAST_MIN_SAMPLES
        && rawEtaSampleCount <= MEMORY_SLOPE_WINDOW
        ? rawEtaSeconds
        : null;
      return memoryStatus("RAM 占用率持续上升；核对可用量", "warning", percent, true, etaSeconds);
    }
    return memoryStatus("RAM 占用率偏高但趋势平稳", "warning", percent, true);
  }
  return memoryStatus("RAM 占用率低于预警线", "good", percent, true);
}
