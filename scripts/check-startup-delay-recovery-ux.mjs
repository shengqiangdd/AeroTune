import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { startupDelayRecoveryViewState } from "../src/lib/startupDelayRecoveryView.js";

const startupSource = await readFile("src/pages/Startup.tsx", "utf8");

assert.deepEqual(startupDelayRecoveryViewState("loading", null), {
  loading: true,
  refreshingSnapshot: false,
  error: false,
  showEmpty: false,
  showRecords: false,
  canRestore: false,
  hasSnapshot: false,
}, "initial load must not claim there are no recovery records");
assert.deepEqual(startupDelayRecoveryViewState("loading", []), {
  loading: true,
  refreshingSnapshot: true,
  error: false,
  showEmpty: false,
  showRecords: false,
  canRestore: false,
  hasSnapshot: true,
}, "refreshing a known-empty snapshot must not present it as current");
assert.deepEqual(startupDelayRecoveryViewState("error", null), {
  loading: false,
  refreshingSnapshot: false,
  error: true,
  showEmpty: false,
  showRecords: false,
  canRestore: false,
  hasSnapshot: false,
}, "failed initial read must stay distinct from a successful empty result");
assert.deepEqual(startupDelayRecoveryViewState("ready", null), {
  loading: false,
  refreshingSnapshot: false,
  error: false,
  showEmpty: false,
  showRecords: false,
  canRestore: false,
  hasSnapshot: false,
}, "restore remains disabled unless a successful inventory snapshot exists");
assert.deepEqual(startupDelayRecoveryViewState("error", [{ name: "App", task: "AeroTune\\App" }]), {
  loading: false,
  refreshingSnapshot: false,
  error: true,
  showEmpty: false,
  showRecords: true,
  canRestore: false,
  hasSnapshot: true,
}, "stale rows may remain visible but cannot be restored after a failed refresh");
assert.deepEqual(startupDelayRecoveryViewState("ready", []), {
  loading: false,
  refreshingSnapshot: false,
  error: false,
  showEmpty: true,
  showRecords: false,
  canRestore: true,
  hasSnapshot: true,
}, "only a successful empty read may show the empty state");
assert.deepEqual(startupDelayRecoveryViewState("ready", [{ name: "App", task: "AeroTune\\App" }]), {
  loading: false,
  refreshingSnapshot: false,
  error: false,
  showEmpty: false,
  showRecords: true,
  canRestore: true,
  hasSnapshot: true,
}, "successful records remain visible and actionable");

assert.match(startupSource, /const loadDelayedStartups = useCallback\(async \(\) =>/);
assert.match(startupSource, /setDelaysStatus\("loading"\)[\s\S]*?api\.listDelayedStartup\(\)[\s\S]*?setDelays\(result\)[\s\S]*?setDelaysStatus\("ready"\)/);
assert.match(startupSource, /catch \(error\)[\s\S]*?setDelaysStatus\("error"\)[\s\S]*?setDelaysError/);
assert.match(startupSource, /Promise\.allSettled\(\[[\s\S]*?api\.listStartup\(\)[\s\S]*?loadDelayedStartups\(\)/);
assert.match(startupSource, /onClick=\{\(\) => void loadDelayedStartups\(\)\}[\s\S]*?重试读取/);
assert.match(startupSource, /delayRecoveryView\.showEmpty[\s\S]*?暂无延迟启动恢复记录/);
assert.match(startupSource, /disabled=\{!delayRecoveryView\.canRestore \|\| restoringDelayTask !== null\}/);
assert.match(startupSource, /const restoreDelayedStartup = async[\s\S]*?if \(delaysStatus !== "ready" \|\| restoringDelayTask !== null\) return;/);
assert.match(startupSource, /await api\.undelayStartupItem\(delay\.name\)[\s\S]*?await load\(\)[\s\S]*?await loadTasks\(\)/);

console.log("Startup delayed-recovery UX passed: loading/empty/error/stale states, retry, and restore gating");
