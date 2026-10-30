import { matchStartupImpactPathToItem } from "./startupImpactMatch.js";

const HEAVY_HINTS = [
  [/onedrive/i, "云同步常驻"],
  [/steam|epic|gog galaxy/i, "游戏平台"],
  [/discord|teams|slack|qq|wechat|weixin|dingtalk|feishu/i, "即时通讯"],
  [/spotify|cloudmusic|kugou|kwmusic/i, "音乐流媒体"],
  [/update|updater|upgrade/i, "更新器"],
  [/helper|agent|daemon|assistant/i, "后台助手"],
];

/** Name/command heuristic only; it does not assert measured startup impact. */
export function startupHeuristicHint(item) {
  const name = typeof item?.name === "string" ? item.name : "";
  const command = typeof item?.cmd === "string" ? item.cmd : "";
  const haystack = `${name} ${command}`;
  for (const [pattern, label] of HEAVY_HINTS) {
    if (pattern.test(haystack)) return label;
  }
  return null;
}

function localDriveEventPath(value) {
  if (typeof value !== "string") return false;
  let path = value.trim();
  if (path.startsWith('"') && path.endsWith('"') && path.length >= 2) {
    path = path.slice(1, -1).trim();
  }
  if (!path || /[\u0000-\u001f\u007f]/.test(path) || /%[^%]+%/.test(path)) return false;
  path = path.replace(/\//g, "\\");
  if (/^\\\\\?\\unc\\/i.test(path)) return false;
  if (/^\\\\\?\\[a-z]:\\/i.test(path)) path = path.slice(4);
  if (path.startsWith("\\\\")) return false;
  return /^[a-z]:\\/i.test(path);
}

/**
 * Build measured-vs-heuristic evidence by startup item. Event 101 evidence is
 * attached only after an unambiguous complete local-drive executable-path match.
 */
export function buildStartupEvidenceByItem(items, impacts) {
  const startupItems = Array.isArray(items) ? items : [];
  const evidenceById = new Map();
  const duplicateIds = new Set();

  for (const item of startupItems) {
    if (!item || typeof item.id !== "string" || !item.id) continue;
    if (evidenceById.has(item.id)) {
      duplicateIds.add(item.id);
      continue;
    }
    evidenceById.set(item.id, {
      heuristic: startupHeuristicHint(item),
      measuredImpacts: [],
    });
  }
  for (const id of duplicateIds) evidenceById.delete(id);

  for (const impact of Array.isArray(impacts) ? impacts : []) {
    if (!Number.isFinite(impact?.degradation_ms) || impact.degradation_ms < 0
      || !Number.isFinite(impact?.total_ms) || impact.total_ms < 0
      || !localDriveEventPath(impact.path)) continue;
    const match = matchStartupImpactPathToItem(impact.path, startupItems);
    if (match.kind !== "exact" || duplicateIds.has(match.item.id)) continue;
    evidenceById.get(match.item.id)?.measuredImpacts.push(impact);
  }

  for (const evidence of evidenceById.values()) {
    evidence.measuredImpacts.sort((a, b) =>
      (Number.isFinite(b.ts) ? b.ts : 0) - (Number.isFinite(a.ts) ? a.ts : 0));
  }
  return evidenceById;
}

export function startupEvidenceExportLabel(evidence) {
  if (!evidence) return "暂无可关联记录（不代表没有影响）";
  const parts = [];
  const latestImpact = evidence.measuredImpacts?.[0];
  if (latestImpact) {
    parts.push(`Windows 事件 101 实测影响 ${latestImpact.degradation_ms.toFixed(0)} ms`);
  }
  if (evidence.heuristic) {
    parts.push(`名称启发式（非实测）：${evidence.heuristic}`);
  }
  return parts.length ? parts.join("；") : "暂无可关联记录（不代表没有影响）";
}
