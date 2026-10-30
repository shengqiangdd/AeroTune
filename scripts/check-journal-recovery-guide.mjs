import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const settings = readFileSync("src/pages/Settings.tsx", "utf8");
const readiness = readFileSync("docs/RELEASE_READINESS.md", "utf8");
const alertStart = settings.indexOf('{changesStatus === "error" && (');
assert.notEqual(alertStart, -1, "journal read errors must have an explicit recovery state");
const alertEnd = settings.indexOf("\n        )}", alertStart);
assert.ok(alertEnd > alertStart, "journal recovery alert should be a bounded JSX block");
const alert = settings.slice(alertStart, alertEnd);

for (const phrase of [
  "安全恢复步骤（不会自动修改文件）",
  "先点“重试”",
  "导出筛选 CSV/JSON 留存描述性清单",
  "导出不含撤销快照，不能修复日志或用于还原",
  "保留当前 <code>changes.json</code> 原件",
  "%APPDATA%\\AeroTune\\changes.json",
  "已验证可信的备份",
  "不要删除、清空、新建或手工编辑原日志",
  "没有可信备份时保留原件并停止",
  "所有撤销/恢复操作保持禁用",
  "当前没有自动隔离或原位修复流程",
]) assert.ok(alert.includes(phrase), `journal recovery guidance must include: ${phrase}`);
assert.match(alert, /onClick=\{\(\) => void load\(\)\}/, "retry should only reload/read the journal");
assert.doesNotMatch(alert, /api\.(revert|clear|repair|quarantine)/i, "error UI must not mutate or discard journal state");
assert.match(settings, /onClick=\{revertAll\} disabled=\{changesStatus !== "ready"/);
assert.match(settings, /onClick=\{\(\) => revertOne\(c\)\}[\s\S]*disabled=\{changesStatus !== "ready"\}/);
assert.match(settings, /onClick=\{\(\) => exportJournal\("csv"\)\} disabled=\{journalExporting \|\| filteredChanges\.length === 0\}/);
assert.match(settings, /onClick=\{\(\) => exportJournal\("json"\)\} disabled=\{journalExporting \|\| filteredChanges\.length === 0\}/);
assert.match(settings, /buildChangeJournalJson\(filteredChanges, appVersion\)/);
assert.match(readiness, /retry-first, preserve-and-restore guidance panel[\s\S]*no in-app repair\/quarantine flow/);

console.log("Journal recovery guidance contract passed: retry-first, preserve original, restore trusted backup only, no mutation on error");
