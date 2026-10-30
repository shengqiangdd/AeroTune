import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const rust = read("../src-tauri/src/disk_analyzer.rs");
const api = read("../src/lib/api.ts");
const duplicateUi = read("../src/components/DuplicateFinderTab.tsx");
const cleanerUi = read("../src/pages/Cleaner.tsx");
const commandStart = rust.indexOf("pub fn recycle_delete(delete_token: String, paths: Vec<String>)");
const commandEnd = rust.indexOf("#[cfg(test)]\nmod recycle_delete_auth_tests", commandStart);
const command = commandStart < 0 || commandEnd < 0 ? "" : rust.slice(commandStart, commandEnd);

assert.ok(command, "recycle_delete must require a scan-issued token");
assert.match(rust, /register_disk_delete_grant\(&result\.files\)/);
assert.match(rust, /register_duplicate_delete_grant\(&result\.groups\)/);
assert.match(rust, /const MAX_DELETE_GRANTS: usize = 4/);
assert.match(rust, /const DELETE_GRANT_TTL: Duration = Duration::from_secs\(30 \* 60\)/);
assert.match(rust, /MAX_RECYCLE_DELETE_BATCH: usize = 100/);
assert.match(command, /canonical_current_file\(path\)/);
assert.match(command, /claim_delete_request\(&mut grants, &delete_token, &resolved\)/);
assert.ok(command.indexOf("claim_delete_request") < command.indexOf("run_checked"), "paths must be claimed before external side effects");
assert.doesNotMatch(command, /DeleteDirectory/, "the tokenized API must not move directories");
assert.match(rust, /remaining <= selected_count/);
assert.match(rust, /claimed\.contains\(path\)/);
assert.match(rust, /concurrent_requests_cannot_replay_the_same_scanned_path/);
assert.match(rust, /failed_external_recycle_does_not_restore_a_claim/);
assert.match(rust, /duplicate_grant_preserves_one_visible_copy_across_batches/);
assert.match(rust, /disk_grant_contains_only_returned_files_and_rejects_mixed_batch_atomically/);

assert.match(api, /recycleDelete: \(deleteToken: string, paths: string\[\]\) =>[\s\S]*invoke<void>\("recycle_delete", \{ deleteToken, paths \}\)/);
assert.match(api, /delete_token: string/);
assert.match(duplicateUi, /Math\.max\(0, group\.files\.length - 1\)/);
assert.match(duplicateUi, /api\.recycleDelete\(deleteToken, batch\)/);
assert.match(duplicateUi, /setResult\(null\);[\s\S]*await scan\(rootToRescan\);/);
assert.match(cleanerUi, /api\.recycleDelete\(deleteToken, \[\.\.\.selected\]\)/);
assert.match(cleanerUi, /api\.recycleDelete\(result\.delete_token, \[f\.path\]\)/);
assert.match(cleanerUi, /setResult\(null\);[\s\S]*await startScan\(rootToRescan\);/);
assert.equal((duplicateUi.match(/api\.recycleDelete\(/g) ?? []).length, 1);
assert.equal((cleanerUi.match(/api\.recycleDelete\(/g) ?? []).length, 2);

console.log("PASS recycle deletion is bounded to fresh, claimed scan results across all callers");
