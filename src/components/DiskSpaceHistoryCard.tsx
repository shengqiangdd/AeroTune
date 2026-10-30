import { memo, useEffect, useMemo, useRef, useState } from "react";
import { Clock3, HardDrive, Trash2 } from "lucide-react";
import type { DiskHistorySample } from "../lib/diskHistory";
import { appendDiskSamples, diskHistoryNeedsRefresh, DISK_HISTORY_SCHEMA, indexDiskHistory, normalizeDriveMount, sanitizeDiskHistory, validatedDiskFreeGb } from "../lib/diskHistory";
import type { Stats } from "../lib/api";
import { Btn, Card, Chip, Spark } from "./ui";

const STORAGE_KEY = "aerotune.disk-space-history.v1";
const WINDOW_SAMPLES = 48;

type StoredHistory = { version: number; samples: DiskHistorySample[] };

function readHistory(): DiskHistorySample[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || (parsed as StoredHistory).version !== DISK_HISTORY_SCHEMA) return [];
    return sanitizeDiskHistory(parsed, Date.now());
  } catch {
    return [];
  }
}

function persistHistory(samples: DiskHistorySample[]) {
  try {
    if (samples.length === 0) {
      window.localStorage.removeItem(STORAGE_KEY);
      return;
    }
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify({
      version: DISK_HISTORY_SCHEMA,
      samples: samples.map(({ ts, mount, totalGb, freeGb }) => ({ ts, mount, totalGb, freeGb })),
    }));
  } catch {
    // Storage quota or browser privacy settings must not affect the live dashboard.
  }
}

function gb(value: number): string {
  return `${value.toFixed(value >= 100 ? 0 : 1)} GB`;
}

function formatSampleTime(timestamp: number): string {
  return new Date(timestamp).toLocaleString([], { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

const DiskHistoryChart = memo(function DiskHistoryChart({ mount, samples }: { mount: string; samples: DiskHistorySample[] }) {
  const first = samples[0];
  const last = samples.at(-1);
  return (
    <>
      <div role="img" aria-label={`${mount} 最近 ${samples.length} 个磁盘可用空间样本趋势`} className="h-20 rounded-lg border border-white/5 bg-white/3 p-2">
        <Spark data={samples.map((sample) => sample.freeGb)} max={Math.max(...samples.map((sample) => sample.totalGb))} color="#fbbf24" height={72} />
      </div>
      <div className="mt-2 flex items-center justify-between gap-2 text-[10px] text-slate-500">
        <span>{first ? formatSampleTime(first.ts) : ""}</span>
        <span>{last ? formatSampleTime(last.ts) : ""}</span>
      </div>
    </>
  );
});

export function DiskSpaceHistoryCard({ stats }: { stats: Stats | null }) {
  const [history, setHistory] = useState<DiskHistorySample[]>(readHistory);
  const historyRef = useRef(history);
  const historyIndex = useMemo(() => indexDiskHistory(history), [history]);
  const [selectedMount, setSelectedMount] = useState<string | null>(null);

  useEffect(() => {
    historyRef.current = history;
  }, [history]);

  useEffect(() => {
    if (!stats || document.hidden || !Array.isArray(stats.disks)) return;
    const now = Date.now();
    if (!diskHistoryNeedsRefresh(historyIndex, stats.disks, now)) return;
    const result = appendDiskSamples(historyRef.current, stats.disks, now);
    if (!result.changed) return;
    historyRef.current = result.samples;
    setHistory(result.samples);
    persistHistory(result.samples);
  }, [historyIndex, stats]);

  const currentMounts = useMemo(
    () => (stats?.disks ?? []).map((disk) => normalizeDriveMount(disk.mount)).filter((mount): mount is string => mount !== null),
    [stats?.disks],
  );
  const mounts = useMemo(() => [...new Set([...currentMounts, ...history.map((sample) => sample.mount)])].sort(), [currentMounts, history]);
  const activeMount = selectedMount && mounts.includes(selectedMount) ? selectedMount : currentMounts[0] ?? mounts[0] ?? null;
  const samples = useMemo(
    () => activeMount ? history.filter((sample) => sample.mount === activeMount).slice(-WINDOW_SAMPLES) : [],
    [activeMount, history],
  );
  const currentDisk = (stats?.disks ?? []).find((disk) => normalizeDriveMount(disk.mount) === activeMount);
  const currentFree = currentDisk
    ? validatedDiskFreeGb(currentDisk.total_gb, currentDisk.used_gb)
    : null;
  const first = samples[0];
  const last = samples.at(-1);
  const delta = first && last && samples.length > 1 ? last.freeGb - first.freeGb : null;

  const clearHistory = () => {
    if (!confirm("清除 AeroTune 本地保存的磁盘空间趋势样本？不会删除或修改任何磁盘文件。")) return;
    historyRef.current = [];
    setHistory([]);
    persistHistory([]);
  };

  return (
    <Card
      title="磁盘空间趋势"
      icon={<HardDrive size={15} className="text-amber-300" />}
      right={(
        <div className="flex items-center gap-2">
          {mounts.length > 0 && (
            <select
              aria-label="选择趋势磁盘卷"
              value={activeMount ?? ""}
              onChange={(event) => setSelectedMount(event.target.value || null)}
              className="max-w-24 rounded-md border border-white/10 bg-black/20 px-2 py-1 text-xs text-slate-300 outline-none focus:border-cyan-400/40"
            >
              {mounts.map((mount) => <option key={mount} value={mount}>{mount}</option>)}
            </select>
          )}
          <Btn variant="ghost" className="px-2 py-1" onClick={clearHistory} disabled={history.length === 0} ariaLabel="清除磁盘趋势历史" title="清除本地趋势历史">
            <Trash2 size={13} />
          </Btn>
        </div>
      )}
    >
      {activeMount ? (
        <>
          <div className="mb-3 flex flex-wrap items-end justify-between gap-2">
            <div>
              <div className="text-2xl font-semibold tabular-nums text-slate-100">{currentFree == null ? "—" : gb(currentFree)}</div>
              <div className="mt-1 text-[11px] text-slate-500">
                {activeMount} {currentDisk ? currentFree === null ? "当前数据不可用" : "当前可用空间" : "当前未检测到"}
              </div>
            </div>
            {delta != null && (
              <Chip tone={delta < -0.1 ? "amber" : "green"}>
                样本区间 {delta > 0 ? "+" : ""}{delta.toFixed(1)} GB
              </Chip>
            )}
          </div>
          {samples.length > 1 ? (
            <DiskHistoryChart mount={activeMount} samples={samples} />
          ) : (
            <div className="flex h-20 flex-col items-center justify-center rounded-lg border border-dashed border-white/8 text-center">
              <Clock3 size={15} className="mb-1 text-slate-600" />
              <span className="text-xs text-slate-400">趋势基线建立中</span>
              <span className="mt-1 text-[10px] text-slate-600">约 30 分钟后采集下一点</span>
            </div>
          )}
        </>
      ) : (
        <div className="py-8 text-center text-xs text-slate-500">等待系统卷容量数据</div>
      )}
      <div className="mt-3 text-[10px] leading-4 text-slate-600">
        仪表盘可见时每 30 分钟采样；最多保留 14 天。仅本机保存盘符、容量与时间，不收集文件路径；变化仅作历史参考，不预测耗尽时间。
      </div>
    </Card>
  );
}
