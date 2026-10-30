import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const rust = read("../src-tauri/src/disk_analyzer.rs");
const api = read("../src/lib/api.ts");
const cleaner = read("../src/pages/Cleaner.tsx");
const duplicateUi = read("../src/components/DuplicateFinderTab.tsx");
const capabilities = read("../src-tauri/capabilities/default.json");

assert.match(rust, /pub enum DiskScanPreset\s*\{\s*UserProfile,\s*SystemDrive,/);
assert.match(rust, /#\[serde\(rename_all = "snake_case"\)\]/);
assert.match(rust, /pub async fn scan_disk\(preset: DiskScanPreset, app: AppHandle\)/);
assert.doesNotMatch(rust, /pub async fn scan_disk\(root:\s*String/);
assert.match(rust, /DiskScanPreset::UserProfile[\s\S]*?std::env::var\("USERPROFILE"\)/);
assert.match(rust, /DiskScanPreset::SystemDrive[\s\S]*?std::env::var\("SystemDrive"\)/);

assert.match(rust, /pub async fn pick_duplicate_scan_root\(app: AppHandle\)[\s\S]*?blocking_pick_folder\(\)[\s\S]*?register_duplicate_root_selection\(&path\)/);
assert.match(rust, /pub async fn scan_duplicate_files\(\s*root_token: String,/);
assert.doesNotMatch(rust, /pub async fn scan_duplicate_files\(\s*root:\s*String/);
assert.match(rust, /resolve_duplicate_root_selection\(&root_token\)/);
assert.match(rust, /const MAX_DUPLICATE_ROOT_SELECTIONS: usize = 4/);
assert.match(rust, /const DUPLICATE_ROOT_SELECTION_TTL: Duration = Duration::from_secs\(30 \* 60\)/);
assert.match(rust, /getrandom::fill\(&mut bytes\)/);
assert.match(rust, /bytes\[6\] = \(bytes\[6\] & 0x0f\) \| 0x40/);
assert.match(rust, /Uuid::from_bytes\(bytes\)/);
assert.match(rust, /pub root_token: String/);
assert.match(rust, /duplicate_root_selection_is_canonical_opaque_and_reusable_only_by_token/);
assert.match(rust, /duplicate_root_selection_expiry_and_disk_presets_fail_closed/);

assert.match(api, /scanDisk: \(preset: DiskScanPreset\) => invoke<DiskScanResult>\("scan_disk", \{ preset \}\)/);
assert.match(api, /pickDuplicateScanRoot: \(\) => invoke<string \| null>\("pick_duplicate_scan_root"\)/);
assert.match(api, /scanDuplicateFiles: \(rootToken: string\) =>[\s\S]*invoke<DuplicateScanResult>\("scan_duplicate_files", \{ rootToken \}\)/);
assert.match(cleaner, /startScan\("user_profile"\)/);
assert.match(cleaner, /startScan\("system_drive"\)/);
assert.match(cleaner, /const rootToRescan = scanPreset/);
assert.match(duplicateUi, /const selectionToken = await api\.pickDuplicateScanRoot\(\)/);
assert.match(duplicateUi, /api\.scanDuplicateFiles\(selectionToken\)/);
assert.match(duplicateUi, /const rootToRescan = result\.root_token/);
assert.match(duplicateUi, /await scan\(rootToRescan\)/);
assert.doesNotMatch(duplicateUi, /@tauri-apps\/plugin-dialog|api\.scanDuplicateFiles\(folder\)/);
assert.doesNotMatch(capabilities, /dialog:allow-open/);

console.log("PASS scan roots are backend-selected and duplicate rescans use bounded opaque handles");
