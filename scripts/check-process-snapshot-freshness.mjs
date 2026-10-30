import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { canMutateProcessRows, nextProcessSnapshotStatus } from "../src/lib/processSnapshotStatus.js";

const ui = readFileSync(new URL("../src/pages/Processes.tsx", import.meta.url), "utf8");
const rust = readFileSync(new URL("../src-tauri/src/processes.rs", import.meta.url), "utf8");

const initial = { hasSnapshot: false, stale: false };
const initialFailure = nextProcessSnapshotStatus(initial, false);
assert.deepEqual(initialFailure, { hasSnapshot: false, stale: true });
assert.equal(canMutateProcessRows(initialFailure, false), false, "no successful snapshot must keep PID actions disabled");

const fresh = nextProcessSnapshotStatus(initialFailure, true);
assert.deepEqual(fresh, { hasSnapshot: true, stale: false }, "a successful retry must restore a fresh snapshot");
assert.equal(canMutateProcessRows(fresh, false), true, "fresh rows may enable process actions");
assert.equal(canMutateProcessRows(fresh, true), false, "actions stay disabled while a refresh is in flight");

const refreshFailure = nextProcessSnapshotStatus(fresh, false);
assert.deepEqual(refreshFailure, { hasSnapshot: true, stale: true }, "a later error must preserve the prior rows but mark them stale");
assert.equal(canMutateProcessRows(refreshFailure, false), false, "stale rows must not issue PID mutations");
assert.equal(nextProcessSnapshotStatus(refreshFailure, true).stale, false, "a later successful refresh clears stale state");

assert.match(ui, /catch \(e\) \{[\s\S]*?setSnapshotStatus\(\(current\) => nextProcessSnapshotStatus\(current, false\)\)[\s\S]*?toastErr\(e\)/);
assert.match(ui, /snapshotStatus\.stale && \([\s\S]*?role="alert"[\s\S]*?onClick=\{load\}[\s\S]*?重试刷新/);
assert.match(ui, /aria-label=\{rowMutationsDisabled \? `\$\{p\.name\}，PID \$\{p\.pid\}，优先级暂不可用[\s\S]*?disabled=\{rowMutationsDisabled\}/);
assert.match(ui, /aria-label=\{rowMutationsDisabled \? `\$\{p\.name\}，PID \$\{p\.pid\}，CPU 亲和性暂不可用[\s\S]*?disabled=\{rowMutationsDisabled\}/);
assert.match(ui, /disabled=\{rowMutationsDisabled\}[\s\S]*?恢复进程暂不可用/);
assert.match(ui, /disabled=\{rowMutationsDisabled\}[\s\S]*?aria-label=\{rowMutationsDisabled[\s\S]*?结束进程暂不可用/);
assert.match(ui, /title=\{rowMutationsDisabled \? `\$\{rowMutationDisabledReason\}/);
assert.match(ui, /const requireVerifiedActionIdentity = \(p: ProcInfo\)[\s\S]*?processMutationsDisabled[\s\S]*?formatWindowsFiletimeUtc\(creationToken\)/);
for (const action of ["kill", "prio", "resume", "affinity"]) {
  const actionStart = ui.indexOf(`const ${action} = async`);
  assert.notEqual(actionStart, -1, `${action} handler must exist`);
  const actionEnd = ui.indexOf("\n  };", actionStart);
  assert.notEqual(actionEnd, -1, `${action} handler must remain bounded`);
  assert.match(ui.slice(actionStart, actionEnd), /requireVerifiedActionIdentity\(p\)/, `${action} handler must fail closed for stale rows`);
}
assert.match(rust, /pub async fn list_processes\(\) -> Result<Vec<ProcInfo>, String>[\s\S]*?\.await[\s\S]*?map_err\(\|_\| "进程列表后台刷新失败，请重试。"\.to_string\(\)\)/);

console.log("Process snapshot freshness passed: stale/unverified rows are read-only; retry restores actions");
