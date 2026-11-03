// 处理器最大状态：当前电源计划是否把 CPU 限制在 100% 以下（会让睿频失效）。可一键恢复为 100%，并可在变更日志撤销。
import { useCallback, useEffect, useRef, useState } from "react";
import { Cpu, RefreshCw } from "lucide-react";
import { api, ProcessorState } from "../lib/api";
import { Btn, Card, Chip } from "./ui";
import { toastErr, toastOk } from "../lib/toast";

function pct(value: number | null) {
  return value === null ? "默认（100%）" : `${value}%`;
}

export function ProcessorLimitCard() {
  const [state, setState] = useState<ProcessorState | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const mounted = useRef(true);
  const requestId = useRef(0);

  const load = useCallback(async () => {
    const id = ++requestId.current;
    setLoading(true);
    setLoadError(null);
    try {
      const next = await api.getProcessorState();
      if (mounted.current && id === requestId.current) setState(next);
    } catch (error) {
      if (mounted.current && id === requestId.current) setLoadError(String(error));
    } finally {
      if (mounted.current && id === requestId.current) setLoading(false);
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void load();
    return () => {
      mounted.current = false;
    };
  }, [load]);

  const restore = async () => {
    if (busy || !state?.limited) return;
    const confirmed = confirm(
      `把当前电源计划的最大处理器状态从 ${pct(state.max_ac)}（交流）/ ${pct(state.max_dc)}（电池）改为 100%？\n\n` +
        "这只是取消频率上限，让 Windows 可以按需使用 CPU 的全部范围（含睿频），不会强制提高频率。" +
        "高负载下功耗和温度可能上升。写入前会记录原值，可在「设置 → 变更日志」撤销。",
    );
    if (!confirmed) return;
    setBusy(true);
    try {
      await api.removeProcessorLimit();
      toastOk("已把最大处理器状态恢复为 100%（可在变更日志撤销）");
    } catch (error) {
      toastErr(error);
    } finally {
      if (mounted.current) setBusy(false);
      void load();
    }
  };

  return (
    <Card
      className="min-w-0"
      title="处理器最大状态"
      icon={<Cpu size={15} className="text-cyan-300" />}
      right={
        <Btn variant="ghost" onClick={() => void load()} disabled={loading || busy}>
          <RefreshCw size={14} className={loading ? "animate-spin" : ""} /> {loading ? "读取中" : "刷新"}
        </Btn>
      }
    >
      {loadError && (
        <div role="alert" className="mb-3 rounded-lg border border-rose-400/20 bg-rose-400/5 px-3 py-2 text-[12px] text-rose-200">
          读取失败：{loadError}
        </div>
      )}
      {loading && state === null && <div className="h-16 animate-pulse rounded-lg bg-white/4" />}
      {state !== null && !loadError && (
        <div className="space-y-3 text-[13px]">
          <div className="flex flex-wrap items-center gap-2">
            <Chip tone={state.limited ? "amber" : "green"}>{state.limited ? "已限制" : "无限制"}</Chip>
            <span className="text-slate-300">
              交流 {pct(state.max_ac)} · 电池 {pct(state.max_dc)}
            </span>
          </div>
          <div className="text-[12px] leading-5 text-slate-400">
            当前电源计划 {state.scheme.slice(0, 8)}…。最大处理器状态低于 100% 会让 CPU 无法进入睿频范围，持续负载下性能受限；Windows 默认是 100%。
          </div>
          {state.written_by_aerotune && (
            <div className="rounded-lg border border-white/8 bg-white/3 px-3 py-2 text-[11px] leading-5 text-slate-400">
              变更日志里有 AeroTune 写入过的记录：{state.written_by_aerotune}
              {!state.restorable_from_journal && "（早期版本留下的记录，没有可还原的快照）"}
            </div>
          )}
          {state.limited ? (
            <Btn onClick={() => void restore()} disabled={busy || loading}>
              {busy ? "恢复中…" : "恢复为 100%"}
            </Btn>
          ) : (
            <div className="text-[12px] text-slate-500">无需处理。</div>
          )}
          <div className="text-[11px] text-slate-500">需要管理员权限。仅修改当前活动的电源计划，不修改其他计划。</div>
        </div>
      )}
    </Card>
  );
}
