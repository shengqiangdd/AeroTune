// 编码模式：平衡计划 + 前台响应优先 + 内存整理
import { useEffect, useRef, useState } from "react";
import { Code2, Gauge, Sparkles, Info } from "lucide-react";
import { api, Stats, TweakStates } from "../lib/api";
import { Card, Btn, Toggle, Chip, PageHead, ModeBanner } from "../components/ui";
import { toastOk, toastErr } from "../lib/toast";
import { ScenarioPreviewCard } from "../components/ScenarioPreviewCard";
import { validCapacityPair } from "../lib/resourceEvidence";

export function DevMode({ stats }: { stats: Stats | null }) {
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
      if (requestId === modeRequestId.current) { setOn(mode === "dev"); setModeStatus("ready"); }
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
      const applied = await api.applyMode(v ? "dev" : "none");
      setOn(v);
      setModeStatus("ready");
      void refresh();
      toastOk(v ? applied.join("；") : "已退出编码模式");
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

  const trim = async () => {
    try {
      const [n, mb] = await api.trimMemory();
      toastOk(`内存整理完成：${n} 个进程，约 ${mb.toFixed(0)} MB 处理前工作集（不代表释放量）`);
    } catch (e) {
      toastErr(e);
    }
  };

  return (
    <div>
      <PageHead
        title="编码模式"
        desc="为 IDE 和编译器提供平衡计划与前台响应优先；内存整理保持独立手动操作"
        right={<Chip tone="cyan">响应优先</Chip>}
      />

      <ModeBanner active={modeStatus === "ready" && on} name="编码模式" onExit={() => toggleMode(false)} />
      {modeStatus === "loading" && <div role="status" className="mb-4 text-xs text-slate-500">正在读取当前模式…</div>}
      {modeStatus === "error" && (
        <div role="alert" className="mb-4 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">模式状态读取失败，开关暂不可用。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void loadMode()}>重试</Btn>
        </div>
      )}
      {statesStatus === "loading" && <div role="status" className="mb-4 text-xs text-slate-500">正在读取开发调优状态…</div>}
      {statesStatus === "error" && (
        <div role="alert" className="mb-4 flex flex-wrap items-start justify-between gap-3 text-xs text-amber-200">
          <span className="min-w-0 flex-1 break-words">调优状态读取失败，前台响应开关已禁用。</span>
          <Btn variant="ghost" className="shrink-0" onClick={() => void refresh()}>重试</Btn>
        </div>
      )}
      <div className="mb-5"><ScenarioPreviewCard mode="dev" /></div>

      <div className="mb-5 grid grid-cols-1 gap-4 2xl:grid-cols-3">
        <Card className="min-w-0 2xl:col-span-2">
          <div className="flex flex-wrap items-center gap-4">
            <div className="flex h-14 w-14 items-center justify-center rounded-2xl bg-cyan-400/12">
              <Code2 size={26} className="text-cyan-300" />
            </div>
            <div className="flex-1">
              <div className="text-base font-medium text-slate-100">一键开启编码模式</div>
              <div className="mt-0.5 text-[13px] text-slate-400">
                切换到平衡电源计划 · 前台响应优先（Win32PrioritySeparation）· 不自动整理内存
              </div>
            </div>
            <Toggle label="编码模式" on={on} onChange={toggleMode} disabled={busy || modeStatus !== "ready"} />
          </div>
          <div className="mt-4 flex gap-2 border-t border-white/6 pt-4 text-[12px] text-slate-400">
            <Info size={14} className="mt-px shrink-0 text-cyan-300/80" />
            前台响应优先会增大前台进程的时间片权重，输入延迟与编译输出更顺滑
          </div>
        </Card>

        <Card title="实时负载" icon={<Gauge size={15} className="text-cyan-300" />}>
          <div className="space-y-2 text-[13px] text-slate-300">
            <div className="flex justify-between">
              <span className="text-slate-400">CPU</span>
              <span className="tabular-nums">{stats?.cpu.toFixed(0) ?? "-"}%</span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-400">系统已用</span>
              <span className="tabular-nums">
                {stats && validCapacityPair(stats.mem_used_gb, stats.mem_total_gb) ? `${stats.mem_used_gb.toFixed(1)} / ${stats.mem_total_gb.toFixed(0)} GB` : "数据不可用"}
              </span>
            </div>
            <div className="flex justify-between">
              <span className="text-slate-400">进程数</span>
              <span className="tabular-nums">{stats?.process_count ?? "-"}</span>
            </div>
          </div>
        </Card>
      </div>

      <div className="grid grid-cols-1 gap-4 xl:grid-cols-2">
        <Card title="前台响应优先" icon={<Sparkles size={15} className="text-violet-300" />}>
          <div className="flex items-center justify-between">
            <div className="pr-4 text-[13px] text-slate-400">
              调整 Win32PrioritySeparation=0x26：前台进程获得更长调度量子，编辑器 / 终端交互更跟手（需要管理员）
            </div>
            <Toggle label="前台响应优先" on={states?.priority_boost ?? false} disabled={statesStatus !== "ready" || states?.priority_boost == null} onChange={(v) => flip("priority_boost", v)} />
          </div>
        </Card>

        <Card title="内存整理" icon={<Sparkles size={15} className="text-emerald-300" />}>
          <div className="text-[13px] text-slate-400">
            对占用超过 80MB 的进程尝试 EmptyWorkingSet。可能造成换页与卡顿，不清空备用缓存，不保证性能收益。
          </div>
          <Btn className="mt-3 w-full" variant="ghost" onClick={trim}>
            立即整理
          </Btn>
        </Card>
      </div>
    </div>
  );
}
