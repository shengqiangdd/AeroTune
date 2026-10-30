import fs from "node:fs";

const app = fs.readFileSync("src/App.tsx", "utf8");
const dashboard = fs.readFileSync("src/pages/Dashboard.tsx", "utf8");
const processes = fs.readFileSync("src/pages/Processes.tsx", "utf8");
const processPolicy = fs.readFileSync("src/lib/processRefreshPolicy.js", "utf8");
const reliability = fs.readFileSync("src/components/ReliabilityTimeline.tsx", "utf8");
const powerInsights = fs.readFileSync("src/components/PowerInsights.tsx", "utf8");
const powerEstimate = fs.readFileSync("src/lib/powerEstimate.js", "utf8");
const processHistory = fs.readFileSync("src/components/ProcessHistoryCard.tsx", "utf8");
const duplicateFinder = fs.readFileSync("src/components/DuplicateFinderTab.tsx", "utf8");
const settings = fs.readFileSync("src/pages/Settings.tsx", "utf8");
const diskHistoryCard = fs.readFileSync("src/components/DiskSpaceHistoryCard.tsx", "utf8");
const diskHistoryChart = diskHistoryCard.match(/const DiskHistoryChart = memo\(function DiskHistoryChart[\s\S]*?\n\}\);/)?.[0] ?? "";
const optimizationLab = fs.readFileSync("src/components/OptimizationLab.tsx", "utf8");
const experimentHistoryView = optimizationLab.match(/const ExperimentHistoryView = memo\(function ExperimentHistoryView[\s\S]*?\n\}\);/)?.[0] ?? "";
const anomalyTimeline = dashboard.match(/const AnomalyTimeline = memo\(function AnomalyTimeline[\s\S]*?\n\}\);/)?.[0] ?? "";
const symptomGuide = fs.readFileSync("src/components/SymptomGuide.tsx", "utf8");
const symptomSelector = symptomGuide.match(/const SymptomSelector = memo\(function SymptomSelector[\s\S]*?\n\}\);/)?.[0] ?? "";
const startup = fs.readFileSync("src/pages/Startup.tsx", "utf8");
const startupTaskPagination = fs.readFileSync("src/lib/startupTaskPagination.js", "utf8");
const checks = [
  {
    ok: /const MemoProcesses\s*=\s*lazy\(/.test(app)
      && /import\("\.\/pages\/Processes"\)[\s\S]*?default: memo\(Processes\)/.test(app)
      && /page === "processes" && <MemoProcesses focusPid=\{focusPid\}/.test(app),
    description: "process table is memoized across parent telemetry updates",
  },
  {
    ok: /const reportPreviewJson = useMemo\([\s\S]*?JSON\.stringify\(redactDiagnosticReport\(reportPreview[\s\S]*?\[reportPreview, includeDeviceIdentifiers, includeDetailedTelemetry\]/.test(dashboard),
    description: "diagnostic preview serialization skips unrelated live-telemetry rerenders",
  },
  {
    ok: Boolean(diskHistoryChart)
      && /const samples = useMemo\([\s\S]*?\[activeMount, history\]/.test(diskHistoryCard)
      && /<DiskHistoryChart mount=\{activeMount\} samples=\{samples\} \/>/.test(diskHistoryCard)
      && /<Spark data=\{samples\.map\(\(sample\) => sample\.freeGb\)\}/.test(diskHistoryChart)
      && /Math\.max\(\.\.\.samples\.map\(\(sample\) => sample\.totalGb\)\)/.test(diskHistoryChart)
      && /formatSampleTime\(first\.ts\)/.test(diskHistoryChart)
      && !/\bstats\b/.test(diskHistoryChart)
      && /validatedDiskFreeGb\(currentDisk\.total_gb, currentDisk\.used_gb\)/.test(diskHistoryCard),
    description: "disk trend SVG work skips live telemetry rerenders while current free space stays live",
  },
  {
    ok: Boolean(experimentHistoryView)
      && /<ExperimentHistoryView[\s\S]*history=\{history\}[\s\S]*consistency=\{consistency\}[\s\S]*onDeleteRecord=\{deleteRecord\}[\s\S]*onResetHistory=\{resetHistory\}/.test(optimizationLab)
      && /const deleteRecord = useCallback\([\s\S]*?\}, \[\]\);/.test(optimizationLab)
      && /const resetHistory = useCallback\([\s\S]*?\}, \[\]\);/.test(optimizationLab)
      && !/\bstats\b|\bstatsStatus\b/.test(experimentHistoryView)
      && /if \(current\.lastSampleId === stats\.sample_id\) return/.test(optimizationLab)
      && /setActiveSession\(next\)/.test(optimizationLab),
    description: "experiment history skips live-sample rerenders without pausing active capture",
  },
  {
    ok: Boolean(anomalyTimeline)
      && /<AnomalyTimeline context=\{context\} \/>/.test(dashboard)
      && /const context = anomalyContext\?\.find\(\(entry\) => entry\.pid === a\.pid && entry\.kind === a\.kind\)/.test(dashboard)
      && /api\.getAnomalyContext\(\)\s*\.then\(\(contexts\) => \{ if \(!disposed\) setAnomalyContext\(contexts\); \}\)/.test(dashboard)
      && /setInterval\(\(\) => \{ if \(!document\.hidden\) refreshDiag\(\); \}, 10000\)/.test(dashboard)
      && !/\bstats\b|\bhist\b/.test(anomalyTimeline),
    description: "event correlation timeline skips unrelated dashboard telemetry renders",
  },
  {
    ok: Boolean(symptomSelector)
      && /<SymptomSelector selectedId=\{selected\.id\} onSelect=\{selectSymptom\} \/>/.test(symptomGuide)
      && /const selectSymptom = useCallback\([\s\S]*?\}, \[\]\);/.test(symptomGuide)
      && /role="group" aria-label="选择要排查的症状"[\s\S]*?SYMPTOMS\.map/.test(symptomSelector)
      && /const matchedRecommendations = useMemo\(\s*\(\) => recommendations\.filter\(\(item\) => symptomForRecommendation\(item\.id\) === selected\.id\),\s*\[recommendations, selected\.id\],\s*\);/.test(symptomGuide)
      && !/\bstats\b|\bdiagnostics\b/.test(symptomSelector),
    description: "symptom selector buttons skip live-evidence rerenders while selection semantics stay intact",
  },
  {
    ok: /const estimate = useMemo\(/.test(powerInsights)
      && /estimateDischarge\(samples, currentPercent\)/.test(powerInsights)
      && /\[telemetryLive, enabled, storageError, currentAcState, currentPercent, samples\]/.test(powerInsights),
    description: "battery discharge estimates are memoized across unrelated telemetry updates",
  },
  {
    ok: /export const BATTERY_MAX_SAMPLE_GAP_MS = BATTERY_SAMPLE_INTERVAL_MS \* 2\.5/.test(powerEstimate)
      && /newer && newer\.timestamp - sample\.timestamp > BATTERY_MAX_SAMPLE_GAP_MS/.test(powerEstimate)
      && /电池估算间隔超\{BATTERY_MAX_SAMPLE_GAP_MS \/ 60_000\}分钟、接电趋势超\{AC_DRAIN_MAX_SAMPLE_GAP_MS \/ 60_000\}分钟时重新累计/.test(powerInsights),
    description: "battery estimates restart after long gaps and explain the threshold",
  },
  {
    ok: /export const AC_DRAIN_MAX_SAMPLE_GAP_MS = BATTERY_SAMPLE_INTERVAL_MS \* 1\.5/.test(powerEstimate)
      && /AC_DRAIN_MIN_COVERAGE_MS = 15 \* 60_000/.test(powerEstimate)
      && /const pluggedDrain = useMemo\([\s\S]*?estimateAcDrain\(runSamples\)/.test(powerInsights)
      && /\[telemetryLive, stats\?\.battery_status_ok, enabled, storageError, currentAcState, currentPercent, latestSample, samples, clockNow\]/.test(powerInsights)
      && /AC_DRAIN_TREND_WINDOW_MS/.test(powerEstimate),
    description: "plugged-in drain observations stay bounded, memoized, and opt-in",
  },
  {
    ok: /const TrendList = memo\(function TrendList/.test(processHistory)
      && /const ranked = useMemo\([\s\S]*?\[trends, metric\]/.test(processHistory)
      && /const trends = useMemo\(\(\) => buildTrends\(history\.samples\), \[history\.samples\]\)/.test(processHistory),
    description: "process-history rankings and child rows are skipped across process refreshes",
  },
  {
    ok: /const selectedPaths = useMemo\(\(\) => \[\.\.\.selected\], \[selected\]\)/.test(duplicateFinder)
      && /const estimatedSelectedBytes = useMemo\([\s\S]*?result\?\.groups\.reduce[\s\S]*?\[result, selected\]/.test(duplicateFinder)
      && /const filteredGroups = useMemo\(/.test(duplicateFinder),
    description: "duplicate-scan selection summaries avoid rescanning results during paging and expansion",
  },
  {
    ok: /const recoveryGroups = useMemo\(\(\) => \{[\s\S]*?for \(const change of changes\)[\s\S]*?return \[\.\.\.groupMap\.values\(\)\]\.sort\([\s\S]*?\}, \[changes\]\);/.test(settings)
      && /recoveryGroups\.slice\(0, 8\)/.test(settings),
    description: "settings recovery groups are recomputed only when change-journal entries change",
  },
  {
    ok: /const enabledItems = useMemo\([\s\S]*?\[items\]/.test(startup)
      && /const logonTasks = useMemo\([\s\S]*?\[tasks\]/.test(startup)
      && /const sortedTasks = useMemo\([\s\S]*?\[tasks\]/.test(startup)
      && /const deferredTaskQuery = useDeferredValue\(taskQuery\)/.test(startup)
      && /filterStartupTasks\(sortedTasks \?\? \[\], deferredTaskQuery\)/.test(startup)
      && /paginateStartupTasks\(matchingTasks, taskPage, STARTUP_TASK_PAGE_SIZE\)/.test(startup)
      && /taskPageResult\.items\.map\(\(t\) =>/.test(startup)
      && /aria-label="搜索计划任务名称、路径、执行命令或发布者"/.test(startup)
      && /<nav aria-label="计划任务分页"/.test(startup)
      && /STARTUP_TASK_PAGE_SIZE = 50/.test(startupTaskPagination)
      && /export function filterStartupTasks\(/.test(startupTaskPagination)
      && !/tasks\s*\.slice\(\)\s*\.sort\(/.test(startup),
    description: "startup task search is deferred, ordered, and rendered in bounded accessible pages",
  },
  {
    ok: [["MemoAutoEngine", "AutoEngine"], ["MemoStartup", "Startup"], ["MemoServices", "Services"], ["MemoCleaner", "Cleaner"], ["MemoNetwork", "Network"], ["MemoSettings", "Settings"]]
      .every(([name, component]) => new RegExp(`const ${name}\\s*=\\s*lazy\\(`).test(app) && app.includes(`default: memo(${component})`) && app.includes(`<${name}`)),
    description: "non-telemetry routes have stable memo boundaries",
  },
  {
    ok: /page === "startup" && <MemoStartup onNav=\{navigate\}/.test(app),
    description: "memoized startup route receives a stable navigation callback",
  },
  {
    ok: /page === "dashboard" && <Dashboard stats=\{stats\} statsStatus=\{status\} hist=\{hist\}/.test(app)
      && /<Sidebar page=\{page\} onNav=\{navigate\} stats=\{stats\}/.test(app),
    description: "telemetry-consuming dashboard and sidebar retain live samples",
  },
  {
    ok: /const MemoTitleBar\s*=\s*memo\(TitleBar\)/.test(app)
      && /<MemoTitleBar admin=\{admin\}/.test(app),
    description: "stable title bar avoids unrelated sensor-driven renders",
  },
  {
    ok: /const pollIfVisible[\s\S]*?document\.hidden[\s\S]*?getCurrentWindow\(\)\.isVisible\(\)[\s\S]*?load\(\)/.test(processes)
      && /processRefreshIntervalMs\(cpuLoadBand, memoryLoadBand, interactionLatencyBand\)/.test(processes)
      && /nearestRankPercentile\(samples, 0\.95\)/.test(processes)
      && /setSearchLatencySampleCount\(sampleUpdate\.rollingSampleCount\)/.test(processes)
      && /setSearchLatencySampleCount\(0\)/.test(processes)
      && /setInterval\(\(\) => \{ void pollIfVisible\(\); \}, autoRefreshMs\)/.test(processes)
      && /cpuLoadBand=\{processLoadBand\}/.test(app)
      && /memoryLoadBand=\{processMemoryLoadBand\}/.test(app)
      && /measureSearchLatency\s*\?\s*interactionLatencyBandFromP95\(searchLatencyP95Ms, searchLatencySampleCount, current\)\s*:\s*"unknown"/.test(processes)
      && /LATENCY_MIN_SAMPLES = 20/.test(processPolicy)
      && /P95 偏高时自动延长进程列表刷新间隔/.test(processes)
      && /cpuLoadBandFromUsage\(stats\?\.cpu, current\.cpu\)/.test(app)
      && /memoryLoadBandFromUsage\(stats\?\.mem_used_gb, stats\?\.mem_total_gb, current\.memory\)/.test(app)
      && /return cpu === current\.cpu && memory === current\.memory \? current : \{ cpu, memory \}/.test(app)
      && /CPU_ELEVATED_ENTER_PCT = 60/.test(processPolicy)
      && /CPU_ELEVATED_EXIT_PCT = 56/.test(processPolicy)
      && /CPU_HIGH_ENTER_PCT = 80/.test(processPolicy)
      && /CPU_HIGH_EXIT_PCT = 76/.test(processPolicy)
      && /MEMORY_ELEVATED_ENTER_PCT = 85/.test(processPolicy)
      && /MEMORY_ELEVATED_EXIT_PCT = 83/.test(processPolicy)
      && /MEMORY_HIGH_ENTER_PCT = 90/.test(processPolicy)
      && /MEMORY_HIGH_EXIT_PCT = 88/.test(processPolicy)
      && /LATENCY_ELEVATED_ENTER_MS = 50/.test(processPolicy)
      && /LATENCY_ELEVATED_EXIT_MS = 40/.test(processPolicy)
      && /LATENCY_HIGH_ENTER_MS = 100/.test(processPolicy)
      && /LATENCY_HIGH_EXIT_MS = 80/.test(processPolicy)
      && /交互 P95 偏高时自动放缓/.test(processes)
      && /Math\.max\(cpuIntervalMs, memoryIntervalMs, interactionLatencyIntervalMs\)/.test(processPolicy),
    description: "process scans pause when hidden and use CPU/RAM/UI-latency hysteresis without telemetry rerenders",
  },
  {
    ok: /const filtered\s*=\s*useMemo\(/.test(processes)
      && /const displayRows\s*=\s*useMemo\(/.test(processes)
      && /requestAnimationFrame\(\(\) => \{[\s\S]*?performance\.now\(\) - startedAt/.test(processes)
      && /recordInteractionLatencySample\(/.test(processes)
      && /sampleUpdate\.shouldUpdateP95/.test(processes)
      && /sampleUpdate\.rollingSampleCount/.test(processes)
      && /const virtualRows\s*=\s*useMemo\(/.test(processes)
      && /virtualRows\.map/.test(processes)
      && /ResizeObserver/.test(processes)
      && /topSpacerHeight/.test(processes)
      && /\[\s*displayRows,\s*measureSearchLatency,\s*query\s*\]/.test(processes)
      && /firstVirtualRowPid/.test(processes)
      && /whitespace-nowrap/.test(processes),
    description: "process rows are virtualized and fixed-height with behavior-tested rolling P95 sampling",
  },
  {
    ok: /const \{ issueCount, updateCount \} = useMemo\(\(\) => \{[\s\S]*?for \(const event of events \?\? \[\]\)[\s\S]*?\}, \[events\]\);/.test(reliability)
      && /const selectedAppName = useMemo\([\s\S]*?\[appKey, events\]/.test(reliability),
    description: "reliability event summaries are cached by snapshot and selected application",
  },
  {
    ok: /export const ReliabilityTimeline = memo\(function ReliabilityTimeline/.test(reliability),
    description: "on-demand reliability history avoids telemetry-driven rerenders",
  },
];

let failed = 0;
for (const check of checks) {
  console.log(`${check.ok ? "PASS" : "FAIL"} ${check.description}`);
  if (!check.ok) failed++;
}
if (failed) {
  console.error(`Render locality contract failed: ${failed}/${checks.length}`);
  process.exitCode = 1;
} else {
  console.log(`Render locality contract passed: ${checks.length} checks`);
}
