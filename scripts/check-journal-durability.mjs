import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const journal = readFileSync("src-tauri/src/journal.rs", "utf8");
const util = readFileSync("src-tauri/src/util.rs", "utf8");
const tweaks = readFileSync("src-tauri/src/tweaks.rs", "utf8");
const auto = readFileSync("src-tauri/src/auto.rs", "utf8");
const services = readFileSync("src-tauri/src/services.rs", "utf8");
const network = readFileSync("src-tauri/src/network.rs", "utf8");
const power = readFileSync("src-tauri/src/power.rs", "utf8");
const settings = readFileSync("src/pages/Settings.tsx", "utf8");
const readiness = readFileSync("docs/RELEASE_READINESS.md", "utf8");

assert.match(util, /SHGetKnownFolderPath\([\s\S]*?FOLDERID_RoamingAppData/);
assert.match(util, /if !path\.is_absolute\(\)[\s\S]*?return Err/);
assert.doesNotMatch(util, /std::env::var\("APPDATA"\)/, "profile data paths must not trust inherited APPDATA");
assert.match(util, /pub fn program_files_directories\([\s\S]*FOLDERID_ProgramFiles[\s\S]*FOLDERID_ProgramFilesX86/);
assert.match(services, /fn service_image_is_modifiable[\s\S]*protected_root_matches[\s\S]*path_is_within\(&root, &executable\)/);
assert.match(services, /service_image_scope_fails_closed_and_requires_a_protected_install_root/);
assert.match(network, /fn build_nic_guid_validation_script[\s\S]*?expected_guid[\s\S]*?Get-NetAdapter/);
assert.match(network, /pub\(crate\) fn nic_guid_is_uniquely_enumerated[\s\S]*?build_nic_guid_validation_script[\s\S]*?run_ps/);
assert.match(readiness, /persists the adapter GUID[\s\S]*?exactly one enumerated adapter/);
const tcpSetter = network.match(/fn set_tcp_autotuning_with\([\s\S]*?\n}\n/)?.[0];
assert.ok(tcpSetter, "TCP writes must use the testable checked transition");
assert.ok(
  tcpSetter.indexOf("normalize_tcp_autotuning_level(&current)") < tcpSetter.indexOf("apply(&requested)"),
  "TCP old state must be restorable before mutation"
);
assert.match(network, /tcp_autotuning_rejects_unrestorable_old_state_before_mutation/);
assert.match(network, /tcp_autotuning_records_a_normalized_restorable_snapshot/);
assert.match(journal, /normalize_tcp_autotuning_level\(level\)/);

const push = journal.match(/fn push\(&mut self, desc: &str, revert: Revert\)[\s\S]*?\n    fn remove/)?.[0];
assert.ok(push, "Journal::push must return an explicit persistence result");
assert.match(push, /self\.save\(\)[\s\S]*?self\.changes\.pop\(\)[\s\S]*?self\.next_id = id[\s\S]*?return Err\(error\)/);

assert.match(journal, /pub fn record\(desc: &str, revert: Revert\) -> Result<\(\), String>/);
assert.match(journal, /fn record_save_failure\([\s\S]*?rollback\(revert\)[\s\S]*?自动还原失败/);
assert.match(journal, /Revert::None[\s\S]*?操作可能已完成，但变更日志无法持久化/);

const writeAhead = journal.match(/fn write_ahead_registry_change_with_journal\([\s\S]*?\n}\n/)?.[0];
assert.ok(writeAhead);
assert.ok(writeAhead.indexOf("journal.push(desc, revert)") < writeAhead.indexOf("match apply()"));
assert.match(writeAhead, /current == old[\s\S]*?journal\.remove\(id\)/);
const powerPlanWriteAhead = journal.match(/fn write_ahead_power_plan_change_with_journal\([\s\S]*?\n}\n/)?.[0];
assert.ok(powerPlanWriteAhead, "manual power-plan changes must use the checked write-ahead path");
assert.ok(powerPlanWriteAhead.indexOf("journal.push(") < powerPlanWriteAhead.indexOf("match apply()"));
assert.match(powerPlanWriteAhead, /read_current\(\)[\s\S]*?old_guid/);
assert.match(powerPlanWriteAhead, /applied_guid[\s\S]*?撤销记录已保留/);
const powerPlanTransaction = journal.match(/pub fn change_power_plan_with_journal\([\s\S]*?\n}\n/)?.[0];
assert.ok(powerPlanTransaction);
assert.ok(powerPlanTransaction.indexOf("JOURNAL.lock()") < powerPlanTransaction.indexOf("if old_guid.eq_ignore_ascii_case(&applied_guid)"), "same-plan no-ops must be serialized too");
assert.match(powerPlanTransaction, /if old_guid\.eq_ignore_ascii_case\(&applied_guid\)[\s\S]*?active_scheme_guid\(\)/);
const powerPlanSetter = power.match(/pub fn set_power_plan\(guid: String\)[\s\S]*?\n}/)?.[0];
assert.ok(powerPlanSetter);
assert.match(powerPlanSetter, /active_scheme_guid\(\)[\s\S]*?list_power_plans\(\)[\s\S]*?change_power_plan_with_journal[\s\S]*?set_plan_raw_if_current/);
assert.match(power, /fn verify_active_scheme_guid\([\s\S]*?eq_ignore_ascii_case/);
assert.match(power, /pub fn set_plan_raw_if_current\([\s\S]*?active_scheme_guid\(\)[\s\S]*?run_checked[\s\S]*?verify_active_scheme_guid/);
assert.doesNotMatch(power, /pub fn set_plan_raw\(/, "unguarded plan setter must not remain available");
assert.match(auto, /change_power_plan_with_journal/);
assert.match(tweaks, /change_power_plan_with_journal/);
assert.match(auto, /restore_plan_exact\([\s\S]*?change_power_plan_with_journal[\s\S]*?set_plan_raw_if_current/);
assert.match(auto, /fn restore_plan_exact\([\s\S]*?write\(guid, current\)/);
assert.match(auto, /set_plan_raw_if_current/);
assert.match(tweaks, /set_plan_raw_if_current/);
assert.match(journal, /fn power_plan_restore_needed[\s\S]*?eq_ignore_ascii_case\(old_guid\)[\s\S]*?eq_ignore_ascii_case\(applied_guid\)/);
assert.match(journal, /fn legacy_power_plan_undo_is_safe[\s\S]*?eq_ignore_ascii_case\(old_guid\)/);
assert.match(journal, /Revert::PowerPlan \{ guid, applied_guid \}[\s\S]*?power_plan_restore_needed[\s\S]*?set_plan_raw_if_current[\s\S]*?legacy_power_plan_undo_is_safe/);
assert.match(power, /fn active_scheme_verification_is_case_insensitive_and_fail_closed/);
for (const name of ["power_plan_undo_treats_old_state_as_noop_and_rejects_third_party_state", "legacy_power_plan_undo_without_applied_guid_only_accepts_already_restored_state", "power_plan_write_ahead_intent_is_durable_before_switch_and_verifies_target", "failed_power_plan_switch_cleans_unchanged_intent_but_keeps_ambiguous_target", "power_plan_write_ahead_refuses_switch_after_concurrent_plan_change"]) {
  assert.match(journal, new RegExp(name));
}
for (const name of ["set_reg_dword", "set_reg_sz", "del_reg_value"]) {
  const setter = journal.match(new RegExp(`pub fn ${name}\\([\\s\\S]*?\\n}\\n`))?.[0];
  assert.ok(setter, `${name} must be present`);
  assert.match(setter, /write_ahead_registry_change\(/);
}
assert.match(journal, /Some\(_\) if current == old => Ok\(false\)/);

assert.match(journal, /failed_journal_save_does_not_publish_an_in_memory_record/);
assert.match(journal, /registry_write_ahead_record_is_durable_before_mutation_and_removed_if_unapplied/);
assert.match(journal, /registry_write_failure_with_applied_value_keeps_recovery_record/);
assert.match(journal, /registry_write_ahead_refuses_mutation_if_intent_cannot_be_saved/);
assert.match(journal, /fn load_from\(path: PathBuf\)/);
assert.match(journal, /MAX_JOURNAL_BYTES/);
assert.match(journal, /pub fn list_changes\(\) -> Result<Vec<Change>, String>/);
assert.match(journal, /fn refresh_if_repaired\(&mut self\)/);
assert.match(journal, /pub fn list_changes\(\)[\s\S]*?journal\.refresh_if_repaired\(\)/);
assert.match(journal, /retry_reloads_only_a_valid_restored_journal_file/);
assert.match(journal, /retry_does_not_treat_a_missing_corrupt_file_as_repaired/);
assert.match(journal, /corrupt_journal_is_not_treated_as_empty_or_overwritten/);
assert.match(journal, /journal_path_with_unreadable_parent_fails_closed/);
assert.match(journal, /oversized_journal_is_bounded_and_preserved/);
assert.match(journal, /journal_does_not_save_an_entry_that_would_exceed_the_load_limit/);
assert.match(journal, /const MAX_SAFE_JS_ID: u64 = 9_007_199_254_740_991/);
assert.match(journal, /pub fn begin_change_group\(label: &str\) -> Result<GroupGuard, String>[\s\S]*?MAX_SAFE_JS_ID/);
assert.match(journal, /fn validate_change_identities\([\s\S]*?ids\.insert\(change\.id\)[\s\S]*?group_id/);
assert.match(journal, /duplicate_ids_make_existing_journal_unhealthy_and_preserve_original_bytes/);
assert.match(journal, /change_identities_reject_js_unsafe_record_and_group_ids/);
assert.match(journal, /group_lookup_cannot_escape_selected_group_on_duplicate_ids/);
assert.match(journal, /fn find_unique_change_index\([\s\S]*?matches\.next\(\)\.is_some\(\)/);
assert.match(journal, /fn find_group_change_index\([\s\S]*?change\.group_id == Some\(group_id\)/);
assert.match(journal, /let idx = find_unique_change_index\(&j\.changes, id\)\?/);
assert.match(journal, /find_group_change_index\(&journal\.changes, group_id, id\)/);
assert.match(settings, /变更日志不可用，无法确认是否为空/);
assert.match(settings, /恢复操作已禁用/);
assert.match(settings, /disabled=\{changesStatus !== "ready"\}/);
assert.match(services, /service_startup_pair_rolls_back_first_write_when_second_fails/);
assert.match(tweaks, /crate::journal::set_epp_with_journal\([\s\S]*?crate::power::set_epp_raw\(Some\(v\)\)/);
assert.match(journal, /fn write_ahead_epp_change_with_journal\([\s\S]*?journal\.push\([\s\S]*?match apply\(\)/);
assert.match(auto, /变更日志写入失败；已触发恢复策略/);
assert.match(readiness, /Undo durability[\s\S]*write-ahead snapshots/);
assert.match(journal, /fn allowed_hklm_registry_revert[\s\S]*snapshot_dword_state[\s\S]*service_restore_target_allowed/);
assert.match(journal, /if !allowed_hklm_registry_revert\(path, value, &old, Some\(&applied\)\)/, "unapproved machine registry writes must fail before mutation");
assert.match(journal, /if !allowed_hklm_registry_revert\(key_path, value, old, applied\)/, "unapproved machine registry undo must fail closed");
assert.match(journal, /machine_registry_undo_rejects_forged_paths_and_invalid_snapshot_types/);
assert.match(journal, /machine_registry_undo_accepts_only_aero_tune_targets_and_ranges/);
assert.match(journal, /state_matches\(old, &\[0, 24\], true\)[\s\S]*?state_matches\(applied, &\[24\], true\)/);
assert.match(journal, /fn validate_nic_registry_identity_with[\s\S]*?normalize_adapter_guid[\s\S]*?guid_is_uniquely_enumerated/);
assert.match(journal, /fn validate_open_nic_registry_key[\s\S]*?get_value\("NetCfgInstanceId"\)[\s\S]*?validate_nic_registry_identity_with/);
assert.match(journal, /fn validate_nic_registry_target[\s\S]*?validate_open_nic_registry_key/);
const existingOnlyWrite = journal.match(/fn open_key_existing_write\([\s\S]*?\n\}/)?.[0];
assert.ok(existingOnlyWrite);
assert.match(existingOnlyWrite, /open_subkey_with_flags/);
assert.doesNotMatch(existingOnlyWrite, /create_subkey/);
assert.match(journal, /pub fn set_nic_reg_dword[\s\S]*?open_key_existing_write[\s\S]*?validate_open_nic_registry_key/);
assert.match(journal, /pub fn del_nic_reg_value[\s\S]*?open_key_existing_write[\s\S]*?validate_open_nic_registry_key/);
assert.match(journal, /let is_nic_target[\s\S]*?open_key_existing_write[\s\S]*?validate_open_nic_registry_key/);
const existingServiceWrite = journal.match(/fn open_registry_key_for_write\([\s\S]*?\n\}/)?.[0];
assert.ok(existingServiceWrite);
assert.match(existingServiceWrite, /machine_service_registry_key_name\([\s\S]*?service_restore_target_allowed[\s\S]*?open_key_existing_write/);
assert.match(existingServiceWrite, /DiagTrack/);
assert.match(journal, /pub fn set_reg_dword[\s\S]*?open_registry_key_for_write/);
assert.match(journal, /pub fn del_reg_value[\s\S]*?open_registry_key_for_write/);
assert.match(journal, /fn revert_registry[\s\S]*?open_registry_key_for_write/);
assert.match(journal, /service_registry_write_targets_are_classified_for_existing_only_access/);
assert.match(journal, /nic_undo_requires_an_existing_key_and_live_adapter_binding/);
assert.match(journal, /fn split_hive\([\s\S]*?eq_ignore_ascii_case/);
assert.match(journal, /registry_hive_parser_handles_unicode_case_expansion_without_panicking/);
assert.match(settings, /summarizeUndoTargets\(items, 20\)[\s\S]*window\.confirm/);
assert.match(readiness, /Elevated undo trust \(release gate; Windows VM acceptance outstanding\)[\s\S]*compiled AeroTune target policy[\s\S]*does not authenticate snapshot origin[\s\S]*disposable Windows VM/);

console.log("PASS journal persistence, registry writes, plan-transition verification, legacy fail-closed undo, and failure recovery are guarded");
