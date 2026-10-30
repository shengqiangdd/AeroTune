// 全自动模式的源码合同：钉住"一键整栈启用、备份还原、自动采纳有审计、
// 破坏性动作永远手动"四条红线。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (file) =>
  readFileSync(new URL(`../${file}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");

const apiSource = read("src/lib/api.ts");
const autoEngineSource = read("src/pages/AutoEngine.tsx");
const autoSource = read("src-tauri/src/auto.rs");
const libSource = read("src-tauri/src/lib.rs");

// ---- IPC 契约 ----
assert.match(apiSource, /setAutopilot: \(enabled: boolean\) => invoke<AutoSettings>\("set_autopilot", \{ enabled \}\)/);
assert.match(apiSource, /autopilot: boolean;/);
assert.match(apiSource, /autopilot_backup: \{[\s\S]*?auto_dispose_anomalies: boolean;\s*\n\s*\} \| null;/);
assert.match(libSource, /auto::set_autopilot,/);

// ---- 备份还原语义 ----
assert.match(autoSource, /pub struct AutopilotBackup \{/);
assert.match(
  autoSource,
  /settings\.autopilot_backup = Some\(AutopilotBackup \{[\s\S]*?auto_dispose_anomalies: settings\.auto_dispose_anomalies,\s*\n\s*\}\);/,
  "enable snapshot includes all seven related switches",
);
assert.match(
  autoSource,
  /if let Some\(backup\) = settings\.autopilot_backup\.take\(\) \{[\s\S]*?settings\.scheduler_shadow = backup\.scheduler_shadow;/,
  "disable restores from the snapshot",
);
assert.match(
  autoSource,
  /\} else \{\s*\n\s*settings\.adaptive = false;\s*\n\s*settings\.auto_dispose_anomalies = false;\s*\n\s*\}\s*\n\s*Ok\("已关闭全自动模式/,
  "missing snapshot falls back to disabling adaptive and dispose",
);
assert.match(autoSource, /return Err\("全自动模式已开启"\.into\(\)\)/, "double-enable is rejected");
assert.match(autoSource, /return Err\("全自动模式未开启"\.into\(\)\)/, "double-disable is rejected");
assert.match(
  autoSource,
  /fn seed_adaptive_baseline\(st: &mut AutoState, reason: &str\)/,
  "adaptive baseline seeding is shared between manual and autopilot paths",
);

// ---- 场景自动采纳：门槛复用、限量、审计 ----
assert.match(autoSource, /const AUTOPILOT_ADOPT_PER_TICK: usize = 2;/, "auto-adoption is rate limited per tick");
assert.match(
  autoSource,
  /state\.settings\.autopilot && state\.settings\.auto_game/,
  "adoption only runs in full autopilot with game recognition on",
);
assert.match(
  autoSource,
  /crate::scene_shadow::snapshot_scene\(&state\.settings\.game_list\)/,
  "auto-adoption reuses the suggestion threshold gate",
);
assert.match(
  autoSource,
  /crate::journal::record\(\s*\n\s*&format!\("全自动模式自动采纳场景建议：/,
  "every auto-adoption is recorded in the change journal",
);
assert.match(
  autoSource,
  /fn autopilot_adopt_scene\(settings: &mut AutoSettings, name: &str\) -> Option<String>/,
  "adoption is a pure function",
);
assert.match(
  autoSource,
  /if !settings\.autopilot \|\| !settings\.auto_game \{\s*\n\s*return None;/,
  "adoption refuses to run outside autopilot",
);

// ---- 红线：破坏性动作永远手动（模块注释与 UI 文案双重钉住） ----
assert.match(
  autoSource,
  /删文件、结束进程、修改网络与服务永远保持手动/,
  "backend charter keeps destructive actions manual",
);
assert.match(
  autoEngineSource,
  /不会自动删除文件、结束进程或修改 DNS \/ 网络 \/ 服务/,
  "autopilot card discloses the destructive-action boundary",
);
assert.match(
  autoEngineSource,
  /开启前自动备份相关开关，关闭时按快照还原并恢复系统基线；每次自动采纳进变更日志/,
  "autopilot card discloses backup/restore and audit trail",
);
assert.match(autoEngineSource, /api\.setAutopilot\(enabled\)/);
assert.match(autoEngineSource, /全自动模式已开启；开启前设置已备份/);

// ---- 异常自动降级子开关：门槛、保护、审计 ----
assert.match(apiSource, /auto_dispose_anomalies: boolean;/);
assert.match(
  autoSource,
  /pub auto_dispose_anomalies: bool,\s*\n\s*\/\/\/ 开启全自动模式前的开关快照/,
  "dispose flag lives beside autopilot in settings",
);
assert.match(
  autoSource,
  /auto_dispose_anomalies: settings\.auto_dispose_anomalies,\s*\n\s*\}\);/,
  "enable snapshot includes the dispose flag",
);
assert.match(
  autoSource,
  /settings\.auto_dispose_anomalies = backup\.auto_dispose_anomalies;/,
  "disable restores the dispose flag",
);
assert.match(
  autoSource,
  /settings\.auto_dispose_anomalies = true;/,
  "enabling autopilot turns dispose on as part of the stack",
);
assert.match(
  autoSource,
  /state\.settings\.autopilot && state\.settings\.auto_dispose_anomalies/,
  "rules engine receives the combined escalation gate",
);
assert.match(
  autoSource,
  /if auto_dispose && !foreground \{/,
  "escalation excludes the foreground process",
);
assert.match(
  autoSource,
  /全自动模式自动处置：\{\} 优先级降至低于正常/,
  "every auto-disposal is recorded in the change journal",
);
assert.match(
  autoSource,
  /（前台进程，仅提醒）/,
  "foreground processes stay notify-only with explicit wording",
);
assert.match(
  autoSource,
  /const AUTOPILOT_ADOPT_PER_TICK: usize = 2;/,
  "existing adoption cap remains pinned",
);
assert.match(autoEngineSource, /异常自动降级/);
assert.match(
  autoEngineSource,
  /系统关键进程与前台进程永远只提醒，进程暂停暂时停用/,
  "sub-toggle copy discloses protections and suspend boundary",
);
assert.match(
  autoEngineSource,
  /savePreference\("auto_dispose_anomalies", value/,
  "sub-toggle reuses the standard settings write path",
);

console.log("autopilot contract passed: 34 checks");
