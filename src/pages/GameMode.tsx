// 游戏模式：一键编排 + 独立调优开关 + GPU 应用偏好
import { useEffect, useRef, useState } from "react";
import { Gamepad2, MonitorStop, Sparkles, Cpu, Info, Microchip } from "lucide-react";
import { api, Stats, TweakStates, GpuPref } from "../lib/api";
import { Card, Btn, Toggle, Chip, PageHead, ModeBanner } from "../components/ui";
import { toastOk, toastErr } from "../lib/toast";
import { ScenarioPreviewCard } from "../components/ScenarioPreviewCard";
import { validCapacityPair } from "../lib/resourceEvidence";

export function GameMode({ stats }: { stats: Stats | null }) {
  const [on, setOn] = useState(false);
  const [states, setStates] = useState<TweakStates | null>(null);
  const [statesStatus, setStatesStatus] = useState<"loading" | "ready" | "error">("loading");
  const tweakRequestId = useRef(0);
  const [busy, setBusy] = useState(false);
  const [modeStatus, setModeStatus] = useState<"loading" | "ready" | "error">("loading");
  const modeRequestId = useRef(0);

  const refresh = async () => {
    const requestId = ++tweakRequestId.current;
    setStatesStatus("loading");
    try {
      const value = await api.getTweakStates();
      if (requestId === tweakRequestId.current) { setStates(value); setStatesStatus("ready"); }
    } catch {
      if (requestId === tweakRequestId.current) setStatesStatus("error");
    }
  };
  const loadMode = async () => {
    const requestId = ++modeRequestId.current;
    setModeStatus("loading");
    try {
      const mode = await api.getModeState();
      if (requestId === modeRequestId.current) { setOn(mode === "game"); setModeStatus("ready"); }
    } catch {
      if (requestId === modeRequestId.current) setModeStatus("error");
    }
  };
  useEffect(() => {
    void loadMode();
    void refresh();
    return () => { modeRequestId.current += 1; tweakRequestId.current += 1; };
  }, []);

  const toggleMode = async (v: boolean) => {
    setBusy(true);
    try {
      const applied = await api.applyMode(v ? "game" : "none");
      setOn(v);
      setModeStatus("ready");
      void refresh();
      toastOk(v ? applied.join("；") : "已退出游戏模式");
    } catch (e) {
      toastErr(e);
      void refresh();
      void loadMode();
    }
    setBusy(false);
  };

  const flip = async (key: string, v: boolean) => {
    try {
      await api.setTweak(key, v);
      void refresh();
      toastOk("已应用，可在「设置 → 变更日志」还原");
    } catch (e) {
      toastErr(e);
      void refresh();
    }
  };

  return (
    <div>
      <PageHead
        title="游戏模式"
        desc="为对局切换高性能计划、关闭后台录制并按平台支持设置 EPP"
        right={<Chip tone="rose">帧率优先</Chip>}
      />

      <ModeBanner active={modeStatus === "ready" && on} name="游戏模式" onExit={() => toggleMode(false)} />
      {modeStatus === "loading" && <div role="status" className="mb-4 text-xs text-slate-500">正在读取当前模式…</div>}
      {modeStatus === "error" && (
        <div role="alert" className="mb-4 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">模式状态读取失败，开关暂不可用。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void loadMode()}>重试</Btn>
        </div>
      )}
      {statesStatus === "loading" && <div role="status" className="mb-4 text-xs text-slate-500">正在读取游戏调优状态…</div>}
      {statesStatus === "error" && (
        <div role="alert" className="mb-4 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">调优状态读取失败，相关开关已禁用。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void refresh()}>重试</Btn>
        </div>
      )}
      <div className="mb-5"><ScenarioPreviewCard mode="game" /></div>

      <div className="mb-5 grid grid-cols-1 gap-4 2xl:grid-cols-3">
        <Card className="min-w-0 2xl:col-span-2">
          <div className="flex flex-wrap items-center gap-4">
            <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-rose-400/12">
              <Gamepad2 size={26} className="text-rose-300" />
            </div>
            <div className="flex-1">
              <div className="text-base font-medium text-slate-100">一键开启游戏模式</div>
              <div className="mt-0.5 text-[13px] text-slate-400">
                切换到可用的高性能电源计划 · 关闭 GameDVR 后台录制 · 支持时设置 EPP
              </div>
            </div>
            <Toggle label="游戏模式" on={on} onChange={toggleMode} disabled={busy || modeStatus !== "ready"} />
          </div>
          <div className="mt-4 flex gap-2 border-t border-white/6 pt-4 text-[12px] text-slate-400">
            <Info size={14} className="mt-px shrink-0 text-cyan-300/80" />
            支持还原的修改会记入「设置 → 变更日志」；恢复前会核对当前值，发现外部改动时保留记录
          </div>
        </Card>

        <Card title="实时负载" icon={<Cpu size={15} className="text-rose-300" />}>
          <div className="space-y-2 text-[13px] text-slate-300">
            <div className="flex justify-between">
              <span className="text-slate-400">CPU</span>
              <span className="tabular-nums">{stats?.cpu.toFixed(0) ?? "-"}%</span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-400">GPU</span>
              <span className="tabular-nums">{stats?.gpu?.utilization != null ? `${stats.gpu.utilization.toFixed(0)}%` : "N/A"}</span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-400">系统已用</span>
              <span className="tabular-nums">
                {stats && validCapacityPair(stats.mem_used_gb, stats.mem_total_gb) ? `${stats.mem_used_gb.toFixed(1)} / ${stats.mem_total_gb.toFixed(0)} GB` : "数据不可用"}
              </span>
            </div>
          </div>
        </Card>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <Card title="GameDVR 后台录制" icon={<MonitorStop size={15} className="text-slate-300" />}>
          <div className="flex items-center justify-between">
            <div className="pr-4 text-[13px] text-slate-400">
              Xbox Game Bar / 后台录制即使不使用也会占用编码器与磁盘 IO，关闭可减少帧率波动
            </div>
            <Toggle label="GameDVR 后台录制" on={states?.gamedvr ?? false} disabled={statesStatus !== "ready" || states?.gamedvr == null} onChange={(v) => flip("gamedvr", v)} />
          </div>
        </Card>

        <Card title="硬件加速 GPU 计划（HAGS）" icon={<Sparkles size={15} className="text-violet-300" />}>
          <div className="flex items-center justify-between">
            <div className="pr-4 text-[13px] text-slate-400">
              {statesStatus === "loading" ? "正在读取 HAGS 状态…"
                : statesStatus === "error" ? "状态读取失败，请重试后再操作。"
                  : states?.hags == null ? "需要管理员权限或较新显卡驱动才能读取"
                    : "降低渲染队列延迟（NVIDIA 10 系+ 建议；修改后需重启生效）"}
            </div>
            <Toggle
              label="硬件加速 GPU 计划（HAGS）"
              on={states?.hags ?? false}
              disabled={statesStatus !== "ready" || states?.hags == null}
              onChange={(v) => flip("hags", v)}
            />
          </div>
        </Card>
      </div>

      <div className="mt-4">
        <EppCard />
      </div>
      <div className="mt-4">
        <GpuPrefCard />
      </div>
    </div>
  );
}

const GPU_PREF_CN: Record<number, string> = { 0: "自动选择", 1: "节能（核显）", 2: "高性能（独显）" };

function GpuPrefCard() {
  const [prefs, setPrefs] = useState<GpuPref[]>([]);
  const [status, setStatus] = useState<"loading" | "ready" | "error">("loading");
  const [busy, setBusy] = useState(false);
  const [exe, setExe] = useState("");

  const load = async () => {
    setStatus("loading");
    try { setPrefs(await api.listGpuPrefs()); setStatus("ready"); }
    catch { setStatus("error"); }
  };
  useEffect(() => { void load(); }, []);

  const set = async (path: string, v: number) => {
    if (!path.trim() || status !== "ready" || busy) return;
    setBusy(true);
    try {
      await api.setGpuPref(path, v);
      await load();
      toastOk("GPU 偏好已更新（Windows 图形设置即时生效）");
    } catch (e) {
      toastErr(e);
    } finally {
      setBusy(false);
    }
  };

  const shortName = (p: string) => p.split("\\").pop() ?? p;

  return (
    <Card title="GPU 应用偏好（双显卡指定）" icon={<Microchip size={15} className="text-emerald-300" />}>
      {status === "loading" && <div role="status" className="mb-3 text-xs text-slate-500">正在读取 GPU 偏好…</div>}
      {status === "error" && (
        <div role="alert" className="mb-3 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">GPU 偏好读取失败，编辑已暂停。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
        </div>
      )}
      <div className="mb-3 flex items-center gap-2">
        <input
          aria-label="游戏或应用完整路径"
          value={exe}
          onChange={(e) => setExe(e.target.value)}
          placeholder="游戏/应用完整路径，如 D:\\Games\\game.exe"
          className="w-80 rounded-lg border border-white/10 bg-white/5 px-3 py-2 text-sm text-slate-200 outline-none placeholder:text-slate-600 focus:border-cyan-400/40"
        />
        <Btn onClick={() => void set(exe.trim(), 2)} disabled={!exe.trim() || status !== "ready" || busy}>
          <Sparkles size={14} /> 强制独显
        </Btn>
      </div>
      {status === "ready" && prefs.length === 0 ? (
        <div className="text-[12px] text-slate-500">
          尚未设置任何偏好 — 笔记本双显卡下，强制独显可避免游戏被错误调度到核显
        </div>
      ) : prefs.length > 0 ? (
        <div className="space-y-1.5">
          {prefs.map((p) => (
            <div key={p.exe} className="flex items-center gap-3 rounded-lg border border-white/6 bg-white/3 px-3.5 py-2">
              <Microchip size={14} className="shrink-0 text-slate-400" />
              <span className="min-w-0 flex-1 truncate text-[13px] text-slate-200" title={p.exe}>
                {shortName(p.exe)} <span className="text-[11px] text-slate-500">{p.exe}</span>
              </span>
              <select
                aria-label={`为 ${shortName(p.exe)} 设置图形处理器偏好`}
                value={p.pref}
                disabled={status !== "ready" || busy}
                onChange={(e) => void set(p.exe, Number(e.target.value))}
                className="rounded-md border border-white/10 bg-white/5 px-2 py-1 text-[12px] text-slate-200 outline-none hover:bg-white/10 disabled:opacity-50"
              >
                {[0, 1, 2].map((v) => (
                  <option key={v} value={v}>
                    {GPU_PREF_CN[v]}
                  </option>
                ))}
              </select>
            </div>
          ))}
        </div>
      ) : null}
      <div className="mt-2 text-[11px] text-slate-600">写入 Windows 图形设置（HKCU DirectX\UserGpuPreferences），修改即时生效、可在变更日志还原</div>
    </Card>
  );
}

function EppCard() {
  const [epp, setEpp] = useState<number | null>(null);
  const [eppStatus, setEppStatus] = useState<"loading" | "ready" | "error">("loading");
  const [busy, setBusy] = useState(false);
  const eppRequestId = useRef(0);
  const load = async () => {
    const requestId = ++eppRequestId.current;
    setEppStatus("loading");
    try {
      const value = await api.getEpp();
      if (requestId === eppRequestId.current) { setEpp(value); setEppStatus("ready"); }
    } catch {
      if (requestId === eppRequestId.current) setEppStatus("error");
    }
  };
  useEffect(() => {
    void load();
    return () => { eppRequestId.current += 1; };
  }, []);
  const set = async (v: number) => {
    if (eppStatus !== "ready" || epp == null || busy) return;
    setBusy(true);
    try {
      await api.setEpp(v);
      setEpp(v);
      toastOk(`能效偏好 EPP → ${v}（可在「设置 → 变更日志」还原）`);
    } catch (e) {
      toastErr(e);
      void load();
    } finally {
      setBusy(false);
    }
  };
  return (
    <Card title="能效偏好 EPP（Intel Speed Shift）" icon={<Info size={15} className="text-cyan-300" />}>
      {eppStatus === "loading" && <div role="status" className="mb-3 text-xs text-slate-500">正在读取 EPP 状态…</div>}
      {eppStatus === "error" && (
        <div role="alert" className="mb-3 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">EPP 读取失败，控制已禁用。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void load()}>重试</Btn>
        </div>
      )}
      <div className="flex items-center justify-between gap-4">
        <div className="text-[13px] text-slate-400">
          直接干预 CPU 调度器的性能-能耗权重：0 = 全核极速响应（游戏推荐），当前值
          <b className="ml-1 text-cyan-300">{eppStatus === "ready" ? epp ?? "当前设备不支持" : eppStatus === "error" ? "读取失败" : "读取中…"}</b>
        </div>
        <div className="flex shrink-0 gap-1.5">
          {[
            { v: 0, label: "极致性能" },
            { v: 25, label: "偏性能" },
            { v: 50, label: "均衡" },
            { v: 85, label: "偏节能" },
          ].map((o) => (
            <button
              key={o.v}
              disabled={busy || eppStatus !== "ready" || epp == null}
              onClick={() => void set(o.v)}
              className={`rounded-lg border px-3 py-1.5 text-[12px] transition-all disabled:opacity-40 ${
                epp === o.v
                  ? "border-cyan-400/40 bg-cyan-400/12 text-cyan-200"
                  : "border-white/10 bg-white/4 text-slate-300 hover:bg-white/10"
              }`}
            >
              {o.label}
            </button>
          ))}
        </div>
      </div>
    </Card>
  );
}
