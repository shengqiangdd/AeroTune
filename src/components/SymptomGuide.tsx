import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Activity,
  ArrowRight,
  BatteryCharging,
  Cpu,
  HardDrive,
  Lightbulb,
  Rocket,
  Thermometer,
  Wifi,
} from "lucide-react";
import { Diagnostics, SmartRecommendation, Stats } from "../lib/api";
import { rankSymptomFocus, symptomForRecommendation } from "../lib/symptomFocus";
import { cpuEvidence, diskEvidence, gpuEvidence, ioEvidence, validCapacityPair } from "../lib/resourceEvidence";
import { baselineTransition } from "../lib/baselineAnnouncement";
import { MEMORY_SLOPE_MIN_SAMPLES, MEMORY_SLOPE_WINDOW, memoryPressureStatus } from "../lib/memoryPressure";
import { PageId } from "./Sidebar";
import { Btn, Card, Chip, Toggle } from "./ui";

type SymptomId = "lag" | "gpu" | "memory" | "io" | "startup" | "battery" | "network" | "disk";
type Evidence = { label: string; value: string; measured: boolean; pending?: boolean };

const SYMPTOMS: {
  id: SymptomId;
  title: string;
  icon: JSX.Element;
  page: PageId;
  pageLabel: string;
  checklist: string[];
}[] = [
  {
    id: "lag",
    title: "卡顿 / 高 CPU",
    icon: <Cpu size={15} />,
    page: "processes",
    pageLabel: "进程管理",
    checklist: [
      "在进程管理中按 CPU 排序，观察占用较高的进程是否持续；单次尖峰不一定代表异常。",
      "对照下方 CPU 异常记录，确认进程名与 PID；关闭或暂停任务由你自行决定。",
    ],
  },
  {
    id: "gpu",
    title: "显卡温度 / 显存",
    icon: <Thermometer size={15} />,
    page: "processes",
    pageLabel: "进程管理",
    checklist: [
      "对照显卡温度、全局利用率与显存用量的持续趋势；单次峰值不等于瓶颈。",
      "查看正在运行的图形应用，并结合散热通风判断；进程页不提供单进程 GPU 指标。",
    ],
  },
  {
    id: "memory",
    title: "内存压力",
    icon: <Activity size={15} />,
    page: "processes",
    pageLabel: "进程管理",
    checklist: [
      "查看内存占用与页面文件使用量；若持续走高，可在进程管理中找出主要占用者。",
      "比较一段时间内的内存趋势，并留意是否有应用持续增长。",
    ],
  },
  {
    id: "io",
    title: "磁盘 I/O",
    icon: <HardDrive size={15} />,
    page: "processes",
    pageLabel: "进程管理",
    checklist: [
      "核对异常 I/O 进程、PID 及滚动基线；持续偏离比单次峰值更值得检查。",
      "在进程管理中按磁盘吞吐排序，确认高 I/O 是否与当前任务相符。",
    ],
  },
  {
    id: "startup",
    title: "启动项检查",
    icon: <Rocket size={15} />,
    page: "startup",
    pageLabel: "开机全景",
    checklist: [
      "核对开机全景中的已启用启动项与计划任务；统计可能包含已禁用项，调整前先确认实际状态。",
      "尚无启动耗时数据，因此此建议仅提示检查项目数量，不代表启动变慢。",
    ],
  },
  {
    id: "battery",
    title: "电池续航",
    icon: <BatteryCharging size={15} />,
    page: "power",
    pageLabel: "省电续航",
    checklist: [
      "先核对当前电量、供电状态和系统估算剩余时间；估算值会随负载变化。",
      "在省电续航页查看电池报告与当前电源计划，再决定是否调整设置。",
    ],
  },
  {
    id: "network",
    title: "网络不稳",
    icon: <Wifi size={15} />,
    page: "network",
    pageLabel: "网络优化",
    checklist: [
      "对照当前收发吞吐；吞吐量只能说明此刻的数据流量，不能单独判断线路质量。",
      "在网络优化页运行线路质量探测，查看延迟、抖动与丢包，再区分本机、网关或 DNS 问题。",
    ],
  },
  {
    id: "disk",
    title: "磁盘空间",
    icon: <HardDrive size={15} />,
    page: "cleaner",
    pageLabel: "空间清理",
    checklist: [
      "先查看各卷已用与剩余空间，留意系统盘是否接近满载。",
      "空间清理页可扫描并预览候选项；本指南不会删除文件或自动清理。",
    ],
  },
];

const SymptomSelector = memo(function SymptomSelector({
  selectedId,
  onSelect,
}: {
  selectedId: SymptomId;
  onSelect: (id: SymptomId) => void;
}) {
  return (
    <div role="group" aria-label="选择要排查的症状" className="mb-4 grid grid-cols-2 gap-2 sm:grid-cols-3 2xl:grid-cols-4">
      {SYMPTOMS.map((symptom) => {
        const active = symptom.id === selectedId;
        return (
          <button
            key={symptom.id}
            type="button"
            aria-pressed={active}
            onClick={() => onSelect(symptom.id)}
            className={`flex min-h-10 items-center gap-2 rounded-lg border px-2.5 py-2 text-left text-xs transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70 ${
              active
                ? "border-cyan-400/35 bg-cyan-400/10 text-cyan-100"
                : "border-white/8 bg-white/3 text-slate-400 hover:bg-white/7 hover:text-slate-200"
            }`}
          >
            <span aria-hidden="true" className="shrink-0">{symptom.icon}</span>
            <span>{symptom.title}</span>
          </button>
        );
      })}
    </div>
  );
});

function finite(value: number | null | undefined): value is number {
  return value != null && Number.isFinite(value);
}

function percent(value: number): string {
  return `${value.toFixed(0)}%`;
}

function formatDuration(seconds: number | null | undefined): string {
  if (!finite(seconds) || seconds < 0) return "系统未提供剩余时间估算";
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return hours > 0 ? `约 ${hours} 小时 ${minutes} 分` : `约 ${minutes} 分钟`;
}

function evidenceFor(id: SymptomId, stats: Stats | null, diagnostics: Diagnostics | null): Evidence[] {
  const rows: Evidence[] = [];
  const add = (label: string, value: string, measured: boolean) => {
    const row: Evidence = { label, value, measured };
    if (!measured && value.startsWith("等待")) row.pending = true;
    rows.push(row);
  };

  if (id === "lag") {
    add("当前 CPU", stats && finite(stats.cpu) ? percent(stats.cpu) : "等待 CPU 遥测", !!stats && finite(stats.cpu));
    rows.push(...cpuEvidence(diagnostics));
  }

  if (id === "gpu") rows.push(...gpuEvidence(stats));

  if (id === "memory") {
    const memoryStats = stats && validCapacityPair(stats.mem_used_gb, stats.mem_total_gb) ? stats : null;
    add(
      "系统报告内存已用",
      memoryStats
        ? `${percent((memoryStats.mem_used_gb / memoryStats.mem_total_gb) * 100)}（已用 ${memoryStats.mem_used_gb.toFixed(1)} / 总量 ${memoryStats.mem_total_gb.toFixed(1)} GB）`
        : "内存容量数据不可用",
      memoryStats != null,
    );
    const diagnosticMemoryPercent = diagnostics?.mem_pct;
    const diagnosticMemoryValid = finite(diagnosticMemoryPercent)
      && diagnosticMemoryPercent >= 0
      && diagnosticMemoryPercent <= 100;
    add(
      "诊断内存水位",
      diagnosticMemoryValid
        ? percent(diagnosticMemoryPercent)
        : diagnostics ? "诊断水位数据无效" : "诊断数据不可用",
      diagnosticMemoryValid,
    );
    const swapValid = stats && validCapacityPair(stats.swap_used_gb, stats.swap_total_gb);
    add(
      "页面文件",
      swapValid ? `${stats.swap_used_gb.toFixed(1)} / ${stats.swap_total_gb.toFixed(1)} GB` : stats ? "容量或使用量未提供" : "等待遥测",
      !!swapValid,
    );
    const memoryTrendSampleCount = diagnostics && finite(diagnostics.mem_slope_sample_count)
      ? Math.max(0, Math.min(MEMORY_SLOPE_WINDOW, Math.trunc(diagnostics.mem_slope_sample_count)))
      : 0;
    const memoryTrendReady = !!diagnostics
      && diagnostics.mem_slope_ready
      && finite(diagnostics.mem_slope);
    const memoryEtaSeconds = memoryPressureStatus(diagnostics).etaSeconds;
    add(
      "内存趋势",
      memoryTrendReady
        ? `${diagnostics!.mem_slope.toFixed(2)} 个百分点 / 秒${memoryEtaSeconds !== null ? `；若趋势延续，线性估算约 ${memoryEtaSeconds} 秒接近 90%` : ""}`
        : diagnostics && !diagnostics.mem_slope_ready
          ? memoryTrendSampleCount < MEMORY_SLOPE_MIN_SAMPLES
            ? `有效实测读数 ${memoryTrendSampleCount}/${MEMORY_SLOPE_MIN_SAMPLES}，趋势暂不判断`
            : `已有 ${memoryTrendSampleCount} 个有效实测读数，趋势就绪状态待同步`
          : "诊断趋势数据不可用",
      memoryTrendReady,
    );
  }

  if (id === "io") rows.push(...ioEvidence(diagnostics));

  if (id === "startup") {
    add("启动耗时", "当前遥测未采集启动耗时", false);
    const startupRecommendation = diagnostics?.deductions.some((item) => item.item === "启动项过多") ?? false;
    add(
      "启动项提示",
      diagnostics ? (startupRecommendation ? "统计的启动项数量偏多，计数可能包含已禁用项" : "当前诊断没有启动项数量提示") : "诊断数据尚未提供",
      diagnostics != null,
    );
  }

  if (id === "battery") {
    const battery = stats?.battery;
    if (!stats) {
      add("电池状态", "等待电池遥测", false);
    } else if (!battery) {
      add("电池状态", stats.battery_status_ok === false ? "电池接口查询失败" : "未检测到电池数据", false);
    } else {
      add("当前电量", finite(battery.percent) ? percent(battery.percent) : "电量百分比未提供", finite(battery.percent));
      add(
        "供电状态",
        battery.ac_connected === true ? "已接通电源" : battery.ac_connected === false ? "使用电池" : "电源连接状态未提供",
        battery.ac_connected != null,
      );
      add("系统估算剩余", formatDuration(battery.secs_left), finite(battery.secs_left));
    }
    add("电池健康 / 长期趋势", "当前仪表盘遥测未提供；省电续航页可生成电池报告", false);
  }

  if (id === "network") {
    add(
      "当前接收 / 发送",
      stats && finite(stats.net_rx_kbps) && finite(stats.net_tx_kbps)
        ? `${stats.net_rx_kbps.toFixed(1)} / ${stats.net_tx_kbps.toFixed(1)} KB/s`
        : "网络吞吐数据不可用",
      !!stats && finite(stats.net_rx_kbps) && finite(stats.net_tx_kbps),
    );
    add("延迟 / 抖动 / 丢包", "尚未采集；可在网络优化页主动运行线路质量探测", false);
    add("Wi-Fi 信号强度", "当前遥测未提供", false);
  }

  if (id === "disk") rows.push(...diskEvidence(stats));

  return rows;
}

function recommendationPid(id: string): number | undefined {
  const match = /^process-(\d+)-(cpu|mem|io)$/.exec(id);
  const pid = match ? Number(match[1]) : NaN;
  return Number.isSafeInteger(pid) && pid > 0 ? pid : undefined;
}

export interface SymptomGuideProps {
  stats: Stats | null;
  diagnostics: Diagnostics | null;
  recommendations: SmartRecommendation[];
  recommendationsLoading: boolean;
  recommendationsError: boolean;
  onNav: (page: PageId, pid?: number) => void;
}

export function SymptomGuide({ stats, diagnostics, recommendations, recommendationsLoading, recommendationsError, onNav }: SymptomGuideProps) {
  const [selectedId, setSelectedId] = useState<SymptomId>(() => rankSymptomFocus(recommendations)?.symptom ?? "lag");
  const [followFocus, setFollowFocus] = useState(true);
  const [focusLiveMessage, setFocusLiveMessage] = useState("");
  const [baselineLiveMessage, setBaselineLiveMessage] = useState("");
  const baselineSnapshot = useRef<ReturnType<typeof baselineTransition>["snapshot"]>(null);
  const announcedBaselineDimensions = useRef(new Set<"cpu" | "mem" | "io">());
  const lastAutoFocus = useRef<SymptomId | null>(null);
  const focus = useMemo(() => rankSymptomFocus(recommendations), [recommendations]);
  const focusedTitle = focus ? SYMPTOMS.find((item) => item.id === focus.symptom)?.title : null;
  useEffect(() => {
    if (!followFocus || !focus) {
      lastAutoFocus.current = null;
      setFocusLiveMessage("");
      return;
    }
    setSelectedId(focus.symptom);
    if (lastAutoFocus.current !== focus.symptom) {
      setFocusLiveMessage(`智能聚焦已切换到${focusedTitle ?? "相关症状"}，依据：${focus.recommendation.title}。`);
      lastAutoFocus.current = focus.symptom;
    }
  }, [followFocus, focus, focusedTitle]);
  useEffect(() => {
    const transition = baselineTransition(baselineSnapshot.current, diagnostics, announcedBaselineDimensions.current);
    baselineSnapshot.current = transition.snapshot;
    if (transition.message) setBaselineLiveMessage(transition.message);
  }, [
    diagnostics?.process_sample_ready,
    diagnostics?.cpu_baseline_warming_processes,
    diagnostics?.mem_baseline_warming_processes,
    diagnostics?.io_baseline_warming_processes,
  ]);
  const selected = SYMPTOMS.find((item) => item.id === selectedId) ?? SYMPTOMS[0];
  const evidence = evidenceFor(selected.id, stats, diagnostics);
  const matchedRecommendations = useMemo(
    () => recommendations.filter((item) => symptomForRecommendation(item.id) === selected.id),
    [recommendations, selected.id],
  );
  const selectSymptom = useCallback((id: SymptomId) => {
    setFollowFocus(false);
    setSelectedId(id);
  }, []);
  const setFocusMode = (enabled: boolean) => {
    setFollowFocus(enabled);
    if (enabled && focus) setSelectedId(focus.symptom);
  };

  return (
    <Card
      title="症状排查与智能聚焦"
      icon={<Lightbulb size={15} className="text-cyan-300" />}
      className="min-w-0"
      right={(
        <div className="flex flex-wrap items-center justify-end gap-2">
          {followFocus && <Chip tone={recommendationsError ? "amber" : focus ? "cyan" : "slate"}>{recommendationsError ? focus ? `建议更新失败 · 保留${focusedTitle}重点` : "建议更新失败" : focus ? `智能聚焦：${focusedTitle}${recommendationsLoading ? " · 更新中" : ""}` : recommendationsLoading ? "正在评估…" : "暂无高优先建议"}</Chip>}
          <div className="flex items-center gap-2">
            <span className="text-[11px] text-slate-400">{followFocus ? "自动跟随" : "手动锁定"}</span>
            <Toggle on={followFocus} onChange={setFocusMode} label="自动跟随最高优先级建议" />
          </div>
        </div>
      )}
    >
      <p className="-mt-2 mb-3 text-xs leading-5 text-slate-400">
        选择困扰查看已测证据与检查路径；智能聚焦跟随最高优先级建议，手动选症状后会锁定。建议只读，不会自动更改系统。
      </p>

      <SymptomSelector selectedId={selected.id} onSelect={selectSymptom} />

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1.1fr)_minmax(0,0.9fr)]">
        <section aria-labelledby="symptom-evidence-title" className="min-w-0">
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
            <div className="flex flex-wrap items-center gap-2">
              <h3 id="symptom-evidence-title" className="text-xs font-semibold text-slate-200">{selected.title} · 当前证据</h3>
              {followFocus && focus?.symptom === selected.id && <Chip tone="cyan">优先建议聚焦</Chip>}
            </div>
            <div className="flex gap-1.5" aria-label="证据图例">
              <Chip tone="cyan">已测</Chip>
              <Chip tone="slate">采样中</Chip>
              <Chip tone="amber">未提供</Chip>
            </div>
          </div>
          <ul className="space-y-1.5">
            {evidence.map((item) => (
              <li key={item.label} className="flex flex-wrap items-start gap-2 rounded-md border border-white/6 bg-white/3 px-2.5 py-2 text-xs">
                <Chip tone={item.measured ? "cyan" : item.pending ? "slate" : "amber"}>{item.measured ? "已测" : item.pending ? "采样中" : "未提供"}</Chip>
                <span className="shrink-0 text-slate-400">{item.label}</span>
                <span className="min-w-0 flex-1 text-slate-200">{item.value}</span>
              </li>
            ))}
          </ul>
        </section>

        <section aria-labelledby="symptom-checklist-title" className="min-w-0">
          <h3 id="symptom-checklist-title" className="mb-2 text-xs font-semibold text-slate-200">建议检查</h3>
          <ol className="space-y-2 text-xs leading-5 text-slate-400">
            {selected.checklist.map((item) => <li key={item} className="flex gap-2"><span className="text-cyan-300">•</span><span>{item}</span></li>)}
          </ol>
          {matchedRecommendations.length > 0 && (
            <div className="mt-3 space-y-1.5">
              <div className="text-[11px] font-medium text-slate-400">匹配的智能建议</div>
              {matchedRecommendations.slice(0, 2).map((item) => (
                <div key={item.id} className="flex items-start gap-2 rounded-md border border-white/8 bg-white/3 px-2.5 py-2">
                  <span className="min-w-0 flex-1">
                    <span className="block text-xs text-slate-200">{item.title}</span>
                    <span className="mt-0.5 block text-[11px] leading-4 text-slate-500">{item.reason}</span>
                  </span>
                  <button
                    type="button"
                    onClick={() => onNav(item.action as PageId, item.action === "processes" ? recommendationPid(item.id) : undefined)}
                    aria-label={`查看${item.title}`}
                    className="rounded-md p-1 text-cyan-300 transition-colors hover:bg-white/8 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-cyan-300/70"
                  >
                    <ArrowRight size={14} />
                  </button>
                </div>
              ))}
            </div>
          )}
          <div className="mt-3 flex flex-wrap items-center gap-2">
            <Btn onClick={() => onNav(selected.page)} className="px-3 py-1.5 text-xs">
              前往{selected.pageLabel}<ArrowRight size={13} />
            </Btn>
            {matchedRecommendations.length === 0 && <span className="text-[11px] text-slate-500">当前没有直接匹配此症状的智能建议。</span>}
          </div>
        </section>
      </div>
      <div className="sr-only" role="status" aria-live="polite" aria-atomic="true">{focusLiveMessage}</div>
      <div id="process-baseline-live-status" className="sr-only" role="status" aria-live="polite" aria-atomic="true">{baselineLiveMessage}</div>
    </Card>
  );
}

export default SymptomGuide;
