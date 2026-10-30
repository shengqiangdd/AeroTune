// 与 Rust 后端对应的类型与 invoke 封装
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type EffectivePowerMode =
  | "battery_saver"
  | "better_battery"
  | "balanced"
  | "high_performance"
  | "max_performance"
  | "game_mode"
  | "mixed_reality";

export interface Stats {
  sample_id: number;
  cpu: number;
  per_core: number[];
  cpu_freq_mhz: number;
  mem_used_gb: number;
  mem_total_gb: number;
  swap_used_gb: number;
  swap_total_gb: number;
  system_drive: string;
  disks: DiskInfo[];
  net_rx_kbps: number;
  net_tx_kbps: number;
  /** GB observed by AeroTune and accumulated across process restarts for the local day. */
  net_today_rx_gb: number;
  net_today_tx_gb: number;
  gpu: GpuInfo | null;
  battery: BatteryInfo | null;
  battery_status_ok?: boolean;
  /** Windows-reported effective power mode; null when unsupported or unrecognized. */
  effective_power_mode: EffectivePowerMode | null;
  uptime_secs: number;
  process_count: number;
}
export interface GpuInfo {
  name: string;
  utilization: number | null;
  mem_used_mb: number | null;
  mem_total_mb: number | null;
  temperature: number | null;
}
export interface BatteryInfo {
  ac_online: boolean;
  ac_connected?: boolean | null;
  percent: number | null;
  secs_left: number | null;
}
export interface PowerDiagnostics {
  requests: string[];
  last_wake: string[];
  wake_timers: string[];
  collected_at_unix: number;
}
export interface DiskInfo {
  mount: string;
  total_gb: number;
  used_gb: number;
  fs: string;
}
export interface ProcInfo {
  pid: number;
  /** Opaque FILETIME token for backend revalidation before PID-bound mutations; absent when row identity is ambiguous. */
  creation_token?: string;
  parent_pid?: number | null;
  name: string;
  cpu: number;
  mem_mb: number;
  io_mb_s: number;
  path: string;
  /** 本地静态证据画像的角色 ID（system-core/windows-background/autostart/updater-suspect/user-app/unknown/self）。 */
  role: string;
  role_label: string;
}
export interface MemoryCandidate {
  pid: number;
  name: string;
  path: string;
  working_set_mb: number;
  private_mb: number;
  cpu: number;
  eligible: boolean;
  reason: string;
  /** Lossless opaque FILETIME token; present only when the backend could verify identity. */
  creation_token?: string;
}
export interface MemoryCandidateIdentity {
  pid: number;
  creation_token: string;
}
export interface MemoryOptimizationReport {
  available_before_mb: number;
  available_after_mb: number | null;
  candidates: MemoryCandidate[];
  requested_pids?: number[];
  processed: number;
  touched_working_set_mb: number;
  estimated_reclaimed_mb: number;
  skipped: number;
}
export interface Plan {
  guid: string;
  name: string;
  active: boolean;
}
export interface TweakStates {
  gamedvr: boolean;
  hags: boolean | null;
  priority_boost: boolean;
}
export interface StartupItem {
  id: string;
  name: string;
  cmd: string;
  source: string;
  enabled: boolean;
  signature_status: "signed" | "unsigned" | "unknown";
  signer_subject: string | null;
}
export interface JunkItem {
  id: string;
  label: string;
  path: string;
  size_mb: number;
  files: number;
  admin_required: boolean;
  confidence: string;
  median_age_days: number;
}
export interface JunkPreviewCategory {
  id: string;
  label: string;
  candidate_files: number;
  candidate_bytes: number;
  sample_paths: string[];
  age_filtered_files: number;
  skipped_files: number;
  permanent_action: boolean;
  exact: boolean;
}
export interface JunkCleanupPreview {
  plan_id: string | null;
  older_than_days: number;
  categories: JunkPreviewCategory[];
  can_execute: boolean;
  over_limit: boolean;
}
export interface JunkCleanupResult {
  cancelled: boolean;
  deleted_files: number;
  deleted_bytes: number;
  skipped_changed: number;
  skipped_unsafe: number;
  failed: number;
  errors: string[];
}
export interface Adapter {
  name: string;
  desc: string;
  status: string;
  speed: string;
}
export interface DnsEntry {
  iface: string;
  servers: string[];
}
export interface ChangeRevert {
  t: string;
  key?: string;
  value?: string;
  iface?: string;
  guid?: string;
  level?: string;
  old?: unknown;
  applied?: unknown;
  nic_guid?: string;
  applied_guid?: string | null;
  applied_level?: string | null;
}
export interface Change {
  id: number;
  ts: number;
  desc: string;
  group_id?: number | null;
  group_label?: string | null;
  revert: ChangeRevert;
}
export interface Anomaly {
  pid: number;
  name: string;
  kind: string;
  value: number;
  baseline: number;
  robust_z: number;
  /** Wall-clock second when the confirming hot streak began; null when unknown. */
  onset_unix: number | null;
}
export interface AnomalyHint {
  /** recent（自身刚启动）/ correlation（他进程时间相近）/ context（此前事件）/ none（空态） */
  tone: string;
  text: string;
}
/** 时间线事件，与提示文字同源同窗；kind: onset | process-start | journal | decision */
export interface TimelineEvent {
  at_unix: number;
  kind: string;
  label: string;
}
export interface AnomalyContext {
  pid: number;
  name: string;
  kind: string;
  onset_unix: number | null;
  age_secs: number | null;
  hints: AnomalyHint[];
  timeline: TimelineEvent[];
}
/** 场景识别影子观测产生的加入名单建议；采纳与否始终由用户决定。 */
export interface SceneSuggestion {
  name: string;
  total_hits: number;
  distinct_days: number;
  last_seen_unix: number;
}
/** 常驻模式学习建议：长期驻留、低活跃、占用可观的进程。只建议不处置。 */
export interface ResidencySuggestion {
  name: string;
  role_hint: string;
  resident_days: number;
  avg_mem_mb: number;
  low_cpu_ratio: number;
  last_seen_unix: number;
}
/** 实验一致性影子分析：只输出观察结论，绝不自动调参。 */
export interface ModeConsistency {
  mode: string;
  analyzed: number;
  cpu_direction: string;
  cpu_median_delta: number | null;
  mem_direction: string;
  mem_median_delta: number | null;
}
export interface ExperimentConsistencyReport {
  modes: ModeConsistency[];
  /** ready（存在达标模式）/ insufficient / inconsistent / empty */
  verdict: string;
  summary: string;
}
/** 上报给一致性分析的单条实验记录（与 OptimizationLab 本地记录同构）。 */
export interface ExperimentRecordInput {
  mode: string;
  status: string;
  baseline: {
    duration_ms: number;
    sample_count: number;
    cpu_pct: number | null;
    memory_used_gb: number | null;
    /** Per-metric valid observation fractions over the full one-minute window (0..1). */
    coverage: Record<string, number>;
  };
  after?: {
    duration_ms: number;
    sample_count: number;
    cpu_pct: number | null;
    memory_used_gb: number | null;
    /** Per-metric valid observation fractions over the full one-minute window (0..1). */
    coverage: Record<string, number>;
  } | null;
}
export interface ProcessInsightEntry {
  pid: number;
  name: string;
  path: string;
  role_id: string;
  role_label: string;
  role_detail: string;
  mem_mb: number;
  cpu: number;
  start_age_secs: number | null;
  /** signed / unsigned / unknown（内嵌 Authenticode 校验，目录签名的 OS 组件不计） */
  signature: string;
}
export interface ProcessInsights {
  generated_unix: number;
  entries: ProcessInsightEntry[];
  signature_deferred: number;
}
export interface Deduction {
  item: string;
  cost: number;
}
export interface Diagnostics {
  score: number;
  deductions: Deduction[];
  anomalies: Anomaly[];
  process_sample_ready: boolean;
  cpu_baseline_warming_processes: number;
  mem_baseline_warming_processes: number;
  io_baseline_warming_processes: number;
  mem_slope: number;
  mem_pct: number | null;
  mem_slope_ready: boolean;
  mem_slope_sample_count: number;
  mem_critical_eta_secs: number | null;
  cpu_trend_pct_per_sec: number;
  cpu_forecast_10s_pct: number;
  cpu_forecast_stability: number;
  cpu_forecast_sample_count: number;
  cpu_forecast_observed_span_secs: number;
  cpu_forecast_ready: boolean;
}
export interface SmartRecommendation {
  id: string;
  title: string;
  reason: string;
  action: string;
  priority: number;
}
export type TrimOutcome =
  | { type: "trimmed"; n: number; mb: number }
  | { type: "skipped"; reason: string };
export interface AutoSettings {
  adaptive: boolean;
  auto_game: boolean;
  auto_power: boolean;
  game_boost: boolean;
  cpu_scheduler: boolean;
  scheduler_shadow: boolean;
  close_to_tray: boolean;
  game_list: string[];
  protected_apps: string[];
  rules: Rule[];
  auto_restore_point: boolean;
  away_idle_secs: number;
  heavy_cpu_threshold: number;
  state_seconds: Record<string, number>;
  auto_dim: boolean;
  history: Record<string, Record<string, number>>;
  /** 全自动模式：整条静默优化栈的总开关（不包含删除/杀进程/网络修改等破坏性动作）。 */
  autopilot: boolean;
  /** 全自动模式子开关：持续超阈值的非关键、非前台进程自动降低优先级（仅 autopilot 开启时生效）。 */
  auto_dispose_anomalies: boolean;
  /** 开启全自动模式前的开关快照；关闭时按此还原。 */
  autopilot_backup: {
    adaptive: boolean;
    auto_game: boolean;
    auto_power: boolean;
    game_boost: boolean;
    cpu_scheduler: boolean;
    scheduler_shadow: boolean;
    auto_dispose_anomalies: boolean;
  } | null;
  /** Set when persisted settings cannot be read or parsed and automation is paused. */
  load_warning?: string;
}
export interface DecisionLog {
  ts: number;
  from: string;
  to: string;
  reason: string;
}
export interface CpuSchedulerStatus {
  supported: boolean;
  eco_qos_processes: number;
  foreground_priority: string | null;
  last_updated_unix: number;
  detail: string;
  monitored_processes: number;
  background_candidates: number;
  background_cpu_cap_pct: number;
  candidate_limit: number;
}
export interface AutoStateInfo {
  cpu_scheduler: CpuSchedulerStatus;
  settings: AutoSettings;
  foreground: string;
  fullscreen: boolean;
  mode: string;
  boosted_pid: number | null;
  cooldown: boolean;
  adaptive: boolean;
  adaptive_state: string;
  baseline_restore_pending: boolean;
  brightness_restore_pending: boolean;
  brightness_restore_snapshot_error: boolean;
  brightness_preserved_value: number | null;
  stats_sample_id: number;
  cpu_core_peak_pct: number;
  cpu_saturated_cores: number;
  cpu_logical_cores: number;
  idle_secs: number;
  hold_secs: number;
  decisions: DecisionLog[];
  selected_algorithm: string;
  algorithm_reason: string;
  algorithm_scores: { id: string; score: number; suitable: boolean; reason: string }[];
}
export interface PingStat {
  ok: number;
  avg: number | null;
  sd: number | null;
  loss_pct: number;
}
export interface NetQuality {
  gateway: PingStat | null;
  dns: PingStat;
  second_dns?: PingStat | null;
  summary: { label: string; detail: string; tone: "rose" | "amber" | "green" };
}
export interface Rule {
  kind: string;
  enabled: boolean;
  threshold: number;
  seconds: number;
  action: string;
}
export interface BootRecord {
  ts: number;
  main_secs: number;
  post_secs: number;
  total_secs: number;
  level: number;
}
export interface StartupImpact {
  ts: number;
  name: string;
  path: string;
  total_ms: number;
  degradation_ms: number;
}
export type ReliabilityEventKind = "app_crash" | "app_hang" | "unexpected_shutdown" | "planned_shutdown" | "update_installed" | "update_failed";
export interface ReliabilityEvent {
  ts: number;
  kind: ReliabilityEventKind;
  title: string;
  detail: string;
  source: string;
}
export interface ReliabilitySourceStatus {
  id: "application" | "system" | "updates";
  label: string;
  available: boolean;
  fetched: number;
  limit_reached: boolean;
  error: string | null;
}
export interface ReliabilitySnapshot {
  window_days: number;
  candidate_count: number;
  omitted_estimate: number;
  invalid_rows: number;
  events: ReliabilityEvent[];
  sources: ReliabilitySourceStatus[];
}
export interface SvcInfo {
  name: string;
  display: string;
  start_mode: string;
  state: string;
  path: string;
  account: string;
  modifiable: boolean;
  start_reg: number | null;
  delayed: number | null;
}
export interface PlanDup {
  guid: string;
  name: string;
  active: boolean;
}
export interface TaskInfo {
  path: string;
  name: string;
  state: string;
  author: string;
  logon: boolean;
  boot: boolean;
  action: string;
  third_party: boolean;
}
export interface DirSize {
  path: string;
  bytes: number;
  files: number;
}
export interface FileSize {
  path: string;
  bytes: number;
  modified: number;
}
export type DiskScanPreset = "user_profile" | "system_drive";
export interface DiskScanResult {
  root: string;
  total_files: number;
  total_bytes: number;
  cancelled: boolean;
  truncated: boolean;
  dirs: DirSize[];
  files: FileSize[];
  duration_ms: number;
  delete_token: string;
}
export interface ScanProgress {
  files: number;
  bytes: number;
  current: string;
}
export interface DuplicateFile {
  path: string;
  bytes: number;
  modified_unix_s: number;
}
export interface DuplicateGroup {
  id: string;
  bytes: number;
  files_total: number;
  files_truncated: boolean;
  logical_duplicate_bytes: number;
  files: DuplicateFile[];
}
export interface DuplicateScanResult {
  root: string;
  files_scanned: number;
  candidate_files: number;
  bytes_hashed: number;
  logical_duplicate_bytes: number;
  skipped_unreadable: number;
  changed_during_scan: number;
  cancelled: boolean;
  truncated: boolean;
  duration_ms: number;
  groups: DuplicateGroup[];
  delete_token: string;
  root_token: string;
}
export interface DuplicateScanProgress {
  phase: string;
  files_scanned: number;
  candidate_files: number;
  bytes_hashed: number;
  current: string;
}
export interface DiskHealth {
  name: string;
  media: string;
  health: string;
  size_gb: number;
  temp: number | null;
  wear: number | null;
  power_on_hours: number | null;
  read_errors: number | null;
  write_errors: number | null;
}
/** SMART 磁盘健康趋势：基于本机有限样本的观察，不做寿命预测。 */
export interface DiskTrend {
  name: string;
  samples: number;
  span_days: number | null;
  wear_delta: number | null;
  wear_rate_per_1000h: number | null;
  hours_delta: number | null;
  error_delta: number | null;
  /** error-rise（新错误）/ wear-moving（磨损上升）/ stable / insufficient */
  tone: string;
  summary: string;
}
export interface DiskTrendReport {
  trends: DiskTrend[];
  stored: boolean;
  skipped_unchanged: number;
  generated_unix: number;
}
export interface VolumeOpt {
  drive: string;
  media: string;
  fragmentation: number | null;
}
export interface DefragStatus {
  volumes: VolumeOpt[];
  task_enabled: boolean;
  last_run: string;
  next_run: string;
}
export interface ResidualItem {
  path: string;
  label: string;
  desc: string;
  size_mb: number | null;
  exists: boolean;
  safe_to_delete: boolean;
  scan_error: string | null;
}
export interface AppxApp {
  /** Readable name from Start menu registration/manifest with a folder-name fallback. */
  name: string;
  /** Original package identity name; useful for searching UUID aliases. */
  package_name: string;
  full_name: string;
  publisher: string;
  install_location: string;
  version: string;
  /** System-signed or protected Windows inbox package. */
  protected: boolean;
}
export interface GpuPref {
  exe: string;
  pref: number;
}
export interface HibernateInfo {
  supported: boolean;
  enabled: boolean;
  size_gb: number;
}
export interface WifiProfile {
  ssid: string;
  auth: string;
  cipher: string;
}
export interface NicPower {
  name: string;
  guid: string;
  pm_disabled: boolean;
}
export interface TcpLatency {
  name: string;
  guid: string;
  ack_freq: number | null;
  nodelay: number | null;
}
export interface ShadowStore {
  drive: string;
  used_gb: number;
  allocated_gb: number;
  max_gb: number;
}
export interface SysInfo {
  os: string;
  version: string;
  arch: string;
  host: string;
  cpu_model: string;
  cores: number;
  admin: boolean;
  exe: string;
  silent: boolean;
}
export interface UpdateReadiness {
  enabled: boolean;
  task_registered: boolean;
  reason: string | null;
}
export interface AvailableUpdate {
  current_version: string;
  available_version: string;
}

export const api = {
  getStats: () => invoke<Stats>("get_stats"),
  onStats: (cb: (s: Stats) => void) => listen<Stats>("stats", (e) => cb(e.payload)),

  getSystemInfo: () => invoke<SysInfo>("get_system_info"),
  exportDiagnosticReport: () => invoke<string>("export_diagnostic_report"),
  saveTextExport: (filename: string, contents: string) =>
    invoke<boolean>("save_text_export", { filename, contents }),
  isAdmin: () => invoke<boolean>("is_admin"),
  isSilentStart: () => invoke<boolean>("is_silent_start"),
  relaunchAsAdmin: () => invoke<void>("relaunch_as_admin"),
  getSignedUpdateReadiness: () => invoke<UpdateReadiness>("get_signed_update_readiness"),
  checkSignedUpdate: () => invoke<AvailableUpdate | null>("check_signed_update"),
  requestSignedUpdate: () => invoke<void>("request_signed_update"),
  openPath: (p: string) => invoke<void>("open_path", { path: p }),

  listProcesses: () => invoke<ProcInfo[]>("list_processes"),
  killProcess: (pid: number, creationToken: string) =>
    invoke<void>("kill_process", { pid, creationToken }),
  setPriority: (pid: number, creationToken: string, priority: number, allowRealtime = false) =>
    invoke<void>("set_priority", { pid, creationToken, priority, allowRealtime }),
  suspendProcess: (pid: number) => invoke<void>("suspend_process", { pid }),
  resumeProcess: (pid: number, creationToken: string) =>
    invoke<void>("resume_process", { pid, creationToken }),
  trimMemory: () => invoke<[number, number]>("trim_memory"),
  previewProcessMemoryOptimization: (pids?: number[]) =>
    invoke<MemoryOptimizationReport>("preview_process_memory_optimization", { pids: pids ?? null }),
  optimizeProcessMemory: (pids: MemoryCandidateIdentity[]) =>
    invoke<MemoryOptimizationReport>("optimize_process_memory", { pids }),

  listPowerPlans: () => invoke<Plan[]>("list_power_plans"),
  setPowerPlan: (guid: string) => invoke<void>("set_power_plan", { guid }),
  batteryReport: () => invoke<string>("battery_report"),
  powerDiagnostics: () => invoke<PowerDiagnostics>("power_diagnostics"),

  getTweakStates: () => invoke<TweakStates>("get_tweak_states"),
  setTweak: (key: string, on: boolean) => invoke<string>("set_tweak", { key, on }),
  applyMode: (mode: string) => invoke<string[]>("apply_mode", { mode }),
  getModeState: () => invoke<string>("get_mode_state"),

  listStartup: () => invoke<StartupItem[]>("list_startup"),
  toggleStartup: (id: string, enable: boolean) => invoke<void>("toggle_startup", { id, enable }),
  bootHistory: () => invoke<BootRecord[]>("boot_history"),
  startupImpacts: () => invoke<StartupImpact[]>("startup_impacts"),
  reliabilityEvents: (days: number) => invoke<ReliabilitySnapshot>("get_reliability_events", { days }),
  delayStartupItem: (name: string, delaySecs: number) =>
    invoke<void>("delay_startup_item", { name, delaySecs }),
  undelayStartupItem: (name: string) => invoke<void>("undelay_startup_item", { name }),
  listDelayedStartup: () => invoke<{ name: string; task: string }[]>("list_delayed_startup"),

  scanJunk: () => invoke<JunkItem[]>("scan_junk"),
  previewJunk: (ids: string[], olderThanDays: number) =>
    invoke<JunkCleanupPreview>("preview_junk", { ids, olderThanDays }),
  cleanJunk: (planId: string) => invoke<JunkCleanupResult>("clean_junk", { planId }),
  emptyRecycleBin: (planId: string) => invoke<boolean>("empty_recycle_bin", { planId }),

  getDiagnostics: () => invoke<Diagnostics>("get_diagnostics"),
  getAnomalyContext: () => invoke<AnomalyContext[]>("get_anomaly_context"),
  getProcessInsights: () => invoke<ProcessInsights>("get_process_insights"),
  getSmartRecommendations: () => invoke<SmartRecommendation[]>("get_smart_recommendations"),
  smartTrim: () => invoke<TrimOutcome>("smart_trim"),

  getAutoSettings: () => invoke<AutoSettings>("get_auto_settings"),
  setAutoSetting: (key: string, on: boolean) =>
    invoke<void>("set_auto_setting", { key, on }),
  retryBaselineRestore: () =>
    invoke<void>("set_auto_setting", { key: "retry_baseline_restore", on: false }),
  retryBrightnessRestore: () =>
    invoke<void>("set_auto_setting", { key: "retry_brightness_restore", on: false }),
  setAutopilot: (enabled: boolean) => invoke<AutoSettings>("set_autopilot", { enabled }),
  getAutoState: () => invoke<AutoStateInfo>("get_auto_state"),
  addGameItem: (name: string) => invoke<string[]>("add_game_item", { name }),
  removeGameItem: (name: string) => invoke<string[]>("remove_game_item", { name }),
  getSceneSuggestions: () => invoke<SceneSuggestion[]>("get_scene_suggestions"),
  dismissSceneSuggestion: (name: string) => invoke<boolean>("dismiss_scene_suggestion", { name }),
  getResidencySuggestions: () => invoke<ResidencySuggestion[]>("get_residency_suggestions"),
  dismissResidencySuggestion: (name: string) => invoke<boolean>("dismiss_residency_suggestion", { name }),
  analyzeExperimentConsistency: (records: ExperimentRecordInput[]) =>
    invoke<ExperimentConsistencyReport>("analyze_experiment_consistency", { records }),
  setProtectedApps: (names: string[]) => invoke<string[]>("set_protected_apps", { names }),
  setRule: (kind: string, enabled: boolean, threshold: number, seconds: number, action: string) =>
    invoke<Rule[]>("set_rule", { kind, enabled, threshold, seconds, action }),

  getEpp: () => invoke<number | null>("get_epp"),
  setEpp: (v: number) => invoke<void>("set_epp", { v }),

  pingQuality: () => invoke<NetQuality>("ping_quality"),

  listServices: () => invoke<SvcInfo[]>("list_services"),
  setServiceStartup: (name: string, mode: string) =>
    invoke<void>("set_service_startup", { name, mode }),

  listLogonTasks: () => invoke<TaskInfo[]>("list_logon_tasks"),
  setTaskState: (task_path: string, task_name: string, enable: boolean) =>
    invoke<void>("set_task_state", { taskPath: task_path, taskName: task_name, enable }),

  scanDisk: (preset: DiskScanPreset) => invoke<DiskScanResult>("scan_disk", { preset }),
  cancelDiskScan: () => invoke<void>("cancel_disk_scan"),
  pickDuplicateScanRoot: () => invoke<string | null>("pick_duplicate_scan_root"),
  scanDuplicateFiles: (rootToken: string) =>
    invoke<DuplicateScanResult>("scan_duplicate_files", { rootToken }),
  cancelDuplicateScan: () => invoke<void>("cancel_duplicate_scan"),
  recycleDelete: (deleteToken: string, paths: string[]) =>
    invoke<void>("recycle_delete", { deleteToken, paths }),
  diskHealth: () => invoke<DiskHealth[]>("disk_health"),
  getDiskHealthTrends: () => invoke<DiskTrendReport>("get_disk_health_trends"),
  dismissDiskTrend: (name: string) => invoke<boolean>("dismiss_disk_trend", { name }),
  onDiskScanProgress: (cb: (p: ScanProgress) => void) =>
    listen<ScanProgress>("disk_scan_progress", (e) => cb(e.payload)),
  onDuplicateScanProgress: (cb: (p: DuplicateScanProgress) => void) =>
    listen<DuplicateScanProgress>("duplicate_scan_progress", (e) => cb(e.payload)),

  listAppx: () => invoke<AppxApp[]>("list_appx").then((apps) => Array.isArray(apps) ? apps : []),
  removeAppx: (fullName: string) => invoke<void>("remove_appx", { fullName }),

  listGpuPrefs: () => invoke<GpuPref[]>("list_gpu_prefs"),
  setGpuPref: (exe: string, pref: number) => invoke<void>("set_gpu_pref", { exe, pref }),

  setProcessAffinity: (pid: number, creationToken: string, kind: string) =>
    invoke<void>("set_process_affinity", { pid, creationToken, kind }),

  setUpdatePause: (paused: boolean) => invoke<void>("set_update_pause", { paused }),
  getUpdatePause: () => invoke<boolean>("get_update_pause"),

  hibernateInfo: () => invoke<HibernateInfo>("hibernate_info"),
  setHibernate: (on: boolean) => invoke<void>("set_hibernate", { on }),

  createRestorePoint: (desc: string) => invoke<string>("create_restore_point", { desc }),

  listWifiProfiles: () => invoke<WifiProfile[]>("list_wifi_profiles"),

  listNicPower: () => invoke<NicPower[]>("list_nic_power"),
  setNicPower: (guid: string, disable: boolean) =>
    invoke<void>("set_nic_power", { guid, disable }),
  listTcpLatency: () => invoke<TcpLatency[]>("list_tcp_latency"),
  setTcpLatency: (guid: string, on: boolean) =>
    invoke<void>("set_tcp_latency", { guid, on }),
  setTelemetryMinimal: (on: boolean) => invoke<void>("set_telemetry_minimal", { on }),
  getTelemetryMinimal: () => invoke<boolean>("get_telemetry_minimal"),
  listShadowStorage: () => invoke<ShadowStore[]>("list_shadow_storage"),
  deleteOldestShadow: (drive: string) => invoke<void>("delete_oldest_shadow", { drive }),
  setShadowMax: (drive: string, maxGb: number) =>
    invoke<void>("set_shadow_max", { drive, maxGb }),
  setAutoNum: (key: string, value: number) => invoke<void>("set_auto_num", { key, value }),
  getStateSeconds: () => invoke<Record<string, number>>("get_state_seconds"),

  exportConfig: () => invoke<string>("export_config"),
  importConfig: () => invoke<boolean>("import_config"),

  compactosStatus: () => invoke<boolean>("compactos_status"),
  setCompactos: (on: boolean) => invoke<void>("set_compactos", { on }),
  memCompressionStatus: () => invoke<boolean>("mem_compression_status"),
  setMemCompression: (on: boolean) => invoke<void>("set_mem_compression", { on }),
  defragStatus: () => invoke<DefragStatus>("defrag_status"),
  defragOptimize: (drive: string) => invoke<void>("defrag_optimize", { drive }),
  defragAnalyze: (drive: string) => invoke<number | null>("defrag_analyze", { drive }),
  setDefragSchedule: (enable: boolean, day: string, time: string) =>
    invoke<void>("set_defrag_schedule", { enable, day, time }),
  scanResiduals: () => invoke<ResidualItem[]>("scan_residuals"),
  removeResidual: (path: string) => invoke<boolean>("remove_residual", { path }),
  dismComponentCleanup: () => invoke<void>("dism_component_cleanup"),
  dismComponentCleanupReset: () => invoke<void>("dism_component_cleanup_reset"),

  listRedundantPlans: () => invoke<PlanDup[]>("list_redundant_plans"),
  removePlan: (guid: string) => invoke<string>("remove_plan", { guid }),

  onAutoActions: (cb: (actions: string[]) => void) =>
    listen<string[]>("auto_actions", (e) => cb(e.payload)),
  onTrayModeApplied: (cb: (message: string) => void) =>
    listen<string>("tray-mode-applied", (e) => cb(e.payload)),
  onTrayModeFailed: (cb: (message: string) => void) =>
    listen<string>("tray-mode-failed", (e) => cb(e.payload)),

  listAdapters: () => invoke<Adapter[]>("list_adapters"),
  getDns: () => invoke<DnsEntry[]>("get_dns"),
  setDns: (iface: string, primary: string, secondary: string) =>
    invoke<void>("set_dns", { iface, primary, secondary }),
  resetDns: (iface: string) => invoke<void>("reset_dns", { iface }),
  flushDns: () => invoke<void>("flush_dns"),
  getTcpAutotuning: () => invoke<string>("get_tcp_autotuning"),
  setTcpAutotuning: (level: string) => invoke<void>("set_tcp_autotuning", { level }),

  listChanges: () => invoke<Change[]>("list_changes"),
  revertChange: (id: number) => invoke<void>("revert_change", { id }),
  revertChangeGroup: (groupId: number) => invoke<number>("revert_change_group", { groupId }),
  revertAll: () => invoke<number>("revert_all"),
  clearJournal: () => invoke<void>("clear_journal"),

  openSettingsPage: (page: string) => invoke<void>("open_settings_page", { page }),

  setAutostart: (enable: boolean) => invoke<void>("set_autostart", { enable }),
  getAutostart: () => invoke<boolean>("get_autostart"),
};
