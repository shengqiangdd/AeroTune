import assert from "node:assert/strict";
import { filterStartupTasks, paginateStartupTasks, STARTUP_TASK_PAGE_SIZE } from "../src/lib/startupTaskPagination.js";

assert.equal(STARTUP_TASK_PAGE_SIZE, 50, "task pages stay bounded to 50 rows");
assert.deepEqual(paginateStartupTasks([], 0), { page: 0, pageCount: 1, items: [] }, "empty task list has a stable page");

const tasks = Array.from({ length: 123 }, (_, index) => ({ id: index + 1 }));
const original = tasks.slice();
const first = paginateStartupTasks(tasks, 0);
const second = paginateStartupTasks(tasks, 1);
const last = paginateStartupTasks(tasks, 2);
assert.equal(first.items.length, 50, "first page is capped");
assert.equal(second.items.length, 50, "middle page is capped");
assert.equal(last.items.length, 23, "final page contains the remainder");
assert.equal(first.pageCount, 3, "page count matches item count");
assert.equal(second.items[0].id, 51, "pages do not overlap");
assert.equal(last.items[22].id, 123, "last task remains reachable");
assert.equal(paginateStartupTasks(tasks, 99).page, 2, "out-of-range pages clamp to the final page");
assert.equal(paginateStartupTasks(tasks, -4).page, 0, "negative pages clamp to the first page");
assert.equal(paginateStartupTasks(tasks, 1.5).page, 0, "fractional page indices normalize to the first page");
assert.deepEqual(paginateStartupTasks(tasks, 0, 0).items, tasks.slice(0, STARTUP_TASK_PAGE_SIZE), "invalid page size falls back to the default");
assert.deepEqual(tasks, original, "pagination does not mutate the sorted task list");

const searchableTasks = [
  { name: "Acme Update", path: "\\Vendor\\", action: "C:\\Program Files\\Acme\\update.exe", author: "Acme" },
  { name: "Telemetry", path: "\\AeroTune\\", action: "aerotune.exe", author: "AeroTune" },
];
const originalSearchableTasks = structuredClone(searchableTasks);
const searchCases = [
  ["case-insensitive name", "UPDATE", [searchableTasks[0]]],
  ["path", "\\aerotune\\", [searchableTasks[1]]],
  ["command", "update.exe", [searchableTasks[0]]],
  ["author", "acme", [searchableTasks[0]]],
  ["whitespace trimming", "  telemetry  ", [searchableTasks[1]]],
  ["multi-field keyword search", "vendor update", [searchableTasks[0]]],
  ["order-independent keyword search", "UPDATE acme", [searchableTasks[0]]],
  ["empty result", "not-found", []],
];
for (const [name, query, expected] of searchCases) {
  assert.deepEqual(filterStartupTasks(searchableTasks, query), expected, `${name} search`);
}
assert.strictEqual(filterStartupTasks(searchableTasks, ""), searchableTasks, "empty query returns the original list");
assert.deepEqual(filterStartupTasks(null, "task"), [], "missing task data returns an empty list");
assert.deepEqual(searchableTasks, originalSearchableTasks, "task search does not mutate source data");

const largeList = Array.from({ length: 100_003 }, (_, index) => index);
const largeLastPage = paginateStartupTasks(largeList, Number.MAX_SAFE_INTEGER);
assert.equal(largeLastPage.pageCount, 2_001, "large inventory page count stays exact");
assert.equal(largeLastPage.page, 2_000, "large inventory clamps to the real final page");
assert.equal(largeLastPage.items.length, 3, "large inventory only renders its final three rows");
assert.equal(largeLastPage.items[0], 100_000, "large inventory final page starts at the correct offset");
const largeSearch = Array.from({ length: 25_000 }, (_, index) => ({ name: `Scheduled ${index}`, path: `\\Vendor\\${index}\\`, action: `C:\\Tasks\\${index}.exe`, author: "Fixture Publisher" }));
const rareMatch = filterStartupTasks(largeSearch, "scheduled 24999");
assert.equal(rareMatch.length, 1, "search finds a rare match in a large inventory");
assert.equal(paginateStartupTasks(rareMatch, 0).items.length, 1, "large-inventory search result remains bounded for rendering");

console.log(`Startup task pagination and search passed: ${13 + searchCases.length + 9} checks`);
