import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";

const api = readFileSync(new URL("../src/lib/api.ts", import.meta.url), "utf8");
const libPath = new URL("../src-tauri/src/lib.rs", import.meta.url);
const libBytes = readFileSync(libPath);
const lib = libBytes.toString("utf8");
const provenance = readFileSync(new URL("../docs/LIB_RS_PROVENANCE_REVIEW.md", import.meta.url), "utf8");
const localFingerprint = createHash("sha256").update(libBytes).digest("hex").toUpperCase();
const recordedCheckpoint = provenance.match(
  /Current local checkpoint: \*\*SHA-256 `([A-F0-9]{64})`\*\*, (\d+) bytes\./,
);
assert.ok(recordedCheckpoint, "the provenance review must record the local SHA-256 checkpoint and byte size");
const documentedCheckpoint = recordedCheckpoint[1];
const documentedBytes = Number(recordedCheckpoint[2]);
const currentBytes = libBytes.byteLength;
assert.equal(
  localFingerprint,
  documentedCheckpoint,
  [
    "lib.rs integrity checkpoint mismatch; release must remain blocked.",
    `Documented local SHA-256: ${documentedCheckpoint}`,
    `Documented checkpoint size: ${documentedBytes} bytes`,
    `Current src-tauri/src/lib.rs SHA-256: ${localFingerprint}`,
    `Current src-tauri/src/lib.rs size: ${currentBytes} bytes`,
    "Do not replace the hash as a workaround; audit the current source and permissions, obtain independent review and explicit user sign-off, then update the checkpoint only if approved.",
  ].join("\n"),
);
const handler = lib.match(/\.invoke_handler\(tauri::generate_handler!\[([\s\S]*?)\]\)/)?.[1];
assert.ok(handler, "the ordinary app must define one explicit Tauri command allowlist");

const frontendCommands = [...new Set(
  [...api.matchAll(/\binvoke(?:<[^\n]*?>)?\(\s*["']([a-z][a-z0-9_]*)["']/g)].map((match) => match[1]),
)].sort();
const registeredCommands = [...new Set(
  [...handler.matchAll(/\b[a-z_]+::([a-z_]+)\b/g)].map((match) => match[1]),
)].sort();

assert.deepEqual(
  registeredCommands,
  frontendCommands,
  `Tauri API/handler mismatch; missing: ${frontendCommands.filter((name) => !registeredCommands.includes(name)).join(", ")}; unused: ${registeredCommands.filter((name) => !frontendCommands.includes(name)).join(", ")}`,
);
assert.match(lib, /pub fn try_acquire_app_update_gate\(\)/);
assert.equal(
  [...lib.matchAll(/tauri_plugin_updater::Builder::new\(\)\.build\(\)/g)].length,
  1,
  "the standard updater plugin must be registered exactly once for the Rust IPC façade",
);
const capabilities = JSON.parse(readFileSync(new URL("../src-tauri/capabilities/default.json", import.meta.url), "utf8"));
assert.equal(
  capabilities.permissions.some((permission) => permission.startsWith("updater:")),
  false,
  "the webview must not receive direct JavaScript updater-plugin permissions",
);
assert.doesNotMatch(lib, /run_update_only|UPDATE_ONLY_ARGUMENT/, "the ordinary app must not expose the legacy helper entrypoint");

console.log(`PASS ${frontendCommands.length} frontend IPC commands match the explicit Rust handler allowlist`);
