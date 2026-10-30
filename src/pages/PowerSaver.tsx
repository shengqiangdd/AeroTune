// 省电续航：电源计划管理 + 电池信息 + 电池报告 + 冗余计划治理
import { useCallback, useEffect, useRef, useState } from "react";
import { BatteryCharging, PlugZap, FileBarChart, Leaf, RefreshCw, Brush as Broom, Clock } from "lucide-react";
import { api, Plan, Stats, PlanDup, PowerDiagnostics } from "../lib/api";
import { batteryAcState, batteryQueryFailed } from "../lib/powerEstimate";
import { effectivePowerModeContext, effectivePowerModePresentation } from "../lib/effectivePowerMode.js";
import type { StatsStatus } from "../hooks/useStats";
import { Card, Btn, Chip, PageHead, ModeBanner, Toggle } from "../components/ui";
import { toastOk, toastErr } from "../lib/toast";
import { PowerInsights } from "../components/PowerInsights";
import { ScenarioPreviewCard } from "../components/ScenarioPreviewCard";

function fmtSecs(s: number | null): string {
  if (s == null) return "—";
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return h > 0 ? `${h} 时 ${m} 分` : `${m} 分钟`;
}

export function PowerSaver({ stats, statsStatus }: { stats: Stats | null; statsStatus: StatsStatus }) {
  const [plans, setPlans] = useState<Plan[]>([]);
  const [plansLoading, setPlansLoading] = useState(true);
  const [plansError, setPlansError] = useState<string | null>(null);
  const [planActionBusy, setPlanActionBusy] = useState(false);
  const [dups, setDups] = useState<PlanDup[]>([]);
  const [mode, setMode] = useState(false);
  const [modeStatus, setModeStatus] = useState<"loading" | "ready" | "error">("loading");
  const [busy, setBusy] = useState(false);
  const active = useRef(false);
  const loadSequence = useRef(0);
  const modeLoadSequence = useRef(0);
  const [sleepDiagnostics, setSleepDiagnostics] = useState<PowerDiagnostics | null>(null);
  const [sleepDiagnosticsLoading, setSleepDiagnosticsLoading] = useState(false);

  const load = useCallback(async () => {
    const requestId = ++loadSequence.current;
    setPlansLoading(true);
    setPlansError(null);
    void Promise.resolve()
      .then(() => api.listRedundantPlans())
      .then((result) => { if (active.current && requestId === loadSequence.current) setDups(result); })
      .catch(() => {});
    try {
      const result = await Promise.resolve().then(() => api.listPowerPlans());
      if (!active.current || requestId !== loadSequence.current) return;
      setPlans(result);
    } catch (error) {
      if (!active.current || requestId !== loadSequence.current) return;
      setPlansError(String(error));
    } finally {
      if (active.current && requestId === loadSequence.current) setPlansLoading(false);
    }
  }, []);
  const loadMode = useCallback(async () => {
    const requestId = ++modeLoadSequence.current;
    setModeStatus("loading");
    try {
      const value = await api.getModeState();
      if (active.current && requestId === modeLoadSequence.current) { setMode(value === "power"); setModeStatus("ready"); }
    } catch {
      if (active.current && requestId === modeLoadSequence.current) setModeStatus("error");
    }
  }, []);
  useEffect(() => {
    active.current = true;
    void load();
    void loadMode();
    return () => { active.current = false; loadSequence.current += 1; modeLoadSequence.current += 1; };
  }, [load, loadMode]);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    api.onTrayModeApplied(() => {
      if (!disposed) void loadMode();
    }).then((stop) => {
      if (disposed) stop();
      else unlisten = stop;
    }).catch(() => {});
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [loadMode]);

  const toggleMode = async (v: boolean) => {
    modeLoadSequence.current += 1;
    setModeStatus("loading");
    setBusy(true);
    try {
      const applied = await api.applyMode(v ? "power" : "none");
      modeLoadSequence.current += 1;
      setMode(v);
      setModeStatus("ready");
      void load();
      toastOk(v ? applied.join("；") : "已退出省电模式");
    } catch (e) {
      modeLoadSequence.current += 1;
      toastErr(e);
      void loadMode();
    } finally {
      setBusy(false);
    }
  };

  const pick = async (guid: string) => {
    if (planActionBusy || plansLoading || plansError) return;
    setPlanActionBusy(true);
    try {
      await api.setPowerPlan(guid);
      await load();
      toastOk("电源计划已切换");
    } catch (e) {
      toastErr(e);
    } finally {
      setPlanActionBusy(false);
    }
  };

  const inspectSleep = async () => {
    setSleepDiagnosticsLoading(true);
    try {
      setSleepDiagnostics(await api.powerDiagnostics());
    } catch (e) {
      toastErr(e);
    } finally {
      setSleepDiagnosticsLoading(false);
    }
  };

  const report = async () => {
    try {
      const p = await api.batteryReport();
      await api.openPath(p);
      toastOk("电池报告已生成并打开");
    } catch (e) {
      toastErr(e);
    }
  };

  const bat = batteryQueryFailed(stats) ? null : stats?.battery ?? null;
  const batteryAc = batteryAcState(bat);
  const batteryFill = batteryAc === false
    ? bat?.percent != null && bat.percent < 20 ? "bg-rose-400" : "bg-amber-400"
    : batteryAc === true ? "grad-btn" : "bg-slate-500";
  const effectivePowerMode = effectivePowerModePresentation(
    stats?.effective_power_mode,
    statsStatus === "loading",
    statsStatus === "stale" || statsStatus === "error",
  );
  const effectivePowerModeHint = statsStatus === "live"
    ? effectivePowerModeContext(
      stats?.effective_power_mode,
      modeStatus === "ready" ? mode : null,
    )
    : null;

  return (
    <div>
      <PageHead
        title="省电续航"
        desc="管理节能计划与能效偏好；查看 Windows 有效能效态、电池报告和本地续航趋势"
        right={
          <Btn variant="ghost" onClick={() => { void load(); }} disabled={plansLoading}>
            <RefreshCw size={14} className={plansLoading ? "animate-spin" : ""} /> {plansLoading ? "读取中…" : "刷新"}
          </Btn>
        }
      />

      <ModeBanner active={modeStatus === "ready" && mode} name="省电模式" onExit={() => toggleMode(false)} />
      {modeStatus === "loading" && <div role="status" className="mb-4 text-xs text-slate-500">正在读取当前模式…</div>}
      {modeStatus === "error" && (
        <div role="alert" className="mb-4 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">模式状态读取失败，开关暂不可用。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void loadMode()}>重试</Btn>
        </div>
      )}
      <div className="mb-5"><ScenarioPreviewCard mode="power" /></div>

      <div className="mb-5 grid grid-cols-1 gap-4 2xl:grid-cols-3">
        <Card className="min-w-0 2xl:col-span-2">
          <div className="flex flex-wrap items-center gap-4">
            <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-emerald-400/12">
              <Leaf size={26} className="text-emerald-300" />
            </div>
            <div className="flex-1">
              <div className="text-base font-medium text-slate-100">一键开启省电模式</div>
              <div className="mt-0.5 text-[13px] text-slate-400">
                切换节能电源计划 · 关闭 GameDVR 后台录制 · 支持时设置 EPP
              </div>
            </div>
            <Toggle label="省电模式" on={mode} onChange={toggleMode} disabled={busy || modeStatus !== "ready"} />
          </div>
        </Card>

        <Card title="电池与 Windows 能效态" icon={<BatteryCharging size={15} className="text-emerald-300" />}>
          {bat ? (
            <div className="space-y-2 text-[13px]">
              <div className="flex items-center justify-between">
                <span className="flex items-center gap-1.5 text-slate-400">
                  {batteryAc === false ? <BatteryCharging size={14} className="text-amber-300" /> : <PlugZap size={14} className={batteryAc === true ? "text-emerald-300" : "text-slate-500"} />}
                  {batteryAc === true ? "已接通电源" : batteryAc === false ? "使用电池" : "电源状态未知"}
                </span>
                <span className="text-lg font-semibold tabular-nums text-slate-100">
                  {bat.percent != null ? `${bat.percent}%` : "未知"}
                </span>
              </div>
              <div className="h-2 overflow-hidden rounded-full bg-white/8">
                <div
                  className={`h-full rounded-full ${batteryFill}`}
                  style={{ width: `${bat.percent ?? 0}%` }}
                />
              </div>
              <div className="text-[11px] text-slate-500">
                系统估算剩余：{fmtSecs(bat.secs_left)}
              </div>
            </div>
          ) : (
            <div className="text-xs text-slate-500">{!stats ? "等待电池遥测数据…" : stats.battery_status_ok === false ? "电池接口查询失败" : "未检测到电池"}</div>
          )}
          <div className="mt-4 border-t border-white/6 pt-3">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <span className="text-[11px] text-slate-400">Windows 当前有效能效态</span>
              <Chip tone={effectivePowerMode.tone} role="status">{effectivePowerMode.label}</Chip>
            </div>
            <p className="mt-2 text-[10px] leading-4 text-slate-500">
              {effectivePowerMode.description} 这是 Windows 的系统级策略信号，与 AeroTune 开关/电源计划不是同一状态，也不是 CPU 频率或性能实测；此卡只读，不会触发设置变更。
            </p>
            {effectivePowerModeHint && (
              <div role="status" className="mt-2 rounded-lg border border-white/6 bg-white/3 p-2 text-[10px] leading-4 text-slate-300">
                {effectivePowerModeHint}
              </div>
            )}
          </div>
        </Card>
      </div>

      <div className="mb-5">
        <PowerInsights stats={stats} statsStatus={statsStatus} />
      </div>

      <div className="grid grid-cols-1 gap-4 2xl:grid-cols-3">
        <Card className="min-w-0 2xl:col-span-2" title="电源计划" icon={<PlugZap size={15} className="text-cyan-300" />}>
          <div className="flex flex-col gap-1.5" aria-busy={plansLoading}>
            {plansLoading && plans.length === 0 && (
              <div role="status" className="py-2 text-xs text-slate-400">正在读取 Windows 电源计划…</div>
            )}
            {plansLoading && plans.length > 0 && (
              <div role="status" className="py-1 text-[11px] text-slate-400">正在刷新；暂时保留上次读取的计划。</div>
            )}
            {plansError && (
              <div role="alert" className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-rose-300/10 bg-rose-300/4 px-3 py-2.5 text-xs text-rose-200">
                <span>电源计划读取失败：{plansError}{plans.length > 0 ? "（保留上次成功结果）" : ""}</span>
                <Btn variant="ghost" onClick={() => { void load(); }} disabled={plansLoading}>重试</Btn>
              </div>
            )}
            {!plansLoading && !plansError && plans.length === 0 && (
              <div role="status" className="flex flex-wrap items-center justify-between gap-2 py-2 text-xs text-slate-300">
                <span>Windows 未返回可用电源计划。</span>
                <Btn variant="ghost" onClick={() => { void load(); }}>重新读取</Btn>
              </div>
            )}
            {plans.map((p) => (
              <button
                key={p.guid}
                type="button"
                aria-pressed={p.active}
                disabled={plansLoading || planActionBusy || busy || !!plansError}
                onClick={() => { void pick(p.guid); }}
                className={`flex items-center justify-between rounded-lg border px-4 py-2.5 text-left text-[13px] transition-all disabled:cursor-not-allowed disabled:opacity-55 ${
                  p.active
                    ? "border-cyan-400/35 bg-cyan-400/10 text-cyan-100"
                    : "border-white/8 bg-white/4 text-slate-300 hover:bg-white/8"
                }`}
              >
                <span>{p.name}</span>
                <Chip tone={p.active ? "cyan" : "slate"}>{p.active ? "使用中" : "点击切换"}</Chip>
              </button>
            ))}
          </div>
        </Card>

        <Card title="电池报告" icon={<FileBarChart size={15} className="text-violet-300" />}>
          <div className="pr-1 text-[13px] text-slate-400">
            生成系统级电池健康报告（设计容量 / 实际容量 / 循环历史），使用 Windows 官方 powercfg 引擎。
          </div>
          <Btn className="mt-3 w-full" onClick={report}>
            生成并打开报告
          </Btn>
        </Card>
      </div>

      <Card className="mt-5" title="睡眠与唤醒诊断" icon={<Clock size={15} className="text-violet-300" />} right={<Btn variant="ghost" onClick={inspectSleep} disabled={sleepDiagnosticsLoading}>{sleepDiagnosticsLoading ? "读取中…" : "读取 Windows 诊断"}</Btn>}>
        <p className="mb-3 text-xs leading-5 text-slate-400">只读读取 powercfg 的睡眠请求、最近唤醒来源和唤醒定时器。结果是当前系统快照，不代表每次睡眠都会由同一来源唤醒。</p>
        {!sleepDiagnostics && <div className="text-xs text-slate-500">按需读取，不会后台轮询或更改电源设置。</div>}
        {sleepDiagnostics && <div className="grid gap-3 lg:grid-cols-3">
          {([
            ["睡眠请求 / 阻止项", sleepDiagnostics.requests],
            ["最近唤醒来源", sleepDiagnostics.last_wake],
            ["唤醒定时器", sleepDiagnostics.wake_timers],
          ] as const).map(([title, lines]) => <section key={title} className="min-w-0 rounded-lg border border-white/6 bg-black/10 p-3">
            <h3 className="mb-2 text-xs font-medium text-slate-300">{title}</h3>
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap break-words text-[10px] leading-4 text-slate-400">{lines.length ? lines.join("\n") : "Windows 未返回文本；这不一定代表系统没有唤醒源。"}</pre>
          </section>)}
        </div>}
        {sleepDiagnostics && <div className="mt-2 text-right text-[10px] text-slate-600">采集时间：{new Date(sleepDiagnostics.collected_at_unix * 1000).toLocaleString()}</div>}
      </Card>

      {/* 冗余计划治理 */}
      {dups.length > 0 && (
        <Card
          className="mb-5"
          title={`电源计划治理 — 发现 ${dups.length} 个同名冗余副本`}
          icon={<Broom size={15} className="text-amber-300" />}
          right={<Chip tone="amber">删除前自动导出 .pow 备份</Chip>}
        >
          <div className="space-y-1.5">
            {dups.map((d) => (
              <div
                key={d.guid}
                className="flex items-center gap-3 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2.5"
              >
                <span className="min-w-0 flex-1 truncate text-[13px] text-slate-300">
                  {d.name} <span className="text-[11px] text-slate-500">· {d.guid}</span>
                </span>
                <Btn
                  variant="danger"
                  onClick={async () => {
                    if (!confirm(`删除冗余电源计划「${d.name}」？\n删除前会导出 .pow 备份到 AeroTune 数据目录，可随时重新导入。`)) return;
                    try {
                      const p = await api.removePlan(d.guid);
                      toastOk(`已删除并备份至 ${p}`);
                      load();
                    } catch (e) {
                      toastErr(e);
                    }
                  }}
                >
                  备份并删除
                </Btn>
              </div>
            ))}
          </div>
          <div className="mt-3 text-[11px] text-slate-600">
            同名计划通常来自 OEM 预装或早期工具重复创建 —— AeroTune v0.6 起已改用 GUID 精确追踪，不会再产生新副本
          </div>
        </Card>
      )}
    </div>
  );
}
