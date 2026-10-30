import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { hasConfiguredSigningKey, hasUpdaterPermission, readCapabilityFiles, readUpdaterReleaseConfigFile, updaterIsEnabled, validateUpdaterReleaseConfig } from "./updaterReleasePolicy.mjs";

const baseConfigPath = resolve("src-tauri/tauri.conf.json");
const baseConfig = JSON.parse(readFileSync(baseConfigPath, "utf8"));
const capabilityDirectory = resolve("src-tauri/capabilities");
const capabilityFiles = readCapabilityFiles(capabilityDirectory);
const hasUpdaterCapability = hasUpdaterPermission(baseConfig?.app?.security?.capabilities)
  || capabilityFiles.some((file) => hasUpdaterPermission(file.content));
const overlayPath = process.env.AEROTUNE_UPDATER_CONFIG?.trim();
const required = process.argv.includes("--required");
const disabledPrerequisites = [
  "published HTTPS feed, Tauri public key, and protected private signing key (key-vault/file ACL policy review still required)",
  "review and measurement of the vendored 256 KiB feed-manifest cap; gate remains off",
  "direct-route DNS pinning or a reviewed system-proxy egress allowlist/rebinding defense for feed/artifact hosts",
  "disposable-Windows-VM validation of preserved system-proxy routing, origin-DNS behavior, and fail-closed fallback",
  "reviewed compressed/expanded artifact byte caps, release-ready bounded downloader, exact-byte signature verification, and protected staging",
  "audited fixed elevated runner/task, task DACL/runtime attestation, and registration approval",
  "protected ProgramData provisioner, external recovery supervisor, and crash-safe rollback path",
  "disposable-VM upgrade, interrupted-install, and rollback validation",
  "supported task principal for standard-user sessions (current scaffold is same-admin-account only)",
];

if (!overlayPath) {
  if (hasUpdaterCapability) {
    console.error("FAIL: updater plugin permissions are present in a capability; the normal app must not expose updater commands.");
    process.exit(1);
  }
  if (updaterIsEnabled(baseConfig)) {
    console.error("FAIL: updater is enabled in the default config; release overlay and security checks are required.");
    process.exit(1);
  }
  if (required) {
    console.error("FAIL: AEROTUNE_UPDATER_CONFIG is required for a signed-updater release.");
    console.error("Remaining prerequisites before enabling signed updates:");
    for (const prerequisite of disabledPrerequisites) console.error(`- ${prerequisite}`);
    console.error("See docs/UPDATER.md for the reviewed sequence; do not bypass UAC or task registration gates.");
    process.exit(1);
  }
  console.log("PASS updater remains disabled: signed release configuration, bounded feed/artifact handling, DNS and system-proxy policy, and audited elevated runner are not ready.");
  console.log("Remaining prerequisites:");
  for (const prerequisite of disabledPrerequisites) console.log(`- ${prerequisite}`);
  console.log("See docs/UPDATER.md; update and task-registration gates remain off.");
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
  signingKeyProtectionReady: false,
  runtimeEndpoint: process.env.AEROTUNE_UPDATE_ENDPOINT?.trim(),
  runtimePublicKey: process.env.AEROTUNE_UPDATE_PUBLIC_KEY?.trim(),
  pipelineReady: false,
  feedResponseBoundedReady: false,
  dnsDestinationPolicyReady: false,
  proxyPolicyReady: false,
  boundedDownloadReady: false,
  elevatedRunnerReady: false,
  capabilityFiles,
});
for (const warning of result.warnings) console.warn(`WARN: ${warning}`);
if (result.errors.length) {
  for (const error of result.errors) console.error(`FAIL: ${error}`);
  process.exit(1);
}

console.log("PASS updater release prerequisites validated; signing material was not printed.");
