import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const admin = read("../src-tauri/src/admin.rs");
const lib = read("../src-tauri/src/lib.rs");
const api = read("../src/lib/api.ts");
const settings = read("../src/pages/Settings.tsx");
const capability = read("../src-tauri/capabilities/default.json");
const commandStart = admin.indexOf("pub async fn import_config(");
const commandEnd = admin.indexOf("#[cfg(test)]\nmod import_tests", commandStart);
const command = commandStart < 0 || commandEnd < 0 ? "" : admin.slice(commandStart, commandEnd);

assert.ok(command, "config import must be one Rust-owned operation");
assert.match(command, /app: AppHandle/);
assert.match(command, /blocking_pick_file\(\)/);
assert.match(command, /read_config_import_file\(&path\)/);
assert.match(command, /let Some\(path\) = selected else \{\s*return Ok\(false\);/);
assert.ok(command.indexOf("return Ok(false)") < command.indexOf("parse_imported_config"), "cancel must not parse or apply a config");
assert.match(command, /Ok\(true\)/);
assert.doesNotMatch(command, /path: String/);
assert.doesNotMatch(command, /unwrap_or_default/);
assert.match(admin, /const MAX_CONFIG_IMPORT_BYTES: u64 = 1_048_576/);
assert.match(admin, /path_text\.starts_with\(r"\\\\"\)/);
assert.match(admin, /metadata\.file_type\(\)\.is_symlink\(\)/);
assert.match(admin, /take\(MAX_CONFIG_IMPORT_BYTES \+ 1\)/);
assert.match(admin, /config_picker_reader_accepts_only_bounded_local_json_files/);
assert.match(lib, /admin::import_config/);
assert.doesNotMatch(lib, /admin::pick_config_file/);
assert.match(api, /importConfig: \(\) => invoke<boolean>\("import_config"\)/);
assert.doesNotMatch(api, /pickConfigFile|importConfig: \(path: string\)/);
assert.match(settings, /const imported = await api\.importConfig\(\);[\s\S]*if \(!imported\) return;/);
assert.doesNotMatch(settings, /pickConfigFile\(\)|importConfig\(file\)/);
assert.doesNotMatch(capability, /"dialog:allow-open"/);

console.log("PASS config selection, bounded read, validation, and apply remain within Rust");
