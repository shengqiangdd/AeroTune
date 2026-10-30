import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { MEMORY_FORECAST_MAX_SECONDS, MEMORY_FORECAST_MIN_SAMPLES, MEMORY_RISING_SLOPE_THRESHOLD, MEMORY_SLOPE_MIN_SAMPLES, MEMORY_SLOPE_WINDOW, memoryPressureStatus } from "../src/lib/memoryPressure.js";
import { mergeMemoryCandidateResults } from "../src/lib/memoryCandidateResults.js";
import { aggregateMemoryOptimizationBatches } from "../src/lib/memoryOptimizationBatch.js";

const processSource = readFileSync(new URL("../src-tauri/src/processes.rs", import.meta.url), "utf8");
const statsSource = readFileSync(new URL("../src-tauri/src/stats.rs", import.meta.url), "utf8");
const diagSource = readFileSync(new URL("../src-tauri/src/diag.rs", import.meta.url), "utf8");
const autoSource = readFileSync(new URL("../src-tauri/src/auto.rs", import.meta.url), "utf8");
const processUiSource = readFileSync(new URL("../src/pages/Processes.tsx", import.meta.url), "utf8");
const apiSource = readFileSync(new URL("../src/lib/api.ts", import.meta.url), "utf8");
const trimMemory = processSource.match(/pub fn trim_memory_sync\(\)[\s\S]*?\n}/)?.[0];
assert.ok(trimMemory, "legacy trim IPC must remain present");
assert.match(trimMemory, /optimize_process_memory_sync\(false, None\)/, "manual trim should reuse its already-safe single scan");
assert.doesNotMatch(trimMemory, /prepare_memory_scan|inspect_memory_candidates/, "manual trim must not rescan and discard a preview");
assert.match(processSource, /if should_wait_for_memory_reclamation\(report\.estimated_reclaimed_mb\)[\s\S]*?post_trim_measurement_delay/);
assert.match(processSource, /pub available_after_mb: Option<f64>/);
assert.match(processSource, /fn validated_available_memory_mb\([\s\S]*?is_finite\(\)[\s\S]*?value >= 0\.0/);
assert.match(processSource, /fn available_memory_mb\(\) -> Option<f64>[\s\S]*?physical_memory_status\(\)\?/);
assert.match(processSource, /report\.available_after_mb = available_memory_mb\(\)/);
assert.match(processSource, /fn should_auto_trim_memory\([\s\S]*?available_mb\.is_finite\(\)[\s\S]*?\(0\.0\.\.=AUTO_TRIM_AVAILABLE_LIMIT_MB\)\.contains\(&available_mb\)/);
const availableMemoryFormatter = processUiSource.match(/function formatAvailableMemory\([\s\S]*?\n}/)?.[0];
assert.ok(availableMemoryFormatter, "available-memory formatter must distinguish missing data");
assert.match(availableMemoryFormatter, /value == null[\s\S]*?Number\.isFinite\(value\)[\s\S]*?未测量/);
assert.match(availableMemoryFormatter, /value\.toFixed\(0\)/);
assert.doesNotMatch(availableMemoryFormatter, /value <= 0/, "a verified zero reading must remain visible as zero");
assert.match(processUiSource, /formatAvailableMemory\(report\.available_after_mb\)/);
assert.match(processUiSource, /formatAvailableMemory\(memoryReport\.available_after_mb\)/);
assert.match(apiSource, /available_after_mb: number \| null/);
assert.match(processSource, /static AUTO_TRIM_LAST_SUCCESS: Mutex<Option<Instant>>/);
assert.match(processSource, /static AUTO_TRIM_LAST_SCAN_ATTEMPT: Mutex<Option<Instant>>/);
assert.match(processSource, /const AUTO_TRIM_SCAN_RETRY_SECS: u64 = 60/);
assert.match(processSource, /fn should_auto_trim_memory\([\s\S]*?available_mb\.is_finite\(\)[\s\S]*?\(0\.0\.\.=AUTO_TRIM_AVAILABLE_LIMIT_MB\)\.contains\(&available_mb\)/);
assert.match(processSource, /fn auto_trim_cooldown_active\(previous: Option<Instant>, now: Instant, cooldown: Duration\)[\s\S]*?checked_duration_since\(previous\)/);
const autoTrim = processSource.match(/pub fn auto_trim_background_memory_if_needed\(\)[\s\S]*?\n}/)?.[0];
assert.ok(autoTrim, "automatic trim task must remain present");
assert.match(autoTrim, /let now = Instant::now\(\)[\s\S]*?AUTO_TRIM_LAST_SUCCESS[\s\S]*?auto_trim_cooldown_active\(\*last_success, now, action_cooldown\)[\s\S]*?AUTO_TRIM_LAST_SCAN_ATTEMPT[\s\S]*?AUTO_TRIM_SCAN_RETRY_SECS[\s\S]*?\*last_scan_attempt = Some\(now\)[\s\S]*?optimize_process_memory_sync\(true, None\)/);
assert.match(autoTrim, /let report = optimize_process_memory_sync_limited_with_cpu_limit\([\s\S]*?auto_trim_commit_success_cooldown\([\s\S]*?Some\(&report\)/);
assert.match(statsSource, /tick\.is_multiple_of\(60\)[\s\S]*?auto_trim_background_memory_if_needed\(\)/);
assert.doesNotMatch(autoTrim, /SystemTime|UNIX_EPOCH/, "automatic trim cooldown must use monotonic time");
assert.match(processSource, /fn selected_process_entries<[\s\S]*?selected_pids: &HashSet<u32>[\s\S]*?processes\s*\.get\(&sysinfo::Pid::from_u32\(\*pid\)\)/);
const inspectMemory = processSource.match(/fn inspect_memory_candidates\([\s\S]*?\n}/)?.[0];
assert.ok(inspectMemory, "memory candidate inspection must remain present");
assert.match(processSource, /pub\(crate\) fn normalize_sysinfo_cpu_usage\(cpu_usage: f32, cpu_count: usize\)[\s\S]*?cpu_usage \/ cpu_count\.max\(1\) as f32\)\.min\(100\.0\)/);
assert.match(processSource, /cpu: normalize_sysinfo_cpu_usage\(process\.cpu_usage\(\), cpu_count\)/);
assert.match(inspectMemory, /let cpu = normalize_sysinfo_cpu_usage\(process\.cpu_usage\(\), cpu_count\)[\s\S]*?working_set,[\s\S]*?private_bytes,[\s\S]*?cpu,[\s\S]*?cpu,/);
assert.match(statsSource, /let cpu_count = sys\.cpus\(\)\.len\(\);[\s\S]*?cpu:\s*crate::processes::normalize_sysinfo_cpu_usage\(\s*process\.cpu_usage\(\),\s*cpu_count,\s*\)/);
assert.match(diagSource, /Process share normalized to whole-machine capacity \(0–100%\)/);
assert.match(statsSource, /proc_sample_cpu_count = cpu_count\.max\(1\)[\s\S]*?d\.set_process_cpu_count\(proc_sample_cpu_count\)/, "diagnostic CPU threshold scale must match the cached process snapshot");
assert.match(diagSource, /let cpu_per_core = s\.cpu \* self\.process_cpu_count as f32;[\s\S]*?t\.cpu\.update\([\s\S]*?cpu_per_core,[\s\S]*?ANOMALY_CPU_THRESHOLD_PER_CORE,[\s\S]*?value: s\.cpu as f64,[\s\S]*?baseline: \(t\.cpu\.center \/ self\.process_cpu_count as f32\)/, "diagnosis preserves per-core sensitivity but reports whole-machine CPU percentages");
assert.match(diagSource, /high_core_process_cpu_detection_preserves_per_core_gate_and_reports_system_percent/);
assert.match(processSource, /fn trusted_windows_directory\(\)[\s\S]*?GetSystemWindowsDirectoryW/);
assert.doesNotMatch(processSource, /std::env::var\("WINDIR"\)/, "system process protection must not trust mutable environment state");
assert.match(processSource, /fn process_protection_reason\([\s\S]*?trusted_windows_directory\(\)[\s\S]*?无法确认 Windows 系统目录/);
assert.match(processSource, /fn foreground_process_id\(\) -> Option<u32>[\s\S]*?thread_id != 0 && pid != 0/);
assert.match(processSource, /fn optimize_process_memory_sync_limited_with_cpu_limit\([\s\S]*?require_foreground_process_id\(foreground_process_id\(\)\)/, "preview and apply preflight fail closed if foreground identity is unavailable");
assert.match(processSource, /let foreground_error = require_foreground_process_id\(current_foreground\)[\s\S]*?let rejection_reason = foreground_error/);
assert.match(processSource, /unknown_foreground_identity_blocks_memory_trim_preflight/);
assert.match(processSource, /windows_directory_query_uses_bounded_os_result_and_fails_closed/);
assert.doesNotMatch(autoSource, /std::env::var\("WINDIR"\)/, "CPU scheduler protection must not trust mutable WINDIR");
assert.match(autoSource, /let windows_root = crate::processes::trusted_windows_directory\(\)/);
assert.match(autoSource, /fn scheduler_process_allowed\([\s\S]*?windows_root: Option<&str>[\s\S]*?let Some\(windows_root\) = windows_root else \{\s*return false/);
assert.match(autoSource, /unknown Windows root must fail closed/);
assert.match(autoSource, /fn scheduler_policy_matches_legacy_string_normalization\([\s\S]*?scheduler_process_allowed_reference/);
assert.match(autoSource, /fn benchmark_scheduler_process_policy_real_process_paths\([\s\S]*?scheduler_process_allowed_reference[\s\S]*?scheduler_process_allowed\(/);
assert.match(autoSource, /fn scheduler_candidate_path_retention_borrows_valid_utf8\([\s\S]*?Cow::Borrowed/);
assert.match(autoSource, /fn benchmark_scheduler_candidate_path_retention\([\s\S]*?owned_nanos[\s\S]*?borrowed_nanos/);
assert.match(autoSource, /fn sort_scheduler_candidates\([\s\S]*?sort_unstable_by\(compare_scheduler_candidates\)/);
assert.match(autoSource, /fn scheduler_unstable_sort_matches_stable_total_order[\s\S]*?sort_scheduler_candidates_stable_reference[\s\S]*?sort_scheduler_candidates[\s\S]*?fn benchmark_scheduler_candidate_sort_stable_vs_unstable[\s\S]*?stable_nanos[\s\S]*?unstable_nanos/);
const schedulerPolicy = autoSource.match(/fn scheduler_process_allowed\([\s\S]*?\n}/)?.[0];
assert.ok(schedulerPolicy, "scheduler policy must remain a bounded helper");
assert.doesNotMatch(schedulerPolicy, /to_ascii_lowercase\(|\.replace\(|format!\(/, "candidate path checks must not allocate normalization strings");
assert.match(processUiSource, /全机逻辑处理器总能力归一到 0–100%/);
assert.match(processSource, /fn benchmark_process_list_steady_sampler_against_two_scan_baseline\(\)[\s\S]*?steady_median < baseline_median/, "manual sampler benchmark must assert the steady path remains cheaper than its two-scan baseline");
assert.match(processSource, /fn benchmark_top_500_selection_against_full_materialization\(\)[\s\S]*?selected_median < full_median/, "manual top-500 benchmark must assert selection remains cheaper than full DTO materialization");
assert.match(inspectMemory, /let ordered = if let Some\(selected\) = eligible_pids \{\s*selected_process_entries\(sys\.processes\(\), selected\)\s*\} else \{[\s\S]*?ordered\.sort_by_key/);
assert.match(inspectMemory, /append_missing_selected_memory_candidates\(&mut candidates, requested\)/);
const defaultSync = processSource.match(/fn optimize_process_memory_sync_limited\([\s\S]*?\n}/)?.[0];
assert.ok(defaultSync, "bounded memory optimizer must remain present");
assert.match(defaultSync, /MAX_TRIM_CPU_PERCENT/);
const foregroundSync = processSource.match(/fn optimize_process_memory_sync_limited_with_cpu_limit\([\s\S]*?\n}/)?.[0];
assert.ok(foregroundSync, "foreground-querying memory optimizer must remain present");
assert.match(foregroundSync, /require_foreground_process_id\(foreground_process_id\(\)\)[\s\S]*?optimize_process_memory_sync_limited_with_cpu_limit_for_foreground/);
const selectedSync = processSource.match(/fn optimize_process_memory_sync_limited_with_cpu_limit_for_foreground\([\s\S]*?\n}/)?.[0];
assert.ok(selectedSync, "foreground-validated memory optimizer must remain present");
assert.match(selectedSync, /require_foreground_process_id\(Some\(foreground_pid\)\)[\s\S]*?validate_requested_pids/);
assert.match(selectedSync, /if !preview_only \{\s*if let Some\(selected_pids\) = eligible_pids\.as_ref\(\)/);
const selectedArmStart = selectedSync.indexOf("if let Some(selected_pids) = eligible_pids.as_ref()");
const selectedArmEnd = selectedSync.indexOf("\n        } else {", selectedArmStart);
assert.ok(selectedArmStart >= 0 && selectedArmEnd > selectedArmStart, "selected apply branch must remain nested under the non-preview mode");
const selectedArm = selectedSync.slice(selectedArmStart, selectedArmEnd);
assert.match(selectedArm, /inspect_selected_memory_candidates\([\s\S]*?min_working_set_bytes[\s\S]*?expected_creation_times\.as_ref\(\)/);
assert.doesNotMatch(selectedArm, /System::new|prepare_memory_scan/, "selected apply must not enumerate or warm all processes");
const fullScanArms = selectedSync.slice(selectedArmEnd);
assert.match(fullScanArms, /System::new\(\)[\s\S]*?prepare_memory_scan/, "previews and unselected operations retain full scanning");
assert.match(selectedSync, /memory_candidate_eligible_with_limits\([\s\S]*?max_cpu_percent[\s\S]*?min_working_set_bytes/);
assert.match(selectedSync, /mark_candidate_report_ineligible\(&mut report, candidate\.pid/);
assert.match(processSource, /fn apply_cpu_samples_to_candidates\([\s\S]*?candidate\.reason = "空闲第三方进程"/);
const selectedInspect = processSource.match(/fn inspect_selected_memory_candidates\([\s\S]*?\n}/)?.[0];
assert.ok(selectedInspect, "selected-PID inspection must remain present");
assert.match(selectedInspect, /OpenProcess\(PROCESS_QUERY_INFORMATION[\s\S]*?query_process_image\(handle\)[\s\S]*?process_times\(handle\)[\s\S]*?read_process_memory\(handle\)/);
assert.match(selectedInspect, /memory_candidate_eligible_without_cpu_with_minimum\([\s\S]*?min_working_set_bytes/);
assert.match(processSource, /rename = "creation_token"[\s\S]*?serialize_with = "serialize_creation_filetime"/);
assert.match(processSource, /fn serialize_creation_filetime[\s\S]*?format_creation_token/);
assert.match(processSource, /pub struct MemoryCandidateIdentity/);
assert.match(processSource, /fn parse_creation_token[\s\S]*?fn parse_memory_candidate_identities/);
assert.match(selectedSync, /candidate\.creation_filetime\.is_some_and[\s\S]*?current_creation_filetime[\s\S]*?sample\.creation_filetime/);
const manualOptimize = processSource.match(/pub async fn optimize_process_memory\([\s\S]*?\n}/)?.[0];
assert.ok(manualOptimize, "manual selected-process command must remain present");
assert.match(manualOptimize, /pids: Vec<MemoryCandidateIdentity>/);
assert.match(manualOptimize, /parse_memory_candidate_identities\(pids\)[\s\S]*?Some\(expected_creation_times\)/);
assert.match(processUiSource, /const identities: MemoryCandidateIdentity\[\] = pids\.map[\s\S]*?seed\?\.candidates\.find[\s\S]*?candidate\?\.creation_token/);
assert.match(processUiSource, /const candidateResults = new Map<number, MemoryCandidate>[\s\S]*?report\.candidates\) candidateResults\.set[\s\S]*?mergeMemoryCandidateResults\([\s\S]*?attemptedPids[\s\S]*?candidateResults\.values\(\)/);
assert.match(processUiSource, /attemptedPids\.push[\s\S]*?report = await api\.optimizeProcessMemory\(batch\)[\s\S]*?catch \(error\)[\s\S]*?interrupted = true[\s\S]*?break/);
assert.match(processUiSource, /const batchReports: MemoryOptimizationReport\[\] = \[\][\s\S]*?batchReports\.push\(report\)[\s\S]*?aggregateMemoryOptimizationBatches\(batchReports, seed, interrupted\)/);
assert.doesNotMatch(processUiSource, /firstBefore\s*\|\||if\s*\(!firstBefore\)/, "valid zero-MB pre-action readings must not be treated as uninitialized");
assert.match(processUiSource, /role="alert"[\s\S]*?memoryOperationWarning/);
assert.match(processUiSource, /if \(interruption\) \{\s*toastErr\(interruption\)/);
assert.match(apiSource, /optimizeProcessMemory: \(pids: MemoryCandidateIdentity\[\]\)/);
const previewRows = [
  { pid: 101, name: "worker.exe", eligible: true, reason: "空闲第三方进程", creation_token: "000000000001e240" },
  { pid: 202, name: "renderer.exe", eligible: true, reason: "空闲第三方进程", creation_token: "000000000001e241" },
  { pid: 303, name: "idle.exe", eligible: true, reason: "空闲第三方进程", creation_token: "000000000001e242" },
  { pid: 404, name: "stale.exe", eligible: true, reason: "空闲第三方进程", creation_token: "000000000001e243" },
];
const mergedRows = mergeMemoryCandidateResults(previewRows, [101, 202, 404], [
  { pid: 101, reason: "CPU 采样不可用或进程实例已变化，跳过" },
  { pid: 202, reason: "本次操作已完成" },
]);
assert.equal(mergedRows[0].reason, "CPU 采样不可用或进程实例已变化，跳过");
assert.equal(mergedRows[0].eligible, false);
assert.equal(mergedRows[0].creation_token, undefined, "attempted preview token must not be reused");
assert.equal(mergedRows[1].reason, "本次操作已完成");
assert.equal(mergedRows[1].eligible, false);
assert.equal(mergedRows[2], previewRows[2], "unselected candidates remain unchanged/selectable");
assert.equal(mergedRows[3].reason, "本次操作结果未返回，请重新分析");
assert.equal(mergedRows[3].eligible, false);

const firstBatchReport = {
  available_before_mb: 0,
  available_after_mb: 4,
  candidates: [],
  processed: 1,
  touched_working_set_mb: 20,
  estimated_reclaimed_mb: 5,
  skipped: 1,
};
const secondBatchReport = {
  available_before_mb: 7,
  available_after_mb: 3,
  candidates: [],
  processed: 2,
  touched_working_set_mb: 40,
  estimated_reclaimed_mb: 11,
  skipped: 0,
};
const combinedBatches = aggregateMemoryOptimizationBatches([firstBatchReport, secondBatchReport], null);
assert.equal(combinedBatches.available_before_mb, 0, "verified zero MB must remain the first-batch baseline");
assert.equal(combinedBatches.available_after_mb, 3, "successful batches report the latest after-measurement");
assert.equal(combinedBatches.processed, 3);
assert.equal(combinedBatches.touched_working_set_mb, 60);
assert.equal(combinedBatches.estimated_reclaimed_mb, 16);
assert.equal(combinedBatches.skipped, 1);
const interruptedBatches = aggregateMemoryOptimizationBatches([firstBatchReport], null, true);
assert.equal(interruptedBatches.available_before_mb, 0);
assert.equal(interruptedBatches.available_after_mb, null, "an interrupted batch must not present a stale after-measurement as final");
const unknownAfterNoResponse = aggregateMemoryOptimizationBatches([], firstBatchReport, true);
assert.equal(unknownAfterNoResponse.available_before_mb, 0);
assert.equal(unknownAfterNoResponse.available_after_mb, null);
assert.equal(unknownAfterNoResponse.processed, 0);
assert.match(processSource, /fn creation_time_matches_expected\([\s\S]*?expected\.get\(&pid\)\.copied\(\) == Some\(actual_creation_filetime\)/);
assert.match(autoTrim, /candidate\s*\.creation_filetime[\s\S]*?Some\(expected_creation_times\)/);
assert.match(autoTrim, /optimize_process_memory_sync_limited_with_cpu_limit\([\s\S]*?AUTO_TRIM_CPU_PERCENT[\s\S]*?AUTO_TRIM_WORKING_SET_BYTES/);
assert.match(processSource, /const AUTO_TRIM_WORKING_SET_BYTES: u64 = 512 \* 1024 \* 1024/);
const cpuSample = processSource.match(/fn sample_process_cpu\([\s\S]*?\n}/)?.[0];
assert.ok(cpuSample, "fresh process CPU sampler must remain present");
assert.match(cpuSample, /if first\.is_empty\(\) \{\s*return result;\s*\}\s*let Some\(logical_processors\) = system_logical_processor_count\(\) else \{\s*return result;\s*\};\s*[^;]*std::thread::sleep\(Duration::from_secs\(1\)\)/);
assert.match(cpuSample, /first\.insert\(\*pid, \(cpu_ticks, creation_filetime, Instant::now\(\)\)\)/);
assert.match(cpuSample, /process_cpu_usage_percent\([\s\S]*?sampled_at\.elapsed\(\)/);
assert.doesNotMatch(cpuSample, /available_parallelism/);
assert.match(processSource, /fn system_logical_processor_count\(\)[\s\S]*?GetActiveProcessorCount\(ALL_PROCESSOR_GROUPS\)/);
assert.match(processSource, /fn process_cpu_usage_percent\([\s\S]*?checked_sub\([\s\S]*?elapsed_seconds[\s\S]*?logical_processors/);

function diagnostic(overrides = {}) {
  return {
    score: 90,
    mem_pct: 80,
    mem_slope: 0,
    mem_slope_ready: true,
    mem_slope_sample_count: MEMORY_SLOPE_MIN_SAMPLES,
    mem_critical_eta_secs: null,
    ...overrides,
  };
}

function expectStatus(input, label, tone, percentValid, percent = Number.NaN, etaSeconds = null) {
  const actual = memoryPressureStatus(input);
  assert.equal(actual.label, label);
  assert.equal(actual.tone, tone);
  assert.equal(actual.percentValid, percentValid);
  assert.equal(actual.etaSeconds, etaSeconds);
  if (percentValid) assert.equal(actual.percent, percent);
}

expectStatus(null, "正在建立基线", "muted", false);
expectStatus(diagnostic({ score: 0 }), "正在建立基线", "muted", false);
for (const invalid of [null, undefined, Number.NaN, Number.POSITIVE_INFINITY, -0.1, 100.1]) {
  expectStatus(diagnostic({ mem_pct: invalid }), "内存水位数据不可用", "muted", false);
}
expectStatus(diagnostic({ mem_pct: 77.99 }), "余量充足", "good", true, 77.99);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: false, mem_slope: 0, mem_slope_sample_count: 6 }),
  "内存趋势实测读数积累中（6/10）",
  "warning",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: false, mem_slope_sample_count: -2 }),
  "内存趋势实测读数积累中（0/10）",
  "warning",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: false, mem_slope_sample_count: 17 }),
  "内存趋势就绪状态待同步（17 个有效实测读数）",
  "warning",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: false, mem_slope_sample_count: 100 }),
  `内存趋势就绪状态待同步（${MEMORY_SLOPE_WINDOW} 个有效实测读数）`,
  "warning",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: false, mem_slope_sample_count: undefined }),
  "内存趋势实测读数积累中（0/10）",
  "warning",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: true, mem_slope: 0.0501 }),
  "持续上升，正在关注",
  "warning",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 84, mem_slope_ready: true, mem_slope: 0.06, mem_slope_sample_count: MEMORY_FORECAST_MIN_SAMPLES, mem_critical_eta_secs: 100 }),
  "持续上升，正在关注",
  "warning",
  true,
  84,
  100,
);
expectStatus(
  diagnostic({ mem_pct: 84, mem_slope_ready: true, mem_slope: 0.06, mem_slope_sample_count: MEMORY_FORECAST_MIN_SAMPLES - 1, mem_critical_eta_secs: 100 }),
  "持续上升，正在关注",
  "warning",
  true,
  84,
);
for (const invalidEta of [0, -1, MEMORY_FORECAST_MAX_SECONDS + 1, 1.5, Number.NaN, Number.POSITIVE_INFINITY]) {
  expectStatus(
    diagnostic({ mem_pct: 84, mem_slope_ready: true, mem_slope: 0.06, mem_slope_sample_count: MEMORY_FORECAST_MIN_SAMPLES, mem_critical_eta_secs: invalidEta }),
    "持续上升，正在关注",
    "warning",
    true,
    84,
  );
}
expectStatus(
  diagnostic({ mem_pct: 84, mem_slope_ready: true, mem_slope: MEMORY_RISING_SLOPE_THRESHOLD, mem_critical_eta_secs: 60 }),
  "偏高但趋势平稳",
  "warning",
  true,
  84,
);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: true, mem_slope: 0.05 }),
  "偏高但趋势平稳",
  "warning",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 78, mem_slope_ready: true, mem_slope: Number.NaN }),
  "内存趋势数据不可用",
  "muted",
  true,
  78,
);
expectStatus(
  diagnostic({ mem_pct: 90, mem_slope_ready: false, mem_slope: Number.NaN }),
  "压力偏高，建议查看进程",
  "warning",
  true,
  90,
);

console.log("Memory pressure checks passed: selected-apply fast path and identity, auto/manual CPU+WS thresholds, report consistency, invalid telemetry, and one-pass trims");
