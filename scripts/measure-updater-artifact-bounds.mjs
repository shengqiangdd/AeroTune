import assert from "node:assert/strict";
import { lstatSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const defaultArchivePath = "src-tauri/target/release/bundle/nsis/AeroTune_1.5.9_x64-setup.nsis.zip";
const artifactPath = resolve(process.argv[2] ?? defaultArchivePath);
const compressedCap = Number(process.env.AEROTUNE_REVIEWED_COMPRESSED_CAP_BYTES);
const expandedCap = Number(process.env.AEROTUNE_REVIEWED_EXPANDED_CAP_BYTES);

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}

let metadata;
try {
  metadata = lstatSync(artifactPath);
} catch {
  fail(`signed Tauri updater ZIP not found: ${artifactPath}`);
}
if (metadata.isSymbolicLink() || !metadata.isFile()) fail("artifact path must be a regular non-symlink file");
if (!Number.isSafeInteger(compressedCap) || compressedCap < 1) {
  fail("AEROTUNE_REVIEWED_COMPRESSED_CAP_BYTES must be a positive reviewed decimal byte cap");
}
if (!Number.isSafeInteger(expandedCap) || expandedCap < 1) {
  fail("AEROTUNE_REVIEWED_EXPANDED_CAP_BYTES must be a positive reviewed decimal byte cap");
}
if (metadata.size > compressedCap) fail(`compressed artifact size ${metadata.size} exceeds reviewed cap ${compressedCap}`);

// Deliberately delegate structural/CRC validation to the production Rust validator. This script
// records reproducible file identity/size evidence only; it never claims signature verification.
const bytes = readFileSync(artifactPath);
const digest = await (async () => {
  const { createHash } = await import("node:crypto");
  return createHash("sha256").update(bytes).digest("hex");
})();
const workflow = readFileSync("docs/UPDATER_RELEASE_BLOCKERS.md", "utf8");
assert.match(workflow, /artifact cap evidence is not a signature or VM acceptance/);
console.log(JSON.stringify({
  artifact: artifactPath,
  bytes: metadata.size,
  sha256: digest,
  compressedCapBytes: compressedCap,
  expandedCapBytes: expandedCap,
  signatureVerified: false,
  note: "Record from the signed-release job after the Rust signature and archive validators succeed; this script alone does not verify either.",
}, null, 2));
