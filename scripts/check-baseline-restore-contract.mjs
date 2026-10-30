import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const auto = readFileSync("src-tauri/src/auto.rs", "utf8");
const brightness = readFileSync("src-tauri/src/brightness.rs", "utf8");
const power = readFileSync("src-tauri/src/power.rs", "utf8");
const admin = readFileSync("src-tauri/src/admin.rs", "utf8");
const api = readFileSync("src/lib/api.ts", "utf8");
const engine = readFileSync("src/pages/AutoEngine.tsx", "utf8");
const settings = readFileSync("src/pages/Settings.tsx", "utf8");

const restoreStart = auto.indexOf("pub fn restore_baseline(");
const restoreEnd = auto.indexOf("pub(crate) fn sync_after_settings_import", restoreStart);
const restore = auto.slice(restoreStart, restoreEnd);
const setAutoStart = auto.indexOf("pub fn set_auto_setting(");
const setAutoEnd = auto.indexOf("/// 自适应引擎开启时的统一初始化", setAutoStart);
const setAuto = auto.slice(setAutoStart, setAutoEnd);
const setAutopilotStart = auto.indexOf("pub fn set_autopilot(");
const setAutopilotEnd = auto.indexOf("#[tauri::command]", setAutopilotStart);
const setAutopilot = auto.slice(setAutopilotStart, setAutopilotEnd);
const importStart = admin.indexOf("pub async fn import_config");
const importEnd = admin.indexOf("#[cfg(test)]", importStart);
const importConfig = admin.slice(importStart, importEnd);
const exitRestoreStart = brightness.indexOf("pub fn restore_on_exit(");
const exitRestoreEnd = brightness.indexOf("#[cfg(test)]", exitRestoreStart);
const exitRestore = brightness.slice(exitRestoreStart, exitRestoreEnd);
const brightnessExecutorStart = brightness.indexOf("fn execute_brightness_task(");
const brightnessExecutorEnd = brightness.indexOf("fn run_worker_loop(", brightnessExecutorStart);
const brightnessExecutor = brightness.slice(brightnessExecutorStart, brightnessExecutorEnd);

assert.match(restore, /pub fn restore_baseline\(&mut self\) -> Result<\(\), String>/);
assert.match(restore, /restore_scheduler_processes\(self\)[\s\S]*self\.restore_foreground_priority\(\)/);
assert.match(restore, /update_brightness\(&mut self\.adaptive, false\);[\s\S]*self\.adaptive\.brightness\.recovery_issue[\s\S]*self\.adaptive\.brightness\.has_saved\(\)[\s\S]*屏幕亮度尚未确认恢复/);
assert.match(restore, /finish_baseline_restore\(&mut self\.adaptive\.baseline_restore_pending, result\)/);
assert.match(restore, /if let Err\(error\) = restore_plan_exact\([\s\S]*return Err\(baseline_restore_error\(&failures\)\)/);
assert.match(restore, /baseline_restore_mismatches\(&baseline, &actual\)[\s\S]*if !failures\.is_empty\(\)[\s\S]*return Err/);
assert.match(auto, /fn finish_baseline_restore\([\s\S]*\*pending = result\.is_err\(\);[\s\S]*result/);
assert.match(auto, /pub baseline_restore_pending: bool/);
assert.match(auto, /baseline_restore_pending: st\.adaptive\.baseline_restore_pending/);
assert.match(auto, /pub brightness_restore_pending: bool/);
assert.match(auto, /pub brightness_restore_snapshot_error: bool/);
assert.match(auto, /pub brightness_preserved_value: Option<u8>/);
assert.match(auto, /brightness_restore_pending: brightness_restore_pending\([\s\S]*st\.adaptive\.brightness\.has_saved\(\)[\s\S]*recovery_issue/);
assert.match(auto, /brightness_preserved_value: st[\s\S]*preserved_notice_value\(Instant::now\(\)\)/);
assert.match(auto, /fn brightness_restore_pending\([\s\S]*snapshot_error[\s\S]*settings\.auto_dim[\s\S]*matches!\(adaptive_state, "power" \| "away"\)/);
assert.match(auto, /AutoState \{[\s\S]*schedule_startup_recovery\(&mut state\.adaptive\.brightness\)/);
assert.match(brightness, /pub restore_pending: bool/);
assert.match(brightness, /BrightnessOutcome::Dimmed \{ original \} =>[\s\S]*guarded_recovery = true/);
assert.match(brightness, /BrightnessOutcome::DimFailed \{ original \} =>[\s\S]*guarded_recovery = true/);
assert.match(power, /fn parse_brightness_level\([\s\S]*level <= 100/);
assert.match(power, /pub fn get_brightness\(\)[\s\S]*parse_brightness_level\(&out\)/);
assert.match(power, /fn brightness_input_accepts_only_the_wmi_percent_range/);
assert.match(power, /pub fn set_brightness_if_current\([\s\S]*build_brightness_compare_write_script[\s\S]*run_ps_with_timeout[\s\S]*parse_brightness_compare_write_result/);
assert.match(power, /fn guarded_brightness_write_checks_current_before_mutating/);
assert.match(brightness, /crate::power::set_brightness_if_current/);
assert.match(brightnessExecutor, /BrightnessTask::Restore\(value\) =>[\s\S]*execute_guarded_recovery\(snapshot_for_original\(value\)\)/);
assert.match(brightnessExecutor, /BrightnessTask::Recover\(snapshot\) => execute_guarded_recovery\(snapshot\)/);
assert.match(brightness, /impl From<Option<u8>> for BrightnessState[\s\S]*guarded_recovery: saved\.is_some\(\)/);
assert.match(brightness, /if self\.restore_pending[\s\S]*self\.restore_pending = true[\s\S]*Some\(BrightnessTask::Restore\(original\)\)/);
assert.match(brightness, /fn persist_snapshot\([\s\S]*atomic_write/);
assert.match(brightness, /pub fn schedule_startup_recovery\([\s\S]*guarded_recovery = true[\s\S]*BrightnessTask::Recover\(snapshot\)/);
assert.match(brightness, /BrightnessOutcome::GuardedRecoveryFailed =>[\s\S]*guarded_recovery = true[\s\S]*RETRY_BACKOFF/);
assert.match(brightness, /if self\.guarded_recovery[\s\S]*Some\(BrightnessTask::Recover\(snapshot_for_original/);
assert.match(brightness, /fn recover_snapshot_with\([\s\S]*current == snapshot\.dimmed[\s\S]*RecoverySuperseded/);
assert.match(exitRestore, /saved\.map\(snapshot_for_original\)[\s\S]*recover_snapshot_with/);
assert.ok(
  exitRestore.indexOf("shutdown_and_drain") < exitRestore.indexOf("load_snapshot"),
  "exit must drain queued brightness writes before reading and restoring the durable snapshot"
);
assert.doesNotMatch(exitRestore, /set_brightness\(value\)/);

assert.match(setAuto, /key == "retry_baseline_restore"[\s\S]*st\.restore_baseline\(\)/);
assert.match(setAuto, /key == "retry_brightness_restore"[\s\S]*brightness\.recovery_issue[\s\S]*schedule_startup_recovery[\s\S]*update_brightness\(&mut st\.adaptive, false\)[\s\S]*brightness\.restore_pending/);
assert.match(setAuto, /if key == "adaptive" && on[\s\S]*baseline_restore_pending/);
assert.match(setAuto, /st\.restore_baseline\(\)\?/);
assert.match(setAutopilot, /baseline_restore_pending[\s\S]*apply_autopilot\(s, enabled\)[\s\S]*st\.restore_baseline\(\)\?/);
assert.ok(importConfig.includes("if st.adaptive.baseline_restore_pending"));
assert.ok(importConfig.indexOf("st.restore_baseline()") < importConfig.indexOf("save_settings_pub(&parsed)"), "config import must restore successfully before committing the imported settings");
assert.match(importConfig, /AdaptiveImportTransition::Stop \| AdaptiveImportTransition::Keep/);

assert.match(api, /brightness_restore_snapshot_error: boolean/);
assert.match(api, /brightness_preserved_value: number \| null/);
assert.match(api, /retryBaselineRestore:[\s\S]*invoke<void>\("set_auto_setting", \{ key: "retry_baseline_restore", on: false \}\)/);
assert.match(api, /retryBrightnessRestore:[\s\S]*invoke<void>\("set_auto_setting", \{ key: "retry_brightness_restore", on: false \}\)/);
assert.match(engine, /const retryBaselineRestore = async \(\)[\s\S]*api\.retryBaselineRestore\(\)[\s\S]*await refresh\(\)/);
assert.match(engine, /const retryBrightnessRestore = async \(\)[\s\S]*api\.retryBrightnessRestore\(\)[\s\S]*await refresh\(\)/);
assert.match(engine, /state\?\.baseline_restore_pending && \([\s\S]*role="alert"[\s\S]*重试恢复基线/);
assert.match(engine, /state\?\.brightness_restore_pending && !state\.baseline_restore_pending[\s\S]*role="alert"[\s\S]*屏幕亮度恢复尚未确认[\s\S]*brightness_restore_snapshot_error[\s\S]*重试亮度恢复/);
assert.match(engine, /disabled=\{!setting \|\| busy \|\| state\?\.baseline_restore_pending === true\}/);
assert.match(engine, /disabled=\{!backendReady \|\| busy \|\| state\?\.baseline_restore_pending === true\}/);
assert.match(settings, /catch \(error\) \{[\s\S]*toastErr\(error\);[\s\S]*await loadSettings\(\)/);

for (const testName of [
  "injected_baseline_restore_failure_propagates_and_sets_retry_state",
  "baseline_restore_reports_unconfirmed_knobs_and_actionable_retry",
  "brightness_restore_pending_matches_requested_scene_and_setting",
  "exact_plan_restore_passes_target_and_expected_current_for_serialized_check",
]) {
  assert.match(auto, new RegExp(`fn ${testName}\\(`), `missing regression ${testName}`);
}
assert.match(brightness, /fn restore_is_not_queued_twice_while_worker_is_pending\(/);
for (const testName of [
  "snapshot_format_is_bounded_and_rejects_invalid_values",
  "dim_persists_snapshot_before_touching_display",
  "failed_dim_write_keeps_backup_and_uses_guarded_recovery",
  "failed_partial_dim_does_not_overwrite_a_later_manual_brightness",
  "startup_recovery_restores_only_the_expected_dimmed_value",
  "guarded_restore_preserves_a_change_since_the_initial_sample",
  "guarded_restore_accepts_original_value_seen_by_recheck",
  "recovery_matrix_restores_only_matching_dim_targets",
  "live_guarded_restore_preserves_manual_brightness_changes",
  "preserved_brightness_notice_expires_and_clears_before_a_new_dim",
  "exit_barrier_drains_inflight_dim_before_closing_worker",
  "startup_retries_remain_guarded_against_external_brightness_changes",
  "unreadable_snapshot_blocks_automatic_brightness_writes",
  "recovery_of_already_restored_value_only_clears_snapshot",
  "legacy_saved_backup_and_retry_stay_guarded",
  "recovery_superseded_clears_stale_in_memory_backup",
]) {
  assert.match(brightness, new RegExp(`fn ${testName}\\(`), `missing brightness recovery regression ${testName}`);
}

console.log("PASS adaptive baseline and durable brightness recovery retain visible, guarded retry state");
