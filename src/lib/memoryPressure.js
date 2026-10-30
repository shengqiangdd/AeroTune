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

/** Summarize memory pressure without interpreting missing telemetry as a flat trend. */
export function memoryPressureStatus(diag) {
  if (!diag || diag.score === 0) {
    return memoryStatus("正在建立基线", "muted");
  }

  const percent = diag.mem_pct;
  if (!Number.isFinite(percent) || percent < 0 || percent > 100) {
    return memoryStatus("内存水位数据不可用", "muted");
  }
  if (percent >= MEMORY_CRITICAL_PCT) {
    return memoryStatus("压力偏高，建议查看进程", "warning", percent, true);
  }
  if (percent >= MEMORY_PRESSURE_ELEVATED_PCT) {
    if (diag.mem_slope_ready !== true) {
      const rawSampleCount = diag.mem_slope_sample_count;
      const sampleCount = Number.isFinite(rawSampleCount)
        ? Math.max(0, Math.min(MEMORY_SLOPE_WINDOW, Math.trunc(rawSampleCount)))
        : 0;
      const sampleStatus = sampleCount >= MEMORY_SLOPE_MIN_SAMPLES
        ? `内存趋势就绪状态待同步（${sampleCount} 个有效实测读数）`
        : `内存趋势实测读数积累中（${sampleCount}/${MEMORY_SLOPE_MIN_SAMPLES}）`;
      return memoryStatus(
        sampleStatus,
        "warning",
        percent,
        true,
      );
    }
    if (!Number.isFinite(diag.mem_slope)) {
      return memoryStatus("内存趋势数据不可用", "muted", percent, true);
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
      return memoryStatus("持续上升，正在关注", "warning", percent, true, etaSeconds);
    }
    return memoryStatus("偏高但趋势平稳", "warning", percent, true);
  }
  return memoryStatus("余量充足", "good", percent, true);
}
