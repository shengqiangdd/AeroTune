import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const drivers = read("src-tauri/src/drivers.rs");
const util = read("src-tauri/src/util.rs");
const lib = read("src-tauri/src/lib.rs");
const api = read("src/lib/api.ts");
const card = read("src/components/StaleDriversCard.tsx");
const cleaner = read("src/pages/Cleaner.tsx");
const capabilities = JSON.parse(read("src-tauri/capabilities/default.json"));

const deleteBody = drivers.split("pub fn delete_stale_driver")[1]?.split("#[cfg(test)]")[0] ?? "";
assert.ok(deleteBody, "the delete command must exist");

// Deleting is admin-gated, validated, re-derived from a fresh listing, never forced, and journaled as irreversible.
assert.match(deleteBody, /if !crate::util::is_admin\(\)/);
assert.match(deleteBody, /is_published_inf_name\(&published\)/);
assert.match(deleteBody, /find_superseded\(&packages\)/, "the target must be re-checked against a fresh listing");
assert.match(deleteBody, /"\/delete-driver"/);
assert.doesNotMatch(deleteBody, /\/force|\/uninstall|\/subdirs|\/install/, "pnputil must not be forced or asked to uninstall devices");
assert.match(deleteBody, /Revert::None/, "the deletion must be journaled as non-revertible");
assert.match(deleteBody, /不可还原/, "the journal text must say the deletion cannot be undone");
// Only the two documented pnputil verbs may be issued anywhere in the module.
const verbs = [...drivers.matchAll(/&\[\s*"(\/[a-z-]+)"/g)].map((match) => match[1]).sort();
assert.deepEqual(verbs, ["/delete-driver", "/enum-drivers"]);
assert.match(drivers, /"\/format",\s*"xml"/, "the listing must use the language-independent XML format");

// The helper is an allowlisted system tool resolved from the Windows directory, never from PATH.
assert.match(util, /"pnputil" \| "pnputil\.exe" => windows_system_directory\(\)\?/);
assert.match(util, /"pnputil" \| "pnputil\.exe" => "pnputil\.exe"/);

// IPC surface: exactly these two commands are registered and exposed, with no new permissions.
assert.match(lib, /^mod drivers;$/m);
assert.match(lib, /drivers::list_stale_drivers,/);
assert.match(lib, /drivers::delete_stale_driver,/);
assert.match(api, /listStaleDrivers: \(\) => invoke<StaleDriverReport>\("list_stale_drivers"\)/);
assert.match(api, /deleteStaleDriver: \(published: string\) => invoke<void>\("delete_stale_driver", \{ published \}\)/);
assert.equal(capabilities.permissions.some((permission) => /driver|pnputil|shell/i.test(permission)), false);

// UI: sizes can be unknown, the user must confirm, the irreversibility is stated, and nothing auto-deletes.
assert.match(api, /size_mb: number \| null/);
assert.match(card, /sizeMb === null \? "大小未知"/);
// The confirmation must gate the delete call: confirm -> early return when declined -> only then delete.
assert.match(
  card,
  /const confirmed = confirm\([\s\S]*?\);\s*if \(!confirmed\) return;\s*setBusy\(driver\.published\);\s*try \{\s*await api\.deleteStaleDriver\(driver\.published\);/,
  "deleteStaleDriver must only run after the user confirms",
);
assert.equal([...card.matchAll(/api\.deleteStaleDriver\(/g)].length, 1, "there must be exactly one delete call site");
assert.match(card, /无法在 AeroTune 中还原/);
assert.match(card, /仍被设备使用的驱动包会被 Windows 拒绝删除/);
assert.match(card, /disabled=\{busy !== null \|\| loading\}/);
assert.doesNotMatch(card, /useEffect\([^)]*deleteStaleDriver/s, "nothing may delete on mount");
assert.match(cleaner, /<StaleDriversCard \/>/);
assert.match(drivers, /fn the_delete_path_never_forces_and_only_accepts_published_names/);
assert.match(drivers, /fn the_same_version_with_a_different_date_is_not_a_replacement/);

console.log("PASS superseded driver packages: pnputil is allowlisted, admin-gated, never forced, listing-verified, irreversible-and-labeled, UI-confirmed");
