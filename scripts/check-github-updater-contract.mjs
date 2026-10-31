import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const updater = readFileSync("src-tauri/src/updater.rs", "utf8");
const main = readFileSync("src-tauri/src/main.rs", "utf8");
const lib = readFileSync("src-tauri/src/lib.rs", "utf8");
const capabilities = JSON.parse(readFileSync("src-tauri/capabilities/default.json", "utf8"));
const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const overlay = JSON.parse(readFileSync("src-tauri/tauri.updater.example.json", "utf8"));

assert.match(main, /LEGACY_UPDATE_ONLY_ARGUMENT: &str = "--aerotune-update-only"/);
assert.match(main, /args_os\(\)[\s\S]*?argument == LEGACY_UPDATE_ONLY_ARGUMENT[\s\S]*?std::process::exit\(2\)/);
assert.doesNotMatch(lib, /UPDATE_ONLY_ARGUMENT|run_update_only/);
assert.doesNotMatch(updater, /run_update_only|AeroTune-SignedUpdater|signed-updater-task\.ps1|tauri_plugin_updater::Builder::new\(\)|crate::update_recovery/);
assert.equal(
  [...lib.matchAll(/tauri_plugin_updater::Builder::new\(\)\.build\(\)/g)].length,
  1,
  "the normal app should register the Rust updater plugin exactly once",
);
assert.equal(capabilities.permissions.some((permission) => permission.startsWith("updater:")), false);
assert.equal(config.plugins.updater.requireSignedVersion, true);
assert.equal(config.plugins.updater.allowDowngrades, false);
assert.equal(config.plugins.updater.windows.installMode, "passive");
assert.equal(overlay.plugins.updater.requireSignedVersion, true);
assert.equal(overlay.plugins.updater.allowDowngrades, false);
assert.equal(overlay.plugins.updater.windows.installMode, "passive");

for (const gate of [
  "UPDATE_FEED_RESPONSE_BOUNDED_READY",
  "UPDATE_PROXY_POLICY_READY",
  "UPDATE_BOUNDED_DOWNLOAD_READY",
  "GITHUB_RELEASE_HOST_POLICY_REVIEWED",
  "GITHUB_RELEASE_WINDOWS_ACCEPTED",
]) {
  assert.match(updater, new RegExp(`const ${gate}: bool = (?:true|false);`));
}
const requestStart = updater.indexOf("pub async fn request_signed_update(");
const requestEnd = updater.indexOf("#[cfg(not(windows))]", requestStart);
assert.ok(requestStart >= 0 && requestEnd > requestStart, "the Windows update IPC command must be explicit");
const installCommand = updater.slice(requestStart, requestEnd);
const verifiedDownload = installCommand.indexOf("download_and_verify_github_update(&update).await?");
const install = installCommand.indexOf("update\n        .install(artifact.bytes())");
assert.ok(verifiedDownload >= 0 && install > verifiedDownload, "only the bounded, verified GitHub payload may reach standard Tauri install");
assert.match(installCommand, /if update\.version != expected_version/);
assert.match(installCommand, /artifact\.matches_target_version\(&update\.version\)/);
assert.doesNotMatch(installCommand, /run_update_only|scheduled task|run_embedded_task_action/);

console.log("PASS standard GitHub updater uses explicit IPC; stale elevated-task arguments fail closed");
