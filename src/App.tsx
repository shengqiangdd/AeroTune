import { Component, lazy, memo, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import { listen } from "@tauri-apps/api/event";
import { TitleBar } from "./components/TitleBar";
import { CornerDownRight, Search } from "lucide-react";
import { Sidebar, PageId, NAV } from "./components/Sidebar";
import { Toaster, toastErr, toastOk } from "./lib/toast";
import { useStats } from "./hooks/useStats";
import { api } from "./lib/api";
import { cpuLoadBandFromUsage, memoryLoadBandFromUsage } from "./lib/processRefreshPolicy";
import type { CpuLoadBand, MemoryLoadBand } from "./lib/processRefreshPolicy";
import {
  clearRecentPageIds,
  getTabFocusTarget,
  nextRecentPageIds,
  persistRecentPageIds,
  rankNavigationItems,
  readRecentPageIds,
  SETTINGS_DEEP_LINKS,
  shouldSelectQuickNavigationResult,
} from "./lib/quickNavigation";
// Load route modules on demand; keep their heavy component work behind memo boundaries.
const Dashboard = lazy(() => import("./pages/Dashboard").then(({ Dashboard }) => ({ default: Dashboard })));
const GameMode = lazy(() => import("./pages/GameMode").then(({ GameMode }) => ({ default: GameMode })));
const PowerSaver = lazy(() => import("./pages/PowerSaver").then(({ PowerSaver }) => ({ default: PowerSaver })));
const DevMode = lazy(() => import("./pages/DevMode").then(({ DevMode }) => ({ default: DevMode })));
const MemoAutoEngine = lazy(() => import("./pages/AutoEngine").then(({ AutoEngine }) => ({ default: memo(AutoEngine) })));
const MemoProcesses = lazy(() => import("./pages/Processes").then(({ Processes }) => ({ default: memo(Processes) })));
const MemoStartup = lazy(() => import("./pages/Startup").then(({ Startup }) => ({ default: memo(Startup) })));
const MemoServices = lazy(() => import("./pages/Services").then(({ Services }) => ({ default: memo(Services) })));
const MemoCleaner = lazy(() => import("./pages/Cleaner").then(({ Cleaner }) => ({ default: memo(Cleaner) })));
const MemoNetwork = lazy(() => import("./pages/Network").then(({ Network }) => ({ default: memo(Network) })));
const MemoSettings = lazy(() => import("./pages/Settings").then(({ Settings }) => ({ default: memo(Settings) })));
const MemoTitleBar = memo(TitleBar);
const PAGE_IDS = NAV.map((item) => item.id);

class RouteErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  render() {
    if (this.state.failed) {
      return (
        <div role="alert" className="mx-auto my-10 max-w-md rounded-xl border border-rose-300/20 bg-rose-300/5 p-4 text-sm text-rose-100">
          <p>当前页面加载失败，内容暂不可用。</p>
          <p className="mt-1 text-xs text-rose-200/80">可尝试重新加载应用，也可使用侧栏切换其他页面。</p>
          <button
            type="button"
            onClick={() => window.location.reload()}
            className="mt-3 rounded-lg border border-white/12 bg-white/8 px-3 py-2 text-xs text-slate-100 hover:bg-white/12 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70"
          >
            重新加载应用
          </button>
        </div>
      );
    }
    return this.props.children;
  }
}

export default function App() {
  const [page, setPage] = useState<PageId>("dashboard");
  const [pageContentReady, setPageContentReady] = useState(false);
  const [focusPid, setFocusPid] = useState<number | null>(null);
  const [admin, setAdmin] = useState(false);
  const [appVersion, setAppVersion] = useState("开发版");
  const [quickOpen, setQuickOpen] = useState(false);
  const [quickQuery, setQuickQuery] = useState("");
  const [quickActiveIndex, setQuickActiveIndex] = useState(0);
  const [requestedSettingsSection, setRequestedSettingsSection] = useState<string | null>(null);
  const [recentPageIds, setRecentPageIds] = useState<PageId[]>(() => readRecentPageIds(PAGE_IDS) as PageId[]);
  const recentPageIdsRef = useRef(recentPageIds);
  const quickInputRef = useRef<HTMLInputElement>(null);
  const quickDialogRef = useRef<HTMLElement>(null);
  const quickListRef = useRef<HTMLDivElement>(null);
  const quickKeyboardScrollRef = useRef(false);
  const quickReturnFocusRef = useRef<HTMLElement | null>(null);
  const mainRef = useRef<HTMLElement | null>(null);
  const pendingRouteFocusRef = useRef(false);
  const navigate = useCallback((target: PageId, pid?: number, fromQuick = false, restoreQuickFocus = true) => {
    const routeChanged = target !== page;
    if (target !== "settings") setRequestedSettingsSection(null);
    setFocusPid(target === "processes" ? pid ?? null : null);
    if (routeChanged) {
      pendingRouteFocusRef.current = true;
      setPage(target);
    }
    setQuickOpen(false);
    setQuickQuery("");
    setQuickActiveIndex(0);
    const returnFocus = quickReturnFocusRef.current;
    quickReturnFocusRef.current = null;
    if (!routeChanged && fromQuick && restoreQuickFocus) requestAnimationFrame(() => returnFocus?.focus());

    const nextRecentPages = nextRecentPageIds(target, recentPageIdsRef.current, PAGE_IDS) as PageId[];
    recentPageIdsRef.current = nextRecentPages;
    setRecentPageIds(nextRecentPages);
    persistRecentPageIds(nextRecentPages, PAGE_IDS);
  }, [page]);
  const clearRecentPages = useCallback(() => {
    const persisted = clearRecentPageIds();
    recentPageIdsRef.current = [];
    setRecentPageIds([]);
    setQuickActiveIndex(0);
    if (persisted) toastOk("最近使用页面记录已清除");
    else toastErr("当前列表已清空，但无法删除本机保存的记录；重启后可能恢复。");
  }, []);
  const quickMatches = useMemo(
    () => rankNavigationItems(NAV, quickQuery, recentPageIds, SETTINGS_DEEP_LINKS),
    [quickQuery, recentPageIds],
  );
  const quickSelected = quickMatches[quickActiveIndex];
  const selectQuickResult = useCallback((item: (typeof quickMatches)[number]) => {
    if ("routeId" in item) {
      setRequestedSettingsSection(item.anchor);
      navigate(item.routeId, undefined, true, false);
    } else {
      navigate(item.id, undefined, true);
    }
  }, [navigate]);
  const handleSettingsSectionHandled = useCallback((found: boolean) => {
    setRequestedSettingsSection(null);
    if (!found) toastErr("无法定位到该设置区段，请从设置页浏览。");
  }, []);
  useEffect(() => {
    if (!pendingRouteFocusRef.current) return;
    const frame = requestAnimationFrame(() => {
      if (!pendingRouteFocusRef.current) return;
      mainRef.current?.focus({ preventScroll: true });
      pendingRouteFocusRef.current = false;
    });
    return () => cancelAnimationFrame(frame);
  }, [page]);
  const closeQuick = useCallback(() => {
    setQuickOpen(false);
    setQuickQuery("");
    setQuickActiveIndex(0);
    const returnFocus = quickReturnFocusRef.current;
    quickReturnFocusRef.current = null;
    requestAnimationFrame(() => returnFocus?.focus());
  }, []);
  useEffect(() => setQuickActiveIndex(0), [quickQuery]);
  useLayoutEffect(() => {
    if (!quickOpen || !quickKeyboardScrollRef.current) return;
    quickKeyboardScrollRef.current = false;
    const list = quickListRef.current;
    const selected = list?.querySelector<HTMLElement>('[aria-selected="true"]');
    if (!list || !selected) return;
    const listBounds = list.getBoundingClientRect();
    const selectedBounds = selected.getBoundingClientRect();
    if (selectedBounds.top < listBounds.top) list.scrollTop += selectedBounds.top - listBounds.top;
    else if (selectedBounds.bottom > listBounds.bottom) list.scrollTop += selectedBounds.bottom - listBounds.bottom;
  }, [quickActiveIndex, quickOpen, quickMatches]);
  const { stats, hist, status, error } = useStats(pageContentReady);
  const [processLoadBands, setProcessLoadBands] = useState<{ cpu: CpuLoadBand; memory: MemoryLoadBand }>({
    cpu: "unknown",
    memory: "unknown",
  });
  useEffect(() => {
    setProcessLoadBands((current) => {
      const cpu = cpuLoadBandFromUsage(stats?.cpu, current.cpu);
      const memory = memoryLoadBandFromUsage(stats?.mem_used_gb, stats?.mem_total_gb, current.memory);
      return cpu === current.cpu && memory === current.memory ? current : { cpu, memory };
    });
  }, [stats?.cpu, stats?.mem_used_gb, stats?.mem_total_gb]);
  const { cpu: processLoadBand, memory: processMemoryLoadBand } = processLoadBands;

  useLayoutEffect(() => {
    if (quickOpen) {
      setQuickActiveIndex(0);
      if (!quickReturnFocusRef.current) {
        quickReturnFocusRef.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      }
      quickInputRef.current?.focus();
    }
  }, [quickOpen]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let unlistenTrayModeApplied: (() => void) | undefined;
    let unlistenTrayModeFailed: (() => void) | undefined;
    let unlistenFocus: (() => void) | undefined;
    let unlistenMainWindowShown: (() => void) | undefined;
    listen("main-window-shown", () => {
      if (!disposed) setPageContentReady(true);
    }).then((stop) => {
      if (disposed) stop();
      else unlistenMainWindowShown = stop;
    }).catch(() => {});
    getVersion().then((version) => { if (!disposed) setAppVersion(version); }).catch(() => { if (!disposed) setAppVersion("开发版"); });
    api.onAutoActions((actions) => {
      if (!disposed) for (const a of actions) toastOk(a);
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});
    api.onTrayModeApplied((message) => {
      if (!disposed) toastOk(message);
    }).then((stop) => {
      if (disposed) stop();
      else unlistenTrayModeApplied = stop;
    }).catch(() => {});
    api.onTrayModeFailed((message) => {
      if (!disposed) toastErr(message);
    }).then((stop) => {
      if (disposed) stop();
      else unlistenTrayModeFailed = stop;
    }).catch(() => {});
    const win = getCurrentWindow();
    const initialize = async () => {
      const [adminResult, silentResult] = await Promise.allSettled([api.isAdmin(), api.isSilentStart()]);
      if (disposed) return;
      const isAdmin = adminResult.status === "fulfilled" && adminResult.value;
      const isSilent = silentResult.status === "fulfilled" && silentResult.value;
      setAdmin(isAdmin);

      // Normal launches stay usable without elevation; admin-only actions can request it explicitly.
      if (!isSilent) {
        await win.show().catch(() => {});
        if (!disposed) setPageContentReady(true);
        return;
      }
      try {
        const stopFocus = await win.onFocusChanged(({ payload: focused }) => {
          if (focused && !disposed) setPageContentReady(true);
        });
        if (disposed) stopFocus();
        else unlistenFocus = stopFocus;
        if (await win.isVisible() && !disposed) setPageContentReady(true);
      } catch {
        // If visibility tracking is unavailable, prefer a usable app over a blank tray window.
        if (!disposed) setPageContentReady(true);
      }
    };
    void initialize();
    return () => {
      disposed = true;
      unlisten?.();
      unlistenTrayModeApplied?.();
      unlistenTrayModeFailed?.();
      unlistenFocus?.();
      unlistenMainWindowShown?.();
    };
  }, []);

  useLayoutEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        if (quickOpen) closeQuick();
        else setQuickOpen(true);
        return;
      }
      if (quickOpen && e.key === "Escape") {
        e.preventDefault();
        closeQuick();
        return;
      }
      if (quickOpen && e.key === "Tab") {
        const dialog = quickDialogRef.current;
        const focusable = dialog
          ? Array.from(dialog.querySelectorAll<HTMLElement>('input:not([disabled]), button:not([disabled]), [tabindex]:not([tabindex="-1"])'))
          : [];
        if (!focusable.length) {
          e.preventDefault();
        } else {
          const target = getTabFocusTarget(focusable, document.activeElement, e.shiftKey, Boolean(dialog?.contains(document.activeElement)));
          if (target) {
            e.preventDefault();
            target.focus();
          }
        }
        return;
      }
      if (quickOpen && document.activeElement === quickInputRef.current && e.key === "ArrowDown" && quickMatches.length) {
        e.preventDefault();
        quickKeyboardScrollRef.current = true;
        setQuickActiveIndex((index) => (index + 1) % quickMatches.length);
      } else if (quickOpen && document.activeElement === quickInputRef.current && e.key === "ArrowUp" && quickMatches.length) {
        e.preventDefault();
        quickKeyboardScrollRef.current = true;
        setQuickActiveIndex((index) => (index - 1 + quickMatches.length) % quickMatches.length);
      } else if (quickOpen && e.key === "Enter" && quickSelected && shouldSelectQuickNavigationResult(document.activeElement, quickInputRef.current, true)) {
        e.preventDefault();
        selectQuickResult(quickSelected);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [closeQuick, navigate, selectQuickResult, quickMatches, quickActiveIndex, quickOpen]);

  const currentPageLabel = NAV.find((item) => item.id === page)?.label ?? "应用";
  return (
    <div className="flex h-full flex-col">
      <MemoTitleBar admin={admin} />
      {(status === "stale" || status === "error") && (
        <div role="status" className="border-b border-amber-400/20 bg-amber-400/10 px-5 py-2 text-xs text-amber-200">
          {status === "error" ? "实时监控连接失败" : "实时数据已停止更新"}；正在自动重试。
          {error && <span className="ml-2 text-amber-300">{error}</span>}
        </div>
      )}
      <div className="flex min-h-0 flex-1">
        <Sidebar page={page} onNav={navigate} stats={stats} appVersion={appVersion} onQuickOpen={() => { setQuickQuery(""); setQuickOpen(true); }} />
        <main
          ref={mainRef}
          data-testid="app-main"
          tabIndex={-1}
          aria-label={`当前页面：${currentPageLabel}`}
          className="min-w-0 flex-1 overflow-y-auto p-4 sm:p-6"
          key={page}
        >
          {pageContentReady ? (
            <RouteErrorBoundary>
              <Suspense fallback={<div role="status" className="flex min-h-[40vh] items-center justify-center text-sm text-slate-400">正在加载页面…</div>}>
                <div className="rise">
                  {page === "dashboard" && <Dashboard stats={stats} statsStatus={status} hist={hist} onNav={navigate} />}
                  {page === "game" && <GameMode stats={stats} />}
                  {page === "power" && <PowerSaver stats={stats} statsStatus={status} />}
                  {page === "dev" && <DevMode stats={stats} />}
                  {page === "auto" && <MemoAutoEngine />}
                  {page === "processes" && <MemoProcesses focusPid={focusPid} cpuLoadBand={processLoadBand} memoryLoadBand={processMemoryLoadBand} />}
                  {page === "startup" && <MemoStartup onNav={navigate} />}
                  {page === "services" && <MemoServices />}
                  {page === "cleaner" && <MemoCleaner />}
                  {page === "network" && <MemoNetwork />}
                  {page === "settings" && <MemoSettings admin={admin} onAdmin={setAdmin} appVersion={appVersion} requestedSection={requestedSettingsSection} onRequestedSectionHandled={handleSettingsSectionHandled} />}
                </div>
              </Suspense>
            </RouteErrorBoundary>
          ) : (
            <div role="status" className="flex min-h-[40vh] items-center justify-center text-sm text-slate-400">正在准备主界面…</div>
          )}
        </main>
      </div>
      {quickOpen && (
        <div className="fixed inset-0 z-[60] flex items-start justify-center bg-black/55 px-4 pt-[12vh] backdrop-blur-sm" onMouseDown={(event) => { if (event.target === event.currentTarget) closeQuick(); }}>
          <section ref={quickDialogRef} role="dialog" aria-modal="true" aria-label="快速跳转" className="glass w-full max-w-xl overflow-hidden border border-white/12 shadow-2xl">
            <div className="flex items-center gap-3 border-b border-white/8 px-4 py-3">
              <Search size={15} className="shrink-0 text-cyan-300" />
              <input ref={quickInputRef} value={quickQuery} onChange={(event) => setQuickQuery(event.target.value)} placeholder="搜索页面或设置区段，如 DNS、更新、撤销…" className="min-w-0 flex-1 bg-transparent text-sm text-slate-100 outline-none placeholder:text-slate-500" aria-label="搜索页面或设置区段" aria-describedby="quick-results-status quick-search-help" aria-controls="quick-navigation-results" aria-activedescendant={quickSelected ? `quick-result-${quickSelected.id}` : undefined} />
              <kbd className="rounded border border-white/10 px-1.5 py-0.5 text-[10px] text-slate-500">Esc</kbd>
            </div>
            <div id="quick-results-status" className="sr-only" role="status" aria-live="polite" aria-atomic="true">
              {quickQuery.trim()
                ? quickMatches.length ? `找到 ${quickMatches.length} 个匹配目标` : "没有匹配页面或设置区段"
                : recentPageIds.length ? `最近使用的页面排在前面，共 ${quickMatches.length} 个页面` : `共 ${quickMatches.length} 个页面；可搜索 DNS、启动项、更新或隐私`}
            </div>
            <div ref={quickListRef} id="quick-navigation-results" role="listbox" aria-label="快速导航结果" className="max-h-[55vh] overflow-y-auto p-2">
              {quickMatches.length ? quickMatches.map((item, index) => (
                <div key={item.id} id={`quick-result-${item.id}`} onMouseEnter={() => setQuickActiveIndex(index)} onClick={() => selectQuickResult(item)} role="option" aria-selected={quickActiveIndex === index} className={`flex cursor-pointer items-center gap-3 rounded-lg px-3 py-2.5 text-sm transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70 ${quickActiveIndex === index ? "bg-cyan-400/12 text-cyan-200" : "text-slate-300 hover:bg-white/8"}`} tabIndex={-1}>
                  <span className="text-cyan-300">{"routeId" in item ? <CornerDownRight size={14} /> : item.icon}</span><span className="flex-1">{item.label}</span><span className="text-[11px] text-slate-500">{item.quickGroup}</span>
                </div>
              )) : <div role="presentation" className="px-3 py-8 text-center text-sm text-slate-500">没有匹配页面或设置区段</div>}
            </div>
            <div id="quick-search-help" className="flex flex-wrap items-center justify-between gap-2 border-t border-white/8 px-4 py-2 text-[10px] text-slate-500">
              <span>↑↓选择 · Enter 跳转/定位 · Esc 返回 · Ctrl+K 开关</span>
              <button type="button" onClick={clearRecentPages} disabled={!recentPageIds.length} aria-label="清除最近使用页面记录" title="只清除本机保存的页面访问顺序" className="rounded px-1.5 py-1 enabled:hover:bg-white/8 enabled:hover:text-slate-300 disabled:cursor-not-allowed disabled:opacity-40">清除最近</button>
            </div>
          </section>
        </div>
      )}
      <Toaster />
    </div>
  );
}
