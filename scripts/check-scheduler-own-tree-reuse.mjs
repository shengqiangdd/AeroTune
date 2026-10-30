import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const auto = readFileSync(new URL("../src-tauri/src/auto.rs", import.meta.url), "utf8");
const processes = readFileSync(new URL("../src-tauri/src/processes.rs", import.meta.url), "utf8");
const start = auto.indexOf("fn tick_shared(");
const end = auto.indexOf("\nfn ", start + 1);
assert.ok(start >= 0 && end > start, "shared scheduler tick should remain a bounded function");
const tick = auto.slice(start, end);

assert.match(auto, /const SCHEDULER_RESCAN_INTERVAL: Duration = Duration::from_secs\(5\)/);
assert.match(tick, /if should_refresh_own_process_tree\([\s\S]*?state\.foreground_own_tree = crate::processes::collect_own_process_tree_pids\(sys\);/);
assert.match(tick, /state\.foreground_own_tree\.clear\(\);/, "unused ticks must not retain a stale tree snapshot");
const collectorCalls = tick.match(/collect_own_process_tree_pids\s*\(\s*sys\s*\)/g) ?? [];
assert.equal(collectorCalls.length, 1, "each scheduler tick collects the immutable process tree at most once");
assert.match(tick, /let own_tree = &state\.foreground_own_tree;/);
assert.match(tick, /scheduler_candidate_is_selectable\([\s\S]*?start_time,\s*own_tree,/);
assert.match(auto, /fn scheduler_filter_skips_own_webview_process_tree\(/, "the existing own-WebView exclusion behavior remains tested");
assert.match(auto, /fn own_tree_refresh_gate_tracks_scheduler_scan_and_foreground_boost_consumers\(/, "refresh cadence is covered for both tree consumers");
assert.match(auto, /fn benchmark_idle_own_tree_refresh_cadence\(/, "a manual idle-cadence benchmark is available");
assert.match(processes, /fn benchmark_own_process_tree_reuse\(/, "a manual same-snapshot timing benchmark is available");
assert.match(processes, /assert_eq!\(\s*first, second,\s*"repeated collection from the same snapshot is deterministic"\s*\)/);

console.log("Scheduler own-tree reuse passed: consumer-gated refresh, stale-snapshot clearing, and benchmark guards");
