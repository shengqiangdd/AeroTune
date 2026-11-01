import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { clearLocalObservationHistory, inspectLocalObservationHistory, LOCAL_OBSERVATION_HISTORY_TARGETS } from "../src/lib/localObservationHistory.js";

const expected = [
  ["network", "aerotune.network-quality-history.v1"],
  ["disk", "aerotune.disk-space-history.v1"],
  ["process", "aerotune.process-history.v1"],
  ["battery", "aerotune.power-insights.samples.v1"],
  ["experiments", "aerotune.optimization-lab.history.v1"],
];
assert.deepEqual(LOCAL_OBSERVATION_HISTORY_TARGETS.map(({ id, key }) => [id, key]), expected);
assert.ok(LOCAL_OBSERVATION_HISTORY_TARGETS.every(({ detail }) => typeof detail === "string" && detail.length > 0));

function fakeStorage({ failKeys = new Set(), presentKeys = new Set(), failReads = false, lengthOverride } = {}) {
  return {
    removed: [],
    valueReads: [],
    keyReads: [],
    get length() {
      if (failReads) throw new Error("simulated storage inspection failure");
      return lengthOverride ?? presentKeys.size;
    },
    key(index) {
      this.keyReads.push(index);
      if (failReads) throw new Error("simulated storage inspection failure");
      return [...presentKeys][index] ?? null;
    },
    getItem(key) {
      this.valueReads.push(key);
      return presentKeys.has(key) ? "sensitive-history-content" : null;
    },
    removeItem(key) {
      this.removed.push(key);
      if (failKeys.has(key)) throw new Error("simulated storage failure");
    },
  };
}

const inventoryStorage = fakeStorage({ presentKeys: new Set([expected[0][1], expected[3][1], "unrelated-user-data-key"]) });
const observedInventory = inspectLocalObservationHistory({ localStorage: inventoryStorage });
assert.deepEqual(observedInventory.map(({ id, state }) => [id, state]), [
  ["network", "present"], ["disk", "absent"], ["process", "absent"], ["battery", "present"], ["experiments", "absent"],
]);
assert.deepEqual(Object.keys(observedInventory[0]).sort(), ["id", "label", "state"], "inventory must never return stored history contents");
assert.deepEqual(inventoryStorage.valueReads, [], "presence inventory must inspect key names without reading stored values");
assert.ok(inspectLocalObservationHistory({ localStorage: null }).every(({ state }) => state === "unknown"));
assert.ok(inspectLocalObservationHistory({ localStorage: fakeStorage({ failReads: true }) }).every(({ state }) => state === "unknown"));
const overLimitStorage = fakeStorage({ lengthOverride: 513 });
assert.ok(inspectLocalObservationHistory({ localStorage: overLimitStorage }).every(({ state }) => state === "unknown"), "over-limit storage must fail closed rather than label unseen keys absent");
assert.deepEqual(overLimitStorage.keyReads, [], "over-limit inspection must stop before enumerating keys");

const selectedStorage = fakeStorage();
const selected = clearLocalObservationHistory(["disk", "battery"], { localStorage: selectedStorage });
assert.deepEqual(selected.map(({ id }) => id), ["disk", "battery"]);
assert.deepEqual(selectedStorage.removed, [expected[1][1], expected[3][1]], "per-category removal must not touch unselected keys");

const defaultNoopStorage = fakeStorage();
assert.deepEqual(clearLocalObservationHistory(undefined, { localStorage: defaultNoopStorage }), [], "omitting IDs must never clear every history category");
assert.deepEqual(defaultNoopStorage.removed, []);
const allTargetIds = LOCAL_OBSERVATION_HISTORY_TARGETS.map(({ id }) => id);
const local = fakeStorage();
const complete = clearLocalObservationHistory(allTargetIds, { localStorage: local });
assert.equal(complete.length, expected.length);
assert.ok(complete.every((result) => result.ok), "all five allowlisted key removals should report success");
assert.deepEqual(local.removed, expected.map(([, key]) => key));

const failingKey = expected[2][1];
const partialStorage = fakeStorage({ failKeys: new Set([failingKey]) });
const partial = clearLocalObservationHistory(allTargetIds, { localStorage: partialStorage });
assert.equal(partial.length, expected.length, "one failure must not prevent remaining key attempts");
assert.deepEqual(partial.filter((result) => !result.ok).map(({ id }) => id), ["process"]);
assert.equal(partialStorage.removed.length, expected.length);
const retry = clearLocalObservationHistory(["process"], { localStorage: fakeStorage() });
assert.deepEqual(retry.map(({ id, ok }) => [id, ok]), [["process", true]], "retry can be restricted to failed IDs");

const unavailable = clearLocalObservationHistory(allTargetIds, { localStorage: null });
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
const targetDetails = Object.fromEntries(LOCAL_OBSERVATION_HISTORY_TARGETS.map(({ id, detail }) => [id, detail]));
const networkHistory = readFileSync("src/lib/networkQualityHistory.js", "utf8");
const diskHistory = readFileSync("src/lib/diskHistory.js", "utf8");
const processHistory = readFileSync("src/components/ProcessHistoryCard.tsx", "utf8");
const batteryHistory = readFileSync("src/components/PowerInsights.tsx", "utf8");
const experimentHistory = readFileSync("src/components/OptimizationLab.tsx", "utf8");
assert.match(networkHistory, /NETWORK_QUALITY_HISTORY_MAX_AGE_MS = 24 \* 60 \* 60 \* 1000/);
assert.match(networkHistory, /NETWORK_QUALITY_HISTORY_MAX_ENTRIES = 300/);
assert.match(targetDetails.network, /300 条[\s\S]*24 小时/);
assert.match(diskHistory, /DISK_HISTORY_RETENTION_MS = 14 \* 24 \* 60 \* 60 \* 1000/);
assert.match(targetDetails.disk, /保留 14 天/);
assert.match(processHistory, /MIN_SAMPLE_INTERVAL_MS = 5 \* 60_000/);
assert.match(processHistory, /MAX_HISTORY_AGE_MS = 7 \* 24 \* 60 \* 60 \* 1_000/);
assert.match(processHistory, /MAX_NAMES_PER_SAMPLE = 20/);
assert.match(targetDetails.process, /5 分钟[\s\S]*20 个名称[\s\S]*7 天/);
assert.match(batteryHistory, /RETENTION_MS = 48 \* 60 \* 60 \* 1000/);
assert.match(targetDetails.battery, /48 小时/);
assert.match(experimentHistory, /const HISTORY_LIMIT = 8/);
assert.match(targetDetails.experiments, /最多 8 条/);
assert.doesNotMatch(helper, /quickNavigation\.recent|optimization-lab\.draft|\.clear\(/, "quick navigation, recoverable draft, and broad Storage.clear stay outside the allowlist");
assert.match(helper, /MAX_LOCAL_STORAGE_KEYS_TO_INSPECT = 512[\s\S]*length > MAX_LOCAL_STORAGE_KEYS_TO_INSPECT/);
assert.match(helper, /storage\.key\(index\)[\s\S]*allowedKeys\.has\(key\)/);
assert.doesNotMatch(helper, /\.getItem\(/, "presence inventory must not read stored history values");
assert.match(helper, /clearLocalObservationHistory\(ids = \[\]/, "an omitted target list must be a safe no-op");
assert.match(settings, /selectedLocalHistoryIds\.includes\(target\.id\)[\s\S]*clearLocalObservationHistory\(targetIds\)/);
assert.match(settings, /aria-labelledby=\{`local-history-\$\{target\.id\}-label`\}[\s\S]*aria-describedby=\{`local-history-\$\{target\.id\}-detail local-history-\$\{target\.id\}-state`\}/);
assert.match(settings, /id=\{`local-history-\$\{target\.id\}-detail`\}[\s\S]*id=\{`local-history-\$\{target\.id\}-state`\}/);
assert.match(settings, /默认不选任何类别[\s\S]*只检查指定 WebView localStorage 键是否存在，不读取或展示历史内容/);
assert.match(settings, /<fieldset[\s\S]*选择要清除的类别[\s\S]*type="checkbox"/);
assert.match(settings, /快速导航记录和可恢复的实验会话草稿也会保留/);
assert.match(helper, /清除此项会恢复默认关闭/);
assert.match(settings, /role="alert"[\s\S]*失败项可能仍保留[\s\S]*重试失败项/);
assert.match(settings, /role="status"[\s\S]*API 未报告失败[\s\S]*不等同于底层介质安全擦除/);
assert.match(settings, /目前没有“清空全部本机数据”功能/);
assert.match(inventory, /per-category, user-confirmed WebView history removal[\s\S]*not secure media erasure/);
assert.match(readiness, /per-category, user-confirmed removal for five named WebView history keys[\s\S]*not a full data wipe/);

console.log("Local history lifecycle contract passed: presence-only inventory, exact-key selection, partial failure/retry, exclusions, and honest erasure wording");
