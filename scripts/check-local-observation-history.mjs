import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { clearLocalObservationHistory, LOCAL_OBSERVATION_HISTORY_TARGETS } from "../src/lib/localObservationHistory.js";

const expected = [
  ["network", "aerotune.network-quality-history.v1"],
  ["disk", "aerotune.disk-space-history.v1"],
  ["process", "aerotune.process-history.v1"],
  ["battery", "aerotune.power-insights.samples.v1"],
  ["experiments", "aerotune.optimization-lab.history.v1"],
];
assert.deepEqual(LOCAL_OBSERVATION_HISTORY_TARGETS.map(({ id, key }) => [id, key]), expected);

function fakeStorage({ failKeys = new Set() } = {}) {
  return {
    removed: [],
    removeItem(key) {
      this.removed.push(key);
      if (failKeys.has(key)) throw new Error("simulated storage failure");
    },
  };
}

const local = fakeStorage();
const complete = clearLocalObservationHistory(undefined, { localStorage: local });
assert.equal(complete.length, expected.length);
assert.ok(complete.every((result) => result.ok), "all five allowlisted key removals should report success");
assert.deepEqual(local.removed, expected.map(([, key]) => key));

const failingKey = expected[2][1];
const partialStorage = fakeStorage({ failKeys: new Set([failingKey]) });
const partial = clearLocalObservationHistory(undefined, { localStorage: partialStorage });
assert.equal(partial.length, expected.length, "one failure must not prevent remaining key attempts");
assert.deepEqual(partial.filter((result) => !result.ok).map(({ id }) => id), ["process"]);
assert.equal(partialStorage.removed.length, expected.length);
const retry = clearLocalObservationHistory(["process"], { localStorage: fakeStorage() });
assert.deepEqual(retry.map(({ id, ok }) => [id, ok]), [["process", true]], "retry can be restricted to failed IDs");

const unavailable = clearLocalObservationHistory(undefined, { localStorage: null });
assert.equal(unavailable.length, expected.length);
assert.ok(unavailable.every((result) => !result.ok), "storage denial must remain visible, never claim success");
assert.deepEqual(clearLocalObservationHistory(["not-allowlisted"], { localStorage: local }), []);

const helper = readFileSync("src/lib/localObservationHistory.js", "utf8");
const settings = readFileSync("src/pages/Settings.tsx", "utf8");
const inventory = readFileSync("docs/DATA_PRIVACY_INVENTORY.md", "utf8");
const readiness = readFileSync("docs/RELEASE_READINESS.md", "utf8");
const keySources = [
  ["src/components/NetworkQualityHistory.tsx", /const STORAGE_KEY = "aerotune\.network-quality-history\.v1"/],
  ["src/components/DiskSpaceHistoryCard.tsx", /const STORAGE_KEY = "aerotune\.disk-space-history\.v1"/],
  ["src/components/ProcessHistoryCard.tsx", /const STORAGE_KEY = "aerotune\.process-history\.v1"/],
  ["src/components/PowerInsights.tsx", /const STORAGE_KEY = "aerotune\.power-insights\.samples\.v1"/],
  ["src/components/OptimizationLab.tsx", /const HISTORY_KEY = "aerotune\.optimization-lab\.history\.v1"/],
];
for (const [path, pattern] of keySources) assert.match(readFileSync(path, "utf8"), pattern, `${path} storage key must stay aligned`);
assert.doesNotMatch(helper, /quickNavigation\.recent|optimization-lab\.draft|\.clear\(/, "quick navigation, recoverable draft, and broad Storage.clear stay outside the allowlist");
assert.match(settings, /只尝试删除 AeroTune WebView localStorage 中以下[\s\S]*快速导航记录和可恢复的实验会话草稿也会保留/);
assert.match(settings, /清除进程历史会同时把其采样开关恢复为默认关闭/);
assert.match(settings, /role="alert"[\s\S]*失败项可能仍保留[\s\S]*重试失败项/);
assert.match(settings, /role="status"[\s\S]*API 未报告失败[\s\S]*不等同于底层介质安全擦除/);
assert.match(settings, /目前没有“清空全部本机数据”功能/);
assert.match(inventory, /limited, user-confirmed WebView history removal[\s\S]*not secure media erasure/);
assert.match(readiness, /limited, user-confirmed WebView history removal for five named keys[\s\S]*not a full data wipe/);

console.log("Local history removal contract passed: exact keys, partial failure/retry, exclusions, and honest erasure wording");
