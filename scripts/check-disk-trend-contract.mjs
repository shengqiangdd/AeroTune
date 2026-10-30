// SMART 磁盘健康趋势的源码合同：钉住"只读观察、有限样本、不做寿命预测"
// 三条红线与低频存储门槛、容量上限、诚实空态。
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (file) =>
  readFileSync(new URL(`../${file}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");

const apiSource = read("src/lib/api.ts");
const cleanerSource = read("src/pages/Cleaner.tsx");
const trendSource = read("src-tauri/src/disk_trend.rs");
const libSource = read("src-tauri/src/lib.rs");

// ---- IPC 契约 ----
assert.match(apiSource, /getDiskHealthTrends: \(\) => invoke<DiskTrendReport>\("get_disk_health_trends"\)/);
assert.match(apiSource, /dismissDiskTrend: \(name: string\) => invoke<boolean>\("dismiss_disk_trend", \{ name \}\)/);
assert.match(libSource, /disk_trend::get_disk_health_trends,/);
assert.match(libSource, /disk_trend::dismiss_disk_trend,/);
assert.match(apiSource, /interface DiskTrend \{[\s\S]*?tone: string;[\s\S]*?summary: string;/);

// ---- 红线：只读观察、有限样本、不预测寿命 ----
assert.match(trendSource, /不做寿命预测/, "trend module keeps its no-forecast charter");
assert.match(trendSource, /字段可用性取决于硬件与驱动/, "trend module keeps its hardware-dependency disclaimer");
assert.match(trendSource, /pub const SAMPLE_MIN_INTERVAL_SECS: u64 = 3600;/, "storage is rate-limited to hourly");
assert.match(trendSource, /pub const SAMPLES_PER_DISK_CAP: usize = 120;/, "per-disk sample count is bounded");
assert.match(trendSource, /const STALE_DISK_SECS: u64 = 30 \* 24 \* 3600;/, "unseen disks are pruned after 30 days");
assert.match(trendSource, /crate::util::atomic_write\(&self\.path, &bytes\)/, "trend store persists via shared atomic write");
assert.match(
  trendSource,
  /crate::disk_analyzer::disk_health\(\)\.await/,
  "trend command reuses the existing disk_health read path (no new system query)",
);
assert.match(
  trendSource,
  /trend\.tone = "error-rise"/,
  "rising uncorrected errors are surfaced as the highest-priority tone",
);
assert.match(trendSource, /仅一次观测，趋势需至少两次不同时间的样本/, "single samples honestly report insufficient evidence");
assert.match(trendSource, /两次观测间隔过近，不足以形成趋势/, "close samples honestly report insufficient evidence");
assert.match(trendSource, /建议关注该磁盘并保持备份/, "error-rise advice prioritizes backup, not action");

// ---- 前端：趋势行证据 + 诚实页脚 ----
assert.match(cleanerSource, /getDiskHealthTrends\(\)/);
assert.match(cleanerSource, /不构成寿命预测；SMART 字段可用性取决于硬件与驱动/, "cleaner footer keeps the no-forecast wording");
assert.match(cleanerSource, /出现新错误请优先备份重要数据/, "cleaner footer routes error-rise to backup first");
assert.match(
  cleanerSource,
  /趋势基于本机 AeroTune 运行期间的有限样本（至少 1 小时间隔）/,
  "cleaner footer scopes observations to app runtime",
);
assert.match(
  cleanerSource,
  /磨损速率 \{trend\.wear_rate_per_1000h\.toFixed\(2\)\}%\/千小时/,
  "wear rate is displayed in physical units",
);

// ---- 隐私清单 ----
assert.match(
  read("docs/DATA_PRIVACY_INVENTORY.md"),
  /disk-health-history\.json/,
  "privacy inventory documents the disk-health-history store",
);

console.log("disk-trend contract passed: 19 checks");
