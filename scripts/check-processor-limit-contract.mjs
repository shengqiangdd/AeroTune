import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");
const power = read("src-tauri/src/power.rs");
const journal = read("src-tauri/src/journal.rs");
const lib = read("src-tauri/src/lib.rs");
const api = read("src/lib/api.ts");
const card = read("src/components/ProcessorLimitCard.tsx");
const page = read("src/pages/PowerSaver.tsx");
const changeJournal = read("src/lib/changeJournal.js");
const capabilities = JSON.parse(read("src-tauri/capabilities/default.json"));

const removeBody = power.split("pub fn remove_processor_limit")[1]?.split("#[cfg(test)]")[0] ?? "";
assert.ok(removeBody, "remove_processor_limit must exist");

// The write is admin-gated, serialised with the other mode operations, and goes through the write-ahead journal.
assert.match(removeBody, /lock_mode_operations\(\)/);
assert.match(removeBody, /if !crate::util::is_admin\(\)/);
assert.match(removeBody, /crate::journal::set_processor_max_with_journal\(/);
assert.doesNotMatch(removeBody, /run_checked\(|set_processor_max_values\([^)]*\);\s*$/m, "the write must not bypass the journal helper");
assert.match(removeBody, /\|\| set_processor_max_values\(100, 100\)/, "the only value ever written is 100");
assert.match(removeBody, /processor_max_pair\(&scheme\)/, "the old value is read before writing");
assert.match(removeBody, /if old == \(100, 100\)/, "nothing is written when the plan is already unlimited");

// The setter only uses powercfg's documented verbs on the processor-max setting of the *current* scheme.
const setter = power.split("fn set_processor_max_values")[1]?.split("#[derive(Serialize")[0] ?? "";
assert.match(setter, /"\/setacvalueindex"/);
assert.match(setter, /"\/setdcvalueindex"/);
assert.match(setter, /"scheme_current"/);
assert.match(setter, /"procthrottlemax"/);
assert.match(setter, /"\/setactive"/);
assert.match(setter, /ac > 100 \|\| dc > 100/, "values are range-checked");
assert.doesNotMatch(setter, /\/delete|\/restoredefaultschemes|\/import|\/duplicatescheme/, "no plan is deleted, imported or duplicated");

// The journal can undo it, but only against a snapshot that still matches (never a blind overwrite).
assert.match(journal, /ProcessorMax \{\s*scheme: String,\s*old_ac: u32,\s*old_dc: u32,\s*applied: u32,\s*\}/);
const arm = journal.split("Revert::ProcessorMax {\n            scheme,")[1]?.split("Revert::None =>")[0] ?? "";
assert.match(arm, /active_scheme_guid\(\)/);
assert.match(arm, /eq_ignore_ascii_case\(&scheme\)/, "undo must target the same plan");
assert.match(arm, /current != \(Some\(applied\), Some\(applied\)\)/, "undo must refuse when the value changed since");
assert.match(arm, /processor_max_pair\(&scheme\)\? != \(Some\(old_ac\), Some\(old_dc\)\)/, "undo must read the old value back");
assert.match(journal, /fn processor_max_ambiguous_outcomes_keep_the_recovery_record/);
assert.match(journal, /fn processor_max_refuses_when_the_value_changed_during_preparation_or_is_unreadable/);

// IPC surface and permissions.
assert.match(lib, /power::get_processor_state,/);
assert.match(lib, /power::remove_processor_limit,/);
assert.match(api, /getProcessorState: \(\) => invoke<ProcessorState>\("get_processor_state"\)/);
assert.match(api, /removeProcessorLimit: \(\) => invoke<void>\("remove_processor_limit"\)/);
assert.equal(capabilities.permissions.some((permission) => /power|shell|powercfg/i.test(permission)), false);
assert.match(power, /#\[tauri::command\(async\)\]\s*pub fn get_processor_state/);
assert.match(power, /#\[tauri::command\(async\)\]\s*pub fn remove_processor_limit/);

// Frontend: the user must confirm, the explanation says what it does and does not do, nothing runs on mount.
assert.match(
  card,
  /const confirmed = confirm\([\s\S]*?\);\s*if \(!confirmed\) return;\s*setBusy\(true\);\s*try \{\s*await api\.removeProcessorLimit\(\);/,
  "removeProcessorLimit must only run after the user confirms",
);
assert.equal([...card.matchAll(/api\.removeProcessorLimit\(/g)].length, 1);
assert.match(card, /不会强制提高频率/);
assert.match(card, /可在「设置 → 变更日志」撤销/);
assert.match(card, /早期版本留下的记录，没有可还原的快照/);
assert.match(page, /<ProcessorLimitCard \/>/);
assert.match(changeJournal, /case "processormax"/);
assert.match(changeJournal, /processormax: "电源"/);

console.log("PASS processor limit: admin-gated, write-ahead journaled, writes only 100, undo is snapshot-checked, UI-confirmed");
