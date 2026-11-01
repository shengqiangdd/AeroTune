import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  MAX_RECENT_PAGES,
  NAVIGATION_SEARCH_TERMS,
  RECENT_NAVIGATION_KEY,
  SETTINGS_DEEP_LINKS,
  clearRecentPageIds,
  getTabFocusTarget,
  nextRecentPageIds,
  persistRecentPageIds,
  rankNavigationItems,
  readRecentPageIds,
  shouldSelectQuickNavigationResult,
} from "../src/lib/quickNavigation.js";

const pages = [
  { id: "dashboard", label: "仪表盘", group: "总览" },
  { id: "game", label: "游戏模式", group: "场景" },
  { id: "power", label: "省电续航", group: "场景" },
  { id: "dev", label: "编码模式", group: "场景" },
  { id: "auto", label: "智能引擎", group: "场景" },
  { id: "processes", label: "进程管理", group: "系统" },
  { id: "startup", label: "开机全景", group: "系统" },
  { id: "services", label: "服务管理", group: "系统" },
  { id: "cleaner", label: "空间清理", group: "系统" },
  { id: "network", label: "网络优化", group: "系统" },
  { id: "settings", label: "设置", group: "系统" },
];
const pageIds = pages.map(({ id }) => id);

assert.deepEqual(Object.keys(NAVIGATION_SEARCH_TERMS).sort(), [...pageIds].sort());
assert.equal(rankNavigationItems(pages, "DNS")[0]?.id, "network");
assert.equal(rankNavigationItems(pages, "Wi-Fi 丢包")[0]?.id, "network");
assert.equal(rankNavigationItems(pages, "Wi-Fi/丢包")[0]?.id, "network");
assert.equal(rankNavigationItems(pages, "临时文件")[0]?.id, "cleaner");
assert.equal(rankNavigationItems(pages, "开机启动项")[0]?.id, "startup");
assert.equal(rankNavigationItems(pages, "CPU 内存")[0]?.id, "processes");
assert.equal(rankNavigationItems(pages, "CPU/内存")[0]?.id, "processes");
assert.equal(rankNavigationItems(pages, "CPU，内存")[0]?.id, "processes");
assert.equal(rankNavigationItems(pages, "设置", ["network"])[0]?.id, "settings");
assert.equal(rankNavigationItems(pages, "这不是功能").length, 0);
assert.equal(rankNavigationItems(pages, "检查更新", [], SETTINGS_DEEP_LINKS)[0]?.anchor, "settings-security-updates");
assert.equal(rankNavigationItems(pages, "更新", [], SETTINGS_DEEP_LINKS)[0]?.anchor, "settings-security-updates");
assert.equal(rankNavigationItems(pages, "撤销", [], SETTINGS_DEEP_LINKS)[0]?.anchor, "settings-change-journal");
assert.equal(rankNavigationItems(pages, "系统保护", [], SETTINGS_DEEP_LINKS)[0]?.anchor, "settings-restore-point");
assert.equal(rankNavigationItems(pages, "隐私", [], SETTINGS_DEEP_LINKS)[0]?.anchor, "settings-local-data");
assert.equal(rankNavigationItems(pages, "WinGet", [], SETTINGS_DEEP_LINKS)[0]?.anchor, "settings-winget-updates");
assert.equal(rankNavigationItems(pages, "", [], SETTINGS_DEEP_LINKS).some((item) => "anchor" in item), false);
assert.ok(SETTINGS_DEEP_LINKS.every((item) => item.routeId === "settings" && item.anchor.startsWith("settings-") && !("action" in item) && !("command" in item)));
console.log("PASS aliases, relevance ranking, and allowlisted navigation-only Settings deep links");

const recentResults = rankNavigationItems(pages, "", ["network", "startup", "network", "unknown"]);
assert.deepEqual(recentResults.slice(0, 2).map(({ id }) => id), ["network", "startup"]);
assert.equal(recentResults[0].quickGroup, "最近使用");
assert.equal(recentResults[2].quickGroup, "总览");
assert.equal(recentResults.length, pages.length);
console.log("PASS recent pages are deduplicated, prioritized, and grouped");

assert.deepEqual(nextRecentPageIds("cleaner", ["network", "cleaner"], pageIds), ["cleaner", "network"]);
assert.deepEqual(nextRecentPageIds("unknown", ["network", "unknown"], pageIds), ["network"]);
assert.equal(nextRecentPageIds("dashboard", pageIds, pageIds).length, MAX_RECENT_PAGES);
const stored = new Map([[RECENT_NAVIGATION_KEY, JSON.stringify(["network", "network", "unknown", null, "startup", "cleaner", "processes", "dashboard", "auto"])]]);
const fakeStorage = {
  getItem: (key) => stored.get(key) ?? null,
  setItem: (key, value) => stored.set(key, value),
  removeItem: (key) => stored.delete(key),
};
assert.deepEqual(readRecentPageIds(pageIds, fakeStorage), ["network", "startup", "cleaner", "processes", "dashboard"]);
persistRecentPageIds(["settings", "invalid", "settings"], pageIds, fakeStorage);
assert.deepEqual(JSON.parse(stored.get(RECENT_NAVIGATION_KEY)), ["settings"]);
assert.equal(clearRecentPageIds(fakeStorage), true);
assert.equal(stored.has(RECENT_NAVIGATION_KEY), false);
assert.deepEqual(readRecentPageIds(pageIds, { getItem: () => "{" }), []);
assert.doesNotThrow(() => persistRecentPageIds(["dashboard"], pageIds, { setItem: () => { throw new Error("quota"); } }));
const staleStorage = new Map([[RECENT_NAVIGATION_KEY, JSON.stringify(["network"])]]);
const blockedStorage = {
  getItem: (key) => staleStorage.get(key) ?? null,
  removeItem: () => { throw new Error("blocked"); },
  setItem: () => { throw new Error("blocked"); },
};
assert.equal(clearRecentPageIds(blockedStorage), false);
assert.deepEqual(readRecentPageIds(pageIds, blockedStorage), ["network"]);
const fallbackStorage = {
  getItem: (key) => staleStorage.get(key) ?? null,
  removeItem: () => { throw new Error("blocked"); },
  setItem: (key, value) => staleStorage.set(key, value),
};
assert.equal(clearRecentPageIds(fallbackStorage), true);
assert.deepEqual(readRecentPageIds(pageIds, fallbackStorage), []);
assert.equal(clearRecentPageIds(null), false);
console.log("PASS local recent history is bounded, validated, clearable, and failure-safe");

const searchInput = {};
const clearButton = {};
assert.equal(shouldSelectQuickNavigationResult(searchInput, searchInput, true), true);
assert.equal(shouldSelectQuickNavigationResult(clearButton, searchInput, true), false);
assert.equal(shouldSelectQuickNavigationResult(searchInput, searchInput, false), false);
const firstControl = {};
const lastControl = {};
const outsideControl = {};
assert.equal(getTabFocusTarget([firstControl, lastControl], outsideControl, false, false), firstControl);
assert.equal(getTabFocusTarget([firstControl, lastControl], outsideControl, true, false), lastControl);
assert.equal(getTabFocusTarget([firstControl, lastControl], firstControl, true, true), lastControl);
assert.equal(getTabFocusTarget([firstControl, lastControl], lastControl, false, true), firstControl);
assert.equal(getTabFocusTarget([firstControl, lastControl], firstControl, false, true), null);
console.log("PASS Enter respects the focused control and Tab always re-enters/traps the modal");

const app = readFileSync("src/App.tsx", "utf8");
const settings = readFileSync("src/pages/Settings.tsx", "utf8");
const ui = readFileSync("src/components/ui.tsx", "utf8");
assert.match(app, /rankNavigationItems\(NAV, quickQuery, recentPageIds, SETTINGS_DEEP_LINKS\)/);
assert.match(app, /selectQuickResult\(quickSelected\)/);
assert.match(app, /onClick=\{\(\) => selectQuickResult\(item\)\}/);
assert.match(app, /setRequestedSettingsSection\(item\.anchor\);[\s\S]*navigate\(item\.routeId,\s*undefined,\s*true,\s*false\)/);
assert.match(app, /if \(target !== "settings"\) setRequestedSettingsSection\(null\)/);
assert.match(app, /const routeChanged = target !== page;[\s\S]*if \(routeChanged\) \{\s*pendingRouteFocusRef\.current = true/);
assert.match(app, /if \(!routeChanged && fromQuick && restoreQuickFocus\) requestAnimationFrame\(\(\) => returnFocus\?\.focus\(\)\)/);
assert.match(settings, /target\.scrollIntoView\(\{ behavior: "smooth", block: "start" \}\)/);
assert.match(settings, /target\.focus\(\{ preventScroll: true \}\)/);
assert.match(ui, /tabIndex=\{focusableTarget \? -1 : undefined\} role=\{focusableTarget \? "region" : undefined\} aria-label=\{focusableTarget \? title : undefined\}/);
for (const link of SETTINGS_DEEP_LINKS) assert.match(settings, new RegExp(`id="${link.anchor}"[\\s\\S]{0,120}focusableTarget`), `missing focusable Settings destination ${link.anchor}`);
assert.match(app, /mainRef\.current\?\.focus\(\{ preventScroll: true \}\)/);
assert.match(app, /tabIndex=\{-1\}[\s\S]*aria-label=\{`当前页面：/);
assert.match(app, /role="status" aria-live="polite" aria-atomic="true"/);
assert.match(app, /requestAnimationFrame\(\(\) => returnFocus\?\.focus\(\)\)/);
assert.match(app, /useLayoutEffect\(\(\) => \{[\s\S]*quickInputRef\.current\?\.focus\(\)/);
assert.match(app, /getTabFocusTarget\(focusable, document\.activeElement, e\.shiftKey, Boolean\(dialog\?\.contains\(document\.activeElement\)\)\)/);
assert.match(app, /shouldSelectQuickNavigationResult\(document\.activeElement, quickInputRef\.current, true\)/);
assert.match(app, /const persisted = clearRecentPageIds\(\)[\s\S]*if \(persisted\) toastOk[\s\S]*else toastErr/);
assert.match(app, /onClick=\{clearRecentPages\}/);
assert.match(app, /quickKeyboardScrollRef\.current = true;[\s\S]*setQuickActiveIndex/);
assert.match(app, /selectedBounds\.top < listBounds\.top[\s\S]*selectedBounds\.bottom > listBounds\.bottom/);
assert.match(app, /ref=\{quickListRef\} id="quick-navigation-results"/);
assert.match(app, /aria-activedescendant=\{quickSelected \? `quick-result-\$\{quickSelected\.id\}`/);
console.log("PASS quick navigation focus, deep-link scrolling, keyboard guards, and announcements are wired");
