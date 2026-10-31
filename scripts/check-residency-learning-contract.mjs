// 常驻模式学习 + 实验一致性影子分析的源码合同：钉住"只观察不处置、
// 无状态不落盘、诚实门槛"三条红线与采样器低频接入。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (file) =>
  readFileSync(new URL(`../${file}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");

const apiSource = read("src/lib/api.ts");
const processesSource = read("src/pages/Processes.tsx");
const labSource = read("src/components/OptimizationLab.tsx");
const residencySource = read("src-tauri/src/residency.rs");
const insightSource = read("src-tauri/src/experiment_insight.rs");
const statsSource = read("src-tauri/src/stats.rs");
const libSource = read("src-tauri/src/lib.rs");

// ---- IPC 契约 ----
assert.match(apiSource, /getResidencySuggestions: \(\) => invoke<ResidencySuggestion\[\]>\("get_residency_suggestions"\)/);
assert.match(apiSource, /dismissResidencySuggestion: \(name: string\) => invoke<boolean>\("dismiss_residency_suggestion", \{ name \}\)/);
assert.match(apiSource, /analyzeExperimentConsistency: \(records: ExperimentRecordInput\[\]\)/);
assert.match(libSource, /residency::get_residency_suggestions,/);
assert.match(libSource, /residency::dismiss_residency_suggestion,/);
assert.match(libSource, /experiment_insight::analyze_experiment_consistency,/);

// ---- 常驻模式学习：观察边界与门槛 ----
assert.match(residencySource, /只建议不处置，\n\/\/ 任何动作由用户在进程管理中自行决定/, "residency module keeps its observe-only charter");
assert.match(residencySource, /只统计 AeroTune 运行期间的观测/, "residency scope is honestly scoped to app-runtime observations");
assert.match(residencySource, /pub const RESIDENCY_OBSERVE_INTERVAL_SECS: u64 = 30 \* 60;/, "observation is low-frequency (30 minutes)");
assert.match(residencySource, /pub const SUGGESTION_MIN_DAYS: usize = 3;/);
assert.match(residencySource, /pub const SUGGESTION_MIN_SAMPLES: u64 = 8;/);
assert.match(residencySource, /pub const SUGGESTION_MIN_AVG_MEM_MB: f64 = 128\.0;/);
assert.match(residencySource, /pub const SUGGESTION_MIN_LOW_CPU_RATIO: f64 = 0\.8;/);
assert.match(residencySource, /const STORE_MAX_ENTRIES: usize = 128;/, "residency store entry count is bounded");
assert.match(residencySource, /crate::util::atomic_write\(&self\.path, &bytes\)/, "residency store persists via shared atomic write");
assert.match(
  residencySource,
  /crate::processes::PROTECTED_PROCESSES\.contains\(&normalized\.as_str\(\)\)/,
  "protected system processes are never observed",
);
assert.match(
  statsSource,
  /crate::residency::observe_from_sampler\(&proc_sample_cache\);/,
  "sampler feeds residency learning from the shared process snapshot",
);
assert.match(
  statsSource,
  /next_residency_observe = std::time::Instant::now\(\)\s*\n\s*\+ Duration::from_secs\(crate::residency::RESIDENCY_OBSERVE_INTERVAL_SECS\)/,
  "observation reschedules at the declared cadence",
);

// ---- 实验一致性：无状态、门槛、非因果、不调参 ----
assert.match(insightSource, /观察层——只输出"是否构成一致性证据"的结论，绝不自动调整任何参数/, "experiment insight keeps its no-auto-tuning charter");
assert.match(insightSource, /无状态、无 IO/, "experiment insight is stateless with no IO");
assert.match(insightSource, /pub const MIN_EXPERIMENTS_PER_MODE: usize = 5;/);
assert.match(insightSource, /pub const CONSISTENCY_RATIO: f64 = 0\.8;/);
assert.match(insightSource, /pub const MIN_WINDOW_COVERAGE: f64 = 0\.7;/, "low-coverage windows are excluded");
assert.match(insightSource, /record\.status != "complete" \{[\s\S]*?continue/, "interrupted experiments never enter analysis");
assert.match(insightSource, /不会据此自动调整参数/, "ready verdict still discloses no auto-tuning");
assert.match(insightSource, /不构成调整依据/, "inconsistent/insufficient verdicts refuse to justify tuning");
assert.match(labSource, /不构成因果结论，也不作为任何自动调参的依据/, "lab consistency card keeps the non-causal no-auto-tuning wording");

// ---- 前端：进程页诚实文案、实验页只读分析 ----
assert.match(processesSource, /只做观察提示、不会自动处置任何进程/, "process page discloses observe-only behavior");
assert.match(
  processesSource,
  /仅 AeroTune 运行期间、每 30 分钟一个样本\），不代表全天行为/,
  "process page honestly scopes what the observations cover",
);
assert.match(
  labSource,
  /api\.analyzeExperimentConsistency\(payload\)/,
  "lab feeds local records into the stateless analysis",
);
assert.match(
  labSource,
  /不构成因果结论，也不作为任何自动调参的依据/,
  "lab consistency card keeps the no-auto-tuning disclaimer",
);
assert.match(
  labSource,
  /const payload: ExperimentRecordInput\[\] = history\.filter\(\(record\) =>\s*comparablePowerSource\(record\.baseline\.powerContext, record\.after\?\.powerContext\),\s*\)\.map\(\(record\) => \(\{/,
  "analysis payload derives from local history after comparable power-source filtering",
);

// ---- 学习建议接入建议管线：低优先级观察类条目、上限、只导航 ----
const recsSource = read("src-tauri/src/recommendations.rs");
assert.match(recsSource, /pub fn append_learning_recommendations\(/);
assert.match(recsSource, /const LEARNING_RECOMMENDATION_CAP: usize = 2;/, "learning entries cannot crowd out telemetry advice");
assert.match(recsSource, /snapshot_residency\(\)/, "recommendation command reuses the residency snapshot gate");
assert.match(recsSource, /snapshot_scene\(&game_list\)/, "recommendation command reuses the scene snapshot gate");
assert.match(recsSource, /仅为本机观察提示，不自动处置/, "resident recommendation keeps its observe-only wording");
assert.match(recsSource, /是否加入名单由你决定/, "scene recommendation keeps its user-controlled wording");
assert.match(
  recsSource,
  /let mut out = recommend_from_snapshots\(&stats, &diagnostics, &system_drive\);\s*\n\s*append_learning_recommendations\(&mut out, &residency, &scene\);/,
  "telemetry recommendations stay first; learning entries append after",
);
assert.match(
  read("docs/DATA_PRIVACY_INVENTORY.md"),
  /scene-shadow\.json/,
  "privacy inventory documents the scene-shadow store",
);
assert.match(
  read("docs/DATA_PRIVACY_INVENTORY.md"),
  /residency-patterns\.json/,
  "privacy inventory documents the residency-patterns store",
);
assert.match(
  read("docs/DATA_PRIVACY_INVENTORY.md"),
  /The experiment-consistency analysis is stateless/,
  "privacy inventory documents the stateless consistency analysis",
);

console.log("residency-learning contract passed: 33 checks");
