import assert from "node:assert/strict";
import { buildDuplicateCsv } from "../src/lib/duplicateCsv.js";

const result = {
  root: "C:\\Users\\Example\\=archive\u0001",
  files_scanned: 12,
  candidate_files: 4,
  bytes_hashed: 4096,
  logical_duplicate_bytes: 128,
  skipped_unreadable: 1,
  changed_during_scan: 2,
  cancelled: true,
  truncated: true,
  duration_ms: 42,
  groups: [{
    id: "128:abcdef",
    bytes: 128,
    files_total: 3,
    files_truncated: true,
    logical_duplicate_bytes: 256,
    files: [{ path: '=HYPERLINK("x","y")', bytes: 128, modified_unix_s: 1700000000 }],
  }],
};

const csv = buildDuplicateCsv(result);
assert.ok(csv.startsWith("\uFEFF"), "CSV starts with UTF-8 BOM");
assert.ok(csv.includes("扫描状态"), "CSV includes localized headers");
assert.ok(csv.includes("已取消（部分结果）"), "CSV labels partial scans");
assert.ok(csv.includes("\"是\""), "CSV preserves truncation flags");
assert.ok(csv.includes("\"'=HYPERLINK("), "formula-like file paths are neutralized");
assert.ok(csv.includes("\"128:abcdef\""), "duplicate group identifiers are present");
assert.ok(csv.includes("\r\n"), "CSV uses CRLF row separators");
assert.ok(!csv.includes("\u0001"), "control characters cannot survive CSV escaping");
console.log("Duplicate CSV contract passed: BOM, metadata, partial-state labeling, quoting, and formula neutralization");
