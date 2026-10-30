use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use sysinfo::{Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::ProcessStatus::{
    EmptyWorkingSet, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
};
use windows_sys::Win32::System::SystemInformation::{
    GetSystemWindowsDirectoryW, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows_sys::Win32::System::Threading::{
    GetActiveProcessorCount, GetProcessAffinityMask, GetProcessTimes, OpenProcess,
    QueryFullProcessImageNameW, SetPriorityClass, SetProcessAffinityMask, TerminateProcess,
    ABOVE_NORMAL_PRIORITY_CLASS, ALL_PROCESSOR_GROUPS, BELOW_NORMAL_PRIORITY_CLASS,
    HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    PROCESS_QUERY_LIMITED_INFORMATION, REALTIME_PRIORITY_CLASS,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const PROCESS_TERMINATE: u32 = 0x0001;
const PROCESS_SET_INFORMATION: u32 = 0x0200;
const PROCESS_SUSPEND_RESUME: u32 = 0x0800;
const PROCESS_SET_QUOTA: u32 = 0x0100;
const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
const PROCESS_SUSPEND_DISABLED_ERROR: &str =
    "进程挂起暂时关闭，以避免 AeroTune 退出后遗留冻结进程；恢复入口仍可使用。";

const MIN_TRIM_WORKING_SET_BYTES: u64 = 128 * 1024 * 1024;
const MAX_TRIM_CPU_PERCENT: f32 = 3.0;
const MAX_MEMORY_CANDIDATES: usize = 20;
const MAX_MEMORY_BATCH: usize = 10;
const AUTO_TRIM_WORKING_SET_BYTES: u64 = 512 * 1024 * 1024;
const AUTO_TRIM_WORKING_SET_MB: f64 = 512.0;
const AUTO_TRIM_CPU_PERCENT: f32 = 1.0;
const AUTO_TRIM_MAX_BATCH: usize = 2;
const AUTO_TRIM_COOLDOWN_SECS: u64 = 15 * 60;
// Match the stats worker's 60-tick gate while allowing newly idle candidates to be reconsidered.
const AUTO_TRIM_SCAN_RETRY_SECS: u64 = 60;
const AUTO_TRIM_AVAILABLE_LIMIT_MB: f64 = 4.0 * 1024.0;
static AUTO_TRIM_RUNNING: AtomicBool = AtomicBool::new(false);
static AUTO_TRIM_LAST_SUCCESS: Mutex<Option<Instant>> = Mutex::new(None);
static AUTO_TRIM_LAST_SCAN_ATTEMPT: Mutex<Option<Instant>> = Mutex::new(None);
const WINDOWS_BACKGROUND_CANDIDATES: &[&str] = &[
    "searchindexer.exe",
    "searchhost.exe",
    "runtimebroker.exe",
    "sihost.exe",
    "widgets.exe",
];

pub(crate) const PROTECTED_PROCESSES: &[&str] = &[
    "system",
    "registry",
    "secure system",
    "memory compression",
    "system idle process",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "lsm.exe",
    "svchost.exe",
    "dwm.exe",
    "explorer.exe",
    "fontdrvhost.exe",
    "audiodg.exe",
    "spoolsv.exe",
    "msmpeng.exe",
    "nissrv.exe",
    "wmiprvse.exe",
];

#[derive(Serialize, Clone)]
pub struct ProcInfo {
    pub pid: u32,
    /// Lossless FILETIME from a query handle; omitted when it cannot be cross-checked against the row.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub creation_token: Option<String>,
    pub parent_pid: Option<u32>,
    pub name: String,
    /// CPU 利用率：自上一列表刷新以来的区间平均值。
    pub cpu: f32,
    pub mem_mb: f64,
    /// 磁盘吞吐（读+写，MB/s，两次采样折算）
    pub io_mb_s: f64,
    pub path: String,
    /// 本地静态证据画像的角色 ID（见 profile.rs）。
    pub role: String,
    pub role_label: String,
}

/// 进程详情：列表行 + 画像 + 创建时间，供洞察类只读命令使用。
pub struct ProcessDetail {
    pub info: ProcInfo,
    pub role: crate::profile::ProcessRole,
    /// 进程创建时间（Unix 秒）；不可读时为 None。
    pub start_unix: Option<u64>,
}

const PROCESS_LIST_LIMIT: usize = 500;
const PROCESS_LIST_WARMUP_INTERVAL: Duration = Duration::from_millis(300);
const PROCESS_LIST_REWARM_AFTER: Duration = Duration::from_secs(30);
const PROCESS_LIST_IDLE_RESET_AFTER: Duration = Duration::from_secs(60);
const PROCESS_LIST_CLEANUP_INTERVAL: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProcessListSamplePlan {
    Warmup,
    Steady(Instant),
}

fn process_list_sample_plan(last_sample: Option<Instant>, now: Instant) -> ProcessListSamplePlan {
    match last_sample {
        Some(last) if now.saturating_duration_since(last) < PROCESS_LIST_REWARM_AFTER => {
            ProcessListSamplePlan::Steady(last)
        }
        _ => ProcessListSamplePlan::Warmup,
    }
}

fn process_list_sampler_idle(last_sample: Option<Instant>, now: Instant) -> bool {
    last_sample
        .is_some_and(|last| now.saturating_duration_since(last) >= PROCESS_LIST_IDLE_RESET_AFTER)
}

fn process_list_sample_elapsed(previous: Instant, completed: Instant) -> Duration {
    completed
        .saturating_duration_since(previous)
        .max(Duration::from_millis(1))
}

struct ProcessListSampler {
    system: System,
    last_sample: Option<Instant>,
    last_creation_filetimes: HashMap<u32, u64>,
    identity_query_buffer: Vec<u16>,
}

impl ProcessListSampler {
    fn new() -> Self {
        Self {
            system: System::new(),
            last_sample: None,
            last_creation_filetimes: HashMap::new(),
            identity_query_buffer: Vec::new(),
        }
    }

    fn sample(&mut self) -> Vec<ProcInfo> {
        let (mut rows, before_refresh_filetimes) = self.sample_plan(collect, true);
        self.last_creation_filetimes =
            mask_unstable_process_row_tokens(&mut rows, &before_refresh_filetimes);
        rows
    }

    fn sample_details(&mut self) -> Vec<ProcessDetail> {
        self.sample_plan(collect_details, false).0
    }

    fn sample_plan<T>(
        &mut self,
        collect_with: impl Fn(&System, Duration) -> Vec<T>,
        verify_row_identity: bool,
    ) -> (Vec<T>, HashMap<u32, u64>) {
        let process_refresh = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet);
        let process_io_refresh = process_refresh.with_disk_usage();
        match process_list_sample_plan(self.last_sample, Instant::now()) {
            ProcessListSamplePlan::Warmup => {
                self.system.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    process_io_refresh,
                );
                let before_refresh_filetimes = if verify_row_identity {
                    capture_top_process_filetimes(&self.system, &mut self.identity_query_buffer)
                } else {
                    HashMap::new()
                };
                if verify_row_identity {
                    // Seed sysinfo's CPU/I/O baseline only after the first identity capture.
                    self.system = System::new();
                    self.system.refresh_processes_specifics(
                        ProcessesToUpdate::All,
                        true,
                        process_io_refresh,
                    );
                }
                let first_sample = Instant::now();
                std::thread::sleep(PROCESS_LIST_WARMUP_INTERVAL);
                self.system.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    process_io_refresh,
                );
                let completed = Instant::now();
                let elapsed = process_list_sample_elapsed(first_sample, completed);
                self.last_sample = Some(completed);
                (
                    collect_with(&self.system, elapsed),
                    before_refresh_filetimes,
                )
            }
            ProcessListSamplePlan::Steady(previous) => {
                let before_refresh_filetimes = if verify_row_identity {
                    let observed_filetimes = capture_top_process_filetimes(
                        &self.system,
                        &mut self.identity_query_buffer,
                    );
                    unchanged_process_filetimes(&self.last_creation_filetimes, &observed_filetimes)
                } else {
                    HashMap::new()
                };
                self.system.refresh_processes_specifics(
                    ProcessesToUpdate::All,
                    true,
                    process_io_refresh,
                );
                let completed = Instant::now();
                let elapsed = process_list_sample_elapsed(previous, completed);
                self.last_sample = Some(completed);
                (
                    collect_with(&self.system, elapsed),
                    before_refresh_filetimes,
                )
            }
        }
    }
}

static PROCESS_LIST_SAMPLER: OnceLock<Mutex<Option<ProcessListSampler>>> = OnceLock::new();
static PROCESS_LIST_CLEANUP_THREAD: OnceLock<()> = OnceLock::new();

fn process_list_sampler_state() -> &'static Mutex<Option<ProcessListSampler>> {
    let state = PROCESS_LIST_SAMPLER.get_or_init(|| Mutex::new(None));
    PROCESS_LIST_CLEANUP_THREAD.get_or_init(|| {
        let state = PROCESS_LIST_SAMPLER
            .get()
            .expect("process-list sampler was just initialized");
        let _ = std::thread::Builder::new()
            .name("aerotune-process-list-cache".into())
            .spawn(move || loop {
                std::thread::sleep(PROCESS_LIST_CLEANUP_INTERVAL);
                let mut sampler = state.lock().unwrap_or_else(|error| error.into_inner());
                if sampler.as_ref().is_some_and(|sampler| {
                    process_list_sampler_idle(sampler.last_sample, Instant::now())
                }) {
                    *sampler = None;
                }
            });
    });
    state
}

#[derive(Serialize, Clone, Debug)]
pub struct MemoryCandidate {
    pub pid: u32,
    pub name: String,
    pub path: String,
    pub working_set_mb: f64,
    pub private_mb: f64,
    pub cpu: f32,
    pub eligible: bool,
    pub reason: String,
    /// Lossless opaque FILETIME identity token, serialized as a fixed-width hex string.
    #[serde(
        rename = "creation_token",
        serialize_with = "serialize_creation_filetime",
        skip_serializing_if = "Option::is_none"
    )]
    creation_filetime: Option<u64>,
}

fn serialize_creation_filetime<S>(value: &Option<u64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    value.map(format_creation_token).serialize(serializer)
}

#[derive(Deserialize, Clone, Debug)]
#[serde(deny_unknown_fields)]
pub struct MemoryCandidateIdentity {
    pub pid: u32,
    pub creation_token: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProcessCpuSample {
    usage_percent: f32,
    creation_filetime: u64,
}

fn apply_cpu_samples_to_candidates(
    candidates: &mut [MemoryCandidate],
    samples: &HashMap<u32, ProcessCpuSample>,
    max_cpu_percent: f32,
) -> usize {
    let mut rejected = 0;
    for candidate in candidates.iter_mut().filter(|candidate| candidate.eligible) {
        let sample = samples
            .get(&candidate.pid)
            .filter(|sample| candidate.creation_filetime == Some(sample.creation_filetime));
        let Some(sample) = sample else {
            candidate.eligible = false;
            candidate.reason = "CPU 采样不可用或进程实例已变化，跳过".into();
            rejected += 1;
            continue;
        };
        candidate.cpu = sample.usage_percent;
        if !cpu_usage_is_eligible(sample.usage_percent, max_cpu_percent) {
            candidate.eligible = false;
            candidate.reason = "当前 CPU 活跃度较高".into();
            rejected += 1;
        } else {
            candidate.reason = "空闲第三方进程".into();
        }
    }
    rejected
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct MemoryOptimizationReport {
    pub available_before_mb: f64,
    pub available_after_mb: Option<f64>,
    pub candidates: Vec<MemoryCandidate>,
    pub processed: usize,
    pub touched_working_set_mb: f64,
    pub estimated_reclaimed_mb: f64,
    pub skipped: usize,
}

fn disk_rate_mb_s(read_bytes: u64, written_bytes: u64, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if !secs.is_finite() || secs <= 0.0 {
        return 0.0;
    }
    read_bytes.saturating_add(written_bytes) as f64 / secs / 1024.0 / 1024.0
}

fn process_priority_order(
    left_cpu: f32,
    left_mem_mb: f64,
    left_pid: u32,
    right_cpu: f32,
    right_mem_mb: f64,
    right_pid: u32,
) -> std::cmp::Ordering {
    let left_cpu = if left_cpu.is_finite() { left_cpu } else { 0.0 };
    let right_cpu = if right_cpu.is_finite() {
        right_cpu
    } else {
        0.0
    };
    right_cpu
        .total_cmp(&left_cpu)
        .then_with(|| right_mem_mb.total_cmp(&left_mem_mb))
        .then_with(|| left_pid.cmp(&right_pid))
}

fn compare_proc_infos(left: &ProcInfo, right: &ProcInfo) -> std::cmp::Ordering {
    process_priority_order(
        left.cpu,
        left.mem_mb,
        left.pid,
        right.cpu,
        right.mem_mb,
        right.pid,
    )
}

fn sort_proc_info_rows(rows: &mut [ProcInfo]) {
    // PID is the final tie-breaker and is unique in a System process snapshot.
    rows.sort_unstable_by(compare_proc_infos);
}

fn sort_process_detail_rows(rows: &mut [ProcessDetail]) {
    // Reuse the same total ordering for details and the lighter list DTOs.
    rows.sort_unstable_by(|left, right| compare_proc_infos(&left.info, &right.info));
}

fn retain_top_by<T>(
    items: &mut Vec<T>,
    limit: usize,
    compare: impl Fn(&T, &T) -> std::cmp::Ordering,
) {
    if items.len() > limit {
        items.select_nth_unstable_by(limit, |left, right| compare(left, right));
        items.truncate(limit);
    }
}

fn materialize_top_by<T, U>(
    mut candidates: Vec<&T>,
    limit: usize,
    compare: impl Fn(&T, &T) -> std::cmp::Ordering,
    materialize: impl FnMut(&T) -> U,
) -> Vec<U> {
    retain_top_by(&mut candidates, limit, |left, right| compare(left, right));
    candidates.into_iter().map(materialize).collect()
}

/// sysinfo reports process CPU relative to one core; convert it to whole-machine 0–100%.
pub(crate) fn normalize_sysinfo_cpu_usage(cpu_usage: f32, cpu_count: usize) -> f32 {
    if !cpu_usage.is_finite() || cpu_usage < 0.0 {
        return cpu_usage;
    }
    (cpu_usage / cpu_count.max(1) as f32).min(100.0)
}

fn to_proc_info(
    process: &Process,
    sample_elapsed: Duration,
    cpu_count: usize,
    autostart_names: &HashSet<String>,
) -> ProcInfo {
    let disk = process.disk_usage();
    let name = process.name().to_string_lossy().into_owned();
    let path = process
        .exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let role = crate::profile::classify_process(&name, &path, autostart_names);
    let pid = process.pid().as_u32();
    ProcInfo {
        pid,
        creation_token: process_creation_token(pid, process.start_time(), &name, &path),
        parent_pid: process.parent().map(|parent| parent.as_u32()),
        name,
        cpu: normalize_sysinfo_cpu_usage(process.cpu_usage(), cpu_count),
        mem_mb: process.memory() as f64 / 1024.0 / 1024.0,
        io_mb_s: disk_rate_mb_s(disk.read_bytes, disk.written_bytes, sample_elapsed),
        path,
        role: role.id.to_string(),
        role_label: role.label.to_string(),
    }
}

fn to_process_detail(
    process: &Process,
    sample_elapsed: Duration,
    cpu_count: usize,
    autostart_names: &HashSet<String>,
) -> ProcessDetail {
    let disk = process.disk_usage();
    let name = process.name().to_string_lossy().into_owned();
    let path = process
        .exe()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let role = crate::profile::classify_process(&name, &path, autostart_names);
    let pid = process.pid().as_u32();
    ProcessDetail {
        info: ProcInfo {
            pid,
            // Insight-only rows are not exposed as resume actions; avoid per-row identity handle queries.
            creation_token: None,
            parent_pid: process.parent().map(|parent| parent.as_u32()),
            name,
            cpu: normalize_sysinfo_cpu_usage(process.cpu_usage(), cpu_count),
            mem_mb: process.memory() as f64 / 1024.0 / 1024.0,
            io_mb_s: disk_rate_mb_s(disk.read_bytes, disk.written_bytes, sample_elapsed),
            path,
            role: role.id.to_string(),
            role_label: role.label.to_string(),
        },
        role,
        start_unix: Some(process.start_time()),
    }
}

fn capture_top_process_filetimes(
    system: &System,
    image_buffer: &mut Vec<u16>,
) -> HashMap<u32, u64> {
    let mut candidates: Vec<&Process> = system.processes().values().collect();
    retain_top_by(&mut candidates, PROCESS_LIST_LIMIT, |left, right| {
        process_priority_order(
            left.cpu_usage(),
            left.memory() as f64 / 1024.0 / 1024.0,
            left.pid().as_u32(),
            right.cpu_usage(),
            right.memory() as f64 / 1024.0 / 1024.0,
            right.pid().as_u32(),
        )
    });
    let mut filetimes = HashMap::with_capacity(candidates.len());
    for process in candidates {
        let Some(expected_path) = process.exe().map(|path| path.to_string_lossy()) else {
            continue;
        };
        let name = process.name().to_string_lossy();
        let pid = process.pid().as_u32();
        if let Some(filetime) = process_creation_filetime_for_sample(
            pid,
            process.start_time(),
            name.as_ref(),
            expected_path.as_ref(),
            image_buffer,
        ) {
            filetimes.insert(pid, filetime);
        }
    }
    filetimes
}

fn unchanged_process_filetimes(
    previous: &HashMap<u32, u64>,
    observed: &HashMap<u32, u64>,
) -> HashMap<u32, u64> {
    previous
        .iter()
        .filter_map(|(pid, filetime)| {
            (observed.get(pid) == Some(filetime)).then_some((*pid, *filetime))
        })
        .collect()
}

fn mask_unstable_process_row_tokens(
    rows: &mut [ProcInfo],
    before_refresh: &HashMap<u32, u64>,
) -> HashMap<u32, u64> {
    let mut after_refresh_filetimes = HashMap::with_capacity(rows.len());
    for row in rows {
        let after_refresh = row
            .creation_token
            .as_deref()
            .and_then(|token| parse_process_creation_token(token).ok());
        if let Some(filetime) = after_refresh {
            after_refresh_filetimes.insert(row.pid, filetime);
        }
        if before_refresh.get(&row.pid).copied() != after_refresh {
            row.creation_token = None;
        }
    }
    after_refresh_filetimes
}

/// 详情收集：与 collect 同序同限，但保留画像角色与创建时间。
fn collect_details(sys: &System, sample_elapsed: Duration) -> Vec<ProcessDetail> {
    let processes = sys.processes();
    let cpu_count = sys.cpus().len();
    // Keep all role labels in this response on one consistent startup snapshot.
    let autostart_names = crate::profile::autostart_snapshot();
    let mut rows: Vec<ProcessDetail> = if processes.len() > PROCESS_LIST_LIMIT {
        materialize_top_by(
            processes.values().collect(),
            PROCESS_LIST_LIMIT,
            |left, right| {
                process_priority_order(
                    left.cpu_usage(),
                    left.memory() as f64 / 1024.0 / 1024.0,
                    left.pid().as_u32(),
                    right.cpu_usage(),
                    right.memory() as f64 / 1024.0 / 1024.0,
                    right.pid().as_u32(),
                )
            },
            |process| to_process_detail(process, sample_elapsed, cpu_count, &autostart_names),
        )
    } else {
        processes
            .values()
            .map(|process| to_process_detail(process, sample_elapsed, cpu_count, &autostart_names))
            .collect()
    };
    sort_process_detail_rows(&mut rows);
    rows
}

fn collect(sys: &System, sample_elapsed: Duration) -> Vec<ProcInfo> {
    let processes = sys.processes();
    let cpu_count = sys.cpus().len();
    // Keep all role labels in this response on one consistent startup snapshot.
    let autostart_names = crate::profile::autostart_snapshot();
    let mut rows: Vec<ProcInfo> = if processes.len() > PROCESS_LIST_LIMIT {
        // Rank borrowed process handles first so unreturned rows never allocate owned names or paths.
        materialize_top_by(
            processes.values().collect(),
            PROCESS_LIST_LIMIT,
            |left, right| {
                process_priority_order(
                    left.cpu_usage(),
                    left.memory() as f64 / 1024.0 / 1024.0,
                    left.pid().as_u32(),
                    right.cpu_usage(),
                    right.memory() as f64 / 1024.0 / 1024.0,
                    right.pid().as_u32(),
                )
            },
            |process| to_proc_info(process, sample_elapsed, cpu_count, &autostart_names),
        )
    } else {
        processes
            .values()
            .map(|process| to_proc_info(process, sample_elapsed, cpu_count, &autostart_names))
            .collect()
    };
    sort_proc_info_rows(&mut rows);
    rows
}

/// First request warms CPU counters; steady polls use one refresh and their actual elapsed interval.
#[tauri::command]
pub async fn list_processes() -> Result<Vec<ProcInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut sampler = process_list_sampler_state()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        sampler.get_or_insert_with(ProcessListSampler::new).sample()
    })
    .await
    .map_err(|_| "进程列表后台刷新失败，请重试。".to_string())
}

/// 只读详情快照：列表行 + 画像角色 + 创建时间，供洞察命令在阻塞线程使用。
pub(crate) fn process_details_snapshot() -> Vec<ProcessDetail> {
    let mut sampler = process_list_sampler_state()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    sampler
        .get_or_insert_with(ProcessListSampler::new)
        .sample_details()
}

const PROCESS_MUTATION_IDENTITY_ERROR: &str =
    "目标进程实例无法验证或已变化，未执行操作；请刷新进程列表或等待下一次进程采样后重试。";
const PROCESS_MUTATION_OPEN_ERROR: &str = "无法打开目标进程（可能已退出或权限不足），未执行操作。";

struct OwnedProcessHandle(HANDLE);

impl OwnedProcessHandle {
    fn as_raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for OwnedProcessHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn parse_process_creation_token(token: &str) -> Result<u64, String> {
    parse_creation_token(token).map_err(|_| PROCESS_MUTATION_IDENTITY_ERROR.to_string())
}

fn open_verified_process_handle(
    pid: u32,
    expected_creation_filetime: u64,
    desired_access: u32,
) -> Result<OwnedProcessHandle, String> {
    if expected_creation_filetime == 0 {
        return Err(PROCESS_MUTATION_IDENTITY_ERROR.into());
    }
    let raw_handle = unsafe { OpenProcess(desired_access, 0, pid) };
    if raw_handle.is_null() {
        return Err(PROCESS_MUTATION_OPEN_ERROR.into());
    }
    let handle = OwnedProcessHandle(raw_handle);
    let actual_creation_filetime = process_times(handle.as_raw()).map(|(creation, _)| creation);
    if !process_creation_identity_matches(expected_creation_filetime, actual_creation_filetime) {
        return Err(PROCESS_MUTATION_IDENTITY_ERROR.into());
    }
    Ok(handle)
}

#[tauri::command]
pub fn kill_process(pid: u32, creation_token: String) -> Result<(), String> {
    let expected_creation_filetime = parse_process_creation_token(&creation_token)?;
    let handle = open_verified_process_handle(
        pid,
        expected_creation_filetime,
        PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
    )?;
    let ok = unsafe { TerminateProcess(handle.as_raw(), 1) };
    if ok == 0 {
        Err(format!("结束进程失败：{}", std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

fn priority_class(priority: u8) -> Result<u32, String> {
    match priority {
        0 => Ok(IDLE_PRIORITY_CLASS),
        1 => Ok(BELOW_NORMAL_PRIORITY_CLASS),
        2 => Ok(NORMAL_PRIORITY_CLASS),
        3 => Ok(ABOVE_NORMAL_PRIORITY_CLASS),
        4 => Ok(HIGH_PRIORITY_CLASS),
        5 => Ok(REALTIME_PRIORITY_CLASS),
        _ => Err("无效的优先级".into()),
    }
}

fn set_priority_class_on_handle(handle: HANDLE, class: u32) -> Result<(), String> {
    let ok = unsafe { SetPriorityClass(handle, class) };
    if ok == 0 {
        Err(format!(
            "设置优先级失败：{}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

fn validate_priority_request(priority: u8, allow_realtime: bool) -> Result<(), String> {
    if priority > 5 {
        return Err("无效的优先级".into());
    }
    if priority == 5 && !allow_realtime {
        return Err("实时优先级可能导致系统服务或界面失去调度机会；必须明确确认后才能使用".into());
    }
    Ok(())
}

pub(crate) fn set_priority_verified(
    pid: u32,
    expected_creation_filetime: Option<u64>,
    priority: u8,
) -> Result<(), String> {
    let expected_creation_filetime = expected_creation_filetime
        .filter(|creation| *creation != 0)
        .ok_or_else(|| PROCESS_MUTATION_IDENTITY_ERROR.to_string())?;
    let class = priority_class(priority)?;
    let handle = open_verified_process_handle(
        pid,
        expected_creation_filetime,
        PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
    )?;
    set_priority_class_on_handle(handle.as_raw(), class)
}

/// priority: 0 空闲 / 1 低于正常 / 2 正常 / 3 高于正常 / 4 高 / 5 实时
#[tauri::command]
pub fn set_priority(
    pid: u32,
    creation_token: String,
    priority: u8,
    allow_realtime: bool,
) -> Result<(), String> {
    validate_priority_request(priority, allow_realtime)?;
    let expected_creation_filetime = parse_process_creation_token(&creation_token)?;
    set_priority_verified(pid, Some(expected_creation_filetime), priority)
}

/// 单 processor group 内的逻辑 CPU 编号分区；不推断 P/E 核。
fn affinity_mask(system: usize, kind: &str, groups: u16) -> Result<usize, String> {
    if groups != 1 {
        return Err("多 processor groups 平台暂不支持亲和性修改".into());
    }
    if system == 0 {
        return Err("系统亲和性掩码为空".into());
    }
    let half = (system.count_ones() / 2).max(1);
    let mut lower = 0usize;
    let mut selected = 0;
    for bit in 0..usize::BITS {
        let mask = 1usize << bit;
        if system & mask != 0 && selected < half {
            lower |= mask;
            selected += 1;
        }
    }
    let result = match kind {
        "all" => system,
        "low" => lower,
        "high" => system & !lower,
        _ => return Err("未知亲和性方案".into()),
    };
    if result == 0 {
        Err("选定分区没有逻辑 CPU".into())
    } else {
        Ok(result)
    }
}

#[tauri::command]
pub fn set_process_affinity(pid: u32, creation_token: String, kind: String) -> Result<(), String> {
    let expected_creation_filetime = parse_process_creation_token(&creation_token)?;
    let groups = unsafe { windows_sys::Win32::System::Threading::GetActiveProcessorGroupCount() };
    if groups != 1 {
        return Err("多 processor groups 平台暂不支持亲和性修改".into());
    }
    let handle = open_verified_process_handle(
        pid,
        expected_creation_filetime,
        PROCESS_SET_INFORMATION | PROCESS_QUERY_INFORMATION,
    )?;
    let mut process = 0usize;
    let mut system = 0usize;
    if unsafe { GetProcessAffinityMask(handle.as_raw(), &mut process, &mut system) } == 0 {
        return Err(format!(
            "读取亲和性失败: {}",
            std::io::Error::last_os_error()
        ));
    }
    let mask = affinity_mask(system, &kind, groups)?;
    if unsafe { SetProcessAffinityMask(handle.as_raw(), mask) } == 0 {
        Err(format!(
            "设置亲和性失败: {}",
            std::io::Error::last_os_error()
        ))
    } else {
        Ok(())
    }
}

type NtResumeFn = unsafe extern "system" fn(windows_sys::Win32::Foundation::HANDLE) -> i32;

const RESUME_IDENTITY_MISMATCH_ERROR: &str =
    "目标进程实例无法验证或已变化，未执行恢复；请刷新进程列表后重试。";

fn process_creation_identity_matches(expected: u64, actual: Option<u64>) -> bool {
    expected != 0 && actual == Some(expected)
}

/// 仅当当前句柄的完整创建时间与捕获令牌匹配时恢复，并在调用期间保持句柄打开。
pub fn resume_pid_raw(pid: u32, expected_creation_filetime: u64) -> Result<(), String> {
    if expected_creation_filetime == 0 {
        return Err(RESUME_IDENTITY_MISMATCH_ERROR.into());
    }
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        if ntdll.is_null() {
            return Err("加载 ntdll 失败".into());
        }
        let symbol = b"NtResumeProcess\0";
        let addr = GetProcAddress(ntdll, symbol.as_ptr());
        let func: NtResumeFn = match addr {
            Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, NtResumeFn>(f),
            None => return Err("恢复符号 NtResumeProcess 不可用".into()),
        };
        let handle = OpenProcess(
            PROCESS_SUSPEND_RESUME | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if handle.is_null() {
            return Err("无法打开进程，可能需要管理员权限".into());
        }
        let actual_creation_filetime = process_times(handle).map(|(creation, _)| creation);
        if !process_creation_identity_matches(expected_creation_filetime, actual_creation_filetime)
        {
            CloseHandle(handle);
            return Err(RESUME_IDENTITY_MISMATCH_ERROR.into());
        }
        // Keep the verified handle open through NtResumeProcess so a reused PID cannot retarget it.
        let status = func(handle);
        CloseHandle(handle);
        if status < 0 {
            Err(format!("恢复操作失败 (NTSTATUS {:#x})", status as u32))
        } else {
            Ok(())
        }
    }
}

fn deny_process_suspension() -> Result<(), String> {
    Err(PROCESS_SUSPEND_DISABLED_ERROR.into())
}

#[tauri::command]
pub fn suspend_process(
    _pid: u32,
    _state: tauri::State<'_, std::sync::Mutex<HashSet<u32>>>,
) -> Result<(), String> {
    deny_process_suspension()
}

#[tauri::command]
pub fn resume_process(
    pid: u32,
    creation_token: String,
    state: tauri::State<'_, std::sync::Mutex<HashSet<u32>>>,
) -> Result<(), String> {
    let expected_creation_filetime = parse_creation_token(&creation_token)
        .map_err(|_| "恢复请求中的进程身份信息无效，未执行恢复；请刷新列表后重试".to_string())?;
    resume_pid_raw(pid, expected_creation_filetime)?;
    state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&pid);
    Ok(())
}

pub(crate) fn normalized_process_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn query_windows_directory(query: impl FnOnce(*mut u16, u32) -> u32) -> Option<String> {
    const WINDOWS_DIRECTORY_BUFFER_LEN: usize = 32_768;
    let mut buffer = vec![0u16; WINDOWS_DIRECTORY_BUFFER_LEN];
    let length = query(buffer.as_mut_ptr(), buffer.len() as u32) as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    let path = String::from_utf16(&buffer[..length]).ok()?;
    let path = path.trim_end_matches(['\\', '/']);
    (!path.is_empty()).then(|| path.to_string())
}

/// Return the shared OS Windows directory, not a mutable per-user `WINDIR` value.
pub(crate) fn trusted_windows_directory() -> Option<&'static str> {
    static WINDOWS_DIRECTORY: OnceLock<Option<String>> = OnceLock::new();
    WINDOWS_DIRECTORY
        .get_or_init(|| {
            query_windows_directory(|buffer, length| unsafe {
                GetSystemWindowsDirectoryW(buffer, length)
            })
        })
        .as_deref()
}

pub(crate) fn normalize_windows_path(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

pub(crate) fn normalize_windows_root(windows_root: &str) -> String {
    windows_root
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_ascii_lowercase()
}

pub(crate) fn is_windows_system_path_normalized(path: &str, normalized_root: &str) -> bool {
    path.strip_prefix(normalized_root)
        .is_some_and(|relative| relative.starts_with('\\'))
}

pub(crate) fn is_windows_system_path(path: &str, windows_root: &str) -> bool {
    let path = normalize_windows_path(path);
    let root = normalize_windows_root(windows_root);
    is_windows_system_path_normalized(&path, &root)
}

pub(crate) fn is_allowed_windows_background_process_normalized(
    normalized_name: &str,
    normalized_path: &str,
    normalized_root: &str,
) -> bool {
    if !WINDOWS_BACKGROUND_CANDIDATES.contains(&normalized_name) {
        return false;
    }
    let Some(relative) = normalized_path
        .strip_prefix(normalized_root)
        .and_then(|relative| relative.strip_prefix('\\'))
    else {
        return false;
    };
    ["system32\\", "syswow64\\", "systemapps\\"]
        .iter()
        .any(|directory| relative.starts_with(directory))
}

pub(crate) fn is_allowed_windows_background_process(
    name: &str,
    path: &str,
    windows_root: &str,
) -> bool {
    let normalized_name = normalized_process_name(name);
    let normalized_path = normalize_windows_path(path);
    let normalized_root = normalize_windows_root(windows_root);
    is_allowed_windows_background_process_normalized(
        &normalized_name,
        &normalized_path,
        &normalized_root,
    )
}

fn is_child_path_ascii_case_insensitive(path: &str, directory: &str) -> bool {
    let path_bytes = path.as_bytes();
    let directory_bytes = directory.as_bytes();
    path_bytes
        .get(..directory_bytes.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(directory_bytes))
        && path_bytes.get(directory_bytes.len()) == Some(&b'\\')
}

/// 收集 AeroTune 自身进程树的 PID（广度优先沿 sysinfo 父链，从当前进程起步），
/// 同时把可执行文件位于 AeroTune 安装目录内的进程并入集合。
/// WebView2 运行时（如 msedgewebview2.exe）宿主进程在系统目录但不在 AeroTune
/// 安装目录内，单靠路径判定无法覆盖，因此需要父链 + 路径双层判定。
pub(crate) fn collect_own_process_tree_pids(sys: &System) -> HashSet<u32> {
    let self_pid = std::process::id();
    let mut tree: HashSet<u32> = HashSet::new();
    tree.insert(self_pid);
    let executable = std::env::current_exe().ok();
    let self_dir = executable
        .as_deref()
        .and_then(|exe| exe.parent())
        .and_then(|dir| dir.to_str());
    // 在同一遍进程快照中收集安装目录成员并构建父子索引。
    let mut parent_to_children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, process) in sys.processes() {
        let pid = pid.as_u32();
        if pid == self_pid {
            continue;
        }
        if let (Some(directory), Some(path)) =
            (self_dir, process.exe().and_then(|exe| exe.to_str()))
        {
            if is_child_path_ascii_case_insensitive(path, directory) {
                tree.insert(pid);
            }
        }
        // 父链未知的进程不视作自身树成员，避免误保护。
        if let Some(parent) = process.parent().map(|parent| parent.as_u32()) {
            parent_to_children.entry(parent).or_default().push(pid);
        }
    }
    // 沿父链广度优先扩展；父链和路径双层判定都保留。
    let mut frontier: Vec<u32> = vec![self_pid];
    while let Some(current) = frontier.pop() {
        let Some(children) = parent_to_children.get(&current) else {
            continue;
        };
        for child in children {
            if tree.insert(*child) {
                frontier.push(*child);
            }
        }
    }
    tree
}

fn process_protection_reason(
    name: &str,
    path: &str,
    pid: u32,
    self_pid: u32,
    foreground_pid: u32,
) -> Option<&'static str> {
    let normalized = normalized_process_name(name);
    if pid == self_pid {
        return Some("AeroTune 自身");
    }
    if pid <= 4 || PROTECTED_PROCESSES.contains(&normalized.as_str()) {
        return Some("Windows 核心/常驻进程");
    }
    let Some(windows_root) = trusted_windows_directory() else {
        return Some("无法确认 Windows 系统目录");
    };
    if is_windows_system_path(path, windows_root)
        && !is_allowed_windows_background_process(name, path, windows_root)
    {
        return Some("Windows 核心/常驻进程");
    }
    if pid == foreground_pid {
        return Some("当前前台进程");
    }
    None
}

fn memory_candidate_eligible(
    name: &str,
    path: &str,
    pid: u32,
    self_pid: u32,
    foreground_pid: u32,
    working_set: u64,
    private_bytes: u64,
    cpu: f32,
) -> Result<(), String> {
    memory_candidate_eligible_with_cpu_limit(
        name,
        path,
        pid,
        self_pid,
        foreground_pid,
        working_set,
        private_bytes,
        cpu,
        MAX_TRIM_CPU_PERCENT,
    )
}

fn memory_candidate_eligible_without_cpu_with_minimum(
    name: &str,
    path: &str,
    pid: u32,
    self_pid: u32,
    foreground_pid: u32,
    working_set: u64,
    private_bytes: u64,
    min_working_set_bytes: u64,
) -> Result<(), String> {
    if let Some(reason) = process_protection_reason(name, path, pid, self_pid, foreground_pid) {
        return Err(reason.to_string());
    }
    if working_set < min_working_set_bytes {
        let minimum_mib = min_working_set_bytes / 1024 / 1024;
        return Err(format!("工作集小于 {minimum_mib} MiB"));
    }
    if private_bytes == 0 {
        return Err("无法读取私有内存".into());
    }
    Ok(())
}

fn cpu_usage_is_eligible(cpu: f32, max_cpu_percent: f32) -> bool {
    cpu.is_finite()
        && cpu >= 0.0
        && max_cpu_percent.is_finite()
        && max_cpu_percent >= 0.0
        && cpu <= max_cpu_percent
}

fn memory_candidate_eligible_with_cpu_limit(
    name: &str,
    path: &str,
    pid: u32,
    self_pid: u32,
    foreground_pid: u32,
    working_set: u64,
    private_bytes: u64,
    cpu: f32,
    max_cpu_percent: f32,
) -> Result<(), String> {
    memory_candidate_eligible_with_limits(
        name,
        path,
        pid,
        self_pid,
        foreground_pid,
        working_set,
        private_bytes,
        cpu,
        max_cpu_percent,
        MIN_TRIM_WORKING_SET_BYTES,
    )
}

fn memory_candidate_eligible_with_limits(
    name: &str,
    path: &str,
    pid: u32,
    self_pid: u32,
    foreground_pid: u32,
    working_set: u64,
    private_bytes: u64,
    cpu: f32,
    max_cpu_percent: f32,
    min_working_set_bytes: u64,
) -> Result<(), String> {
    memory_candidate_eligible_without_cpu_with_minimum(
        name,
        path,
        pid,
        self_pid,
        foreground_pid,
        working_set,
        private_bytes,
        min_working_set_bytes,
    )?;
    if !cpu_usage_is_eligible(cpu, max_cpu_percent) {
        return Err("当前 CPU 活跃度较高或数据不可用".into());
    }
    Ok(())
}

fn foreground_process_id() -> Option<u32> {
    unsafe {
        let window = GetForegroundWindow();
        if window.is_null() {
            return None;
        }
        let mut pid = 0;
        let thread_id = GetWindowThreadProcessId(window, &mut pid);
        (thread_id != 0 && pid != 0).then_some(pid)
    }
}

const UNKNOWN_FOREGROUND_PROCESS: &str = "无法确认当前前台进程，已跳过内存整理";

fn require_foreground_process_id(pid: Option<u32>) -> Result<u32, &'static str> {
    pid.filter(|pid| *pid != 0)
        .ok_or(UNKNOWN_FOREGROUND_PROCESS)
}

fn same_process_image(expected_name: &str, expected_path: &str, actual_path: &str) -> bool {
    if !expected_path.is_empty() {
        let normalize_path = |path: &str| path.replace('/', "\\").to_ascii_lowercase();
        return normalize_path(expected_path) == normalize_path(actual_path);
    }
    std::path::Path::new(actual_path)
        .file_name()
        .map(|name| name.to_string_lossy().eq_ignore_ascii_case(expected_name))
        .unwrap_or(false)
}

const PROCESS_IMAGE_PATH_BUFFER_CHARS: usize = 32768;

fn query_process_image_with_buffer(
    h: windows_sys::Win32::Foundation::HANDLE,
    buffer: &mut Vec<u16>,
) -> Option<String> {
    if buffer.len() < PROCESS_IMAGE_PATH_BUFFER_CHARS {
        buffer.resize(PROCESS_IMAGE_PATH_BUFFER_CHARS, 0);
    }
    let mut length = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(h, 0, buffer.as_mut_ptr(), &mut length) };
    if ok == 0 || length as usize > buffer.len() {
        return None;
    }
    Some(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn query_process_image(h: windows_sys::Win32::Foundation::HANDLE) -> Option<String> {
    query_process_image_with_buffer(h, &mut Vec::new())
}

fn read_process_memory(h: windows_sys::Win32::Foundation::HANDLE) -> Option<(u64, u64)> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
        PrivateUsage: 0,
    };
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            h,
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
    };
    (ok != 0).then_some((counters.WorkingSetSize as u64, counters.PrivateUsage as u64))
}

fn selected_process_entries<'a, T>(
    processes: &'a HashMap<sysinfo::Pid, T>,
    selected_pids: &HashSet<u32>,
) -> Vec<(u32, &'a T)> {
    selected_pids
        .iter()
        .filter_map(|pid| {
            processes
                .get(&sysinfo::Pid::from_u32(*pid))
                .map(|process| (*pid, process))
        })
        .collect()
}

fn append_missing_selected_memory_candidates(
    candidates: &mut Vec<MemoryCandidate>,
    requested: &HashSet<u32>,
) {
    for pid in requested {
        if !candidates.iter().any(|candidate| candidate.pid == *pid) {
            candidates.push(MemoryCandidate {
                pid: *pid,
                name: "进程已退出或 PID 已变化".into(),
                path: String::new(),
                working_set_mb: 0.0,
                private_mb: 0.0,
                cpu: 0.0,
                eligible: false,
                reason: "进程已退出或 PID 已变化".into(),
                creation_filetime: None,
            });
        }
    }
    candidates.sort_by_key(|candidate| candidate.pid);
}

fn rejected_memory_candidate(
    pid: u32,
    name: String,
    path: String,
    reason: impl Into<String>,
    creation_filetime: Option<u64>,
) -> MemoryCandidate {
    MemoryCandidate {
        pid,
        name,
        path,
        working_set_mb: 0.0,
        private_mb: 0.0,
        cpu: 0.0,
        eligible: false,
        reason: reason.into(),
        creation_filetime,
    }
}

fn mark_candidate_report_ineligible(
    report: &mut MemoryOptimizationReport,
    pid: u32,
    reason: impl Into<String>,
) {
    if let Some(candidate) = report
        .candidates
        .iter_mut()
        .find(|candidate| candidate.pid == pid)
    {
        candidate.eligible = false;
        candidate.reason = reason.into();
    }
}

fn format_creation_token(creation_filetime: u64) -> String {
    format!("{creation_filetime:016x}")
}

fn parse_creation_token(token: &str) -> Result<u64, String> {
    if token.len() != 16 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("候选进程身份令牌格式无效，请重新分析".into());
    }
    let creation_filetime =
        u64::from_str_radix(token, 16).map_err(|_| "候选进程身份令牌格式无效，请重新分析")?;
    if creation_filetime == 0 {
        return Err("候选进程身份令牌无效，请重新分析".into());
    }
    Ok(creation_filetime)
}

fn parse_memory_candidate_identities(
    selections: Vec<MemoryCandidateIdentity>,
) -> Result<(HashSet<u32>, HashMap<u32, u64>), String> {
    if selections.len() > MAX_MEMORY_BATCH {
        return Err(format!("单次最多选择 {MAX_MEMORY_BATCH} 个进程"));
    }
    let mut pids = HashSet::with_capacity(selections.len());
    let mut creation_times = HashMap::with_capacity(selections.len());
    for selection in selections {
        if !pids.insert(selection.pid) {
            return Err("候选进程 PID 重复，请重新分析".into());
        }
        creation_times.insert(
            selection.pid,
            parse_creation_token(&selection.creation_token)?,
        );
    }
    Ok((pids, creation_times))
}

fn creation_time_matches_expected(
    pid: u32,
    actual_creation_filetime: u64,
    expected_creation_times: Option<&HashMap<u32, u64>>,
) -> bool {
    expected_creation_times
        .is_none_or(|expected| expected.get(&pid).copied() == Some(actual_creation_filetime))
}

fn inspect_selected_memory_candidates(
    selected_pids: &HashSet<u32>,
    self_pid: u32,
    foreground_pid: u32,
    min_working_set_bytes: u64,
    expected_creation_times: Option<&HashMap<u32, u64>>,
) -> (Vec<MemoryCandidate>, usize) {
    let mut candidates = Vec::with_capacity(selected_pids.len());
    for pid in selected_pids {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, *pid) };
        if handle.is_null() {
            candidates.push(rejected_memory_candidate(
                *pid,
                "进程已退出或无法访问".into(),
                String::new(),
                "无法读取进程内存信息，请重新分析",
                None,
            ));
            continue;
        }
        let Some(path) = query_process_image(handle) else {
            unsafe { CloseHandle(handle) };
            candidates.push(rejected_memory_candidate(
                *pid,
                "未知进程".into(),
                String::new(),
                "无法核实进程映像路径",
                None,
            ));
            continue;
        };
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let process_creation = process_times(handle).map(|(creation, _)| creation);
        let memory = read_process_memory(handle);
        unsafe { CloseHandle(handle) };
        if name.is_empty() {
            candidates.push(rejected_memory_candidate(
                *pid,
                "未知进程".into(),
                path,
                "无法识别进程名称",
                process_creation,
            ));
            continue;
        }
        let Some(creation_filetime) = process_creation else {
            candidates.push(rejected_memory_candidate(
                *pid,
                name,
                path,
                "无法核实进程创建时间",
                None,
            ));
            continue;
        };
        if !creation_time_matches_expected(*pid, creation_filetime, expected_creation_times) {
            candidates.push(rejected_memory_candidate(
                *pid,
                name,
                path,
                "进程实例已变化，请重新分析",
                Some(creation_filetime),
            ));
            continue;
        }
        let Some((working_set, private_bytes)) = memory else {
            candidates.push(rejected_memory_candidate(
                *pid,
                name,
                path,
                "无法读取工作集/私有内存",
                Some(creation_filetime),
            ));
            continue;
        };
        let eligibility = memory_candidate_eligible_without_cpu_with_minimum(
            &name,
            &path,
            *pid,
            self_pid,
            foreground_pid,
            working_set,
            private_bytes,
            min_working_set_bytes,
        );
        candidates.push(MemoryCandidate {
            pid: *pid,
            name,
            path,
            working_set_mb: working_set as f64 / 1024.0 / 1024.0,
            private_mb: private_bytes as f64 / 1024.0 / 1024.0,
            cpu: 0.0,
            eligible: eligibility.is_ok(),
            reason: eligibility
                .err()
                .unwrap_or_else(|| "等待新鲜 CPU 样本".into()),
            creation_filetime: Some(creation_filetime),
        });
    }
    append_missing_selected_memory_candidates(&mut candidates, selected_pids);
    let skipped = candidates
        .iter()
        .filter(|candidate| !candidate.eligible)
        .count();
    (candidates, skipped)
}

fn inspect_memory_candidates(
    sys: &System,
    foreground_pid: u32,
    eligible_pids: Option<&HashSet<u32>>,
) -> (Vec<MemoryCandidate>, usize) {
    let self_pid = std::process::id();
    let cpu_count = sys.cpus().len();
    let mut candidates = Vec::new();
    let mut omitted = 0usize;
    // 自动全盘扫描会遍历所有进程；自身进程树（含 WebView2 运行时子进程）
    // 一律视为受保护，避免清理自己拖垮 UI/后端。
    let own_tree = if eligible_pids.is_none() {
        Some(collect_own_process_tree_pids(sys))
    } else {
        None
    };
    let ordered = if let Some(selected) = eligible_pids {
        selected_process_entries(sys.processes(), selected)
    } else {
        let mut ordered = sys
            .processes()
            .iter()
            .map(|(pid, process)| (pid.as_u32(), process))
            .collect::<Vec<_>>();
        ordered.sort_by_key(|(_, process)| std::cmp::Reverse(process.memory()));
        ordered
    };

    for (pid, process) in ordered {
        let initial_working_set = process.memory();
        if eligible_pids.is_none()
            && initial_working_set < MIN_TRIM_WORKING_SET_BYTES
            && candidates.len() >= MAX_MEMORY_CANDIDATES * 2
        {
            omitted += 1;
            continue;
        }

        let name = process.name().to_string_lossy().into_owned();
        let mut path = process
            .exe()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut protection = if own_tree.as_ref().is_some_and(|tree| tree.contains(&pid)) {
            Some("AeroTune 自身进程树")
        } else {
            process_protection_reason(&name, &path, pid, self_pid, foreground_pid)
        };
        let mut working_set = initial_working_set;
        let mut private_bytes = 0;
        let mut memory_readable = false;
        let mut image_verified = false;
        let mut creation_filetime = None;

        if protection.is_none() {
            unsafe {
                let handle = OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid);
                if handle.is_null() {
                    protection = Some("无法读取进程内存信息");
                } else {
                    match query_process_image(handle) {
                        None => protection = Some("无法核实进程映像路径"),
                        Some(actual_path) => {
                            image_verified = true;
                            let system_path_protected = trusted_windows_directory().map(|root| {
                                is_windows_system_path(&actual_path, root)
                                    && !is_allowed_windows_background_process(
                                        &name,
                                        &actual_path,
                                        root,
                                    )
                            });
                            if system_path_protected.is_none() {
                                protection = Some("无法确认 Windows 系统目录");
                            } else if system_path_protected == Some(true) {
                                protection = Some("Windows 系统目录进程");
                            } else if !path.is_empty()
                                && !same_process_image(&name, &path, &actual_path)
                            {
                                protection = Some("进程映像已变化，需重新分析");
                            } else {
                                path = actual_path;
                                if let Some((current_creation_filetime, _)) = process_times(handle)
                                {
                                    creation_filetime = Some(current_creation_filetime);
                                    if let Some((current_working_set, current_private_bytes)) =
                                        read_process_memory(handle)
                                    {
                                        working_set = current_working_set;
                                        private_bytes = current_private_bytes;
                                        memory_readable = true;
                                    } else {
                                        protection = Some("无法读取工作集/私有内存");
                                    }
                                } else {
                                    protection = Some("无法核实进程创建时间");
                                }
                            }
                        }
                    }
                    CloseHandle(handle);
                }
            }
        }

        let cpu = normalize_sysinfo_cpu_usage(process.cpu_usage(), cpu_count);
        let eligibility = if let Some(reason) = protection {
            Err(reason.to_string())
        } else if !image_verified {
            Err("无法核实进程映像路径".into())
        } else if creation_filetime.is_none() {
            Err("无法核实进程创建时间".into())
        } else if !memory_readable {
            Err("无法读取工作集/私有内存".into())
        } else {
            memory_candidate_eligible(
                &name,
                &path,
                pid,
                self_pid,
                foreground_pid,
                working_set,
                private_bytes,
                cpu,
            )
        };

        if eligible_pids.is_none()
            && working_set < MIN_TRIM_WORKING_SET_BYTES
            && candidates.len() >= MAX_MEMORY_CANDIDATES
        {
            continue;
        }
        candidates.push(MemoryCandidate {
            pid,
            name,
            path,
            working_set_mb: working_set as f64 / 1024.0 / 1024.0,
            private_mb: private_bytes as f64 / 1024.0 / 1024.0,
            cpu,
            eligible: eligibility.is_ok(),
            reason: eligibility.err().unwrap_or_else(|| "空闲第三方进程".into()),
            creation_filetime,
        });
    }

    if let Some(requested) = eligible_pids {
        append_missing_selected_memory_candidates(&mut candidates, requested);
    } else {
        candidates.sort_by(|left, right| {
            right.eligible.cmp(&left.eligible).then_with(|| {
                right
                    .working_set_mb
                    .partial_cmp(&left.working_set_mb)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        candidates.truncate(MAX_MEMORY_CANDIDATES * 2);
    }

    let skipped = candidates
        .iter()
        .filter(|candidate| !candidate.eligible)
        .count()
        + omitted;
    if eligible_pids.is_none() {
        candidates.truncate(MAX_MEMORY_CANDIDATES);
    }
    (candidates, skipped)
}

fn filetime_ticks(time: FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

fn process_times(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<(u64, u64)> {
    let mut creation = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut exit = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut kernel = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut user = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    let creation_filetime = filetime_ticks(creation);
    (creation_filetime != 0).then_some((
        creation_filetime,
        filetime_ticks(kernel).saturating_add(filetime_ticks(user)),
    ))
}

const WINDOWS_UNIX_EPOCH_FILETIME_TICKS: u64 = 116_444_736_000_000_000;
const FILETIME_TICKS_PER_SECOND: u64 = 10_000_000;

fn filetime_matches_unix_start(creation_filetime: u64, start_unix: u64) -> bool {
    creation_filetime
        .checked_sub(WINDOWS_UNIX_EPOCH_FILETIME_TICKS)
        .is_some_and(|ticks| ticks / FILETIME_TICKS_PER_SECOND == start_unix)
}

fn process_row_matches_current_identity(
    expected_name: &str,
    expected_path: &str,
    actual_path: &str,
    creation_filetime: u64,
    expected_start_unix: u64,
) -> bool {
    same_process_image(expected_name, expected_path, actual_path)
        && filetime_matches_unix_start(creation_filetime, expected_start_unix)
}

pub(crate) fn process_creation_filetime_for_sample(
    pid: u32,
    expected_start_unix: u64,
    expected_name: &str,
    expected_path: &str,
    image_buffer: &mut Vec<u16>,
) -> Option<u64> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let actual_path = query_process_image_with_buffer(handle, image_buffer);
    let creation_filetime = process_times(handle).map(|(creation, _)| creation);
    unsafe { CloseHandle(handle) };
    match (actual_path, creation_filetime) {
        (Some(actual_path), Some(creation_filetime))
            if process_row_matches_current_identity(
                expected_name,
                expected_path,
                &actual_path,
                creation_filetime,
                expected_start_unix,
            ) =>
        {
            Some(creation_filetime)
        }
        _ => None,
    }
}

fn process_creation_token(
    pid: u32,
    expected_start_unix: u64,
    expected_name: &str,
    expected_path: &str,
) -> Option<String> {
    let mut image_buffer = Vec::new();
    process_creation_filetime_for_sample(
        pid,
        expected_start_unix,
        expected_name,
        expected_path,
        &mut image_buffer,
    )
    .map(format_creation_token)
}

fn system_logical_processor_count() -> Option<u32> {
    let count = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
    (count > 0).then_some(count)
}

fn process_cpu_usage_percent(
    before_cpu_ticks: u64,
    after_cpu_ticks: u64,
    elapsed: Duration,
    logical_processors: u32,
) -> Option<f32> {
    let cpu_ticks = after_cpu_ticks.checked_sub(before_cpu_ticks)?;
    let elapsed_seconds = elapsed.as_secs_f64();
    if logical_processors == 0 || !elapsed_seconds.is_finite() || elapsed_seconds <= 0.0 {
        return None;
    }
    let percent =
        cpu_ticks as f64 / 10_000_000.0 / elapsed_seconds / logical_processors as f64 * 100.0;
    if !percent.is_finite() || percent < 0.0 {
        return None;
    }
    let percent = percent as f32;
    percent.is_finite().then_some(percent.clamp(0.0, 100.0))
}

fn sample_process_cpu(pids: &HashSet<u32>) -> HashMap<u32, ProcessCpuSample> {
    let mut result = HashMap::new();
    if pids.is_empty() {
        return result;
    }
    let mut first = HashMap::with_capacity(pids.len());
    for pid in pids {
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, *pid);
            if handle.is_null() {
                continue;
            }
            if let Some((creation_filetime, cpu_ticks)) = process_times(handle) {
                first.insert(*pid, (cpu_ticks, creation_filetime, Instant::now()));
            }
            CloseHandle(handle);
        }
    }
    if first.is_empty() {
        return result;
    }
    let Some(logical_processors) = system_logical_processor_count() else {
        return result;
    };
    // Keep a full one-second interval, then normalize by each PID's measured elapsed time.
    std::thread::sleep(Duration::from_secs(1));
    for pid in pids {
        let Some((before_cpu, before_creation, sampled_at)) = first.get(pid).copied() else {
            continue;
        };
        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, *pid);
            if handle.is_null() {
                continue;
            }
            if let Some((creation_filetime, after_cpu)) = process_times(handle) {
                if creation_filetime == before_creation {
                    if let Some(usage_percent) = process_cpu_usage_percent(
                        before_cpu,
                        after_cpu,
                        sampled_at.elapsed(),
                        logical_processors,
                    ) {
                        result.insert(
                            *pid,
                            ProcessCpuSample {
                                usage_percent,
                                creation_filetime,
                            },
                        );
                    }
                }
            }
            CloseHandle(handle);
        }
    }
    result
}

fn physical_memory_status() -> Option<(u32, f64)> {
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        dwMemoryLoad: 0,
        ullTotalPhys: 0,
        ullAvailPhys: 0,
        ullTotalPageFile: 0,
        ullAvailPageFile: 0,
        ullTotalVirtual: 0,
        ullAvailVirtual: 0,
        ullAvailExtendedVirtual: 0,
    };
    let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
    (ok != 0).then_some((
        status.dwMemoryLoad,
        status.ullAvailPhys as f64 / 1024.0 / 1024.0,
    ))
}

fn validated_available_memory_mb(value: f64) -> Option<f64> {
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn available_memory_mb() -> Option<f64> {
    let (_, available) = physical_memory_status()?;
    validated_available_memory_mb(available)
}

fn should_auto_trim_memory(memory_load_pct: u32, available_mb: f64) -> bool {
    memory_load_pct >= 88
        && available_mb.is_finite()
        && (0.0..=AUTO_TRIM_AVAILABLE_LIMIT_MB).contains(&available_mb)
}

fn auto_trim_cooldown_active(previous: Option<Instant>, now: Instant, cooldown: Duration) -> bool {
    previous.is_some_and(|previous| {
        now.checked_duration_since(previous)
            .is_none_or(|elapsed| elapsed < cooldown)
    })
}

fn auto_trim_commit_success_cooldown(
    last_success: &mut Option<Instant>,
    now: Instant,
    report: Option<&MemoryOptimizationReport>,
) {
    if report.is_some_and(|report| report.processed > 0) {
        *last_success = Some(now);
    }
}

/// 只回收大体量、低活跃度的第三方进程工作集；保护前台和核心进程，并报告真实工作集变化。
pub fn optimize_process_memory_sync(
    preview_only: bool,
    eligible_pids: Option<HashSet<u32>>,
) -> Result<MemoryOptimizationReport, String> {
    optimize_process_memory_sync_limited(preview_only, eligible_pids, MAX_MEMORY_BATCH)
}

fn optimize_process_memory_sync_limited(
    preview_only: bool,
    eligible_pids: Option<HashSet<u32>>,
    max_batch: usize,
) -> Result<MemoryOptimizationReport, String> {
    optimize_process_memory_sync_limited_with_cpu_limit(
        preview_only,
        eligible_pids,
        max_batch,
        MAX_TRIM_CPU_PERCENT,
        MIN_TRIM_WORKING_SET_BYTES,
        None,
    )
}

fn optimize_process_memory_sync_limited_with_cpu_limit(
    preview_only: bool,
    eligible_pids: Option<HashSet<u32>>,
    max_batch: usize,
    max_cpu_percent: f32,
    min_working_set_bytes: u64,
    expected_creation_times: Option<HashMap<u32, u64>>,
) -> Result<MemoryOptimizationReport, String> {
    let foreground_pid =
        require_foreground_process_id(foreground_process_id()).map_err(str::to_string)?;
    optimize_process_memory_sync_limited_with_cpu_limit_for_foreground(
        preview_only,
        eligible_pids,
        max_batch,
        max_cpu_percent,
        min_working_set_bytes,
        expected_creation_times,
        foreground_pid,
    )
}

fn optimize_process_memory_sync_limited_with_cpu_limit_for_foreground(
    preview_only: bool,
    eligible_pids: Option<HashSet<u32>>,
    max_batch: usize,
    max_cpu_percent: f32,
    min_working_set_bytes: u64,
    expected_creation_times: Option<HashMap<u32, u64>>,
    foreground_pid: u32,
) -> Result<MemoryOptimizationReport, String> {
    let foreground_pid =
        require_foreground_process_id(Some(foreground_pid)).map_err(str::to_string)?;
    let max_batch = max_batch.clamp(1, MAX_MEMORY_BATCH);
    if let Some(pids) = eligible_pids.as_ref() {
        validate_requested_pids(pids, std::process::id(), foreground_pid)?;
    }
    let self_pid = std::process::id();
    let (before, mut candidates, skipped) = if !preview_only {
        if let Some(selected_pids) = eligible_pids.as_ref() {
            let before =
                available_memory_mb().ok_or_else(|| "无法读取当前物理可用内存".to_string())?;
            let (candidates, skipped) = inspect_selected_memory_candidates(
                selected_pids,
                self_pid,
                foreground_pid,
                min_working_set_bytes,
                expected_creation_times.as_ref(),
            );
            (before, candidates, skipped)
        } else {
            let mut sys = System::new();
            prepare_memory_scan(&mut sys);
            let before =
                available_memory_mb().ok_or_else(|| "无法读取当前物理可用内存".to_string())?;
            let (candidates, skipped) =
                inspect_memory_candidates(&sys, foreground_pid, eligible_pids.as_ref());
            (before, candidates, skipped)
        }
    } else {
        let mut sys = System::new();
        prepare_memory_scan(&mut sys);
        let before = available_memory_mb().ok_or_else(|| "无法读取当前物理可用内存".to_string())?;
        let (candidates, skipped) =
            inspect_memory_candidates(&sys, foreground_pid, eligible_pids.as_ref());
        (before, candidates, skipped)
    };
    let mut skipped = skipped;
    let mut report = MemoryOptimizationReport {
        available_before_mb: before,
        available_after_mb: Some(before),
        candidates: candidates.clone(),
        skipped,
        ..Default::default()
    };
    if preview_only {
        return Ok(report);
    }
    let cpu_sample_pids = candidates
        .iter()
        .filter(|candidate| candidate.eligible)
        .take(max_batch)
        .map(|candidate| candidate.pid)
        .collect::<HashSet<_>>();
    let current_cpu_by_pid = sample_process_cpu(&cpu_sample_pids);
    skipped +=
        apply_cpu_samples_to_candidates(&mut candidates, &current_cpu_by_pid, max_cpu_percent);
    report.candidates = candidates.clone();
    report.skipped = skipped;
    for candidate in candidates.iter().filter(|candidate| candidate.eligible) {
        if report.processed >= max_batch {
            break;
        }
        unsafe {
            let handle = OpenProcess(
                PROCESS_SET_QUOTA | PROCESS_QUERY_INFORMATION,
                0,
                candidate.pid,
            );
            if handle.is_null() {
                skipped += 1;
                mark_candidate_report_ineligible(
                    &mut report,
                    candidate.pid,
                    "进程已退出或无法访问",
                );
                continue;
            }
            let Some((before_ws, private_bytes)) = read_process_memory(handle) else {
                CloseHandle(handle);
                skipped += 1;
                mark_candidate_report_ineligible(
                    &mut report,
                    candidate.pid,
                    "无法读取工作集/私有内存",
                );
                continue;
            };
            let current_image = query_process_image(handle);
            let current_foreground = foreground_process_id();
            let foreground_error = require_foreground_process_id(current_foreground)
                .err()
                .map(str::to_string);
            let current_foreground_pid = current_foreground.unwrap_or(0);
            let current_cpu_sample = current_cpu_by_pid.get(&candidate.pid).copied();
            let current_cpu = current_cpu_sample.map(|sample| sample.usage_percent);
            let current_creation_filetime = process_times(handle).map(|(creation, _)| creation);
            let verified_instance = candidate.creation_filetime.is_some_and(|creation| {
                current_creation_filetime == Some(creation)
                    && current_cpu_sample.is_some_and(|sample| sample.creation_filetime == creation)
            });
            let verified_image = current_image.as_ref().is_some_and(|image| {
                trusted_windows_directory().is_some_and(|windows_root| {
                    same_process_image(&candidate.name, &candidate.path, image)
                        && (!is_windows_system_path(image, windows_root)
                            || is_allowed_windows_background_process(
                                &candidate.name,
                                image,
                                windows_root,
                            ))
                })
            });
            let rejection_reason = foreground_error
                .or_else(|| {
                    process_protection_reason(
                        &candidate.name,
                        &candidate.path,
                        candidate.pid,
                        self_pid,
                        current_foreground_pid,
                    )
                    .map(str::to_string)
                })
                .or_else(|| (!verified_image).then(|| "进程映像已变化或无法核实".to_string()))
                .or_else(|| {
                    (!verified_instance).then(|| "进程实例已变化或 CPU 样本不可用".to_string())
                })
                .or_else(|| {
                    memory_candidate_eligible_with_limits(
                        &candidate.name,
                        &candidate.path,
                        candidate.pid,
                        self_pid,
                        current_foreground_pid,
                        before_ws,
                        private_bytes,
                        current_cpu.unwrap_or(f32::INFINITY),
                        max_cpu_percent,
                        min_working_set_bytes,
                    )
                    .err()
                });
            if let Some(reason) = rejection_reason {
                CloseHandle(handle);
                skipped += 1;
                mark_candidate_report_ineligible(&mut report, candidate.pid, reason);
                continue;
            }
            let trimmed = EmptyWorkingSet(handle) != 0;
            let after_ws = if trimmed {
                read_process_memory(handle).map(|(working_set, _)| working_set)
            } else {
                None
            };
            CloseHandle(handle);
            if let Some(after_ws) = after_ws {
                report.processed += 1;
                report.touched_working_set_mb += before_ws as f64 / 1024.0 / 1024.0;
                report.estimated_reclaimed_mb +=
                    before_ws.saturating_sub(after_ws) as f64 / 1024.0 / 1024.0;
                mark_candidate_report_ineligible(&mut report, candidate.pid, "本次操作已完成");
            } else {
                skipped += 1;
                mark_candidate_report_ineligible(
                    &mut report,
                    candidate.pid,
                    "工作集调整未确认成功",
                );
            }
        }
    }
    report.skipped = skipped;
    if should_wait_for_memory_reclamation(report.estimated_reclaimed_mb) {
        post_trim_measurement_delay();
    }
    report.available_after_mb = available_memory_mb();
    Ok(report)
}

/// Under sustained high memory pressure, reclaim at most two large, idle background working sets.
pub fn auto_trim_background_memory_if_needed() -> Result<Option<MemoryOptimizationReport>, String> {
    if AUTO_TRIM_RUNNING.swap(true, Ordering::AcqRel) {
        return Ok(None);
    }
    let result = (|| {
        let Some((memory_load_pct, available_mb)) = physical_memory_status() else {
            return Err("无法读取 Windows 物理内存状态".into());
        };
        if !should_auto_trim_memory(memory_load_pct, available_mb) {
            return Ok(None);
        }
        let now = Instant::now();
        let action_cooldown = Duration::from_secs(AUTO_TRIM_COOLDOWN_SECS);
        let last_success = AUTO_TRIM_LAST_SUCCESS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if auto_trim_cooldown_active(*last_success, now, action_cooldown) {
            return Ok(None);
        }
        drop(last_success);
        let mut last_scan_attempt = AUTO_TRIM_LAST_SCAN_ATTEMPT
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if auto_trim_cooldown_active(
            *last_scan_attempt,
            now,
            Duration::from_secs(AUTO_TRIM_SCAN_RETRY_SECS),
        ) {
            return Ok(None);
        }
        *last_scan_attempt = Some(now);
        drop(last_scan_attempt);
        let preview = optimize_process_memory_sync(true, None)?;
        let expected_creation_times = preview
            .candidates
            .iter()
            .filter(|candidate| {
                candidate.eligible
                    && candidate.working_set_mb >= AUTO_TRIM_WORKING_SET_MB
                    && candidate.cpu <= AUTO_TRIM_CPU_PERCENT
            })
            .filter_map(|candidate| {
                candidate
                    .creation_filetime
                    .map(|creation| (candidate.pid, creation))
            })
            .take(AUTO_TRIM_MAX_BATCH)
            .collect::<HashMap<_, _>>();
        let eligible_pids = expected_creation_times
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        if eligible_pids.is_empty() {
            return Ok(None);
        }
        let report = optimize_process_memory_sync_limited_with_cpu_limit(
            false,
            Some(eligible_pids),
            AUTO_TRIM_MAX_BATCH,
            AUTO_TRIM_CPU_PERCENT,
            AUTO_TRIM_WORKING_SET_BYTES,
            Some(expected_creation_times),
        )?;
        let mut last_success = AUTO_TRIM_LAST_SUCCESS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        auto_trim_commit_success_cooldown(&mut last_success, Instant::now(), Some(&report));
        Ok(Some(report))
    })();
    AUTO_TRIM_RUNNING.store(false, Ordering::Release);
    result
}

#[tauri::command]
pub async fn preview_process_memory_optimization(
    pids: Option<Vec<u32>>,
) -> Result<MemoryOptimizationReport, String> {
    let eligible_pids = pids.map(|values| values.into_iter().collect());
    tauri::async_runtime::spawn_blocking(move || optimize_process_memory_sync(true, eligible_pids))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn optimize_process_memory(
    pids: Vec<MemoryCandidateIdentity>,
) -> Result<MemoryOptimizationReport, String> {
    let (eligible_pids, expected_creation_times) = parse_memory_candidate_identities(pids)?;
    let foreground_pid =
        require_foreground_process_id(foreground_process_id()).map_err(str::to_string)?;
    validate_requested_pids(&eligible_pids, std::process::id(), foreground_pid)?;
    tauri::async_runtime::spawn_blocking(move || {
        optimize_process_memory_sync_limited_with_cpu_limit(
            false,
            Some(eligible_pids),
            MAX_MEMORY_BATCH,
            MAX_TRIM_CPU_PERCENT,
            MIN_TRIM_WORKING_SET_BYTES,
            Some(expected_creation_times),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

fn validate_requested_pids(
    pids: &HashSet<u32>,
    self_pid: u32,
    foreground_pid: u32,
) -> Result<(), String> {
    if pids.is_empty() {
        return Err("至少选择一个进程 PID".into());
    }
    if pids.len() > MAX_MEMORY_BATCH {
        return Err(format!("单次最多选择 {MAX_MEMORY_BATCH} 个进程"));
    }
    if pids.contains(&self_pid) || pids.iter().any(|pid| *pid <= 4) {
        return Err("选择中含有系统保留 PID".into());
    }
    if pids.contains(&foreground_pid) {
        return Err("当前前台进程已保护，请重新分析候选列表".into());
    }
    Ok(())
}

/// 兼容旧入口：单次扫描并返回实际观测的工作集回收量。
pub fn trim_memory_sync() -> Result<(usize, f64), String> {
    let report = optimize_process_memory_sync(false, None)?;
    Ok((report.processed, report.estimated_reclaimed_mb))
}

#[tauri::command]
pub async fn trim_memory() -> Result<(usize, f64), String> {
    tauri::async_runtime::spawn_blocking(trim_memory_sync)
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TrimOutcome {
    Trimmed { n: usize, mb: f64 },
    Skipped { reason: String },
}

/// 智能内存整理：结合 EWMA 水位与回归斜率决定是否出手，避免无意义整理。
#[tauri::command]
pub async fn smart_trim(
    diag: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::diag::Engine>>>,
) -> Result<TrimOutcome, String> {
    let decision = diag.lock().unwrap().trim_needed();
    match decision {
        Err(reason) => Ok(TrimOutcome::Skipped {
            reason: reason.to_string(),
        }),
        Ok(()) => {
            let (n, mb) = tauri::async_runtime::spawn_blocking(trim_memory_sync)
                .await
                .map_err(|e| e.to_string())??;
            Ok(TrimOutcome::Trimmed { n, mb })
        }
    }
}

fn prepare_memory_scan(sys: &mut System) {
    let refresh = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_exe(UpdateKind::OnlyIfNotSet);
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh);
    std::thread::sleep(Duration::from_millis(1000));
    sys.refresh_processes_specifics(ProcessesToUpdate::All, false, refresh);
}

fn post_trim_measurement_delay() {
    std::thread::sleep(Duration::from_millis(1000));
}

fn should_wait_for_memory_reclamation(reclaimed_mb: f64) -> bool {
    reclaimed_mb.is_finite() && reclaimed_mb > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_process_suspension_fails_closed_with_recovery_guidance() {
        let error = deny_process_suspension().unwrap_err();
        assert!(error.contains("挂起暂时关闭"));
        assert!(error.contains("恢复入口仍可使用"));
    }

    #[test]
    fn process_identity_must_match_the_exact_nonzero_creation_time() {
        let expected = 0x01dc_1234_5678_9abcu64;
        assert!(process_creation_identity_matches(expected, Some(expected)));
        assert!(!process_creation_identity_matches(
            expected,
            Some(expected + 1)
        ));
        assert!(!process_creation_identity_matches(expected, None));
        assert!(!process_creation_identity_matches(0, Some(0)));
    }

    #[test]
    fn verified_priority_requires_sampled_identity_before_opening() {
        assert_eq!(
            set_priority_verified(0, None, 1).unwrap_err(),
            PROCESS_MUTATION_IDENTITY_ERROR
        );
        assert_eq!(
            set_priority_verified(0, Some(0), 1).unwrap_err(),
            PROCESS_MUTATION_IDENTITY_ERROR
        );
    }

    #[test]
    fn process_list_identity_crosscheck_uses_path_and_second_resolution_start_time() {
        let start_unix = 42;
        let start_of_second =
            WINDOWS_UNIX_EPOCH_FILETIME_TICKS + start_unix * FILETIME_TICKS_PER_SECOND;
        assert!(process_row_matches_current_identity(
            "worker.exe",
            r"C:\Apps\worker.exe",
            r"c:/apps/WORKER.exe",
            start_of_second,
            start_unix,
        ));
        assert!(!process_row_matches_current_identity(
            "worker.exe",
            r"C:\Apps\worker.exe",
            r"C:\Apps\different.exe",
            start_of_second,
            start_unix,
        ));
        assert!(!process_row_matches_current_identity(
            "worker.exe",
            r"C:\Apps\worker.exe",
            r"C:\Apps\worker.exe",
            start_of_second + FILETIME_TICKS_PER_SECOND,
            start_unix,
        ));
        // A one-sided sysinfo cross-check is still ambiguous; the list sampler must bracket the refresh.
        assert!(filetime_matches_unix_start(
            start_of_second + FILETIME_TICKS_PER_SECOND - 1,
            start_unix
        ));
        assert!(!filetime_matches_unix_start(0, start_unix));
    }

    fn synthetic_process_info(pid: u32) -> ProcInfo {
        ProcInfo {
            pid,
            creation_token: None,
            parent_pid: None,
            name: format!("process-{pid}.exe"),
            cpu: match pid % 9 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                remainder => remainder as f32 * 7.25,
            },
            mem_mb: if pid.is_multiple_of(13) {
                f64::NAN
            } else {
                ((pid * 31) % 1000) as f64
            },
            io_mb_s: 0.0,
            path: String::new(),
            role: "user-app".into(),
            role_label: "用户应用".into(),
        }
    }

    #[test]
    fn process_row_tokens_are_kept_only_when_filetime_matches_across_refresh() {
        let first_filetime = 0x01dc_1234_5678_9abcu64;
        let changed_filetime = first_filetime + 1;
        let mut stable = synthetic_process_info(1);
        stable.creation_token = Some(format_creation_token(first_filetime));
        let mut changed = synthetic_process_info(2);
        changed.creation_token = Some(format_creation_token(changed_filetime));
        let mut new_to_top = synthetic_process_info(3);
        new_to_top.creation_token = Some(format_creation_token(first_filetime));
        let unverified = synthetic_process_info(4);
        let mut rows = vec![stable, changed, new_to_top, unverified];
        let before_refresh = HashMap::from([
            (1, first_filetime),
            (2, first_filetime),
            (4, changed_filetime),
        ]);
        let previous_filetimes = HashMap::from([(1, first_filetime), (2, first_filetime)]);
        let current_pre_refresh = HashMap::from([(1, first_filetime), (2, changed_filetime)]);
        assert_eq!(
            unchanged_process_filetimes(&previous_filetimes, &current_pre_refresh),
            HashMap::from([(1, first_filetime)])
        );

        let next_filetimes = mask_unstable_process_row_tokens(&mut rows, &before_refresh);

        let expected_token = format_creation_token(first_filetime);
        assert_eq!(
            rows[0].creation_token.as_deref(),
            Some(expected_token.as_str())
        );
        assert!(
            rows[1].creation_token.is_none(),
            "changed identity must fail closed"
        );
        assert!(
            rows[2].creation_token.is_none(),
            "newly selected rows need a stable refresh first"
        );
        assert!(
            rows[3].creation_token.is_none(),
            "unavailable post-refresh identity remains read-only"
        );
        assert_eq!(
            next_filetimes,
            HashMap::from([
                (1, first_filetime),
                (2, changed_filetime),
                (3, first_filetime)
            ])
        );
    }

    #[test]
    fn process_row_identity_change_stays_read_only_until_followup_sample_is_stable() {
        let pid = 77;
        let prior_filetime = 0x01dc_1234_5678_9abcu64;
        let replacement_filetime = prior_filetime + 1;
        let prior_rows = HashMap::from([(pid, prior_filetime)]);
        let replacement_observed_before = HashMap::from([(pid, replacement_filetime)]);
        let stable_before = unchanged_process_filetimes(&prior_rows, &replacement_observed_before);
        assert!(
            stable_before.is_empty(),
            "pre-refresh replacement must fail closed"
        );

        let replacement_token = format_creation_token(replacement_filetime);
        let mut first_replacement_row = synthetic_process_info(pid);
        first_replacement_row.creation_token = Some(replacement_token.clone());
        let mut first_rows = [first_replacement_row];
        let cached_replacement = mask_unstable_process_row_tokens(&mut first_rows, &stable_before);
        assert!(
            first_rows[0].creation_token.is_none(),
            "a changed process instance remains read-only for the transition sample"
        );
        assert_eq!(
            cached_replacement,
            HashMap::from([(pid, replacement_filetime)])
        );

        let next_pre_refresh = HashMap::from([(pid, replacement_filetime)]);
        let stable_next_pre = unchanged_process_filetimes(&cached_replacement, &next_pre_refresh);
        assert_eq!(stable_next_pre, next_pre_refresh);
        let mut stable_followup_row = synthetic_process_info(pid);
        stable_followup_row.creation_token = Some(replacement_token.clone());
        let mut followup_rows = [stable_followup_row];
        let after_followup = mask_unstable_process_row_tokens(&mut followup_rows, &stable_next_pre);
        assert_eq!(
            followup_rows[0].creation_token.as_deref(),
            Some(replacement_token.as_str()),
            "the replacement becomes actionable only after a stable follow-up sample"
        );
        assert_eq!(after_followup, HashMap::from([(pid, replacement_filetime)]));
    }

    fn synthetic_process_detail(pid: u32) -> ProcessDetail {
        ProcessDetail {
            info: synthetic_process_info(pid),
            role: crate::profile::ProcessRole {
                id: "user-app",
                label: "用户应用",
                detail: "synthetic row".into(),
            },
            start_unix: Some(pid as u64),
        }
    }

    fn reference_own_process_tree_pids_two_pass(sys: &System) -> HashSet<u32> {
        let self_pid = std::process::id();
        let mut tree = HashSet::from([self_pid]);
        if let Ok(exe) = std::env::current_exe() {
            if let Some(self_dir) = exe.parent().and_then(|dir| dir.to_str()) {
                for (pid, process) in sys.processes() {
                    let pid = pid.as_u32();
                    if pid == self_pid {
                        continue;
                    }
                    if let Some(path) = process.exe().and_then(|exe| exe.to_str()) {
                        if is_child_path_ascii_case_insensitive(path, self_dir) {
                            tree.insert(pid);
                        }
                    }
                }
            }
        }
        let mut parent_to_children: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, process) in sys.processes() {
            let pid = pid.as_u32();
            if pid == self_pid {
                continue;
            }
            if let Some(parent) = process.parent().map(|parent| parent.as_u32()) {
                parent_to_children.entry(parent).or_default().push(pid);
            }
        }
        let mut frontier = vec![self_pid];
        while let Some(current) = frontier.pop() {
            if let Some(children) = parent_to_children.get(&current) {
                for child in children {
                    if tree.insert(*child) {
                        frontier.push(*child);
                    }
                }
            }
        }
        tree
    }

    fn refreshed_process_snapshot() -> System {
        let mut sys = System::new();
        let process_refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh);
        assert!(
            !sys.processes().is_empty(),
            "the process snapshot should be available"
        );
        sys
    }

    #[test]
    fn top_process_selection_keeps_best_rows_and_stabilizes_equal_metrics() {
        let candidates = [
            (6u32, 40.0f32, 10.0f64),
            (3, 70.0, 5.0),
            (2, 70.0, 5.0),
            (8, 70.0, 7.0),
            (1, 20.0, 100.0),
        ];
        let compare = |left: &(u32, f32, f64), right: &(u32, f32, f64)| {
            process_priority_order(left.1, left.2, left.0, right.1, right.2, right.0)
        };
        let mut materialized = 0;

        let mut selected =
            materialize_top_by(candidates.iter().collect(), 3, compare, |candidate| {
                materialized += 1;
                *candidate
            });
        selected.sort_by(compare);

        assert_eq!(materialized, 3);
        assert_eq!(
            selected,
            vec![(8, 70.0, 7.0), (2, 70.0, 5.0), (3, 70.0, 5.0)]
        );
    }

    #[test]
    fn unstable_process_row_sort_matches_stable_pid_total_order() {
        let row_ids = (1..=513).rev().collect::<Vec<_>>();
        let mut stable_rows = row_ids
            .iter()
            .copied()
            .map(synthetic_process_info)
            .collect::<Vec<_>>();
        let mut unstable_rows = row_ids
            .iter()
            .copied()
            .map(synthetic_process_info)
            .collect::<Vec<_>>();
        stable_rows.sort_by(compare_proc_infos);
        sort_proc_info_rows(&mut unstable_rows);
        assert_eq!(
            stable_rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
            unstable_rows.iter().map(|row| row.pid).collect::<Vec<_>>()
        );

        let mut stable_details = row_ids
            .iter()
            .copied()
            .map(synthetic_process_detail)
            .collect::<Vec<_>>();
        let mut unstable_details = row_ids
            .iter()
            .copied()
            .map(synthetic_process_detail)
            .collect::<Vec<_>>();
        stable_details.sort_by(|left, right| compare_proc_infos(&left.info, &right.info));
        sort_process_detail_rows(&mut unstable_details);
        assert_eq!(
            stable_details
                .iter()
                .map(|row| row.info.pid)
                .collect::<Vec<_>>(),
            unstable_details
                .iter()
                .map(|row| row.info.pid)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "manual Release benchmark for stable and unstable process-row sorting"]
    fn benchmark_process_list_sort_stable_vs_unstable() {
        const ROWS: u32 = 500;
        const BATCHES: usize = 101;
        let base_rows = (1..=ROWS)
            .rev()
            .map(synthetic_process_info)
            .collect::<Vec<_>>();
        let measure_stable = || {
            let mut rows = base_rows.clone();
            let started = Instant::now();
            rows.sort_by(compare_proc_infos);
            std::hint::black_box(rows);
            started.elapsed().as_nanos()
        };
        let measure_unstable = || {
            let mut rows = base_rows.clone();
            let started = Instant::now();
            sort_proc_info_rows(&mut rows);
            std::hint::black_box(rows);
            started.elapsed().as_nanos()
        };
        for _ in 0..7 {
            std::hint::black_box(measure_stable());
            std::hint::black_box(measure_unstable());
        }
        let mut stable_nanos = Vec::with_capacity(BATCHES);
        let mut unstable_nanos = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                stable_nanos.push(measure_stable());
                unstable_nanos.push(measure_unstable());
            } else {
                unstable_nanos.push(measure_unstable());
                stable_nanos.push(measure_stable());
            }
        }
        stable_nanos.sort_unstable();
        unstable_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "process list sort ({} rows): stable median/p95={:.2}/{:.2} us; unstable in-place={:.2}/{:.2} us",
            ROWS,
            stable_nanos[BATCHES / 2] as f64 / 1_000.0,
            stable_nanos[p95_index] as f64 / 1_000.0,
            unstable_nanos[BATCHES / 2] as f64 / 1_000.0,
            unstable_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    #[ignore = "manual synthetic process-list top-500 allocation benchmark"]
    fn benchmark_top_500_selection_against_full_materialization() {
        #[derive(Clone)]
        struct Candidate {
            pid: u32,
            cpu: f32,
            mem_mb: f64,
            name: String,
            path: String,
        }

        let candidates = (1..=20_000u32)
            .map(|pid| Candidate {
                pid,
                cpu: ((pid * 97) % 10_000) as f32 / 100.0,
                mem_mb: ((pid * 31) % 1_000_000) as f64 / 1024.0,
                name: format!("worker-{pid}"),
                path: format!(r"C:\Synthetic\worker-{pid}.exe"),
            })
            .collect::<Vec<_>>();
        let compare_candidate = |left: &Candidate, right: &Candidate| {
            process_priority_order(
                left.cpu,
                left.mem_mb,
                left.pid,
                right.cpu,
                right.mem_mb,
                right.pid,
            )
        };
        let compare_info = |left: &ProcInfo, right: &ProcInfo| {
            process_priority_order(
                left.cpu,
                left.mem_mb,
                left.pid,
                right.cpu,
                right.mem_mb,
                right.pid,
            )
        };
        let mut full_times = Vec::new();
        let mut selected_times = Vec::new();
        let (mut full_materialized, mut selected_materialized) = (0usize, 0usize);

        for _ in 0..7 {
            let started = Instant::now();
            let mut full_rows = candidates
                .iter()
                .map(|candidate| ProcInfo {
                    pid: candidate.pid,
                    creation_token: None,
                    parent_pid: None,
                    name: candidate.name.clone(),
                    cpu: candidate.cpu,
                    mem_mb: candidate.mem_mb,
                    io_mb_s: 0.0,
                    path: candidate.path.clone(),
                    role: "unknown".into(),
                    role_label: "未知".into(),
                })
                .collect::<Vec<_>>();
            full_rows.sort_by(compare_info);
            full_rows.truncate(PROCESS_LIST_LIMIT);
            full_times.push(started.elapsed());
            full_materialized += candidates.len();
            let expected_pids = full_rows.iter().map(|row| row.pid).collect::<Vec<_>>();

            let started = Instant::now();
            let mut selected_rows = materialize_top_by(
                candidates.iter().collect(),
                PROCESS_LIST_LIMIT,
                compare_candidate,
                |candidate| ProcInfo {
                    pid: candidate.pid,
                    creation_token: None,
                    parent_pid: None,
                    name: candidate.name.clone(),
                    cpu: candidate.cpu,
                    mem_mb: candidate.mem_mb,
                    io_mb_s: 0.0,
                    path: candidate.path.clone(),
                    role: "unknown".into(),
                    role_label: "未知".into(),
                },
            );
            selected_rows.sort_by(compare_info);
            selected_times.push(started.elapsed());
            selected_materialized += selected_rows.len();
            assert_eq!(
                selected_rows.iter().map(|row| row.pid).collect::<Vec<_>>(),
                expected_pids
            );
        }

        assert_eq!(full_materialized, 7 * candidates.len());
        assert_eq!(selected_materialized, 7 * PROCESS_LIST_LIMIT);
        full_times.sort_unstable();
        selected_times.sort_unstable();
        let full_median = full_times[full_times.len() / 2];
        let selected_median = selected_times[selected_times.len() / 2];
        assert!(
            selected_median < full_median,
            "top-500 selection should remain cheaper than materializing every process DTO"
        );
        eprintln!(
            "synthetic top-500 benchmark: inventory={}, full median={}us, select/materialize median={}us, DTO rows {} -> {} per pass",
            candidates.len(),
            full_median.as_micros(),
            selected_median.as_micros(),
            full_materialized / 7,
            selected_materialized / 7
        );
    }

    #[test]
    fn process_list_sampler_warms_once_rewarms_after_idle_and_tracks_real_elapsed_time() {
        let now = Instant::now();
        assert_eq!(
            process_list_sample_plan(None, now),
            ProcessListSamplePlan::Warmup
        );
        assert_eq!(
            process_list_sample_plan(Some(now), now + Duration::from_secs(5)),
            ProcessListSamplePlan::Steady(now)
        );
        assert_eq!(
            process_list_sample_plan(Some(now), now + PROCESS_LIST_REWARM_AFTER),
            ProcessListSamplePlan::Warmup
        );
        assert!(!process_list_sampler_idle(
            Some(now),
            now + Duration::from_secs(59)
        ));
        assert!(process_list_sampler_idle(
            Some(now),
            now + PROCESS_LIST_IDLE_RESET_AFTER
        ));
        assert_eq!(
            process_list_sample_elapsed(now, now + Duration::from_secs(8)),
            Duration::from_secs(8)
        );
        assert_eq!(
            process_list_sample_elapsed(now, now),
            Duration::from_millis(1)
        );
    }

    fn legacy_process_list_sample() -> Vec<ProcInfo> {
        let mut system = System::new();
        let process_refresh = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_disk_usage();
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh);
        let first_sample = Instant::now();
        std::thread::sleep(PROCESS_LIST_WARMUP_INTERVAL);
        system.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh);
        collect(&system, first_sample.elapsed())
    }

    #[test]
    #[ignore = "manual Windows process-list sampling benchmark"]
    fn benchmark_process_list_steady_sampler_against_two_scan_baseline() {
        let mut baseline = Vec::new();
        for _ in 0..2 {
            let started = Instant::now();
            let rows = legacy_process_list_sample();
            assert!(
                !rows.is_empty(),
                "Windows process inventory should be readable"
            );
            baseline.push(started.elapsed());
        }

        let mut sampler = ProcessListSampler::new();
        let warmup_started = Instant::now();
        let warmup_rows = sampler.sample();
        assert!(
            !warmup_rows.is_empty(),
            "warmup process inventory should be readable"
        );
        let warmup_elapsed = warmup_started.elapsed();
        let mut steady = Vec::new();
        for _ in 0..5 {
            let started = Instant::now();
            let rows = sampler.sample();
            assert!(
                !rows.is_empty(),
                "steady process inventory should be readable"
            );
            steady.push(started.elapsed());
        }
        baseline.sort_unstable();
        steady.sort_unstable();
        let baseline_median = baseline[baseline.len() / 2];
        let steady_median = steady[steady.len() / 2];
        assert!(
            steady_median < baseline_median,
            "steady one-refresh sampling should remain cheaper than the two-scan baseline"
        );
        eprintln!(
            "process-list benchmark: baseline two-scan median={}ms, warmup={}ms, steady one-scan median={}ms, processes={}",
            baseline_median.as_millis(),
            warmup_elapsed.as_millis(),
            steady_median.as_millis(),
            warmup_rows.len()
        );
    }

    #[test]
    fn realtime_priority_requires_explicit_opt_in() {
        assert!(validate_priority_request(2, false).is_ok());
        assert!(validate_priority_request(5, false).is_err());
        assert!(validate_priority_request(5, true).is_ok());
        assert!(validate_priority_request(6, true).is_err());
    }

    #[test]
    fn protects_core_foreground_and_application_processes() {
        let windows_root = trusted_windows_directory().expect("Windows directory API succeeds");
        let vendor_system_path = format!("{windows_root}\\System32\\vendor.exe");
        let indexer_system_path = format!("{windows_root}\\System32\\SearchIndexer.exe");
        let indexer_temp_path = format!("{windows_root}\\Temp\\SearchIndexer.exe");
        assert_eq!(
            process_protection_reason("csrss.exe", "", 900, 1234, 5678),
            Some("Windows 核心/常驻进程")
        );
        assert_eq!(
            process_protection_reason("chrome.exe", "", 1234, 9999, 1234),
            Some("当前前台进程")
        );
        assert_eq!(
            process_protection_reason("aerotune.exe", "", 1234, 1234, 5678),
            Some("AeroTune 自身")
        );
        assert_eq!(
            process_protection_reason("chrome.exe", "", 4321, 9999, 1234),
            None
        );
        assert_eq!(
            process_protection_reason("vendor.exe", &vendor_system_path, 4321, 9999, 1234),
            Some("Windows 核心/常驻进程")
        );
        assert_eq!(
            process_protection_reason("SearchIndexer.exe", &indexer_system_path, 4321, 9999, 1234),
            None
        );
        assert_eq!(
            process_protection_reason("SearchIndexer.exe", &indexer_temp_path, 4321, 9999, 1234),
            Some("Windows 核心/常驻进程")
        );
        assert!(same_process_image(
            "chrome.exe",
            r"C:\Apps\Chrome\chrome.exe",
            r"c:/apps/chrome/CHROME.exe"
        ));
        assert!(!same_process_image(
            "chrome.exe",
            r"C:\Apps\Chrome\chrome.exe",
            r"C:\Windows\System32\svchost.exe"
        ));
    }

    #[test]
    fn own_tree_single_pass_matches_two_pass_reference() {
        let sys = refreshed_process_snapshot();
        assert_eq!(
            collect_own_process_tree_pids(&sys),
            reference_own_process_tree_pids_two_pass(&sys)
        );
    }

    #[test]
    #[ignore = "manual Windows benchmark for one-pass own-tree collection"]
    fn benchmark_own_process_tree_single_pass_vs_two_pass_reference() {
        const BATCHES: usize = 101;
        let sys = refreshed_process_snapshot();
        let expected = reference_own_process_tree_pids_two_pass(&sys);
        assert_eq!(collect_own_process_tree_pids(&sys), expected);
        for _ in 0..7 {
            std::hint::black_box(reference_own_process_tree_pids_two_pass(&sys));
            std::hint::black_box(collect_own_process_tree_pids(&sys));
        }
        let mut two_pass_nanos = Vec::with_capacity(BATCHES);
        let mut one_pass_nanos = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            let measure_two_pass = || {
                let started = Instant::now();
                let tree = reference_own_process_tree_pids_two_pass(&sys);
                std::hint::black_box(&tree);
                started.elapsed().as_nanos()
            };
            let measure_one_pass = || {
                let started = Instant::now();
                let tree = collect_own_process_tree_pids(&sys);
                std::hint::black_box(&tree);
                started.elapsed().as_nanos()
            };
            if batch % 2 == 0 {
                two_pass_nanos.push(measure_two_pass());
                one_pass_nanos.push(measure_one_pass());
            } else {
                one_pass_nanos.push(measure_one_pass());
                two_pass_nanos.push(measure_two_pass());
            }
        }
        two_pass_nanos.sort_unstable();
        one_pass_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "own-tree collection ({} processes, {} protected): two-pass median/p95={:.1}/{:.1} us; one-pass={:.1}/{:.1}",
            sys.processes().len(),
            expected.len(),
            two_pass_nanos[BATCHES / 2] as f64 / 1_000.0,
            two_pass_nanos[p95_index] as f64 / 1_000.0,
            one_pass_nanos[BATCHES / 2] as f64 / 1_000.0,
            one_pass_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn own_tree_install_path_match_preserves_ascii_case_and_separator_boundary() {
        let cases = [
            (
                r"C:\Program Files\AeroTune\AeroTune.exe",
                r"C:\Program Files\AeroTune",
            ),
            (
                r"c:\PROGRAM FILES\aerotune\webview2.exe",
                r"C:\Program Files\AeroTune",
            ),
            (
                r"C:\Program Files\AeroTune-Dev\helper.exe",
                r"C:\Program Files\AeroTune",
            ),
            (r"C:\Program Files\AeroTune", r"C:\Program Files\AeroTune"),
            (
                r"C:/Program Files/AeroTune/helper.exe",
                r"C:\Program Files\AeroTune",
            ),
            (r"C:\用户\AeroTune\worker.exe", r"c:\用户\aerotune"),
            (r"C:\ÄPP\AeroTune\worker.exe", r"c:\äpp\aerotune"),
        ];
        for (path, directory) in cases {
            let lowercase_path = path.to_ascii_lowercase();
            let lowercase_directory = directory.to_ascii_lowercase();
            let legacy = lowercase_path.starts_with(&lowercase_directory)
                && lowercase_path.as_bytes().get(lowercase_directory.len()) == Some(&b'\\');
            assert_eq!(
                is_child_path_ascii_case_insensitive(path, directory),
                legacy,
                "path match must preserve legacy ASCII case and separator behavior for {path:?} under {directory:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual Windows benchmark for allocation-free own-tree path matching"]
    fn benchmark_own_tree_path_prefix_comparison() {
        const BATCHES: usize = 101;
        const SCANS_PER_BATCH: usize = 20;
        let mut sys = System::new();
        let process_refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh);
        let directory = std::env::current_exe()
            .expect("current executable path should be available")
            .parent()
            .and_then(|path| path.to_str())
            .expect("current executable directory should be UTF-8")
            .to_string();
        let lowercase_directory = directory.to_ascii_lowercase();
        let paths = sys
            .processes()
            .values()
            .filter_map(|process| process.exe()?.to_str().map(str::to_string))
            .collect::<Vec<_>>();
        assert!(!paths.is_empty(), "process image paths should be available");
        let legacy_match_count = paths
            .iter()
            .filter(|path| {
                let lowered = path.to_ascii_lowercase();
                lowered.starts_with(&lowercase_directory)
                    && lowered.as_bytes().get(lowercase_directory.len()) == Some(&b'\\')
            })
            .count();
        let direct_match_count = paths
            .iter()
            .filter(|path| is_child_path_ascii_case_insensitive(path, &directory))
            .count();
        assert_eq!(direct_match_count, legacy_match_count);

        let mut legacy_micros = Vec::with_capacity(BATCHES);
        let mut direct_micros = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            let measure_legacy = || {
                let started = Instant::now();
                let mut matches = 0usize;
                for _ in 0..SCANS_PER_BATCH {
                    for path in &paths {
                        let lowered = std::hint::black_box(path.as_str()).to_ascii_lowercase();
                        matches += (lowered.starts_with(&lowercase_directory)
                            && lowered.as_bytes().get(lowercase_directory.len()) == Some(&b'\\'))
                            as usize;
                    }
                }
                std::hint::black_box(matches);
                started.elapsed().as_secs_f64() * 1_000_000.0 / SCANS_PER_BATCH as f64
            };
            let measure_direct = || {
                let started = Instant::now();
                let mut matches = 0usize;
                for _ in 0..SCANS_PER_BATCH {
                    for path in &paths {
                        matches += is_child_path_ascii_case_insensitive(
                            std::hint::black_box(path.as_str()),
                            &directory,
                        ) as usize;
                    }
                }
                std::hint::black_box(matches);
                started.elapsed().as_secs_f64() * 1_000_000.0 / SCANS_PER_BATCH as f64
            };
            if batch % 2 == 0 {
                legacy_micros.push(measure_legacy());
                direct_micros.push(measure_direct());
            } else {
                direct_micros.push(measure_direct());
                legacy_micros.push(measure_legacy());
            }
        }
        legacy_micros.sort_by(f64::total_cmp);
        direct_micros.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "own-tree path prefix ({} real process paths/scan): lowercase allocation median/p95={:.3}/{:.3} us/scan; direct comparison={:.3}/{:.3}",
            paths.len(),
            legacy_micros[BATCHES / 2],
            legacy_micros[p95_index],
            direct_micros[BATCHES / 2],
            direct_micros[p95_index],
        );
    }

    #[test]
    fn own_tree_pids_are_protected_in_full_scan_only() {
        // 真实案例：自动全盘扫描会把低 CPU、大工作集的自身 WebView2
        // 运行时（msedgewebview2.exe）当作清理候选，拖垮前端。树内
        // PID 必须在进入逐进程保护判定前就被挡下；手动选定路径
        // （eligible_pids=Some）不受影响，仍按用户选择走。
        let sys = System::new();
        let own_tree = collect_own_process_tree_pids(&sys);
        assert!(
            own_tree.contains(&std::process::id()),
            "self pid always belongs to the own tree"
        );
        // 空系统快照不应推断出陌生 PID 属于自身树。
        assert!(!own_tree.contains(&999_999));
    }

    #[test]
    #[ignore = "manual Windows benchmark for cached scheduler own-tree reuse"]
    fn benchmark_own_process_tree_reuse() {
        let mut sys = System::new();
        let process_refresh = ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet);
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, process_refresh);
        assert!(
            !sys.processes().is_empty(),
            "Windows process snapshot should be available"
        );
        let expected_tree = collect_own_process_tree_pids(&sys);
        assert!(expected_tree.contains(&std::process::id()));

        // Warm both paths on the same immutable process snapshot.
        for _ in 0..9 {
            std::hint::black_box(collect_own_process_tree_pids(&sys));
            std::hint::black_box(collect_own_process_tree_pids(&sys));
        }
        let mut one_call_nanos = Vec::with_capacity(101);
        let mut two_call_nanos = Vec::with_capacity(101);
        for _ in 0..101 {
            let started = Instant::now();
            let one = collect_own_process_tree_pids(&sys);
            std::hint::black_box(&one);
            one_call_nanos.push(started.elapsed().as_nanos());
            assert_eq!(one, expected_tree, "one cached snapshot remains equivalent");

            let started = Instant::now();
            let first = collect_own_process_tree_pids(&sys);
            let second = collect_own_process_tree_pids(&sys);
            std::hint::black_box((&first, &second));
            two_call_nanos.push(started.elapsed().as_nanos());
            assert_eq!(
                first, second,
                "repeated collection from the same snapshot is deterministic"
            );
            assert_eq!(second, expected_tree);
        }
        one_call_nanos.sort_unstable();
        two_call_nanos.sort_unstable();
        let p95_index = |len: usize| (len * 95 + 99) / 100 - 1;
        let one_median = one_call_nanos[one_call_nanos.len() / 2];
        let two_median = two_call_nanos[two_call_nanos.len() / 2];
        let one_p95 = one_call_nanos[p95_index(one_call_nanos.len())];
        let two_p95 = two_call_nanos[p95_index(two_call_nanos.len())];
        eprintln!(
            "own-tree snapshot benchmark: processes={}, own_tree={}, cached one-call median/p95={} / {} us; duplicate two-call median/p95={} / {} us",
            sys.processes().len(),
            expected_tree.len(),
            one_median as f64 / 1_000.0,
            one_p95 as f64 / 1_000.0,
            two_median as f64 / 1_000.0,
            two_p95 as f64 / 1_000.0,
        );
    }

    #[test]
    fn windows_background_allowlist_requires_a_system_executable_location() {
        let windows_root = trusted_windows_directory().expect("Windows directory API succeeds");
        assert!(is_allowed_windows_background_process(
            "SearchIndexer.exe",
            &format!("{windows_root}\\System32\\SearchIndexer.exe"),
            windows_root
        ));
        assert!(!is_allowed_windows_background_process(
            "SearchIndexer.exe",
            &format!("{windows_root}\\Temp\\SearchIndexer.exe"),
            windows_root
        ));
        assert!(!is_allowed_windows_background_process(
            "svchost.exe",
            &format!("{windows_root}\\System32\\svchost.exe"),
            windows_root
        ));
    }

    fn previous_is_windows_system_path(path: &str, windows_root: &str) -> bool {
        let path = path.replace('/', "\\").to_ascii_lowercase();
        let root = windows_root
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_ascii_lowercase();
        path.starts_with(&format!("{root}\\"))
    }

    fn previous_is_allowed_windows_background_process(
        name: &str,
        path: &str,
        windows_root: &str,
    ) -> bool {
        let normalized = normalized_process_name(name);
        if !WINDOWS_BACKGROUND_CANDIDATES.contains(&normalized.as_str()) {
            return false;
        }
        let full = path.replace('/', "\\").to_ascii_lowercase();
        let root = windows_root
            .trim_end_matches(['\\', '/'])
            .replace('/', "\\")
            .to_ascii_lowercase();
        let Some(relative) = full.strip_prefix(&format!("{root}\\")) else {
            return false;
        };
        ["system32\\", "syswow64\\", "systemapps\\"]
            .iter()
            .any(|directory| relative.starts_with(directory))
    }

    #[test]
    fn normalized_windows_path_helpers_match_previous_boundary_behavior() {
        let windows_root = trusted_windows_directory().expect("Windows directory API succeeds");
        let trailing_root = format!("{windows_root}/");
        let paths = [
            format!("{windows_root}\\System32\\SearchIndexer.exe"),
            format!(
                "{}/SystemApps/Widget.exe",
                windows_root.replace('\\', "/").to_ascii_uppercase()
            ),
            format!("{windows_root}Old\\System32\\SearchIndexer.exe"),
            windows_root.to_string(),
            String::new(),
        ];
        for path in &paths {
            assert_eq!(
                is_windows_system_path(path, windows_root),
                previous_is_windows_system_path(path, windows_root),
                "path={path}"
            );
            assert_eq!(
                is_windows_system_path_normalized(
                    &normalize_windows_path(path),
                    &normalize_windows_root(windows_root),
                ),
                previous_is_windows_system_path(path, windows_root),
                "normalized path={path}"
            );
            assert_eq!(
                is_windows_system_path(path, &trailing_root),
                previous_is_windows_system_path(path, &trailing_root),
                "path={path}, root={trailing_root}"
            );
        }
        let allowed_cases = [
            (
                "SearchIndexer.exe",
                format!("{windows_root}/System32/SearchIndexer.exe"),
            ),
            (
                "SearchHost.exe",
                format!("{windows_root}/SystemApps/Widget.exe"),
            ),
            (
                "SearchIndexer.exe",
                format!("{windows_root}/Temp/SearchIndexer.exe"),
            ),
            (
                "svchost.exe",
                format!("{windows_root}/System32/svchost.exe"),
            ),
            (
                "SearchIndexer.exe",
                format!("{windows_root}Old/System32/SearchIndexer.exe"),
            ),
        ];
        for (name, path) in allowed_cases {
            assert_eq!(
                is_allowed_windows_background_process(name, &path, &trailing_root),
                previous_is_allowed_windows_background_process(name, &path, &trailing_root),
                "name={name}, path={path}"
            );
            let normalized_name = normalized_process_name(name);
            let normalized_path = normalize_windows_path(&path);
            let normalized_root = normalize_windows_root(&trailing_root);
            assert_eq!(
                is_allowed_windows_background_process_normalized(
                    &normalized_name,
                    &normalized_path,
                    &normalized_root,
                ),
                previous_is_allowed_windows_background_process(name, &path, &trailing_root),
                "normalized name={name}, path={path}"
            );
        }
    }

    #[test]
    fn unknown_foreground_identity_blocks_memory_trim_preflight() {
        assert_eq!(
            require_foreground_process_id(None),
            Err(UNKNOWN_FOREGROUND_PROCESS)
        );
        assert_eq!(
            require_foreground_process_id(Some(0)),
            Err(UNKNOWN_FOREGROUND_PROCESS)
        );
        assert_eq!(require_foreground_process_id(Some(4242)), Ok(4242));
    }

    #[test]
    fn windows_directory_query_uses_bounded_os_result_and_fails_closed() {
        let expected = r"Z:\Windows".encode_utf16().collect::<Vec<_>>();
        let returned = query_windows_directory(|buffer, capacity| {
            if expected.len() + 1 > capacity as usize {
                return (expected.len() + 1) as u32;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(expected.as_ptr(), buffer, expected.len());
                *buffer.add(expected.len()) = 0;
            }
            expected.len() as u32
        });
        assert_eq!(returned.as_deref(), Some(r"Z:\Windows"));
        assert_eq!(query_windows_directory(|_, _| 0), None);
        assert_eq!(query_windows_directory(|_, capacity| capacity), None);
    }

    #[test]
    fn physical_memory_query_returns_plausible_available_memory() {
        let available = available_memory_mb().expect("Windows memory query succeeds");
        assert!(available.is_finite() && available > 0.0);
    }

    #[test]
    fn automatic_memory_relief_requires_high_load_and_low_available_memory() {
        assert!(should_auto_trim_memory(88, 4096.0));
        assert!(!should_auto_trim_memory(87, 4096.0));
        assert!(!should_auto_trim_memory(95, 4096.1));
        assert!(
            should_auto_trim_memory(99, 0.0),
            "verified zero available memory is critical pressure"
        );
        assert!(!should_auto_trim_memory(99, -1.0));
        assert!(!should_auto_trim_memory(99, f64::NAN));
    }

    #[test]
    fn automatic_memory_relief_action_cooldown_uses_monotonic_elapsed_time() {
        let now = Instant::now();
        let cooldown = Duration::from_secs(AUTO_TRIM_COOLDOWN_SECS);
        assert!(!auto_trim_cooldown_active(None, now, cooldown));
        assert!(auto_trim_cooldown_active(Some(now), now, cooldown));
        assert!(auto_trim_cooldown_active(
            Some(now.checked_sub(cooldown - Duration::from_secs(1)).unwrap()),
            now,
            cooldown,
        ));
        assert!(!auto_trim_cooldown_active(
            Some(now.checked_sub(cooldown).unwrap()),
            now,
            cooldown,
        ));
        assert!(auto_trim_cooldown_active(
            Some(now.checked_add(Duration::from_secs(1)).unwrap()),
            now,
            cooldown,
        ));
    }

    #[test]
    fn automatic_memory_relief_noop_scans_have_a_bounded_retry_backoff() {
        let now = Instant::now();
        let retry = Duration::from_secs(AUTO_TRIM_SCAN_RETRY_SECS);
        assert_eq!(AUTO_TRIM_SCAN_RETRY_SECS, 60);
        assert!(!auto_trim_cooldown_active(None, now, retry));
        assert!(auto_trim_cooldown_active(Some(now), now, retry));
        assert!(auto_trim_cooldown_active(
            Some(now.checked_sub(retry - Duration::from_secs(1)).unwrap()),
            now,
            retry,
        ));
        assert!(!auto_trim_cooldown_active(
            Some(now.checked_sub(retry).unwrap()),
            now,
            retry,
        ));
    }

    #[test]
    fn automatic_memory_relief_commits_action_cooldown_only_after_processing() {
        let now = Instant::now();
        let previous_success = Some(
            now.checked_sub(Duration::from_secs(AUTO_TRIM_COOLDOWN_SECS))
                .unwrap(),
        );
        let mut last_success = previous_success;
        let mut report = MemoryOptimizationReport::default();

        auto_trim_commit_success_cooldown(&mut last_success, now, None);
        assert_eq!(
            last_success, previous_success,
            "no candidates or a failed scan must not commit an action cooldown"
        );
        auto_trim_commit_success_cooldown(&mut last_success, now, Some(&report));
        assert_eq!(
            last_success, previous_success,
            "a final recheck that processed no PID is not a completed relief action"
        );

        report.processed = 1;
        auto_trim_commit_success_cooldown(&mut last_success, now, Some(&report));
        assert_eq!(last_success, Some(now));
        assert!(auto_trim_cooldown_active(
            last_success,
            now,
            Duration::from_secs(AUTO_TRIM_COOLDOWN_SECS),
        ));
    }

    #[test]
    fn post_trim_wait_is_skipped_when_no_memory_was_reclaimed() {
        assert!(should_wait_for_memory_reclamation(0.01));
        assert!(!should_wait_for_memory_reclamation(0.0));
        assert!(!should_wait_for_memory_reclamation(-1.0));
        assert!(!should_wait_for_memory_reclamation(f64::NAN));
        assert!(!should_wait_for_memory_reclamation(f64::INFINITY));
    }

    #[test]
    fn invalid_memory_measurements_stay_unknown_and_valid_zero_is_preserved() {
        assert_eq!(validated_available_memory_mb(1024.0), Some(1024.0));
        assert_eq!(
            validated_available_memory_mb(0.0),
            Some(0.0),
            "a successful zero reading is distinct from an API failure"
        );
        assert_eq!(validated_available_memory_mb(-1.0), None);
        assert_eq!(validated_available_memory_mb(f64::NAN), None);
        assert_eq!(validated_available_memory_mb(f64::INFINITY), None);
    }

    #[test]
    fn unavailable_post_action_memory_measurement_serializes_as_null() {
        let report = MemoryOptimizationReport {
            available_before_mb: 2048.0,
            available_after_mb: None,
            ..Default::default()
        };
        let json = serde_json::to_value(report).expect("report serialization succeeds");
        assert!(json["available_after_mb"].is_null());
    }

    #[test]
    fn preview_reports_bounded_ranked_candidates_and_protects_self() {
        let report = optimize_process_memory_sync_limited_with_cpu_limit_for_foreground(
            true,
            None,
            MAX_MEMORY_BATCH,
            MAX_TRIM_CPU_PERCENT,
            MIN_TRIM_WORKING_SET_BYTES,
            None,
            std::process::id(),
        )
        .expect("preview scan succeeds with a controlled known foreground PID");
        assert!(report.available_before_mb.is_finite() && report.available_before_mb > 0.0);
        assert!(report
            .available_after_mb
            .is_some_and(|available| available.is_finite() && available > 0.0));
        assert!(report.candidates.len() <= MAX_MEMORY_CANDIDATES);
        assert!(report
            .candidates
            .iter()
            .all(|candidate| candidate.pid != std::process::id() || !candidate.eligible));
        assert!(report
            .candidates
            .windows(2)
            .all(|pair| { !pair[1].eligible || pair[0].eligible }));
        assert!(report
            .candidates
            .iter()
            .all(|candidate| { !candidate.eligible || candidate.working_set_mb >= 128.0 }));
    }

    #[test]
    fn requested_pid_validation_rejects_empty_core_self_oversized_and_foreground_lists() {
        let empty = HashSet::new();
        assert!(validate_requested_pids(&empty, 9000, 8000).is_err());

        let core = HashSet::from([4]);
        assert!(validate_requested_pids(&core, 9000, 8000).is_err());

        let self_pid = HashSet::from([9000]);
        assert!(validate_requested_pids(&self_pid, 9000, 8000).is_err());

        let foreground = HashSet::from([8000]);
        assert!(validate_requested_pids(&foreground, 9000, 8000).is_err());

        let too_many = (100..100 + MAX_MEMORY_BATCH as u32 + 1).collect::<HashSet<_>>();
        assert!(validate_requested_pids(&too_many, 9000, 8000).is_err());

        let okay = HashSet::from([101, 102]);
        assert!(validate_requested_pids(&okay, 9000, 8000).is_ok());
    }

    #[test]
    fn selected_memory_lookup_visits_only_live_pids_and_preserves_missing_placeholders() {
        let processes = (1_000u32..=10_000)
            .map(|pid| (sysinfo::Pid::from_u32(pid), pid))
            .collect::<HashMap<_, _>>();
        let requested = HashSet::from([10_000, 1_001, 99_999]);
        let selected = selected_process_entries(&processes, &requested);

        assert_eq!(selected.len(), 2);
        assert!(selected.iter().all(|(pid, _)| requested.contains(pid)));
        let mut candidates = selected
            .iter()
            .map(|(pid, marker)| MemoryCandidate {
                pid: *pid,
                name: format!("worker-{marker}"),
                path: String::new(),
                working_set_mb: 256.0,
                private_mb: 128.0,
                cpu: 0.0,
                eligible: true,
                reason: "空闲第三方进程".into(),
                creation_filetime: Some(*pid as u64),
            })
            .collect::<Vec<_>>();
        append_missing_selected_memory_candidates(&mut candidates, &requested);

        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.pid)
                .collect::<Vec<_>>(),
            vec![1_001, 10_000, 99_999],
            "selected results remain sorted by PID regardless of hash iteration order"
        );
        let missing = candidates.last().unwrap();
        assert!(!missing.eligible);
        assert_eq!(missing.name, "进程已退出或 PID 已变化");
        assert_eq!(missing.reason, "进程已退出或 PID 已变化");
    }

    #[test]
    fn automatic_selected_scan_requires_preview_creation_identity() {
        let expected = HashMap::from([(42, 123_456_u64)]);
        assert!(creation_time_matches_expected(42, 123_456, None));
        assert!(creation_time_matches_expected(42, 123_456, Some(&expected)));
        assert!(!creation_time_matches_expected(
            42,
            123_457,
            Some(&expected)
        ));
        assert!(!creation_time_matches_expected(
            99,
            123_456,
            Some(&expected)
        ));
    }

    #[test]
    fn process_info_serializes_only_a_verified_creation_token() {
        let mut info = synthetic_process_info(42);
        info.creation_token = Some(format_creation_token(0x01dc_1234_5678_9abc));
        let serialized = serde_json::to_value(&info).unwrap();
        assert_eq!(serialized["creation_token"], "01dc123456789abc");

        info.creation_token = None;
        let serialized = serde_json::to_value(&info).unwrap();
        assert!(serialized.get("creation_token").is_none());
    }

    #[test]
    fn manual_creation_token_roundtrip_preserves_full_filetime_precision() {
        let creation_filetime = 0x01dc_1234_5678_9abcu64;
        let token = format_creation_token(creation_filetime);
        assert_eq!(token.len(), 16);
        assert_eq!(parse_creation_token(&token).unwrap(), creation_filetime);

        let candidate = MemoryCandidate {
            pid: 42,
            name: "worker.exe".into(),
            path: r"C:\Apps\worker.exe".into(),
            working_set_mb: 512.0,
            private_mb: 256.0,
            cpu: 0.5,
            eligible: true,
            reason: "空闲第三方进程".into(),
            creation_filetime: Some(creation_filetime),
        };
        let json = serde_json::to_value(&candidate).unwrap();
        assert_eq!(json["creation_token"], token);
        assert!(json.get("creation_filetime").is_none());

        let selection: MemoryCandidateIdentity = serde_json::from_value(serde_json::json!({
            "pid": 42,
            "creation_token": json["creation_token"].as_str().unwrap(),
        }))
        .unwrap();
        let (pids, expected) = parse_memory_candidate_identities(vec![selection]).unwrap();
        assert_eq!(pids, HashSet::from([42]));
        assert_eq!(expected.get(&42), Some(&creation_filetime));
        assert!(creation_time_matches_expected(
            42,
            creation_filetime,
            Some(&expected)
        ));
        assert!(!creation_time_matches_expected(
            42,
            creation_filetime + 1,
            Some(&expected)
        ));
    }

    #[test]
    fn manual_creation_tokens_reject_malformed_duplicate_and_oversized_requests() {
        assert!(parse_creation_token("123").is_err());
        assert!(parse_creation_token("000000000000000g").is_err());
        assert!(parse_creation_token("0000000000000000").is_err());
        assert!(
            serde_json::from_value::<MemoryCandidateIdentity>(serde_json::json!({
                "pid": 42,
            }))
            .is_err()
        );
        assert!(
            serde_json::from_value::<MemoryCandidateIdentity>(serde_json::json!({
                "pid": 42,
                "creation_token": "000000000001e240",
                "unexpected": true,
            }))
            .is_err()
        );

        let selection = MemoryCandidateIdentity {
            pid: 42,
            creation_token: "000000000001e240".into(),
        };
        assert!(
            parse_memory_candidate_identities(vec![selection.clone(), selection.clone()]).is_err()
        );
        assert!(parse_memory_candidate_identities(
            (0..=MAX_MEMORY_BATCH)
                .map(|pid| MemoryCandidateIdentity {
                    pid: pid as u32 + 100,
                    creation_token: "000000000001e240".into(),
                })
                .collect()
        )
        .is_err());
    }

    #[test]
    fn process_mutation_tokens_fail_closed_with_refresh_guidance() {
        let expected = 0x01dc_1234_5678_9abcu64;
        let token = format_creation_token(expected);
        assert_eq!(parse_process_creation_token(&token).unwrap(), expected);
        assert_eq!(
            parse_process_creation_token("0000000000000000").unwrap_err(),
            PROCESS_MUTATION_IDENTITY_ERROR
        );
        assert_eq!(
            parse_process_creation_token("000000000000000g").unwrap_err(),
            PROCESS_MUTATION_IDENTITY_ERROR
        );
    }

    #[test]
    fn final_memory_action_status_is_reflected_in_the_candidate_report() {
        let mut report = MemoryOptimizationReport {
            candidates: vec![MemoryCandidate {
                pid: 42,
                name: "worker.exe".into(),
                path: r"C:\Apps\worker.exe".into(),
                working_set_mb: 512.0,
                private_mb: 256.0,
                cpu: 0.5,
                eligible: true,
                reason: "空闲第三方进程".into(),
                creation_filetime: Some(123_456),
            }],
            ..Default::default()
        };

        mark_candidate_report_ineligible(&mut report, 42, "进程实例已变化");
        let candidate = &report.candidates[0];
        assert!(!candidate.eligible);
        assert_eq!(candidate.reason, "进程实例已变化");
    }

    #[test]
    #[ignore = "manual synthetic selected-PID lookup versus all-process sort benchmark"]
    fn benchmark_selected_memory_lookup_against_full_process_sort() {
        let processes = (1u32..=100_000)
            .map(|pid| (sysinfo::Pid::from_u32(pid), (pid * 97 % 1_000_000) as u64))
            .collect::<HashMap<_, _>>();
        let selected = HashSet::from([17, 9_999, 52_345, 99_999, 100_000]);

        let started = Instant::now();
        let direct = selected_process_entries(&processes, &selected)
            .into_iter()
            .map(|(pid, _)| pid)
            .collect::<HashSet<_>>();
        let direct_elapsed = started.elapsed();

        let started = Instant::now();
        let mut all = processes
            .iter()
            .map(|(pid, bytes)| (pid.as_u32(), bytes))
            .collect::<Vec<_>>();
        all.sort_by_key(|(_, bytes)| std::cmp::Reverse(**bytes));
        let full_scan = all
            .into_iter()
            .filter(|(pid, _)| selected.contains(pid))
            .map(|(pid, _)| pid)
            .collect::<HashSet<_>>();
        let full_sort_elapsed = started.elapsed();

        assert_eq!(direct, full_scan);
        assert!(
            direct_elapsed < full_sort_elapsed,
            "selected PID lookup should remain cheaper than materializing and sorting every process"
        );
        println!(
            "selected direct lookup: {direct_elapsed:?}; previous all-process sort/filter: {full_sort_elapsed:?}"
        );
    }

    #[test]
    #[ignore = "manual Windows selected inspection versus full refresh benchmark; does not actuate processes"]
    fn benchmark_selected_memory_inspection_avoids_full_process_refresh() {
        let self_pid = std::process::id();
        let selected = HashSet::from([self_pid]);
        let foreground_pid = foreground_process_id().unwrap_or_default();

        let started = Instant::now();
        let (direct_candidates, _) = inspect_selected_memory_candidates(
            &selected,
            self_pid,
            foreground_pid,
            MIN_TRIM_WORKING_SET_BYTES,
            None,
        );
        let direct_elapsed = started.elapsed();

        let started = Instant::now();
        let mut sys = System::new();
        prepare_memory_scan(&mut sys);
        let process_count = sys.processes().len();
        let (full_scan_candidates, _) =
            inspect_memory_candidates(&sys, foreground_pid, Some(&selected));
        let full_scan_elapsed = started.elapsed();

        assert_eq!(direct_candidates.len(), 1);
        assert_eq!(full_scan_candidates.len(), 1);
        println!(
            "selected direct inspection: {direct_elapsed:?}; full refresh + 1s warm-up + selected inspection: {full_scan_elapsed:?}; processes observed: {process_count}"
        );
    }

    #[test]
    fn memory_candidate_requires_large_idle_unprotected_private_process() {
        assert!(memory_candidate_eligible(
            "chrome.exe",
            "",
            100,
            999,
            200,
            256 * 1024 * 1024,
            128 * 1024 * 1024,
            1.0,
        )
        .is_ok());
        assert!(memory_candidate_eligible(
            "chrome.exe",
            "",
            100,
            999,
            100,
            256 * 1024 * 1024,
            128 * 1024 * 1024,
            1.0,
        )
        .is_err());
        assert!(memory_candidate_eligible(
            "chrome.exe",
            "",
            100,
            999,
            200,
            64 * 1024 * 1024,
            128 * 1024 * 1024,
            1.0,
        )
        .is_err());
        assert!(memory_candidate_eligible(
            "chrome.exe",
            "",
            100,
            999,
            200,
            256 * 1024 * 1024,
            128 * 1024 * 1024,
            30.0,
        )
        .is_err());
        assert!(memory_candidate_eligible(
            "chrome.exe",
            "",
            100,
            999,
            200,
            256 * 1024 * 1024,
            0,
            1.0,
        )
        .is_err());
        assert!(memory_candidate_eligible(
            "vendor.exe",
            r"C:\Windows\System32\vendor.exe",
            100,
            999,
            200,
            256 * 1024 * 1024,
            128 * 1024 * 1024,
            1.0,
        )
        .is_err());
    }

    #[test]
    fn automatic_memory_relief_revalidates_with_stricter_cpu_limit_than_manual() {
        let eligible_at = |cpu, limit| {
            memory_candidate_eligible_with_cpu_limit(
                "chrome.exe",
                "",
                100,
                999,
                200,
                256 * 1024 * 1024,
                128 * 1024 * 1024,
                cpu,
                limit,
            )
        };

        assert_eq!(AUTO_TRIM_CPU_PERCENT, 1.0);
        assert_eq!(MAX_TRIM_CPU_PERCENT, 3.0);
        assert!(eligible_at(1.0, AUTO_TRIM_CPU_PERCENT).is_ok());
        assert!(eligible_at(1.01, AUTO_TRIM_CPU_PERCENT).is_err());
        assert!(eligible_at(2.0, MAX_TRIM_CPU_PERCENT).is_ok());
        assert!(eligible_at(3.0, MAX_TRIM_CPU_PERCENT).is_ok());
        assert!(eligible_at(3.01, MAX_TRIM_CPU_PERCENT).is_err());
        assert!(eligible_at(-0.1, MAX_TRIM_CPU_PERCENT).is_err());
        assert!(eligible_at(f32::NAN, MAX_TRIM_CPU_PERCENT).is_err());
    }

    #[test]
    fn automatic_memory_relief_revalidates_the_large_working_set_threshold() {
        let eligible_at = |working_set| {
            memory_candidate_eligible_with_limits(
                "chrome.exe",
                "",
                100,
                999,
                200,
                working_set,
                128 * 1024 * 1024,
                0.5,
                AUTO_TRIM_CPU_PERCENT,
                AUTO_TRIM_WORKING_SET_BYTES,
            )
        };

        assert_eq!(AUTO_TRIM_WORKING_SET_BYTES, 512 * 1024 * 1024);
        assert!(eligible_at(AUTO_TRIM_WORKING_SET_BYTES).is_ok());
        assert!(eligible_at(AUTO_TRIM_WORKING_SET_BYTES - 1).is_err());
        assert!(
            memory_candidate_eligible(
                "chrome.exe",
                "",
                100,
                999,
                200,
                256 * 1024 * 1024,
                128 * 1024 * 1024,
                2.0,
            )
            .is_ok(),
            "manual policy retains the 128 MiB floor"
        );
    }

    #[test]
    fn process_cpu_percentage_uses_measured_interval_and_total_system_capacity() {
        assert_eq!(
            process_cpu_usage_percent(0, 10_000_000, Duration::from_secs(1), 4),
            Some(25.0)
        );
        assert_eq!(
            process_cpu_usage_percent(0, 10_000_000, Duration::from_secs(2), 4),
            Some(12.5),
            "actual wall elapsed, not the nominal sleep, determines the rate"
        );
        assert_eq!(
            process_cpu_usage_percent(0, 10_000_000, Duration::from_secs(1), 1),
            Some(100.0)
        );
        assert_eq!(
            process_cpu_usage_percent(10, 10, Duration::from_secs(1), 4),
            Some(0.0)
        );
        assert_eq!(
            process_cpu_usage_percent(20_000_000, 0, Duration::from_secs(1), 1),
            None,
            "a regressed cumulative counter is unavailable, not zero CPU"
        );
        assert_eq!(
            process_cpu_usage_percent(0, 10_000_000, Duration::ZERO, 4),
            None
        );
        assert_eq!(
            process_cpu_usage_percent(0, 10_000_000, Duration::from_secs(1), 0),
            None
        );
        assert_eq!(
            process_cpu_usage_percent(0, u64::MAX, Duration::from_nanos(1), 1),
            Some(100.0),
            "impossible spikes are bounded above full-system capacity"
        );
    }

    #[test]
    fn sysinfo_process_cpu_usage_is_normalized_to_whole_machine_capacity() {
        assert_eq!(normalize_sysinfo_cpu_usage(400.0, 4), 100.0);
        assert_eq!(normalize_sysinfo_cpu_usage(40.0, 8), 5.0);
        assert_eq!(normalize_sysinfo_cpu_usage(2.5, 4), 0.625);
        assert_eq!(normalize_sysinfo_cpu_usage(150.0, 1), 100.0);
        assert_eq!(normalize_sysinfo_cpu_usage(12.5, 0), 12.5);
        assert!(normalize_sysinfo_cpu_usage(f32::NAN, 4).is_nan());
    }

    #[test]
    fn process_cpu_denominator_counts_all_active_windows_processor_groups() {
        let active_processors = unsafe { GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) };
        assert!(active_processors > 0);
        assert_eq!(system_logical_processor_count(), Some(active_processors));
    }

    #[test]
    fn cpu_samples_fail_closed_on_identity_mismatch_and_keep_policy_thresholds() {
        let candidate = |pid, creation_filetime| MemoryCandidate {
            pid,
            name: "worker.exe".into(),
            path: r"C:\Apps\worker.exe".into(),
            working_set_mb: 256.0,
            private_mb: 128.0,
            cpu: 0.0,
            eligible: true,
            reason: "等待新鲜 CPU 样本".into(),
            creation_filetime: Some(creation_filetime),
        };
        let sample = |usage_percent, creation_filetime| ProcessCpuSample {
            usage_percent,
            creation_filetime,
        };
        let mut auto_candidates = vec![
            candidate(1, 11),
            candidate(2, 22),
            candidate(3, 33),
            candidate(4, 44),
        ];
        let auto_samples = HashMap::from([
            (1, sample(1.0, 11)),
            (2, sample(1.01, 22)),
            (3, sample(0.5, 999)),
        ]);

        assert_eq!(
            apply_cpu_samples_to_candidates(
                &mut auto_candidates,
                &auto_samples,
                AUTO_TRIM_CPU_PERCENT,
            ),
            3
        );
        assert!(auto_candidates[0].eligible);
        assert_eq!(auto_candidates[0].cpu, 1.0);
        assert_eq!(auto_candidates[0].reason, "空闲第三方进程");
        assert!(!auto_candidates[1].eligible, "auto remains capped at 1%");
        assert!(
            !auto_candidates[2].eligible,
            "PID reuse fails identity match"
        );
        assert!(
            !auto_candidates[3].eligible,
            "missing CPU sample is fail-closed"
        );

        let mut manual_candidates = vec![candidate(5, 55)];
        let manual_samples = HashMap::from([(5, sample(2.0, 55))]);
        assert_eq!(
            apply_cpu_samples_to_candidates(
                &mut manual_candidates,
                &manual_samples,
                MAX_TRIM_CPU_PERCENT,
            ),
            0
        );
        assert!(manual_candidates[0].eligible, "manual gate remains at 3%");
    }

    #[test]
    fn process_io_rate_uses_actual_elapsed_and_saturates_byte_sum() {
        assert!(
            (disk_rate_mb_s(3 * 1024 * 1024, 1024 * 1024, Duration::from_secs(2)) - 2.0).abs()
                < 1e-9
        );
        assert_eq!(
            disk_rate_mb_s(u64::MAX, u64::MAX, Duration::from_secs(1)),
            u64::MAX as f64 / 1024.0 / 1024.0
        );
        assert_eq!(disk_rate_mb_s(100, 100, Duration::ZERO), 0.0);
    }

    #[test]
    fn affinity_uses_system_not_current_mask() {
        assert_eq!(affinity_mask(0b10101010, "all", 1).unwrap(), 0b10101010);
        assert_eq!(affinity_mask(0b10101010, "low", 1).unwrap(), 0b1010);
        assert_eq!(affinity_mask(0b10101010, "high", 1).unwrap(), 0b10100000);
    }
    #[test]
    fn affinity_rejects_groups_empty_unknown_and_single_upper() {
        assert!(affinity_mask(usize::MAX, "all", 2).is_err());
        assert!(affinity_mask(0, "all", 1).is_err());
        assert!(affinity_mask(1, "high", 1).is_err());
        assert!(affinity_mask(3, "P", 1).is_err());
        assert_eq!(
            affinity_mask(usize::MAX, "low", 1).unwrap().count_ones(),
            usize::BITS / 2
        );
    }
}
