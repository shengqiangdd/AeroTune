import fs from "node:fs";

const rust = fs.readFileSync("src-tauri/src/reliability.rs", "utf8");
const api = fs.readFileSync("src/lib/api.ts", "utf8");
const ui = fs.readFileSync("src/components/ReliabilityTimeline.tsx", "utf8");
const powershell = rust.split('let script = r#"')[1]?.split('"#')[0] ?? "";
const checks = [
  {
    ok: /validate_reliability_window[\s\S]*\(1\.\.=90\)/.test(rust)
      && /get_reliability_events\(days: u16\)/.test(rust)
      && /value=\{7\}[\s\S]*value=\{30\}[\s\S]*value=\{90\}/.test(ui),
    description: "backend and UI share a validated 7/30/90-day query window",
  },
  {
    ok: /Query-Channel[\s\S]*Get-WinEvent -ListLog[\s\S]*limit_reached/.test(rust)
      && /EXPECTED_SOURCES[\s\S]*application[\s\S]*system[\s\S]*updates/.test(rust),
    description: "each Windows event channel reports independent availability and truncation",
  },
  {
    ok: /candidate_count[\s\S]*omitted_estimate[\s\S]*invalid_rows/.test(rust)
      && /返回 \{snapshot\.events\.length\} \/ 200 条/.test(ui)
      && /显示更多/.test(ui),
    description: "capped event samples expose omissions and allow paging through returned rows",
  },
  {
    ok: /\("application", 1000\)[\s\S]*\("application", 1002\)[\s\S]*\("system", 41\)[\s\S]*\("system", 1074\)[\s\S]*\("updates", 19\)[\s\S]*\("updates", 20\)/.test(rust)
      && !/param1/i.test(powershell),
    description: "only supported event IDs and allowlisted update title fields are surfaced",
  },
  {
    ok: /reliabilityEvents: \(days: number\) => invoke<ReliabilitySnapshot>\("get_reliability_events", \{ days \}\)/.test(api),
    description: "typed frontend API passes the selected range and receives source metadata",
  },
  {
    ok: /unavailableSources\.length > 0[\s\S]*结果不完整/.test(ui)
      && /刷新失败，保留上次成功结果/.test(ui)
      && /lastFetchedAt/.test(ui),
    description: "partial sources and stale snapshots are clearly labeled",
  },
  {
    ok: /snapshot_captured_at_utc[\s\S]*api\.saveTextExport\(filename,\s*JSON\.stringify\(report,\s*null,\s*2\)\)/.test(ui),
    description: "user-triggered JSON export includes the captured snapshot metadata",
  },
  {
    ok: /buildRecurringAppGroups[\s\S]*filterReliabilityEvents[\s\S]*buildAppIssueDayBuckets/.test(ui)
      && /aria-pressed=\{appKey === app\.key\}/.test(ui)
      && /本地时区[\s\S]*事件时间相邻不表示存在因果关系/.test(ui)
      && /snapshotQuality\?\.incomplete/.test(ui),
    description: "per-app filtering and timezone-labeled trends preserve incomplete-snapshot and non-causality caveats",
  },
];

let failed = 0;
for (const check of checks) {
  console.log(`${check.ok ? "PASS" : "FAIL"} ${check.description}`);
  if (!check.ok) failed++;
}
if (failed) {
  console.error(`Reliability contract failed: ${failed}/${checks.length}`);
  process.exitCode = 1;
} else {
  console.log(`Reliability contract passed: ${checks.length} checks`);
}
