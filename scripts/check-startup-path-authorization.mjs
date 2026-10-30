import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const source = readFileSync(new URL("../src-tauri/src/startup.rs", import.meta.url), "utf8");
const userSource = source.indexOf("validated_startup_file(&supplied, &user_root)");
const commonSource = source.indexOf("validated_startup_file(&supplied, &common_root)");
const commonAdminCheck = source.indexOf("!crate::util::is_admin()", commonSource);
const enableSource = source.indexOf("validated_startup_file(&supplied, &disabled_root)");
const restoreTarget = source.indexOf("let user_root = user_startup_dir()", enableSource);
const validatorStart = source.indexOf("fn validated_startup_file(");
const validatorEnd = source.indexOf("fn path_entry_exists(", validatorStart);
const basenameStart = source.indexOf("fn valid_startup_basename(");
const basenameEnd = source.indexOf("/// Accept only a regular", basenameStart);
const validator = validatorStart < 0 || validatorEnd < 0 ? "" : source.slice(validatorStart, validatorEnd);
const basename = basenameStart < 0 || basenameEnd < 0 ? "" : source.slice(basenameStart, basenameEnd);

assert.ok(userSource >= 0 && commonSource > userSource && enableSource > commonSource && restoreTarget > enableSource, "both file-toggle directions must remain explicitly validated");
assert.ok(commonAdminCheck > commonSource && commonAdminCheck < enableSource, "common Startup mutations require an elevated token");
assert.match(validator, /!path\.is_absolute\(\)/);
assert.match(validator, /symlink_metadata\(path\)/);
assert.match(validator, /metadata\.is_file\(\)/);
assert.match(validator, /metadata\.file_type\(\)\.is_symlink\(\)/);
assert.match(validator, /canonical_file\.parent\(\) != Some\(canonical_root\.as_path\(\)\)/);
assert.match(basename, /"lnk" \| "exe" \| "bat"/);
assert.ok(basename.includes("character.is_control()") && basename.includes(":*?") && basename.includes("<>|"), "Windows ADS and invalid filename characters must be rejected");
assert.doesNotMatch(validator, /starts_with\(/, "path containment must not use a vulnerable string prefix");
assert.match(source, /startup_file_validation_requires_a_direct_regular_allowed_child/);
assert.match(source, /restore_source_is_confined_but_keeps_legacy_disabled_names/);

console.log("PASS startup file moves are restricted to canonical direct children of trusted roots");
