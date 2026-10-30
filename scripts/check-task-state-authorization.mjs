import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const tasks = readFileSync(new URL("../src-tauri/src/tasks.rs", import.meta.url), "utf8");
const enumerator = tasks.match(/const TASK_ENUMERATION_SCRIPT: &str = r#"([\s\S]*?)"#;/)?.[1];
assert.ok(enumerator);
assert.match(enumerator, /\$folderPath -like '\\Microsoft\*'/);

const mapper = tasks.match(/fn map_task\([\s\S]*?\n\}/)?.[0];
assert.ok(mapper);
assert.match(mapper, /starts_with\("\\\\microsoft"\)/);

const validator = tasks.match(/fn valid_task_target\([\s\S]*?\n\}/)?.[0];
assert.ok(validator);
assert.match(validator, /starts_with\('\\\\'\)/);
assert.match(validator, /ends_with\('\\\\'\)/);
assert.match(validator, /starts_with\("\\\\microsoft"\)/);
assert.match(validator, /matches!\(part, "\." \| "\.\."\)/);
assert.match(validator, /name\.contains\('\\\\'\)/);
assert.match(validator, /name\.contains\('\/'\)/);

const command = tasks.match(/pub async fn set_task_state\([\s\S]*?\n\}/)?.[0];
assert.ok(command);
assert.ok(command.indexOf("valid_task_target(&task_path, &task_name)") < command.indexOf("format!(\"{task_path}{task_name}\")"));
assert.match(command, /只允许修改格式有效的非 Microsoft 计划任务/);
assert.match(tasks, /task_state_rejects_microsoft_and_malformed_scheduler_paths/);
assert.match(tasks, /microsoft_tasks_excluded_at_filter_level/);

console.log("PASS task-state mutation matches the enumerator's non-Microsoft scope and rejects malformed paths");
