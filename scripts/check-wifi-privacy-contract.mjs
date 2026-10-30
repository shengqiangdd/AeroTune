import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const ui = read("src/pages/Network.tsx");
const api = read("src/lib/api.ts");
const rust = read("src-tauri/src/network.rs");

const wifiType = api.match(/export interface WifiProfile \{([\s\S]*?)\n\}/)?.[1];
assert.ok(wifiType, "frontend must define the saved Wi-Fi metadata contract");
assert.doesNotMatch(wifiType, /password/i, "frontend Wi-Fi metadata must not carry credentials");
assert.doesNotMatch(ui, /shownPw|w\.password|KeyRound/, "network UI must not retain or reveal saved Wi-Fi secrets");
assert.doesNotMatch(ui, /useEffect\(\s*\(\)\s*=>\s*\{\s*loadWifi\(\);/, "saved profile metadata must not be fetched automatically on page entry");
assert.match(ui, /onClick=\{loadWifi\}/, "reading saved profile metadata must require an explicit user action");
assert.match(ui, /不读取、不显示或上传 Wi-Fi 密码/, "UI must disclose the credential boundary before reading profile metadata");

const wifiCommand = rust.match(/pub fn list_wifi_profiles\(\)[\s\S]*?let script = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(wifiCommand, "backend must keep the WLAN metadata query scoped to its command");
assert.match(wifiCommand, /netsh wlan export profile folder=/);
assert.doesNotMatch(wifiCommand, /key\s*=\s*clear/i, "metadata query must never export plaintext Wi-Fi keys");
assert.doesNotMatch(wifiCommand, /sharedKey|password/i, "metadata query must not parse secret fields");
assert.match(wifiCommand, /finally\s*\{\s*Remove-Item[\s\S]*?-Recurse[\s\S]*?-Force/, "temporary metadata exports must be cleaned in a finally block");
assert.match(rust, /wifi_parser_maps_metadata_without_retaining_credential_fields/);

console.log("PASS saved Wi-Fi profile metadata is explicit, plaintext-key-free, and not retained in frontend state");
