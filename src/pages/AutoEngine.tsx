// 智能引擎：让自动化保持透明、可感知、可控。
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity, BatteryCharging, BrainCircuit, CheckCircle2, Clock3, Cpu, Gamepad2,
  History, Lightbulb, Monitor, RefreshCw, Rocket, ShieldCheck, Sparkles, Timer, Gauge, Plus, X,
} from "lucide-react";
import { api, AutoStateInfo, Diagnostics, SceneSuggestion } from "../lib/api";
import { Btn, Card, Chip, PageHead, Toggle } from "../components/ui";
import { toastErr, toastOk } from "../lib/toast";
import { fmtTime } from "../lib/format";
import {
  cpuForecastGateMessage,
  cpuForecastGateState,
  cpuForecastPollDelayMs,
  cpuForecastTriggerPct,
  cpuForecastWarmupProgress,
} from "../lib/cpuForecastGate";
import { memoryPressureStatus } from "../lib/memoryPressure";
import { strategyDecisionViewState } from "../lib/strategyDecisionView.js";
import { createRefreshCoordinator } from "../lib/refreshCoordinator";

const MODES: Record<string, { title: string; summary: string; icon: typeof Gamepad2 }> = {
  none: { title: "日常均衡", summary: "维持响应与功耗的平衡", icon: Sparkles },
  game: { title: "游戏专注", summary: "优先保障前台游戏响应", icon: Gamepad2 },
  media: { title: "影音体验", summary: "兼顾播放流畅与后台安静", icon: Monitor },
  away: { title: "暂时离开", summary: "降低空闲期间的系统消耗", icon: Clock3 },
  power: { title: "电池续航", summary: "根据负载平衡续航与响应", icon: BatteryCharging },
  heavy: { title: "持续重载", summary: "编译或渲染期间保持续航性能", icon: Cpu },
  dev: { title: "编码工作", summary: "提升前台交互响应", icon: Cpu },
};
const ALGORITHMS: Record<string, string> = {
  responsiveness: "交互响应优先",
  throughput: "持续吞吐优先",
  efficiency: "能效优先",
  load_balance: "负载均衡",
  adaptive_hybrid: "自适应混合",
};
const QUALITY_COPY: Record<string, string> = {
  game: "前台进入全屏或命中游戏名单后，连续确认信号再切换，减少误判。",
  media: "识别到影音应用时优先保持播放体验，不会把浏览器全屏直接当成游戏。",
  away: "超过离开阈值后进入低活动策略；恢复键鼠操作后重新评估。",
  power: "电池供电时结合当前负载选择策略，避免重任务期间过度压低响应。",
  heavy: "持续负载达到确认条件后保留工作性能，避免短暂尖峰造成来回切换。",
  none: "没有明确场景时保持日常均衡，不为微小波动频繁改动系统。",
  dev: "编码场景侧重前台响应与均衡运行。",
};
function ageLabel(ts: number): string {
  const age = Math.max(0, Math.floor(Date.now() / 1000 - ts));
  if (age < 60) return `${age} 秒前`;
  if (age < 3600) return `${Math.floor(age / 60)} 分钟前`;
  if (age < 86400) return `${Math.floor(age / 3600)} 小时前`;
  return fmtTime(ts);
}
function duration(seconds: number): string {
  if (seconds < 60) return `${seconds} 秒`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)} 分钟`;
  return `${Math.floor(seconds / 3600)} 小时 ${Math.floor(seconds % 3600 / 60)} 分`;
}

export function AutoEngine() {
  const [state, setState] = useState<AutoStateInfo | null>(null);
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [seconds, setSeconds] = useState<Record<string, number>>({});
  const [busy, setBusy] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [gameDraft, setGameDraft] = useState("");
  const [protectedDraft, setProtectedDraft] = useState("");
  const [sceneSuggestions, setSceneSuggestions] = useState<SceneSuggestion[]>([]);
  const [sceneAdopting, setSceneAdopting] = useState<string | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [error, setError] = useState("");
  const refreshCoordinatorRef = useRef<{
    refresh: () => Promise<void>;
    poll: () => Promise<void>;
    dispose: () => void;
  } | null>(null);
  const pollFailureCount = useRef(0);
  const forecastReady = diag?.cpu_forecast_ready === true;
  const forecastReadyRef = useRef(forecastReady);
  forecastReadyRef.current = forecastReady;

  // 场景识别建议：只读拉取，失败保持静默（建议是锦上添花，不阻塞页面）。
  const loadSceneSuggestions = useCallback(async () => {
    try {
      setSceneSuggestions(await api.getSceneSuggestions());
    } catch {
      // 保持上次快照
    }
  }, []);
  useEffect(() => { void loadSceneSuggestions(); }, [loadSceneSuggestions]);

  const refresh = useCallback(() =>
    refreshCoordinatorRef.current?.refresh() ?? Promise.resolve(),
  []);
  useEffect(() => {
    let active = true;
    let timer: number | undefined;
    let coordinator: {
      poll: () => Promise<void>;
    } | null = null;
    const load = async () => {
      try {
        const [autoState, diagnostics, today] = await Promise.all([
          api.getAutoState(), api.getDiagnostics(), api.getStateSeconds(),
        ]);
        if (!active) return;
        pollFailureCount.current = 0;
        forecastReadyRef.current = diagnostics.cpu_forecast_ready === true;
        setState(autoState); setDiag(diagnostics); setSeconds(today); setUpdatedAt(Date.now()); setError("");
      } catch (e) {
        if (!active) return;
        pollFailureCount.current = Math.min(4, pollFailureCount.current + 1);
        setError(String(e));
      }
    };
    const schedulePoll = () => {
      if (!active) return;
      if (timer !== undefined) window.clearTimeout(timer);
      const delay = cpuForecastPollDelayMs(forecastReadyRef.current, document.hidden, pollFailureCount.current);
      timer = window.setTimeout(() => {
        if (!active) return;
        if (document.hidden) schedulePoll();
        else if (coordinator) void coordinator.poll();
      }, delay);
    };
    const createdCoordinator = createRefreshCoordinator({
      runRefresh: load,
      runPoll: load,
      setRefreshing,
      schedulePoll,
    });
    coordinator = createdCoordinator;
    refreshCoordinatorRef.current = createdCoordinator;
    const onVisibilityChange = () => {
      if (!active || document.hidden) return;
      if (timer !== undefined) window.clearTimeout(timer);
      void createdCoordinator.poll();
    };
    document.addEventListener("visibilitychange", onVisibilityChange);
    void createdCoordinator.poll();
    return () => {
      active = false;
      if (timer !== undefined) window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      createdCoordinator.dispose();
      if (refreshCoordinatorRef.current === createdCoordinator) refreshCoordinatorRef.current = null;
    };
  }, []);

  const setting = state?.settings;
  const activeMode = state?.adaptive_state ?? "none";
  const mode = MODES[activeMode] ?? MODES.none;
  const latestDecision = state?.decisions[0];
  const backendReady = (state?.stats_sample_id ?? 0) > 0;
  const strategyScores = state?.algorithm_scores ?? [];
  const strategyView = strategyDecisionViewState(backendReady, setting?.adaptive ?? null, strategyScores.length);
  const todayTotal = Object.values(seconds).reduce((sum, n) => sum + n, 0);
  const topMode = useMemo(() => Object.entries(seconds).sort((a, b) => b[1] - a[1])[0], [seconds]);
  const memoryStatus = memoryPressureStatus(diag);
  const memoryEtaLabel = !backendReady || memoryStatus.etaSeconds === null
    ? null
    : memoryStatus.etaSeconds < 60
      ? `${memoryStatus.etaSeconds} 秒`
      : `${Math.ceil(memoryStatus.etaSeconds / 60)} 分钟`;
  const forecastTriggerPct = cpuForecastTriggerPct(setting?.heavy_cpu_threshold);
  const forecastWarmupSamples = Math.max(0, diag?.cpu_forecast_sample_count ?? 0);
  const forecastWarmupSpanSecs = Number.isFinite(diag?.cpu_forecast_observed_span_secs)
    ? Math.max(0, diag?.cpu_forecast_observed_span_secs ?? 0)
    : 0;
  const forecastWarmupProgress = cpuForecastWarmupProgress(forecastWarmupSamples, forecastWarmupSpanSecs);
  const forecastGateState = cpuForecastGateState({
    ready: Boolean(backendReady && diag?.cpu_forecast_ready),
    adaptive: setting?.adaptive ?? false,
    holdSeconds: state?.hold_secs ?? 0,
    idleSeconds: state?.idle_secs ?? Number.NaN,
    awayThreshold: setting?.away_idle_secs ?? Number.NaN,
    stability: diag?.cpu_forecast_stability ?? Number.NaN,
    slopePctPerSec: diag?.cpu_trend_pct_per_sec ?? Number.NaN,
    forecastPct: diag?.cpu_forecast_10s_pct ?? Number.NaN,
    triggerPct: forecastTriggerPct,
  });
  const forecastGateMessage = cpuForecastGateMessage(forecastGateState, forecastTriggerPct);
  const previousForecastGate = useRef("");
  const [forecastLiveMessage, setForecastLiveMessage] = useState("");
  useEffect(() => {
    if (forecastGateState === "warming") {
      previousForecastGate.current = "warming";
      setForecastLiveMessage("");
      return;
    }
    const signature = `${forecastGateState}:${forecastTriggerPct}`;
    if (previousForecastGate.current !== signature) {
      setForecastLiveMessage(forecastGateMessage);
      previousForecastGate.current = signature;
    }
  }, [forecastGateMessage, forecastGateState, forecastTriggerPct]);

  const setAdaptive = async (enabled: boolean) => {
    setBusy(true);
    try {
      await api.setAutoSetting("adaptive", enabled);
      await refresh();
      toastOk(enabled ? "智能引擎已接管场景判断" : "智能引擎已暂停并恢复原始系统基线");
    } catch (e) {
      toastErr(e);
      await refresh();
    } finally {
      setBusy(false);
    }
  };
  const toggleAutopilot = async (enabled: boolean) => {
    setBusy(true);
    try {
      await api.setAutopilot(enabled);
      await refresh();
      toastOk(enabled ? "全自动模式已开启；开启前设置已备份，关闭时自动还原" : "全自动模式已关闭，已还原此前设置并恢复系统基线");
    } catch (e) {
      toastErr(e);
      await refresh();
    } finally {
      setBusy(false);
    }
  };
  const retryBaselineRestore = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await api.retryBaselineRestore();
      await refresh();
      toastOk("原始系统基线恢复已完成");
    } catch (e) {
      toastErr(e);
      await refresh();
    } finally {
      setBusy(false);
    }
  };
  const retryBrightnessRestore = async () => {
    if (busy) return;
    setBusy(true);
    try {
      await api.retryBrightnessRestore();
      await refresh();
      toastOk("已请求核验或恢复亮度；最终状态由后台确认，提示会保留至确认完成");
    } catch (e) {
      toastErr(e);
      await refresh();
    } finally {
      setBusy(false);
    }
  };
  const savePreference = async (key: string, value: boolean, enabledText: string, disabledText: string) => {
    setBusy(true);
    try { await api.setAutoSetting(key, value); await refresh(); toastOk(value ? enabledText : disabledText); }
    catch (e) { toastErr(e); }
    finally { setBusy(false); }
  };
  const addGame = async () => {
    const name = gameDraft.trim();
    if (!name) return;
    setBusy(true);
    try { await api.addGameItem(name); setGameDraft(""); await refresh(); toastOk(`已加入游戏识别名单：${name}`); }
    catch (e) { toastErr(e); }
    finally { setBusy(false); }
  };
  const removeGame = async (name: string) => {
    setBusy(true);
    try { await api.removeGameItem(name); await refresh(); toastOk(`已移除：${name}`); }
    catch (e) { toastErr(e); }
    finally { setBusy(false); }
  };
  const saveProtectedApps = async (names: string[]) => {
    setBusy(true);
    try { await api.setProtectedApps(names); await refresh(); }
    catch (e) { toastErr(e); }
    finally { setBusy(false); }
  };
  const addProtected = async () => {
    const raw = protectedDraft.trim().toLowerCase();
    if (!raw) return;
    const name = raw.endsWith(".exe") ? raw : `${raw}.exe`;
    const current = setting?.protected_apps ?? [];
    if (current.some((item) => item.toLowerCase() === name)) { setProtectedDraft(""); return; }
    await saveProtectedApps([...current, name]);
    setProtectedDraft("");
    toastOk(`已保护后台进程：${name}`);
  };

  return (
    <div>
      <PageHead title="智能引擎" desc="自动理解使用场景，适时调整系统策略；重要变化清晰可见，随时可暂停。"
        right={<Btn variant="ghost" onClick={() => void refresh()} disabled={refreshing}><RefreshCw size={14} className={refreshing ? "animate-spin" : ""} /> {refreshing ? "更新中…" : "更新状态"}</Btn>} />
      {error && <div role="alert" className="mb-4 rounded-xl border border-amber-400/20 bg-amber-400/8 px-4 py-3 text-sm text-amber-200">引擎状态暂时未能更新，正在重试：{error}</div>}
      {!error && !backendReady && <div role="status" className="mb-4 rounded-xl border border-amber-400/20 bg-amber-400/8 px-4 py-3 text-sm text-amber-200">监控基线尚未就绪；页面数据以占位状态显示，正在等待首轮采样。</div>}
      {state?.baseline_restore_pending && (
        <div role="alert" className="mb-4 flex flex-wrap items-start justify-between gap-3 rounded-xl border border-amber-400/25 bg-amber-400/8 px-4 py-3 text-sm text-amber-100">
          <div className="min-w-0 flex-1 break-words">
            <div className="font-semibold">系统基线尚未完整恢复</div>
            <p className="mt-1 text-xs leading-5">部分系统设置可能仍处于上次场景状态；当前会话的恢复快照已保留。建议保持 AeroTune 运行并重试；退出前可到「设置 → 变更日志」检查持久化记录。完成恢复前，自动控制开关会保持锁定。</p>
          </div>
          <Btn variant="ghost" className="shrink-0" onClick={() => void retryBaselineRestore()} disabled={busy}>
            {busy ? "恢复中…" : "重试恢复基线"}
          </Btn>
        </div>
      )}
      {state?.brightness_restore_pending && !state.baseline_restore_pending && (
        <div role="alert" className="mb-4 flex flex-wrap items-start justify-between gap-3 rounded-xl border border-amber-400/25 bg-amber-400/8 px-4 py-3 text-sm text-amber-100">
          <div className="min-w-0 flex-1 break-words">
            <div className="font-semibold">屏幕亮度恢复尚未确认</div>
            <p className="mt-1 text-xs leading-5">{state?.brightness_restore_snapshot_error ? "上次亮度恢复快照不可读；AeroTune 未修改当前亮度，请手动核对后重试读取。" : "亮度恢复快照已持久化；后台任务可能仍在执行或等待失败退避。启动与退出恢复只会在亮度仍匹配 AeroTune 上次变暗结果时写回原值，外部改动会被保留。"}</p>
          </div>
          <Btn variant="ghost" className="shrink-0" onClick={() => void retryBrightnessRestore()} disabled={busy}>
            {busy ? "检查中…" : state?.brightness_restore_snapshot_error ? "重试读取快照" : "重试亮度恢复"}
          </Btn>
        </div>
      )}
      {state?.brightness_preserved_value != null && (
        <div role="status" className="mb-4 break-words rounded-xl border border-emerald-400/20 bg-emerald-400/8 px-4 py-3 text-sm text-emerald-100">
          当前亮度为 {state.brightness_preserved_value}%；它与 AeroTune 上次自动变暗目标不同，已保留当前值且未覆盖。此提示约 30 秒后消失。
        </div>
      )}

      <section className={`mb-5 overflow-hidden rounded-2xl border p-5 ${setting?.autopilot ? "border-violet-300/25 bg-gradient-to-br from-violet-400/12 via-slate-900/70 to-cyan-400/10" : "border-white/10 bg-white/3"}`}>
        <div className="flex flex-col items-start gap-4 sm:flex-row sm:items-center">
          <div className="flex h-12 w-12 shrink-0 items-center justify-center rounded-2xl border border-violet-200/15 bg-violet-300/10"><Rocket size={24} className="text-violet-200" /></div>
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <h2 className="text-base font-semibold text-white">全自动模式</h2>
              <Chip tone={setting?.autopilot ? "violet" : "slate"}>{setting?.autopilot ? "全自动运行中" : "未开启"}</Chip>
              {setting?.autopilot_backup && <Chip tone="slate">已备份开启前设置</Chip>}
            </div>
            <p className="mt-1 text-xs leading-5 text-slate-300">一键启用整条静默优化栈：场景识别（游戏 / 影音 / 离开 / 重载 / 电池）、电源计划与 EPP 调优、GameDVR、前台优先、后台 EcoQoS 压制、内存压力缓解，以及场景名单自动采纳——全程无需手动点击，模式切换通过系统通知可见。</p>
            <p className="mt-1.5 text-[11px] leading-5 text-slate-500">开启前自动备份相关开关，关闭时按快照还原并恢复系统基线；每次自动采纳进变更日志，可一键移除。边界：不会自动删除文件、结束进程或修改 DNS / 网络 / 服务；进程暂停暂时停用，旧挂起规则仅提醒。</p>
            {setting?.autopilot && (
              <div className="mt-3 flex flex-wrap items-center justify-between gap-3 rounded-xl border border-white/8 bg-black/15 px-3.5 py-2.5">
                <div className="min-w-0 flex-1">
                  <div className="text-xs font-medium text-slate-200">异常自动降级</div>
                  <p className="mt-0.5 text-[11px] leading-5 text-slate-500">持续超阈值的非关键、非前台进程自动把"仅提醒"规则升级为降低优先级（低于正常，可逆、进变更日志）；系统关键进程与前台进程永远只提醒，进程暂停暂时停用。</p>
                </div>
                <Toggle label="异常自动降级" on={setting?.auto_dispose_anomalies ?? false} disabled={busy} onChange={(value) => savePreference("auto_dispose_anomalies", value, "异常自动降级已开启：持续异常的非关键、非前台进程会将仅提醒规则升级为降低优先级", "异常自动降级已关闭：规则恢复为仅提醒")} />
              </div>
            )}
          </div>
          <div className="flex w-full items-center justify-between gap-3 rounded-xl border border-white/8 bg-black/15 px-4 py-3 sm:w-auto sm:justify-start"><span className="text-sm text-slate-200">全自动</span>
            <Toggle label="全自动模式" on={setting?.autopilot ?? false} disabled={!setting || busy || state?.baseline_restore_pending === true} onChange={(value) => void toggleAutopilot(value)} /></div>
        </div>
      </section>

      <section className="mb-5 overflow-hidden rounded-2xl border border-cyan-300/15 bg-gradient-to-br from-cyan-400/10 via-slate-900/70 to-violet-500/10 p-5">
        <div className="flex flex-col items-start gap-4 sm:flex-row sm:items-center sm:gap-5">
          <div className="flex h-14 w-14 shrink-0 items-center justify-center rounded-2xl border border-cyan-200/15 bg-cyan-300/10 sm:h-16 sm:w-16"><BrainCircuit size={28} className="text-cyan-200 sm:hidden" /><BrainCircuit size={30} className="hidden text-cyan-200 sm:block" /></div>
          <div className="min-w-0 w-full flex-1 sm:w-auto">
            <div className="flex flex-wrap items-center gap-2"><h2 className="text-lg font-semibold text-white">{!backendReady ? "引擎连接中" : setting?.adaptive ? mode.title : "由你掌控"}</h2>
              <Chip tone={setting?.adaptive ? "cyan" : "slate"}>{!backendReady ? "等待监控采样" : setting?.adaptive ? "自动适配已开启" : "自动适配已暂停"}</Chip>
              {state && state.hold_secs > 0 && <Chip tone="amber">手动模式保持 {duration(state.hold_secs)}</Chip>}
            </div>
            <p className="mt-1 text-sm text-slate-300">{setting?.adaptive ? mode.summary : "自动场景切换已暂停；关闭时会恢复开启前的系统基线。"}</p>
            <p className="mt-2 max-w-3xl text-xs leading-5 text-slate-400">{setting?.adaptive ? QUALITY_COPY[activeMode] ?? QUALITY_COPY.none : "重新开启后，引擎会根据前台应用、活动状态、电源来源与负载重新判断。"}</p>
          </div>
          <div className="flex w-full items-center justify-between gap-3 rounded-xl border border-white/8 bg-black/15 px-4 py-3 sm:w-auto sm:justify-start"><span className="text-sm text-slate-300">自动适配</span>
            <Toggle label="自动适配" on={setting?.adaptive ?? false} disabled={!backendReady || busy || state?.baseline_restore_pending === true} onChange={setAdaptive} /></div>
        </div>
        <div className="mt-5 grid gap-3 border-t border-white/8 pt-4 sm:grid-cols-3">
          <div className="flex items-center gap-2 text-xs text-slate-300"><ShieldCheck size={15} className="text-emerald-300" />{setting?.adaptive ? "场景确认 + 负载缓冲，减少反复切换" : "暂停自动调节"}</div>
          <div className="flex items-center gap-2 text-xs text-slate-300"><Timer size={15} className="text-cyan-300" />{state?.hold_secs ? `手动覆盖剩余 ${duration(state.hold_secs)}` : "手动切换后可暂时覆盖"}</div>
          <div className="flex items-center gap-2 text-xs text-slate-300"><RefreshCw size={15} className="text-violet-300" />状态每 15 秒刷新{updatedAt ? ` · ${new Date(updatedAt).toLocaleTimeString()}` : ""}</div>
        </div>
      </section>

      <div className="mb-5">
        <Card title="Windows CPU 调度引擎" icon={<Gauge size={15} className="text-emerald-300" />} right={
          <div className="flex flex-wrap items-center justify-end gap-2"><Chip tone={setting?.cpu_scheduler ? "green" : "slate"}>{setting?.cpu_scheduler ? "自动调度开启" : "仅监控"}</Chip><Toggle label="自动 CPU 调度" on={setting?.cpu_scheduler ?? false} disabled={!setting || busy} onChange={(value) => savePreference("cpu_scheduler", value, "CPU 调度引擎已开启", "CPU 调度引擎已关闭，原调度状态将恢复")} /></div>
        }>
          <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-5">
            <SchedulerMetric label="当前场景" value={MODES[activeMode]?.title ?? "日常均衡"} detail={state?.selected_algorithm ? ALGORITHMS[state.selected_algorithm] ?? state.selected_algorithm : "等待负载采样"} />
            <SchedulerMetric label="逐核压力" value={state?.cpu_logical_cores ? `${state.cpu_saturated_cores} / ${state.cpu_logical_cores}` : "等待采样"} detail={state?.cpu_logical_cores ? `峰值 ${state.cpu_core_peak_pct.toFixed(0)}% · 热核阈值 85%` : "读取逻辑处理器负载"} />
            <SchedulerMetric label="后台 EcoQoS" value={state?.cpu_scheduler ? `${state.cpu_scheduler.eco_qos_processes} / ${state.cpu_scheduler.candidate_limit}` : "读取中"} detail={`仅处理单核标度 CPU ≤ ${state?.cpu_scheduler?.background_cpu_cap_pct ?? 2.5}% 的候选进程（多核进程值可超过 100%）`} />
            <SchedulerMetric label="前台提升" value={state?.cpu_scheduler?.foreground_priority ?? "未启用"} detail="全屏游戏稳定确认后采用高于正常优先级" />
            <SchedulerMetric label="监控范围" value={state?.cpu_scheduler ? `${state.cpu_scheduler.monitored_processes} 个进程` : "等待采样"} detail={`后台候选 ${state?.cpu_scheduler?.background_candidates ?? 0} 个`} />
          </div>
          <div className="mt-3 rounded-xl border border-white/6 bg-black/15 px-3.5 py-3 text-xs leading-5 text-slate-400" role="status" aria-label="CPU 调度状态">
            {state?.cpu_scheduler?.detail ?? "等待 Windows CPU 调度能力检测"}。日常场景下，连续核心饱和至少确认 3 秒（托盘 2 秒采样时约 4 秒）后优先响应，并临时应用可回滚的前台调度偏置；不切换电源计划。该指标用于定位 CPU 瓶颈，不等同于帧时间或输入延迟。引擎保护系统与前台进程；日常场景下最多轻调 10 个稳定、低活跃后台进程（EcoQoS 与 Windows 后台优先级），并保留原值用于恢复。物理内存占用 ≥88% 且可用 ≤4GB 时，每 15 分钟最多回收 2 个大体量空闲进程的工作集；私有提交量保持不变，后续按需回涨。CPU Sets/异构核心分配遵从 Windows 自身调度，避免固定绑核。
          </div>
        </Card>
      </div>

      <div className="mb-5 grid gap-4 md:grid-cols-2 xl:grid-cols-3 2xl:grid-cols-5">
        <Card title="最近一次判断" icon={<Lightbulb size={15} className="text-amber-300" />}>
          <div className="text-sm font-medium text-slate-100">{latestDecision ? `${MODES[latestDecision.to]?.title ?? latestDecision.to} · ${ageLabel(latestDecision.ts)}` : setting?.adaptive ? "正在观察你的使用状态" : "尚无自动场景判断"}</div>
          <p className="mt-2 min-h-10 text-xs leading-5 text-slate-400">{latestDecision?.reason ?? "场景出现稳定变化时会显示切换原因；短暂波动会被过滤。"}</p>
        </Card>
        <Card title="今天的使用画像" icon={<Activity size={15} className="text-cyan-300" />}>
          {!backendReady || todayTotal === 0 ? <p className="py-3 text-xs text-slate-400">{backendReady ? "引擎开始工作后逐步形成，不会对历史使用做猜测。" : "等待引擎连接后读取本机使用画像。"}</p> : <>
            <div className="flex items-baseline gap-2"><span className="text-2xl font-semibold tabular-nums text-white">{duration(todayTotal)}</span><span className="text-xs text-slate-500">已观察</span></div>
            <p className="mt-1 text-xs text-slate-400">主要场景：{topMode ? MODES[topMode[0]]?.title ?? topMode[0] : "—"}</p>
            <div className="mt-3 flex h-2 overflow-hidden rounded-full bg-white/8">{Object.entries(seconds).map(([key, value]) => <span key={key} title={`${MODES[key]?.title ?? key} ${duration(value)}`} className="h-full bg-cyan-400/80 even:bg-violet-400/80" style={{ width: `${value / todayTotal * 100}%` }} />)}</div>
          </>}
        </Card>
        <Card title="策略决策状态" icon={<BrainCircuit size={15} className="text-cyan-300" />}>
          {strategyView === "waiting" && (
            <div role="status">
              <div className="text-sm font-semibold text-slate-200">等待监控基线</div>
              <p className="mt-1 text-xs leading-5 text-slate-400">尚未收到可用于评估的监控样本；暂不把旧评分展示为当前决策。</p>
            </div>
          )}
          {strategyView === "warming" && (
            <div role="status">
              <div className="text-sm font-semibold text-cyan-100">自动适配运行中 · 等待首次评估</div>
              <p className="mt-1 text-xs leading-5 text-slate-400">{state?.algorithm_reason || "正在收集系统负载样本，首轮策略评分尚未生成。"}</p>
            </div>
          )}
          {strategyView === "paused_empty" && (
            <div role="status">
              <div className="text-sm font-semibold text-slate-200">自动适配已暂停</div>
              <p className="mt-1 text-xs leading-5 text-slate-400">暂停前没有可展示的策略评估。重新开启后，引擎会基于新样本重新判断。</p>
            </div>
          )}
          {(strategyView === "active" || strategyView === "paused_snapshot") && (
            <>
              <div className="flex flex-wrap items-center justify-between gap-2">
                <span className="text-[11px] font-medium text-slate-400">{strategyView === "active" ? "当前选中策略" : "暂停前最后一次选择"}</span>
                <Chip tone={strategyView === "active" ? "cyan" : "amber"}>{strategyView === "active" ? "自动适配运行中" : "自动适配已暂停"}</Chip>
              </div>
              <div className="mt-1 text-sm font-semibold text-cyan-100">{ALGORITHMS[state?.selected_algorithm ?? ""] ?? state?.selected_algorithm ?? "未知策略"}</div>
              <p className="mt-1 text-xs leading-5 text-slate-400">{state?.algorithm_reason || "暂无策略选择原因。"}</p>
              {strategyView === "paused_snapshot" && (
                <p className="mt-2 rounded-lg border border-amber-300/15 bg-amber-300/5 px-2.5 py-2 text-[11px] leading-4 text-amber-100">以下仅为暂停前评分快照，不代表当前正在应用该策略；暂停期间不会继续评估。</p>
              )}
              <div className="mt-3 space-y-2">
                {strategyScores.map((candidate) => {
                  const selected = candidate.id === state?.selected_algorithm;
                  return (
                    <div key={candidate.id} className="rounded-lg border border-white/6 bg-white/2 px-2.5 py-2">
                      <div className="flex items-center justify-between gap-2">
                        <div className="flex min-w-0 items-center gap-1.5">
                          <span className="truncate text-[11px] text-slate-300">{ALGORITHMS[candidate.id] ?? candidate.id}</span>
                          <Chip tone={selected ? "cyan" : candidate.suitable ? "green" : "slate"}>{selected ? "选中" : candidate.suitable ? "可选" : "低分"}</Chip>
                        </div>
                        <span className="shrink-0 text-[11px] tabular-nums text-slate-400">{candidate.score}/100</span>
                      </div>
                      <div className="mt-1.5 h-1.5 overflow-hidden rounded-full bg-white/8" role="img" aria-label={`${ALGORITHMS[candidate.id] ?? candidate.id}：${candidate.score} 分`}>
                        <div className={`h-full rounded-full ${selected ? "bg-cyan-300" : candidate.suitable ? "bg-emerald-300/80" : "bg-slate-500/70"}`} style={{ width: `${Math.max(0, Math.min(100, candidate.score))}%` }} />
                      </div>
                      <p className="mt-1 text-[10px] leading-4 text-slate-500">{candidate.reason}</p>
                    </div>
                  );
                })}
              </div>
              <p className="mt-2 text-[10px] leading-4 text-slate-500">分数仅表示相对适配程度，不是概率；策略切换还受滞回、确认时长和人工保持条件约束。</p>
            </>
          )}
        </Card>
        <Card title="内存状态" icon={<Cpu size={15} className="text-violet-300" />}>
          <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
            <span className="shrink-0 text-2xl font-semibold tabular-nums text-white">
              {backendReady && memoryStatus.percentValid ? `${memoryStatus.percent.toFixed(0)}%` : "—"}
            </span>
            <span className={`min-w-0 break-words text-xs ${memoryStatus.tone === "warning" ? "text-amber-300" : memoryStatus.tone === "good" ? "text-emerald-300" : "text-slate-400"}`}>
              {memoryStatus.label}
            </span>
          </div>
          {backendReady && memoryStatus.percentValid && (
            <div className="mt-3 h-2 overflow-hidden rounded-full bg-white/8">
              <div
                className={`h-full rounded-full ${memoryStatus.percent >= 90 ? "bg-rose-400" : memoryStatus.percent >= 78 ? "bg-amber-300" : "bg-emerald-400"}`}
                style={{ width: `${memoryStatus.percent}%` }}
              />
            </div>
          )}
          <p className="mt-2 break-words text-xs text-slate-500">{memoryEtaLabel ? `若近 45 秒趋势延续，约 ${memoryEtaLabel} 接近 90%（线性估算）。` : "持续关注系统趋势；只有明显压力时才提示处理。"}</p>
        </Card>
        <Card title="10 秒 CPU 趋势预报" icon={<Activity size={15} className="text-cyan-300" />}>
          {!backendReady || !diag?.cpu_forecast_ready ? (
            <div className="py-2">
              <div role="status" className="text-xs text-slate-400">正在积累基线（至少 10 次独立采样且覆盖 5 秒；托盘驻留时采样更慢）…</div>
              <div className="mt-2 break-words text-[11px] text-slate-500">
                独立采样 {forecastWarmupSamples}/10 · 观测跨度 {forecastWarmupSpanSecs.toFixed(1)}/5 秒
              </div>
              <div
                className="mt-2 h-1.5 overflow-hidden rounded-full bg-white/8"
                role="progressbar"
                aria-label="CPU 预报基线收集进度"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={forecastWarmupProgress}
                aria-valuetext={`独立采样 ${forecastWarmupSamples}/10，观测跨度 ${forecastWarmupSpanSecs.toFixed(1)}/5 秒`}
              >
                <div className="h-full rounded-full bg-cyan-300 transition-[width] duration-300" style={{ width: `${forecastWarmupProgress}%` }} />
              </div>
            </div>
          ) : (
            <>
              <div className="flex flex-wrap items-baseline gap-2">
                <span className="text-2xl font-semibold tabular-nums text-white">{diag.cpu_forecast_10s_pct.toFixed(0)}%</span>
                <Chip tone={diag.cpu_forecast_stability < 60 ? "amber" : diag.cpu_trend_pct_per_sec >= 0.75 ? "rose" : "cyan"}>
                  {diag.cpu_forecast_stability < 60 ? "波动较大，暂不预警" : diag.cpu_trend_pct_per_sec >= 0.75 ? "趋势上行" : diag.cpu_trend_pct_per_sec <= -0.75 ? "趋势回落" : "相对平稳"}
                </Chip>
              </div>
              <div className="mt-3 h-2 overflow-hidden rounded-full bg-white/8" role="progressbar" aria-label="未来 10 秒预计 CPU 使用率" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(diag.cpu_forecast_10s_pct)}><div className={`h-full rounded-full ${diag.cpu_forecast_10s_pct >= 85 ? "bg-rose-400" : diag.cpu_forecast_10s_pct >= 65 ? "bg-amber-300" : "bg-cyan-300"}`} style={{ width: `${Math.min(100, Math.max(0, diag.cpu_forecast_10s_pct))}%` }} /></div>
              <p className="mt-2 text-[11px] leading-4 text-slate-400">稳健斜率 {diag.cpu_trend_pct_per_sec >= 0 ? "+" : ""}{diag.cpu_trend_pct_per_sec.toFixed(2)}%/秒 · 趋势稳定度 {diag.cpu_forecast_stability.toFixed(0)}%</p>
              <p className="mt-1 text-[10px] leading-4 text-slate-500">{forecastGateState === "monitor" ? "自动适配已暂停，预测仅供观察。 " : forecastGateState === "held" ? "手动保持中，预测仅供观察。 " : forecastGateState === "idle" ? "已达到离开阈值，预测不参与提前响应。 " : forecastGateState === "invalid" ? "预测或活动数据无效，等待有效采样。 " : ""}色带仅表示预计负载区间；提前响应线 {forecastTriggerPct}%（配置阈值 +5），还需斜率 ≥0.75%/秒、稳定度 ≥60。Theil–Sen 估算不是帧时间或概率。</p>
              <div className="sr-only" role="status" aria-live="polite" aria-atomic="true">{forecastLiveMessage}</div>
            </>
          )}
        </Card>
      </div>

      <div className="mb-5"><Card title="按你的习惯微调" icon={<Sparkles size={15} className="text-cyan-300" />}>
        <div className="divide-y divide-white/6">
          <Preference title="省电时自动调暗屏幕" detail="进入省电或离开状态时稍微降低亮度，恢复活动后还原此前亮度。" checked={setting?.auto_dim ?? true} disabled={!setting || busy} onChange={(v) => savePreference("auto_dim", v, "已开启亮度联动", "已关闭亮度联动")} />
          <Preference title="识别全屏游戏" detail="全屏游戏稳定运行后自动切换；浏览器和影音应用会被单独识别。" checked={setting?.auto_game ?? false} disabled={!setting || busy} onChange={(v) => savePreference("auto_game", v, "已开启游戏识别", "已关闭游戏识别")} />
          <Preference title="识别电池供电" detail="拔掉电源后按负载进入省电策略；插回电源后不会擅自切回其他模式。" checked={setting?.auto_power ?? false} disabled={!setting || busy} onChange={(v) => savePreference("auto_power", v, "已开启电池场景", "已关闭电池场景")} />
        </div>
        <p className="mt-3 text-[11px] text-slate-500">默认处置规则仅提醒，不会自动挂起或结束进程。进程暂停暂时关闭，旧的自动挂起规则也仅按提醒处理。</p>
      </Card></div>

      <div className="mb-5 grid gap-4 xl:grid-cols-2">
        <Card title="应用策略与保护" icon={<ShieldCheck size={15} className="text-emerald-300" />}>
          <p className="mb-3 text-xs leading-5 text-slate-400">游戏名单用于识别无边框游戏；保护名单按可执行文件名匹配，命中后不进入后台 EcoQoS/低优先级调度。若有同名程序，请留意名称匹配范围。</p>
          <div className="mb-4 flex gap-2">
            <input aria-label="游戏进程名" value={gameDraft} onChange={(e) => setGameDraft(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") void addGame(); }} placeholder="例如 game.exe" className="min-w-0 flex-1 rounded-lg border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-200 outline-none focus:border-cyan-300/40" />
            <Btn variant="ghost" disabled={busy || !gameDraft.trim()} onClick={addGame}><Plus size={14} /> 添加游戏</Btn>
          </div>
          <div className="mb-4 flex flex-wrap gap-2">
            {(setting?.game_list ?? []).map((name) => <Chip key={name} tone="cyan"><span className="flex items-center gap-1.5">{name}<button aria-label={`移除游戏 ${name}`} onClick={() => removeGame(name)} className="rounded hover:text-white"><X size={12} /></button></span></Chip>)}
            {!setting?.game_list?.length && <span className="text-xs text-slate-500">尚未添加自定义游戏</span>}
          </div>
          {sceneSuggestions.length > 0 && (
            <div className="mb-4 rounded-xl border border-violet-300/15 bg-violet-400/5 p-3">
              <div className="mb-1.5 text-xs font-medium text-violet-200">场景识别建议 · 影子观测</div>
              <div className="divide-y divide-white/6">
                {sceneSuggestions.map((suggestion) => (
                  <div key={suggestion.name} className="flex flex-wrap items-center gap-2 py-1.5 text-xs">
                    <span className="min-w-0 flex-1 truncate text-slate-200" title={suggestion.name}>{suggestion.name}</span>
                    <span className="tabular-nums text-slate-500">{suggestion.total_hits} 次确认全屏会话 · 跨 {suggestion.distinct_days} 天</span>
                    <Btn
                      variant="ghost"
                      disabled={busy || sceneAdopting === suggestion.name}
                      aria-label={`把 ${suggestion.name} 加入游戏识别名单`}
                      onClick={async () => {
                        setSceneAdopting(suggestion.name);
                        try {
                          await api.addGameItem(suggestion.name);
                          toastOk(`已加入游戏识别名单：${suggestion.name}`);
                          await refresh();
                          await loadSceneSuggestions();
                        } catch (e) {
                          toastErr(e);
                        } finally {
                          setSceneAdopting(null);
                        }
                      }}
                    >
                      加入名单
                    </Btn>
                    <Btn
                      variant="ghost"
                      aria-label={`忽略 ${suggestion.name} 的场景建议`}
                      onClick={async () => {
                        try {
                          await api.dismissSceneSuggestion(suggestion.name);
                          await loadSceneSuggestions();
                        } catch (e) {
                          toastErr(e);
                        }
                      }}
                    >
                      忽略
                    </Btn>
                  </div>
                ))}
              </div>
              <p className="mt-2 text-[10px] leading-4 text-slate-500">由本机影子观测产生：该进程多次进入确认的 D3D 全屏会话但不在名单中。加入名单会改变游戏场景识别，是否采纳由你决定；忽略只清除这条观测记录，后续会话会重新累积。</p>
            </div>
          )}
          <div className="flex gap-2">
            <input aria-label="保护进程名" value={protectedDraft} onChange={(e) => setProtectedDraft(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") void addProtected(); }} placeholder="保护后台进程，如 sync.exe" className="min-w-0 flex-1 rounded-lg border border-white/10 bg-black/20 px-3 py-2 text-sm text-slate-200 outline-none focus:border-emerald-300/40" />
            <Btn variant="ghost" disabled={busy || !protectedDraft.trim()} onClick={addProtected}><Plus size={14} /> 保护</Btn>
          </div>
          <div className="mt-3 flex flex-wrap gap-2">
            {(setting?.protected_apps ?? []).map((name) => <Chip key={name} tone="green"><span className="flex items-center gap-1.5">{name}<button aria-label={`取消保护 ${name}`} onClick={() => saveProtectedApps((setting?.protected_apps ?? []).filter((item) => item !== name))} className="rounded hover:text-white"><X size={12} /></button></span></Chip>)}
            {!setting?.protected_apps?.length && <span className="text-xs text-slate-500">尚无保护项</span>}
          </div>
          <div className="mt-3 border-t border-white/6 pt-2">
            <Preference title="调度影子模式" detail="继续统计候选进程，但恢复已有调度改动且不应用新的进程优先级/EcoQoS。" checked={setting?.scheduler_shadow ?? false} disabled={!setting || busy} onChange={(v) => savePreference("scheduler_shadow", v, "已开启影子模式，仅观测候选", "已关闭影子模式，按当前调度设置运行")} />
          </div>
        </Card>
        <Card title="异常处置规则" icon={<Lightbulb size={15} className="text-amber-300" />}>
          <p className="mb-3 text-xs leading-5 text-slate-400">规则仅在持续越过阈值后触发，建议先用“提醒”。显式“降低优先级”规则可能影响当前前台应用；“异常自动降级”只把提醒升级到非关键、非前台进程，系统关键进程始终仅提醒。进程暂停暂时停用，旧挂起规则按仅提醒处理。</p>
          <div className="space-y-3">
            {(setting?.rules ?? []).map((rule) => <div key={rule.kind} className="grid grid-cols-[1fr_auto] gap-2 rounded-xl border border-white/6 bg-black/10 p-3 sm:grid-cols-[1fr_100px_92px_130px_auto] sm:items-center">
              <span className="text-sm text-slate-200">{rule.kind === "cpu" ? "CPU" : rule.kind === "mem" ? "内存 MB" : "磁盘 IO KB/s"}</span>
              <input aria-label={`${rule.kind}阈值`} type="number" min={0} defaultValue={rule.threshold} onBlur={(e) => { const value = Number(e.currentTarget.value); if (Number.isFinite(value) && value >= 0 && value !== rule.threshold) api.setRule(rule.kind, rule.enabled, value, rule.seconds, rule.action).then(refresh).catch(toastErr); }} className="w-full rounded border border-white/10 bg-black/20 px-2 py-1.5 text-xs text-slate-200" />
              <input aria-label={`${rule.kind}持续秒数`} type="number" min={1} max={3600} defaultValue={rule.seconds} onBlur={(e) => { const value = Math.max(1, Math.min(3600, Math.floor(Number(e.currentTarget.value)))); if (value !== rule.seconds) api.setRule(rule.kind, rule.enabled, rule.threshold, value, rule.action).then(refresh).catch(toastErr); }} className="w-full rounded border border-white/10 bg-black/20 px-2 py-1.5 text-xs text-slate-200" />
              <select aria-label={`${rule.kind}动作`} value={rule.action} onChange={(e) => api.setRule(rule.kind, rule.enabled, rule.threshold, rule.seconds, e.target.value).then(refresh).catch(toastErr)} className="rounded border border-white/10 bg-slate-900 px-2 py-1.5 text-xs text-slate-200"><option value="notify">仅提醒</option><option value="deprio">降低优先级</option></select>
              <Toggle label={`启用${rule.kind}规则`} on={rule.enabled} disabled={busy} onChange={(v) => api.setRule(rule.kind, v, rule.threshold, rule.seconds, rule.action).then(refresh).catch(toastErr)} />
            </div>)}
          </div>
        </Card>
      </div>

      <div className="mb-5"><Card title="最近的场景变化" icon={<History size={15} className="text-violet-300" />} right={<span className="text-xs text-slate-500">最多保留 20 次</span>}>
        {!state?.decisions.length ? <div className="py-5 text-center text-sm text-slate-400">暂无变化记录。引擎会先观察、再确认，不会因短暂活动频繁切换。</div> : <div className="space-y-2">
          {state.decisions.slice(0, 6).map((decision, index) => { const to = MODES[decision.to] ?? MODES.none; const Icon = to.icon;
            return <div key={`${decision.ts}-${index}`} className="flex items-start gap-3 rounded-xl border border-white/6 bg-white/3 px-3.5 py-3">
              <span className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-lg bg-cyan-300/10"><Icon size={15} className="text-cyan-200" /></span>
              <div className="min-w-0 flex-1"><div className="flex flex-wrap items-center gap-2 text-sm"><span className="font-medium text-slate-100">{MODES[decision.from]?.title ?? decision.from}</span><span className="text-slate-600">→</span><span className="font-medium text-cyan-200">{to.title}</span><span className="text-xs text-slate-500">{ageLabel(decision.ts)}</span></div><p className="mt-1 text-xs leading-5 text-slate-400">{decision.reason}</p></div>
            </div>;
          })}
        </div>}
      </Card></div>
    </div>
  );
}

function SchedulerMetric({ label, value, detail }: { label: string; value: string; detail: string }) {
  return <div className="min-w-0 rounded-xl border border-white/6 bg-white/3 px-3.5 py-3"><div className="text-[11px] font-medium text-slate-500">{label}</div><div className="mt-1 truncate text-sm font-semibold text-slate-100" title={value}>{value}</div><div className="mt-1 text-[11px] leading-4 text-slate-400">{detail}</div></div>;
}

function Preference({ title, detail, checked, disabled, onChange }: { title: string; detail: string; checked: boolean; disabled?: boolean; onChange: (value: boolean) => void }) {
  return <div className="flex items-center gap-4 py-3.5"><div className="min-w-0 flex-1"><div className="text-sm font-medium text-slate-200">{title}</div><p className="mt-0.5 text-xs leading-5 text-slate-400">{detail}</p></div><Toggle label={title} on={checked} disabled={disabled} onChange={onChange} /></div>;
}
