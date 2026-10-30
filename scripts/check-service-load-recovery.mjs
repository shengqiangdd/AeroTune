import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const ui = readFileSync("src/pages/Services.tsx", "utf8");
const load = ui.match(/const load = useCallback\(async \(notifyFailure = true\) => \{[\s\S]*?\n  \}, \[\]\);/)?.[0];
const change = ui.match(/const change = async \(s: SvcInfo, mode: string\) => \{[\s\S]*?\n  \};/)?.[0];
const selector = ui.match(/<select[\s\S]*?onChange=\{\(e\) => change\(s, e\.target\.value\)\}[\s\S]*?\n\s*>/)?.[0];
const exportAction = ui.match(/const exportCsv = async \(\) => \{[\s\S]*?\n  \};/)?.[0];

assert.ok(load, "missing service-list loader");
assert.match(load, /loadingRef\.current \|\| busyRef\.current/);
assert.match(load, /const freshList = await api\.listServices\(\)[\s\S]*setList\(freshList\)[\s\S]*loadFailedRef\.current = false[\s\S]*setLoadFailed\(false\)/);
assert.match(load, /catch \(e\)[\s\S]*loadFailedRef\.current = true[\s\S]*setLoadFailed\(true\)/);
assert.doesNotMatch(load, /catch[\s\S]*setList\(\[\]\)/, "a failed read must preserve, not erase or mislabel, the last good list");

assert.ok(change, "missing service startup mutation handler");
assert.match(change, /busyRef\.current \|\| loadingRef\.current \|\| loadFailedRef\.current/);
assert.match(change, /finally[\s\S]*busyRef\.current = false[\s\S]*await load\(!mutationFailed\)/);
assert.ok(selector);
assert.match(selector, /disabled=\{loading \|\| loadFailed \|\| busyService !== null \|\| !s\.modifiable\}/);
assert.match(ui, /onClick=\{\(\) => void load\(\)\} disabled=\{loading \|\| busyService !== null\}[\s\S]*重试/);

assert.ok(exportAction);
assert.match(exportAction, /loadingRef\.current \|\| loadFailedRef\.current/);
assert.match(ui, /disabled=\{exporting \|\| loading \|\| loadFailed \|\| filtered\.length === 0\}/);
assert.match(ui, /loadFailed && \([\s\S]*role="alert"[\s\S]*服务修改与 CSV 导出已暂停[\s\S]*重试/);
assert.match(ui, /filtered\.length === 0 && \([\s\S]*loadFailed[\s\S]*正在读取服务列表/);
assert.match(ui, /loading && list\.length > 0[\s\S]*role="status"[\s\S]*当前快照暂不可修改/);

console.log("PASS service list failures preserve and label stale snapshots, disable writes/exports, and recover by retry");
