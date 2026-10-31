import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import {
  hasConfiguredSigningKey,
  hasUpdaterPermission,
  isValidTauriPublicKey,
  readCapabilityFiles,
  readUpdaterReleaseConfigFile,
  updaterIsEnabled,
  validateUpdaterReleaseConfig,
} from "./updaterReleasePolicy.mjs";
import { frontendBuildEnvironment, hasSigningMaterial, tauriBundleSigningEnvironment } from "./build-frontend-sanitized.mjs";
import { createValidatedUpdaterOverlaySnapshot, removeValidatedUpdaterOverlaySnapshot } from "./updaterOverlaySnapshot.mjs";
import { updaterReleaseTestEnvironment } from "./updaterReleaseEnvironment.mjs";

const endpoint = "https://github.com/shengqiangdd/AeroTune/releases/latest/download/latest.json";
const publicKey = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE0OUVGNDc0NkZBNkU2RDcKUldUWDVxWnZkUFNlcERHV2cyYno4bjZBclFuQk13Z1FLMWdlVm9zb3JjYkx6Y3VuRCtLSitjWWU=";
const baseConfig = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const overlayTemplate = JSON.parse(readFileSync("src-tauri/tauri.updater.example.json", "utf8"));
const overlay = structuredClone(overlayTemplate);
overlay.plugins.updater.pubkey = publicKey;
const updater = readFileSync("src-tauri/src/updater.rs", "utf8");
const lib = readFileSync("src-tauri/src/lib.rs", "utf8");
const main = readFileSync("src-tauri/src/main.rs", "utf8");
const api = readFileSync("src/lib/api.ts", "utf8");
const settings = readFileSync("src/pages/Settings.tsx", "utf8");
const buildGuard = readFileSync("src-tauri/build.rs", "utf8");
const releasePolicy = readFileSync("scripts/updaterReleasePolicy.mjs", "utf8");
const readinessScript = readFileSync("scripts/check-updater-release-ready.mjs", "utf8");
const releaseBuilder = readFileSync("scripts/build-updater-release.mjs", "utf8");
const releaseWorkflow = readFileSync(".github/workflows/release-windows.yml", "utf8");
const ciWorkflow = readFileSync(".github/workflows/ci.yml", "utf8");
const manifestScript = readFileSync("scripts/create-github-release-manifest.mjs", "utf8");
const overlayScript = readFileSync("scripts/create-github-updater-overlay.mjs", "utf8");
const tagScript = readFileSync("scripts/validate-github-release-tag.mjs", "utf8");
const capabilities = readCapabilityFiles("src-tauri/capabilities");

assert.equal(baseConfig.productName, "AeroTune");
assert.equal(baseConfig.identifier, "com.aerotune.desktop");
assert.deepEqual(baseConfig.bundle.targets, ["nsis"]);
assert.notEqual(baseConfig.bundle.createUpdaterArtifacts, true);
assert.equal(baseConfig.bundle.windows.nsis.installMode, "perMachine");
assert.equal(baseConfig.plugins.updater.endpoints.length, 0);
assert.equal(baseConfig.plugins.updater.pubkey, "");
assert.equal(baseConfig.plugins.updater.requireSignedVersion, true);
assert.equal(baseConfig.plugins.updater.allowDowngrades, false);
assert.equal(baseConfig.plugins.updater.windows.installMode, "passive");
assert.equal(overlay.plugins.updater.endpoints[0], endpoint);
assert.equal(overlay.plugins.updater.windows.installMode, "passive");
assert.equal(updaterIsEnabled(baseConfig), false);
assert.equal(updaterIsEnabled(overlay), true);
assert.equal(isValidTauriPublicKey(publicKey), true);
assert.equal(capabilities.some((file) => hasUpdaterPermission(file.content)), false);
assert.equal(JSON.parse(readFileSync("package.json", "utf8")).dependencies["@tauri-apps/plugin-updater"], undefined);
assert.equal((lib.match(/tauri_plugin_updater::Builder::new\(\)\.build\(\)/g) ?? []).length, 1);
assert.match(lib, /\.invoke_handler\(tauri::generate_handler!/);
assert.match(main, /try_acquire_app_update_gate/);
assert.match(main, /LEGACY_UPDATE_ONLY_ARGUMENT/);
assert.match(main, /std::process::exit\(2\)/);
assert.doesNotMatch(main, /run_update_only/);
assert.match(api, /requestSignedUpdate: \(expectedVersion: string\)/);
assert.match(api, /invoke<void>\("request_signed_update", \{ expectedVersion \}\)/);
assert.match(settings, /window\.confirm\(/);
assert.match(settings, /GitHub Releases/);
assert.match(settings, /UAC/);
assert.match(settings, /requestSignedUpdate\(availableUpdate\.available_version\)/);
assert.doesNotMatch(settings, /受保护更新任务|ProgramData/);

const signingEnvironment = {
  TAURI_SIGNING_PRIVATE_KEY: "secret-test-material",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "secret-test-password",
  tauri_signing_private_key_path: "C:\\private\\key",
  AEROTUNE_UPDATER_CONFIG: "overlay.json",
  AEROTUNE_UPDATE_ENDPOINT: endpoint,
  AEROTUNE_UPDATE_PUBLIC_KEY: publicKey,
  TAURI_CONFIG: "{}",
  KEEP_FOR_TESTS: "yes",
};
const sanitized = frontendBuildEnvironment(signingEnvironment);
assert.equal(sanitized.TAURI_SIGNING_PRIVATE_KEY, undefined);
assert.equal(sanitized.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, undefined);
assert.equal(sanitized.tauri_signing_private_key_path, undefined);
assert.equal(sanitized.KEEP_FOR_TESTS, "yes");
assert.equal(hasSigningMaterial(signingEnvironment), true);
assert.equal(hasSigningMaterial({ TAURI_SIGNING_PRIVATE_KEY: " " }), false);
assert.deepEqual(tauriBundleSigningEnvironment({
  TAURI_SIGNING_PRIVATE_KEY: "secret-key",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "secret-password",
  KEEP_FOR_BUNDLE: "yes",
}), {
  TAURI_SIGNING_PRIVATE_KEY: "secret-key",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "secret-password",
  KEEP_FOR_BUNDLE: "yes",
});
assert.deepEqual(updaterReleaseTestEnvironment(signingEnvironment), { KEEP_FOR_TESTS: "yes" });
assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY: "encrypted-minisign-input" }), true);
assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY: "REPLACE_WITH_PRIVATE_KEY" }), false);
assert.equal(hasConfiguredSigningKey({}), false);

const validReleaseInputs = {
  privateKeyPresent: true,
  runtimeEndpoint: endpoint,
  runtimePublicKey: publicKey,
  githubUpdaterAcceptanceReady: true,
  capabilityFiles: capabilities,
};
assert.deepEqual(validateUpdaterReleaseConfig(baseConfig, overlay, validReleaseInputs).errors, []);
assert.ok(validateUpdaterReleaseConfig(baseConfig, overlay, {
  ...validReleaseInputs,
  githubUpdaterAcceptanceReady: false,
}).errors.some((error) => error.includes("remains fail-closed")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, overlay, {
  ...validReleaseInputs,
  privateKeyPresent: false,
}).errors.some((error) => error.includes("TAURI_SIGNING_PRIVATE_KEY")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, overlay, {
  ...validReleaseInputs,
  runtimeEndpoint: "https://attacker.example/latest.json",
}).errors.some((error) => error.includes("AEROTUNE_UPDATE_ENDPOINT")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  plugins: { updater: { ...overlay.plugins.updater, endpoints: ["https://github.com/attacker/AeroTune/releases/latest/download/latest.json"] } },
}, validReleaseInputs).errors.some((error) => error.includes("fixed AeroTune GitHub Releases")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  plugins: { updater: { ...overlay.plugins.updater, pubkey: "REPLACE_WITH_PUBLIC_KEY" } },
}, validReleaseInputs).errors.some((error) => error.includes("public key")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  plugins: { updater: { ...overlay.plugins.updater, allowDowngrades: true } },
}, validReleaseInputs).errors.some((error) => error.includes("allowDowngrades")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  plugins: { updater: { ...overlay.plugins.updater, requireSignedVersion: false } },
}, validReleaseInputs).errors.some((error) => error.includes("requireSignedVersion")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  plugins: { updater: { ...overlay.plugins.updater, dangerousAcceptInvalidCerts: true } },
}, validReleaseInputs).errors.some((error) => error.includes("dangerousAcceptInvalidCerts")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  plugins: { updater: { ...overlay.plugins.updater, windows: { installMode: "quiet", installerArgs: [] } } },
}, validReleaseInputs).errors.some((error) => error.includes("passive")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  bundle: { ...overlay.bundle, targets: ["msi"] },
}, validReleaseInputs).errors.some((error) => error.includes("only the NSIS")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, {
  ...overlay,
  app: { security: { capabilities: [{ identifier: "unsafe", permissions: ["updater:default"] }] } },
}, validReleaseInputs).errors.some((error) => error.includes("app.security.capabilities")));
assert.ok(validateUpdaterReleaseConfig(baseConfig, overlay, {
  ...validReleaseInputs,
  capabilityFiles: [{ path: "unsafe.json", content: '{"permissions":["updater:allow-check"]}' }],
}).errors.some((error) => error.includes("unsafe.json")));

const temporaryDirectory = mkdtempSync(join(tmpdir(), "aerotune-updater-contract-"));
try {
  const overlayPath = join(temporaryDirectory, "overlay.json");
  writeFileSync(overlayPath, JSON.stringify(overlay));
  assert.deepEqual(readUpdaterReleaseConfigFile(overlayPath), overlay);
  const snapshot = createValidatedUpdaterOverlaySnapshot(overlayPath);
  try {
    assert.deepEqual(readUpdaterReleaseConfigFile(snapshot.path), overlay);
  } finally {
    removeValidatedUpdaterOverlaySnapshot(snapshot);
  }
  assert.throws(() => readUpdaterReleaseConfigFile(temporaryDirectory), /regular, non-symlink file/);
  const oversizedPath = join(temporaryDirectory, "oversized.json");
  writeFileSync(oversizedPath, " ".repeat(64 * 1024 + 1));
  assert.throws(() => readUpdaterReleaseConfigFile(oversizedPath), /64 KiB/);
} finally {
  const resolvedDirectory = resolve(temporaryDirectory);
  assert.equal(dirname(resolvedDirectory), resolve(tmpdir()), "refusing to remove unexpected updater test directory");
  rmSync(resolvedDirectory, { recursive: true, force: true });
}

for (const gate of [
  "UPDATE_FEED_RESPONSE_BOUNDED_READY",
  "UPDATE_PROXY_POLICY_READY",
  "UPDATE_BOUNDED_DOWNLOAD_READY",
  "GITHUB_RELEASE_HOST_POLICY_REVIEWED",
  "GITHUB_RELEASE_WINDOWS_ACCEPTED",
]) {
  assert.match(updater, new RegExp(`const ${gate}: bool = (?:true|false);`));
}
assert.match(updater, /fn github_release_update_readiness_error\(\) -> Option<String>/);
assert.match(updater, /validate_github_release_asset_url\(&update\.download_url\)\?/);
assert.match(updater, /fn github_redirect_url_is_allowed\(/);
assert.match(updater, /fn github_release_asset_url_is_allowed\(/);
assert.match(updater, /const MAX_UPDATER_REDIRECT_HOPS: usize = 3/);
assert.match(updater, /PROPOSED_MAX_UPDATE_ARTIFACT_BYTES: usize = 64 \* 1024 \* 1024/);
assert.match(updater, /PROPOSED_MAX_UPDATE_EXPANDED_BYTES: u64 = 128 \* 1024 \* 1024/);
assert.match(updater, /collect_bounded_update_response\(response, PROPOSED_MAX_UPDATE_ARTIFACT_BYTES\)/);
assert.match(updater, /verify_update_signature\(update, bytes\)/);
assert.match(updater, /validate_update_archive_payload\(bytes, version\)/);
assert.match(updater, /download_and_verify_github_update\(&update\)\.await\?/);
assert.equal((updater.match(/\.install\(artifact\.bytes\(\)\)/g) ?? []).length, 1);
assert.doesNotMatch(updater, /download_and_install\(/);
assert.doesNotMatch(updater, /run_embedded_task_action|AeroTune-SignedUpdater/);
assert.match(updater, /\.configure_client\(configure_github_updater_client\)/);
assert.match(updater, /github_updater_redirect_is_safe/);
assert.match(updater, /update\.version != expected_version/);
assert.match(updater, /artifact\.matches_target_version\(&update\.version\)/);
assert.match(buildGuard, /the ordinary app must register exactly one Tauri updater plugin/);
assert.match(buildGuard, /GITHUB_RELEASE_WINDOWS_ACCEPTED/);
assert.match(buildGuard, /update\.install\(artifact\.bytes\(\)\)/);
assert.match(buildGuard, /must keep plugins\.updater\.windows\.installMode set to passive so the per-machine installer can request UAC/);

assert.match(readinessScript, /githubUpdaterAcceptanceReady/);
assert.match(readinessScript, /GITHUB_RELEASE_WINDOWS_ACCEPTED/);
assert.match(readinessScript, /protected GitHub release environment/);
assert.doesNotMatch(readinessScript, /scheduled task|ProgramData|rollback path/);
assert.match(releasePolicy, /fixed AeroTune GitHub Releases latest\.json URL/);
assert.match(releasePolicy, /standard Tauri updater stages the signed package under the current user's temp directory/);
assert.match(releasePolicy, /does not provide AeroTune's former crash-safe rollback/);
assert.match(releaseBuilder, /check-updater-release-ready\.mjs.*"--required"/);
assert.match(releaseBuilder, /createValidatedUpdaterOverlaySnapshot/);
assert.match(releaseWorkflow, /environment: aerotune-release/);
assert.match(releaseWorkflow, /contents: write/);
assert.match(releaseWorkflow, /TAURI_SIGNING_PRIVATE_KEY: \$\{\{ secrets\.TAURI_SIGNING_PRIVATE_KEY \}\}/);
assert.match(releaseWorkflow, /AEROTUNE_UPDATE_PUBLIC_KEY: \$\{\{ vars\.AEROTUNE_UPDATE_PUBLIC_KEY \}\}/);
const jobEnvironmentAt = releaseWorkflow.indexOf("    env:");
const jobStepsAt = releaseWorkflow.indexOf("\n    steps:", jobEnvironmentAt);
assert.ok(jobEnvironmentAt >= 0 && jobStepsAt > jobEnvironmentAt, "release job environment must be bounded");
assert.doesNotMatch(releaseWorkflow.slice(jobEnvironmentAt, jobStepsAt), /AEROTUNE_UPDATE_ENDPOINT|AEROTUNE_UPDATE_PUBLIC_KEY|TAURI_SIGNING_PRIVATE_KEY/);
const releaseTestsAt = releaseWorkflow.indexOf("name: Run release-quality tests without signing secrets");
const overlayCreationAt = releaseWorkflow.indexOf("name: Create temporary updater overlay from the public key");
assert.ok(releaseTestsAt >= 0 && overlayCreationAt > releaseTestsAt, "release tests must precede production updater configuration");
assert.doesNotMatch(releaseWorkflow.slice(releaseTestsAt, overlayCreationAt), /AEROTUNE_UPDATE_ENDPOINT|AEROTUNE_UPDATE_PUBLIC_KEY|TAURI_SIGNING_PRIVATE_KEY/);
assert.match(releaseWorkflow, /if: github\.ref == 'refs\/heads\/main'/);
assert.match(releaseWorkflow, /gh release create/);
const releaseStateStepAt = releaseWorkflow.indexOf("name: Resolve release tag commit and clean checkout state");
const validateTagStepAt = releaseWorkflow.indexOf("name: Validate tag and synchronized release versions");
assert.ok(releaseStateStepAt >= 0 && validateTagStepAt > releaseStateStepAt, "release metadata must be written before tag validation runs in a later step");
assert.match(releaseWorkflow.slice(releaseStateStepAt, validateTagStepAt), /GITHUB_ENV/);
const validateTagStepEnd = releaseWorkflow.indexOf("\n      - name:", validateTagStepAt);
assert.match(releaseWorkflow.slice(validateTagStepAt, validateTagStepEnd < 0 ? undefined : validateTagStepEnd), /node scripts\/validate-github-release-tag\.mjs/);
assert.match(ciWorkflow, /contents: read/);
assert.doesNotMatch(ciWorkflow, /contents: write/);
assert.match(overlayScript, /isValidTauriPublicKey/);
assert.match(manifestScript, /windows-x86_64-nsis/);
assert.match(manifestScript, /expectedArchiveName = `AeroTune_\$\{version\}_x64-setup\.nsis\.zip`/);
assert.match(manifestScript, /Buffer\.from\(signatureText, "utf8"\)\.toString\("base64"\)/);
assert.match(manifestScript, /\.nsis\\\.zip/);
assert.match(manifestScript, /const signatureName = `\$\{archiveName\}\.sig`/);
assert.match(manifestScript, /GITHUB_RELEASE_ENDPOINT|github\.com\/shengqiangdd\/AeroTune/);
assert.match(tagScript, /AEROTUNE_RELEASE_TAG_COMMIT/);
assert.match(tagScript, /AEROTUNE_RELEASE_WORKTREE_CLEAN/);

console.log("PASS standard GitHub updater contracts, signing boundary, release workflow, and early-access gate policy");
