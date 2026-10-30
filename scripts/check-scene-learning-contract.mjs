// 场景识别影子观测 + 事件关联时间线的源码合同：钉住"纯观察、用户确认才生效、
// 非因果声明"三条红线，以及门槛规则、持久化边界与前端防抖。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (file) =>
  readFileSync(new URL(`../${file}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");

const apiSource = read("src/lib/api.ts");
const dashboardSource = read("src/pages/Dashboard.tsx");
const autoEngineSource = read("src/pages/AutoEngine.tsx");
const autoSource = read("src-tauri/src/auto.rs");
const shadowSource = read("src-tauri/src/scene_shadow.rs");
const rootcauseSource = read("src-tauri/src/rootcause.rs");
const libSource = read("src-tauri/src/lib.rs");

// ---- IPC 契约 ----
assert.match(apiSource, /getSceneSuggestions: \(\) => invoke<SceneSuggestion\[\]>\("get_scene_suggestions"\)/);
assert.match(apiSource, /dismissSceneSuggestion: \(name: string\) => invoke<boolean>\("dismiss_scene_suggestion", \{ name \}\)/);
assert.match(libSource, /scene_shadow::get_scene_suggestions,/);
assert.match(libSource, /scene_shadow::dismiss_scene_suggestion,/);
assert.match(apiSource, /interface TimelineEvent \{/);
assert.match(apiSource, /timeline: TimelineEvent\[\];/);

// ---- 影子观测红线：纯观察、会话去重、门槛 ----
assert.match(shadowSource, /纯观察——不加名单任何行为不变/, "shadow store keeps its observation-only charter");
assert.match(shadowSource, /pub const SUGGESTION_MIN_TOTAL: u32 = 3;/);
assert.match(shadowSource, /pub const SUGGESTION_MIN_DAYS: usize = 2;/);
assert.match(shadowSource, /const STORE_MAX_ENTRIES: usize = 64;/, "shadow store entry count is bounded");
assert.match(shadowSource, /const STALE_ENTRY_SECS: u64 = 30 \* 24 \* 3600;/, "stale entries expire");
assert.match(
  shadowSource,
  /!game_list[\s\S]*?eq_ignore_ascii_case\(name\)/,
  "already-listed processes never appear as suggestions",
);
assert.match(
  shadowSource,
  /crate::util::atomic_write\(&self\.path, &bytes\)/,
  "shadow store persists through the shared atomic write helper",
);
assert.match(
  autoSource,
  /full_ready && !fg_norm\.is_empty\(\) && !state\.settings\.game_list\.contains\(&fg_norm\)/,
  "observation hook fires only for confirmed fullscreen sessions outside the game list",
);
assert.match(
  autoSource,
  /state\.scene_shadow_seen\.as_deref\(\) != Some\(&fg_norm\)/,
  "one observation per sustained fullscreen session per process",
);
assert.match(autoSource, /纯观察不改任何行为；每个持续全屏会话对同一进程只记一次/);

// ---- 时间线：与提示同窗、上限、起点标记 ----
assert.match(rootcauseSource, /const TIMELINE_MAX_EVENTS: usize = 12;/);
assert.match(
  rootcauseSource,
  /events\.sort_by_key\(\|event\| event\.at_unix\);/,
  "timeline events are chronologically sorted including the onset marker",
);
assert.match(rootcauseSource, /pub fn build_timeline\(/);
assert.match(dashboardSource, /相关事件时间线，非因果结论/, "timeline legend keeps the non-causal disclaimer");
assert.match(dashboardSource, /function AnomalyTimeline\(\{ context \}/);
assert.match(
  dashboardSource,
  /const others = events\.filter\(\(event\) => event\.kind !== "onset"\);\s*\n\s*if \(!onset \|\| others\.length === 0\) return null;/,
  "timeline renders only with an onset marker and at least one correlated event",
);

// ---- AutoEngine 建议卡：采纳走既有 add_game_item，忽略仅清观测 ----
assert.match(autoEngineSource, /场景识别建议 · 影子观测/);
assert.match(autoEngineSource, /await api\.addGameItem\(suggestion\.name\);/, "adoption reuses the existing game-list write path");
assert.match(autoEngineSource, /await api\.dismissSceneSuggestion\(suggestion\.name\);/);
assert.match(
  autoEngineSource,
  /是否采纳由你决定；忽略只清除这条观测记录，后续会话会重新累积/,
  "suggestion card discloses user-controlled adoption and dismiss semantics",
);
assert.match(
  autoEngineSource,
  /const loadSceneSuggestions = useCallback\(async \(\) => \{[\s\S]*?\} catch \{[\s\S]*?保持上次快照/,
  "suggestion loading fails silent and keeps the last snapshot",
);

console.log("scene-learning contract passed: 22 checks");
