import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const tweaks = readFileSync("src-tauri/src/tweaks.rs", "utf8");
const auto = readFileSync("src-tauri/src/auto.rs", "utf8");
const journal = readFileSync("src-tauri/src/journal.rs", "utf8");
const power = readFileSync("src-tauri/src/power.rs", "utf8");
const util = readFileSync("src-tauri/src/util.rs", "utf8");
const registry = readFileSync("src-tauri/src/lib.rs", "utf8");
const api = readFileSync("src/lib/api.ts", "utf8");
const gameMode = readFileSync("src/pages/GameMode.tsx", "utf8");
const previewCard = readFileSync("src/components/ScenarioPreviewCard.tsx", "utf8");

const inspectStart = tweaks.indexOf("fn inspect_game_mode_state()");
const inspectEnd = tweaks.indexOf("fn display_dword(", inspectStart);
assert.ok(inspectStart >= 0 && inspectEnd > inspectStart, "GameMode read-only inspection block must be present");
const inspection = tweaks.slice(inspectStart, inspectEnd);
assert.match(inspection, /list_power_plans\(\)/);
assert.match(inspection, /active_scheme_guid\(\)/);
assert.match(inspection, /plan_guids_for_preview\(\)/);
assert.match(inspection, /preview_existing_plan\(/);
assert.match(inspection, /read_reg_dword_detailed\(/);
assert.match(inspection, /inspect_epp_plan_setting\(/);
assert.doesNotMatch(inspection, /resolve_plan\(|duplicatescheme|write_mode|set_reg_dword|set_plan_raw|apply_mode_core|begin_change_group|rollback_change_group|auto_restore_point|MANUAL_HOLD/,
  "preflight inspection may read only; it must not apply, persist, or create recovery entries");

assert.match(util, /pub fn app_dir_path_read_only\(\) -> Result<PathBuf, String>[\s\S]*roaming_app_data_dir\(\)\?\.join\("AeroTune"\)/);
assert.match(auto, /pub\(crate\) fn plan_guids_for_preview\(\)[\s\S]*app_dir_path_read_only\(\)[\s\S]*std::fs::read/);
const pureResolverStart = auto.indexOf("pub(crate) fn preview_existing_plan(");
const pureResolverEnd = auto.indexOf("/// 解析目标计划", pureResolverStart);
assert.ok(pureResolverStart >= 0 && pureResolverEnd > pureResolverStart);
assert.doesNotMatch(auto.slice(pureResolverStart, pureResolverEnd), /update_settings|duplicatescheme|save_settings|resolve_plan\(/,
  "preview target resolution must not persist or duplicate a power plan");
assert.match(journal, /pub fn read_reg_dword_detailed\([\s\S]*KEY_READ[\s\S]*classify_dword_read/);
assert.match(power, /fn epp_plan_ac_setting_path\(guid: &str\)[\s\S]*PowerSchemes\\\{\}\\\{\}\\\{\}[\s\S]*guid, SUB_PROCESSOR, PERFEPP/);
assert.match(power, /inspect_epp_plan_setting\(guid: &str\)[\s\S]*ACSettingIndex/);

assert.match(tweaks, /const GAME_MODE_PREVIEW_TTL: Duration = Duration::from_secs\(120\)/);
assert.match(tweaks, /fn consume_at\([\s\S]*\.tokens[\s\S]*\.remove\(id\)/,
  "preview IDs must be consumed once");
assert.match(tweaks, /getrandom::fill\(&mut token_bytes\)/);
assert.match(tweaks, /static MODE_OPERATION_LOCK:[\s\S]*pub\(crate\) fn lock_mode_operations/);
assert.match(auto, /fn apply_knobs\([\s\S]*lock_mode_operations\(\)/);
assert.match(auto, /fn restore_baseline_inner\([\s\S]*lock_mode_operations\(\)/);
assert.match(auto, /fn apply_knobs\([\s\S]*lock_mode_operations\(\)[\s\S]*manual_hold_active\(\)/);
assert.match(auto, /apply_mode_core_automatic\("power"\)/);
assert.match(auto, /apply_mode_core_automatic\("game"\)/);
assert.match(tweaks, /pub\(crate\) fn apply_mode_core_automatic\([\s\S]*lock_mode_operations\(\)[\s\S]*manual_hold_active\(\)/);
assert.match(tweaks, /pub fn set_tweak\([\s\S]*auto\s*\.lock\(\)[\s\S]*lock_mode_operations\(\)/);
assert.match(power, /pub fn set_power_plan\(guid: String\)[\s\S]*lock_mode_operations\(\)/);
assert.match(power, /pub fn set_epp\(v: u32\)[\s\S]*lock_mode_operations\(\)/);
assert.match(journal, /pub fn revert_change\(expected: Change\)[\s\S]*lock_mode_operations\(\)[\s\S]*JOURNAL\.lock/);
assert.match(journal, /pub fn revert_change_group\([\s\S]*lock_mode_operations\(\)[\s\S]*revert_change_group_inner/);
assert.match(journal, /pub fn revert_all\([\s\S]*lock_mode_operations\(\)[\s\S]*JOURNAL\.lock/);
assert.match(journal, /pub fn clear_journal\([\s\S]*lock_mode_operations\(\)[\s\S]*JOURNAL\.lock/);
assert.match(journal, /pub fn set_reg_dword_if_current\([\s\S]*old != expected_old[\s\S]*write_ahead_registry_change_with_journal/);
assert.match(journal, /pub\(crate\) fn rollback_change_group\(group_id: u64\) -> Result<usize, String> \{\s*revert_change_group_inner\(group_id, None\)\s*\}/);
assert.match(tweaks, /fn set_gamedvr_if_current\([\s\S]*set_reg_dword_if_current/);
assert.match(tweaks, /fn try_epp_expected\([\s\S]*ensure_active_plan_matches[\s\S]*old != expected[\s\S]*Some\(target_plan_guid\)/);
assert.match(tweaks, /PreviewEppAction::Unknown => \{[\s\S]*ensure_active_plan_matches\(target_plan_guid\)[\s\S]*预检状态未知[\s\S]*已跳过/);
assert.match(tweaks, /fn apply_mode_steps\([\s\S]*active_plan_guid[\s\S]*set_plan_with_journal\(cur, guid\.clone\(\)/);
assert.match(tweaks, /let mut plan_may_remain = false[\s\S]*persist_mode_with_recovery[\s\S]*if plan_may_remain/);
assert.match(tweaks, /pub fn apply_mode_core\(mode: &str\)[\s\S]*if mode == "game"[\s\S]*inspect_game_mode_state\(\)[\s\S]*未发现现成高性能计划/);
assert.match(tweaks, /fn apply_mode_with_preview_core\([\s\S]*consume_game_mode_preview\(preview_id\)[\s\S]*inspect_game_mode_state\(\)[\s\S]*apply_if_preview_matches[\s\S]*apply_mode_core_with_preview_plan\(mode, Some\(plan_resolution\), true\)/,
  "apply must consume/recheck the snapshot and pass the confirmed target into the transaction");
assert.match(tweaks, /Some\(PreviewGamePlan::Existing \{ guid, name, \.\. \}\) => \{\s*\(guid\.clone\(\), name\.clone\(\), false\)\s*\}/);
assert.match(tweaks, /Some\(PreviewGamePlan::MissingConfirmed\s*\{[\s\S]*?resolve_game_plan_after_confirmed_missing\(\)\?/);
assert.match(tweaks, /require_preview_confirmation\(&expected, missing_plan_confirmed\)\?/);
assert.match(tweaks, /fn resolve_game_plan_after_confirmed_missing\(\)[\s\S]*invalidate_plan_cache\(\)[\s\S]*list_power_plans\(\)/);
assert.match(tweaks, /fn resolve_game_plan_automatic\(\)[\s\S]*plans_before[\s\S]*resolve_plan[\s\S]*可能已创建高性能计划/);
assert.match(tweaks, /pub async fn get_tweak_states\(preview_mode: Option<String>\)[\s\S]*spawn_blocking[\s\S]*preview_game_mode_core/);
assert.match(tweaks, /pub async fn apply_mode\([\s\S]*preview_id: Option<String>[\s\S]*preview_confirmed: Option<bool>[\s\S]*apply_mode_with_preview_core/);
assert.match(registry, /tweaks::get_tweak_states,[\s\S]*tweaks::apply_mode,/);
assert.doesNotMatch(registry, /tweaks::preview_mode|tweaks::apply_mode_with_preview/,
  "retain the reviewed Tauri command-registration checkpoint; reuse existing IPC commands");
assert.match(api, /getTweakStates: \(previewMode\?: "game"\)[\s\S]*"get_tweak_states", \{ previewMode: previewMode \?\? null \}/);
assert.match(api, /applyMode: \(mode: string, previewId\?: string, previewConfirmed\?: boolean\)[\s\S]*"apply_mode", \{[\s\S]*previewConfirmed: previewConfirmed \?\? null/);

assert.match(gameMode, /api\.getTweakStates\("game"\)/);
assert.match(gameMode, /modePreview\.requires_confirmation[\s\S]*window\.confirm/);
assert.match(gameMode, /api\.applyMode\("game", previewId!, previewConfirmed\)/);
assert.match(gameMode, /previewConfirmed = true/);
assert.match(gameMode, /disabled=\{modeBusy \|\| busy \|\| eppStatus !== "ready" \|\| epp == null\}/);
assert.match(gameMode, /statesStatus === "ready" && states\?\.gamedvr != null/);
assert.match(gameMode, /GameDVR 状态未知；控制已禁用。/);
assert.match(gameMode, /disabled=\{busy\}/);
assert.match(tweaks, /fn gamedvr_state_from_reads[\s\S]*game_config_store[\s\S]*\.ok\(\)\?[\s\S]*app_capture\.ok\(\)\?/);
assert.match(gameMode, /Date\.now\(\) >= modePreview\.expires_at_unix_ms/);
assert.doesNotMatch(gameMode, /api\.applyMode\(v \? "game"/,
  "GameMode enable must go through the preview-token command");
assert.match(previewCard, /本机状态预检（只读）[\s\S]*快照于[\s\S]*应用时会复核活动计划[\s\S]*不一致时拒绝继续/);
assert.match(previewCard, /当前：\{step\.current\} → 目标：\{step\.target\}/);
assert.match(previewCard, /此模式的本机当前值尚未由状态化预检读取/,
  "unmigrated modes must not imply their static cards are live state");

console.log("Mode preflight contract passed: read-only GameMode snapshot, explicit unknowns, one-use TTL token, stale-state rejection, and honest static-mode scope");
