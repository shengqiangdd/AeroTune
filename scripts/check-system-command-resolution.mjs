import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const util = readFileSync(new URL("../src-tauri/src/util.rs", import.meta.url), "utf8");
const resolver = util.match(/fn resolve_executable\(program: &str\)[\s\S]*?\n\}/)?.[0];
assert.ok(resolver, "command runner must resolve trusted Windows tools explicitly");
assert.match(util, /GetSystemDirectoryW/);
assert.match(util, /FILE_ATTRIBUTE_REPARSE_POINT/);
assert.match(resolver, /verify_system_executable/);
assert.match(resolver, /"takeown"[\s\S]*"icacls"[\s\S]*"cmd"/);
assert.match(resolver, /"powercfg"[\s\S]*"ipconfig"[\s\S]*"defrag"[\s\S]*"schtasks"[\s\S]*"dism"[\s\S]*"compact"[\s\S]*"vssadmin"/);
assert.match(resolver, /WindowsPowerShell[\s\S]*powershell\.exe/);
assert.match(resolver, /拒绝通过 PATH 启动未审核的命令/);
assert.doesNotMatch(resolver, /SystemRoot|Ok\(program\.into\(\)\)/);
const commandRunner = util.match(/fn command_output_with_timeout\([\s\S]*?\n\}/)?.[0];
assert.ok(commandRunner);
assert.match(commandRunner, /Command::new\(resolve_executable\(program\)\?\)/);
assert.match(util, /system_tools_resolve_to_verified_windows_system_paths/);

console.log("PASS fixed Windows system tools launch from verified absolute system paths");
