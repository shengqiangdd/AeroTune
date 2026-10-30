import { memo, useMemo, useState } from "react";
import { AlertTriangle, CalendarClock, CheckCircle2, Download, RefreshCw, ShieldCheck } from "lucide-react";
import { api } from "../lib/api";
import type { ReliabilityEvent, ReliabilityEventKind, ReliabilitySnapshot } from "../lib/api";
import {
  assessReliabilitySnapshot,
  buildAppIssueDayBuckets,
  buildRecurringAppGroups,
  filterReliabilityEvents,
  updateReliabilityFilters,
} from "../lib/reliabilityTimeline.js";
import type { ReliabilityEventFilter, ReliabilityViewState } from "../lib/reliabilityTimeline.js";
import { toastErr, toastOk } from "../lib/toast";
import { Btn, Card, Chip } from "./ui";

type EventFilter = ReliabilityEventFilter;

const EVENT_META: Record<ReliabilityEventKind, { label: string; tone: "rose" | "amber" | "green" | "slate" }> = {
  app_crash: { label: "应用崩溃", tone: "rose" },
  app_hang: { label: "应用挂起", tone: "amber" },
  unexpected_shutdown: { label: "非正常关机", tone: "rose" },
  planned_shutdown: { label: "计划关机", tone: "slate" },
  update_installed: { label: "更新成功", tone: "green" },
  update_failed: { label: "更新失败", tone: "amber" },
};

const ISSUE_KINDS = new Set<ReliabilityEventKind>(["app_crash", "app_hang", "unexpected_shutdown"]);
const UPDATE_KINDS = new Set<ReliabilityEventKind>(["update_installed", "update_failed"]);
const PAGE_SIZE = 12;

function formatTime(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  return Number.isNaN(date.getTime()) ? "时间未知" : date.toLocaleString();
}

function formatCaptureTime(timestamp: number | null): string {
  return timestamp == null ? "未读取" : new Date(timestamp).toLocaleString();
}

export const ReliabilityTimeline = memo(function ReliabilityTimeline() {
  const [snapshot, setSnapshot] = useState<ReliabilitySnapshot | null>(null);
  const [lastFetchedAt, setLastFetchedAt] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [viewState, setViewState] = useState<ReliabilityViewState>({ eventFilter: "all", appKey: null, visibleCount: PAGE_SIZE });
  const { eventFilter: filter, appKey, visibleCount } = viewState;
  const [days, setDays] = useState(30);
  const setEventFilter = (eventFilter: EventFilter) => setViewState((current) => updateReliabilityFilters(current, { eventFilter, appKey: null }, PAGE_SIZE));
  const toggleAppFilter = (nextKey: string) => setViewState((current) => updateReliabilityFilters(current, { appKey: current.appKey === nextKey ? null : nextKey }, PAGE_SIZE));
  const showMore = () => setViewState((current) => ({ ...current, visibleCount: current.visibleCount + PAGE_SIZE }));

  const events = snapshot?.events ?? null;
  const load = async () => {
    if (loading) return;
    const requestedDays = days;
    setLoading(true);
    setError(null);
    try {
      const nextSnapshot = await api.reliabilityEvents(requestedDays);
      setSnapshot(nextSnapshot);
      setLastFetchedAt(Date.now());
      setViewState((current) => ({ ...current, visibleCount: PAGE_SIZE }));
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const changeRange = (nextDays: number) => {
    if (loading || nextDays === days) return;
    setDays(nextDays);
    setSnapshot(null);
    setLastFetchedAt(null);
    setError(null);
    setViewState((current) => updateReliabilityFilters(current, { eventFilter: "all", appKey: null }, PAGE_SIZE));
  };

  const exportEvents = async () => {
    if (!snapshot || exporting) return;
    setExporting(true);
    try {
      const filename = `AeroTune-reliability-${days}d-${new Date().toISOString().slice(0, 10)}.json`;
      const report = {
        app: "AeroTune",
        report_type: "windows_reliability_events",
        exported_at_utc: new Date().toISOString(),
        snapshot_captured_at_utc: lastFetchedAt == null ? null : new Date(lastFetchedAt).toISOString(),
        ...snapshot,
      };
      const saved = await api.saveTextExport(filename, JSON.stringify(report, null, 2));
      if (!saved) return;
      toastOk(`已导出 ${snapshot.events.length} 条可靠性事件摘要`);
    } catch (reason) {
      toastErr(reason);
    } finally {
      setExporting(false);
    }
  };

  const filteredEvents = useMemo(
    () => (events ? filterReliabilityEvents(events, filter, appKey) : []),
    [events, filter, appKey],
  );
  const { issueCount, updateCount } = useMemo(() => {
    let issues = 0;
    let updates = 0;
    for (const event of events ?? []) {
      if (ISSUE_KINDS.has(event.kind)) issues += 1;
      if (UPDATE_KINDS.has(event.kind)) updates += 1;
    }
    return { issueCount: issues, updateCount: updates };
  }, [events]);
  const recurringApps = useMemo(() => buildRecurringAppGroups(events ?? []), [events]);
  const selectedAppName = useMemo(
    () => appKey ? events?.find((event) => event.title.trim().replace(/\s+/g, " ").toLowerCase() === appKey)?.title ?? appKey : null,
    [appKey, events],
  );
  const localTimeZone = Intl.DateTimeFormat().resolvedOptions().timeZone || "本地时区";
  const appDayBuckets = useMemo(
    () => appKey && events && snapshot ? buildAppIssueDayBuckets(events, appKey, days, lastFetchedAt ?? Date.now(), localTimeZone) : [],
    [appKey, events, snapshot, days, lastFetchedAt, localTimeZone],
  );
  const snapshotQuality = snapshot ? assessReliabilitySnapshot(snapshot) : null;
  const maxAppDailyCount = Math.max(1, appDayBuckets.reduce((max, bucket) => Math.max(max, bucket.crashes + bucket.hangs), 0));
  const selectedAppIssueCount = appDayBuckets.reduce((sum, bucket) => sum + bucket.crashes + bucket.hangs, 0);
  const unavailableSources = snapshot?.sources.filter((source) => !source.available) ?? [];
  const cappedSources = snapshot?.sources.filter((source) => source.limit_reached) ?? [];
  const liveMessage = loading
    ? `正在读取近 ${days} 天的 Windows 可靠性事件。`
    : error
      ? snapshot ? `刷新失败，保留上次成功快照，读取于 ${formatCaptureTime(lastFetchedAt)}。` : `读取事件失败：${error}`
      : snapshot ? `读取完成，返回 ${snapshot.events.length} 条事件；${unavailableSources.length} 个日志来源不可用。` : "";

  return (
    <Card
      title={`系统可靠性时间线 · 近 ${days} 天`}
      icon={<ShieldCheck size={15} className="text-cyan-300" />}
      right={
        <div className="flex items-center gap-2">
          <label className="sr-only" htmlFor="reliability-range">事件查询范围</label>
          <select
            id="reliability-range"
            aria-label="事件查询范围"
            value={days}
            onChange={(event) => changeRange(Number(event.target.value))}
            disabled={loading}
            className="rounded-md border border-white/10 bg-slate-900 px-2 py-1.5 text-[11px] text-slate-300 outline-none focus-visible:ring-2 focus-visible:ring-cyan-300 disabled:opacity-50"
          >
            <option value={7}>近 7 天</option>
            <option value={30}>近 30 天</option>
            <option value={90}>近 90 天</option>
          </select>
          <Btn variant="ghost" onClick={load} disabled={loading}>
            <RefreshCw size={13} className={loading ? "animate-spin" : ""} />
            {loading ? "读取中…" : snapshot ? "刷新" : "读取事件"}
          </Btn>
        </div>
      }
    >
      <div role="region" aria-label="系统可靠性事件结果" aria-busy={loading}>
        <div className="sr-only" role="status" aria-live="polite" aria-atomic="true">{liveMessage}</div>
        {snapshot && lastFetchedAt != null && (
          <div className="mb-3 text-[11px] text-slate-400" aria-label="最近成功读取时间">
            最近读取：{formatCaptureTime(lastFetchedAt)}{loading ? " · 正在刷新，暂时显示此快照" : error ? " · 刷新失败，当前为旧快照" : ""}
          </div>
        )}

        {error && (
          <div role="status" className="mb-3 rounded-lg border border-rose-300/10 bg-rose-300/4 px-3 py-2.5 text-xs text-rose-200">
            {snapshot ? "刷新失败，保留上次成功结果：" : "读取事件日志失败："}{error}
          </div>
        )}
        {snapshot && unavailableSources.length > 0 && (
          <div role="status" className="mb-3 rounded-lg border border-amber-300/10 bg-amber-300/4 px-3 py-2.5 text-xs text-amber-200">
            部分日志来源不可读取，结果不完整：{unavailableSources.map((source) => `${source.label}${source.error ? `（${source.error}）` : ""}`).join("、")}。
          </div>
        )}
        {snapshot && (snapshot.omitted_estimate > 0 || cappedSources.length > 0) && (
          <div role="status" className="mb-3 rounded-lg border border-amber-300/10 bg-amber-300/4 px-3 py-2.5 text-xs text-amber-200">
            当前为有上限的事件样本：{snapshot.omitted_estimate > 0 ? `至少 ${snapshot.omitted_estimate} 条已采集事件未包含在返回列表。` : ""}
            {cappedSources.length > 0 ? `以下来源触及单次读取上限，可能还有更早记录：${cappedSources.map((source) => source.label).join("、")}。` : ""}
          </div>
        )}
        {snapshot && snapshot.invalid_rows > 0 && (
          <div role="status" className="mb-3 rounded-lg border border-amber-300/10 bg-amber-300/4 px-3 py-2.5 text-xs text-amber-200">
            有 {snapshot.invalid_rows} 条事件记录字段异常，已跳过；不会把解析失败显示为“无事件”。
          </div>
        )}

        <div className="mb-3 flex flex-wrap items-center gap-2" aria-label="可靠性事件摘要">
          <Chip tone={issueCount > 0 ? "amber" : "green"}>{snapshot ? `故障类样本 ${issueCount} 条` : "尚未读取"}</Chip>
          {snapshot && <Chip tone="cyan">更新样本 {updateCount} 条</Chip>}
          {snapshot && <Chip tone={snapshot.omitted_estimate > 0 ? "amber" : "slate"}>采集 {snapshot.candidate_count} · 返回 {snapshot.events.length} / 200 条</Chip>}
        </div>

        {snapshot && (
          <div className="mb-3 flex flex-wrap items-center gap-1.5" role="group" aria-label="Windows 事件日志来源状态">
            {snapshot.sources.map((source) => (
              <Chip
                key={source.id}
                tone={!source.available ? "rose" : source.limit_reached ? "amber" : "green"}
                className="text-[10px]"
              >
                {source.label} · {!source.available ? "不可读" : `${source.fetched}${source.limit_reached ? "+" : ""} 条`}
              </Chip>
            ))}
          </div>
        )}

        {snapshot && (
          <div className="mb-3 flex flex-wrap items-center gap-2">
            <div className="flex flex-wrap gap-1" role="group" aria-label="筛选可靠性事件">
              {(["all", "issues", "system", "updates"] as const).map((value) => (
                <button
                  key={value}
                  type="button"
                  aria-pressed={filter === value}
                  onClick={() => setEventFilter(value)}
                  className={`rounded-md px-2 py-1 text-[11px] transition focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300 ${filter === value ? "bg-cyan-300/12 text-cyan-100" : "text-slate-300 hover:bg-white/5 hover:text-white"}`}
                >
                  {value === "all" ? "全部" : value === "issues" ? "故障" : value === "system" ? "系统" : "更新"}
                </button>
              ))}
            </div>
            <div className="ml-auto flex flex-wrap items-center gap-2">
              <span className="text-[11px] text-slate-400">导出包含事件标题与摘要</span>
              <Btn variant="ghost" onClick={exportEvents} disabled={!snapshot || exporting || loading}>
                <Download size={12} className={exporting ? "animate-pulse" : ""} />{exporting ? "导出中…" : "导出当前快照"}
              </Btn>
            </div>
          </div>
        )}

        {snapshot && (recurringApps.length > 0 || appKey) && (
          <div className="mb-3 flex flex-wrap items-center gap-2 rounded-md border border-amber-300/10 bg-amber-300/4 px-3 py-2 text-[11px]">
            <span className="text-amber-100">重复应用（返回样本）</span>
            {recurringApps.map((app) => <button key={app.key} type="button" aria-pressed={appKey === app.key} onClick={() => toggleAppFilter(app.key)} className={`rounded-md border px-2 py-1 transition-colors ${appKey === app.key ? "border-amber-200/30 bg-amber-200/10 text-amber-100" : "border-white/8 text-slate-300 hover:bg-white/5"}`} title={`崩溃 ${app.crashes} 次，挂起 ${app.hangs} 次；点击筛选`}>
              {app.label} · {app.count} 次（崩溃 {app.crashes} / 挂起 {app.hangs}）
            </button>)}
            {appKey && <button type="button" onClick={() => toggleAppFilter(appKey)} className="ml-auto text-cyan-200 hover:text-white">清除应用筛选：{selectedAppName} ×</button>}
          </div>
        )}

        {appKey && snapshot && (
          <section className="mb-3 rounded-lg border border-white/8 bg-black/10 p-3" aria-label={`${selectedAppName} 崩溃与挂起趋势`}>
            <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
              <div className="text-xs font-medium text-slate-200">{selectedAppName} · 每日故障样本</div>
              <div className="text-[10px] text-slate-500">{days} 天 · 本地时区 {localTimeZone} · 共 {selectedAppIssueCount} 条</div>
            </div>
            <div role="img" aria-label={`按本地日历统计 ${selectedAppName} 每日崩溃与挂起次数`} className="flex h-20 items-end gap-0.5 overflow-x-auto rounded-md border border-white/5 bg-white/2 px-1 py-1">
              {appDayBuckets.map((bucket) => {
                const total = bucket.crashes + bucket.hangs;
                const crashHeight = bucket.crashes === 0 ? 0 : Math.max(3, (bucket.crashes / maxAppDailyCount) * 100);
                const hangHeight = bucket.hangs === 0 ? 0 : Math.max(3, (bucket.hangs / maxAppDailyCount) * 100);
                return <div key={bucket.dayKey} title={`${bucket.dayKey} · 崩溃 ${bucket.crashes} · 挂起 ${bucket.hangs}`} className="flex h-full min-w-[5px] flex-1 flex-col justify-end">
                  <div className="w-full rounded-t bg-rose-400/80" style={{ height: `${crashHeight}%` }} />
                  <div className="w-full bg-amber-300/80" style={{ height: `${hangHeight}%` }} />
                  {total === 0 && <div className="h-px w-full bg-white/10" />}
                </div>;
              })}
            </div>
            <div className="mt-2 flex flex-wrap items-center gap-3 text-[10px] text-slate-500">
              <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-sm bg-rose-400" />崩溃</span>
              <span className="flex items-center gap-1"><i className="h-2 w-2 rounded-sm bg-amber-300" />挂起</span>
              <span>每个柱代表一个本地日历日；仅汇总当前返回的事件样本。</span>
            </div>
            {snapshotQuality?.incomplete && <div role="status" className="mt-2 text-[10px] text-amber-200">日志来源不完整或结果达到读取上限，趋势可能低于实际记录数。</div>}
            <div className="mt-1 text-[10px] text-slate-600">事件时间相邻不表示存在因果关系。</div>
          </section>
        )}

        {loading && !snapshot ? (
          <div className="rounded-lg border border-white/6 bg-black/10 px-3 py-4 text-center text-xs text-slate-300" role="status">
            正在读取近 {days} 天的 Windows 可靠性日志…
          </div>
        ) : snapshot == null ? (
          <div className="rounded-lg border border-white/6 bg-black/10 px-3 py-4 text-center text-xs text-slate-300">
            <CalendarClock size={16} className="mx-auto mb-2 text-slate-400" />
            按需读取 Windows 应用、系统与更新日志；只展示限长摘要。导出文件会包含事件标题和摘要。
          </div>
        ) : snapshot.events.length === 0 ? (
          <div className="rounded-lg border border-white/6 bg-black/10 px-3 py-4 text-center text-xs text-slate-300">
            <ShieldCheck size={16} className="mx-auto mb-2 text-emerald-300/80" />
            近 {days} 天在可读取的来源中未找到所列事件；日志可能被清理、禁用或不可访问，空结果不证明没有发生故障。
          </div>
        ) : filteredEvents.length === 0 ? (
          <div className="py-4 text-center text-xs text-slate-300">当前筛选下没有匹配事件。</div>
        ) : (
          <>
            <div className="max-h-96 space-y-1.5 overflow-y-auto pr-1">
              {filteredEvents.slice(0, visibleCount).map((event, index) => {
                const meta = EVENT_META[event.kind];
                const success = event.kind === "update_installed";
                return (
                  <article key={`${event.ts}-${event.kind}-${index}`} className="flex items-start gap-2.5 rounded-lg border border-white/6 bg-white/2 px-3 py-2.5">
                    {success
                      ? <CheckCircle2 size={14} className="mt-0.5 shrink-0 text-emerald-300" />
                      : <AlertTriangle size={14} className={`mt-0.5 shrink-0 ${meta.tone === "rose" ? "text-rose-300" : meta.tone === "amber" ? "text-amber-300" : "text-slate-400"}`} />}
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="truncate text-xs font-medium text-slate-100">{event.title}</span>
                        <Chip tone={meta.tone}>{meta.label}</Chip>
                      </div>
                      {event.detail && <p className="mt-1 break-words text-[11px] leading-4 text-slate-300">{event.detail}</p>}
                      <div className="mt-1 text-[11px] text-slate-400">{formatTime(event.ts)} · {event.source}</div>
                    </div>
                  </article>
                );
              })}
            </div>
            {filteredEvents.length > visibleCount && (
              <div className="mt-2 text-center">
                <Btn variant="ghost" onClick={showMore}>
                  显示更多（剩余 {filteredEvents.length - visibleCount} 条）
                </Btn>
              </div>
            )}
          </>
        )}
        <div className="mt-3 flex items-start gap-1.5 border-t border-white/6 pt-2.5 text-[11px] leading-5 text-slate-400">
          <AlertTriangle size={12} className="mt-1 shrink-0" />
          查询最多 90 天；最多返回 200 条。单个日志源触及读取上限时，较早记录可能被省略。无事件和来源不可读会分别标示。
        </div>
      </div>
    </Card>
  );
});

export default ReliabilityTimeline;
