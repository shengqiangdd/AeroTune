import { readFileSync } from "node:fs";

const source = readFileSync(new URL("../src-tauri/src/auto.rs", import.meta.url), "utf8");
const checks = [
  {
    description: "restore targets are limited to a still-owned, changed priority class",
    pattern: /fn priority_restore_target\(current: u32, scheduler_owned: u32, original: u32\) -> Option<u32>[\s\S]*?current != 0 && current == scheduler_owned && original != 0 && current != original[\s\S]*?then_some\(original\)/,
  },
  {
    description: "native restoration applies and verifies the selected original class",
    pattern: /fn restore_priority_class_if_owned\([\s\S]*?let Some\(target\) = priority_restore_target\(current, scheduler_owned, original\) else \{[\s\S]*?return true;[\s\S]*?SetPriorityClass\(handle, target\) != 0 && GetPriorityClass\(handle\) == target/,
  },
  {
    description: "foreground restore failures do not discard their snapshot",
    pattern: /pub\(crate\) fn restore_priority_snapshot\([\s\S]*?let restored = restore_priority_class_if_owned\([\s\S]*?if restored \{\s*Ok\(\(\)\)\s*\} else \{\s*Err/,
  },
  {
    description: "scheduler-map restores retain entries unless priority restore succeeds",
    pattern: /let priority_ok = snapshot\.original_priority_class\.is_none_or\(\|priority\| \{\s*restore_priority_class_if_owned\(handle, BACKGROUND_PRIORITY_CLASS, priority\)\s*\}\);\s*if qos_ok && priority_ok \{\s*snapshots\.remove\(&pid\);/,
  },
  {
    description: "active scheduler restores retain entries unless priority restore succeeds",
    pattern: /let priority_restored =[\s\S]*?restore_priority_class_if_owned\([\s\S]*?priority,[\s\S]*?if qos_restored && priority_restored \{[\s\S]*?state\.scheduler_originals\.remove\(&pid\);/,
  },
  {
    description: "identity-query unavailability is not treated as confirmed PID reuse",
    pattern: /enum IdentityCheck \{\s*Match,\s*Mismatch,\s*Unavailable,[\s\S]*?None => IdentityCheck::Unavailable/,
  },
  {
    description: "foreground restore retains snapshots when identity cannot be verified",
    pattern: /fn restore_foreground_priority\(&mut self\) -> Result<\(\), String>[\s\S]*?IdentityCheck::Unavailable => \{[\s\S]*?优先级恢复快照已保留/,
  },
  {
    description: "scheduler scan retains snapshots when handle identity is unavailable",
    pattern: /IdentityCheck::Unavailable => \{\s*unsafe \{ CloseHandle\(handle\) \};\s*tracing::debug!\([\s\S]*?保留 CPU 恢复快照/,
  },
  {
    description: "foreground recovery snapshot is durable before priority mutation",
    pattern: /match persist_foreground_snapshot\(Some\(&snapshot\)\) \{\s*Err\(error\) => tracing::warn!\([\s\S]*?Ok\(\(\)\) => \{\s*let priority_before = GetPriorityClass\(handle\);[\s\S]*?SetPriorityClass\(\s*handle,\s*target_priority_class/,
  },
  {
    description: "unresolved foreground recovery snapshots cannot be overwritten",
    pattern: /if let Err\(error\) = state\.restore_foreground_priority\(\)[\s\S]*?if state\.foreground_priority_original\.is_none\(\) \{\s*if let Some\(\(image_path, creation_time\)\) = process_identity\(pid\)/,
  },
  {
    description: "a persisted foreground boost is reused only while its class is active",
    pattern: /let already_same =\s*state[\s\S]*?snapshot\.pid == pid[\s\S]*?priority_class_for_snapshot\(snapshot\)\s*== Some\(FOREGROUND_BOOST_PRIORITY_CLASS\)/,
  },
  {
    description: "foreground boosts exclude already-high priority classes",
    pattern: /fn foreground_priority_target\(original: u32\) -> Option<u32> \{\s*matches!\(\s*original,\s*IDLE_PRIORITY_CLASS \| BACKGROUND_PRIORITY_CLASS \| NORMAL_PRIORITY_CLASS\s*\)/,
  },
  {
    description: "foreground apply outcomes preserve foreign changes and retain unreadable state",
    pattern: /match foreground_boost_outcome\([\s\S]*?ForegroundBoostOutcome::NotApplied => \{[\s\S]*?persist_foreground_snapshot\(None\)[\s\S]*?ForegroundBoostOutcome::Unverified => \{[\s\S]*?state\.foreground_priority_original =\s*Some\(snapshot\)/,
  },
];

let failed = false;
for (const { description, pattern } of checks) {
  if (pattern.test(source)) {
    console.log(`PASS ${description}`);
  } else {
    console.error(`FAIL ${description}`);
    failed = true;
  }
}

if (failed) process.exit(1);
