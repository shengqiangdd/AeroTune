function csvCell(value) {
  const text = String(value);
  const formulaSafe = /^[\s\u0000-\u001f]*[=+@\-]/.test(text) ? `'${text}` : text;
  const clean = formulaSafe.replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F\u007F]/g, "");
  return `"${clean.replace(/"/g, '""')}"`;
}

/** Build a UTF-8 BOM CSV for all groups and paths returned by the completed scan. */
export function buildDuplicateCsv(result) {
  const rows = [
    [
      "扫描目录",
      "扫描状态",
      "结果可能截断",
      "无法读取项",
      "扫描中变更项",
      "候选文件数",
      "完整读取字节",
      "重复组 ID",
      "单文件字节",
      "组文件总数",
      "本组路径截断",
      "逻辑重复容量字节",
      "文件路径",
      "修改时间 Unix 秒",
    ],
    ...result.groups.flatMap((group) =>
      group.files.map((file) => [
        result.root,
        result.cancelled ? "已取消（部分结果）" : "完成",
        result.truncated ? "是" : "否",
        result.skipped_unreadable,
        result.changed_during_scan,
        result.candidate_files,
        result.bytes_hashed,
        group.id,
        group.bytes,
        group.files_total,
        group.files_truncated ? "是" : "否",
        group.logical_duplicate_bytes,
        file.path,
        file.modified_unix_s,
      ]),
    ),
  ];
  return "\uFEFF" + rows.map((row) => row.map(csvCell).join(",")).join("\r\n");
}
