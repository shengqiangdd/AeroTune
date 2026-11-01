export const RECENT_NAVIGATION_KEY = "aerotune.quickNavigation.recent.v1";
export const MAX_RECENT_PAGES = 5;

export const NAVIGATION_SEARCH_TERMS = Object.freeze({
  dashboard: ["性能", "系统信息", "硬件信息", "诊断", "诊断报告", "健康评分", "故障", "崩溃", "蓝屏"],
  game: ["游戏", "显卡", "GPU", "全屏", "游戏优化"],
  power: ["省电", "电池", "续航", "耗电", "电源计划", "电池报告"],
  dev: ["开发", "开发者", "编译", "代码", "编程", "开发模式"],
  auto: ["智能", "自动优化", "自适应", "后台策略", "CPU 调度", "自动调度"],
  processes: ["进程", "任务管理器", "CPU", "处理器", "内存", "占用", "PID", "线程", "磁盘 I/O"],
  startup: ["开机", "启动项", "自启动", "开机启动", "开机启动项", "启动程序", "计划任务", "登录任务"],
  services: ["服务", "Windows 服务", "后台服务", "服务状态"],
  cleaner: ["清理", "垃圾文件", "临时文件", "重复文件", "空间释放", "磁盘清理", "卸载", "存储"],
  network: ["网络", "DNS", "Wi-Fi", "网卡", "丢包", "延迟", "网络质量", "TCP"],
  settings: ["设置", "撤销", "回滚", "恢复", "日志", "变更记录", "管理员", "导入配置", "导出配置"],
});

// Searchable, navigation-only destinations inside the long Settings page.
export const SETTINGS_DEEP_LINKS = Object.freeze([
  {
    id: "settings-link-change-journal",
    label: "变更日志与恢复胶囊",
    group: "设置 · 变更恢复",
    routeId: "settings",
    anchor: "settings-change-journal",
    keywords: ["变更日志", "撤销", "回滚", "恢复胶囊", "还原", "操作记录"],
  },
  {
    id: "settings-link-restore-point",
    label: "系统还原点",
    group: "设置 · 变更恢复",
    routeId: "settings",
    anchor: "settings-restore-point",
    keywords: ["还原点", "恢复点", "系统保护", "重大调优"],
  },
  {
    id: "settings-link-privacy-data",
    label: "本机数据与隐私说明",
    group: "设置 · 隐私",
    routeId: "settings",
    anchor: "settings-local-data",
    keywords: ["隐私", "本机数据", "本地存储", "数据保留", "清除历史", "诊断数据"],
  },
  {
    id: "settings-link-security-updates",
    label: "AeroTune 安全更新",
    group: "设置 · 更新",
    routeId: "settings",
    anchor: "settings-security-updates",
    keywords: ["安全更新", "检查更新", "GitHub Releases", "安装包", "签名", "应用版本"],
  },
  {
    id: "settings-link-winget-updates",
    label: "WinGet 软件更新观察",
    group: "设置 · 软件更新",
    routeId: "settings",
    anchor: "settings-winget-updates",
    keywords: ["WinGet", "软件更新", "应用更新", "包管理", "已安装软件"],
  },
]);

function browserStorage() {
  try {
    return globalThis.localStorage ?? null;
  } catch {
    return null;
  }
}

function cleanPageIds(value, validIds) {
  if (!Array.isArray(value)) return [];
  const valid = new Set(validIds);
  const seen = new Set();
  const result = [];
  for (const id of value) {
    if (typeof id !== "string" || !valid.has(id) || seen.has(id)) continue;
    seen.add(id);
    result.push(id);
    if (result.length >= MAX_RECENT_PAGES) break;
  }
  return result;
}

export function readRecentPageIds(validIds, storage = browserStorage()) {
  try {
    const raw = storage?.getItem(RECENT_NAVIGATION_KEY);
    return cleanPageIds(raw ? JSON.parse(raw) : [], validIds);
  } catch {
    return [];
  }
}

export function nextRecentPageIds(pageId, recentIds, validIds) {
  if (!validIds.includes(pageId)) return cleanPageIds(recentIds, validIds);
  return cleanPageIds([pageId, ...cleanPageIds(recentIds, validIds)], validIds);
}

export function persistRecentPageIds(pageIds, validIds, storage = browserStorage()) {
  const safeIds = cleanPageIds(pageIds, validIds);
  try {
    storage?.setItem(RECENT_NAVIGATION_KEY, JSON.stringify(safeIds));
  } catch {
    // Navigation remains usable if WebView storage is unavailable or full.
  }
}

export function clearRecentPageIds(storage = browserStorage()) {
  if (!storage) return false;
  try {
    storage.removeItem(RECENT_NAVIGATION_KEY);
    return true;
  } catch {
    try {
      storage.setItem(RECENT_NAVIGATION_KEY, "[]");
      return true;
    } catch {
      return false;
    }
  }
}

export function shouldSelectQuickNavigationResult(activeElement, inputElement, hasSelection) {
  return Boolean(hasSelection && inputElement && activeElement === inputElement);
}

export function getTabFocusTarget(focusables, activeElement, shiftKey, focusInsideDialog) {
  if (!Array.isArray(focusables) || focusables.length === 0) return null;
  const first = focusables[0];
  const last = focusables[focusables.length - 1];
  if (!focusInsideDialog) return shiftKey ? last : first;
  if (shiftKey && activeElement === first) return last;
  if (!shiftKey && activeElement === last) return first;
  return null;
}

function normalizeSearchText(value) {
  return String(value ?? "")
    .normalize("NFKC")
    .toLocaleLowerCase()
    .replace(/[\s\p{P}\p{S}]+/gu, "");
}

/**
 * Search route titles, groups, and human task synonyms, with deterministic relevance ranking.
 * @template {{ id: string, label: string, group: string }} T
 * @param {T[]} items
 * @param {string} query
 * @param {string[]} recentIds
 * @returns {(T & { quickGroup: string })[]}
 */
export function rankNavigationItems(items, query, recentIds = [], deepLinks = []) {
  const validIds = items.map((item) => item.id);
  const recents = cleanPageIds(recentIds, validIds);
  const byId = new Map(items.map((item) => [item.id, item]));
  const normalizedQuery = normalizeSearchText(query);

  if (!normalizedQuery) {
    const recentSet = new Set(recents);
    return [
      ...recents.map((id) => byId.get(id)).filter(Boolean).map((item) => ({ ...item, quickGroup: "最近使用" })),
      ...items.filter((item) => !recentSet.has(item.id)).map((item) => ({ ...item, quickGroup: item.group })),
    ];
  }

  const tokens = String(query).split(/[\s\p{P}\p{S}]+/u).map(normalizeSearchText).filter(Boolean);
  const candidates = [...items, ...deepLinks];
  return candidates
    .map((item, index) => {
      const label = normalizeSearchText(item.label);
      const group = normalizeSearchText(item.group);
      const keywords = [
        ...(NAVIGATION_SEARCH_TERMS[item.id] ?? []),
        ...(Array.isArray(item.keywords) ? item.keywords : []),
      ].map(normalizeSearchText);
      const searchable = [label, group, ...keywords].join("");
      if (!tokens.every((token) => searchable.includes(token))) return null;

      const score = tokens.reduce((total, token) => {
        if (label === token) return total + 1000;
        if (label.startsWith(token)) return total + 700;
        if (label.includes(token)) return total + 500;
        if (keywords.some((keyword) => keyword === token)) return total + 400;
        if (keywords.some((keyword) => keyword.includes(token))) return total + 300;
        if (group.includes(token)) return total + 150;
        return total + 100;
      }, 0);
      const recentRank = recents.indexOf(item.id);
      const destinationRank = typeof item.routeId === "string" ? 1 : 0;
      return { item, index, score, destinationRank, recentRank: recentRank < 0 ? Number.MAX_SAFE_INTEGER : recentRank };
    })
    .filter((result) => result !== null)
    .sort((a, b) => b.score - a.score || b.destinationRank - a.destinationRank || a.recentRank - b.recentRank || a.index - b.index)
    .map(({ item }) => ({ ...item, quickGroup: item.group }));
}
