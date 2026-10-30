import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import {
  hasConfiguredSigningKey,
  hasPrivateKeyInput,
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

const VALID_TEST_TAURI_PUBLIC_KEY = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgRTc2MjBGMTg0MkI0RTgxRgpSV1FmNkxSQ0dBOWk1M21sWWVjTzRJelQ1MVRHUHB2V3VjTlNDaDFDQk0wUVRhTG43M1k3R0ZPMw==";
const VALID_TAURI_GENERATED_PUBLIC_KEY = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE0OUVGNDc0NkZBNkU2RDcKUldUWDVxWnZkUFNlcERHV2cyYno4bjZBclFuQk13Z1FLMWdlVm9zb3JjYkx6Y3VuRCtLSitjWWU=";
const config = JSON.parse(readFileSync("src-tauri/tauri.conf.json", "utf8"));
const cargo = readFileSync("src-tauri/Cargo.toml", "utf8");
const packageJson = JSON.parse(readFileSync("package.json", "utf8"));
const guardedTauri = readFileSync("scripts/guarded-tauri.mjs", "utf8");
const buildGuard = readFileSync("src-tauri/build.rs", "utf8");
const readinessScript = readFileSync("scripts/check-updater-release-ready.mjs", "utf8");
const releasePolicySource = readFileSync("scripts/updaterReleasePolicy.mjs", "utf8");
const updaterSource = readFileSync("src-tauri/src/updater.rs", "utf8");
const recoverySource = readFileSync("src-tauri/src/update_recovery.rs", "utf8");
const updaterDoc = readFileSync("docs/UPDATER.md", "utf8");
const updaterBuildGuardTest = readFileSync("scripts/check-updater-build-guard.mjs", "utf8");
const frontendBuildScript = readFileSync("scripts/build-frontend-sanitized.mjs", "utf8");
const releaseBuilderSource = readFileSync("scripts/build-updater-release.mjs", "utf8");
const overlaySnapshotSource = readFileSync("scripts/updaterOverlaySnapshot.mjs", "utf8");
const releaseEnvironmentSource = readFileSync("scripts/updaterReleaseEnvironment.mjs", "utf8");
const readmeSource = readFileSync("README.md", "utf8");
const changelogSource = readFileSync("CHANGELOG.md", "utf8");
const installUninstallPlan = readFileSync("docs/INSTALL_UNINSTALL_ACCEPTANCE.md", "utf8");
const readiness = readFileSync("docs/RELEASE_READINESS.md", "utf8");
const privacyInventory = readFileSync("docs/DATA_PRIVACY_INVENTORY.md", "utf8");
const capabilities = JSON.parse(readFileSync("src-tauri/capabilities/default.json", "utf8"));
const exampleOverlay = JSON.parse(readFileSync("src-tauri/tauri.updater.example.json", "utf8"));
const signingEnvFixture = {
  TAURI_SIGNING_PRIVATE_KEY: "secret-value",
  tauri_signing_private_key_path: "C:\\secrets\\updater.key",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "secret-password",
  KEEP_FOR_FRONTEND: "preserved",
};
const sanitizedFrontendEnv = frontendBuildEnvironment(signingEnvFixture);
assert.equal(sanitizedFrontendEnv.TAURI_SIGNING_PRIVATE_KEY, undefined);
assert.equal(sanitizedFrontendEnv.tauri_signing_private_key_path, undefined);
assert.equal(sanitizedFrontendEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD, undefined);
assert.equal(hasSigningMaterial(signingEnvFixture), true);
assert.equal(hasSigningMaterial({ TAURI_SIGNING_PRIVATE_KEY: "  " }), false);
assert.deepEqual(tauriBundleSigningEnvironment({
  tauri_signing_private_key_path: "C:\\secure\\aerotune.key",
  tauri_signing_private_key_password: "bundle-password",
  KEEP_FOR_BUNDLE: "preserved",
}), {
  KEEP_FOR_BUNDLE: "preserved",
  TAURI_SIGNING_PRIVATE_KEY: "C:\\secure\\aerotune.key",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "bundle-password",
});
assert.deepEqual(tauriBundleSigningEnvironment({
  TAURI_SIGNING_PRIVATE_KEY: "inline-private-key-content",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "inline-password",
}), {
  TAURI_SIGNING_PRIVATE_KEY: "inline-private-key-content",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "inline-password",
});
assert.equal(sanitizedFrontendEnv.KEEP_FOR_FRONTEND, "preserved");
assert.equal(signingEnvFixture.TAURI_SIGNING_PRIVATE_KEY, "secret-value");
assert.deepEqual(updaterReleaseTestEnvironment({
  TAURI_SIGNING_PRIVATE_KEY: "private-key",
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD: "password",
  Aerotune_Updater_Config: "release.json",
  aerotune_update_endpoint: "https://updates.example.invalid/feed.json",
  AEROTUNE_UPDATE_PUBLIC_KEY: "public-key",
  Tauri_Config: "{}",
  KEEP_FOR_TEST: "preserved",
}), { KEEP_FOR_TEST: "preserved" });
assert.match(frontendBuildScript, /spawnSync\("npm run build"/);
assert.match(frontendBuildScript, /env: frontendBuildEnvironment\(source\)/);
assert.match(frontendBuildScript, /TAURI_SIGNING_PRIVATE_KEY_PATH/);
assert.match(frontendBuildScript, /TAURI_SIGNING_PRIVATE_KEY_PASSWORD/);
assert.match(frontendBuildScript, /export function tauriBundleSigningEnvironment/);
const overlayFileFixtureRoot = mkdtempSync(join(tmpdir(), "aerotune-overlay-file-"));
try {
  const validOverlayPath = join(overlayFileFixtureRoot, "release.json");
  const validOverlay = { bundle: { targets: ["nsis"] }, plugins: { updater: {} } };
  writeFileSync(validOverlayPath, JSON.stringify(validOverlay));
  assert.deepEqual(readUpdaterReleaseConfigFile(validOverlayPath), validOverlay);
  assert.throws(
    () => removeValidatedUpdaterOverlaySnapshot({ directory: overlayFileFixtureRoot, path: validOverlayPath }),
    /Refusing to remove an unexpected updater overlay snapshot path/,
  );
  const overlaySnapshot = createValidatedUpdaterOverlaySnapshot(validOverlayPath);
  try {
    assert.deepEqual(readUpdaterReleaseConfigFile(overlaySnapshot.path), validOverlay);
    writeFileSync(validOverlayPath, JSON.stringify({ changed: true }));
    assert.deepEqual(readUpdaterReleaseConfigFile(overlaySnapshot.path), validOverlay);
  } finally {
    removeValidatedUpdaterOverlaySnapshot(overlaySnapshot);
  }
  assert.throws(() => readUpdaterReleaseConfigFile(overlaySnapshot.path), (error) => error.code === "ENOENT");
  assert.throws(() => readUpdaterReleaseConfigFile(overlayFileFixtureRoot), /regular, non-symlink file/);
  const arrayOverlayPath = join(overlayFileFixtureRoot, "array.json");
  writeFileSync(arrayOverlayPath, "[]");
  assert.throws(() => readUpdaterReleaseConfigFile(arrayOverlayPath), /JSON object/);
  const invalidUtf8OverlayPath = join(overlayFileFixtureRoot, "invalid-utf8.json");
  writeFileSync(invalidUtf8OverlayPath, Buffer.from([0x7b, 0xff, 0x7d]));
  assert.throws(() => readUpdaterReleaseConfigFile(invalidUtf8OverlayPath), /valid UTF-8/);
  const oversizedOverlayPath = join(overlayFileFixtureRoot, "oversized.json");
  writeFileSync(oversizedOverlayPath, " ".repeat(64 * 1024 + 1));
  assert.throws(() => readUpdaterReleaseConfigFile(oversizedOverlayPath), /64 KiB/);
} finally {
  const resolvedOverlayFixtureRoot = resolve(overlayFileFixtureRoot);
  assert.equal(dirname(resolvedOverlayFixtureRoot), resolve(tmpdir()), "refusing to remove an unexpected overlay test path");
  rmSync(resolvedOverlayFixtureRoot, { recursive: true, force: true });
}

const cargoPackageVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
assert.equal(config.productName, "AeroTune");
assert.equal(config.identifier, "com.aerotune.desktop");
assert.equal(config.version, cargoPackageVersion);
assert.equal(config.build.beforeBuildCommand, "node scripts/build-frontend-sanitized.mjs");
assert.deepEqual(config.bundle.targets, ["nsis"]);
assert.deepEqual(exampleOverlay.bundle.targets, ["nsis"]);
assert.equal(config.bundle.windows.nsis.installMode, "perMachine");
assert.equal(config.bundle.windows.nsis.template, undefined);
assert.match(installUninstallPlan, /\*\*Status: accepted for the scoped 1\.5\.9 smoke test\.\*\*/);
assert.match(installUninstallPlan, /does not claim a blank offline machine can install without WebView2 or that the separate 1\.5\.8 upgrade path was verified/);
assert.match(installUninstallPlan, /Not covered by this scoped pass/);
assert.match(installUninstallPlan, /Preferred uninstall behavior[\s\S]*preserving per-user settings, history, journal, and recovery snapshots by default/);
for (const scenario of [
  "Standard-user clean install",
  "Upgrade with existing state",
  "Standard-user uninstall",
  "Retained-data cleanup",
  "UAC denial / interruption",
]) {
  assert.ok(installUninstallPlan.includes(scenario), `missing required install/uninstall acceptance scenario: ${scenario}`);
}
assert.match(readiness, /install\/upgrade\/uninstall[\s\S]*INSTALL_UNINSTALL_ACCEPTANCE\.md/);
assert.match(privacyInventory, /INSTALL_UNINSTALL_ACCEPTANCE\.md[\s\S]*not yet validated/);
assert.equal(config.bundle.windows.allowDowngrades, false);
assert.equal(config.plugins.updater.requireSignedVersion, true);
assert.equal(config.plugins.updater.allowDowngrades, false);
assert.equal(config.plugins.updater.dangerousInsecureTransportProtocol, false);
assert.equal(config.plugins.updater.dangerousAcceptInvalidCerts, false);
assert.equal(config.plugins.updater.dangerousAcceptInvalidHostnames, false);
assert.equal(updaterIsEnabled(config), false);
assert.match(cargo, /tauri-plugin-updater\s*=\s*"=2\.14\.0"/);
assert.match(cargo, /reqwest\s*=\s*\{\s*version\s*=\s*"0\.13",\s*default-features\s*=\s*false\s*\}/);
assert.match(cargo, /rustls\s*=\s*\{\s*version\s*=\s*"0\.23",\s*default-features\s*=\s*false,\s*features\s*=\s*\["ring"\]\s*\}/);
assert.equal(packageJson.dependencies["@tauri-apps/plugin-updater"], undefined);
assert.equal(packageJson.scripts.tauri, "node scripts/guarded-tauri.mjs");
assert.equal(packageJson.scripts["test:updater-release-contract"], "node scripts/check-updater-release-contract.mjs && node scripts/check-updater-build-guard.mjs");
assert.match(updaterBuildGuardTest, /spawnSync\("cargo"/);
assert.match(updaterBuildGuardTest, /TAURI_CONFIG: JSON\.stringify\(testCase\.config\)/);
assert.match(updaterBuildGuardTest, /PASS ordinary Cargo check passes without injected overlay/);
assert.match(guardedTauri, /scripts\/check-updater-release-ready\.mjs/);
assert.match(guardedTauri, /argument === "--config" \|\| argument === "-c"/);
assert.match(guardedTauri, /argument\.startsWith\("--config="\)/);
assert.match(guardedTauri, /argument\.startsWith\("-c="\)/);
assert.match(guardedTauri, /argument === "--runner"/);
assert.match(guardedTauri, /custom runner that can skip Cargo security checks/);
assert.match(buildGuard, /rerun-if-env-changed=TAURI_CONFIG/);
assert.match(buildGuard, /validate_windows_installer_overlay/);
assert.match(buildGuard, /validate_windows_installer_overlay\(&base, "base config"\)\?/);
assert.match(buildGuard, /validate_windows_installer_overlay\(&overlay, &path\.display\(\)\.to_string\(\)\)\?/);
assert.match(buildGuard, /validate_app_identity/);
assert.match(buildGuard, /validate_build_hooks/);
assert.match(buildGuard, /validate_release_overlay_scope/);
assert.match(buildGuard, /validate_build_hooks/);
assert.match(buildGuard, /build-frontend-sanitized\.mjs/);
assert.match(buildGuard, /beforeBundleCommand because it would run with updater signing credentials/);
assert.match(releasePolicySource, /beforeBundleCommand must remain unset because it would run with updater signing credentials/);
assert.match(buildGuard, /CARGO_PKG_VERSION/);
assert.match(buildGuard, /mainBinaryName/);
assert.match(updaterBuildGuardTest, /productName install-directory override/);
assert.match(updaterBuildGuardTest, /signed app version override/);
assert.match(updaterBuildGuardTest, /frontend build command override/);
assert.match(updaterBuildGuardTest, /remote app window override/);
assert.match(updaterBuildGuardTest, /updaterReleaseTestEnvironment\(process\.env\)/);
assert.match(updaterBuildGuardTest, /const cargoEnvironment = updaterReleaseTestEnvironment\(environment\)/);
assert.match(updaterBuildGuardTest, /cargoEnvironment\.TAURI_CONFIG = environment\.TAURI_CONFIG/);
assert.match(buildGuard, /bundle\.targets must remain exactly/);
assert.match(buildGuard, /base bundle\.targets must explicitly be/);
assert.match(buildGuard, /validate_updater_security/);
assert.match(buildGuard, /updater_is_enabled/);
assert.match(buildGuard, /scan_capabilities/);
assert.match(buildGuard, /crate::update_recovery::ensure_no_pending_journal\(RECOVERY_STORE_REQUIRED\)/);
assert.match(updaterSource, /const RECOVERY_STORE_REQUIRED: bool\s*=\s*UPDATE_PIPELINE_READY\s*\|\|\s*TASK_DEFINITION_ATTESTATION_READY/);
assert.match(buildGuard, /const UPDATE_PIPELINE_READY: bool = false/);
assert.match(buildGuard, /const UPDATE_DNS_DESTINATION_POLICY_READY: bool = false/);
assert.match(buildGuard, /const UPDATE_PROXY_POLICY_READY: bool = false/);
assert.match(buildGuard, /const UPDATE_BOUNDED_DOWNLOAD_READY: bool = false/);
assert.match(buildGuard, /VerifiedUpdateArtifact/);
assert.match(buildGuard, /journal must enforce strict SemVer precedence and bind hashes to verified artifacts/);
assert.match(buildGuard, /fn validate_runtime_update_inputs\(/);
assert.match(buildGuard, /"fn valid_tauri_public_key\("/);
assert.match(buildGuard, /"if !valid_tauri_public_key\(public_key\)"/);
assert.match(buildGuard, /"tauri_public_key_must_decode_to_a_valid_minisign_key"/);
assert.match(buildGuard, /if !dns_destination_policy_ready/);
assert.match(buildGuard, /if !proxy_policy_ready/);
assert.match(buildGuard, /must not override the selected Windows system-proxy policy with noProxy/);
assert.match(buildGuard, /must not set an explicit updater proxy outside the reviewed system-proxy policy/);
assert.match(buildGuard, /dangerousInsecureTransportProtocol/);
assert.match(buildGuard, /dangerousAcceptInvalidCerts/);
assert.match(buildGuard, /dangerousAcceptInvalidHostnames/);
assert.match(buildGuard, /installerArgs an empty array/);
assert.match(buildGuard, /installMode set to quiet/);
assert.match(buildGuard, /bundle\.windows\.nsis\.installMode/);
assert.match(buildGuard, /custom NSIS template/);
assert.match(buildGuard, /custom NSIS installer hooks with elevated script execution/);
assert.match(buildGuard, /custom NSIS language source files/);
assert.match(buildGuard, /must not set bundle\.windows\.signCommand/);
assert.match(buildGuard, /must not set bundle\.windows\.nsis\.installDir/);
assert.match(buildGuard, /windows\.contains_key\("sign-command"\)/);
assert.match(buildGuard, /nsis\.contains_key\("install-dir"\)/);
assert.match(buildGuard, /installer-hooks/);
assert.match(buildGuard, /canonical NSIS keys; serde aliases are forbidden/);
assert.match(buildGuard, /let compact_source: String = source/);
assert.match(buildGuard, /\.filter\(\|character\| !character\.is_whitespace\(\)\)/);
assert.match(buildGuard, /compact_source\.contains\("::install\("\)/);
assert.match(buildGuard, /compact_source\.contains\("\.download\("\)/);
assert.match(buildGuard, /compact_source\.contains\("::download\("\)/);
assert.match(buildGuard, /dangerous-insecure-transport-protocol/);
assert.match(buildGuard, /require-signed-version/);
assert.match(buildGuard, /allow-downgrades/);
assert.match(buildGuard, /installer-args/);
assert.match(buildGuard, /install-mode/);
assert.match(buildGuard, /if !bounded_download_ready/);
assert.match(buildGuard, /if !task_definition_attestation_ready/);
assert.match(updaterSource, /fn validate_runtime_update_inputs\(/);
assert.match(updaterSource, /if !dns_destination_policy_ready/);
assert.match(updaterSource, /if !proxy_policy_ready/);
assert.match(updaterSource, /if !bounded_download_ready/);
assert.match(updaterSource, /if !task_definition_attestation_ready/);
assert.match(updaterSource, /validate_runtime_update_inputs\([\s\S]*?UPDATE_DNS_DESTINATION_POLICY_READY,\s*UPDATE_PROXY_POLICY_READY,\s*UPDATE_BOUNDED_DOWNLOAD_READY,\s*TASK_DEFINITION_ATTESTATION_READY,\s*UPDATE_PIPELINE_READY/);
assert.match(updaterSource, /RESERVED_UPDATE_HOST_SUFFIXES/);
assert.match(updaterSource, /raw_host\.ends_with\('\.'\)/);
assert.match(updaterSource, /url\.port\(\)\.is_some\(\)/);
assert.match(updaterSource, /url\.fragment\(\)\.is_some\(\)/);
assert.match(updaterSource, /!allow_query && url\.query\(\)\.is_some\(\)/);
assert.match(updaterSource, /fn valid_tauri_public_key\(public_key: &str\) -> bool/);
assert.match(updaterSource, /if !valid_tauri_public_key\(public_key\)/);
assert.match(updaterSource, /tauri_public_key_must_decode_to_a_valid_minisign_key/);
assert.match(updaterSource, /missing_feed_or_public_key_fails_closed_without_network_access/);
assert.match(releasePolicySource, /export function isValidTauriPublicKey\(publicKey\)/);
assert.match(releasePolicySource, /plugins\.updater\.pubkey must be a valid Tauri Minisign public key encoded as base64/);
assert.match(updaterSource, /validate_artifact_request_headers\(&update\.headers\)\?/);
assert.match(updaterSource, /artifact_download_rejects_unreviewed_custom_headers/);
assert.match(updaterSource, /lower_name\.starts_with\("aerotune_"\)/);
assert.match(updaterSource, /lower_name\.ends_with\("-setup\.exe"\)/);
assert.doesNotMatch(updaterSource, /lower_name\.ends_with\("\.msi"\)/);
assert.match(updaterSource, /update_archive_rejects_msi_non_setup_and_wrong_product_executables/);
const rustReservedHostBody = updaterSource.match(/const RESERVED_UPDATE_HOST_SUFFIXES: &\[&str\] = &\[([\s\S]*?)\];/)?.[1];
const nodeReservedHostBody = releasePolicySource.match(/const RESERVED_DNS_SUFFIXES = \[([\s\S]*?)\];/)?.[1];
assert.ok(rustReservedHostBody && nodeReservedHostBody, "both feed validators must define reserved host suffixes");
const extractReservedSuffixes = (body) => [...body.matchAll(/"([^"]+)"/g)].map(([, suffix]) => suffix.replace(/^\./, ""));
assert.deepEqual(extractReservedSuffixes(rustReservedHostBody), extractReservedSuffixes(nodeReservedHostBody), "Rust runtime and Node release preflight reserved-host suffixes must match");
const rustUnversionedPublicKey = updaterSource.match(/const VALID_TEST_TAURI_PUBLIC_KEY: &str = "([^"]+)";/)?.[1];
const rustTauriSignerPublicKey = updaterSource.match(/const VALID_TAURI_GENERATED_PUBLIC_KEY: &str = "([^"]+)";/)?.[1];
assert.equal(rustUnversionedPublicKey, VALID_TEST_TAURI_PUBLIC_KEY, "Rust and Node unversioned Minisign test vectors must stay identical");
assert.equal(rustTauriSignerPublicKey, VALID_TAURI_GENERATED_PUBLIC_KEY, "Rust and Node Tauri signer test vectors must stay identical");
assert.match(updaterSource, /更新源 DNS 目的地址策略尚未通过审核/);
assert.match(updaterSource, /系统代理保留策略尚未通过 VM 验证/);
assert.match(updaterSource, /固定更新任务定义和访问控制尚未通过运行时验收/);
assert.equal((updaterSource.match(/crate::update_recovery::ensure_no_pending_journal\(RECOVERY_STORE_REQUIRED\)/g) ?? []).length, 2);
assert.match(recoverySource, /windows_store::ensure_no_pending_journal\(store_required\)/);
assert.match(recoverySource, /fn is_missing_optional_store\(/);
assert.match(recoverySource, /pub fn ensure_no_pending_journal\(store_required: bool\)[\s\S]*?if !store_required \{\s*return ensure_optional_store_at\(&program_data_path\(\)\?\);\s*\}\s*let store = open_verified_store\(\)\?/);
assert.match(recoverySource, /fn ensure_optional_store_at\(program_data: &Path\)[\s\S]*?open_program_data_directories\(program_data\)[\s\S]*?Err\(_\) => return Ok\(\(\)\)[\s\S]*?has_expected_directory_security\(&root_handle\)[\s\S]*?open_verified_store_at\(program_data\)/);
assert.match(recoverySource, /untrusted_optional_store_does_not_block_startup_or_delete_evidence/);
assert.match(recoverySource, /fn open_verified_program_data\(program_data: &Path\).*?has_expected_program_data_security/s);
assert.match(recoverySource, /fn open_program_data_directories\(program_data: &Path\).*?open_directory\(program_data\)/s);
assert.match(recoverySource, /disabled_update_gate_tolerates_an_absent_store/);
assert.match(recoverySource, /optional_store_is_skipped_only_when_missing/);
assert.match(recoverySource, /SE_DACL_PROTECTED/);
assert.match(recoverySource, /FILE_ATTRIBUTE_REPARSE_POINT/);
assert.match(recoverySource, /reject_any_directory_entry/);
assert.match(recoverySource, /fn create_prepared\(/);
assert.match(recoverySource, /fn advance\(/);
assert.match(recoverySource, /CREATE_NEW/);
assert.match(recoverySource, /FlushFileBuffers/);
assert.match(recoverySource, /MOVEFILE_WRITE_THROUGH/);
assert.match(recoverySource, /D:P\(D;;WD;;;OW\)/);
assert.match(recoverySource, /FILE_FLAG_OPEN_REPARSE_POINT/);
assert.match(recoverySource, /trait JournalCommitIo/);
assert.match(recoverySource, /current_process_can_write_journal_file/);
assert.match(recoverySource, /OpenThreadToken/);
assert.match(recoverySource, /FILE_GENERIC_WRITE\s*\|\s*FILE_READ_ATTRIBUTES/);
assert.match(recoverySource, /check_access\(FILE_GENERIC_WRITE\s*\|\s*FILE_READ_ATTRIBUTES\)/);
assert.match(recoverySource, /let bytes = journal\.encode\(\)\?;\s*io\.preflight\(\)\?;\s*io\.create_temp\(\)\?/);
assert.match(recoverySource, /atomic_writer_exercises_disposable_temp_directory_with_current_token/);
assert.match(recoverySource, /atomic_writer_stops_at_each_injected_failure_and_preserves_evidence/);
assert.match(recoverySource, /rollback_is_reachable_from_each_post_intent_phase_and_preserves_identity/);
assert.match(recoverySource, /restricted_current_owner_lacks_write_dac_but_retains_read/);
assert.match(recoverySource, /CheckTokenMembership/);
assert.match(updaterDoc, /Recovery decision matrix \(design; supervisor not implemented\)/);
assert.match(updaterDoc, /When recovery is required or a protected store is present/);
assert.match(updaterDoc, /distinct recovery-mode path for an abandoned/);
assert.match(updaterDoc, /When `RECOVERY_STORE_REQUIRED` is enabled/);
assert.match(updaterDoc, /missing ProgramData updater store is tolerated, and the known ProgramData volume\/root is still opened without following reparse points/);
assert.match(updaterDoc, /untrusted user-created roots are ignored read-only and never removed, avoiding a ProgramData startup-denial vector/);
assert.match(updaterDoc, /Strict owner\/DACL validation is mandatory when recovery is enabled/);
assert.match(updaterDoc, /absence is tolerated, but a present store is inspected read-only/);
assert.match(updaterDoc, /process tree to quiesce/);
assert.match(updaterDoc, /signed-version binding with the pinned public key/);
assert.match(updaterDoc, /No such cleanup API exists yet/);
assert.match(updaterDoc, /rollback_required/);
assert.match(updaterDoc, /re-enumerate to prove the directory is empty/i);
assert.doesNotMatch(recoverySource, /CreateDirectoryW|SetSecurityInfo|SetFileSecurityW|MOVEFILE_COPY_ALLOWED|MOVEFILE_DELAY_UNTIL_REBOOT/);
assert.equal((recoverySource.match(/if !crate::updater::RECOVERY_STORE_REQUIRED/g) ?? []).length, 2);
assert.match(buildGuard, /both journal writer entry points must remain readiness-gated/);
assert.match(buildGuard, /journal writer entry points must remain private/);
assert.doesNotMatch(updaterSource, /create_prepared_journal|advance_journal/);
assert.match(updaterSource, /SHGetKnownFolderPath/);
assert.doesNotMatch(updaterSource, /var_os\("ProgramFiles"\)/);
assert.match(updaterSource, /trusted_program_files_root_resolves_to_existing_absolute_known_folder/);
assert.match(updaterDoc, /SHGetKnownFolderPath/);
assert.match(updaterDoc, /TAURI_SIGNING_PRIVATE_KEY_PATH/);
assert.match(updaterDoc, /IP-literal hosts are rejected/);
assert.match(updaterDoc, /Node release preflight validates configured feed endpoints/);
assert.match(updaterDoc, /HTTPS, multi-label DNS, default port, no credentials\/query\/fragment\/trailing dot, and no reserved\/local names/);
assert.match(updaterDoc, /rejection messages do not echo the full URL/);
assert.match(updaterDoc, /Rust runtime validates feed and artifact URLs independently before use/);
assert.match(updaterDoc, /artifact URLs may retain signed CDN query parameters but reject fragments/);
assert.match(updaterDoc, /not DNS answers/);
assert.match(updaterDoc, /Direct-route DNS answer is loopback\/private\/link-local\/ULA, or rebinds between checks/);
assert.match(updaterDoc, /Windows system proxy is configured, bypassed, or unavailable/);
assert.match(updaterDoc, /prove no silent direct fallback/);
assert.match(updaterDoc, /A separate hard-off destination-policy gate/);
assert.match(updaterDoc, /checks bounded feed-response, pipeline, DNS, system-proxy, bounded-download, and task-attestation readiness before constructing the updater plugin, so current hard-off gates stop even feed requests/);
assert.match(updaterDoc, /UPDATE_PROXY_POLICY_READY/);
assert.match(updaterDoc, /The selected policy preserves Windows system-proxy use/);
assert.match(updaterDoc, /proxy resolves the origin hostname, so local DNS pinning cannot attest that destination/);
assert.match(updaterDoc, /Reject `noProxy` and explicit proxy overrides unless separately reviewed/);
assert.match(updaterDoc, /Upstream `tauri-plugin-updater` 2\.14\.0 `Updater::check` calls `Response::json\(\)\.await` directly without an explicit response-body size cap/);
assert.match(updaterDoc, /AeroTune now vendors a local patch that replaces this with bounded streaming JSON accumulation \(256 KiB prototype cap\)/);
assert.match(updaterDoc, /Patch provenance and upgrade\/rebase notes are in \[UPSTREAM_PATCH\.md\]\(vendor\/tauri-plugin-updater\/UPSTREAM_PATCH\.md\)/);
assert.match(updaterDoc, /Review and measure the vendored feed-manifest patch before enabling checks: it caps responses at 256 KiB/);
assert.match(readinessScript, /direct-route DNS pinning or a reviewed system-proxy egress allowlist\/rebinding defense/);
assert.match(readmeSource, /应用内自动更新当前保持关闭。开启前仍需完成签名发布、HTTPS feed、受限下载、恢复\/回滚及隔离 Windows VM 验证/);
assert.match(readmeSource, /详见\[`docs\/UPDATER\.md`\]\(docs\/UPDATER\.md\)/);
assert.match(readmeSource, /详见\[`CHANGELOG\.md`\]\(CHANGELOG\.md\)/);
assert.doesNotMatch(readmeSource, /^##.*(?:v[0-9]+\.[0-9]+|20[0-9]{2}-[0-9]{2}-[0-9]{2})/m);
assert.match(changelogSource, /1\.5\.4/);
assert.match(changelogSource, /v0\.3/);
assert.match(updaterDoc, /custom redirect policy allows at most three hops/);
assert.match(updaterDoc, /Cargo dependency is pinned exactly to 2\.14\.0 and patched locally/);
assert.match(updaterDoc, /all updater requests have a 30-second timeout/);
assert.match(updaterDoc, /bounded request builder rejects any unreviewed custom artifact headers \(none are configured\), applies the same redirect\/30-second timeout policy, and rejects explicit `noProxy` or custom-proxy overrides/);
assert.match(updaterDoc, /VM verification is still required/);
assert.match(updaterDoc, /run a local HTTP redirect through the configured client/);
assert.match(updaterDoc, /actual HTTPS feed\/CDN redirect behavior still requires disposable-VM integration validation/);
assert.match(updaterDoc, /call Tauri `Update::verify` on those exact bytes/);
assert.match(updaterDoc, /structurally valid Tauri Minisign public key \(release preflight and Rust runtime both parse the base64-wrapped key\)/);
assert.match(updaterDoc, /dangerousInsecureTransportProtocol.*explicitly false and rejected when true in build\/release overlays/);
assert.match(updaterDoc, /Guards also reject Tauri's hyphenated serde aliases/);
assert.match(updaterDoc, /Cargo build guard pins root `productName`, `identifier`, and `version` to AeroTune's protected identity and Cargo package version/);
assert.match(updaterDoc, /requires both base config and overlays to leave `bundle\.windows\.signCommand` and `bundle\.windows\.nsis\.installDir` unset/);
assert.match(updaterDoc, /beforeBuildCommand` uses `scripts\/build-frontend-sanitized\.mjs`, which removes all updater signing credential variables before spawning `npm run build`/);
assert.match(updaterDoc, /separates `tauri build --no-bundle` \(credentials stripped before Cargo\/build scripts\) from `tauri bundle`/);
assert.match(updaterDoc, /contract tests receive neither signing material nor inherited updater overlay\/runtime configuration/);
assert.match(updaterDoc, /maps a path-only `TAURI_SIGNING_PRIVATE_KEY_PATH` input to Tauri's `TAURI_SIGNING_PRIVATE_KEY` build\/bundle input and removes the signer-only alias/);
assert.match(updaterDoc, /snapshots the validated JSON overlay and uses that same temporary file for required preflight and bundling/);
assert.match(updaterDoc, /pin NSIS `installMode` to `perMachine`, reject the accepted `install-mode`\/`installer-hooks` serde aliases, custom NSIS templates, hooks, and language source files/);
assert.match(updaterDoc, /base bundle and every signed-updater overlay must also target exactly `\["nsis"\]`; MSI, `all`, and omitted release targets are rejected/);
assert.match(updaterDoc, /updater release overlay is allowlisted to `\$schema`, `bundle`, and `plugins`, with nested key allowlists that reject a Windows `signCommand`, NSIS `installDir`\/hooks\/template, custom updater headers/);
assert.match(updaterDoc, /regular, non-symlink, valid-UTF-8 JSON overlay files up to 64 KiB/);
assert.match(updaterDoc, /do not use the default `Update::install` path/);
assert.match(updaterDoc, /Base and helper configs pin `windows\.installMode` to `quiet` only for an already-elevated fixed runner/);
assert.match(updaterDoc, /`windows\.installerArgs` to an empty array/);
assert.match(updaterDoc, /arbitrary NSIS\/MSI arguments could redirect installation outside protected Program Files/);
assert.match(updaterDoc, /private-field `VerifiedUpdateArtifact` only after Tauri verifies the exact bytes\/signature/);
assert.match(updaterDoc, /private journal factory accepts verified update and rollback artifact values/);
assert.match(updaterDoc, /exposes a read-back check that rejects a length mismatch before hashing a future staged copy/);
assert.match(updaterDoc, /future file reader must still enforce the artifact byte cap/);
assert.match(updaterDoc, /Neither the read-back check nor this type boundary is wired to filesystem staging or the journal writer/);
assert.match(updaterDoc, /authoritative streaming byte counter even when length is absent/);
assert.match(updaterDoc, /64 MiB compressed transport cap and a 128 MiB actual-expanded cap/);
assert.match(updaterDoc, /after signature verification it accepts only a ZIP with one root-level unencrypted `AeroTune_\.\.\.-setup\.exe` NSIS installer, rejects MSI, non-setup executables, wrong-product names, links, and traversal, and drains the member to EOF\/CRC under that cap/);
assert.match(updaterDoc, /Neither limit is release-approved/);
assert.match(updaterDoc, /Review and measure the vendored feed-manifest patch before enabling checks: it caps responses at 256 KiB/);
assert.match(updaterDoc, /direct EXE\/MSI updater payloads are currently rejected/);
assert.match(updaterDoc, /strict SemVer precedence progression \(build metadata alone does not count\)/);
assert.match(updaterSource, /pub\(crate\) struct VerifiedUpdateArtifact/);
assert.match(updaterSource, /fn verify_and_wrap_update_artifact\(/);
assert.match(updaterSource, /pub\(crate\) fn verify_staged_bytes\(/);
assert.match(updaterSource, /if staged_bytes\.len\(\) != self\.bytes\.len\(\)/);
assert.match(updaterSource, /let staged_digest: \[u8; 32\] = Sha256::digest\(staged_bytes\)\.into\(\);/);
assert.match(updaterSource, /if staged_digest != self\.sha256/);
assert.match(updaterSource, /verified_artifact_binds_exact_bytes_target_version_and_digest/);
assert.match(updaterSource, /fn verify_update_signature\([\s\S]*\.verify\(bytes\)/);
assert.match(updaterSource, /from_verified_artifacts\(/);
assert.match(recoverySource, /target\.cmp_precedence\(&from\) != std::cmp::Ordering::Greater/);
assert.match(recoverySource, /journal_rejects_build_metadata_only_version_progression/);
assert.match(recoverySource, /&crate::updater::VerifiedUpdateArtifact/);
assert.match(recoverySource, /update_artifact\.sha256_hex\(\)/);
assert.match(recoverySource, /rollback_artifact\.sha256_hex\(\)/);
assert.doesNotMatch(updaterSource, /pub (?:bytes|target_version|sha256):/);
assert.doesNotMatch(recoverySource, /pub struct UpdateTransactionJournal|pub (?:schema_version|transaction_id|from_version|target_version|update_artifact_sha256|rollback_artifact_sha256|phase):/);
assert.match(updaterDoc, /Disposable Windows VM acceptance matrix \(not yet executed\)/);
assert.match(updaterDoc, /build-metadata-only difference/);
assert.match(updaterDoc, /none of these end-to-end scenarios has passed/);
assert.match(updaterDoc, /registration stays hard-off in this build/);
assert.match(updaterDoc, /One elevated read-only `Check` returned exit code 0/);
assert.match(updaterDoc, /subsequent non-elevated `Check` reported that the fixed `AeroTune-SignedUpdater` task is not registered/);
assert.match(updaterDoc, /full Task Scheduler was not enumerated/);
assert.match(updaterDoc, /rejects overrides other than the exact `Program Files\\AeroTune\\AeroTune\.exe` target/);
assert.equal(packageJson.scripts["build:updater-release"], "node scripts/build-updater-release.mjs");
assert.match(releaseBuilderSource, /const testEnvironment = updaterReleaseTestEnvironment\(environment\)/);
assert.match(releaseEnvironmentSource, /frontendBuildEnvironment\(source\)/);
assert.match(releaseEnvironmentSource, /AEROTUNE_UPDATER_CONFIG/);
assert.match(releaseEnvironmentSource, /AEROTUNE_UPDATE_ENDPOINT/);
assert.match(releaseEnvironmentSource, /AEROTUNE_UPDATE_PUBLIC_KEY/);
assert.match(releaseEnvironmentSource, /TAURI_CONFIG/);
assert.match(releaseBuilderSource, /run\("elevated task contract", \["scripts\/check-elevated-test-task-contract\.mjs"\], testEnvironment\)/);
assert.match(releaseBuilderSource, /run\("signed updater task contract".*testEnvironment\)/);
assert.match(releaseBuilderSource, /run\("updater release contracts".*testEnvironment\)/);
assert.match(releaseBuilderSource, /run\("dynamic Cargo guard matrix", \["scripts\/check-updater-build-guard\.mjs"\], testEnvironment\)/);
assert.match(releaseBuilderSource, /createValidatedUpdaterOverlaySnapshot\(sourceOverlayPath\)/);
assert.match(releaseBuilderSource, /environment\.AEROTUNE_UPDATER_CONFIG = overlaySnapshot\.path/);
assert.match(releaseBuilderSource, /removeValidatedUpdaterOverlaySnapshot\(overlaySnapshot\)/);
assert.match(overlaySnapshotSource, /lstatSync/);
assert.match(overlaySnapshotSource, /Refusing to remove a non-directory updater overlay snapshot path/);
assert.match(releaseBuilderSource, /required updater release preflight.*"--required"\], environment\)/);
assert.match(releaseBuilderSource, /guarded-tauri\.mjs", "build", "--no-bundle"/);
assert.match(releaseBuilderSource, /guarded-tauri\.mjs", "bundle", "--config", overlayPath/);
assert.match(guardedTauri, /isBundle = args\[0\] === "bundle"/);
assert.match(guardedTauri, /hasSigningMaterial\(env\)/);
assert.match(guardedTauri, /frontendBuildEnvironment\(env\)/);
assert.match(guardedTauri, /tauriBundleSigningEnvironment\(env\)/);
assert.equal(capabilities.permissions.some((permission) => permission.startsWith("updater:")), false);
assert.equal(hasUpdaterPermission(capabilities), false);
const nestedCapabilityRoot = mkdtempSync(join(tmpdir(), "aerotune-capabilities-"));
try {
  const nestedCapabilityDirectory = join(nestedCapabilityRoot, "nested", "windows");
  mkdirSync(nestedCapabilityDirectory, { recursive: true });
  writeFileSync(join(nestedCapabilityDirectory, "unsafe.json"), JSON.stringify({ permissions: ["updater:default"] }));
  const nestedCapabilities = readCapabilityFiles(nestedCapabilityRoot);
  assert.equal(nestedCapabilities.length, 1);
  assert.match(nestedCapabilities[0].path, /nested[\\/]windows[\\/]unsafe\.json$/);
  assert.ok(validateUpdaterReleaseConfig(config, { plugins: { updater: {} } }, { capabilityFiles: nestedCapabilities }).errors.some((error) => error.includes("unsafe.json")));
} finally {
  rmSync(nestedCapabilityRoot, { recursive: true, force: true });
}
assert.deepEqual(config.plugins.updater.endpoints, []);
assert.equal(config.plugins.updater.pubkey, "");
assert.equal(config.plugins.updater.requireSignedVersion, true);
assert.equal(config.plugins.updater.allowDowngrades, false);
assert.equal(config.plugins.updater.dangerousInsecureTransportProtocol, false);
assert.equal(config.plugins.updater.dangerousAcceptInvalidCerts, false);
assert.equal(config.plugins.updater.dangerousAcceptInvalidHostnames, false);
assert.equal(config.plugins.updater.windows.installMode, "quiet");
assert.deepEqual(config.plugins.updater.windows.installerArgs, []);
assert.equal(exampleOverlay.plugins.updater.windows.installMode, "quiet");
assert.deepEqual(exampleOverlay.plugins.updater.windows.installerArgs, []);
assert.equal(exampleOverlay.bundle.windows.nsis.installMode, "perMachine");
for (const option of [
  "dangerousInsecureTransportProtocol",
  "dangerousAcceptInvalidCerts",
  "dangerousAcceptInvalidHostnames",
]) {
  assert.equal(exampleOverlay.plugins.updater[option], false);
}
const exampleResult = validateUpdaterReleaseConfig(config, exampleOverlay, { privateKeyPresent: false, elevatedRunnerReady: false });
assert.ok(exampleResult.errors.some((error) => error.includes("public key")));
assert.ok(exampleResult.errors.some((error) => error.includes("Reserved/example")));
assert.ok(exampleResult.errors.some((error) => error.includes("TAURI_SIGNING_PRIVATE_KEY")));
assert.ok(exampleResult.errors.some((error) => error.includes("Configure exactly one")));
assert.ok(exampleResult.errors.some((error) => error.includes("runner/task")));
assert.equal(hasPrivateKeyInput(undefined), false);
assert.equal(hasPrivateKeyInput("   "), false);
assert.equal(hasPrivateKeyInput("REPLACE_WITH_PRIVATE_KEY"), false);
assert.equal(hasPrivateKeyInput("Path or content of your private key"), false);
assert.equal(hasPrivateKeyInput("C:\\secure\\aerotune-signing.key"), false);
assert.equal(hasPrivateKeyInput("package.json"), false, "repository files must never qualify as signing keys");
assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY_PATH: "package.json" }), false);
assert.equal(hasConfiguredSigningKey({}), false);
assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY: "REPLACE_WITH_PRIVATE_KEY" }), false);
assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY: "encrypted-minisign-secret-input" }), true);
assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY: "inline", TAURI_SIGNING_PRIVATE_KEY_PATH: "key" }), false);
const signingKeyFixtureRoot = mkdtempSync(join(tmpdir(), "aerotune-signing-key-"));
try {
  const emptySigningKeyPath = join(signingKeyFixtureRoot, "empty.key");
  writeFileSync(emptySigningKeyPath, "");
  assert.equal(hasPrivateKeyInput(emptySigningKeyPath), false);
  assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY_PATH: emptySigningKeyPath }), false);
  const directoryNamedKey = join(signingKeyFixtureRoot, "directory.key");
  mkdirSync(directoryNamedKey);
  assert.equal(hasPrivateKeyInput(directoryNamedKey), false);
  assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY_PATH: directoryNamedKey }), false);
  const signingKeyPath = join(signingKeyFixtureRoot, "test.key");
  writeFileSync(signingKeyPath, "test-only placeholder bytes");
  assert.equal(hasPrivateKeyInput(signingKeyPath), true);
  assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY_PATH: signingKeyPath }), true);
  assert.equal(hasConfiguredSigningKey({ tauri_signing_private_key_path: signingKeyPath }), true);
  const missingSigningKeyPath = join(signingKeyFixtureRoot, "missing.key");
  assert.equal(hasPrivateKeyInput(missingSigningKeyPath), false);
  assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY_PATH: missingSigningKeyPath }), false);
  assert.equal(hasConfiguredSigningKey({ TAURI_SIGNING_PRIVATE_KEY: missingSigningKeyPath }), false);
} finally {
  rmSync(signingKeyFixtureRoot, { recursive: true, force: true });
}
assert.match(readinessScript, /privateKeyPresent:\s*hasConfiguredSigningKey\(process\.env\)/);
assert.match(readinessScript, /signingKeyProtectionReady:\s*false/);
assert.match(readinessScript, /key-vault\/file ACL policy review still required/);
assert.match(updaterDoc, /does not inspect Windows ACLs or prove a CI secret-store policy/);
assert.match(readinessScript, /readUpdaterReleaseConfigFile\(overlayPath\)/);
assert.match(releasePolicySource, /lstatSync/);
assert.match(releasePolicySource, /MAX_UPDATER_RELEASE_CONFIG_BYTES = 64 \* 1024/);
assert.match(releasePolicySource, /before\.isSymbolicLink\(\) \|\| !before\.isFile\(\)/);
assert.match(releasePolicySource, /!keyStats\.isSymbolicLink\(\)/);
assert.match(releasePolicySource, /!isInsideRepository\(path\)/);
assert.match(releasePolicySource, /source\.byteLength !== before\.size/);
assert.match(releasePolicySource, /new TextDecoder\("utf-8", \{ fatal: true \}\)/);
assert.match(readinessScript, /review and measurement of the vendored 256 KiB feed-manifest cap/);
assert.match(readinessScript, /bounded feed\/artifact handling, DNS and system-proxy policy/);
assert.match(readinessScript, /disposable-Windows-VM validation of preserved system-proxy routing, origin-DNS behavior, and fail-closed fallback/);
assert.doesNotMatch(readinessScript, /console\.(?:log|error|warn)\([^\n]*(?:process\.env|TAURI_SIGNING_PRIVATE_KEY|keyInput)/);
assert.match(readinessScript, /feedResponseBoundedReady: false/);
assert.match(readinessScript, /dnsDestinationPolicyReady: false/);
assert.match(readinessScript, /proxyPolicyReady: false/);
assert.match(readinessScript, /boundedDownloadReady: false/);
assert.match(readinessScript, /reviewed compressed\/expanded artifact byte caps/);
assert.match(releasePolicySource, /vendored 256 KiB feed-manifest cap is a prototype and has not passed release review/);
assert.match(releasePolicySource, /Direct-route DNS destination pinning or a reviewed system-proxy egress allowlist\/rebinding defense is not implemented/);
assert.match(releasePolicySource, /selected policy preserves Windows system proxy, but its routing, origin-DNS behavior, and fail-closed fallback have not passed disposable-VM validation/);
assert.match(releasePolicySource, /base beforeBuildCommand must use the sanitized frontend wrapper/);
assert.match(releasePolicySource, /selected Windows system-proxy policy forbids noProxy overrides/);
assert.match(releasePolicySource, /Explicit updater proxy overrides require a separately reviewed proxy policy/);
assert.match(releasePolicySource, /Reviewed artifact byte caps, a release-ready bounded downloader, exact-byte signature verification, and protected staging are not complete/);

const overlay = {
  bundle: { createUpdaterArtifacts: true, targets: ["nsis"], windows: { allowDowngrades: false } },
  plugins: {
    updater: {
      pubkey: VALID_TEST_TAURI_PUBLIC_KEY,
      endpoints: ["https://updates.aerotune.net/releases/latest.json"],
      requireSignedVersion: true,
      allowDowngrades: false,
      windows: { installMode: "quiet", installerArgs: [] },
    },
  },
};

assert.ok(isValidTauriPublicKey(VALID_TEST_TAURI_PUBLIC_KEY));
assert.ok(isValidTauriPublicKey(VALID_TAURI_GENERATED_PUBLIC_KEY));
const CRLF_TEST_TAURI_PUBLIC_KEY = Buffer.from(
  Buffer.from(VALID_TEST_TAURI_PUBLIC_KEY, "base64").toString("utf8").replace(/\n/g, "\r\n"),
  "utf8",
).toString("base64");
assert.ok(isValidTauriPublicKey(CRLF_TEST_TAURI_PUBLIC_KEY));
assert.ok(!isValidTauriPublicKey("RWQPolicyTestKeyNotCryptographicallyValid"));
assert.ok(!isValidTauriPublicKey("@@not-base64@@"));
assert.ok(validateUpdaterReleaseConfig(config, {
  ...overlay,
  plugins: { updater: { ...overlay.plugins.updater, pubkey: "RWQPolicyTestKeyNotCryptographicallyValid" } },
}, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("valid Tauri Minisign public key")));

assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, productName: "OtherProduct" }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("must not override productName")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, identifier: "com.attacker.other" }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("must not override identifier")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, version: "1.5.8" }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("must not override version")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, mainBinaryName: "OtherApp" }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("must not override mainBinaryName")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, "product-name": "AeroTune" }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias product-name")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, "main-binary-name": "AeroTune" }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias main-binary-name")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, build: { beforeBuildCommand: "node malicious-build-hook.mjs" } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("root key build is forbidden")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, app: { windows: [{ url: "https://attacker.example" }] } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("root key app is forbidden")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, notification: { enabled: false } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("plugin notification is outside")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { ...overlay.bundle.windows, signCommand: "cmd.exe /c echo signing" } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("bundle.windows key signCommand is outside")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { ...overlay.bundle.windows, nsis: { installMode: "perMachine", installDir: "$PROGRAMFILES64\\Other" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("bundle.windows.nsis key installDir is outside")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, headers: { Authorization: "Bearer test-only" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("plugins.updater key headers is outside")));
assert.ok(validateUpdaterReleaseConfig({ ...config, productName: "OtherProduct" }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("base productName must remain AeroTune")));
assert.ok(validateUpdaterReleaseConfig({
  ...config,
  bundle: { ...config.bundle, windows: { ...config.bundle.windows, signCommand: "cmd.exe /c echo signing" } },
}, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("base bundle.windows.signCommand must remain unset")));
assert.ok(validateUpdaterReleaseConfig({
  ...config,
  bundle: { ...config.bundle, windows: { ...config.bundle.windows, nsis: { ...config.bundle.windows.nsis, installMode: "currentUser" } } },
}, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("protected AeroTune updater design requires the base NSIS installMode to remain perMachine")));
assert.ok(validateUpdaterReleaseConfig({
  ...config,
  bundle: { ...config.bundle, windows: { ...config.bundle.windows, nsis: { ...config.bundle.windows.nsis, installDir: "$PROGRAMFILES64\\Other" } } },
}, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("base bundle.windows.nsis.installDir must remain unset")));
assert.ok(validateUpdaterReleaseConfig({
  ...config,
  bundle: { ...config.bundle, windows: { ...config.bundle.windows, "sign-command": "cmd.exe /c echo signing" } },
}, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias sign-command")));
assert.ok(validateUpdaterReleaseConfig({
  ...config,
  bundle: { ...config.bundle, windows: { ...config.bundle.windows, nsis: { ...config.bundle.windows.nsis, "install-dir": "$PROGRAMFILES64\\Other" } } },
}, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias install-dir")));
assert.ok(validateUpdaterReleaseConfig({ ...config, build: { ...config.build, beforeBuildCommand: "npm run build" } }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("sanitized frontend wrapper")));
assert.ok(validateUpdaterReleaseConfig({ ...config, build: { ...config.build, beforeBundleCommand: "node unsigned-hook.mjs" } }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("beforeBundleCommand must remain unset")));
assert.ok(validateUpdaterReleaseConfig({ ...config, identifier: "com.attacker.other" }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("base identifier must remain com.aerotune.desktop")));
assert.ok(validateUpdaterReleaseConfig({ ...config, version: "not-semver" }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("base version must be a strict SemVer")));
assert.ok(validateUpdaterReleaseConfig(config, overlay, { privateKeyPresent: false, elevatedRunnerReady: true }).errors.some((error) => error.includes("TAURI_SIGNING_PRIVATE_KEY")));
assert.ok(validateUpdaterReleaseConfig(config, overlay, { privateKeyPresent: true, elevatedRunnerReady: false }).errors.some((error) => error.includes("runner/task")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, requireSignedVersion: false } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("requireSignedVersion")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, allowDowngrades: true } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("allowDowngrades")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, "require-signed-version": false } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias require-signed-version")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, "dangerous-insecure-transport-protocol": true } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias dangerous-insecure-transport-protocol")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, "allow-downgrades": true } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias allow-downgrades")));
const baseAliasResult = validateUpdaterReleaseConfig({ ...config, plugins: { ...config.plugins, updater: { ...config.plugins.updater, "require-signed-version": true } } }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true });
assert.ok(baseAliasResult.errors.some((error) => error.includes("base plugins.updater must not use serde alias require-signed-version")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, windows: { ...overlay.plugins.updater.windows, "install-mode": "passive" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias install-mode")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, windows: { ...overlay.plugins.updater.windows, "installer-args": ["/D=C:\\\\Temp"] } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias installer-args")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { "allow-downgrades": true } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias allow-downgrades")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: ["http://updates.aerotune.net/latest.json"] } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("HTTPS")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: ["https://127.0.0.1/latest.json"] } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("DNS hostname")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: ["https://[::1]/latest.json"] } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("DNS hostname")));
for (const endpoint of [
  "https://2130706433/latest.json",
  "https://0x7f000001/latest.json",
  "https://0177.0.0.1/latest.json",
  "https://127.1/latest.json",
  "https://127.0.0.1./latest.json",
]) {
  const result = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: [endpoint] } } }, { privateKeyPresent: true, elevatedRunnerReady: true });
  assert.ok(result.errors.some((error) => error.includes("DNS hostname")), `non-canonical IP feed must be rejected: ${endpoint}`);
}
for (const endpoint of [
  "https://localhost./latest.json",
  "https://updates.localhost./latest.json",
  "https://example.com./latest.json",
  "https://updates.example.net/latest.json",
  "https://updates.example.net./latest.json",
]) {
  const result = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: [endpoint] } } }, { privateKeyPresent: true, elevatedRunnerReady: true });
  assert.ok(result.errors.some((error) => error.includes("Reserved/example updater host")), `reserved trailing-dot host must be rejected: ${endpoint}`);
}
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: ["https://user:pass@updates.aerotune.net/latest.json"] } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("credentials")));
const sensitiveEndpoint = "http://user:supersecret@updates.aerotune.net/latest.json?token=supersecret";
const sensitiveEndpointResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: [sensitiveEndpoint] } } }, { privateKeyPresent: true, elevatedRunnerReady: true });
assert.ok(sensitiveEndpointResult.errors.some((error) => error.includes("HTTPS")));
assert.ok(sensitiveEndpointResult.errors.every((error) => !error.includes("supersecret") && !error.includes("token=")));
for (const [endpoint, expectedError] of [
  ["https://updates.aerotune.net:8443/latest.json", "default HTTPS port"],
  ["https://updates.aerotune.net/latest.json?token=secret", "query strings or fragments"],
  ["https://updates.aerotune.net/latest.json#release", "query strings or fragments"],
  ["https://updates/latest.json", "fully-qualified DNS hostname"],
  ["https://updates.aerotune.local/latest.json", "Reserved/example updater host"],
  ["https://updates.aerotune.corp/latest.json", "Reserved/example updater host"],
  ["https://updates.aerotune.lan/latest.json", "Reserved/example updater host"],
  ["https://updates.aerotune.intranet/latest.json", "Reserved/example updater host"],
  ["https://updates.aerotune.private/latest.json", "Reserved/example updater host"],
  ["https://updates.aerotune.localdomain/latest.json", "Reserved/example updater host"],
  ["https://updates.aerotune.net./latest.json", "trailing dot"],
]) {
  const result = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, endpoints: [endpoint] } } }, { privateKeyPresent: true, elevatedRunnerReady: true });
  assert.ok(result.errors.some((error) => error.includes(expectedError)), `unsafe feed URL must be rejected (${expectedError}): ${endpoint}`);
}
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, dangerousAcceptInvalidCerts: true } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("dangerousAcceptInvalidCerts")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, pubkey: "REPLACE_WITH_PUBLIC_KEY" } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("public key")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, plugins: { updater: { ...overlay.plugins.updater, windows: { installMode: "passive" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("quiet")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { nsis: { installMode: "currentUser" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("perMachine")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { nsis: { "install-mode": "currentUser" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias install-mode")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { nsis: {} } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("explicitly preserve installMode perMachine")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { nsis: { installMode: "perMachine", template: "custom.nsi" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("template must remain unset")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { nsis: { installMode: "perMachine", installerHooks: "custom.nsh" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("installerHooks must remain unset")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { nsis: { installMode: "perMachine", "installer-hooks": "custom.nsh" } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("serde alias installer-hooks")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { nsis: { installMode: "perMachine", customLanguageFiles: { English: "custom.nsh" } } } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("customLanguageFiles must remain unset")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, windows: { allowDowngrades: true } } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("bundle.windows.allowDowngrades")));
assert.ok(validateUpdaterReleaseConfig({ ...config, bundle: { ...config.bundle, windows: { ...config.bundle.windows, allowDowngrades: true } } }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("bundle.windows.allowDowngrades")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, targets: ["msi"] } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("only the NSIS")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, targets: "nsis" } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("only the NSIS")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, targets: "all" } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("only the NSIS")));
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, bundle: { ...overlay.bundle, targets: ["nsis", "msi"] } }, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("only the NSIS")));
const overlayMissingTargets = { ...overlay, bundle: { ...overlay.bundle } };
delete overlayMissingTargets.bundle.targets;
assert.ok(validateUpdaterReleaseConfig(config, overlayMissingTargets, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("explicitly target only the NSIS")));
assert.ok(validateUpdaterReleaseConfig({ ...config, bundle: { ...config.bundle, targets: "all" } }, overlay, { privateKeyPresent: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("base bundle.targets must be exactly")));
assert.ok(validateUpdaterReleaseConfig(config, overlay, { privateKeyPresent: true, runtimeEndpoint: "https://wrong.aerotune.net/feed.json", runtimePublicKey: overlay.plugins.updater.pubkey, pipelineReady: true, elevatedRunnerReady: true }).errors.some((error) => error.includes("AEROTUNE_UPDATE_ENDPOINT")));
const hardOffResult = validateUpdaterReleaseConfig(config, overlay, {
  privateKeyPresent: true,
  runtimeEndpoint: overlay.plugins.updater.endpoints[0],
  runtimePublicKey: overlay.plugins.updater.pubkey,
  pipelineReady: false,
  elevatedRunnerReady: false,
});
assert.ok(hardOffResult.errors.some((error) => error.includes("Private signing-key secret-store/file ACL protection has not been audited")));
assert.ok(hardOffResult.errors.some((error) => error.includes("code-level updater gate remains disabled")));
assert.ok(hardOffResult.errors.some((error) => error.includes("runner/task")));
assert.ok(hardOffResult.errors.some((error) => error.includes("vendored 256 KiB feed-manifest cap is a prototype")));
assert.ok(hardOffResult.errors.some((error) => error.includes("Direct-route DNS destination pinning")));
assert.ok(hardOffResult.errors.some((error) => error.includes("selected policy preserves Windows system proxy")));
assert.ok(hardOffResult.errors.some((error) => error.includes("Reviewed artifact byte caps, a release-ready bounded downloader, exact-byte signature verification")));
const validReleaseInputs = {
  privateKeyPresent: true,
  signingKeyProtectionReady: true,
  runtimeEndpoint: overlay.plugins.updater.endpoints[0],
  runtimePublicKey: overlay.plugins.updater.pubkey,
  pipelineReady: true,
  feedResponseBoundedReady: true,
  dnsDestinationPolicyReady: true,
  proxyPolicyReady: true,
  boundedDownloadReady: true,
  elevatedRunnerReady: true,
};
const feedUnreadyResult = validateUpdaterReleaseConfig(config, overlay, { ...validReleaseInputs, feedResponseBoundedReady: false });
assert.ok(feedUnreadyResult.errors.some((error) => error.includes("vendored 256 KiB feed-manifest cap is a prototype")));
const dnsUnreadyResult = validateUpdaterReleaseConfig(config, overlay, { ...validReleaseInputs, dnsDestinationPolicyReady: false });
assert.ok(dnsUnreadyResult.errors.some((error) => error.includes("Direct-route DNS destination pinning")));
const proxyUnreadyResult = validateUpdaterReleaseConfig(config, overlay, { ...validReleaseInputs, proxyPolicyReady: false });
assert.ok(proxyUnreadyResult.errors.some((error) => error.includes("selected policy preserves Windows system proxy")));
const insecureTransportResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, dangerousInsecureTransportProtocol: true } } }, validReleaseInputs);
assert.ok(insecureTransportResult.errors.some((error) => error.includes("dangerousInsecureTransportProtocol must remain disabled")));
const invalidTlsResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, dangerousAcceptInvalidCerts: true, dangerousAcceptInvalidHostnames: true } } }, validReleaseInputs);
assert.ok(invalidTlsResult.errors.some((error) => error.includes("dangerousAcceptInvalidCerts and dangerousAcceptInvalidHostnames")));
const nullTransportOverrideResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, dangerousInsecureTransportProtocol: null } } }, validReleaseInputs);
assert.ok(nullTransportOverrideResult.errors.some((error) => error.includes("dangerousInsecureTransportProtocol must remain disabled")));
const nullTlsOverrideResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, dangerousAcceptInvalidCerts: null } } }, validReleaseInputs);
assert.ok(nullTlsOverrideResult.errors.some((error) => error.includes("dangerousAcceptInvalidCerts and dangerousAcceptInvalidHostnames")));
const noProxyOverrideResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, noProxy: true } } }, validReleaseInputs);
assert.ok(noProxyOverrideResult.errors.some((error) => error.includes("system-proxy policy forbids noProxy overrides")));
const nullNoProxyOverrideResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, noProxy: null } } }, validReleaseInputs);
assert.ok(nullNoProxyOverrideResult.errors.some((error) => error.includes("system-proxy policy forbids noProxy overrides")));
const explicitProxyResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, proxy: "http://proxy.example.net:8080" } } }, validReleaseInputs);
assert.ok(explicitProxyResult.errors.some((error) => error.includes("Explicit updater proxy overrides require a separately reviewed proxy policy")));
const installerArgumentResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, windows: { ...overlay.plugins.updater.windows, installerArgs: ["/D=C:\\Temp"] } } } }, validReleaseInputs);
assert.ok(installerArgumentResult.errors.some((error) => error.includes("installerArgs must remain an empty array")));
const nullInstallerArgumentResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, windows: { ...overlay.plugins.updater.windows, installerArgs: null } } } }, validReleaseInputs);
assert.ok(nullInstallerArgumentResult.errors.some((error) => error.includes("installerArgs must remain an empty array")));
const installModeOverrideResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, windows: { ...overlay.plugins.updater.windows, installMode: "passive" } } } }, validReleaseInputs);
assert.ok(installModeOverrideResult.errors.some((error) => error.includes("installMode must remain quiet")));
const nullWindowsOverrideResult = validateUpdaterReleaseConfig(config, { ...overlay, plugins: { ...overlay.plugins, updater: { ...overlay.plugins.updater, windows: null } } }, validReleaseInputs);
assert.ok(nullWindowsOverrideResult.errors.some((error) => error.includes("windows must be an object")));
const boundedDownloadUnreadyResult = validateUpdaterReleaseConfig(config, overlay, { ...validReleaseInputs, boundedDownloadReady: false });
assert.ok(boundedDownloadUnreadyResult.errors.some((error) => error.includes("Reviewed artifact byte caps, a release-ready bounded downloader, exact-byte signature verification")));
assert.deepEqual(validateUpdaterReleaseConfig(config, overlay, validReleaseInputs).errors, []);
assert.ok(validateUpdaterReleaseConfig(config, { ...overlay, app: { security: { capabilities: [{ identifier: "unsafe", permissions: ["updater:default"] }] } } }, validReleaseInputs).errors.some((error) => error.includes("app.security.capabilities")));
assert.ok(validateUpdaterReleaseConfig(config, overlay, { ...validReleaseInputs, capabilityFiles: [{ path: "test.json", content: '{"permissions":["updater:allow-install"]}' }] }).errors.some((error) => error.includes("test.json")));
console.log("PASS updater remains disabled by default and signed per-machine release checks fail closed");
