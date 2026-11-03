import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import {
  CHANGE_PAGE_SIZE,
  paginateChanges,
} from "../src/lib/changeJournal.js";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

// ---------------------------------------------------------------------------
// 1. Slow external-process commands must not run on the Tauri main (UI) thread.
//    A synchronous #[tauri::command] runs on the main thread; `async fn` or `(async)` does not.
// ---------------------------------------------------------------------------
const MUST_NOT_BLOCK_UI = {
  "network.rs": ["list_adapters", "get_dns", "get_tcp_autotuning", "list_wifi_profiles", "list_nic_power", "list_tcp_latency", "ping_quality"],
  "power.rs": ["list_power_plans", "battery_report", "hibernate_info", "list_redundant_plans"],
  "cleaner.rs": ["compactos_status", "mem_compression_status", "remove_appx", "set_compactos", "set_mem_compression"],
  "software_updates.rs": ["check_winget_updates"],
  "admin.rs": ["get_system_info", "relaunch_as_admin"],
};
for (const [file, names] of Object.entries(MUST_NOT_BLOCK_UI)) {
  const source = read(`src-tauri/src/${file}`);
  for (const name of names) {
    const asyncAttribute = new RegExp(`#\\[tauri::command\\(async\\)\\]\\s*pub fn ${name}\\(`);
    const asyncFn = new RegExp(`#\\[tauri::command\\]\\s*pub async fn ${name}\\(`);
    assert.ok(
      asyncAttribute.test(source) || asyncFn.test(source),
      `${file}::${name} spawns external processes and must not run on the UI main thread (use #[tauri::command(async)] or async fn)`,
    );
  }
}

// Guard against regressions elsewhere: any *new* synchronous command that directly spawns a process
// must be listed above or explicitly acknowledged here (these are short, local, or already reviewed).
const ACKNOWLEDGED_SYNC_SPAWNERS = new Set([
  "open_path", "open_settings_page", // fire-and-forget Explorer launch, no wait
  "delete_oldest_shadow", "set_shadow_max", "recycle_delete",
  "flush_dns", "set_tcp_autotuning", "remove_plan", "set_hibernate",
]);
const SPAWN = /run_ps_with_timeout_result|run_ps_with_timeout|run_ps\(|run_checked_with_timeout|run_checked\(|run_output_checked_with_timeout|run_output_checked\(|util::run\(|run_with_timeout|Command::new|\.spawn\(/;
const offenders = [];
for (const file of readdirSync(new URL("../src-tauri/src/", import.meta.url)).filter((f) => f.endsWith(".rs"))) {
  const lines = read(`src-tauri/src/${file}`).split("\n");
  for (let i = 0; i < lines.length; i++) {
    const attribute = lines[i].match(/^\s*#\[tauri::command(\(async\))?\]/);
    if (!attribute) continue;
    const fnIndex = lines.findIndex((line, index) => index > i && index < i + 8 && /^\s*(pub(\([a-z]+\))?\s+)?(async\s+)?fn\s+\w+/.test(line));
    if (fnIndex < 0) continue;
    const header = lines[fnIndex].match(/^\s*(?:pub(?:\([a-z]+\))?\s+)?(async\s+)?fn\s+(\w+)/);
    if (header[1] || attribute[1]) continue; // already off the main thread
    let end = fnIndex;
    while (end < lines.length && lines[end] !== "}") end++;
    if (SPAWN.test(lines.slice(fnIndex, end + 1).join("\n")) && !ACKNOWLEDGED_SYNC_SPAWNERS.has(header[2])) {
      offenders.push(`${file}::${header[2]}`);
    }
  }
}
assert.deepEqual(offenders, [], `new synchronous commands that spawn processes run on the UI thread: ${offenders.join(", ")}`);

// ---------------------------------------------------------------------------
// 2. Elevated relaunch must wait for the previous instance; a refused ordinary launch focuses it.
// ---------------------------------------------------------------------------
const main = read("src-tauri/src/main.rs");
const admin = read("src-tauri/src/admin.rs");
const config = JSON.parse(read("src-tauri/tauri.conf.json"));

const mainArgument = main.match(/const RELAUNCH_ELEVATED_ARGUMENT: &str = "([^"]+)";/)?.[1];
const adminArgument = admin.match(/const RELAUNCH_ELEVATED_ARGUMENT: &str = "([^"]+)";/)?.[1];
assert.ok(mainArgument, "main.rs must define the relaunch argument");
assert.equal(adminArgument, mainArgument, "admin.rs and main.rs must agree on the relaunch argument");
assert.match(admin, /Start-Process -FilePath '\{quoted\}' -ArgumentList '\{RELAUNCH_ELEVATED_ARGUMENT\}' -Verb RunAs/);
assert.match(main, /RELAUNCH_GATE_WAIT: Duration = Duration::from_secs\(\d+\)/, "the relaunch wait must be bounded");
assert.match(main, /fn acquire_launch_gate\(\s*wait_for_previous_instance: bool/);
assert.match(main, /Err\(error\) if !wait_for_previous_instance \|\| Instant::now\(\) >= deadline/, "ordinary launches must still fail closed immediately");
assert.match(main, /if !relaunched \{\s*focus_running_instance\(\);/, "a refused ordinary launch must focus the running instance");
assert.equal(
  main.match(/const MAIN_WINDOW_TITLE: &str = "([^"]+)";/)?.[1],
  config.app.windows[0].title,
  "the focus title must match the configured main window title",
);
// The legacy hard-off path and the update gate must be unchanged by the relaunch support.
assert.match(main, /argument == LEGACY_UPDATE_ONLY_ARGUMENT[\s\S]*?std::process::exit\(2\)/);
assert.match(main, /try_acquire_app_update_gate/);
assert.doesNotMatch(main, /run_update_only/);

// ---------------------------------------------------------------------------
// 3. Change-journal display paging never alters the data used for undo or export.
// ---------------------------------------------------------------------------
const rows = Array.from({ length: 7147 }, (_, index) => ({ id: index + 1 }));
const first = paginateChanges(rows, 0);
assert.equal(first.items.length, CHANGE_PAGE_SIZE);
assert.equal(first.total, 7147);
assert.equal(first.pageCount, Math.ceil(7147 / CHANGE_PAGE_SIZE));
const last = paginateChanges(rows, 10_000);
assert.equal(last.page, last.pageCount - 1, "an out-of-range page is clamped");
assert.equal(last.end, 7147);
assert.equal(paginateChanges(rows, -5).page, 0);
assert.equal(paginateChanges(rows, Number.NaN).page, 0);
assert.deepEqual(paginateChanges([], 3), { page: 0, pageCount: 1, total: 0, start: 0, end: 0, items: [] });
assert.equal(paginateChanges(rows, 0, 0).items.length, CHANGE_PAGE_SIZE, "an invalid page size falls back to the default");
assert.equal(rows.length, 7147, "paging must not mutate its input");

const settings = read("src/pages/Settings.tsx");
assert.match(settings, /journalView\.items\.map\(\(c\) =>/, "only the current page is rendered");
assert.match(settings, /buildChangeJournalCsv\(filteredChanges, appVersion\)/, "exports use the full filtered list, not the page");
assert.match(settings, /reversibleChanges = changes\.filter/, "undo confirmation keeps using the full list");
assert.doesNotMatch(settings, /confirmUndoTargets\(journalView/, "undo must never be derived from the visible page");
assert.match(settings, /aria-label="变更日志上一页"[\s\S]*aria-label="变更日志下一页"/);

// ---------------------------------------------------------------------------
// 4. A failed Win32PrioritySeparation write must back off instead of rewriting the journal every tick.
// ---------------------------------------------------------------------------
const auto = read("src-tauri/src/auto.rs");
assert.match(auto, /const PRIORITY_WRITE_RETRY_BACKOFF: Duration = Duration::from_secs\(15 \* 60\);/);
assert.match(auto, /fn priority_write_retry_ready\(retry_after: &mut Option<Instant>, now: Instant\) -> bool/);
assert.match(
  auto,
  /if cur\.priority_separation != target\.priority_separation\s*&& priority_write_retry_ready\(&mut ad\.priority_write_retry_after, Instant::now\(\)\)/,
  "the adaptive priority write must be gated by its retry deadline",
);
assert.match(auto, /ad\.priority_write_retry_after = Some\(attempt \+ PRIORITY_WRITE_RETRY_BACKOFF\)/, "a failed write must arm the backoff");
assert.match(auto, /ad\.priority_write_retry_after = None;/, "a successful write must clear the backoff");
assert.match(auto, /failed_priority_write_is_not_retried_until_backoff_expires/);
// ---------------------------------------------------------------------------
// 5. No machine-specific Cargo target path is committed (it breaks other clones and the CI default).
// ---------------------------------------------------------------------------
const cargoConfig = read("src-tauri/.cargo/config.toml");
assert.doesNotMatch(cargoConfig, /^\s*target-dir\s*=/m, "do not commit a machine-specific target-dir");
assert.doesNotMatch(cargoConfig, /[A-Za-z]:[\\/]/, "do not commit drive-specific paths");
console.log("PASS slow commands stay off the UI thread; elevated relaunch waits for the old instance; journal paging is display-only; failed priority writes back off; no machine-specific Cargo path");
