import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(path, "utf8");
const settings = read("src/pages/Settings.tsx");
const sectionStart = settings.indexOf("查看数据类别、保留周期与分享范围");
assert.notEqual(sectionStart, -1, "Settings must provide an expandable data-retention disclosure");
const disclosure = settings.slice(sectionStart, settings.indexOf("</details>", sectionStart));
assert.ok(disclosure.length > 0, "the disclosure details panel must close correctly");
assert.match(settings, /本页内容依据当前源码，尚待产品\/法律审阅，不是正式隐私声明/);

for (const phrase of [
  "最多保留 5 个页面 ID",
  "最多 300 条且不超过 24 小时",
  "可见探测来源地址",
  "保留 14 天",
  "每 5 分钟最多采样一次、每次最多 20 个程序名及其 CPU/内存汇总、保留 7 天",
  "保留 48 小时",
  "最多保留 8 条汇总记录",
  "2 小时后过期",
  "变更日志没有自动淘汰",
  "计划备份保留至移除",
  "14 天以前的文件",
  "不会自动上传",
  "尚未通过一次性 Windows VM 验证",
]) assert.ok(disclosure.includes(phrase), `Settings disclosure must state: ${phrase}`);

assert.match(read("src/lib/quickNavigation.js"), /MAX_RECENT_PAGES = 5/);
assert.match(read("src/lib/networkQualityHistory.js"), /NETWORK_QUALITY_HISTORY_MAX_AGE_MS = 24 \* 60 \* 60 \* 1000/);
assert.match(read("src/lib/networkQualityHistory.js"), /NETWORK_QUALITY_HISTORY_MAX_ENTRIES = 300/);
assert.match(read("src/lib/diskHistory.js"), /DISK_HISTORY_RETENTION_MS = 14 \* 24 \* 60 \* 60 \* 1000/);
assert.match(read("src/components/ProcessHistoryCard.tsx"), /MIN_SAMPLE_INTERVAL_MS = 5 \* 60_000[\s\S]*MAX_HISTORY_AGE_MS = 7 \* 24 \* 60 \* 60 \* 1_000[\s\S]*MAX_NAMES_PER_SAMPLE = 20/);
assert.match(read("src/components/PowerInsights.tsx"), /const RETENTION_MS = 48 \* 60 \* 60 \* 1000[\s\S]*useState\(false\)/);
assert.match(read("src/components/OptimizationLab.tsx"), /const HISTORY_LIMIT = 8[\s\S]*const DRAFT_TTL_MS = 2 \* 60 \* 60 \* 1000[\s\S]*window\.sessionStorage\.setItem/);
assert.match(read("src-tauri/src/disk_trend.rs"), /SAMPLES_PER_DISK_CAP: usize = 120[\s\S]*STALE_DISK_SECS: u64 = 30 \* 24 \* 3600/);
assert.match(read("src-tauri/src/scene_shadow.rs"), /STALE_ENTRY_SECS: u64 = 30 \* 24 \* 3600/);
assert.match(read("src-tauri/src/residency.rs"), /STALE_ENTRY_SECS: u64 = 30 \* 24 \* 3600/);
assert.match(read("src-tauri/src/lib.rs"), /LOG_RETENTION: std::time::Duration = std::time::Duration::from_secs\(14 \* 24 \* 60 \* 60\)/);

console.log("Privacy disclosure contract passed: visible controls, source-aligned local retention, sharing, and uninstall caveats");
