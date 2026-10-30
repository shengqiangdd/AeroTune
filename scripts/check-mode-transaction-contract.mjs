import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const tweaks = readFileSync("src-tauri/src/tweaks.rs", "utf8");
const journal = readFileSync("src-tauri/src/journal.rs", "utf8");
const power = readFileSync("src-tauri/src/power.rs", "utf8");
const auto = readFileSync("src-tauri/src/auto.rs", "utf8");
const applyCore = tweaks.slice(tweaks.indexOf("pub fn apply_mode_core("), tweaks.indexOf("#[tauri::command]", tweaks.indexOf("pub fn apply_mode_core(")));
const modeSteps = tweaks.slice(tweaks.indexOf("fn apply_mode_steps("), tweaks.indexOf("fn run_mode_group<"));
const groupRunner = tweaks.slice(tweaks.indexOf("fn run_mode_group<"), tweaks.indexOf("/// 场景模式编排核心"));
const eppWriter = journal.slice(journal.indexOf("fn write_ahead_epp_change_with_journal("), journal.indexOf("pub(crate) fn set_epp_with_journal("));

assert.match(groupRunner, /match apply\(\)[\s\S]*Err\(apply_error\)[\s\S]*rollback\(\)[\s\S]*已自动回滚[\s\S]*自动回滚未能完全确认[\s\S]*恢复记录已保留/);
assert.match(applyCore, /validate_mode_access\(mode, crate::util::is_admin\(\)\)\?[\s\S]*begin_change_group[\s\S]*run_mode_group\([\s\S]*apply_mode_steps\(mode, &mut applied\)[\s\S]*drop\(group_guard\.take\(\)\)[\s\S]*revert_change_group\(group_id\)[\s\S]*persist_mode_with_recovery\([\s\S]*write_mode\(mode\)[\s\S]*MANUAL_HOLD\.lock\(\)\.unwrap\(\) = Some\(Instant::now\(\)\)/);
assert.match(tweaks, /fn write_mode_with\([\s\S]*persist\(mode\.as_bytes\(\)\)[\s\S]*模式状态保存失败/);
assert.match(tweaks, /pub\(crate\) fn write_mode\(mode: &str\) -> Result<\(\), String>[\s\S]*atomic_write\(&mode_path\(\), bytes\)/);
assert.match(tweaks, /fn persist_mode_with_recovery\([\s\S]*Err\(persist_error\)[\s\S]*rollback\(group_id\)[\s\S]*恢复记录已保留/);
assert.match(auto, /match crate::tweaks::write_mode\(target\)[\s\S]*Err\(error\)[\s\S]*actions\.push\(format!\("\{action\}；场景状态标记未保存：\{error\}"\)\)/);
assert.match(auto, /crate::tweaks::write_mode\("none"\)\.map_err\([\s\S]*baseline_restore_error/);
assert.match(modeSteps, /"game" =>[\s\S]*set_plan_with_journal[\s\S]*set_gamedvr\(false\)\?[\s\S]*try_epp\(0/);
assert.match(modeSteps, /"power" =>[\s\S]*set_plan_with_journal[\s\S]*set_gamedvr\(false\)\?[\s\S]*try_epp\(85/);
assert.match(modeSteps, /"dev" =>[\s\S]*set_plan_with_journal[\s\S]*set_priority_boost\(true\)\?[\s\S]*try_epp\(25/);

assert.ok(eppWriter.indexOf("journal.push(") < eppWriter.indexOf("match apply()"), "EPP undo intent must be durable before powercfg writes");
assert.match(eppWriter, /read_current\(\)[\s\S]*apply\(\)[\s\S]*read_current\(\)[\s\S]*恢复记录已保留/);
assert.match(tweaks, /fn try_epp[\s\S]*epp_supported\(\)[\s\S]*get_epp_value\(\)[\s\S]*set_epp_with_journal/);
assert.match(power, /pub fn set_epp\(v: u32\)[\s\S]*epp_supported\(\)[\s\S]*get_epp_value\(\)[\s\S]*set_epp_with_journal[\s\S]*set_epp_raw\(Some\(v\)\)/);
assert.match(power, /EppWriteOutcome::AppliedWithWarning\(warning\)[\s\S]*return Err\(format!\("\{warning\}；恢复记录已保留/);
assert.match(journal, /crate::power::set_epp_raw\(Some\(old\)\)[\s\S]*crate::power::get_epp_value\(\) != Some\(old\)[\s\S]*保留恢复记录/);
assert.match(power, /run_checked\([\s\S]*"\/setacvalueindex"[\s\S]*run_checked\("powercfg", &\["\/setactive", "scheme_current"\]\)/);

for (const testName of [
  "scenario_group_rolls_back_every_prior_step_failure",
  "scenario_group_preserves_failure_and_recovery_record_when_compensation_conflicts",
  "mode_status_write_failure_is_returned_with_context",
  "mode_marker_failure_rolls_back_the_committed_scenario_group",
  "mode_marker_failure_keeps_unresolved_group_recovery_visible",
  "epp_write_ahead_retains_intent_when_command_errors_after_target_was_written",
  "epp_write_ahead_removes_intent_when_failure_is_verified_unchanged",
  "epp_write_ahead_preserves_record_when_post_write_state_is_unreadable",
  "epp_write_ahead_refuses_changed_preflight_value_without_mutating",
]) {
  assert.match(tweaks + journal, new RegExp(`fn ${testName}\\(`), `missing regression ${testName}`);
}

console.log("PASS atomic mode labels surface write failures, compensate manual groups, and preserve unresolved records/EPP intents");
