import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const admin = read("../src-tauri/src/admin.rs");
const startup = read("../src/pages/Startup.tsx");
const settings = read("../src/pages/Settings.tsx");

const parser = admin.match(/fn parse_local_open_path\([\s\S]*?\n\}/)?.[0];
assert.ok(parser, "open_path must have a lexical path parser");
assert.match(parser, /bytes\[0\]\.is_ascii_alphabetic\(\)/);
assert.match(parser, /bytes\[1\] != b':'/);
assert.match(parser, /matches!\(bytes\[2\], b'\\\\' \| b'\/'\)/);
assert.match(parser, /path\[2\.\.\]\.contains\(':'\)/);
assert.match(parser, /component, "\." \| "\.\."/);
assert.doesNotMatch(parser, /std::fs::|metadata\(|canonicalize\(|Command::new/);

const resolver = admin.match(/fn resolve_local_open_path\([\s\S]*?\n\}/)?.[0];
assert.ok(resolver, "open_path must resolve validated local targets");
assert.ok(resolver.indexOf("parse_local_open_path(input)?") < resolver.indexOf("GetDriveTypeW("));
assert.ok(resolver.indexOf("GetDriveTypeW(") < resolver.indexOf("symlink_metadata("));
assert.ok(resolver.indexOf("symlink_metadata(") < resolver.indexOf("canonicalize("));
assert.match(resolver, /DRIVE_FIXED \| DRIVE_REMOVABLE \| DRIVE_CDROM \| DRIVE_RAMDISK/);
assert.match(resolver, /FILE_ATTRIBUTE_REPARSE_POINT/);
assert.match(resolver, /metadata\.file_type\(\)\.is_symlink\(\)/);
assert.match(resolver, /!target_metadata\.is_dir\(\) && !target_metadata\.is_file\(\)/);

const command = admin.match(/pub fn open_path\(path: String\)[\s\S]*?\n\}/)?.[0];
assert.ok(command);
assert.ok(command.indexOf("resolve_local_open_path(&path)?") < command.indexOf("Command::new("));
assert.match(command, /let explorer = system_explorer_path\(\)\?/);
assert.match(command, /Command::new\(&explorer\)/);
assert.doesNotMatch(command, /metadata\(&p\)|Command::new\("explorer(?:\.exe)?"\)/);
assert.match(admin, /GetWindowsDirectoryW[\s\S]*join\("explorer\.exe"\)/);
const settingsCommand = admin.match(/pub fn open_settings_page\(page: String\)[\s\S]*?\n\}/)?.[0];
assert.ok(settingsCommand);
assert.match(settingsCommand, /let explorer = system_explorer_path\(\)\?/);
assert.match(settingsCommand, /Command::new\(&explorer\)/);
assert.doesNotMatch(admin, /Command::new\("explorer(?:\.exe)?"\)/);
assert.match(admin, /open_path_rejects_unc_devices_relative_paths_and_ads_lexically/);
assert.match(admin, /open_path_accepts_only_drive_rooted_absolute_local_candidates/);
assert.match(startup, /void api\.openPath\(path\)\.catch\(toastErr\)/);
assert.match(settings, /void api\.openPath\(info\.exe\)\.catch\(toastErr\)/);

console.log("PASS open_path rejects network/device input before filesystem probes and opens only canonical local targets");
