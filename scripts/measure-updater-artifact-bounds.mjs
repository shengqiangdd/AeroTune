import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

const cargoToml = readFileSync("src-tauri/Cargo.toml", "utf8");
const version = cargoToml.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version) throw new Error("Cannot determine Cargo package version.");
const defaultArchivePath = `src-tauri/target/release/bundle/nsis/AeroTune_${version}_x64-setup.nsis.zip`;
const artifactPath = resolve(process.argv[2] ?? defaultArchivePath);
const compressedCap = Number(process.env.AEROTUNE_REVIEWED_COMPRESSED_CAP_BYTES);
const expandedCap = Number(process.env.AEROTUNE_REVIEWED_EXPANDED_CAP_BYTES);
const updaterSource = readFileSync("src-tauri/src/updater.rs", "utf8");

function fail(message) {
  console.error(`FAIL: ${message}`);
  process.exit(1);
}
function sourceCap(name) {
  const match = updaterSource.match(new RegExp(`const ${name}: (?:usize|u64) = (\\d+) \\* 1024 \\* 1024;`));
  return match ? Number(match[1]) * 1024 * 1024 : null;
}

let metadata;
try {
  metadata = lstatSync(artifactPath);
} catch {
  fail(`Tauri updater ZIP not found: ${artifactPath}`);
}
if (metadata.isSymbolicLink() || !metadata.isFile()) fail("artifact path must be a regular non-symlink file");
if (!Number.isSafeInteger(compressedCap) || compressedCap < 1) fail("AEROTUNE_REVIEWED_COMPRESSED_CAP_BYTES must be a positive byte cap");
if (!Number.isSafeInteger(expandedCap) || expandedCap < 1) fail("AEROTUNE_REVIEWED_EXPANDED_CAP_BYTES must be a positive byte cap");
if (compressedCap !== sourceCap("PROPOSED_MAX_UPDATE_ARTIFACT_BYTES")) fail("compressed cap must exactly match the Rust updater source limit");
if (expandedCap !== sourceCap("PROPOSED_MAX_UPDATE_EXPANDED_BYTES")) fail("expanded cap must exactly match the Rust updater source limit");
if (metadata.size > compressedCap) fail(`compressed artifact size ${metadata.size} exceeds cap ${compressedCap}`);

const signaturePath = `${artifactPath}.sig`;
const signatureStat = lstatSync(signaturePath);
if (signatureStat.isSymbolicLink() || !signatureStat.isFile() || signatureStat.size === 0) fail("signed updater .sig file must be a non-empty regular file");
const encodedSignature = readFileSync(signaturePath, "utf8").trim();
if (!encodedSignature) fail("updater .sig is empty");
const decodedSignature = Buffer.from(encodedSignature, "base64");
if (decodedSignature.toString("base64") !== encodedSignature) fail("updater .sig is not canonical base64");
let signatureText;
try {
  signatureText = new TextDecoder("utf-8", { fatal: true }).decode(decodedSignature);
} catch {
  fail("decoded updater .sig is not valid UTF-8");
}
if (!signatureText.startsWith("untrusted comment:") || !signatureText.includes("trusted comment:")) fail("updater .sig does not decode to a Tauri Minisign signature");
const publicKey = process.env.AEROTUNE_UPDATE_PUBLIC_KEY?.trim();
if (!publicKey) fail("AEROTUNE_UPDATE_PUBLIC_KEY is required to verify the signed release artifact");
const verification = spawnSync("cargo", [
  "run", "--locked", "--manifest-path", "src-tauri/Cargo.toml", "--example", "verify-updater-signature", "--release", "--", artifactPath, version,
], { stdio: "inherit", env: process.env });
if (verification.error) fail(`cannot launch Minisign verification (${verification.error.message})`);
if (verification.status !== 0) fail("updater archive signature or signed app version does not match the configured public key and release version");

const bytes = readFileSync(artifactPath);
const minEocd = Math.max(0, bytes.length - (22 + 0xffff));
let eocd = -1;
for (let offset = bytes.length - 22; offset >= minEocd; offset -= 1) {
  if (bytes.readUInt32LE(offset) === 0x06054b50) {
    const commentLength = bytes.readUInt16LE(offset + 20);
    if (offset + 22 + commentLength === bytes.length) {
      eocd = offset;
      break;
    }
  }
}
if (eocd < 0) fail("ZIP end-of-central-directory record is missing or malformed");
const diskNumber = bytes.readUInt16LE(eocd + 4);
const centralDisk = bytes.readUInt16LE(eocd + 6);
const entriesOnDisk = bytes.readUInt16LE(eocd + 8);
const entryCount = bytes.readUInt16LE(eocd + 10);
const centralSize = bytes.readUInt32LE(eocd + 12);
const centralOffset = bytes.readUInt32LE(eocd + 16);
if (diskNumber !== 0 || centralDisk !== 0 || entriesOnDisk !== 1 || entryCount !== 1) fail("updater ZIP must be a single-disk archive with exactly one entry");
if (centralOffset + centralSize !== eocd || centralOffset + 46 > eocd) fail("ZIP central directory bounds are inconsistent");
if (bytes.readUInt32LE(centralOffset) !== 0x02014b50) fail("ZIP central directory header is invalid");
const flags = bytes.readUInt16LE(centralOffset + 8);
const compressionMethod = bytes.readUInt16LE(centralOffset + 10);
const compressedEntryBytes = bytes.readUInt32LE(centralOffset + 20);
const expandedEntryBytes = bytes.readUInt32LE(centralOffset + 24);
const filenameLength = bytes.readUInt16LE(centralOffset + 28);
const extraLength = bytes.readUInt16LE(centralOffset + 30);
const commentLength = bytes.readUInt16LE(centralOffset + 32);
const diskStart = bytes.readUInt16LE(centralOffset + 34);
const externalAttributes = bytes.readUInt32LE(centralOffset + 38);
const localHeaderOffset = bytes.readUInt32LE(centralOffset + 42);
if ((flags & 1) !== 0 || diskStart !== 0 || compressedEntryBytes === 0xffffffff || expandedEntryBytes === 0xffffffff) fail("encrypted, multi-disk, or ZIP64 updater entries are not accepted by this size measurement");
if (compressionMethod !== 0 && compressionMethod !== 8) fail("unexpected ZIP compression method");
if (centralOffset + 46 + filenameLength + extraLength + commentLength !== eocd) fail("ZIP central directory entry lengths are inconsistent");
if (localHeaderOffset + 30 > centralOffset || bytes.readUInt32LE(localHeaderOffset) !== 0x04034b50) fail("ZIP local file header is invalid");
const filename = new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(centralOffset + 46, centralOffset + 46 + filenameLength));
if (!/^AeroTune_.*_x64-setup\.exe$/i.test(filename) || filename.includes("/") || filename.includes("\\")) fail("ZIP must contain exactly one root-level AeroTune x64 NSIS setup executable");
if (((externalAttributes >>> 16) & 0xf000) === 0xa000) fail("symbolic-link updater entries are not accepted");
if (expandedEntryBytes > expandedCap) fail(`expanded archive size ${expandedEntryBytes} exceeds cap ${expandedCap}`);
if (compressedEntryBytes > compressedCap) fail(`compressed entry size ${compressedEntryBytes} exceeds cap ${compressedCap}`);

const digest = createHash("sha256").update(bytes).digest("hex");
const releaseBlockers = readFileSync("docs/UPDATER_RELEASE_BLOCKERS.md", "utf8");
assert.match(releaseBlockers, /artifact-specific measurements and limits have not been reviewed against a signed Windows release/);
console.log(JSON.stringify({
  artifact: artifactPath,
  artifactBytes: metadata.size,
  compressedEntryBytes,
  expandedEntryBytes,
  sha256: digest,
  compressedCapBytes: compressedCap,
  expandedCapBytes: expandedCap,
  signaturePresent: true,
  signatureVerified: true,
  signedVersion: version,
  note: "Minisign and signed-version verification passed against the configured public key; artifact-size review and local Windows acceptance remain separate gates.",
}, null, 2));
