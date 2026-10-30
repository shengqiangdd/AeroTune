import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const capability = JSON.parse(read("../src-tauri/capabilities/default.json"));
const permissions = capability.permissions;
const fsPermissions = permissions.filter((permission) =>
  typeof permission === "string" ? permission.startsWith("fs:") : permission.identifier?.startsWith("fs:"));
assert.deepEqual(fsPermissions, [], "the main WebView must not receive filesystem plugin commands or path scopes");
assert.ok(!permissions.includes("dialog:allow-open"), "renderer must not receive dialog-based filesystem selection commands");
assert.ok(!permissions.includes("dialog:allow-save"), "exports must not use renderer-side Save and filesystem scopes");

function sourceFiles(directory) {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sourceFiles(path);
    return /\.(?:[cm]?[jt]sx?)$/.test(entry.name) ? [path] : [];
  });
}

const srcRoot = fileURLToPath(new URL("../src", import.meta.url));
let exportCallCount = 0;
for (const path of sourceFiles(srcRoot)) {
  const source = readFileSync(path, "utf8");
  assert.doesNotMatch(source, /@tauri-apps\/plugin-fs|@tauri-apps\/plugin-dialog|\b(?:readTextFile|writeTextFile)\s*\(/, `${path} must not use renderer filesystem/dialog APIs`);
  assert.doesNotMatch(source, /\bawait\s+save\s*\(/, `${path} must not open a renderer-side Save dialog`);
  exportCallCount += [...source.matchAll(/api\.saveTextExport\s*\(/g)].length;
}
assert.equal(exportCallCount, 7, "review all frontend export call sites when this boundary changes");

const api = read("../src/lib/api.ts");
assert.match(api, /saveTextExport:[\s\S]{0,120}invoke<boolean>\("save_text_export"/);
const backend = read("../src-tauri/src/admin.rs");
assert.match(backend, /const MAX_TEXT_EXPORT_BYTES: usize = 32 \* 1024 \* 1024/);
assert.match(backend, /fn validate_text_export_request\([\s\S]{0,160}filename: &str,[\s\S]{0,80}content_bytes: usize/);
assert.match(backend, /pub async fn save_text_export\(\s*app: AppHandle,\s*filename: String,\s*contents: String,/);
assert.match(backend, /\.blocking_save_file\(\)[\s\S]{0,180}\.transpose\(\)/);
assert.match(backend, /crate::util::atomic_write\(&path, contents\.as_bytes\(\)\)/);
assert.doesNotMatch(backend, /save_text_export\([\s\S]{0,120}path:\s*String/);

const tauriLib = read("../src-tauri/src/lib.rs");
assert.match(tauriLib, /admin::save_text_export/);
assert.doesNotMatch(tauriLib, /tauri_plugin_fs::init\(\)/, "the filesystem plugin must not be registered for the WebView");
assert.doesNotMatch(read("../src-tauri/Cargo.toml"), /^tauri-plugin-fs\s*=/m, "do not keep a direct Rust filesystem-plugin dependency");
assert.doesNotMatch(read("../package.json"), /@tauri-apps\/plugin-fs/, "do not ship the renderer filesystem API");
assert.doesNotMatch(read("../package.json"), /@tauri-apps\/plugin-dialog/, "do not ship renderer-side dialog APIs");
assert.doesNotMatch(read("../pnpm-lock.yaml"), /@tauri-apps\/plugin-dialog/, "do not retain an unused pnpm dialog dependency");
const diskAnalyzer = read("../src-tauri/src/disk_analyzer.rs");
assert.match(diskAnalyzer, /pub async fn pick_duplicate_scan_root[\s\S]*?blocking_pick_folder\(\)/);
assert.doesNotMatch(diskAnalyzer, /pub async fn scan_duplicate_files\(\s*root:\s*String/);

console.log(`PASS renderer has no filesystem/dialog IPC; all ${exportCallCount} exports use the bounded Rust Save-dialog writer`);
