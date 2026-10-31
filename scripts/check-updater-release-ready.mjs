import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import {
  hasConfiguredSigningKey,
  hasUpdaterPermission,
  readCapabilityFiles,
  readUpdaterReleaseConfigFile,
  updaterIsEnabled,
  validateUpdaterReleaseConfig,
} from "./updaterReleasePolicy.mjs";

const baseConfigPath = resolve("src-tauri/tauri.conf.json");
const baseConfig = JSON.parse(readFileSync(baseConfigPath, "utf8"));
const updaterSource = readFileSync("src-tauri/src/updater.rs", "utf8");
const capabilityDirectory = resolve("src-tauri/capabilities");
const capabilityFiles = readCapabilityFiles(capabilityDirectory);
const hasUpdaterCapability = hasUpdaterPermission(baseConfig?.app?.security?.capabilities)
  || capabilityFiles.some((file) => hasUpdaterPermission(file.content));
const overlayPath = process.env.AEROTUNE_UPDATER_CONFIG?.trim();
const required = process.argv.includes("--required");
const acceptanceGates = [
  "UPDATE_FEED_RESPONSE_BOUNDED_READY",
  "UPDATE_PROXY_POLICY_READY",
  "UPDATE_BOUNDED_DOWNLOAD_READY",
  "GITHUB_RELEASE_HOST_POLICY_REVIEWED",
  "GITHUB_RELEASE_WINDOWS_ACCEPTED",
];
const githubUpdaterAcceptanceReady = acceptanceGates.every((name) =>
  new RegExp(`const ${name}: bool = true;`).test(updaterSource),
);
const earlyAccessRisks = [
  "the first live GitHub HTTPS redirect and system-proxy path will be observed during the v1.5.11 acceptance",
  "UAC cancellation, concurrent launch during NSIS, interrupted/file-in-use recovery, and installer repair remain untested; the owner explicitly authorized early access",
];

if (!overlayPath) {
  if (hasUpdaterCapability) {
    console.error("FAIL: updater plugin permissions are present in a capability; the Rust IPC façade must remain the only updater surface.");
    process.exit(1);
  }
  if (updaterIsEnabled(baseConfig)) {
    console.error("FAIL: updater is enabled in the default config; release overlay and security checks are required.");
    process.exit(1);
  }
  if (required) {
    console.error("FAIL: AEROTUNE_UPDATER_CONFIG is required for a signed GitHub-updater release.");
    if (githubUpdaterAcceptanceReady) {
      console.error("Release gates are owner-authorized for early access; residual tests remain:");
      for (const risk of earlyAccessRisks) console.error(`- ${risk}`);
    } else {
      console.error("Required updater acceptance gates are not all approved.");
    }
    process.exit(1);
  }
  console.log("PASS base config remains updater-disabled; a signed release overlay and matching key from the protected GitHub release environment are required.");
  if (githubUpdaterAcceptanceReady) {
    for (const risk of earlyAccessRisks) console.log(`EARLY-ACCESS RISK: ${risk}`);
  } else {
    console.log("Updater release gates remain disabled pending acceptance.");
  }
  process.exit(0);
}

let overlay;
try {
  overlay = readUpdaterReleaseConfigFile(overlayPath);
} catch (error) {
  console.error(`FAIL: cannot read updater release overlay (${error instanceof Error ? error.message : "invalid JSON"}).`);
  process.exit(1);
}

const result = validateUpdaterReleaseConfig(baseConfig, overlay, {
  privateKeyPresent: hasConfiguredSigningKey(process.env),
  runtimeEndpoint: process.env.AEROTUNE_UPDATE_ENDPOINT?.trim(),
  runtimePublicKey: process.env.AEROTUNE_UPDATE_PUBLIC_KEY?.trim(),
  githubUpdaterAcceptanceReady,
  capabilityFiles,
});
for (const warning of result.warnings) console.warn(`WARN: ${warning}`);
if (result.errors.length) {
  for (const error of result.errors) console.error(`FAIL: ${error}`);
  process.exit(1);
}

console.log("PASS GitHub updater release prerequisites validated; signing material was not printed.");
