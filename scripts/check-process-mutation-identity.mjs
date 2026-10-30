import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { relative, join } from "node:path";
import { fileURLToPath } from "node:url";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const rust = read("src-tauri/src/processes.rs");
const diag = read("src-tauri/src/diag.rs");
const stats = read("src-tauri/src/stats.rs");
const ui = read("src/pages/Processes.tsx");
const api = read("src/lib/api.ts");
const dashboard = read("src/pages/Dashboard.tsx");
const auto = read("src-tauri/src/auto.rs");
const autoUi = read("src/pages/AutoEngine.tsx");
const release = read("docs/RELEASE_READINESS.md");

function functionBody(source, signature) {
  const start = source.indexOf(signature);
  assert.notEqual(start, -1, `missing function: ${signature}`);
  const end = source.indexOf("\n}", start);
  assert.notEqual(end, -1, `unterminated function: ${signature}`);
  return source.slice(start, end + 2);
}

function frontendSources(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return frontendSources(path);
    return /\.[jt]sx?$/.test(entry.name) ? [path] : [];
  });
}

const frontendRoot = fileURLToPath(new URL("../src", import.meta.url));
const mutationInvocations = frontendSources(frontendRoot).flatMap((path) => {
  const source = readFileSync(path, "utf8");
  return [...source.matchAll(/invoke(?:<[^>]*>)?\(\s*["'](kill_process|set_priority|set_process_affinity)["']/g)].map((match) => ({
    file: relative(frontendRoot, path).split("\\").join("/"),
    command: match[1],
  }));
});
assert.deepEqual(
  mutationInvocations.map((call) => call.command).sort(),
  ["kill_process", "set_priority", "set_process_affinity"].sort(),
  "all mutation IPC commands must remain centralized in exactly the required API wrappers",
);
assert.ok(
  mutationInvocations.every((call) => call.file === "lib/api.ts"),
  "no page/component may invoke PID mutation commands directly",
);

assert.match(
  rust,
  /fn process_creation_identity_matches\(expected: u64, actual: Option<u64>\) -> bool \{\s*expected != 0 && actual == Some\(expected\)/,
  "process mutations require an exact, nonzero FILETIME match",
);
assert.match(
  rust,
  /fn open_verified_process_handle\([\s\S]*?OpenProcess\(desired_access, 0, pid\)[\s\S]*?process_times\(handle\.as_raw\(\)\)[\s\S]*?process_creation_identity_matches\(expected_creation_filetime, actual_creation_filetime\)[\s\S]*?Ok\(handle\)/,
  "the handle returned to callers must be opened and identity-checked before mutation",
);
assert.match(rust, /impl Drop for OwnedProcessHandle[\s\S]*?CloseHandle\(self\.0\)/);
assert.match(rust, /fn parse_process_creation_token\(token: &str\)[\s\S]*?parse_creation_token\(token\)[\s\S]*?PROCESS_MUTATION_IDENTITY_ERROR/);
assert.match(rust, /pub\(crate\) fn process_creation_filetime_for_sample\([\s\S]*?process_row_matches_current_identity\([\s\S]*?Some\(creation_filetime\)/);
assert.match(rust, /fn query_process_image_with_buffer\([\s\S]*?buffer\.resize\(PROCESS_IMAGE_PATH_BUFFER_CHARS, 0\)[\s\S]*?String::from_utf16_lossy\(&buffer\[\.\.length as usize\]\)/);
assert.match(stats, /let mut process_image_query_buffer: Vec<u16> = Vec::new\(\)[\s\S]*?capture_process_identities[\s\S]*?process_creation_filetime_for_sample\([\s\S]*?&mut process_image_query_buffer[\s\S]*?proc_creation_filetime_cache\.insert/);
assert.match(stats, /d\.set_process_creation_filetimes\(&proc_creation_filetime_cache\)/);
assert.match(diag, /#\[serde\(skip_serializing\)\]\s*pub\(crate\) creation_filetime: Option<u64>/);
assert.match(diag, /exact_identity_changed[\s\S]*?PidTrack::default\(\)[\s\S]*?creation_filetime: t\.creation_filetime/);
assert.match(diag, /confirmed_anomaly_carries_streak_onset_time_and_internal_identity/);
assert.match(diag, /pid_reuse_resets_metric_baselines_and_drops_stale_io_alerts/);

const kill = functionBody(rust, "pub fn kill_process(");
assert.match(kill, /creation_token: String[\s\S]*?parse_process_creation_token\(&creation_token\)\?[\s\S]*?open_verified_process_handle\([\s\S]*?PROCESS_TERMINATE \| PROCESS_QUERY_LIMITED_INFORMATION[\s\S]*?TerminateProcess\(handle\.as_raw\(\), 1\)/);

const verifiedPriority = functionBody(rust, "pub(crate) fn set_priority_verified(");
assert.match(verifiedPriority, /expected_creation_filetime: Option<u64>[\s\S]*?filter\(\|creation\| \*creation != 0\)[\s\S]*?open_verified_process_handle\([\s\S]*?PROCESS_SET_INFORMATION \| PROCESS_QUERY_LIMITED_INFORMATION[\s\S]*?set_priority_class_on_handle\(handle\.as_raw\(\), class\)/);
const priority = functionBody(rust, "pub fn set_priority(");
assert.match(priority, /creation_token: String[\s\S]*?parse_process_creation_token\(&creation_token\)\?[\s\S]*?set_priority_verified\(pid, Some\(expected_creation_filetime\), priority\)/);
assert.doesNotMatch(priority, /set_priority_raw\(/, "the UI command must not reopen the PID through the automatic raw path");

const affinity = functionBody(rust, "pub fn set_process_affinity(");
assert.match(affinity, /creation_token: String[\s\S]*?parse_process_creation_token\(&creation_token\)\?[\s\S]*?open_verified_process_handle\([\s\S]*?PROCESS_SET_INFORMATION \| PROCESS_QUERY_INFORMATION[\s\S]*?GetProcessAffinityMask\(handle\.as_raw\(\)[\s\S]*?SetProcessAffinityMask\(handle\.as_raw\(\)/);

assert.match(api, /killProcess: \(pid: number, creationToken: string\)[\s\S]*?invoke<void>\("kill_process", \{ pid, creationToken \}\)/);
assert.match(api, /setPriority: \(pid: number, creationToken: string, priority: number, allowRealtime = false\)[\s\S]*?invoke<void>\("set_priority", \{ pid, creationToken, priority, allowRealtime \}\)/);
assert.match(api, /setProcessAffinity: \(pid: number, creationToken: string, kind: string\)[\s\S]*?invoke<void>\("set_process_affinity", \{ pid, creationToken, kind \}\)/);

assert.match(ui, /const requireVerifiedActionIdentity = \(p: ProcInfo\)[\s\S]*?processMutationsDisabled[\s\S]*?formatWindowsFiletimeUtc\(creationToken\)[\s\S]*?return \{ creationToken, creationTime \}/);
assert.match(ui, /api\.killProcess\(p\.pid, identity\.creationToken\)/);
assert.match(ui, /api\.setPriority\(p\.pid, identity\.creationToken, level, level === 5\)/);
assert.match(ui, /api\.setProcessAffinity\(p\.pid, identity\.creationToken, kind\)/);
assert.match(ui, /api\.resumeProcess\(p\.pid, identity\.creationToken\)/);
assert.match(ui, /确定结束「\$\{p\.name\}」\(PID \$\{p\.pid\}，创建时间 \$\{identity\.creationTime\}\)/);
assert.match(ui, /已向 \$\{p\.name\}（PID \$\{p\.pid\}，\$\{identity\.creationTime\}）发送结束请求/);
assert.match(ui, /aria-label=\{rowMutationsDisabled \? `\$\{p\.name\}，PID \$\{p\.pid\}，优先级暂不可用/);
assert.match(ui, /aria-label=\{rowMutationsDisabled \? `\$\{p\.name\}，PID \$\{p\.pid\}，CPU 亲和性暂不可用/);
assert.match(ui, /rowMutationsDisabled = processMutationsDisabled \|\| !identityVerified/);
assert.match(rust, /fn sample\(&mut self\)[\s\S]*?sample_plan\(collect, true\)[\s\S]*?last_creation_filetimes[\s\S]*?mask_unstable_process_row_tokens/);
assert.match(rust, /fn sample_details\(&mut self\)[\s\S]*?sample_plan\(collect_details, false\)/);
assert.match(rust, /ProcessListSamplePlan::Warmup[\s\S]*?capture_top_process_filetimes[\s\S]*?self\.system = System::new\(\)[\s\S]*?refresh_processes_specifics[\s\S]*?first_sample = Instant::now\(\)[\s\S]*?std::thread::sleep[\s\S]*?refresh_processes_specifics[\s\S]*?collect_with/);
assert.match(rust, /ProcessListSamplePlan::Steady\(previous\)[\s\S]*?capture_top_process_filetimes[\s\S]*?unchanged_process_filetimes[\s\S]*?refresh_processes_specifics[\s\S]*?process_list_sample_elapsed\(previous, completed\)[\s\S]*?collect_with/);
assert.match(rust, /fn unchanged_process_filetimes[\s\S]*?observed\.get\(pid\) == Some\(filetime\)/);
assert.match(rust, /fn capture_top_process_filetimes[\s\S]*?process_creation_filetime_for_sample\([\s\S]*?image_buffer/);
assert.match(rust, /fn mask_unstable_process_row_tokens[\s\S]*?before_refresh\.get\(&row\.pid\)\.copied\(\) != after_refresh[\s\S]*?row\.creation_token = None/);
assert.match(rust, /process_row_tokens_are_kept_only_when_filetime_matches_across_refresh/);
assert.match(rust, /process_row_identity_change_stays_read_only_until_followup_sample_is_stable/);

const resume = functionBody(rust, "pub fn resume_pid_raw(");
assert.match(resume, /PROCESS_SUSPEND_RESUME \| PROCESS_QUERY_LIMITED_INFORMATION[\s\S]*?process_times\(handle\)[\s\S]*?process_creation_identity_matches\(expected_creation_filetime, actual_creation_filetime\)[\s\S]*?let status = func\(handle\)/);
assert.match(api, /resumeProcess: \(pid: number, creationToken: string\)[\s\S]*?invoke<void>\("resume_process", \{ pid, creationToken \}\)/);

assert.doesNotMatch(dashboard, /api\.killProcess\(/, "stale diagnostic anomalies must not terminate directly by PID");
assert.match(dashboard, /onClick=\{\(\) => onNav\("processes", a\.pid\)\}/);
const verifiedAutoCalls = [...auto.matchAll(/set_priority_verified\(a\.pid, a\.creation_filetime, 1\)/g)];
assert.equal(verifiedAutoCalls.length, 2, "both direct deprio and autopilot escalation must use sampled identity");
assert.doesNotMatch(auto, /set_priority_raw\(a\.pid, 1\)/, "automatic actions must not fall back to PID-only mutation");
assert.match(auto, /configured_deprio_preserves_foreground_policy_and_fails_closed_without_identity/);
assert.match(auto, /本次持续异常期间不会自动重试[\s\S]*?待异常解除后重新触发/);
assert.match(auto, /显式配置的 `deprio` 规则保留用户选定的动作，可能作用于前台应用/);
assert.match(autoUi, /显式“降低优先级”规则可能影响当前前台应用[\s\S]*?“异常自动降级”只把提醒升级到非关键、非前台进程/);
assert.doesNotMatch(rust, /set_priority_raw/, "the PID-only priority setter must not remain available");
assert.match(release, /PID-only `set_priority_raw` path has been removed[\s\S]*?Both explicit `deprio` rules and autopilot escalation carry the sampled FILETIME into a same-handle verified setter[\s\S]*?thresholds\/durations, autopilot and critical-process checks, and auto-dispose's non-foreground exclusion remain unchanged[\s\S]*?explicitly configured `deprio` rules may still affect a foreground process/i);
assert.match(release, /On steady scans, the list retains an action token only when exact FILETIME from the previous row snapshot, a current pre-refresh query, and the post-refresh row token all agree[\s\S]*?warmup reseeds sysinfo's CPU\/I\/O baseline after its pre-token capture[\s\S]*?previous\/pre\/post exact-identity check narrows but does not make row sampling atomic/i);
assert.match(release, /automatic FILETIME is still queried adjacent to \(not atomically with\) sysinfo metrics[\s\S]*?any observed coarse\/exact identity transition now discards that scan's process metrics/i);
assert.match(release, /On a disposable Windows VM[\s\S]*?PID replacement across the list-refresh bracket becomes read-only with refresh guidance[\s\S]*?trigger both configured `deprio` and autopilot auto-dispose/);
assert.match(rust, /process_identity_must_match_the_exact_nonzero_creation_time/);
assert.match(rust, /process_mutation_tokens_fail_closed_with_refresh_guidance/);

console.log("Process mutation identity passed: manual and automatic priority actions verify sampled FILETIME on the mutation handle");
