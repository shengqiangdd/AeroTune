import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { effectivePowerModeContext, effectivePowerModePresentation } from "../src/lib/effectivePowerMode.js";

const expected = {
  battery_saver: ["电池节能", "amber"],
  better_battery: ["偏向续航", "green"],
  balanced: ["平衡", "cyan"],
  high_performance: ["高性能", "rose"],
  max_performance: ["最佳性能", "rose"],
  game_mode: ["游戏模式", "violet"],
  mixed_reality: ["混合现实", "violet"],
};
for (const [mode, [label, tone]] of Object.entries(expected)) {
  const presentation = effectivePowerModePresentation(mode);
  assert.equal(presentation.label, label, `${mode} has a clear Chinese label`);
  assert.equal(presentation.tone, tone, `${mode} has stable visual semantics`);
}
assert.equal(effectivePowerModePresentation(null).label, "未知 / 暂不可用");
assert.equal(effectivePowerModePresentation("future_mode").label, "未知 / 暂不可用");
assert.equal(effectivePowerModePresentation("__proto__").label, "未知 / 暂不可用");
assert.equal(effectivePowerModePresentation(null, true).label, "读取中…");
const staleMode = effectivePowerModePresentation("balanced", false, true);
assert.equal(staleMode.label, "上次观察：平衡");
assert.match(staleMode.description, /不能视作当前模式/);
assert.equal(effectivePowerModePresentation(null, false, true).label, "上次状态未知");
assert.match(effectivePowerModePresentation(null).description, /不会据此推断当前模式/);
assert.match(effectivePowerModeContext("battery_saver", false), /AeroTune 省电预设已关闭/);
assert.equal(effectivePowerModeContext("battery_saver", true), null);
assert.match(effectivePowerModeContext("high_performance", true), /Windows 报告的是高性能/);
assert.equal(effectivePowerModeContext("high_performance", false), null);
assert.equal(effectivePowerModeContext("balanced", true), null);
assert.equal(effectivePowerModeContext("battery_saver", null), null);

const stats = readFileSync("src-tauri/src/stats.rs", "utf8");
const api = readFileSync("src/lib/api.ts", "utf8");
const powerPage = readFileSync("src/pages/PowerSaver.tsx", "utf8");
const dashboard = readFileSync("src/pages/Dashboard.tsx", "utf8");
const app = readFileSync("src/App.tsx", "utf8");
assert.match(stats, /PowerRegisterForEffectivePowerModeNotifications/);
assert.match(stats, /EFFECTIVE_POWER_MODE_V2, EFFECTIVE_POWER_MODE_V1/);
assert.match(stats, /effective_power_mode: current_effective_power_mode\(\)/);
assert.match(api, /effective_power_mode: EffectivePowerMode \| null/);
assert.match(powerPage, /effectivePowerModePresentation\([\s\S]*?statsStatus === "stale" \|\| statsStatus === "error"/);
assert.match(powerPage, /api\.onTrayModeApplied\(\(\) => \{/);
assert.match(powerPage, /if \(!disposed\) void loadMode\(\)/);
assert.match(powerPage, /unlisten\?\.\(\)/);
assert.match(powerPage, /const requestId = \+\+modeLoadSequence\.current/);
const toggleModeSource = powerPage.slice(powerPage.indexOf("const toggleMode"), powerPage.indexOf("const pick", powerPage.indexOf("const toggleMode")));
assert.match(toggleModeSource, /modeLoadSequence\.current \+= 1;[\s\S]*?await api\.applyMode[\s\S]*?modeLoadSequence\.current \+= 1;[\s\S]*?setMode\(v\)/, "manual mode writes invalidate reads both before and after the write");
assert.match(toggleModeSource, /catch \(e\) \{[\s\S]*?modeLoadSequence\.current \+= 1;[\s\S]*?void loadMode\(\)/, "failed manual writes refresh the actual mode instead of leaving a raced snapshot");
assert.match(powerPage, /statsStatus === "live"[\s\S]*?effectivePowerModeContext\(/, "stale snapshots are excluded from current pairing hints");
assert.match(powerPage, /effectivePowerModeHint &&/);
assert.match(powerPage, /Windows 有效能效态/);
assert.match(powerPage, /不是 CPU 频率或性能实测/);
assert.match(powerPage, /与 AeroTune 开关\/电源计划不是同一状态/);
assert.match(powerPage, /此卡只读，不会触发设置变更/);
assert.match(dashboard, /effectivePowerModePresentation\(stats\?\.effective_power_mode, statsStatus === "loading", statsStatus === "stale" \|\| statsStatus === "error"\)/);
assert.match(app, /Dashboard stats=\{stats\} statsStatus=\{status\}/);
assert.match(app, /PowerSaver stats=\{stats\} statsStatus=\{status\}/);
assert.match(dashboard, /Windows 能效态/);
assert.match(dashboard, /系统级策略信号，不代表 CPU 频率或性能实测/);
assert.match(dashboard, /flex flex-wrap items-center justify-between gap-2/);

console.log("Effective power mode contract passed: native mapping, unknown fallback, dashboard visibility, and read-only copy");
