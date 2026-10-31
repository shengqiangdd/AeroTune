import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";

const document = readFileSync("docs/UPDATER_RELEASE_BLOCKERS.md", "utf8");
const updater = readFileSync("src-tauri/src/updater.rs", "utf8");
const main = readFileSync("src-tauri/src/main.rs", "utf8");
const releasePolicy = readFileSync("scripts/updaterReleasePolicy.mjs", "utf8");
const readinessScript = readFileSync("scripts/check-updater-release-ready.mjs", "utf8");
const releaseWorkflow = readFileSync(".github/workflows/release-windows.yml", "utf8");
const ciWorkflow = readFileSync(".github/workflows/ci.yml", "utf8");

assert.match(document, /base Tauri config remains updater-disabled by default/);
assert.match(document, /main checkout has Git metadata and an `origin`/);
assert.doesNotMatch(document, /no `\.git` metadata or configured remote/);
for (const gate of [
  "UPDATE_FEED_RESPONSE_BOUNDED_READY",
  "UPDATE_PROXY_POLICY_READY",
  "UPDATE_BOUNDED_DOWNLOAD_READY",
  "GITHUB_RELEASE_HOST_POLICY_REVIEWED",
  "GITHUB_RELEASE_WINDOWS_ACCEPTED",
]) {
  assert.match(updater, new RegExp(`const ${gate}: bool = (?:true|false);`));
  assert.match(document, new RegExp(`\\b${gate}\\b`));
}
assert.match(updater, /fn github_release_update_readiness_error\(\) -> Option<String>/);
assert.match(updater, /if let Some\(reason\) = github_release_update_readiness_error\(\)/);
assert.match(main, /try_acquire_app_update_gate/);
assert.match(main, /LEGACY_UPDATE_ONLY_ARGUMENT/);
assert.match(main, /std::process::exit\(2\)/);
assert.doesNotMatch(updater, /run_update_only|run_embedded_task_action|AeroTune-SignedUpdater|signed-updater-task\.ps1|tauri_plugin_updater::Builder::new\(\)/);
assert.match(updater, /update\s*\.install\(artifact\.bytes\(\)\)/);
assert.match(releasePolicy, /githubUpdaterAcceptanceReady = false/);
assert.match(readinessScript, /signed release overlay and matching key from the protected GitHub release environment are required/);
assert.match(releaseWorkflow, /build:[\s\S]*?contents: read/);
assert.match(releaseWorkflow, /publish:[\s\S]*?contents: write/);
assert.match(releaseWorkflow, /actions\/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9/);
assert.match(releaseWorkflow, /actions\/download-artifact@9000827ccba6bdab643e8b6fd33ac0654aef8333/);
assert.match(ciWorkflow, /contents: read/);
assert.doesNotMatch(ciWorkflow, /contents: write/);
assert.match(document, /does not provide AeroTune's prior protected transaction journal or crash-safe rollback/);
assert.match(document, /local install record: the user-approved NSIS test installer upgraded the existing Program Files installation from 1\.5\.9 to 1\.5\.10-local\.1/);
assert.doesNotMatch(document, /exact scheduled task `AeroTune-SignedUpdater` is not registered/);

const strictGate = spawnSync(process.execPath, ["scripts/check-updater-release-ready.mjs", "--required"], {
  encoding: "utf8",
  env: Object.fromEntries(Object.entries(process.env).filter(([name]) =>
    !["AEROTUNE_UPDATER_CONFIG", "AEROTUNE_UPDATE_ENDPOINT", "AEROTUNE_UPDATE_PUBLIC_KEY", "TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PATH", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD"].includes(name),
  )),
});
assert.equal(strictGate.status, 1, "required release readiness must fail without a release overlay");
assert.match(strictGate.stderr, /AEROTUNE_UPDATER_CONFIG is required/);
assert.doesNotMatch(strictGate.stderr, /TAURI_SIGNING_PRIVATE_KEY[^\n]*=/);

console.log("PASS base updater config stays disabled; early-access risks are documented and publication is isolated from the read-only build job");
