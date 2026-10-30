const MIN_STABILITY = 60;
const MIN_RISING_SLOPE = 0.75;
export const CPU_FORECAST_WARMUP_SAMPLE_TARGET = 10;
export const CPU_FORECAST_WARMUP_SPAN_SECS = 5;
const FORECAST_WARMUP_POLL_MS = 3000;
const FORECAST_IDLE_POLL_MS = 15000;
const FORECAST_MAX_POLL_MS = 30000;

/** Returns the slower of the independent-sample and observed-span warm-up gates. */
export function cpuForecastWarmupProgress(sampleCount, observedSpanSecs) {
  const samples = Number.isFinite(sampleCount) ? Math.max(0, sampleCount) : 0;
  const spanSecs = Number.isFinite(observedSpanSecs) ? Math.max(0, observedSpanSecs) : 0;
  const sampleProgress = samples / CPU_FORECAST_WARMUP_SAMPLE_TARGET;
  const spanProgress = spanSecs / CPU_FORECAST_WARMUP_SPAN_SECS;
  return Math.round(Math.min(1, sampleProgress, spanProgress) * 100);
}

/** Polls quickly while warming, then adds bounded backoff after transient failures. */
export function cpuForecastPollDelayMs(ready, hidden = false, failureCount = 0) {
  const baseDelay = ready || hidden ? FORECAST_IDLE_POLL_MS : FORECAST_WARMUP_POLL_MS;
  const failures = Number.isFinite(failureCount) ? Math.max(0, Math.floor(failureCount)) : 0;
  const multiplier = 2 ** Math.min(4, failures);
  return Math.min(FORECAST_MAX_POLL_MS, baseDelay * multiplier);
}

/** Mirrors the backend's configured heavy-load threshold plus its forecast margin. */
export function cpuForecastTriggerPct(heavyThreshold) {
  const configuredThreshold = Number.isFinite(heavyThreshold) ? heavyThreshold : 60;
  return Math.min(100, Math.max(30, configuredThreshold) + 5);
}

/** Classifies whether a ten-second CPU estimate can inform adaptive response. */
export function cpuForecastGateState({
  ready,
  adaptive,
  holdSeconds,
  idleSeconds,
  awayThreshold,
  stability,
  slopePctPerSec,
  forecastPct,
  triggerPct,
}) {
  if (!ready) return "warming";
  if (!adaptive) return "monitor";
  if (Number.isFinite(holdSeconds) && holdSeconds > 0) return "held";
  if (!Number.isFinite(idleSeconds) || !Number.isFinite(awayThreshold)) return "invalid";
  if (idleSeconds >= awayThreshold) return "idle";
  if (!Number.isFinite(triggerPct) || !Number.isFinite(stability)
    || !Number.isFinite(forecastPct) || !Number.isFinite(slopePctPerSec)) return "invalid";
  if (stability < MIN_STABILITY) return "unstable";
  if (slopePctPerSec < MIN_RISING_SLOPE) return "slope";
  return forecastPct >= triggerPct ? "above" : "below";
}

export function cpuForecastGateMessage(state, triggerPct) {
  switch (state) {
    case "warming": return "正在积累独立 CPU 采样。";
    case "monitor": return "自动适配已暂停；CPU 预测仅用于观察，不会触发提前响应。";
    case "held": return "手动模式保持中；预测继续显示，自动策略切换等待保持结束。";
    case "idle": return "已达到离开判定阈值；CPU 预测不参与提前响应。";
    case "invalid": return "CPU 趋势或活动状态数据无效，等待下一次有效采样。";
    case "unstable": return "CPU 预测稳定度未达 60，暂不参与提前响应。";
    case "slope": return "CPU 上升斜率未达每秒 0.75 个百分点，暂不提前响应。";
    case "above": return `CPU 预测满足 ${triggerPct}% 负载线、趋势和稳定度条件；完整场景策略仍会复核。`;
    case "below": return `CPU 预测尚未达到 ${triggerPct}% 负载线。`;
  }
}
