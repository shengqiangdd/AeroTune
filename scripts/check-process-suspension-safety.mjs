import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const processesRust = read("src-tauri/src/processes.rs");
const autoRust = read("src-tauri/src/auto.rs");
const adminRust = read("src-tauri/src/admin.rs");
const processesUi = read("src/pages/Processes.tsx");
const autoUi = read("src/pages/AutoEngine.tsx");
const api = read("src/lib/api.ts");
const releaseReadiness = read("docs/RELEASE_READINESS.md");

assert.match(processesRust, /PROCESS_SUSPEND_DISABLED_ERROR[\s\S]*恢复入口仍可使用/);
assert.match(processesRust, /pub fn suspend_process\([\s\S]*?deny_process_suspension\(\)/);
assert.doesNotMatch(processesRust, /NtSuspendProcess|suspend_pid_raw|nt_process_call/);
assert.match(processesRust, /pub fn resume_pid_raw\([\s\S]*?let symbol = b"NtResumeProcess\\0"/);
assert.match(
  processesRust,
  /pub fn resume_pid_raw\(pid: u32, expected_creation_filetime: u64\)[\s\S]*?let actual_creation_filetime = process_times\(handle\)[\s\S]*?process_creation_identity_matches\(expected_creation_filetime, actual_creation_filetime\)[\s\S]*?let status = func\(handle\)/,
  "resume must verify the expected process instance on the same still-open handle",
);
assert.match(
  processesRust,
  /fn process_creation_token\([\s\S]*?expected_start_unix: u64[\s\S]*?expected_name: &str[\s\S]*?expected_path: &str[\s\S]*?\)[\s\S]*?PROCESS_QUERY_LIMITED_INFORMATION[\s\S]*?query_process_image\(handle\)[\s\S]*?process_times\(handle\)[\s\S]*?process_row_matches_current_identity/,
  "row identity must match its path and sampled creation second before exposing resume",
);
assert.match(
  processesRust,
  /fn filetime_matches_unix_start[\s\S]*?ticks \/ FILETIME_TICKS_PER_SECOND == start_unix/,
  "sysinfo start-time correlation is explicitly second-resolution",
);
assert.match(
  processesRust,
  /pub fn resume_process\([\s\S]*?creation_token: String[\s\S]*?parse_creation_token\(&creation_token\)[\s\S]*?resume_pid_raw\(pid, expected_creation_filetime\)/,
);
assert.match(processesRust, /fn process_creation_identity_matches[\s\S]*?expected != 0 && actual == Some\(expected\)/);
assert.match(processesRust, /new_process_suspension_fails_closed_with_recovery_guidance/);
assert.match(
  releaseReadiness,
  /On steady scans, the list retains an action token only when exact FILETIME from the previous row snapshot, a current pre-refresh query, and the post-refresh row token all agree[\s\S]*?warmup reseeds sysinfo's CPU\/I\/O baseline after its pre-token capture[\s\S]*?previous\/pre\/post exact-identity check narrows but does not make row sampling atomic[\s\S]*?automatic FILETIME is still queried adjacent to \(not atomically with\) sysinfo metrics[\s\S]*?any observed coarse\/exact identity transition now discards that scan's process metrics/i,
  "manual row-token bracketing and automatic transition-sample mitigation must remain visible in release notes",
);

assert.match(autoRust, /normalize_disabled_suspend_actions[\s\S]*?rule\.action == "suspend"[\s\S]*?rule\.action = "notify"/);
assert.match(autoRust, /SETTINGS_SUSPEND_MIGRATION_WARNING[\s\S]*?自动挂起规则已转为仅提醒/);
assert.match(autoRust, /fn validate_rule_action[\s\S]*?"suspend" => Err\("进程暂停暂时关闭/);
assert.match(autoRust, /"suspend" => \{[\s\S]*?suspend_notice_seen\.insert\(a\.pid\)[\s\S]*?进程暂停暂时关闭，已按仅提醒处理/);
assert.doesNotMatch(autoRust, /crate::processes::suspend_pid_raw\(/);
assert.match(autoRust, /disabled_suspend_rule_only_notifies_and_deduplicates/);

assert.match(adminRust, /if r\.action == "suspend"[\s\S]*?自动挂起规则已停用/);
assert.match(adminRust, /legacy_suspend\.rules\[0\]\.action = "suspend"[\s\S]*?validate_import\(legacy_suspend\)/);

assert.match(processesUi, /<button\s+type="button"\s+disabled\s+aria-label=\{`挂起进程暂不可用/);
assert.doesNotMatch(processesUi, /api\.suspendProcess\(/);
assert.match(processesUi, /onClick=\{\(\) => resume\(p\)\}/);
assert.match(processesUi, /确定尝试恢复[\s\S]*如果该进程由其他程序暂停/);
assert.doesNotMatch(autoUi, /option value="suspend"/);
assert.match(autoUi, /进程暂停暂时(?:关闭|停用)[\s\S]*旧挂起规则(?:仅提醒|按仅提醒处理)/);
assert.match(api, /suspendProcess:\s*\(pid: number\) => invoke<void>\("suspend_process"/);

console.log("PASS suspension fails closed; legacy rules downgrade; resume requires verified process identity");
