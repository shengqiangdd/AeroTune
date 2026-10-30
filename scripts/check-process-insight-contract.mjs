// 进程画像与异常根因引导的源码合同：钉住命令注册、轮询防抖、
// 诚实声明（非因果、不自动处置）与启发式边界，防止后续改动悄悄丢失保障。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (file) =>
  readFileSync(new URL(`../${file}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");

const apiSource = read("src/lib/api.ts");
const dashboardSource = read("src/pages/Dashboard.tsx");
const processesSource = read("src/pages/Processes.tsx");
const libSource = read("src-tauri/src/lib.rs");
const profileSource = read("src-tauri/src/profile.rs");
const rootcauseSource = read("src-tauri/src/rootcause.rs");
const diagSource = read("src-tauri/src/diag.rs");

// ---- IPC 契约：前端封装与后端注册一一对应 ----
assert.match(apiSource, /getAnomalyContext: \(\) => invoke<AnomalyContext\[\]>\("get_anomaly_context"\)/);
assert.match(apiSource, /getProcessInsights: \(\) => invoke<ProcessInsights>\("get_process_insights"\)/);
assert.match(libSource, /profile::get_process_insights,/);
assert.match(libSource, /rootcause::get_anomaly_context,/);
assert.match(apiSource, /onset_unix: number \| null;/);
assert.match(apiSource, /role: string;\n\s*role_label: string;/);

// ---- 仪表盘：根因提示仅在有异常时拉取，空态即清空并防重入 ----
assert.match(
  dashboardSource,
  /if \(value\.anomalies\.length === 0\) \{\s*\n\s*setAnomalyContext\(null\);\s*\n\s*\} else if \(!contextPending\.current\) \{/,
  "anomaly context polling must be gated on confirmed anomalies with a pending latch",
);
assert.match(dashboardSource, /getAnomalyContext\(\)/);
assert.match(
  dashboardSource,
  /仅提示可能相关，不构成因果结论/,
  "dashboard must disclose that root-cause hints are correlation, not causation",
);
assert.match(dashboardSource, /onsetAgeLabel\(context\.age_secs\)/, "anomaly rows surface the recorded onset age");

// ---- 进程页：角色徽章与洞察面板的诚实边界 ----
assert.match(
  processesSource,
  /const NOTABLE_ROLE_META[\s\S]*"updater-suspect"[\s\S]*autostart[\s\S]*unknown/,
  "process table badges limited to notable roles (updater-suspect/autostart/unknown)",
);
assert.match(processesSource, /本地画像：\$\{p\.role_label\}/, "role badge tooltip explains its local heuristic origin");
assert.match(processesSource, /后台常驻洞察 · 非系统进程（按内存排序）/, "insights panel is scoped to non-system processes");
assert.match(
  processesSource,
  /不联网查询；角色分类是启发式，可能误判/,
  "insights footer discloses local-only evidence and heuristic fallibility",
);
assert.match(
  processesSource,
  /处置动作始终由你决定，此处不自动更改任何进程/,
  "insights footer must promise no automatic action",
);
assert.match(
  processesSource,
  /if \(!initialProcessListLoaded\.current \|\| insightsPending\.current \|\| disposed \|\| document\.hidden\) return;/,
  "insights polling waits for the first successful process list and keeps its re-entrancy/visibility guards",
);
assert.match(
  processesSource,
  /const processes = await api\.listProcesses\(\);[\s\S]*?if \(!initialProcessListLoaded\.current\) \{\s*initialProcessListLoaded\.current = true;\s*requestInitialInsights\.current\?\.\(\);\s*\}/,
  "the initial insights request is released only after a successful process-list response",
);
assert.match(processesSource, /\["进程", "PID", "角色",/, "CSV export includes the profile role column");

// ---- 画像引擎：预算与缓存上限、签名边界声明 ----
assert.match(profileSource, /const SIGNATURE_BUDGET_PER_CALL: usize = 8;/, "signature verification budget per call is capped");
assert.match(profileSource, /const SIGNATURE_CACHE_CAP: usize = 512;/, "signature cache is bounded");
assert.match(
  profileSource,
  /if \*budget == 0 \{\s*\n\s*return SignatureEvidence::unknown\(\);\s*\n\s*\}/,
  "exhausted budget returns unknown without caching so later calls can verify",
);
assert.match(
  profileSource,
  /（catalog-signed）的 OS 组件会呈现为 unsigned/,
  "embedded-signature scope must be documented for catalog-signed OS binaries",
);
assert.match(
  profileSource,
  /PROTECTED_PROCESSES\.contains\(&normalized\.as_str\(\)\)[\s\S]*ROLE_SYSTEM_CORE/,
  "protected names classify as system-core before updater heuristics",
);
assert.match(
  profileSource,
  /!matches!\(\s*detail\.role\.id,\s*ROLE_SYSTEM_CORE \| ROLE_WINDOWS_BACKGROUND \| ROLE_SELF\s*\)/,
  "insights never surface system-core, windows-background, or AeroTune itself",
);
assert.match(profileSource, /只做只读分类与展示，不联网查询、不驱动任何自动处置/, "profile module keeps its read-only charter");

// ---- 根因关联：窗口边界与诚实空态 ----
assert.match(rootcauseSource, /const OWN_START_WINDOW_SECS: u64 = 180;/);
assert.match(rootcauseSource, /const CONTEXT_LOOKBACK_SECS: u64 = 300;/);
assert.match(rootcauseSource, /未发现同期的本地事件/, "empty correlation result states the honest none-found state");
assert.match(rootcauseSource, /未记录异常起点时间/, "missing onset reports an explicit unknown instead of guessing");
assert.match(
  rootcauseSource,
  /if anomalies\.is_empty\(\) \{\s*\n\s*return Ok\(Vec::new\(\)\);\s*\n\s*\}/,
  "context command returns empty without work when no anomalies exist",
);
assert.match(rootcauseSource, /不证明因果/, "root-cause module keeps its non-causal charter");

// ---- 诊断引擎：起点时间只在 streak 起始打点、墙钟未记录时不伪造 ----
assert.match(diagSource, /pub fn note_wallclock\(&mut self, now_unix: u64\)/);
assert.match(
  diagSource,
  /if self\.streak == 0 && now_unix > 0 \{\s*\n\s*self\.hot_since_unix = Some\(now_unix\);\s*\n\s*\}/,
  "onset stamps only on the 0→1 streak transition with a real wall clock",
);
assert.match(
  diagSource,
  /self\.streak = 0;\s*\n\s*self\.hot_since_unix = None;/,
  "cooling a streak clears its onset stamp",
);

console.log("process-insight contract passed");
