import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  cpuForecastGateMessage,
  cpuForecastGateState,
  cpuForecastPollDelayMs,
  cpuForecastTriggerPct,
  cpuForecastWarmupProgress,
} from "../src/lib/cpuForecastGate.js";

const autoSource = readFileSync(new URL("../src-tauri/src/auto.rs", import.meta.url), "utf8");

const base = {
  ready: true,
  adaptive: true,
  holdSeconds: 0,
  idleSeconds: 30,
  awayThreshold: 300,
  stability: 80,
  slopePctPerSec: 1.2,
  forecastPct: 75,
  triggerPct: 65,
};
const classify = (overrides = {}) => cpuForecastGateState({ ...base, ...overrides });

assert.equal(cpuForecastWarmupProgress(0, 0), 0);
assert.equal(cpuForecastWarmupProgress(5, 10), 50, "sample count is the limiting warm-up gate");
assert.equal(cpuForecastWarmupProgress(15, 2.5), 50, "observed span is the limiting warm-up gate");
assert.equal(cpuForecastWarmupProgress(10, 5), 100);
assert.equal(cpuForecastWarmupProgress(15, 8), 100, "progress clamps after both thresholds");
assert.equal(cpuForecastWarmupProgress(Number.NaN, 5), 0);
assert.equal(cpuForecastWarmupProgress(10, Number.NaN), 0);
assert.equal(cpuForecastWarmupProgress(-4, -2), 0);
assert.equal(cpuForecastPollDelayMs(false), 3000);
assert.equal(cpuForecastPollDelayMs(false, false, 1), 6000);
assert.equal(cpuForecastPollDelayMs(false, false, 2), 12000);
assert.equal(cpuForecastPollDelayMs(false, false, 4), 30000, "warm-up retries cap at 30 seconds");
assert.equal(cpuForecastPollDelayMs(true), 15000);
assert.equal(cpuForecastPollDelayMs(true, false, 1), 30000, "ready-state retry backoff respects the cap");
assert.equal(cpuForecastPollDelayMs(false, true), 15000);
assert.equal(cpuForecastPollDelayMs(false, false, -2), 3000);
assert.equal(cpuForecastPollDelayMs(false, false, Number.NaN), 3000);

assert.equal(cpuForecastTriggerPct(30), 35);
assert.equal(cpuForecastTriggerPct(60), 65);
assert.equal(cpuForecastTriggerPct(95), 100);
assert.equal(cpuForecastTriggerPct(10), 35);
assert.equal(cpuForecastTriggerPct(120), 100);
assert.equal(cpuForecastTriggerPct(Number.NaN), 65);
assert.equal(classify({ ready: false, adaptive: false }), "warming");
assert.equal(classify({ adaptive: false, forecastPct: 100 }), "monitor");
assert.equal(cpuForecastGateMessage("monitor", 65), "自动适配已暂停；CPU 预测仅用于观察，不会触发提前响应。");
assert.equal(classify({ holdSeconds: 1, forecastPct: 100 }), "held");
assert.equal(cpuForecastGateMessage("held", 65), "手动模式保持中；预测继续显示，自动策略切换等待保持结束。");
assert.equal(classify({ idleSeconds: 299 }), "above");
assert.equal(classify({ idleSeconds: 300 }), "idle");
assert.equal(cpuForecastGateMessage("idle", 65), "已达到离开判定阈值；CPU 预测不参与提前响应。");
assert.equal(classify({ idleSeconds: Number.NaN }), "invalid");
assert.equal(classify({ awayThreshold: Number.NaN }), "invalid");
assert.equal(cpuForecastGateMessage("invalid", 65), "CPU 趋势或活动状态数据无效，等待下一次有效采样。");
assert.equal(classify({ stability: 59.99 }), "unstable");
assert.equal(classify({ stability: 60 }), "above");
assert.equal(classify({ slopePctPerSec: 0.749 }), "slope");
assert.equal(classify({ slopePctPerSec: 0.75 }), "above");
assert.equal(classify({ forecastPct: 64.99 }), "below");
assert.equal(classify({ forecastPct: 65 }), "above");
assert.equal(classify({ triggerPct: 100, forecastPct: 99.99 }), "below");
assert.equal(classify({ triggerPct: 100, forecastPct: 100 }), "above");
assert.equal(classify({ stability: Number.NaN }), "invalid");
assert.equal(classify({ forecastPct: Number.NaN }), "invalid");
assert.equal(classify({ slopePctPerSec: Number.NaN }), "invalid");
assert.equal(classify({ triggerPct: Number.NaN }), "invalid");

assert.match(autoSource, /fn adaptive_knob_recheck_precedes_calibration_and_respects_residence/);
assert.match(autoSource, /fn adaptive_knobs_due\([\s\S]*?priority_gate\.ready_for_reevaluation\(now\)/);
assert.match(autoSource, /if target == ad_state[\s\S]*?adaptive_knobs_due\([\s\S]*?priority_separation_gate[\s\S]*?if knobs_due/, "stable-scene pending priority changes must be reevaluated at their gate deadline");

console.log("CPU forecast gate passed: confidence, adaptive gates, timer cadence, and threshold boundaries");
