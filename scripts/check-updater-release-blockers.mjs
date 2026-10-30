import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const document = readFileSync("docs/UPDATER_RELEASE_BLOCKERS.md", "utf8");
const runtime = readFileSync("src-tauri/src/updater.rs", "utf8");
const recovery = readFileSync("src-tauri/src/update_recovery.rs", "utf8");
const transaction = readFileSync("src-tauri/src/updater_transaction.rs", "utf8");
const task = readFileSync("scripts/signed-updater-task.ps1", "utf8");
const policy = readFileSync("scripts/updaterReleasePolicy.mjs", "utf8");

assert.match(document, /Status: updates remain disabled/);
assert.match(document, /No `AEROTUNE_UPDATER_CONFIG`/);
assert.match(document, /exact scheduled task `AeroTune-SignedUpdater` is not registered/);
assert.match(document, /Current shell is not elevated/);
assert.match(document, /no `\.git` metadata or configured remote/);
assert.match(document, /Unsigned historic NSIS installers are not representative/);
assert.match(document, /independent protected recovery host\/supervisor outside `Program Files\\AeroTune`/);
assert.match(runtime, /const UPDATE_PIPELINE_READY: bool = false/);
assert.match(runtime, /const UPDATE_DNS_DESTINATION_POLICY_READY: bool = false/);
assert.match(runtime, /const UPDATE_PROXY_POLICY_READY: bool = false/);
assert.match(runtime, /const UPDATE_BOUNDED_DOWNLOAD_READY: bool = false/);
assert.match(runtime, /const TASK_DEFINITION_ATTESTATION_READY: bool = false/);
assert.match(recovery, /Updates therefore remain disabled/);
assert.match(transaction, /pub\(crate\) fn plan_recovery/);
assert.match(transaction, /P::InstallIntent =>/);
assert.match(transaction, /P::RollbackIntent =>/);
assert.match(transaction, /P::Committed \| P::RolledBack => blocked\(\)/);
assert.match(task, /\$RegistrationEnabled = \$false/);
assert.match(policy, /if \(!pipelineReady\)/);
assert.match(policy, /if \(!boundedDownloadReady\)/);
assert.match(document, /Local tests and successful compilation are not substitutes/);

const strictGate = spawnSync(process.execPath, ["scripts/check-updater-release-ready.mjs", "--required"], {
  encoding: "utf8",
  env: Object.fromEntries(Object.entries(process.env).filter(([name]) =>
    !["AEROTUNE_UPDATER_CONFIG", "AEROTUNE_UPDATE_ENDPOINT", "AEROTUNE_UPDATE_PUBLIC_KEY", "TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PATH"].includes(name),
  )),
});
assert.equal(strictGate.status, 1, "required release readiness must fail without a release overlay");
assert.match(strictGate.stderr, /AEROTUNE_UPDATER_CONFIG is required/);
assert.doesNotMatch(strictGate.stderr, /TAURI_SIGNING_PRIVATE_KEY[^\\n]*=/);

console.log("PASS updater release blockers remain evidence-based, strict release mode fails closed, and all hard-off gates are preserved");
