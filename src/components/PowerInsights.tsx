import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { BatteryCharging, Clock3, PlugZap, Trash2 } from "lucide-react";
import type { Stats } from "../lib/api";
import type { StatsStatus } from "../hooks/useStats";
import {
  AC_DRAIN_MAX_SAMPLE_GAP_MS,
  AC_DRAIN_MIN_COVERAGE_MS,
  AC_DRAIN_MIN_DROP_PERCENT,
  AC_DRAIN_MIN_SAMPLES,
  BATTERY_MAX_SAMPLE_GAP_MS,
  BATTERY_SAMPLE_INTERVAL_MS,
  batteryAcState,
  batteryQueryFailed,
  estimateAcDrain,
  estimateDischarge,
} from "../lib/powerEstimate";
import { Btn, Card, Chip, Toggle } from "./ui";

type PowerSample = {
  timestamp: number;
  acConnected: boolean | null;
  percent: number;
};

const STORAGE_KEY = "aerotune.power-insights.samples.v1";
const SAMPLE_INTERVAL_MS = BATTERY_SAMPLE_INTERVAL_MS;
const RETENTION_MS = 48 * 60 * 60 * 1000;

function sampleFromStats(stats: Stats | null, timestamp: number, statsStatus: StatsStatus): PowerSample | null {
  if (statsStatus !== "live" || stats?.battery_status_ok !== true || batteryQueryFailed(stats)) return null;
  const battery = stats?.battery;
  const percent = battery?.percent;
  if (!battery || percent == null || !Number.isFinite(percent) || percent < 0 || percent > 100) return null;

  const acConnected = batteryAcState(battery);
  return { timestamp, acConnected, percent };
}

function sanitizeSamples(value: unknown): PowerSample[] {
  if (!Array.isArray(value)) return [];
  return value
    .filter((item): item is Record<string, unknown> => typeof item === "object" && item !== null)
    .map((item) => ({
      timestamp: item.timestamp,
      acConnected: item.acConnected,
      percent: item.percent,
    }))
    .filter(
      (item): item is PowerSample =>
        typeof item.timestamp === "number" &&
        Number.isFinite(item.timestamp) &&
        (item.acConnected === null || typeof item.acConnected === "boolean") &&
        typeof item.percent === "number" &&
        Number.isFinite(item.percent) &&
        item.percent >= 0 &&
        item.percent <= 100,
    )
    .sort((a, b) => a.timestamp - b.timestamp);
}

function pruneSamples(samples: PowerSample[], now: number): PowerSample[] {
  const cutoff = now - RETENTION_MS;
  return samples.filter((sample) => sample.timestamp >= cutoff && sample.timestamp <= now);
}

function writeSamples(samples: PowerSample[]) {
  if (samples.length === 0) {
    window.localStorage.removeItem(STORAGE_KEY);
    return;
  }
  // Serialize only the three permitted data fields.
  window.localStorage.setItem(
    STORAGE_KEY,
    JSON.stringify(samples.map(({ timestamp, acConnected, percent }) => ({ timestamp, acConnected, percent }))),
  );
}

function loadSamples(): PowerSample[] {
  const raw = window.localStorage.getItem(STORAGE_KEY);
  if (!raw) return [];
  return sanitizeSamples(JSON.parse(raw));
}

function formatDurationHours(hours: number | null): string {
  if (hours == null || !Number.isFinite(hours) || hours < 0) return "未知";
  const totalMinutes = Math.round(hours * 60);
  const h = Math.floor(totalMinutes / 60);
  const m = totalMinutes % 60;
  return h > 0 ? `${h}时 ${m}分` : `${m}分钟`;
}

function formatNativeEstimate(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0) return "未知";
  return formatDurationHours(seconds / 3600);
}

function formatSampleTime(timestamp: number | undefined): string {
  if (timestamp == null) return "暂无";
  return new Date(timestamp).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}

export function PowerInsights({ stats, statsStatus }: { stats: Stats | null; statsStatus: StatsStatus }) {
  const [enabled, setEnabled] = useState(false);
  const [samples, setSamples] = useState<PowerSample[]>([]);
  const [storageError, setStorageError] = useState(false);
  const [clockNow, setClockNow] = useState(() => Date.now());
  const samplesRef = useRef<PowerSample[]>([]);
  const latestStatsRef = useRef(stats);
  const latestStatsStatusRef = useRef(statsStatus);
  const lastSampleAtRef = useRef(0);
  const storageReadyRef = useRef(true);
  const acDrainInvalidatedAtRef = useRef<number | null>(null);

  latestStatsRef.current = stats;
  latestStatsStatusRef.current = statsStatus;

  // Never combine an AC-drain trend across an observed unknown, stale, or unplugged interval.
  useEffect(() => {
    const battery = statsStatus === "live" && stats?.battery_status_ok === true && !batteryQueryFailed(stats) ? stats?.battery ?? null : null;
    const percent = battery?.percent;
    const validPercent = percent != null && Number.isFinite(percent) && percent >= 0 && percent <= 100;
    if (!enabled || statsStatus !== "live" || stats?.battery_status_ok !== true || batteryQueryFailed(stats) || batteryAcState(battery) !== true || !validPercent) {
      acDrainInvalidatedAtRef.current = Date.now();
    }
  }, [enabled, statsStatus, stats?.battery_status_ok, stats?.battery?.ac_connected, stats?.battery?.percent]);

  const replaceSamples = useCallback((next: PowerSample[]) => {
    samplesRef.current = next;
    setSamples(next);
  }, []);

  // Prune expired opt-in data on page open even when recording is currently off.
  useEffect(() => {
    storageReadyRef.current = true;
    try {
      const cleaned = pruneSamples(loadSamples(), Date.now());
      writeSamples(cleaned);
      replaceSamples(cleaned);
      lastSampleAtRef.current = cleaned[cleaned.length - 1]?.timestamp ?? 0;
      setStorageError(false);
    } catch {
      storageReadyRef.current = false;
      setStorageError(true);
      replaceSamples([]);
      lastSampleAtRef.current = 0;
    }
  }, [replaceSamples]);

  useEffect(() => {
    if (!enabled) return;
    storageReadyRef.current = true;

    try {
      const cleaned = pruneSamples(loadSamples(), Date.now());
      writeSamples(cleaned);
      replaceSamples(cleaned);
      lastSampleAtRef.current = cleaned[cleaned.length - 1]?.timestamp ?? 0;
      setStorageError(false);
    } catch {
      storageReadyRef.current = false;
      setStorageError(true);
      replaceSamples([]);
      lastSampleAtRef.current = 0;
    }

    const collect = () => {
      const now = Date.now();
      setClockNow(now);
      if (!storageReadyRef.current) return;
      const current = pruneSamples(samplesRef.current, now);
      const pruned = current.length !== samplesRef.current.length;
      if (now - lastSampleAtRef.current < SAMPLE_INTERVAL_MS) {
        if (pruned) {
          try {
            writeSamples(current);
            replaceSamples(current);
          } catch {
            storageReadyRef.current = false;
            setStorageError(true);
          }
        }
        return;
      }

      const sample = sampleFromStats(latestStatsRef.current, now, latestStatsStatusRef.current);
      if (!sample) {
        if (pruned) {
          try {
            writeSamples(current);
            replaceSamples(current);
          } catch {
            storageReadyRef.current = false;
            setStorageError(true);
          }
        }
        return;
      }

      const next = pruneSamples([...current, sample], now);
      try {
        writeSamples(next);
        lastSampleAtRef.current = now;
        replaceSamples(next);
        setStorageError(false);
      } catch {
        storageReadyRef.current = false;
        setStorageError(true);
      }
    };

    collect();
    const timer = window.setInterval(collect, 10_000);
    return () => window.clearInterval(timer);
  }, [enabled, replaceSamples]);

  const clearSamples = () => {
    try {
      window.localStorage.removeItem(STORAGE_KEY);
      storageReadyRef.current = true;
      replaceSamples([]);
      lastSampleAtRef.current = Date.now();
      setStorageError(false);
    } catch {
      storageReadyRef.current = false;
      setStorageError(true);
    }
  };

  const telemetryLive = statsStatus === "live";
  const battery = !telemetryLive || stats?.battery_status_ok !== true || batteryQueryFailed(stats) ? null : stats?.battery ?? null;
  const currentPercent =
    battery?.percent != null && Number.isFinite(battery.percent) && battery.percent >= 0 && battery.percent <= 100
      ? battery.percent
      : null;
  const currentAcState = batteryAcState(battery);
  const estimate = useMemo(
    () => telemetryLive && enabled && !storageError && currentAcState === false
      ? estimateDischarge(samples, currentPercent)
      : null,
    [telemetryLive, enabled, storageError, currentAcState, currentPercent, samples],
  );
  const latestSample = samples[samples.length - 1];
  const pluggedDrain = useMemo(() => {
    if (
      !telemetryLive || stats?.battery_status_ok !== true || !enabled || storageError || currentAcState !== true || currentPercent == null ||
      latestSample?.acConnected !== true || clockNow - latestSample.timestamp > AC_DRAIN_MAX_SAMPLE_GAP_MS
    ) return null;
    const runSamples = samples.filter((sample) => sample.timestamp > (acDrainInvalidatedAtRef.current ?? Number.NEGATIVE_INFINITY));
    return estimateAcDrain(runSamples);
  }, [telemetryLive, stats?.battery_status_ok, enabled, storageError, currentAcState, currentPercent, latestSample, samples, clockNow]);

  const collectionStatus = statsStatus === "error"
    ? "实时遥测连接失败 · 暂不判断"
    : statsStatus === "stale"
      ? "实时遥测已停止更新 · 暂不判断"
      : statsStatus === "loading"
        ? "等待实时遥测"
        : batteryQueryFailed(stats)
          ? "电池接口查询失败 · 暂不估算"
          : stats?.battery_status_ok !== true
            ? "电池接口状态未知 · 暂不估算"
            : !enabled
            ? "尚未开启本地样本记录"
            : storageError
              ? "本地存储不可用"
              : pluggedDrain
                ? "接电期间观测到净放电"
                : estimate
                  ? "电池续航估算就绪"
                  : currentAcState === false
                    ? "收集电池续航样本"
                    : currentAcState === true
                      ? "收集样本中 · 观察接电电量变化"
                      : "电源状态未知 · 暂不估算";

  return (
    <Card
      title="电池续航洞察"
      icon={<BatteryCharging size={15} className="text-emerald-300" />}
      right={<Chip role="status" tone={estimate || pluggedDrain ? "green" : statsStatus === "error" || statsStatus === "stale" || batteryQueryFailed(stats) || (statsStatus === "live" && stats?.battery_status_ok !== true) ? "amber" : "slate"}>{collectionStatus}</Chip>}
      className="min-w-0"
    >
      <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2 text-xs text-slate-400">
          {currentAcState === true ? (
            <PlugZap size={14} className="text-emerald-300" />
          ) : (
            <BatteryCharging size={14} className="text-amber-300" />
          )}
          <span>
            {currentAcState === true ? "已接通电源" : currentAcState === false ? "使用电池" : "电源状态未知"}
            {currentPercent == null ? " · 电量未知" : ` · ${currentPercent.toFixed(0)}%`}
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <Toggle label="本地电量样本记录" on={enabled} onChange={setEnabled} />
          <Btn variant="danger" onClick={clearSamples}>
            <Trash2 size={14} /> 清除本地样本
          </Btn>
        </div>
      </div>

      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
        <section className="rounded-xl border border-white/8 bg-white/3 p-3" aria-label="本地观察估算">
          <div className="flex items-center gap-1.5 text-xs text-slate-400">
            <Clock3 size={13} className="text-cyan-300" /> 本地观察估算
          </div>
          <div className="mt-2 text-lg font-semibold tabular-nums text-slate-100">
            {estimate ? `约 ${formatDurationHours(estimate.hours)}` : "未知"}
          </div>
          <div className="mt-1 space-y-1 text-[11px] text-slate-400">
            <div>
              观测范围：
              {estimate
                ? `${formatDurationHours(estimate.rangeHours[0])}–${formatDurationHours(estimate.rangeHours[1])}`
                : "未知"}
            </div>
            <div>放电斜率：{estimate ? `${estimate.percentPerHour.toFixed(1)}%/小时` : "未知"}</div>
            <div>
              质量：{estimate ? estimate.quality : "未知"}
              {estimate ? ` · ${estimate.sampleCount} 条 / ${Math.round(estimate.spanMinutes)} 分钟` : ""}
            </div>
          </div>
        </section>

        <section className="rounded-xl border border-white/8 bg-white/3 p-3" aria-label="Windows 原生估算">
          <div className="flex items-center gap-1.5 text-xs text-slate-400">
            <PlugZap size={13} className="text-violet-300" /> Windows 原生估算
          </div>
          <div className="mt-2 text-lg font-semibold tabular-nums text-slate-100">
            {formatNativeEstimate(battery?.secs_left)}
          </div>
          <div className="mt-1 text-[11px] text-slate-500">来源：Windows 电池遥测</div>
        </section>

        <section className="min-w-0 rounded-xl border border-white/8 bg-white/3 p-3" aria-label="接电期间电量观察">
          <div className="flex items-center gap-1.5 text-xs text-slate-400">
            <PlugZap size={13} className="text-emerald-300" /> 接电期间电量观察
          </div>
          <div className="mt-2 break-words text-lg font-semibold tabular-nums text-slate-100">
            {pluggedDrain ? `约 ${pluggedDrain.percentPerHour.toFixed(1)} 个百分点/小时` : enabled && telemetryLive && currentAcState === true ? "观察中" : "暂不可用"}
          </div>
          <div className="mt-1 min-h-8 text-[11px] leading-4 text-slate-400">
            {pluggedDrain
              ? `最近 ${Math.round(pluggedDrain.spanMinutes)} 分钟的采样点均报告 AC；电量净降 ${pluggedDrain.dropPercent.toFixed(1)} 个百分点 · ${pluggedDrain.sampleCount} 条样本`
              : !enabled
                ? "开启本地样本记录后观察；默认不记录。"
                : !telemetryLive
                  ? "实时遥测未更新，暂不判断。"
                  : batteryQueryFailed(stats)
                    ? "电池接口查询失败，暂不判断。"
                    : stats?.battery_status_ok !== true
                      ? "电池接口状态未知，暂不判断。"
                      : currentAcState === false
                        ? "仅在接通电源时观察；当前为电池供电。"
                        : currentAcState === true
                          ? `需要覆盖至少 ${Math.round(AC_DRAIN_MIN_COVERAGE_MS / 60_000)} 分钟、${AC_DRAIN_MIN_SAMPLES} 条均报告 AC 的样本，且净降至少 ${AC_DRAIN_MIN_DROP_PERCENT} 个百分点。`
                          : "供电状态或电量未知，不作判断。"}
          </div>
          <p className="mt-2 text-[10px] leading-4 text-slate-600">
            只报告电量百分比变化；AC 在线不代表适配器功率足够，也不能据此诊断硬件故障。
          </p>
        </section>
      </div>

      <div className="mt-3 flex flex-wrap items-center justify-between gap-2 text-[11px] text-slate-500" aria-live="polite">
        <span>
          {enabled
            ? `本机样本 ${samples.length} 条 · 最近采样 ${formatSampleTime(latestSample?.timestamp)}`
            : "开启后开始记录；关闭时停止新增样本"}
        </span>
        <span className="min-w-0 break-words">仅保存时间、电源状态和有效查询样本；每分钟至多一条。过期样本在重新打开本页或开启记录时按48小时留存窗口清理。接电趋势只说明已记录的采样点报告 AC，点间瞬时断连不可见；电池估算间隔超{BATTERY_MAX_SAMPLE_GAP_MS / 60_000}分钟、接电趋势超{AC_DRAIN_MAX_SAMPLE_GAP_MS / 60_000}分钟时重新累计。</span>
      </div>
      {!enabled && (
        <p className="mt-1 text-[11px] text-slate-600">样本记录默认关闭；开启后样本仅存于此设备的 localStorage。</p>
      )}
      {storageError && (
        <p className="mt-2 text-[11px] text-rose-300" role="status">
          本地样本存储读取或写入失败；估算保持未知。
        </p>
      )}
    </Card>
  );
}
