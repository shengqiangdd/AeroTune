import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api } from "../lib/api";
import type { NetQuality, PingStat } from "../lib/api";
import {
  NETWORK_QUALITY_HISTORY_MAX_ENTRIES,
  NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS,
  normalizeNetworkQualityEntry,
  parseNetworkQualityHistoryEntry,
  trimNetworkQualityHistory,
} from "../lib/networkQualityHistory";
import type { NetworkQualityHistoryEntry } from "../lib/networkQualityHistory";
import { Btn, Card, Chip } from "./ui";

const STORAGE_KEY = "aerotune.network-quality-history.v1";
const MAX_ENTRIES = NETWORK_QUALITY_HISTORY_MAX_ENTRIES;
const MIN_SAMPLE_INTERVAL_MS = NETWORK_QUALITY_HISTORY_MIN_SAMPLE_INTERVAL_MS;
type HistoryEntry = NetworkQualityHistoryEntry;

type Aggregate = {
  latency: number | null;
  loss: number | null;
  samples: number;
};

function readHistory(): HistoryEntry[] {
  if (typeof window === "undefined") return [];

  try {
    const stored = window.localStorage.getItem(STORAGE_KEY);
    if (!stored) return [];

    const parsed: unknown = JSON.parse(stored);
    if (!Array.isArray(parsed)) return [];

    const now = Date.now();
    const entries = parsed
      .map((entry) => parseNetworkQualityHistoryEntry(entry, now))
      .filter((entry): entry is HistoryEntry => entry !== null);
    return trimNetworkQualityHistory(entries, now);
  } catch {
    return [];
  }
}

function persistHistory(entries: HistoryEntry[]) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(entries));
  } catch {
    // Storage may be unavailable or full; the current session still works.
  }
}

function aggregate(stats: Array<PingStat | null | undefined>): Aggregate {
  const available = stats.filter((stat): stat is PingStat => stat != null);
  const latencies = available
    .map((stat) => stat.avg)
    .filter((latency): latency is number => latency !== null);

  return {
    latency: latencies.length ? latencies.reduce((sum, latency) => sum + latency, 0) / latencies.length : null,
    loss: available.length
      ? available.reduce((sum, stat) => sum + stat.loss_pct, 0) / available.length
      : null,
    samples: available.length,
  };
}

function formatLatency(value: number | null): string {
  return value === null ? "—" : `${value.toFixed(1)} ms`;
}

function formatStatLatency(stat: PingStat): string {
  if (stat.avg !== null) return `${stat.avg.toFixed(1)} ms`;
  return stat.ok === 0 ? "不可达" : "—";
}

function formatLoss(value: number | null): string {
  return value === null ? "—" : `${value.toFixed(1)}%`;
}

function formatTime(ts: number): string {
  return new Date(ts).toLocaleString(undefined, {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function SummaryTile({
  label,
  aggregate: value,
  detail,
}: {
  label: string;
  aggregate: Aggregate;
  detail: string;
}) {
  return (
    <div className="rounded-lg border border-white/8 bg-white/3 px-3.5 py-3">
      <div className="text-[11px] text-slate-500">{label}</div>
      <div className="mt-1 text-lg font-semibold tabular-nums text-slate-100">
        {formatLatency(value.latency)}
      </div>
      <div className="mt-1 flex flex-wrap items-center justify-between gap-x-3 gap-y-1 text-[11px] text-slate-400">
        <span>平均丢包 {formatLoss(value.loss)}</span>
        <span>{detail}</span>
      </div>
    </div>
  );
}

export function NetworkQualityHistory() {
  const [history, setHistory] = useState<HistoryEntry[]>(readHistory);
  const [observing, setObserving] = useState(false);
  const [probing, setProbing] = useState(false);
  const [pageVisible, setPageVisible] = useState(
    () => typeof document === "undefined" || document.visibilityState === "visible",
  );
  const [probeError, setProbeError] = useState<string | null>(null);

  const historyRef = useRef(history);
  const observingRef = useRef(false);
  const probePendingRef = useRef(false);
  const mountedRef = useRef(false);
  const scheduleRef = useRef<(() => void) | null>(null);
  const historyEpochRef = useRef(0);
  const lastProbeAtRef = useRef(history.length ? history[history.length - 1].ts : 0);

  const recordSample = useCallback((quality: NetQuality, ts: number) => {
    const entry = normalizeNetworkQualityEntry(quality, ts);
    if (!entry) return false;

    const next = trimNetworkQualityHistory([...historyRef.current, entry], Date.now());
    historyRef.current = next;
    setHistory(next);
    persistHistory(next);
    return true;
  }, []);

  const clearHistory = useCallback(() => {
    historyEpochRef.current += 1;
    historyRef.current = [];
    setHistory([]);
    persistHistory([]);
    setProbeError(null);
  }, []);

  const stopObservation = useCallback(() => {
    observingRef.current = false;
    setObserving(false);
  }, []);

  const startObservation = useCallback(() => {
    if (probePendingRef.current) return;
    observingRef.current = true;
    setProbeError(null);
    setObserving(true);
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      observingRef.current = false;
    };
  }, []);

  useEffect(() => {
    persistHistory(historyRef.current);
  }, []);

  // Expire old entries even when observation is stopped; this housekeeping never probes.
  useEffect(() => {
    const timer = window.setInterval(() => {
      const current = historyRef.current;
      const next = trimNetworkQualityHistory(current, Date.now());
      if (next.length !== current.length) {
        historyRef.current = next;
        setHistory(next);
        persistHistory(next);
      }
    }, 60_000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    if (!observing) {
      observingRef.current = false;
      return;
    }

    if (typeof document === "undefined") return;
    observingRef.current = true;

    let disposed = false;
    let timer: number | null = null;

    const schedule = () => {
      if (disposed || !observingRef.current || document.visibilityState !== "visible") return;
      if (timer !== null) window.clearTimeout(timer);

      const elapsed = Date.now() - lastProbeAtRef.current;
      const wait = Math.max(0, MIN_SAMPLE_INTERVAL_MS - elapsed);
      timer = window.setTimeout(() => {
        timer = null;
        void probe();
      }, wait);
    };

    const probe = async () => {
      if (disposed || !observingRef.current || document.visibilityState !== "visible") return;
      // A previous request can outlive a stop/restart or a document visibility change.
      if (probePendingRef.current) return;

      probePendingRef.current = true;
      setProbing(true);
      const startedAt = Date.now();
      const epoch = historyEpochRef.current;
      lastProbeAtRef.current = startedAt;

      try {
        const quality: NetQuality = await api.pingQuality();
        if (
          mountedRef.current && observingRef.current &&
          document.visibilityState === "visible" && epoch === historyEpochRef.current
        ) {
          if (recordSample(quality, startedAt)) setProbeError(null);
          else setProbeError("探测结果格式异常，本次样本未保存。");
        }
      } catch {
        if (mountedRef.current && observingRef.current) {
          setProbeError("网络质量探测失败；观察仍会按间隔继续尝试。");
        }
      } finally {
        probePendingRef.current = false;
        if (mountedRef.current) setProbing(false);
        if (observingRef.current) {
          if (disposed) scheduleRef.current?.();
          else schedule();
        }
      }
    };

    const onVisibilityChange = () => {
      const visible = document.visibilityState === "visible";
      setPageVisible(visible);
      if (visible) {
        schedule();
      } else if (timer !== null) {
        window.clearTimeout(timer);
        timer = null;
      }
    };

    scheduleRef.current = schedule;
    document.addEventListener("visibilitychange", onVisibilityChange);
    setPageVisible(document.visibilityState === "visible");
    schedule();

    return () => {
      disposed = true;
      observingRef.current = false;
      if (timer !== null) window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisibilityChange);
      if (scheduleRef.current === schedule) scheduleRef.current = null;
    };
  }, [observing, recordSample]);

  const aggregates = useMemo(() => ({
    gateway: aggregate(history.map((entry) => entry.gateway)),
    dns: aggregate(history.map((entry) => entry.dns)),
  }), [history]);

  const latestFirst = useMemo(() => [...history].reverse(), [history]);

  const statusLabel = observing
    ? pageVisible
      ? probing ? "观察中 · 探测中" : "观察中"
      : "观察中 · 页面隐藏时暂停"
    : probing
      ? "已停止 · 当前探测收尾中"
      : "已停止";

  return (
    <Card
      title="网络质量历史"
      right={
        <div className="flex flex-wrap items-center justify-end gap-2">
          <Chip tone={observing ? "green" : "slate"}>{statusLabel}</Chip>
          {observing ? (
            <Btn variant="ghost" onClick={stopObservation}>停止观察</Btn>
          ) : (
            <Btn onClick={startObservation} disabled={probing}>开始观察</Btn>
          )}
          <Btn
            variant="danger"
            onClick={clearHistory}
            disabled={history.length === 0 && !probing}
          >
            清除历史
          </Btn>
        </div>
      }
    >
      <div className="space-y-4">
        <p className="text-[12px] leading-5 text-slate-400">
          仅在你点击“开始观察”后采样；组件卸载或页面隐藏时暂停，页面重新可见后继续。最短采样间隔为 5 分钟，300 条上限可覆盖 24 小时连续观察。
          探测只读取 ICMP 结果，不会自动更改 DNS；网关或 DNS 响应不代表具体应用或游戏服务器可达，也不等同于其实际延迟。
        </p>

        <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
          <SummaryTile
            label="网关平均 RTT"
            aggregate={aggregates.gateway}
            detail={`${aggregates.gateway.samples} 个样本`}
          />
          <SummaryTile
            label="DNS 平均 RTT"
            aggregate={aggregates.dns}
            detail={`${aggregates.dns.samples} 个样本`}
          />
          <div className="rounded-lg border border-white/8 bg-white/3 px-3.5 py-3">
            <div className="text-[11px] text-slate-500">保存样本</div>
            <div className="mt-1 text-lg font-semibold tabular-nums text-slate-100">
              {history.length}<span className="ml-1 text-sm font-normal text-slate-400">/ {MAX_ENTRIES}</span>
            </div>
            <div className="mt-1 text-[11px] text-slate-400">仅保留最近 24 小时</div>
          </div>
        </div>

        {probeError && (
          <div role="status" className="rounded-lg border border-rose-400/20 bg-rose-400/5 px-3.5 py-2.5 text-[12px] text-rose-300">
            {probeError}
          </div>
        )}

        {history.length === 0 ? (
          <div className="rounded-lg border border-dashed border-white/10 px-4 py-7 text-center text-[12px] text-slate-500">
            尚无样本。开始观察后将以至少 5 分钟的间隔记录网关与 DNS 的 ICMP 质量。
          </div>
        ) : (
          <div className="max-h-80 overflow-y-auto rounded-lg border border-white/8">
            <ol aria-label="网络质量探测时间线" className="divide-y divide-white/6">
              {latestFirst.map((entry, index) => (
                <li
                  key={`${entry.ts}-${index}`}
                  className="grid grid-cols-1 gap-2 px-3.5 py-3 sm:grid-cols-[minmax(130px,0.9fr)_repeat(3,minmax(0,1fr))] sm:items-center"
                >
                  <time dateTime={new Date(entry.ts).toISOString()} className="text-[11px] tabular-nums text-slate-500">
                    {formatTime(entry.ts)}
                  </time>
                  <TimelineStat label="网关" stat={entry.gateway} />
                  <TimelineStat label="DNS" stat={entry.dns} />
                  {entry.second_dns ? (
                    <TimelineStat label="DNS 2" stat={entry.second_dns} />
                  ) : (
                    <span className="hidden sm:block" aria-hidden="true" />
                  )}
                </li>
              ))}
            </ol>
          </div>
        )}
      </div>
    </Card>
  );
}

function TimelineStat({ label, stat }: { label: string; stat: PingStat | null }) {
  return (
    <div className="flex min-w-0 items-center justify-between gap-2 text-[11px]">
      <span className="text-slate-500">{label}</span>
      {stat ? (
        <span className="tabular-nums text-slate-300">
          {formatStatLatency(stat)} <span className="text-slate-500">·</span> 丢包 {stat.loss_pct.toFixed(0)}%
        </span>
      ) : (
        <span className="text-slate-600">—</span>
      )}
    </div>
  );
}

export default NetworkQualityHistory;
