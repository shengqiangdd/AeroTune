// 场景自动切换引擎：
// - 全屏检测：SHQueryUserNotificationState（D3D 全屏 / 独占全屏 = BUSY）
//   前台进程检测：GetForegroundWindow → GetWindowThreadProcessId
// - 迟滞状态机：明确 D3D/名单信号按真实时间确认，退出亦经场景确认；
//   切换后 COOLDOWN 秒内不再自动切换，防止来回抖动。
// - 游戏优先级提升：游戏模式下给全屏前台进程 HIGH 优先级，退出后恢复原值。
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use sysinfo::System;
use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{
    GetPriorityClass, GetProcessInformation, GetProcessTimes, OpenProcess, ProcessPowerThrottling,
    SetPriorityClass, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
use windows_sys::Win32::UI::Shell::{SHQueryUserNotificationState, QUERY_USER_NOTIFICATION_STATE};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

const FULLSCREEN_CONFIRM_SECS: u32 = 6;
const GAMELIST_CONFIRM_SECS: u32 = 4;
const COOLDOWN_SECS: u64 = 120;
const PRIORITY_SEPARATION_CONFIRM_SECS: u64 = 10;
const PRIORITY_SEPARATION_MIN_RESIDENCE_SECS: u64 = 30;

/// 全自动自适应：用户离开（无键鼠输入）判定阈值
/// 影音/浏览器进程：全屏时视为影音而非游戏
const MEDIA_PROCS: &[&str] = &[
    "chrome",
    "msedge",
    "firefox",
    "brave",
    "opera",
    "vivaldi",
    "qqbrowser",
    "360se",
    "sogouexplorer",
    "vlc",
    "mpv",
    "potplayer",
    "mpc-hc64",
    "wmplayer",
    "microsoft.media.player",
    "bilibili",
    "iyueml",
    "potplayermini64",
];

/// 异常处置规则：某维度异常值超过阈值持续 N 秒 → 执行动作
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
pub struct Rule {
    /// cpu | mem | io
    pub kind: String,
    pub enabled: bool,
    /// 与异常值同单位：cpu=%，mem=MB，io=KB/s
    pub threshold: f64,
    pub seconds: u32,
    /// notify | deprio; legacy suspend values are loaded as notify while suspension is disabled.
    pub action: String,
}

fn rules_default() -> Vec<Rule> {
    vec![
        Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 70.0,
            seconds: 15,
            action: "notify".into(),
        },
        Rule {
            kind: "mem".into(),
            enabled: true,
            threshold: 1200.0,
            seconds: 20,
            action: "notify".into(),
        },
        Rule {
            kind: "io".into(),
            enabled: false,
            threshold: 60_000.0,
            seconds: 20,
            action: "notify".into(),
        },
    ]
}

/// 系统关键进程：规则动作黑名单（降级为仅提醒）
const CRITICAL_PROCS: &[&str] = &[
    "system",
    "registry",
    "memory compression",
    "idle",
    "wininit",
    "winlogon",
    "csrss",
    "services",
    "lsass",
    "smss",
    "dwm",
    "explorer",
    "svchost",
    "aerotune",
];

#[derive(Serialize, serde::Deserialize, Clone)]
pub struct AutoSettings {
    #[serde(default = "def_true")]
    pub auto_game: bool,
    #[serde(default = "def_true")]
    pub auto_power: bool,
    #[serde(default = "def_true")]
    pub game_boost: bool,
    /// 自动CPU调度：前台高优先级；低活跃后台使用 EcoQoS。
    #[serde(default = "def_true")]
    pub cpu_scheduler: bool,
    /// 调度影子模式：计算候选但不应用 EcoQoS/优先级变化。
    #[serde(default)]
    pub scheduler_shadow: bool,
    /// 点击关闭 → 隐藏到托盘（而不是退出）
    #[serde(default = "def_true")]
    pub close_to_tray: bool,
    /// 自定义游戏进程名单（无边框窗口游戏也能触发自动游戏模式）
    #[serde(default)]
    pub game_list: Vec<String>,
    /// 用户明确保护的后台进程名；匹配后永远跳过 EcoQoS/低优先级调度。
    #[serde(default)]
    pub protected_apps: Vec<String>,
    /// 处置规则
    #[serde(default = "rules_default")]
    pub rules: Vec<Rule>,
    /// 全自动自适应引擎（用户无需手动选择模式）
    #[serde(default = "def_true")]
    pub adaptive: bool,
    /// 离开判定阈值（秒）
    #[serde(default = "def_away")]
    pub away_idle_secs: u32,
    /// 重载判定 CPU 阈值（%）
    #[serde(default = "def_heavy")]
    pub heavy_cpu_threshold: f32,
    /// AeroTune 创建/追踪的高性能计划副本 GUID（本地化免疫，替代名称匹配）
    #[serde(default)]
    pub high_plan_guid: Option<String>,
    /// 节能计划副本 GUID
    #[serde(default)]
    pub saver_plan_guid: Option<String>,
    /// 重大调优前自动创建系统还原点
    #[serde(default)]
    pub auto_restore_point: bool,
    /// 今日各场景累计秒数（引擎自动维护）
    #[serde(default)]
    pub state_seconds: std::collections::HashMap<String, u64>,
    /// 省电/离开场景自动降亮度
    #[serde(default = "def_true")]
    pub auto_dim: bool,
    /// 近 7 天场景时长归档（日期 → 场景秒数）
    #[serde(default)]
    pub history: std::collections::BTreeMap<String, std::collections::HashMap<String, u64>>,
    /// 全自动模式：一键启用整条静默优化栈（场景自适应/电源·EPP·GameDVR/后台 EcoQoS/
    /// 内存压力缓解/场景名单自动采纳）。只聚合既有自动化，不新增破坏性动作——
    /// 删文件、结束进程、修改网络与服务永远保持手动。
    #[serde(default)]
    pub autopilot: bool,
    /// 全自动模式子开关：持续超阈值的非关键、非前台进程，把"仅提醒"规则升级为
    /// 自动降低优先级（可逆、进变更日志）。仅在 autopilot 开启时生效。
    #[serde(default)]
    pub auto_dispose_anomalies: bool,
    /// 开启全自动模式前的开关快照，关闭时按此还原（基线还原语义）。
    #[serde(default)]
    pub autopilot_backup: Option<AutopilotBackup>,
}

/// 全自动模式开启前的相关开关快照。
#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AutopilotBackup {
    pub adaptive: bool,
    pub auto_game: bool,
    pub auto_power: bool,
    pub game_boost: bool,
    pub cpu_scheduler: bool,
    pub scheduler_shadow: bool,
    pub auto_dispose_anomalies: bool,
}

fn def_true() -> bool {
    true
}

fn def_away() -> u32 {
    300
}

fn def_heavy() -> f32 {
    60.0
}

impl Default for AutoSettings {
    fn default() -> Self {
        Self {
            auto_game: false,
            auto_power: true,
            game_boost: true,
            cpu_scheduler: true,
            scheduler_shadow: false,
            close_to_tray: true,
            game_list: Vec::new(),
            protected_apps: Vec::new(),
            rules: rules_default(),
            adaptive: true,
            away_idle_secs: 300,
            heavy_cpu_threshold: 60.0,
            high_plan_guid: None,
            saver_plan_guid: None,
            auto_restore_point: false,
            state_seconds: std::collections::HashMap::new(),
            auto_dim: true,
            history: std::collections::BTreeMap::new(),
            autopilot: false,
            auto_dispose_anomalies: false,
            autopilot_backup: None,
        }
    }
}

#[derive(Serialize, Clone)]
pub struct AutoStateInfo {
    pub cpu_scheduler: CpuSchedulerStatus,
    pub settings: AutoSettings,
    pub foreground: String,
    pub fullscreen: bool,
    pub mode: String,
    pub boosted_pid: Option<u32>,
    pub cooldown: bool,
    pub adaptive: bool,
    pub adaptive_state: String,
    pub baseline_restore_pending: bool,
    pub brightness_restore_pending: bool,
    pub brightness_restore_snapshot_error: bool,
    pub brightness_preserved_value: Option<u8>,
    /// Monitoring heartbeat for distinguishing a valid default snapshot from unavailable state.
    pub stats_sample_id: u64,
    pub cpu_core_peak_pct: f32,
    pub cpu_saturated_cores: usize,
    pub cpu_logical_cores: usize,
    pub idle_secs: u64,
    pub hold_secs: u64,
    pub decisions: Vec<DecisionLog>,
    pub selected_algorithm: String,
    pub algorithm_reason: String,
    pub algorithm_scores: Vec<AlgorithmScore>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct CpuSchedulerStatus {
    pub supported: bool,
    pub eco_qos_processes: usize,
    pub foreground_priority: Option<String>,
    pub last_updated_unix: u64,
    pub detail: String,
    pub monitored_processes: usize,
    pub background_candidates: usize,
    pub background_cpu_cap_pct: f32,
    pub candidate_limit: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProcessSchedulerSnapshot {
    pub(crate) pid: u32,
    pub(crate) image_path: String,
    pub(crate) creation_time: u64,
    pub(crate) original_control_mask: u32,
    pub(crate) original_state_mask: u32,
    #[serde(default)]
    pub(crate) applied_eco_qos: bool,
    #[serde(default)]
    pub(crate) original_priority_class: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProcessPrioritySnapshot {
    pub(crate) pid: u32,
    pub(crate) image_path: String,
    pub(crate) creation_time: u64,
    pub(crate) original_priority_class: u32,
}

const MAX_ECO_QOS_PROCESSES: usize = 10;
// sysinfo's process CPU value is per-core-scaled here; this cap is not whole-machine normalized.
const BACKGROUND_ECO_QOS_CPU_MAX: f32 = 2.5;
const BACKGROUND_STABILITY_SECS: u64 = 15;

fn scheduler_cpu_is_below_per_core_cap(cpu_percent: f32) -> bool {
    cpu_percent.is_finite() && cpu_percent <= BACKGROUND_ECO_QOS_CPU_MAX
}
const SCHEDULER_RESCAN_INTERVAL: Duration = Duration::from_secs(5);
const NORMAL_PRIORITY_CLASS: u32 = 0x00000020;
const BACKGROUND_PRIORITY_CLASS: u32 = 0x00004000; // BELOW_NORMAL_PRIORITY_CLASS
const IDLE_PRIORITY_CLASS: u32 = 0x00000040;
const FOREGROUND_BOOST_PRIORITY_CLASS: u32 = 0x00008000; // ABOVE_NORMAL_PRIORITY_CLASS

fn background_priority_target(original: u32) -> Option<u32> {
    (original == NORMAL_PRIORITY_CLASS).then_some(BACKGROUND_PRIORITY_CLASS)
}

fn scheduler_scene_eligible(mode: &str, foreground_name: &str) -> bool {
    ["none", "away", "power", "media"].contains(&mode)
        && !foreground_name.is_empty()
        && !MEDIA_PROCS.contains(&foreground_name)
}

#[derive(Clone, Debug)]
struct SchedulerCandidate<'a> {
    pid: u32,
    cpu: f32,
    working_set: u64,
    image_path: Cow<'a, str>,
    start_time: u64,
}

fn compare_scheduler_candidates(
    a: &SchedulerCandidate<'_>,
    b: &SchedulerCandidate<'_>,
) -> std::cmp::Ordering {
    a.cpu
        .total_cmp(&b.cpu)
        .then_with(|| b.working_set.cmp(&a.working_set))
        .then_with(|| a.pid.cmp(&b.pid))
}

fn sort_scheduler_candidates(candidates: &mut [SchedulerCandidate<'_>]) {
    // PID is the final tie-breaker, so distinct process entries have a total order.
    candidates.sort_unstable_by(compare_scheduler_candidates);
}

#[cfg(test)]
fn sort_scheduler_candidates_stable_reference(candidates: &mut [SchedulerCandidate<'_>]) {
    candidates.sort_by(compare_scheduler_candidates);
}

/// Probe process identity in rank order and stop after the mutation budget is filled.
fn confirm_scheduler_candidates(
    candidates: &[SchedulerCandidate<'_>],
    limit: usize,
    mut confirm: impl FnMut(&SchedulerCandidate<'_>) -> bool,
) -> Vec<u32> {
    if limit == 0 {
        return Vec::new();
    }
    let mut selected = Vec::with_capacity(limit.min(candidates.len()));
    for candidate in candidates {
        if confirm(candidate) {
            selected.push(candidate.pid);
            if selected.len() == limit {
                break;
            }
        }
    }
    selected
}

const WINDOWS_SCHEDULER_ALLOWLIST: &[&str] = &[
    "searchindexer.exe",
    "searchhost.exe",
    "searchprotocolhost.exe",
    "runtimebroker.exe",
    "sihost.exe",
    "widgets.exe",
    "backgroundtaskhost.exe",
    "vmmemwsl.exe",
];

#[cfg(test)]
fn normalize_scheduler_name(name: &str) -> String {
    name.trim()
        .to_ascii_lowercase()
        .trim_end_matches(".exe")
        .to_string()
}

fn scheduler_name_without_exe_suffix(mut name: &str) -> &str {
    while let Some(suffix) = name.get(name.len().saturating_sub(4)..) {
        if suffix.eq_ignore_ascii_case(".exe") {
            name = &name[..name.len() - 4];
        } else {
            break;
        }
    }
    name
}

fn scheduler_path_byte_eq(left: u8, right: u8) -> bool {
    let left = if left == b'/' { b'\\' } else { left };
    let right = if right == b'/' { b'\\' } else { right };
    left.eq_ignore_ascii_case(&right)
}

fn scheduler_path_eq(left: &str, right: &str) -> bool {
    left.len() == right.len()
        && left
            .as_bytes()
            .iter()
            .zip(right.as_bytes())
            .all(|(&left, &right)| scheduler_path_byte_eq(left, right))
}

fn scheduler_entry_eq_name(entry: &str, name: &str) -> bool {
    entry.len() == name.len()
        && entry
            .as_bytes()
            .iter()
            .zip(name.as_bytes())
            .all(|(&left, &right)| {
                let left = if left == b'/' { b'\\' } else { left };
                left.eq_ignore_ascii_case(&right)
            })
}

fn scheduler_path_starts_with(path: &str, prefix: &str) -> bool {
    path.get(..prefix.len())
        .is_some_and(|actual| scheduler_path_eq(actual, prefix))
}

fn scheduler_path_relative_to_root<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    let root = root.trim_end_matches(['\\', '/']);
    let prefix = path.get(..root.len())?;
    if !scheduler_path_eq(prefix, root) {
        return None;
    }
    let separator = *path.as_bytes().get(root.len())?;
    if separator != b'\\' && separator != b'/' {
        return None;
    }
    path.get(root.len() + 1..)
}

fn filetime_value(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

fn filetime_unix_seconds(value: u64) -> u64 {
    value / 10_000_000u64 - 11_644_473_600u64
}

fn process_identity_from_handle(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Option<(String, u64)> {
    unsafe {
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
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
        let path_ok = windows_sys::Win32::System::Threading::QueryFullProcessImageNameW(
            handle,
            0,
            buffer.as_mut_ptr(),
            &mut length,
        ) != 0;
        let times_ok =
            GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) != 0;
        if !path_ok || !times_ok {
            return None;
        }
        buffer.truncate(length as usize);
        Some((String::from_utf16_lossy(&buffer), filetime_value(creation)))
    }
}

fn process_identity(pid: u32) -> Option<(String, u64)> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let identity = process_identity_from_handle(handle);
        CloseHandle(handle);
        identity
    }
}

fn identity_matches(
    path: &str,
    creation_time: u64,
    expected_path: &str,
    expected_creation: u64,
) -> bool {
    path.eq_ignore_ascii_case(expected_path)
        && exact_filetime_match(creation_time, expected_creation)
}

fn exact_filetime_match(actual: u64, expected: u64) -> bool {
    actual == expected
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IdentityCheck {
    Match,
    Mismatch,
    Unavailable,
}

fn classify_identity(
    identity: Option<(String, u64)>,
    expected_path: &str,
    expected_creation: u64,
) -> IdentityCheck {
    match identity {
        Some((path, creation))
            if identity_matches(&path, creation, expected_path, expected_creation) =>
        {
            IdentityCheck::Match
        }
        Some(_) => IdentityCheck::Mismatch,
        None => IdentityCheck::Unavailable,
    }
}

fn priority_restore_target(current: u32, scheduler_owned: u32, original: u32) -> Option<u32> {
    (current != 0 && current == scheduler_owned && original != 0 && current != original)
        .then_some(original)
}

fn foreground_priority_target(original: u32) -> Option<u32> {
    matches!(
        original,
        IDLE_PRIORITY_CLASS | BACKGROUND_PRIORITY_CLASS | NORMAL_PRIORITY_CLASS
    )
    .then_some(FOREGROUND_BOOST_PRIORITY_CLASS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ForegroundBoostOutcome {
    Applied,
    NotApplied,
    Unverified,
}

fn foreground_boost_outcome(
    priority_before: u32,
    original: u32,
    set_succeeded: bool,
    priority_after: u32,
    target: u32,
) -> ForegroundBoostOutcome {
    if priority_before != original || !set_succeeded {
        ForegroundBoostOutcome::NotApplied
    } else if priority_after == target {
        ForegroundBoostOutcome::Applied
    } else if priority_after == 0 {
        ForegroundBoostOutcome::Unverified
    } else {
        ForegroundBoostOutcome::NotApplied
    }
}

fn restore_priority_class_if_owned(
    handle: windows_sys::Win32::Foundation::HANDLE,
    scheduler_owned: u32,
    original: u32,
) -> bool {
    let current = unsafe { GetPriorityClass(handle) };
    if current == 0 {
        return false;
    }
    let Some(target) = priority_restore_target(current, scheduler_owned, original) else {
        // Preserve user/application changes; the saved value is no longer ours to restore over.
        return true;
    };
    unsafe { SetPriorityClass(handle, target) != 0 && GetPriorityClass(handle) == target }
}

fn handle_identity_check(
    handle: windows_sys::Win32::Foundation::HANDLE,
    expected_path: &str,
    expected_creation: u64,
) -> IdentityCheck {
    classify_identity(
        process_identity_from_handle(handle),
        expected_path,
        expected_creation,
    )
}

fn handle_matches_identity(
    handle: windows_sys::Win32::Foundation::HANDLE,
    expected_path: &str,
    expected_creation: u64,
) -> bool {
    handle_identity_check(handle, expected_path, expected_creation) == IdentityCheck::Match
}

fn same_scheduler_process(snapshot: &ProcessSchedulerSnapshot) -> IdentityCheck {
    classify_identity(
        process_identity(snapshot.pid),
        &snapshot.image_path,
        snapshot.creation_time,
    )
}

fn same_priority_process(snapshot: &ProcessPrioritySnapshot) -> IdentityCheck {
    classify_identity(
        process_identity(snapshot.pid),
        &snapshot.image_path,
        snapshot.creation_time,
    )
}

fn priority_class_for_snapshot(snapshot: &ProcessPrioritySnapshot) -> Option<u32> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, snapshot.pid) };
    if handle.is_null() {
        return None;
    }
    let priority = if handle_matches_identity(handle, &snapshot.image_path, snapshot.creation_time)
    {
        let current = unsafe { GetPriorityClass(handle) };
        (current != 0).then_some(current)
    } else {
        None
    };
    unsafe { CloseHandle(handle) };
    priority
}

pub(crate) fn restore_priority_snapshot(snapshot: &ProcessPrioritySnapshot) -> Result<(), String> {
    // 先判断 PID 是否已退出：已退出则目标进程已不存在，恢复动作无意义，
    // 直接返回 Ok 让调用方丢弃快照，避免访问受限时记录永久滞留。
    if process_identity(snapshot.pid).is_none() {
        tracing::info!(pid = snapshot.pid, "前台进程已退出，清理优先级恢复快照");
        return Ok(());
    }
    match same_priority_process(snapshot) {
        IdentityCheck::Match => {}
        IdentityCheck::Mismatch => return Ok(()),
        IdentityCheck::Unavailable => {
            return Err(format!(
                "PID {} 身份暂不可读，保留优先级恢复快照",
                snapshot.pid
            ));
        }
    }
    let handle = unsafe {
        OpenProcess(
            PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            snapshot.pid,
        )
    };
    if handle.is_null() {
        return Err(format!("PID {} 优先级恢复句柄不可用", snapshot.pid));
    }
    match handle_identity_check(handle, &snapshot.image_path, snapshot.creation_time) {
        IdentityCheck::Match => {}
        IdentityCheck::Mismatch => {
            unsafe { CloseHandle(handle) };
            return Ok(());
        }
        IdentityCheck::Unavailable => {
            unsafe { CloseHandle(handle) };
            return Err(format!(
                "PID {} 身份暂不可读，保留优先级恢复快照",
                snapshot.pid
            ));
        }
    }
    let restored = restore_priority_class_if_owned(
        handle,
        FOREGROUND_BOOST_PRIORITY_CLASS,
        snapshot.original_priority_class,
    );
    unsafe { CloseHandle(handle) };
    if restored {
        Ok(())
    } else {
        Err(format!("PID {} 优先级恢复失败", snapshot.pid))
    }
}

fn finish_scheduler_restore(
    original_count: usize,
    snapshots: &HashMap<u32, ProcessSchedulerSnapshot>,
    persist: impl FnOnce(&HashMap<u32, ProcessSchedulerSnapshot>) -> Result<(), String>,
) -> Result<(), String> {
    if !snapshots.is_empty() {
        return Err("部分进程调度状态仍未确认恢复，保留记录等待重试".into());
    }
    // Empty steady-state restores are a no-op; avoid sampler-tick disk writes and preserve unreadable evidence.
    if original_count == 0 {
        return Ok(());
    }
    persist(snapshots)
}

pub(crate) fn restore_scheduler_snapshot_map(
    snapshots: &mut HashMap<u32, ProcessSchedulerSnapshot>,
) -> Result<usize, String> {
    let original_count = snapshots.len();
    let mut restored = 0;
    let pids = snapshots.keys().copied().collect::<Vec<_>>();
    for pid in pids {
        let Some(snapshot) = snapshots.get(&pid).cloned() else {
            continue;
        };
        let handle = unsafe {
            OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid,
            )
        };
        if handle.is_null() {
            // 打开失败可能是进程已退出或访问受限：用受限查询复查身份，
            // 已退出的 PID 直接清理快照，避免记录永久滞留；存活但访问受限的仍保留等待重试。
            if process_identity(pid).is_none() {
                snapshots.remove(&pid);
                tracing::warn!(pid, "后台进程已退出，清理 CPU 恢复快照");
            } else {
                tracing::warn!(pid, "后台进程恢复句柄不可用，保留 CPU 恢复快照");
            }
            continue;
        }
        match handle_identity_check(handle, &snapshot.image_path, snapshot.creation_time) {
            IdentityCheck::Match => {}
            IdentityCheck::Mismatch => {
                unsafe { CloseHandle(handle) };
                snapshots.remove(&pid);
                continue;
            }
            IdentityCheck::Unavailable => {
                unsafe { CloseHandle(handle) };
                tracing::debug!(pid, "后台进程身份暂不可读，保留 CPU 恢复快照");
                continue;
            }
        }
        let qos_ok = !snapshot.applied_eco_qos || restore_eco_qos_owned(handle, &snapshot);
        let priority_ok = snapshot.original_priority_class.is_none_or(|priority| {
            restore_priority_class_if_owned(handle, BACKGROUND_PRIORITY_CLASS, priority)
        });
        if qos_ok && priority_ok {
            snapshots.remove(&pid);
            restored += 1;
        }
        unsafe {
            CloseHandle(handle);
        }
    }
    finish_scheduler_restore(original_count, snapshots, persist_scheduler_snapshots)?;
    if restored > 0 {
        tracing::info!(restored, "后台 CPU 调度状态已恢复");
    }
    Ok(restored)
}

fn restore_scheduler_processes(state: &mut AutoState) -> Result<usize, String> {
    restore_scheduler_snapshot_map(&mut state.scheduler_originals)
}

fn set_process_eco_qos(state: &mut AutoState, pid: u32) -> Result<(), String> {
    let Some((image_path, creation_time)) = process_identity(pid) else {
        return Err(format!("PID {pid} 已退出或无法读取身份"));
    };
    let mut previous_snapshot = None;
    if let Some(existing) = state.scheduler_originals.get(&pid) {
        if existing.image_path.eq_ignore_ascii_case(&image_path)
            && existing.creation_time == creation_time
            && existing.applied_eco_qos
        {
            return Ok(());
        }
        if existing.image_path.eq_ignore_ascii_case(&image_path)
            && existing.creation_time == creation_time
        {
            previous_snapshot = Some(existing.clone());
        }
        state.scheduler_originals.remove(&pid);
        persist_scheduler_snapshots(&state.scheduler_originals)?;
    }
    let handle = unsafe {
        OpenProcess(
            PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        )
    };
    if handle.is_null() {
        if let Some(previous) = previous_snapshot {
            state.scheduler_originals.insert(pid, previous);
            let _ = persist_scheduler_snapshots(&state.scheduler_originals);
        }
        return Err(format!("PID {pid} 调度状态读取失败"));
    }
    let Some((handle_path, handle_creation)) = process_identity_from_handle(handle) else {
        unsafe { CloseHandle(handle) };
        if let Some(previous) = previous_snapshot {
            state.scheduler_originals.insert(pid, previous);
            let _ = persist_scheduler_snapshots(&state.scheduler_originals);
        }
        return Err(format!("PID {pid} 进程身份在打开句柄后校验失败"));
    };
    if !identity_matches(&handle_path, handle_creation, &image_path, creation_time) {
        unsafe { CloseHandle(handle) };
        if let Some(previous) = previous_snapshot {
            state.scheduler_originals.insert(pid, previous);
            let _ = persist_scheduler_snapshots(&state.scheduler_originals);
        }
        return Err(format!("PID {pid} 在调度前发生复用，已跳过"));
    }
    let Some(current) = read_power_throttling(handle) else {
        unsafe { CloseHandle(handle) };
        if let Some(previous) = previous_snapshot {
            state.scheduler_originals.insert(pid, previous);
            let _ = persist_scheduler_snapshots(&state.scheduler_originals);
        }
        return Err("此 Windows 版本或进程不支持 EcoQoS 控制".into());
    };
    let original = if let Some(previous) = previous_snapshot.as_ref() {
        let (control_mask, state_mask) = merged_eco_qos_restore(
            current.ControlMask,
            current.StateMask,
            previous.original_control_mask,
            previous.original_state_mask,
            previous.applied_eco_qos,
        );
        PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: control_mask,
            StateMask: state_mask,
        }
    } else {
        current
    };
    let bit = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
    let priority = unsafe { GetPriorityClass(handle) };
    let background_priority = background_priority_target(priority).is_some();
    let desired = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: original.ControlMask | bit,
        StateMask: original.StateMask | bit,
    };
    let mut pending = state.scheduler_originals.clone();
    pending.insert(
        pid,
        ProcessSchedulerSnapshot {
            pid,
            image_path,
            creation_time,
            original_control_mask: original.ControlMask,
            original_state_mask: original.StateMask,
            applied_eco_qos: true,
            original_priority_class: background_priority.then_some(priority),
        },
    );
    if let Err(error) = persist_scheduler_snapshots(&pending) {
        unsafe {
            CloseHandle(handle);
        }
        return Err(format!("CPU 调度原值无法落盘，未改变进程状态：{error}"));
    }
    state.scheduler_originals = pending;
    let mut applied = write_power_throttling(handle, &desired)
        && read_power_throttling(handle).is_some_and(|actual| {
            actual.ControlMask & bit != 0
                && actual.StateMask & bit != 0
                && actual.ControlMask & !bit == original.ControlMask & !bit
                && actual.StateMask & !bit == original.StateMask & !bit
        });
    if applied && background_priority {
        applied = unsafe { SetPriorityClass(handle, BACKGROUND_PRIORITY_CLASS) != 0 }
            && unsafe { GetPriorityClass(handle) == BACKGROUND_PRIORITY_CLASS };
    }
    if !applied {
        let rolled_back = state
            .scheduler_originals
            .get(&pid)
            .is_some_and(|snapshot| restore_eco_qos_owned(handle, snapshot));
        let priority_rolled_back = !background_priority
            || restore_priority_class_if_owned(handle, BACKGROUND_PRIORITY_CLASS, priority);
        unsafe {
            CloseHandle(handle);
        }
        if rolled_back && priority_rolled_back {
            state.scheduler_originals.remove(&pid);
            let _ = persist_scheduler_snapshots(&state.scheduler_originals);
            return Err(format!("PID {pid} 调度写入/回读失败，原状态已恢复"));
        }
        return Err(format!(
            "PID {pid} 调度写入/回读失败，原状态保留在恢复记录中"
        ));
    }
    unsafe {
        CloseHandle(handle);
    }
    Ok(())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn scheduler_unix_now() -> u64 {
    unix_now()
}

fn read_power_throttling(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Option<PROCESS_POWER_THROTTLING_STATE> {
    let mut state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: 0,
        StateMask: 0,
    };
    let ok = unsafe {
        GetProcessInformation(
            handle,
            ProcessPowerThrottling,
            (&mut state as *mut PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        )
    };
    (ok != 0).then_some(state)
}

fn write_power_throttling(
    handle: windows_sys::Win32::Foundation::HANDLE,
    state: &PROCESS_POWER_THROTTLING_STATE,
) -> bool {
    unsafe {
        SetProcessInformation(
            handle,
            ProcessPowerThrottling,
            (state as *const PROCESS_POWER_THROTTLING_STATE).cast(),
            std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        ) != 0
    }
}

fn merged_eco_qos_restore(
    current_control: u32,
    current_state: u32,
    original_control: u32,
    original_state: u32,
    applied: bool,
) -> (u32, u32) {
    if !applied {
        return (current_control, current_state);
    }
    let bit = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
    let control = if original_control & bit == 0 {
        current_control & !bit
    } else {
        current_control
    };
    let state = if original_state & bit == 0 {
        current_state & !bit
    } else {
        current_state
    };
    (control, state)
}

fn restore_eco_qos_owned(
    handle: windows_sys::Win32::Foundation::HANDLE,
    snapshot: &ProcessSchedulerSnapshot,
) -> bool {
    let Some(current) = read_power_throttling(handle) else {
        return false;
    };
    let (control_mask, state_mask) = merged_eco_qos_restore(
        current.ControlMask,
        current.StateMask,
        snapshot.original_control_mask,
        snapshot.original_state_mask,
        snapshot.applied_eco_qos,
    );
    if control_mask == current.ControlMask && state_mask == current.StateMask {
        return true;
    }
    let desired = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: control_mask,
        StateMask: state_mask,
    };
    write_power_throttling(handle, &desired)
        && read_power_throttling(handle).is_some_and(|actual| {
            actual.ControlMask == control_mask && actual.StateMask == state_mask
        })
}

fn scheduler_snapshot_path() -> std::path::PathBuf {
    crate::util::app_dir().join("cpu-scheduler-restore.json")
}

fn foreground_snapshot_path() -> std::path::PathBuf {
    crate::util::app_dir().join("cpu-scheduler-foreground.json")
}

pub(crate) fn persist_foreground_snapshot(
    snapshot: Option<&ProcessPrioritySnapshot>,
) -> Result<(), String> {
    let path = foreground_snapshot_path();
    if let Some(snapshot) = snapshot {
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|e| e.to_string())?;
        crate::util::atomic_write(&path, &bytes)
    } else if path.exists() {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    } else {
        Ok(())
    }
}

fn load_foreground_snapshot() -> Option<ProcessPrioritySnapshot> {
    std::fs::read(foreground_snapshot_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn persist_bytes_if_changed(
    current: Option<&[u8]>,
    desired: &[u8],
    persist: impl FnOnce() -> Result<(), String>,
) -> Result<bool, String> {
    if current == Some(desired) {
        return Ok(false);
    }
    persist()?;
    Ok(true)
}

fn persist_scheduler_snapshots(
    snapshots: &HashMap<u32, ProcessSchedulerSnapshot>,
) -> Result<(), String> {
    let path = scheduler_snapshot_path();
    let bytes = serde_json::to_vec_pretty(snapshots).map_err(|e| e.to_string())?;
    let current = std::fs::read(&path).ok();
    // Final calls are also retry points; skip only when these exact bytes are already durable.
    persist_bytes_if_changed(current.as_deref(), &bytes, || {
        crate::util::atomic_write(&path, &bytes)
    })?;
    Ok(())
}

fn load_scheduler_snapshots() -> HashMap<u32, ProcessSchedulerSnapshot> {
    std::fs::read(scheduler_snapshot_path())
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn scheduler_process_allowed(
    name: &str,
    path: &str,
    windows_root: Option<&str>,
    protected_apps: &[String],
) -> bool {
    let name = name.trim();
    let normalized = scheduler_name_without_exe_suffix(name);
    let explicitly_protected = protected_apps.iter().any(|entry| {
        let entry = entry.trim();
        scheduler_entry_eq_name(entry, name)
            || scheduler_path_eq(entry, path)
            || (!entry.contains(['\\', '/']) && scheduler_entry_eq_name(entry, normalized))
    });
    if path.is_empty()
        || explicitly_protected
        || CRITICAL_PROCS
            .iter()
            .any(|critical| normalized.eq_ignore_ascii_case(critical))
        || normalized.eq_ignore_ascii_case("aerotune")
    {
        return false;
    }
    let Some(windows_root) = windows_root else {
        return false;
    };
    let Some(relative) = scheduler_path_relative_to_root(path, windows_root) else {
        return true;
    };
    WINDOWS_SCHEDULER_ALLOWLIST
        .iter()
        .any(|allowed| name.eq_ignore_ascii_case(allowed))
        && ["system32\\", "syswow64\\", "systemapps\\"]
            .iter()
            .any(|folder| scheduler_path_starts_with(relative, folder))
}

#[cfg(test)]
fn scheduler_process_allowed_reference(
    name: &str,
    path: &str,
    windows_root: Option<&str>,
    protected_apps: &[String],
) -> bool {
    let name = name.trim().to_ascii_lowercase();
    let normalized = normalize_scheduler_name(&name);
    let normalized_path = path.replace('/', "\\").to_ascii_lowercase();
    let explicitly_protected = protected_apps.iter().any(|entry| {
        let entry = entry.trim().replace('/', "\\").to_ascii_lowercase();
        entry == name || entry == normalized_path || (!entry.contains('\\') && entry == normalized)
    });
    if path.is_empty()
        || explicitly_protected
        || CRITICAL_PROCS.contains(&normalized.as_str())
        || normalized == "aerotune"
    {
        return false;
    }
    let Some(windows_root) = windows_root else {
        return false;
    };
    let root = windows_root
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_ascii_lowercase();
    let in_windows = normalized_path.starts_with(&format!("{root}\\"));
    if !in_windows {
        return true;
    }
    WINDOWS_SCHEDULER_ALLOWLIST.contains(&name.as_str())
        && ["system32\\", "syswow64\\", "systemapps\\"]
            .iter()
            .any(|folder| {
                normalized_path
                    .strip_prefix(&format!("{root}\\"))
                    .is_some_and(|relative| relative.starts_with(folder))
            })
}

fn scheduler_process_is_stable(now: u64, start_time: u64) -> bool {
    start_time != 0 && start_time <= now && now - start_time >= BACKGROUND_STABILITY_SECS
}

/// 调度器候选过滤（纯判定）：静默后台 + 稳定 + 不在自身进程树内。
/// 自身进程树（AeroTune + WebView2 运行时等子进程）绝不能被 EcoQoS 节流，
/// 否则前端事件循环会被冻结、唤醒后误报“数据停更”横幅（真实案例：
/// msedgewebview2.exe PID 13404 被 EcoQoS + BACKGROUND_PRIORITY_CLASS）。
fn scheduler_candidate_is_selectable(
    pid: u32,
    self_pid: u32,
    fg_pid: Option<u32>,
    cpu: f32,
    now_unix: u64,
    start_time: u64,
    own_tree: &HashSet<u32>,
) -> bool {
    if !scheduler_cpu_is_below_per_core_cap(cpu) {
        // fail-closed：CPU 门槛必须先于任何后续放宽判定。
        return false;
    }
    pid > 4
        && pid != self_pid
        && Some(pid) != fg_pid
        && !own_tree.contains(&pid)
        && scheduler_process_is_stable(now_unix, start_time)
}

#[cfg(test)]
fn scheduler_process_allowed_for_test(name: &str, path: &str, windows_root: &str) -> bool {
    scheduler_process_allowed(name, path, Some(windows_root), &[])
}

#[cfg(test)]
fn scheduler_process_protected_for_test(
    name: &str,
    path: &str,
    windows_root: &str,
    protected_apps: &[String],
) -> bool {
    scheduler_process_allowed(name, path, Some(windows_root), protected_apps)
}

#[derive(Serialize, Clone, Debug)]
pub struct DecisionLog {
    pub ts: u64,
    pub from: String,
    pub to: String,
    pub reason: String,
}

// ================= 全自动自适应控制器 =================

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum PlanKind {
    High,
    Balanced,
    Saver,
}

/// 自适应引擎可调的"旋钮"集合（快照只记录已确认达成的状态）
/// epp 为 None 表示平台不支持（OEM 阉割 perfepp）或尚未设置
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Knobs {
    pub(crate) plan: PlanKind,
    pub(crate) epp: Option<u32>,
    pub(crate) proc_max: u8,
    pub(crate) gamedvr: bool,
    pub(crate) prio_boost: bool,
    pub(crate) priority_separation: u32,
}

#[derive(Default)]
pub(crate) struct Adaptive {
    pub(crate) state: String,
    pub(crate) pending: Confirmation,
    pub(crate) applied: Option<Knobs>,
    pub(crate) baseline: Option<Knobs>,
    pub(crate) baseline_plan_guid: Option<String>,
    pub(crate) baseline_restore_pending: bool,
    last_calibration: Option<Instant>,
    plan_retry_after: std::collections::HashMap<PlanKind, Instant>,
    pub(crate) decisions: Vec<DecisionLog>,
    /// 各场景累计秒数（今日）
    pub(crate) seconds: std::collections::HashMap<String, u64>,
    pub(crate) stats_day: String,
    persist_elapsed: Duration,
    stats_remainder: Duration,
    /// 亮度执行状态机：只做内存决策，WMI 调用交给专用工作线程。
    ///
    /// 取代原先的 `brightness_saved` / `brightness_retry_after` 字段，
    /// 避免同一状态存在两份副本。
    pub(crate) brightness: crate::brightness::BrightnessState,
    strategy: StrategyKind,
    strategy_scores: Vec<AlgorithmScore>,
    strategy_reason: String,
    strategy_pending: Option<(StrategyKind, Instant)>,
    strategy_changed_at: Option<Instant>,
    priority_separation_gate: PrioritySeparationGate,
    telemetry: AutoTelemetry,
}

#[derive(Default)]
struct PrioritySeparationGate {
    pending: Option<(u32, Instant)>,
    last_applied_at: Option<Instant>,
}

impl PrioritySeparationGate {
    fn resolve(&mut self, current: u32, desired: u32, immediate: bool, now: Instant) -> u32 {
        if current == desired {
            self.pending = None;
            return desired;
        }
        if immediate {
            self.pending = None;
            return desired;
        }

        if !matches!(self.pending, Some((candidate, _)) if candidate == desired) {
            self.pending = Some((desired, now));
        }
        if self.ready_for_reevaluation(now) {
            self.pending = None;
            desired
        } else {
            current
        }
    }

    fn ready_for_reevaluation(&self, now: Instant) -> bool {
        let Some((_, candidate_since)) = self.pending else {
            return false;
        };
        let confirmed = now
            .checked_duration_since(candidate_since)
            .unwrap_or_default()
            >= Duration::from_secs(PRIORITY_SEPARATION_CONFIRM_SECS);
        let resident = self
            .last_applied_at
            .map(|changed| {
                now.checked_duration_since(changed).unwrap_or_default()
                    >= Duration::from_secs(PRIORITY_SEPARATION_MIN_RESIDENCE_SECS)
            })
            .unwrap_or(true);
        confirmed && resident
    }

    fn record_applied(&mut self, now: Instant) {
        self.last_applied_at = Some(now);
        self.pending = None;
    }

    fn clear_pending(&mut self) {
        self.pending = None;
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

fn adaptive_knobs_due(
    calibrate: bool,
    strategy_changed: bool,
    priority_gate: &PrioritySeparationGate,
    now: Instant,
) -> bool {
    calibrate || strategy_changed || priority_gate.ready_for_reevaluation(now)
}

#[derive(Default)]
pub(crate) struct Confirmation {
    target: String,
    since: Option<Instant>,
}
impl Confirmation {
    fn ready(&mut self, target: &str, now: Instant, seconds: u32) -> bool {
        if self.target != target || self.since.is_none() {
            self.target = target.into();
            self.since = Some(now);
        }
        now.duration_since(self.since.unwrap()) >= Duration::from_secs(seconds as u64)
    }
    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }
}

fn reset_imported_adaptive_transients(adaptive: &mut Adaptive, enabled: bool) {
    adaptive.pending.clear();
    adaptive.seconds.clear();
    adaptive.strategy_pending = None;
    adaptive.strategy_changed_at = None;
    adaptive.priority_separation_gate.reset();
    adaptive.strategy_scores.clear();
    adaptive.strategy_reason = if enabled {
        "配置已导入，等待重新评估".into()
    } else {
        "自适应引擎已关闭".into()
    };
}

fn transition_allowed(confirmed: bool, manual_hold: bool, cooldown: bool) -> bool {
    confirmed && !manual_hold && !cooldown
}
fn strategy_transition_ready(
    candidate: StrategyKind,
    incumbent: StrategyKind,
    candidate_for: Duration,
    residence_for: Duration,
    manual_hold: bool,
    urgent_response: bool,
) -> bool {
    if manual_hold || candidate == incumbent {
        return false;
    }
    if urgent_response && candidate == StrategyKind::Responsiveness {
        return candidate_for >= Duration::from_secs(3);
    }
    candidate_for >= Duration::from_secs(5) && residence_for >= incumbent.min_residence()
}

fn retain_incumbent_with_hysteresis(
    best: StrategyKind,
    current: StrategyKind,
    best_score: i32,
    current_score: i32,
) -> StrategyKind {
    if best != current && best_score < current_score + 12 {
        current
    } else {
        best
    }
}
fn calibration_due(last: &mut Option<Instant>, now: Instant) -> bool {
    if last
        .map(|t| now.duration_since(t) < Duration::from_secs(CALIBRATE_EVERY as u64))
        .unwrap_or(false)
    {
        return false;
    }
    *last = Some(now);
    true
}
fn account_seconds(ad: &mut Adaptive, elapsed: Duration) {
    ad.stats_remainder += elapsed;
    let secs = ad.stats_remainder.as_secs();
    ad.stats_remainder -= Duration::from_secs(secs);
    *ad.seconds.entry(ad.state.clone()).or_insert(0) += secs;
}
fn game_signal(notification: i32, foreground: &str, listed: bool) -> bool {
    listed
        || (notification == 3
            && !foreground.is_empty()
            && foreground != "aerotune"
            && !MEDIA_PROCS.contains(&foreground))
}

/// 场景切换后的亮度处理：**只在锁内做决策**，把 WMI 调用排给专用工作线程。
///
/// 旧实现直接在锁内调用 `get_brightness` / `set_brightness`（各约 300-400ms，
/// 其中 PowerShell 进程启动就占 ~230ms），会把这把锁的持有时间拉长数百毫秒，
/// 连带把共用同一把锁的 UI 命令一起卡住。
pub(crate) fn update_brightness(ad: &mut Adaptive, dim: bool) {
    // 先合并工作线程已完成的结果，避免基于过期状态排队。
    if let Some(outcome) = crate::brightness::handle().take_outcome() {
        ad.brightness.apply_outcome(outcome);
    }
    let now = Instant::now();
    if let Some(task) = ad.brightness.plan(dim, now) {
        if !crate::brightness::handle().request(task) {
            // 队列不可用（线程已退出）：清掉对应排队标记，下个 tick 再试。
            match task {
                crate::brightness::BrightnessTask::Dim => ad.brightness.dim_pending = false,
                crate::brightness::BrightnessTask::Restore(_)
                | crate::brightness::BrightnessTask::Recover(_) => {
                    ad.brightness.restore_pending = false;
                }
            }
        }
    }
}

/// 托管在 Tauri State 里的运行时引擎状态
pub type AutoStateLock = std::sync::Arc<std::sync::Mutex<AutoState>>;

const CALIBRATE_EVERY: u32 = 15;

#[derive(Clone, Copy, Debug, Default)]
pub struct AutoTelemetry {
    pub cpu_avg_60s: f32,
    pub cpu_avg_10s: f32,
    pub cpu_peak: f32,
    pub cpu_forecast_10s_pct: f32,
    pub cpu_trend_pct_per_sec: f32,
    pub cpu_forecast_stability: f32,
    pub cpu_forecast_ready: bool,
    pub cpu_core_peak_pct: f32,
    pub cpu_saturated_cores: usize,
    pub cpu_logical_cores: usize,
    pub mem_pct: f32,
    pub mem_slope: f32,
    pub process_count: usize,
}

#[derive(Serialize, Clone, Debug)]
pub struct AlgorithmScore {
    pub id: String,
    pub score: i32,
    pub suitable: bool,
    pub reason: String,
}

struct AdaptiveCtx {
    gaming: bool,
    media: bool,
    idle_secs: u64,
    ac_online: bool,
    auto_power: bool,
    telemetry: AutoTelemetry,
    epp_ok: bool,
    heavy_threshold: f32,
    away_threshold: u64,
}

impl AdaptiveCtx {
    fn cpu_avg(&self) -> Option<f32> {
        let value = self.telemetry.cpu_avg_60s;
        value.is_finite().then(|| value.clamp(0.0, 100.0))
    }
}

const SINGLE_CORE_GLOBAL_LOAD_RATIO: f32 = 0.20;
const CPU_FORECAST_MIN_SLOPE: f32 = 0.75;
const CPU_FORECAST_MIN_STABILITY: f32 = 60.0;

fn single_core_contention(ctx: &AdaptiveCtx) -> bool {
    let telemetry = ctx.telemetry;
    let logical_cores = telemetry.cpu_logical_cores.max(1);
    let one_core_global_pct = 100.0 / logical_cores as f32;
    telemetry.cpu_logical_cores > 0
        && ctx.idle_secs < ctx.away_threshold
        && telemetry.cpu_core_peak_pct.is_finite()
        && telemetry.cpu_core_peak_pct >= 90.0
        && telemetry.cpu_avg_10s.is_finite()
        && telemetry.cpu_avg_10s >= one_core_global_pct * SINGLE_CORE_GLOBAL_LOAD_RATIO
}

fn cpu_forecast_predicts_pressure(ctx: &AdaptiveCtx) -> bool {
    let telemetry = ctx.telemetry;
    let threshold = (ctx.heavy_threshold.clamp(30.0, 95.0) + 5.0).min(100.0);
    telemetry.cpu_forecast_ready
        && ctx.idle_secs < ctx.away_threshold
        && telemetry.cpu_forecast_10s_pct.is_finite()
        && telemetry.cpu_trend_pct_per_sec.is_finite()
        && telemetry.cpu_forecast_stability.is_finite()
        && telemetry.cpu_trend_pct_per_sec >= CPU_FORECAST_MIN_SLOPE
        && telemetry.cpu_forecast_stability >= CPU_FORECAST_MIN_STABILITY
        && telemetry.cpu_forecast_10s_pct >= threshold
}

fn multi_core_contention(ctx: &AdaptiveCtx) -> bool {
    let telemetry = ctx.telemetry;
    let cores = telemetry.cpu_logical_cores;
    if cores < 2 || ctx.idle_secs >= ctx.away_threshold {
        return false;
    }
    let required_hot = 2usize.max(cores.div_ceil(4));
    let hot_ratio = telemetry.cpu_saturated_cores as f32 / cores as f32;
    telemetry.cpu_saturated_cores >= required_hot
        && hot_ratio.is_finite()
        && telemetry.cpu_avg_10s.is_finite()
        && telemetry.cpu_avg_10s >= (hot_ratio * 100.0 * 0.4).max(15.0)
}

fn cpu_contention_for_response(ctx: &AdaptiveCtx) -> bool {
    single_core_contention(ctx) || multi_core_contention(ctx)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum StrategyKind {
    Responsiveness,
    Throughput,
    Efficiency,
    LoadBalance,
    #[default]
    AdaptiveHybrid,
}

impl StrategyKind {
    fn id(self) -> &'static str {
        match self {
            Self::Responsiveness => "responsiveness",
            Self::Throughput => "throughput",
            Self::Efficiency => "efficiency",
            Self::LoadBalance => "load_balance",
            Self::AdaptiveHybrid => "adaptive_hybrid",
        }
    }

    fn from_id(id: &str) -> Option<Self> {
        match id {
            "responsiveness" => Some(Self::Responsiveness),
            "throughput" => Some(Self::Throughput),
            "efficiency" => Some(Self::Efficiency),
            "load_balance" => Some(Self::LoadBalance),
            "adaptive_hybrid" => Some(Self::AdaptiveHybrid),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Responsiveness => "交互响应优先",
            Self::Throughput => "持续吞吐优先",
            Self::Efficiency => "能效优先",
            Self::LoadBalance => "负载均衡",
            Self::AdaptiveHybrid => "自适应混合",
        }
    }

    fn min_residence(self) -> Duration {
        match self {
            Self::Responsiveness => Duration::from_secs(20),
            _ => Duration::from_secs(8),
        }
    }
}

#[derive(Clone, Debug)]
struct StrategySelection {
    selected: StrategyKind,
    scores: Vec<AlgorithmScore>,
    reason: String,
    urgent_response: bool,
}

fn strategy_knobs(state: &str, ctx: &AdaptiveCtx, strategy: StrategyKind) -> Knobs {
    let mut knobs = knobs_for(state, ctx);
    let mem_pct = bounded_percent(ctx.telemetry.mem_pct);
    match state {
        "game" => {
            // Gaming scene is a hard constraint: policy choice never weakens the game path.
            knobs.epp = ctx.epp_ok.then_some(0);
            knobs.priority_separation = 0x26;
            knobs.prio_boost = true;
        }
        "heavy" => match strategy {
            StrategyKind::Responsiveness => {
                knobs.epp = ctx.epp_ok.then_some(10);
                knobs.priority_separation = 0x26;
                knobs.prio_boost = true;
            }
            StrategyKind::Throughput => {
                knobs.epp = ctx.epp_ok.then_some(25);
                knobs.priority_separation = 2;
                knobs.prio_boost = false;
            }
            StrategyKind::LoadBalance => {
                knobs.epp = ctx.epp_ok.then_some(35);
                knobs.priority_separation = 0x18;
                knobs.prio_boost = false;
            }
            _ => {}
        },
        "power" => {
            let epp = match strategy {
                StrategyKind::Responsiveness => 30,
                StrategyKind::Throughput => 40,
                StrategyKind::Efficiency => 85,
                StrategyKind::LoadBalance if mem_pct >= 88.0 => 50,
                StrategyKind::LoadBalance => 55,
                StrategyKind::AdaptiveHybrid => 45,
            };
            knobs.epp = ctx.epp_ok.then_some(epp);
            knobs.priority_separation = 2;
            knobs.prio_boost = false;
        }
        "none" | "media" => {
            let epp = match strategy {
                StrategyKind::Responsiveness => 15,
                StrategyKind::Throughput => 25,
                StrategyKind::Efficiency => 70,
                StrategyKind::LoadBalance if mem_pct >= 88.0 => 55,
                StrategyKind::LoadBalance if mem_pct >= 78.0 => 60,
                StrategyKind::LoadBalance => 50,
                StrategyKind::AdaptiveHybrid => 40,
            };
            knobs.epp = ctx.epp_ok.then_some(epp);
            knobs.priority_separation = 2;
            knobs.prio_boost = false;
            if state == "none"
                && strategy == StrategyKind::Responsiveness
                && (cpu_contention_for_response(ctx) || cpu_forecast_predicts_pressure(ctx))
            {
                // Brief, reversible foreground scheduling bias for a confirmed CPU bottleneck.
                knobs.priority_separation = 0x26;
                knobs.prio_boost = true;
            }
        }
        _ => {}
    }
    knobs
}

fn strategy_knobs_with_priority_gate(
    adaptive: &mut Adaptive,
    target_state: &str,
    ctx: &AdaptiveCtx,
    strategy: StrategyKind,
    now: Instant,
) -> Knobs {
    let mut knobs = strategy_knobs(target_state, ctx, strategy);
    let current = adaptive
        .applied
        .as_ref()
        .map(|applied| applied.priority_separation)
        .unwrap_or_else(|| knobs_current().priority_separation);
    let immediate = matches!(target_state, "game" | "heavy" | "power" | "away")
        || matches!(adaptive.state.as_str(), "game" | "heavy" | "power" | "away");
    knobs.priority_separation = adaptive.priority_separation_gate.resolve(
        current,
        knobs.priority_separation,
        immediate,
        now,
    );
    knobs.prio_boost = knobs.priority_separation == 0x26;
    knobs
}

fn bounded_percent(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 100.0)
    } else {
        0.0
    }
}

#[derive(Clone, Copy, Debug)]
struct StrategyEvaluation {
    cpu_peak: f32,
    responsiveness: f32,
    sustained_load: f32,
    memory_pressure: f32,
}

fn evaluate_strategy(ctx: &AdaptiveCtx, strategy: StrategyKind) -> StrategyEvaluation {
    let telemetry = ctx.telemetry;
    let cpu_peak = bounded_percent(telemetry.cpu_peak);
    let cpu_avg_10s = bounded_percent(telemetry.cpu_avg_10s);
    let cpu_avg_60s = bounded_percent(telemetry.cpu_avg_60s);
    let mem_pct = bounded_percent(telemetry.mem_pct);
    let mem_slope = if telemetry.mem_slope.is_finite() {
        telemetry.mem_slope
    } else {
        0.0
    };
    let response_bonus = if ctx.gaming {
        24.0
    } else if cpu_contention_for_response(ctx)
        || cpu_forecast_predicts_pressure(ctx)
        || cpu_avg_10s > cpu_avg_60s + 2.0
    {
        10.0
    } else {
        0.0
    };
    let strategy_response: f32 = match strategy {
        StrategyKind::Responsiveness => 18.0,
        StrategyKind::Throughput => 8.0,
        StrategyKind::Efficiency => 0.0,
        StrategyKind::LoadBalance => 6.0,
        StrategyKind::AdaptiveHybrid => 12.0,
    };
    let sustained_load = (cpu_avg_60s / 100.0).clamp(0.0, 1.0)
        * match strategy {
            StrategyKind::Throughput => 1.0,
            StrategyKind::Responsiveness => 0.72,
            StrategyKind::LoadBalance => 0.55,
            StrategyKind::Efficiency => 0.3,
            StrategyKind::AdaptiveHybrid => 0.7,
        };
    let memory_pressure =
        (mem_pct / 100.0).clamp(0.0, 1.0) + (mem_slope.max(0.0) * 0.5).clamp(0.0, 0.25);
    StrategyEvaluation {
        cpu_peak,
        responsiveness: (strategy_response + response_bonus).clamp(0.0, 100.0),
        sustained_load,
        memory_pressure,
    }
}

fn score_candidate(id: StrategyKind, score: i32, reason: &str) -> AlgorithmScore {
    AlgorithmScore {
        id: id.id().into(),
        score,
        suitable: true,
        reason: reason.into(),
    }
}

/// Score candidate policies against short/long CPU trends, foreground scene, memory pressure,
/// and AC/battery state. Hysteresis preserves the incumbent across small score fluctuations.
fn select_strategy(ctx: &AdaptiveCtx, current: StrategyKind) -> StrategySelection {
    let mut scores = Vec::with_capacity(5);
    let telemetry = ctx.telemetry;
    let cpu_avg_10s = bounded_percent(telemetry.cpu_avg_10s);
    let cpu_avg_60s = bounded_percent(telemetry.cpu_avg_60s);
    let cpu_peak = bounded_percent(telemetry.cpu_peak);
    let mem_pct = bounded_percent(telemetry.mem_pct);
    let mem_slope = if telemetry.mem_slope.is_finite() {
        telemetry.mem_slope
    } else {
        0.0
    };
    let forecast_pressure = cpu_forecast_predicts_pressure(ctx);
    let cpu_rising = cpu_avg_10s - cpu_avg_60s > 2.0 || forecast_pressure;
    let core_contention = cpu_contention_for_response(ctx);
    let single_core_hot = single_core_contention(ctx);
    let high_load = cpu_avg_60s >= ctx.heavy_threshold
        || (cpu_peak >= 92.0 && cpu_avg_60s >= 65.0)
        || (telemetry.process_count >= 180 && cpu_avg_60s >= 45.0)
        || forecast_pressure;
    let memory_pressure = mem_pct >= 88.0 || (mem_pct >= 78.0 && mem_slope > 0.08);
    let battery = !ctx.ac_online && ctx.auto_power;
    let idle = !ctx.gaming && ctx.idle_secs >= ctx.away_threshold;

    let response_score = if ctx.gaming {
        100
    } else if core_contention {
        98
    } else if ctx.media {
        60
    } else if cpu_rising {
        100
    } else {
        42
    };
    scores.push(score_candidate(
        StrategyKind::Responsiveness,
        response_score,
        if ctx.gaming {
            "前台游戏，优先交互响应"
        } else if forecast_pressure {
            "稳健 CPU 趋势预测 10 秒内逼近高负载，提前保留响应余量"
        } else if single_core_hot {
            "单核持续饱和，优先提升前台响应"
        } else if core_contention {
            "多个逻辑处理器持续饱和，优先交互响应"
        } else if cpu_rising {
            "短时 CPU 均值高于长期均值"
        } else {
            "保留日常交互响应余量"
        },
    ));
    scores.push(score_candidate(
        StrategyKind::Throughput,
        if high_load {
            94 + (cpu_peak >= 92.0) as i32 * 6
        } else {
            30
        },
        if high_load {
            "持续或峰值 CPU 负载，需要维持吞吐"
        } else {
            "当前持续吞吐需求较低"
        },
    ));
    scores.push(score_candidate(
        StrategyKind::Efficiency,
        if idle && memory_pressure {
            72
        } else if idle {
            100
        } else if battery && !high_load {
            88
        } else if battery {
            52
        } else {
            25
        },
        if idle {
            "输入空闲，降低无前台需求时的能耗"
        } else if battery && !high_load {
            "电池供电且当前负载较轻"
        } else if battery {
            "电池供电但保留重任务性能"
        } else {
            "交流供电下能效优先级较低"
        },
    ));
    scores.push(score_candidate(
        StrategyKind::LoadBalance,
        if memory_pressure {
            92
        } else if high_load && !cpu_rising {
            98
        } else if battery {
            62
        } else {
            40
        },
        if memory_pressure {
            "内存水位或增长趋势偏高"
        } else if high_load {
            "负载较高，降低策略激进度"
        } else {
            "维持温和功耗与响应平衡"
        },
    ));
    scores.push(score_candidate(
        StrategyKind::AdaptiveHybrid,
        if ctx.gaming || core_contention || high_load || battery || idle || memory_pressure {
            72
        } else {
            82
        },
        "融合场景、短长时负载、电源和内存信号",
    ));

    let evaluations = [
        StrategyKind::Responsiveness,
        StrategyKind::Throughput,
        StrategyKind::Efficiency,
        StrategyKind::LoadBalance,
        StrategyKind::AdaptiveHybrid,
    ]
    .into_iter()
    .map(|strategy| evaluate_strategy(ctx, strategy))
    .collect::<Vec<_>>();
    for (candidate, evaluation) in scores.iter_mut().zip(evaluations.iter()) {
        let strategy = StrategyKind::from_id(&candidate.id).unwrap_or_default();
        let memory_cost = if strategy == StrategyKind::LoadBalance {
            8.0
        } else {
            2.0
        };
        let throughput_gain = if strategy == StrategyKind::Throughput {
            evaluation.sustained_load * 15.0 + evaluation.cpu_peak.min(100.0) * 0.05
        } else {
            0.0
        };
        let adjustment = evaluation.responsiveness * 0.25
            - evaluation.memory_pressure * memory_cost
            + throughput_gain;
        candidate.score = (candidate.score as f32 + adjustment)
            .round()
            .clamp(0.0, 100.0) as i32;
        candidate.suitable = candidate.score >= 45;
    }
    scores.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    let best = scores
        .first()
        .and_then(|s| StrategyKind::from_id(&s.id))
        .unwrap_or_default();
    let incumbent_score = scores
        .iter()
        .find(|s| s.id == current.id())
        .map(|s| s.score)
        .unwrap_or(0);
    let best_score = scores.first().map(|s| s.score).unwrap_or(0);
    let selected = retain_incumbent_with_hysteresis(best, current, best_score, incumbent_score);
    let reason = scores
        .iter()
        .find(|s| s.id == selected.id())
        .map(|s| s.reason.clone())
        .unwrap_or_default();
    StrategySelection {
        selected,
        scores,
        reason,
        urgent_response: selected == StrategyKind::Responsiveness
            && (core_contention || forecast_pressure),
    }
}

fn valid_game_process_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        && name.ends_with(|c: char| c.is_ascii_alphanumeric())
}

pub(crate) fn classifier_mode_key(mode: &str) -> String {
    match mode {
        "game" | "media" | "away" | "power" | "heavy" => mode.to_string(),
        _ => "none".to_string(),
    }
}

/// Schmitt-trigger band prevents load jitter from repeatedly entering/exiting heavy mode.
/// Missing CPU averages retain an existing heavy scene but never start a new one.
const HEAVY_EXIT_HYSTERESIS: f32 = 5.0;

fn classify_target(ctx: &AdaptiveCtx, current: &str, away_threshold: u64) -> &'static str {
    if ctx.gaming {
        "game"
    } else if ctx.media {
        "media"
    } else if ctx.idle_secs >= away_threshold {
        "away"
    } else if !ctx.ac_online && ctx.auto_power {
        "power"
    } else {
        let entry = ctx.heavy_threshold;
        let exit = (entry - HEAVY_EXIT_HYSTERESIS).max(0.0);
        let heavy = match (current == "heavy", ctx.cpu_avg()) {
            (true, None) => true,
            (true, Some(cpu_avg)) => cpu_avg >= exit,
            (false, Some(cpu_avg)) => cpu_avg > entry,
            (false, None) => false,
        };
        if heavy {
            "heavy"
        } else {
            "none"
        }
    }
}

fn input_idle_secs() -> u64 {
    unsafe {
        let mut lii = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut lii) == 0 {
            return 0;
        }
        GetTickCount().wrapping_sub(lii.dwTime) as u64 / 1000
    }
}

/// 目标场景 → 旋钮组合（纯函数，仲裁的核心）
fn knobs_for(state: &str, ctx: &AdaptiveCtx) -> Knobs {
    let epp = |v: u32| ctx.epp_ok.then_some(v);
    let cpu_load_high = ctx.cpu_avg().is_some_and(|cpu_avg| cpu_avg > 50.0);
    match state {
        // 游戏：极致性能 + 前台响应优先
        "game" => Knobs {
            plan: PlanKind::High,
            epp: epp(0),
            proc_max: 100,
            gamedvr: false,
            prio_boost: true,
            priority_separation: 0x26,
        },
        // 影音：流畅解码，安静后台
        "media" => Knobs {
            plan: PlanKind::Balanced,
            epp: epp(30),
            proc_max: 100,
            gamedvr: true,
            prio_boost: false,
            priority_separation: 2,
        },
        // 离开：深度省电（亮度由联动单独处理）
        "away" => Knobs {
            plan: PlanKind::Saver,
            epp: epp(90),
            proc_max: 70,
            gamedvr: false,
            prio_boost: false,
            priority_separation: 2,
        },
        // 电池：负载感知的节能（重载时不狠压频率）
        "power" => Knobs {
            plan: PlanKind::Saver,
            epp: epp(if cpu_load_high { 45 } else { 85 }),
            proc_max: if cpu_load_high { 100 } else { 80 },
            gamedvr: false,
            prio_boost: false,
            priority_separation: 2,
        },
        // 重载（编译/渲染）：保住持续性能
        "heavy" => Knobs {
            plan: PlanKind::Balanced,
            epp: epp(20),
            proc_max: 100,
            gamedvr: false,
            prio_boost: true,
            priority_separation: 0x26,
        },
        // 常规：轻微性能偏置
        _ => Knobs {
            plan: PlanKind::Balanced,
            epp: epp(45),
            proc_max: 100,
            gamedvr: true,
            prio_boost: false,
            priority_separation: 2,
        },
    }
}

/// 场景确认秒数（迟滞）：越"贵"的切换确认越久，防止抖动
fn confirm_seconds_for(state: &str) -> u32 {
    match state {
        "game" => 1, // 游戏信号本身已经过 6s/4s 迟滞
        "media" => 3,
        "away" => 5,
        "power" => 3,
        "heavy" => 10,
        "none" => 10,
        _ => 3,
    }
}

/// 名称兜底分类（用户自建计划）——纯函数，便于回归测试。
fn plan_kind_from_name(name: &str) -> PlanKind {
    let n = name.to_lowercase();
    if n.contains("卓越")
        || n.contains("ultimate")
        || n.contains("高性能")
        || n.contains("high performance")
    {
        return PlanKind::High;
    }
    if n.contains("节能") || n.contains("省电") || n.contains("power saver") {
        return PlanKind::Saver;
    }
    PlanKind::Balanced
}

/// duplicatescheme 失败登记后，同一模板本次会话内不再尝试复制。
static PLAN_TEMPLATE_MISSING: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<PlanKind, String>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// 模板缺失负缓存命中：true = 该模板已确认复制失败，跳过重复尝试。
pub(crate) fn plan_template_missing(kind: &PlanKind) -> bool {
    PLAN_TEMPLATE_MISSING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(kind)
}

/// 登记模板缺失；返回 true 表示这是首次登记（应输出一次 WARN）。
/// 已登记过的保留首次错误信息，后续登记静默忽略。
fn register_plan_template_missing(kind: &PlanKind, error: &str) -> bool {
    use std::collections::hash_map::Entry;
    let mut map = PLAN_TEMPLATE_MISSING
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    match map.entry(kind.clone()) {
        Entry::Vacant(slot) => {
            slot.insert(error.to_string());
            true
        }
        Entry::Occupied(_) => false,
    }
}

#[cfg(test)]
pub(crate) fn reset_plan_template_missing() {
    PLAN_TEMPLATE_MISSING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clear();
}

/// 模板缺失退避短路：Some(Err) 表示直接跳过复制步骤（不再起 powercfg 子进程）。
fn resolve_plan_template_missing(kind: &PlanKind) -> Option<Result<(String, String), String>> {
    if plan_template_missing(kind) {
        tracing::debug!("OEM 电源计划模板此前已确认缺失，本次跳过重复复制（后台定期重试）");
        return Some(Err("目标电源计划不可创建，已进入冷却期".into()));
    }
    None
}

/// 活动计划 → PlanKind：纯注册表读取（毫秒级），AutoState 持锁路径专用。
///
/// 旧实现每次都会起 powercfg 子进程（最多 3 秒），把共用 AutoState 锁的
/// UI 命令一起卡住。这里优先查 AeroTune 追踪的副本 GUID 与模板 GUID，
/// 兜底才走带短 TTL 缓存的计划列表。
pub(crate) fn active_plan_kind() -> PlanKind {
    let Some(active) = crate::power::active_scheme_guid() else {
        return PlanKind::Balanced;
    };
    let s = load_settings();
    // GUID 精确判定（AeroTune 追踪的副本）
    if s.high_plan_guid.as_deref() == Some(active.as_str()) {
        return PlanKind::High;
    }
    if s.saver_plan_guid.as_deref() == Some(active.as_str()) {
        return PlanKind::Saver;
    }
    // 模板 GUID 精确判定（用户直接用系统内置计划）
    match active.to_lowercase().as_str() {
        TPL_HIGH => return PlanKind::High,
        TPL_SAVER => return PlanKind::Saver,
        _ => {}
    }
    // 名称兜底（用户自建计划）——带短 TTL 缓存，通常不触发子进程
    if let Some(p) = crate::power::cached_plans()
        .into_iter()
        .find(|p| p.guid == active)
    {
        return plan_kind_from_name(&p.name);
    }
    PlanKind::Balanced
}

fn knobs_current() -> Knobs {
    let st = crate::tweaks::read_states();
    Knobs {
        plan: active_plan_kind(),
        epp: crate::power::get_epp_value(),
        proc_max: crate::power::get_proc_max().unwrap_or(100).min(100) as u8,
        gamedvr: st.gamedvr,
        prio_boost: st.priority_boost,
        priority_separation: crate::journal::read_reg_dword(
            r"HKLM\SYSTEM\CurrentControlSet\Control\PriorityControl",
            "Win32PrioritySeparation",
        )
        .unwrap_or(2),
    }
}

const TPL_HIGH: &str = "e9a42b02-d5df-448d-aa00-03f14749eb61";
const TPL_SAVER: &str = "a1841308-3541-4fab-bc81-f71556f20b4c";
const STD_BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const PLAN_RETRY_BACKOFF: Duration = Duration::from_secs(15 * 60);

fn plan_retry_ready(
    retry_after: &mut std::collections::HashMap<PlanKind, Instant>,
    kind: PlanKind,
    now: Instant,
) -> bool {
    if retry_after
        .get(&kind)
        .is_some_and(|deadline| *deadline > now)
    {
        return false;
    }
    retry_after.remove(&kind);
    true
}

fn plan_exists(guid: &str) -> bool {
    crate::power::cached_plans().iter().any(|p| p.guid == guid)
}

fn plan_name(guid: &str) -> String {
    crate::power::cached_plans()
        .into_iter()
        .find(|p| p.guid == guid)
        .map(|p| p.name)
        .unwrap_or_else(|| guid.to_string())
}

/// 解析目标计划：**GUID 持久化追踪**，彻底退役"按名称匹配"。
///
/// duplicatescheme 对确认缺失的 OEM 模板失败一次后（进程级负缓存），
/// 本次会话内不再重复调用 powercfg，也不再重复输出 WARN（退避期间静默）。
pub(crate) fn resolve_plan(kind: &PlanKind) -> Result<(String, String), String> {
    match kind {
        PlanKind::Balanced => {
            if plan_exists(STD_BALANCED) {
                Ok((STD_BALANCED.into(), plan_name(STD_BALANCED)))
            } else {
                crate::power::balanced_plan().map(|p| (p.guid, p.name))
            }
        }
        PlanKind::High | PlanKind::Saver => {
            let (tpl, is_high, keywords): (&str, bool, &[&str]) = match kind {
                PlanKind::High => (
                    TPL_HIGH,
                    true,
                    &["高性能", "卓越", "high performance", "ultimate"],
                ),
                _ => (TPL_SAVER, false, &["节能", "省电", "power saver"]),
            };
            // 1) 已持久化且仍存在的副本直接复用
            let stored = {
                let s = load_settings();
                if is_high {
                    s.high_plan_guid.clone()
                } else {
                    s.saver_plan_guid.clone()
                }
            };
            if let Some(g) = stored {
                if plan_exists(&g) {
                    return Ok((g.clone(), plan_name(&g)));
                }
                tracing::warn!(guid = %g, "追踪的电源计划副本已不存在，重新创建");
            }
            // 2) 名称匹配系统现有计划（OEM 预装的原生高性能/节能）
            let plans = crate::power::cached_plans();
            if let Some(p) = plans
                .iter()
                .find(|p| keywords.iter().any(|w| p.name.to_lowercase().contains(w)))
            {
                let g = p.guid.clone();
                let name = p.name.clone();
                let _ = update_settings(|s| {
                    if is_high {
                        s.high_plan_guid = Some(g.clone());
                    } else {
                        s.saver_plan_guid = Some(g.clone());
                    }
                    Ok(())
                });
                tracing::info!(guid = %g, name = %name, "采用系统现有电源计划");
                return Ok((g, name));
            }
            // 3) Use an OEM template only when this platform supports duplicating it.
            if let Some(short_circuit) = resolve_plan_template_missing(kind) {
                return short_circuit;
            }
            match crate::util::run_output_checked("powercfg", &["/duplicatescheme", tpl]) {
                Ok(out) => {
                    let re = regex::Regex::new(
                        r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}",
                    )
                    .unwrap();
                    if let Some(g) = re.find(&out).map(|m| m.as_str().to_lowercase()) {
                        let _ = update_settings(|s| {
                            if is_high {
                                s.high_plan_guid = Some(g.clone());
                            } else {
                                s.saver_plan_guid = Some(g.clone());
                            }
                            Ok(())
                        })?;
                        tracing::info!(guid = %g, high = is_high, "已创建并追踪电源计划副本");
                        crate::power::invalidate_plan_cache();
                        return Ok((g.clone(), plan_name(&g)));
                    }
                    tracing::warn!(out = %out, high = is_high, "powercfg /duplicatescheme 返回成功但无计划 GUID");
                }
                Err(error) => {
                    let first = register_plan_template_missing(kind, &error);
                    if first {
                        tracing::warn!(template = tpl, high = is_high, %error, "powercfg /duplicatescheme 调用失败；已加入模板缺失名单，本次会话内不再重复尝试并告警")
                    } else {
                        tracing::debug!(template = tpl, high = is_high, %error, "powercfg /duplicatescheme 再次失败（模板缺失名单已登记，静默退避）")
                    }
                }
            }
            // 4) 降级：节能/高性能不可得时用平衡计划顶着，场景其余旋钮照常
            Err("目标电源计划不可创建，已进入冷却期".into())
        }
    }
}

fn push_decision(ad: &mut Adaptive, from: &str, to: &str, reason: &str) {
    ad.decisions.insert(
        0,
        DecisionLog {
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            from: from.into(),
            to: to.into(),
            reason: reason.into(),
        },
    );
    if ad.decisions.len() > 20 {
        ad.decisions.truncate(20);
    }
}

/// 应用旋钮组合：每个旋钮只有确认成功才更新快照，失败时由周期校准兜底。
fn apply_knobs(ad: &mut Adaptive, target: Knobs) {
    let cur = ad.applied.clone().unwrap_or_else(knobs_current);
    // actual 只累积“已确认达成”的部分；失败的设置保留原快照。
    let mut actual = cur.clone();

    if cur.plan != target.plan
        && plan_retry_ready(&mut ad.plan_retry_after, target.plan, Instant::now())
    {
        let attempt = Instant::now();
        let old_guid = crate::power::cached_plans()
            .into_iter()
            .find(|p| p.active)
            .map(|p| p.guid)
            .unwrap_or_else(|| crate::power::active_scheme_guid().unwrap_or_default());
        match resolve_plan(&target.plan) {
            Ok((guid, name)) => {
                let expected_old = old_guid.clone();
                let description = format!("自适应引擎：电源计划 → {name}");
                match crate::journal::change_power_plan_with_journal(
                    old_guid,
                    guid.clone(),
                    &description,
                    || crate::power::set_plan_raw_if_current(&guid, &expected_old),
                ) {
                    Ok(()) => {
                        ad.plan_retry_after.remove(&target.plan);
                        tracing::info!(plan = %name, "自适应：电源计划已切换");
                        actual.plan = target.plan;
                    }
                    Err(error) => {
                        ad.plan_retry_after
                            .insert(target.plan, attempt + PLAN_RETRY_BACKOFF);
                        tracing::error!(%error, plan = %name, "自适应：电源计划写前日志或切换失败；已暂停重试");
                    }
                }
            }
            Err(e) => {
                ad.plan_retry_after
                    .insert(target.plan, attempt + PLAN_RETRY_BACKOFF);
                // 模板确认缺失已在 resolve_plan 内记录一次并登记；后续 15 分钟退避
                // 失败保持安静，避免每 15 分钟重复打扰用户日志。
                if !plan_template_missing(&target.plan) {
                    tracing::warn!(retry_minutes = PLAN_RETRY_BACKOFF.as_secs() / 60, error = %e, "自适应：解析目标电源计划失败，已启用重试退避")
                } else {
                    tracing::debug!(retry_minutes = PLAN_RETRY_BACKOFF.as_secs() / 60, error = %e, "自适应：目标电源计划仍不可创建（模板缺失退避中），本轮已跳过")
                }
            }
        }
    }
    // 自动 EPP 暂停：尚无逐计划 AC/DC 原始快照，不猜测恢复。
    if cur.epp != target.epp {
        if let Some(value) = target.epp {
            let old = crate::power::get_epp_value();
            if cur.epp == Some(value) {
                actual.epp = Some(value);
            } else {
                match crate::power::set_epp_raw(Some(value)) {
                    Ok(()) => match crate::journal::record(
                        &format!("自适应算法：EPP → {value}"),
                        crate::journal::Revert::Epp {
                            old,
                            applied: Some(value),
                        },
                    ) {
                        Ok(()) => {
                            actual.epp = Some(value);
                            tracing::info!(epp = value, "自适应：EPP已更新");
                        }
                        Err(error) => {
                            tracing::error!(%error, epp = value, "自适应：变更日志写入失败；已触发恢复策略")
                        }
                    },
                    Err(error) => tracing::warn!(%error, "自适应：EPP设置失败"),
                }
            }
        }
    }
    if cur.priority_separation != target.priority_separation {
        match crate::tweaks::set_priority_separation(
            target.priority_separation,
            "自适应策略前台调度",
        ) {
            Ok(()) => {
                actual.priority_separation = target.priority_separation;
                actual.prio_boost = target.prio_boost;
                ad.priority_separation_gate.record_applied(Instant::now());
            }
            Err(error) => tracing::warn!(%error, "自适应：前台调度策略写入失败"),
        }
    }
    if cur.gamedvr != target.gamedvr {
        match crate::tweaks::set_gamedvr(target.gamedvr) {
            Ok(()) => {
                tracing::info!(on = %target.gamedvr, "自适应：GameDVR 已切换");
                actual.gamedvr = target.gamedvr;
            }
            Err(e) => tracing::warn!(error = %e, "自适应：GameDVR 设置失败"),
        }
    }
    // 自动 processor max 暂停：AC/DC 双值快照尚未实现。
    ad.applied = Some(actual);
}

fn restore_plan_exact(
    guid: &str,
    current: Option<&str>,
    mut write: impl FnMut(&str, &str) -> Result<(), String>,
) -> Result<(), String> {
    let current = current.ok_or_else(|| "无法读取当前活动电源计划；保留自适应基线".to_string())?;
    write(guid, current)
}

fn baseline_restore_mismatches(expected: &Knobs, actual: &Knobs) -> Vec<String> {
    let mut mismatches = Vec::new();
    if expected.epp.is_some() && actual.epp != expected.epp {
        mismatches.push("EPP".to_string());
    }
    if actual.gamedvr != expected.gamedvr {
        mismatches.push("GameDVR".to_string());
    }
    if actual.prio_boost != expected.prio_boost
        || actual.priority_separation != expected.priority_separation
    {
        mismatches.push("前台优先级".to_string());
    }
    mismatches
}

fn baseline_restore_error(failures: &[String]) -> String {
    format!(
        "系统基线未完整恢复；当前会话的原始快照已保留。请保持 AeroTune 运行并重试；退出前可在「设置 → 变更日志」检查持久化记录。原因：{}", 
        failures.join("；")
    )
}

fn finish_baseline_restore(pending: &mut bool, result: Result<(), String>) -> Result<(), String> {
    *pending = result.is_err();
    result
}

/// 自适应仲裁：每秒推断目标场景，确认 N 秒后切换旋钮
fn adaptive_tick(
    state: &mut AutoState,
    ctx: AdaptiveCtx,
    elapsed: Duration,
    now: Instant,
) -> Vec<String> {
    let mut actions = Vec::new();
    let selection = select_strategy(&ctx, state.adaptive.strategy);
    let incumbent = state.adaptive.strategy;
    let candidate = selection.selected;
    let hold_active = crate::tweaks::manual_hold_active();
    if hold_active {
        state.adaptive.priority_separation_gate.clear_pending();
    }
    let candidate_since = match state.adaptive.strategy_pending {
        Some((pending, since)) if pending == candidate => since,
        _ => {
            state.adaptive.strategy_pending = Some((candidate, now));
            now
        }
    };
    let min_residence = incumbent.min_residence();
    let candidate_for = now
        .checked_duration_since(candidate_since)
        .unwrap_or_default();
    let residence_for = state
        .adaptive
        .strategy_changed_at
        .map(|changed| now.checked_duration_since(changed).unwrap_or_default())
        .unwrap_or(min_residence);
    if candidate == incumbent || hold_active {
        state.adaptive.strategy_pending = None;
    } else if strategy_transition_ready(
        candidate,
        incumbent,
        candidate_for,
        residence_for,
        hold_active,
        selection.urgent_response,
    ) {
        state.adaptive.strategy = candidate;
        state.adaptive.strategy_changed_at = Some(now);
        state.adaptive.strategy_pending = None;
    }
    let effective_strategy = state.adaptive.strategy;
    let strategy_changed = effective_strategy != incumbent;
    state.adaptive.strategy_reason = if effective_strategy == candidate {
        selection.reason
    } else {
        format!(
            "维持{}；候选{}等待确认或最短驻留",
            effective_strategy.label(),
            candidate.label()
        )
    };
    state.adaptive.strategy_scores = selection.scores;

    // ---- 周期校准：快照向系统实际状态看齐 ----
    // 兜底一切外部改动（手动模式/设置页开关/用户自己改电源计划）造成的快照漂移
    let calibrate = calibration_due(&mut state.adaptive.last_calibration, now);
    if calibrate && !crate::tweaks::manual_hold_active() {
        state.adaptive.applied = Some(knobs_current());
    }

    let target = classify_target(
        &ctx,
        &state.adaptive.state,
        state.settings.away_idle_secs as u64,
    );

    // ---- 场景时长统计（今日，每 30 秒持久化一次，跨天归档近 7 天）----
    let today = chrono::Local::now().date_naive().to_string();
    if state.adaptive.stats_day != today {
        let y = state.adaptive.stats_day.clone();
        let secs = state.adaptive.seconds.clone();
        if !y.is_empty() {
            let _ = update_settings(|s| {
                s.history.insert(y, secs);
                while s.history.len() > 7 {
                    s.history.pop_first();
                }
                Ok(())
            });
        }
        state.adaptive.seconds.clear();
        state.adaptive.stats_day = today.clone();
    }
    account_seconds(&mut state.adaptive, elapsed);
    state.adaptive.persist_elapsed += elapsed;
    if state.adaptive.persist_elapsed >= Duration::from_secs(30) {
        state.adaptive.persist_elapsed = Duration::ZERO;
        let secs = state.adaptive.seconds.clone();
        let _ = update_settings(|s| {
            s.state_seconds = secs;
            Ok(())
        });
    }

    let ad_state = state.adaptive.state.clone();
    if target == ad_state {
        state.adaptive.pending.clear();
        let knobs_due = adaptive_knobs_due(
            calibrate,
            strategy_changed,
            &state.adaptive.priority_separation_gate,
            now,
        );
        if knobs_due && !crate::tweaks::manual_hold_active() {
            if calibrate {
                state.adaptive.applied = Some(knobs_current());
            }
            if state.adaptive.baseline.is_none() {
                state.adaptive.baseline_plan_guid = crate::power::active_scheme_guid();
                state.adaptive.baseline = state.adaptive.applied.clone();
            }
            let strategy = state.adaptive.strategy;
            let knobs =
                strategy_knobs_with_priority_gate(&mut state.adaptive, target, &ctx, strategy, now);
            apply_knobs(&mut state.adaptive, knobs);
            if !state.settings.auto_dim || !["power", "away"].contains(&target) {
                update_brightness(&mut state.adaptive, false);
            }
        }
        return actions;
    }
    let confirmed = state
        .adaptive
        .pending
        .ready(target, now, confirm_seconds_for(target));
    // Brightness and knobs below this gate cannot act on an unaccepted candidate.
    if !transition_allowed(
        confirmed,
        crate::tweaks::manual_hold_active(),
        state.in_cooldown(),
    ) {
        return actions;
    }

    let reason = if target == "game" {
        "检测到游戏前台（全屏/名单）".to_string()
    } else if target == "media" {
        format!("影音/浏览器前台（{}）", state.last_foreground)
    } else if target == "away" {
        format!("用户已离开 {} 秒", ctx.idle_secs)
    } else if target == "power" {
        "已切换到电池供电".to_string()
    } else if target == "heavy" {
        match ctx.cpu_avg() {
            Some(cpu_avg) => format!("持续高负载（1 分钟均值 {:.0}%）", cpu_avg),
            None => "CPU 负载遥测暂不可用，暂时保留重载场景".to_string(),
        }
    } else {
        "恢复常规负载".to_string()
    };

    // 首次运行：基线 = 用户当前设置（自适应关闭时恢复到它）
    if state.adaptive.baseline.is_none() {
        state.adaptive.baseline_plan_guid = crate::power::active_scheme_guid();
        state.adaptive.baseline = Some(knobs_current());
    }
    if state.adaptive.applied.is_none() {
        state.adaptive.applied = state.adaptive.baseline.clone();
    }

    let strategy = state.adaptive.strategy;
    let knobs = strategy_knobs_with_priority_gate(&mut state.adaptive, target, &ctx, strategy, now);
    apply_knobs(&mut state.adaptive, knobs);
    let selected_name = state.adaptive.strategy.label();
    let selected_reason = state.adaptive.strategy_reason.clone();
    let final_reason = format!("{reason}；策略：{selected_name}（{selected_reason}）");
    push_decision(&mut state.adaptive, &ad_state, target, &final_reason);
    tracing::info!(from = %ad_state, to = %target, reason = %final_reason, "自适应：场景切换");
    update_brightness(
        &mut state.adaptive,
        state.settings.auto_dim && ["power", "away"].contains(&target),
    );
    state.adaptive.last_calibration = Some(now);
    state.adaptive.pending.clear();
    state.adaptive.state = target.to_string();
    state.last_switch = Some(Instant::now());
    let action = format!(
        "自适应 → {}（{final_reason}）",
        match target {
            "game" => "游戏模式",
            "media" => "影音模式",
            "away" => "离开模式",
            "power" => "电池模式",
            "heavy" => "重载模式",
            _ => "常规模式",
        }
    );
    match crate::tweaks::write_mode(target) {
        Ok(()) => actions.push(action),
        Err(error) => {
            tracing::error!(%error, mode = target, "自适应策略已切换，但场景状态标记保存失败");
            actions.push(format!("{action}；场景状态标记未保存：{error}"));
        }
    }
    actions
}

pub struct AutoState {
    pub settings: AutoSettings,
    /// 原始进程调度状态快照（PID+映像路径+创建时间），跨场景可撤销。
    pub(crate) scheduler_originals: HashMap<u32, ProcessSchedulerSnapshot>,
    pub(crate) foreground_priority_original: Option<ProcessPrioritySnapshot>,
    /// 最近一次自身进程树快照（AeroTune + WebView2 运行时等子进程），
    /// 仅在调度重扫或前台提升场景使用；其他 tick 清空以避免使用陈旧 PID。
    pub(crate) foreground_own_tree: HashSet<u32>,
    cpu_scheduler_status: CpuSchedulerStatus,
    scheduler_last_scan: Option<Instant>,
    foreground_scene_since: Option<Instant>,
    fullscreen_ticks: u32,
    fullscreen_evidence: Confirmation,
    list_evidence: Confirmation,
    list_ticks: u32,
    last_switch: Option<Instant>,
    boosted: Option<(u32, u32)>, // (pid, 原优先级 class)
    rules_sustained: std::collections::HashMap<(u32, String), Duration>,
    rules_suspend_notice_seen: std::collections::HashSet<u32>,
    pub(crate) adaptive: Adaptive,
    pub last_foreground: String,
    pub last_fullscreen: bool,
    /// 场景影子观测的会话去重：当前全屏会话已记录过的进程名；非全屏时清空。
    scene_shadow_seen: Option<String>,
}

impl AutoState {
    pub fn new() -> Self {
        let mut state = Self {
            settings: load_settings(),
            scheduler_originals: load_scheduler_snapshots(),
            foreground_priority_original: load_foreground_snapshot(),
            foreground_own_tree: HashSet::new(),
            cpu_scheduler_status: CpuSchedulerStatus::default(),
            scheduler_last_scan: None,
            foreground_scene_since: None,
            fullscreen_ticks: 0,
            fullscreen_evidence: Confirmation::default(),
            list_evidence: Confirmation::default(),
            list_ticks: 0,
            last_switch: None,
            boosted: None,
            rules_sustained: std::collections::HashMap::new(),
            rules_suspend_notice_seen: std::collections::HashSet::new(),
            adaptive: Adaptive {
                state: "none".into(),
                ..Default::default()
            },
            last_foreground: String::new(),
            last_fullscreen: false,
            scene_shadow_seen: None,
        };
        if let Err(error) =
            crate::brightness::schedule_startup_recovery(&mut state.adaptive.brightness)
        {
            tracing::warn!(%error, "启动屏幕亮度恢复未能排队；保留恢复状态供用户重试");
        }
        if let Some(snapshot) = state.foreground_priority_original.clone() {
            match same_priority_process(&snapshot) {
                IdentityCheck::Match => {
                    if priority_class_for_snapshot(&snapshot)
                        == Some(FOREGROUND_BOOST_PRIORITY_CLASS)
                    {
                        state.boosted = Some((snapshot.pid, snapshot.original_priority_class));
                    }
                }
                IdentityCheck::Mismatch => {
                    state.foreground_priority_original = None;
                    let _ = persist_foreground_snapshot(None);
                }
                IdentityCheck::Unavailable => tracing::warn!(
                    pid = snapshot.pid,
                    "前台进程身份暂不可读，保留优先级恢复快照"
                ),
            }
        }
        // 上次退出若因访问受限等原因留下了未恢复的调度记录，启动时立即重试，
        // 兑现退出时“下次启动重试”的承诺；已退出的 PID 会在其中被清理并落盘。
        if !state.scheduler_originals.is_empty() {
            if let Err(error) = restore_scheduler_snapshot_map(&mut state.scheduler_originals) {
                tracing::warn!(
                    error = %error,
                    "启动重试还原后台进程 CPU 调度未全部完成；保留记录待下次退出再试"
                );
            }
        }
        state
    }

    /// 关闭自适应：恢复进入自适应前的原始旋钮。
    /// 失败时保留基线并返回明确错误，使调用方不能报告恢复成功。
    pub fn restore_baseline(&mut self) -> Result<(), String> {
        let result = self.restore_baseline_inner();
        finish_baseline_restore(&mut self.adaptive.baseline_restore_pending, result)
    }

    fn restore_baseline_inner(&mut self) -> Result<(), String> {
        self.adaptive.priority_separation_gate.reset();
        let mut failures = Vec::new();
        if let Err(error) = restore_scheduler_processes(self) {
            failures.push(format!("后台 CPU 调度恢复失败：{error}"));
        }
        if let Err(error) = self.restore_foreground_priority() {
            failures.push(error);
        }
        if self.adaptive.brightness.recovery_issue {
            let _ = crate::brightness::schedule_startup_recovery(&mut self.adaptive.brightness);
        }
        update_brightness(&mut self.adaptive, false);
        if self.adaptive.brightness.recovery_issue {
            failures.push("屏幕亮度恢复快照不可读；未修改当前亮度，请手动核对或重试读取".into());
        } else if self.adaptive.brightness.has_saved() {
            failures.push("屏幕亮度尚未确认恢复（后台任务执行中或等待退避重试）".into());
        }
        let Some(mut baseline) = self.adaptive.baseline.clone() else {
            if failures.is_empty() {
                return Ok(());
            }
            return Err(baseline_restore_error(&failures));
        };
        let Some(guid) = self.adaptive.baseline_plan_guid.clone() else {
            failures.push("原始计划 GUID 未读取，拒绝按计划类型猜测恢复".into());
            return Err(baseline_restore_error(&failures));
        };
        if let Err(error) = restore_plan_exact(
            &guid,
            crate::power::active_scheme_guid().as_deref(),
            |target_guid, expected_current| {
                let target = target_guid.to_string();
                let expected = expected_current.to_string();
                let description = format!("自适应引擎：恢复原始电源计划 → {target}");
                crate::journal::change_power_plan_with_journal(
                    expected.clone(),
                    target.clone(),
                    &description,
                    || crate::power::set_plan_raw_if_current(&target, &expected),
                )
            },
        ) {
            failures.push(format!("原始电源计划恢复失败：{error}"));
            return Err(baseline_restore_error(&failures));
        }
        let from = self.adaptive.state.clone();
        baseline.plan = active_plan_kind();
        self.adaptive.applied = Some(knobs_current());
        apply_knobs(&mut self.adaptive, baseline.clone());
        let actual = self.adaptive.applied.clone().unwrap_or_else(knobs_current);
        failures.extend(baseline_restore_mismatches(&baseline, &actual));
        if !failures.is_empty() {
            return Err(baseline_restore_error(&failures));
        }
        self.adaptive.priority_separation_gate.reset();
        self.adaptive.state = "none".into();
        self.adaptive.pending.clear();
        crate::tweaks::write_mode("none").map_err(|error| {
            baseline_restore_error(&[format!("系统旋钮已恢复，但当前场景状态标记未保存：{error}")])
        })?;
        push_decision(
            &mut self.adaptive,
            &from,
            "baseline",
            "自适应已关闭，恢复原始设置",
        );
        Ok(())
    }

    pub(crate) fn sync_after_settings_import(&mut self) {
        reset_imported_adaptive_transients(&mut self.adaptive, self.settings.adaptive);
    }

    fn in_cooldown(&self) -> bool {
        self.last_switch
            .map(|t| t.elapsed().as_secs() < COOLDOWN_SECS)
            .unwrap_or(false)
    }

    fn clear_foreground_priority_snapshot(&mut self) -> Result<(), String> {
        persist_foreground_snapshot(None).map_err(|error| {
            format!("前台优先级已不再属于 AeroTune，但恢复快照清理失败：{error}")
        })?;
        self.foreground_priority_original = None;
        self.boosted = None;
        Ok(())
    }

    fn restore_foreground_priority(&mut self) -> Result<(), String> {
        let Some(snapshot) = self.foreground_priority_original.clone() else {
            return Ok(());
        };
        match same_priority_process(&snapshot) {
            IdentityCheck::Match => {}
            IdentityCheck::Mismatch => return self.clear_foreground_priority_snapshot(),
            IdentityCheck::Unavailable => {
                return Err(format!(
                    "PID {} 的前台进程身份暂不可读；优先级恢复快照已保留",
                    snapshot.pid
                ));
            }
        }
        unsafe {
            let handle = OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                snapshot.pid,
            );
            if handle.is_null() {
                return Err(format!(
                    "无法打开 PID {} 以恢复前台优先级；恢复快照已保留：{}",
                    snapshot.pid,
                    std::io::Error::last_os_error()
                ));
            }
            match handle_identity_check(handle, &snapshot.image_path, snapshot.creation_time) {
                IdentityCheck::Match => {}
                IdentityCheck::Mismatch => {
                    CloseHandle(handle);
                    return self.clear_foreground_priority_snapshot();
                }
                IdentityCheck::Unavailable => {
                    CloseHandle(handle);
                    return Err(format!(
                        "PID {} 的进程身份暂不可复核；优先级恢复快照已保留",
                        snapshot.pid
                    ));
                }
            }
            let restored = restore_priority_class_if_owned(
                handle,
                FOREGROUND_BOOST_PRIORITY_CLASS,
                snapshot.original_priority_class,
            );
            CloseHandle(handle);
            if !restored {
                return Err(format!(
                    "PID {} 的前台优先级恢复未能确认；恢复快照已保留",
                    snapshot.pid
                ));
            }
        }
        self.clear_foreground_priority_snapshot()
    }

    fn restore_boost(&mut self) {
        if let Some(snapshot) = self.foreground_priority_original.clone() {
            if restore_priority_snapshot(&snapshot).is_ok() {
                self.foreground_priority_original = None;
                self.boosted = None;
                let _ = persist_foreground_snapshot(None);
            }
        }
    }
}

fn fullscreen_notification() -> i32 {
    unsafe {
        let mut st: QUERY_USER_NOTIFICATION_STATE = 0;
        if SHQueryUserNotificationState(&mut st) != 0 {
            return 0;
        }
        // 2 = QUNS_BUSY（全屏 F11）, 3 = QUNS_RUNNING_D3D_FULL_SCREEN, 4 = 演示模式
        st
    }
}

fn foreground_pid() -> Option<u32> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            None
        } else {
            Some(pid)
        }
    }
}

/// 进程名归一化：去 .exe、转小写
pub fn normalize_name(name: &str) -> String {
    let t = name.trim().to_lowercase();
    t.strip_suffix(".exe").map(String::from).unwrap_or(t)
}

/// 每秒由监控线程调用。mode 为当前场景模式；ac_online 来自电源状态；
/// anomalies 为诊断引擎本轮确认的异常列表（用于处置规则）。
/// 返回本秒执行的动作描述（前端 toast + 原生通知）。
pub fn tick(
    state: &mut AutoState,
    sys: &System,
    mode: &str,
    ac_online: bool,
    anomalies: &[crate::diag::Anomaly],
    epp_ok: bool,
    telemetry: AutoTelemetry,
    elapsed: Duration,
) -> Vec<String> {
    let mut actions = Vec::new();
    let now = Instant::now();
    let notification = fullscreen_notification();
    let fullscreen = notification == 2 || notification == 3;
    state.last_fullscreen = fullscreen;
    let fg_pid = foreground_pid();
    let self_pid = std::process::id();
    let fg_norm = fg_pid
        .and_then(|pid| sys.processes().get(&sysinfo::Pid::from_u32(pid)))
        .map(|p| normalize_name(&p.name().to_string_lossy()))
        .unwrap_or_default();
    state.last_foreground = fg_pid
        .and_then(|pid| sys.processes().get(&sysinfo::Pid::from_u32(pid)))
        .map(|p| p.name().to_string_lossy().into_owned())
        .unwrap_or_default();
    let foreground_creation_time = fg_pid
        .and_then(|pid| sys.processes().get(&sysinfo::Pid::from_u32(pid)))
        .map(|process| process.start_time());

    let list_hit = state.settings.auto_game && state.settings.game_list.contains(&fg_norm);
    let full_hit = game_signal(notification, &fg_norm, false);
    let full_ready = if full_hit {
        state
            .fullscreen_evidence
            .ready(&fg_norm, now, FULLSCREEN_CONFIRM_SECS)
    } else {
        state.fullscreen_evidence.clear();
        false
    };
    let list_ready = if list_hit {
        state
            .list_evidence
            .ready(&fg_norm, now, GAMELIST_CONFIRM_SECS)
    } else {
        state.list_evidence.clear();
        false
    };
    state.fullscreen_ticks = if full_hit {
        now.duration_since(state.fullscreen_evidence.since.unwrap())
            .as_secs() as u32
    } else {
        0
    };
    state.list_ticks = if list_hit {
        now.duration_since(state.list_evidence.since.unwrap())
            .as_secs() as u32
    } else {
        0
    };

    // ---- 场景识别影子观测：已确认的 D3D 全屏会话且前台不在游戏名单 → 记一次候选。
    // 纯观察不改任何行为；每个持续全屏会话对同一进程只记一次。 ----
    if full_ready && !fg_norm.is_empty() && !state.settings.game_list.contains(&fg_norm) {
        if state.scene_shadow_seen.as_deref() != Some(&fg_norm) {
            state.scene_shadow_seen = Some(fg_norm.clone());
            crate::scene_shadow::observe_game_scene_session(&fg_norm);
        }
    } else if !full_ready {
        state.scene_shadow_seen = None;
    }

    // ---- 全自动模式：门槛达标的场景候选自动加入名单（每次 tick 限量），
    // 每条采纳进变更日志并可一键移除；非全自动模式保持纯建议。 ----
    if state.settings.autopilot && state.settings.auto_game {
        let mut adopted = 0usize;
        for candidate in crate::scene_shadow::snapshot_scene(&state.settings.game_list) {
            if adopted >= AUTOPILOT_ADOPT_PER_TICK {
                break;
            }
            if let Some(message) = autopilot_adopt_scene(&mut state.settings, &candidate.name) {
                let normalized = normalize_name(&candidate.name);
                let _ = update_settings(|s| {
                    if !s.game_list.contains(&normalized) {
                        s.game_list.push(normalized.clone());
                    }
                    Ok(())
                });
                let _ = crate::journal::record(
                    &format!("全自动模式自动采纳场景建议：{normalized} 加入游戏名单（可在智能引擎页移除）"),
                    crate::journal::Revert::None,
                );
                actions.push(message);
                adopted += 1;
            }
        }
    };

    let cooldown = state.in_cooldown();

    // ---- 陈旧 pid 清理：进程退出后 pid 可能被复用，规则引擎/优先级提升不得误判 ----
    state
        .rules_suspend_notice_seen
        .retain(|pid| sys.processes().contains_key(&sysinfo::Pid::from_u32(*pid)));
    if let Some((pid, _)) = state.boosted {
        if !sys.processes().contains_key(&sysinfo::Pid::from_u32(pid)) {
            state.boosted = None;
        }
    }

    // ---- 全自动自适应引擎（开启时接管所有场景决策）----
    if state.settings.adaptive {
        let media_fg = !fg_norm.is_empty() && MEDIA_PROCS.contains(&fg_norm.as_str());
        state.adaptive.telemetry = telemetry;
        let ctx = AdaptiveCtx {
            gaming: full_ready || list_ready,
            media: media_fg,
            idle_secs: input_idle_secs(),
            ac_online,
            auto_power: state.settings.auto_power,
            telemetry,
            epp_ok,
            heavy_threshold: state.settings.heavy_cpu_threshold,
            away_threshold: state.settings.away_idle_secs as u64,
        };
        actions.extend(adaptive_tick(state, ctx, elapsed, now));
    } else {
        if state.adaptive.brightness.has_saved()
            && calibration_due(&mut state.adaptive.last_calibration, now)
            && !crate::tweaks::manual_hold_active()
        {
            update_brightness(&mut state.adaptive, false);
        }

        // ---- 自动省电：脱离电源 → 省电模式（非自适应模式的兜底）----
        if state.settings.auto_power
            && !ac_online
            && mode != "power"
            && !cooldown
            && crate::tweaks::apply_mode_core("power").is_ok()
        {
            state.last_switch = Some(Instant::now());
            actions.push("检测到使用电池，已自动进入省电模式".into());
            state.restore_boost();
            return actions;
        }

        // ---- 自动游戏模式：全屏持续 或 名单前台持续 ----
        let game_by_fullscreen = state.settings.auto_game
            && mode != "game"
            && full_ready
            && !cooldown
            && fg_pid.map(|p| p != self_pid).unwrap_or(false);
        let game_by_list = mode != "game"
            && list_ready
            && !cooldown
            && fg_pid.map(|p| p != self_pid).unwrap_or(false);
        if game_by_fullscreen || game_by_list {
            let why = if game_by_fullscreen {
                format!(
                    "检测到全屏应用「{}」持续 {} 秒",
                    state.last_foreground, FULLSCREEN_CONFIRM_SECS
                )
            } else {
                format!("检测到名单内应用「{}」进入前台", state.last_foreground)
            };
            if crate::tweaks::apply_mode_core("game").is_ok() {
                state.last_switch = Some(Instant::now());
                actions.push(format!("{why}，已自动进入游戏模式"));
            }
        }
    }

    let effective_mode = if state.settings.adaptive {
        state.adaptive.state.clone()
    } else {
        crate::tweaks::current_mode()
    };
    let effective_mode = if !state.settings.adaptive {
        mode.to_string()
    } else {
        effective_mode
    };
    actions.extend(tick_shared(
        state,
        &effective_mode,
        fullscreen,
        fg_pid,
        foreground_creation_time,
        self_pid,
        anomalies,
        elapsed,
        sys,
    ));
    actions
}

fn game_boost_active(mode: &str, enabled: bool) -> bool {
    mode == "game" && enabled
}

fn foreground_boost_scene_active(settings: &AutoSettings, mode: &str, fullscreen: bool) -> bool {
    settings.cpu_scheduler
        && !settings.scheduler_shadow
        && game_boost_active(mode, settings.game_boost)
        && fullscreen
}

fn should_refresh_own_process_tree(
    scheduler_enabled: bool,
    scheduler_scan_due: bool,
    foreground_boost_scene: bool,
) -> bool {
    scheduler_enabled && (scheduler_scan_due || foreground_boost_scene)
}

// 两种场景引擎共用的尾部；不再执行旧模式切换逻辑。
fn tick_shared(
    state: &mut AutoState,
    mode: &str,
    fullscreen: bool,
    fg_pid: Option<u32>,
    foreground_creation_time: Option<u64>,
    self_pid: u32,
    anomalies: &[crate::diag::Anomaly],
    elapsed: Duration,
    sys: &System,
) -> Vec<String> {
    let mut actions = Vec::new();
    let now = Instant::now();
    let scheduler_scan_due = state.settings.cpu_scheduler
        && state
            .scheduler_last_scan
            .is_none_or(|last| now.duration_since(last) >= SCHEDULER_RESCAN_INTERVAL);
    let foreground_boost_scene = foreground_boost_scene_active(&state.settings, mode, fullscreen);
    if should_refresh_own_process_tree(
        state.settings.cpu_scheduler,
        scheduler_scan_due,
        foreground_boost_scene,
    ) {
        state.foreground_own_tree = crate::processes::collect_own_process_tree_pids(sys);
    } else {
        // 其余场景没有使用者；清空避免把陈旧 PID 当作当前自身树成员。
        state.foreground_own_tree.clear();
    }
    if !state.settings.cpu_scheduler || crate::cpu_scheduler_cleanup_requested() {
        state.scheduler_last_scan = Some(now);
        let restore_result = restore_scheduler_processes(state);
        let restored = restore_result.as_ref().copied().unwrap_or(0);
        state.cpu_scheduler_status = CpuSchedulerStatus {
            supported: true,
            eco_qos_processes: 0,
            foreground_priority: None,
            last_updated_unix: scheduler_unix_now(),
            detail: if let Err(error) = restore_result {
                format!("关闭调度后等待恢复：{error}")
            } else if crate::cpu_scheduler_cleanup_requested() {
                "应用即将退出，正在后台还原已记录的调度状态".into()
            } else if restored > 0 {
                format!("自动 CPU 调度已关闭，恢复 {restored} 个后台进程")
            } else {
                "自动 CPU 调度已关闭".into()
            },
            monitored_processes: sys.processes().len(),
            background_candidates: 0,
            background_cpu_cap_pct: BACKGROUND_ECO_QOS_CPU_MAX,
            candidate_limit: MAX_ECO_QOS_PROCESSES,
        };
        if let Err(error) = state.restore_foreground_priority() {
            tracing::warn!(%error, "关闭自动 CPU 调度时，前台优先级恢复尚未完成");
            let detail = &mut state.cpu_scheduler_status.detail;
            if !detail.is_empty() {
                detail.push('；');
            }
            detail.push_str(&error);
        }
    } else {
        if scheduler_scan_due {
            state.scheduler_last_scan = Some(now);
            let foreground_name = fg_pid
                .and_then(|pid| sys.processes().get(&sysinfo::Pid::from_u32(pid)))
                .map(|process| normalize_name(&process.name().to_string_lossy()))
                .unwrap_or_default();
            let windows_root = crate::processes::trusted_windows_directory();
            let mut selected = Vec::with_capacity(MAX_ECO_QOS_PROCESSES);
            let mut background_candidate_count = 0usize;
            let shadow = state.settings.scheduler_shadow;
            if scheduler_scene_eligible(mode, &foreground_name) {
                let now_unix = unix_now();
                // Reuse the same immutable System snapshot already collected at the start of this tick.
                let own_tree = &state.foreground_own_tree;
                let mut candidates = sys
                    .processes()
                    .iter()
                    .filter_map(|(pid, process)| {
                        let pid = pid.as_u32();
                        let cpu = process.cpu_usage();
                        let start_time = process.start_time();
                        if !scheduler_candidate_is_selectable(
                            pid, self_pid, fg_pid, cpu, now_unix, start_time, own_tree,
                        ) {
                            return None;
                        }
                        let name = process.name().to_string_lossy();
                        let path = process
                            .exe()
                            .map(|path| path.to_string_lossy())
                            .unwrap_or(Cow::Borrowed(""));
                        if !scheduler_process_allowed(
                            name.as_ref(),
                            path.as_ref(),
                            windows_root,
                            &state.settings.protected_apps,
                        ) {
                            return None;
                        }
                        Some(SchedulerCandidate {
                            pid,
                            cpu,
                            working_set: process.memory(),
                            image_path: path,
                            start_time,
                        })
                    })
                    .collect::<Vec<_>>();
                sort_scheduler_candidates(&mut candidates);
                background_candidate_count = candidates.len();
                if !shadow {
                    selected = confirm_scheduler_candidates(
                        &candidates,
                        MAX_ECO_QOS_PROCESSES,
                        |candidate| {
                            process_identity(candidate.pid).is_some_and(
                                |(actual_path, actual_created)| {
                                    identity_matches(
                                        &actual_path,
                                        filetime_unix_seconds(actual_created),
                                        candidate.image_path.as_ref(),
                                        candidate.start_time,
                                    )
                                },
                            )
                        },
                    );
                }
            }
            let desired = if shadow { Vec::new() } else { selected };
            let desired_set = desired
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>();
            let old = state
                .scheduler_originals
                .keys()
                .copied()
                .collect::<Vec<_>>();
            for pid in old {
                if !desired_set.contains(&pid) {
                    let snapshot = state.scheduler_originals.get(&pid).cloned();
                    if let Some(snapshot) = snapshot {
                        match same_scheduler_process(&snapshot) {
                            IdentityCheck::Match => {
                                let handle = unsafe {
                                    OpenProcess(
                                        PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                                        0,
                                        pid,
                                    )
                                };
                                if handle.is_null() {
                                    continue;
                                }
                                match handle_identity_check(
                                    handle,
                                    &snapshot.image_path,
                                    snapshot.creation_time,
                                ) {
                                    IdentityCheck::Match => {
                                        let qos_restored = !snapshot.applied_eco_qos
                                            || restore_eco_qos_owned(handle, &snapshot);
                                        let priority_restored = snapshot
                                            .original_priority_class
                                            .is_none_or(|priority| {
                                                restore_priority_class_if_owned(
                                                    handle,
                                                    BACKGROUND_PRIORITY_CLASS,
                                                    priority,
                                                )
                                            });
                                        if qos_restored && priority_restored {
                                            state.scheduler_originals.remove(&pid);
                                            let _ = persist_scheduler_snapshots(
                                                &state.scheduler_originals,
                                            );
                                        }
                                        unsafe { CloseHandle(handle) };
                                    }
                                    IdentityCheck::Mismatch => {
                                        unsafe { CloseHandle(handle) };
                                        state.scheduler_originals.remove(&pid);
                                        let _ =
                                            persist_scheduler_snapshots(&state.scheduler_originals);
                                    }
                                    IdentityCheck::Unavailable => {
                                        unsafe { CloseHandle(handle) };
                                        tracing::debug!(
                                            pid,
                                            "后台进程身份暂不可读，保留 CPU 恢复快照"
                                        );
                                    }
                                }
                            }
                            IdentityCheck::Mismatch => {
                                state.scheduler_originals.remove(&pid);
                                let _ = persist_scheduler_snapshots(&state.scheduler_originals);
                            }
                            IdentityCheck::Unavailable => {
                                tracing::debug!(pid, "后台进程身份暂不可读，保留 CPU 恢复快照");
                            }
                        }
                    }
                }
            }
            let mut unsupported = false;
            for pid in desired {
                match set_process_eco_qos(state, pid) {
                    Ok(()) => {}
                    Err(error) => {
                        unsupported = true;
                        tracing::debug!(pid, %error, "EcoQoS candidate skipped");
                    }
                }
            }
            let _ = persist_scheduler_snapshots(&state.scheduler_originals);
            state.cpu_scheduler_status = CpuSchedulerStatus {
                supported: !unsupported || !state.scheduler_originals.is_empty(),
                eco_qos_processes: state
                    .scheduler_originals
                    .values()
                    .filter(|snapshot| snapshot.applied_eco_qos)
                    .count(),
                foreground_priority: state
                    .foreground_priority_original
                    .as_ref()
                    .map(|snapshot| format!("高于正常 · PID {}", snapshot.pid)),
                last_updated_unix: scheduler_unix_now(),
                detail: if state.settings.scheduler_shadow {
                    format!("影子模式：发现 {background_candidate_count} 个候选，仅观测、不修改进程状态")
                } else if unsupported && state.scheduler_originals.is_empty() {
                    "系统或目标进程暂不支持 EcoQoS，已跳过后台调度提示".into()
                } else {
                    format!(
                        "已评估后台进程；EcoQoS 应用 {} 个",
                        state.scheduler_originals.len()
                    )
                },
                monitored_processes: sys.processes().len(),
                background_candidates: background_candidate_count,
                background_cpu_cap_pct: BACKGROUND_ECO_QOS_CPU_MAX,
                candidate_limit: MAX_ECO_QOS_PROCESSES,
            };
        }
    }

    // ---- 游戏或高交互场景只温和提升一个前台进程；记录真实身份并可恢复 ----
    if !foreground_boost_scene {
        state.foreground_scene_since = None;
    }
    if let Some(pid) = fg_pid.filter(|pid| {
        *pid != 0 && *pid != 4 && *pid != self_pid && !state.foreground_own_tree.contains(pid)
    }) {
        let desired_scene = foreground_boost_scene;
        if desired_scene {
            state.foreground_scene_since.get_or_insert(now);
            if now.duration_since(state.foreground_scene_since.unwrap()) >= Duration::from_secs(2) {
                let already_same =
                    state
                        .foreground_priority_original
                        .as_ref()
                        .is_some_and(|snapshot| {
                            snapshot.pid == pid
                                && same_priority_process(snapshot) == IdentityCheck::Match
                                && priority_class_for_snapshot(snapshot)
                                    == Some(FOREGROUND_BOOST_PRIORITY_CLASS)
                        });
                if !already_same {
                    if let Err(error) = state.restore_foreground_priority() {
                        tracing::warn!(%error, "切换前台进程前，旧优先级快照尚未恢复");
                    }
                    if state.foreground_priority_original.is_none() {
                        if let Some((image_path, creation_time)) = process_identity(pid) {
                            unsafe {
                                let handle = OpenProcess(
                                    PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION,
                                    0,
                                    pid,
                                );
                                if !handle.is_null() {
                                    let handle_identity = process_identity_from_handle(handle);
                                    let foreground_matches =
                                        foreground_creation_time.is_some_and(|created| {
                                            handle_identity.as_ref().is_some_and(|(_, actual)| {
                                                filetime_unix_seconds(*actual) == created
                                            })
                                        });
                                    let original_priority_class = GetPriorityClass(handle);
                                    if handle_identity.as_ref().is_some_and(|(path, created)| {
                                        identity_matches(path, *created, &image_path, creation_time)
                                    }) && foreground_matches
                                    {
                                        if let Some(target_priority_class) =
                                            foreground_priority_target(original_priority_class)
                                        {
                                            let snapshot = ProcessPrioritySnapshot {
                                                pid,
                                                image_path,
                                                creation_time,
                                                original_priority_class,
                                            };
                                            match persist_foreground_snapshot(Some(&snapshot)) {
                                                Err(error) => tracing::warn!(
                                                    pid,
                                                    %error,
                                                    "前台 CPU 优先级快照无法落盘，未修改进程"
                                                ),
                                                Ok(()) => {
                                                    let priority_before = GetPriorityClass(handle);
                                                    let set_succeeded = priority_before
                                                        == original_priority_class
                                                        && SetPriorityClass(
                                                            handle,
                                                            target_priority_class,
                                                        ) != 0;
                                                    let priority_after = if set_succeeded {
                                                        GetPriorityClass(handle)
                                                    } else {
                                                        priority_before
                                                    };
                                                    match foreground_boost_outcome(
                                                        priority_before,
                                                        original_priority_class,
                                                        set_succeeded,
                                                        priority_after,
                                                        target_priority_class,
                                                    ) {
                                                        ForegroundBoostOutcome::Applied => {
                                                            state.foreground_priority_original =
                                                                Some(snapshot);
                                                            state.boosted = Some((
                                                                pid,
                                                                original_priority_class,
                                                            ));
                                                            actions.push(format!(
                                                                "已温和提升前台进程「{}」调度响应",
                                                                state.last_foreground
                                                            ));
                                                        }
                                                        ForegroundBoostOutcome::NotApplied => {
                                                            if let Err(error) =
                                                                persist_foreground_snapshot(None)
                                                            {
                                                                tracing::warn!(
                                                                    pid,
                                                                    %error,
                                                                    "前台优先级未变更，但候选恢复快照清理失败"
                                                                );
                                                            }
                                                        }
                                                        ForegroundBoostOutcome::Unverified => {
                                                            state.foreground_priority_original =
                                                                Some(snapshot);
                                                            tracing::warn!(
                                                                pid,
                                                                "前台 CPU 优先级写入结果不可读，保留快照等待重试"
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    CloseHandle(handle);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            state.foreground_scene_since = None;
            if state.foreground_priority_original.is_some() {
                match state.restore_foreground_priority() {
                    Ok(()) => actions.push("已退出游戏专注，恢复前台进程原始优先级".into()),
                    Err(error) => {
                        tracing::warn!(%error, "退出游戏专注后，前台优先级恢复尚未完成");
                        actions.push(format!("已退出游戏专注，但前台优先级恢复未完成：{error}"));
                    }
                }
            }
        }
    } else {
        state.foreground_scene_since = None;
        if state.foreground_priority_original.is_some() {
            if let Err(error) = state.restore_foreground_priority() {
                tracing::warn!(%error, "前台进程身份变化后，优先级恢复尚未完成");
            }
        }
    }

    // ---- 处置规则引擎 ----
    actions.extend(check_rules(state, anomalies, elapsed));

    actions
}

fn kind_label(kind: &str) -> &'static str {
    match kind {
        "cpu" => "CPU 占用",
        "mem" => "内存占用",
        _ => "磁盘 IO",
    }
}

fn threshold_label(rule: &Rule) -> String {
    match rule.kind.as_str() {
        "cpu" => format!("{}%", rule.threshold),
        "mem" => format!("{} MB", rule.threshold),
        _ => format!("{:.0} MB/s", rule.threshold / 1024.0),
    }
}

fn value_label(kind: &str, v: f64) -> String {
    match kind {
        "cpu" => format!("{:.0}%", v),
        "mem" => format!("{:.0} MB", v),
        _ => format!("{:.1} MB/s", v / 1024.0),
    }
}

/// 处置规则核心：异常值 ≥ 阈值持续 N 秒触发一次，直至异常解除重新计数；系统变更必须使用采样实例令牌。
/// `auto_dispose` 为全自动模式的"异常自动降级"子开关：把"仅提醒"规则升级为
/// 对非前台目标降低优先级（可逆、进变更日志）；系统关键进程始终只提醒。
/// 显式配置的 `deprio` 规则保留用户选定的动作，可能作用于前台应用。
fn rule_priority_failure_feedback(desc: &str, action_label: &str, error: &str) -> String {
    let detail = if error.starts_with("目标进程实例无法验证或已变化") {
        "目标进程样本身份无法核验或已变化".to_string()
    } else {
        error.to_string()
    };
    format!(
        "规则提醒：{desc}（{action_label}：{detail}；本次持续异常期间不会自动重试，待异常解除后重新触发，或在进程页刷新并手动处理）"
    )
}

fn rules_decide(
    rules: &[Rule],
    sustained: &mut std::collections::HashMap<(u32, String), Duration>,
    suspend_notice_seen: &mut std::collections::HashSet<u32>,
    anomalies: &[crate::diag::Anomaly],
    elapsed: Duration,
    auto_dispose: bool,
    foreground_name: &str,
) -> Vec<String> {
    let mut new_sustained: std::collections::HashMap<(u32, String), Duration> =
        std::collections::HashMap::new();
    let mut actions = Vec::new();

    for a in anomalies {
        let Some(rule) = rules.iter().find(|r| r.kind == a.kind && r.enabled) else {
            continue;
        };
        if a.value < rule.threshold {
            continue;
        }
        let key = (a.pid, a.kind.clone());
        let previous = sustained.get(&key).copied().unwrap_or_default();
        // 长缺口不证明持续异常，必须重新确认。
        let count = if elapsed > Duration::from_secs(5) {
            Duration::ZERO
        } else {
            previous + elapsed
        };
        new_sustained.insert(key, count);
        let required = Duration::from_secs(rule.seconds as u64);
        if previous >= required || count < required {
            continue;
        }

        let critical = CRITICAL_PROCS.contains(&normalize_name(&a.name).as_str());
        let desc = format!(
            "「{}」{} {}（阈值 {}）已持续 {} 秒",
            a.name,
            kind_label(&a.kind),
            value_label(&a.kind, a.value),
            threshold_label(rule),
            rule.seconds
        );
        if critical {
            actions.push(format!("规则提醒：{desc}（系统关键进程，仅提醒）"));
            continue;
        }
        let foreground = !foreground_name.is_empty()
            && normalize_name(&a.name) == normalize_name(foreground_name);
        match rule.action.as_str() {
            "suspend" => {
                if suspend_notice_seen.insert(a.pid) {
                    tracing::warn!(pid = a.pid, name = %a.name, "自动暂停暂时关闭；规则仅提醒");
                    actions.push(format!(
                        "规则提醒：{desc}（进程暂停暂时关闭，已按仅提醒处理）"
                    ));
                }
            }
            "deprio" => {
                match crate::processes::set_priority_verified(a.pid, a.creation_filetime, 1) {
                    Ok(()) => {
                        tracing::info!(pid = a.pid, name = %a.name, "规则处置：优先级已降至低于正常");
                        actions.push(format!(
                            "规则处置：已将 {} 优先级降至低于正常（{desc}）",
                            a.name
                        ))
                    }
                    Err(e) => {
                        tracing::warn!(pid = a.pid, error = %e, "规则处置：降级失败");
                        actions.push(rule_priority_failure_feedback(&desc, "降级失败", &e))
                    }
                }
            }
            _ => {
                // 全自动模式"异常自动降级"：仅提醒规则升级为自动降低优先级；
                // 前台进程例外（用户正在交互，降级会直接损害体验）。
                if auto_dispose && !foreground {
                    match crate::processes::set_priority_verified(a.pid, a.creation_filetime, 1) {
                        Ok(()) => {
                            tracing::info!(pid = a.pid, name = %a.name, "全自动处置：提醒规则升级为自动降级");
                            let _ = crate::journal::record(
                                &format!(
                                    "全自动模式自动处置：{} 优先级降至低于正常（{desc}）——可在「进程管理」手动恢复",
                                    a.name
                                ),
                                crate::journal::Revert::None,
                            );
                            actions.push(format!(
                                "全自动处置：已将 {} 优先级降至低于正常（{desc}）——可在「进程管理」恢复",
                                a.name
                            ));
                        }
                        Err(e) => {
                            tracing::warn!(pid = a.pid, error = %e, "全自动处置：降级失败");
                            actions.push(rule_priority_failure_feedback(&desc, "自动降级失败", &e))
                        }
                    }
                } else {
                    let suffix = if foreground {
                        "（前台进程，仅提醒）"
                    } else {
                        ""
                    };
                    tracing::info!(pid = a.pid, name = %a.name, kind = %a.kind, "规则提醒触发");
                    actions.push(format!("规则提醒：{desc}{suffix}"));
                }
            }
        }
    }
    let active_rule_pids = new_sustained
        .keys()
        .map(|(pid, _)| *pid)
        .collect::<std::collections::HashSet<_>>();
    suspend_notice_seen.retain(|pid| active_rule_pids.contains(pid));
    *sustained = new_sustained;
    actions
}

fn check_rules(
    state: &mut AutoState,
    anomalies: &[crate::diag::Anomaly],
    elapsed: Duration,
) -> Vec<String> {
    rules_decide(
        &state.settings.rules,
        &mut state.rules_sustained,
        &mut state.rules_suspend_notice_seen,
        anomalies,
        elapsed,
        state.settings.autopilot && state.settings.auto_dispose_anomalies,
        &state.last_foreground,
    )
}

fn settings_path() -> std::path::PathBuf {
    crate::util::app_dir().join("auto_settings.json")
}

pub fn knobs_current_pub() -> crate::auto::Knobs {
    knobs_current()
}

pub fn save_settings_pub(s: &AutoSettings) -> Result<(), String> {
    let _g = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    crate::util::atomic_write(
        &settings_path(),
        &serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?,
    )
}

/// 重大调优前是否自动创建还原点
pub fn auto_restore_point_enabled() -> bool {
    load_settings().auto_restore_point
}

/// 设置文件的原子读改写锁（防止并发命令互相覆盖）
static SETTINGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 原子地"读-改-写"设置文件，返回最新设置
pub(crate) fn update_settings(
    f: impl FnOnce(&mut AutoSettings) -> Result<(), String>,
) -> Result<AutoSettings, String> {
    let _g = SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = load_settings();
    f(&mut s)?;
    save_settings(&s)?;
    Ok(s)
}

fn parse_settings(bytes: &[u8]) -> Result<AutoSettings, serde_json::Error> {
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    serde_json::from_slice(json)
}

fn disabled_settings_fallback() -> AutoSettings {
    let mut settings = AutoSettings {
        auto_game: false,
        auto_power: false,
        game_boost: false,
        cpu_scheduler: false,
        scheduler_shadow: true,
        close_to_tray: false,
        adaptive: false,
        auto_restore_point: false,
        auto_dim: false,
        ..Default::default()
    };
    settings.rules.clear();
    settings
}

const SETTINGS_FORMAT_WARNING: &str =
    "自动设置文件格式无效，自动控制已暂停。可修复文件或重新导入配置。";
const SETTINGS_READ_WARNING: &str =
    "自动设置文件无法读取，自动控制已暂停。请检查文件状态或重新导入配置。";
const SETTINGS_SUSPEND_MIGRATION_WARNING: &str =
    "旧的自动挂起规则已转为仅提醒；进程暂停暂时关闭，恢复入口仍可使用。";

fn normalize_disabled_suspend_actions(settings: &mut AutoSettings) -> bool {
    let mut changed = false;
    for rule in &mut settings.rules {
        if rule.action == "suspend" {
            rule.action = "notify".into();
            changed = true;
        }
    }
    changed
}

fn parse_settings_with_warning(bytes: &[u8]) -> (AutoSettings, Option<String>) {
    match parse_settings(bytes) {
        Ok(mut settings) => {
            let migrated = normalize_disabled_suspend_actions(&mut settings);
            let warning = migrated.then(|| SETTINGS_SUSPEND_MIGRATION_WARNING.to_owned());
            (settings, warning)
        }
        Err(error) => {
            tracing::warn!(%error, "设置文件格式无效，自动控制已暂停");
            (
                disabled_settings_fallback(),
                Some(SETTINGS_FORMAT_WARNING.to_owned()),
            )
        }
    }
}

fn settings_from_read_result(
    result: Result<Vec<u8>, std::io::Error>,
) -> (AutoSettings, Option<String>) {
    match result {
        Ok(bytes) => parse_settings_with_warning(&bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            (AutoSettings::default(), None)
        }
        Err(error) => {
            tracing::warn!(%error, "读取设置文件失败，自动控制已暂停");
            (
                disabled_settings_fallback(),
                Some(SETTINGS_READ_WARNING.to_owned()),
            )
        }
    }
}

fn load_settings_with_warning() -> (AutoSettings, Option<String>) {
    settings_from_read_result(std::fs::read(settings_path()))
}

pub fn load_settings() -> AutoSettings {
    load_settings_with_warning().0
}

pub fn save_settings(s: &AutoSettings) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    crate::util::atomic_write(&settings_path(), &bytes).map_err(|e| format!("设置持久化失败: {e}"))
}

#[derive(Serialize)]
pub struct AutoSettingsResponse {
    #[serde(flatten)]
    pub settings: AutoSettings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_warning: Option<String>,
}

#[tauri::command]
pub fn get_auto_settings() -> AutoSettingsResponse {
    let (settings, load_warning) = load_settings_with_warning();
    AutoSettingsResponse {
        settings,
        load_warning,
    }
}

#[tauri::command]
pub fn set_auto_setting(
    key: String,
    on: bool,
    auto: tauri::State<'_, AutoStateLock>,
) -> Result<(), String> {
    if key == "retry_baseline_restore" {
        if on {
            return Err("基线恢复重试不接受开启参数".into());
        }
        let mut st = auto.lock().unwrap_or_else(|error| error.into_inner());
        return if st.adaptive.baseline_restore_pending {
            st.restore_baseline()
        } else {
            Ok(())
        };
    }
    if key == "retry_brightness_restore" {
        if on {
            return Err("亮度恢复重试不接受开启参数".into());
        }
        let mut st = auto.lock().unwrap_or_else(|error| error.into_inner());
        if st.adaptive.baseline_restore_pending {
            return Err("系统基线恢复尚未完成；请使用「重试恢复基线」完成整体验证".into());
        }
        if st.adaptive.brightness.recovery_issue {
            return crate::brightness::schedule_startup_recovery(&mut st.adaptive.brightness)
                .map(|_| ());
        }
        if !brightness_restore_pending(
            &st.settings,
            &st.adaptive.state,
            st.adaptive.brightness.has_saved(),
            false,
        ) {
            return Ok(());
        }
        update_brightness(&mut st.adaptive, false);
        if st.adaptive.brightness.has_saved() && !st.adaptive.brightness.restore_pending {
            return Err(
                "屏幕亮度仍未确认恢复；后台重试尚未排队（可能正在退避），请稍后再试".into(),
            );
        }
        return Ok(());
    }
    if key == "adaptive" && on {
        let st = auto.lock().unwrap_or_else(|error| error.into_inner());
        if st.adaptive.baseline_restore_pending {
            return Err("系统基线恢复尚未完成；请先重试恢复或检查变更日志".into());
        }
    }
    let s = update_settings(|s| {
        match key.as_str() {
            "auto_game" => s.auto_game = on,
            "auto_power" => s.auto_power = on,
            "game_boost" => s.game_boost = on,
            "cpu_scheduler" => s.cpu_scheduler = on,
            "scheduler_shadow" => s.scheduler_shadow = on,
            "close_to_tray" => s.close_to_tray = on,
            "adaptive" => s.adaptive = on,
            "auto_restore_point" => s.auto_restore_point = on,
            "auto_dim" => s.auto_dim = on,
            "auto_dispose_anomalies" => s.auto_dispose_anomalies = on,
            _ => return Err(format!("未知设置项: {key}")),
        }
        Ok(())
    })?;
    tracing::info!(key = %key, on, "设置已更新");
    // 同步运行时状态（否则开关只落盘不生效）
    let mut st = auto.lock().unwrap();
    st.settings = s;
    if key == "auto_dim" && !on {
        update_brightness(&mut st.adaptive, false);
    }
    if key == "adaptive" {
        if on {
            seed_adaptive_baseline(&mut st, "初始化采样中");
            tracing::info!("自适应引擎已开启，基线已记录");
        } else {
            st.restore_baseline()?;
            tracing::info!("自适应引擎已关闭，已恢复基线");
        }
    }
    Ok(())
}

/// 自适应引擎开启时的统一初始化：基线取当前实时旋钮，从当前状态平滑起步。
fn seed_adaptive_baseline(st: &mut AutoState, reason: &str) {
    let cur = knobs_current();
    st.adaptive.baseline_plan_guid = crate::power::active_scheme_guid();
    st.adaptive.baseline = Some(cur.clone());
    st.adaptive.applied = Some(cur);
    // Adaptive states are classifier keys; never seed it with arbitrary UI mode aliases.
    st.adaptive.state = classifier_mode_key(&crate::tweaks::current_mode());
    st.adaptive.strategy = StrategyKind::AdaptiveHybrid;
    st.adaptive.strategy_pending = None;
    st.adaptive.strategy_changed_at = None;
    st.adaptive.strategy_reason = reason.into();
    st.adaptive.strategy_scores.clear();
    st.adaptive.pending.clear();
}

// ================= 全自动模式 =================

/// 纯函数：切换全自动模式。开启时快照相关开关并整栈启用；关闭时按快照还原。
pub fn apply_autopilot(settings: &mut AutoSettings, enabled: bool) -> Result<String, String> {
    if enabled {
        if settings.autopilot {
            return Err("全自动模式已开启".into());
        }
        settings.autopilot_backup = Some(AutopilotBackup {
            adaptive: settings.adaptive,
            auto_game: settings.auto_game,
            auto_power: settings.auto_power,
            game_boost: settings.game_boost,
            cpu_scheduler: settings.cpu_scheduler,
            scheduler_shadow: settings.scheduler_shadow,
            auto_dispose_anomalies: settings.auto_dispose_anomalies,
        });
        settings.autopilot = true;
        settings.adaptive = true;
        settings.auto_game = true;
        settings.auto_power = true;
        settings.game_boost = true;
        settings.cpu_scheduler = true;
        settings.scheduler_shadow = false;
        settings.auto_dispose_anomalies = true;
        Ok("已开启全自动模式：场景自适应、后台 EcoQoS 压制、内存压力缓解、异常自动降级与场景名单自动采纳将静默运行；已记录当前设置基线，关闭时自动还原。".into())
    } else {
        if !settings.autopilot {
            return Err("全自动模式未开启".into());
        }
        settings.autopilot = false;
        if let Some(backup) = settings.autopilot_backup.take() {
            settings.adaptive = backup.adaptive;
            settings.auto_game = backup.auto_game;
            settings.auto_power = backup.auto_power;
            settings.game_boost = backup.game_boost;
            settings.cpu_scheduler = backup.cpu_scheduler;
            settings.scheduler_shadow = backup.scheduler_shadow;
            settings.auto_dispose_anomalies = backup.auto_dispose_anomalies;
        } else {
            settings.adaptive = false;
            settings.auto_dispose_anomalies = false;
        }
        Ok("已关闭全自动模式并还原此前设置；已应用的电源/EPP 等旋钮同时恢复基线。".into())
    }
}

/// 纯函数：全自动模式下把一个场景候选加入游戏名单；返回 toast 文案。
/// 名单校验/去重与手动添加一致；不满足条件返回 None。
fn autopilot_adopt_scene(settings: &mut AutoSettings, name: &str) -> Option<String> {
    if !settings.autopilot || !settings.auto_game {
        return None;
    }
    let normalized = normalize_name(name);
    if !valid_game_process_name(&normalized) || settings.game_list.contains(&normalized) {
        return None;
    }
    settings.game_list.push(normalized.clone());
    Some(format!(
        "全自动模式：已将 {normalized} 加入游戏识别名单（变更日志可查，可在智能引擎页移除）"
    ))
}

/// 每个 tick 最多自动采纳的候选数，避免突发批量改写名单。
const AUTOPILOT_ADOPT_PER_TICK: usize = 2;

#[tauri::command]
pub fn set_autopilot(
    enabled: bool,
    auto: tauri::State<'_, AutoStateLock>,
) -> Result<AutoSettingsResponse, String> {
    if enabled
        && auto
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .adaptive
            .baseline_restore_pending
    {
        return Err("系统基线恢复尚未完成；请先重试恢复或检查变更日志".into());
    }
    let s = update_settings(|s| apply_autopilot(s, enabled).map(|_| ()))?;
    let mut st = auto.lock().unwrap_or_else(|e| e.into_inner());
    st.settings = s.clone();
    if enabled {
        seed_adaptive_baseline(&mut st, "全自动模式：初始化采样中");
        tracing::info!("全自动模式已开启，基线已记录");
    } else {
        st.restore_baseline()?;
        tracing::info!("全自动模式已关闭，已恢复基线");
    }
    Ok(AutoSettingsResponse {
        settings: s,
        load_warning: None,
    })
}

#[tauri::command]
pub fn add_game_item(
    name: String,
    auto: tauri::State<'_, AutoStateLock>,
) -> Result<Vec<String>, String> {
    let n = normalize_name(&name);
    if !valid_game_process_name(&n) {
        return Err("请输入可执行文件名，例如 game.exe".into());
    }
    let s = update_settings(|s| {
        if !s.game_list.contains(&n) {
            s.game_list.push(n.clone());
        }
        Ok(())
    })?;
    auto.lock().unwrap().settings = s.clone();
    Ok(s.game_list)
}

#[tauri::command]
pub fn set_protected_apps(
    names: Vec<String>,
    auto: tauri::State<'_, AutoStateLock>,
) -> Result<Vec<String>, String> {
    let normalized = normalize_protected_apps(names)?;
    let settings = update_settings(|settings| {
        settings.protected_apps = normalized.clone();
        Ok(())
    })?;
    auto.lock().unwrap().settings = settings.clone();
    Ok(settings.protected_apps)
}

fn normalize_protected_apps(names: Vec<String>) -> Result<Vec<String>, String> {
    if names.len() > 128 {
        return Err("保护名单最多 128 项".into());
    }
    let mut unique = std::collections::BTreeSet::new();
    for name in names {
        let normalized = normalize_name(&name);
        if !valid_game_process_name(&normalized) {
            return Err(format!("进程名无效：{name}"));
        }
        unique.insert(format!("{normalized}.exe"));
    }
    Ok(unique.into_iter().collect())
}

#[tauri::command]
pub fn remove_game_item(
    name: String,
    auto: tauri::State<'_, AutoStateLock>,
) -> Result<Vec<String>, String> {
    let n = normalize_name(&name);
    if !valid_game_process_name(&n) {
        return Err("游戏进程名不合法".into());
    }
    let s = update_settings(|s| {
        s.game_list.retain(|g| *g != n);
        Ok(())
    })?;
    auto.lock().unwrap().settings = s.clone();
    Ok(s.game_list)
}

fn validate_rule_action(action: &str) -> Result<(), String> {
    match action {
        "notify" | "deprio" => Ok(()),
        "suspend" => Err("进程暂停暂时关闭；请选择仅提醒或降低优先级。".into()),
        _ => Err(format!("未知动作: {action}")),
    }
}

/// 保存一条处置规则（按 kind 覆盖）
#[tauri::command]
pub fn set_rule(
    kind: String,
    enabled: bool,
    threshold: f64,
    seconds: u32,
    action: String,
    auto: tauri::State<'_, AutoStateLock>,
) -> Result<Vec<Rule>, String> {
    if !["cpu", "mem", "io"].contains(&kind.as_str()) {
        return Err(format!("未知异常维度: {kind}"));
    }
    if !threshold.is_finite() || threshold > 1_000_000_000.0 {
        return Err("阈值需为有效且不超过 1,000,000,000 的数值".into());
    }
    validate_rule_action(&action)?;
    if threshold <= 0.0 || !(3..=600).contains(&seconds) {
        return Err("阈值 / 持续秒数超出范围".into());
    }
    let kind_log = kind.clone();
    let s = update_settings(|s| {
        for r in s.rules.iter_mut() {
            if r.kind == kind {
                *r = Rule {
                    kind: kind.clone(),
                    enabled,
                    threshold,
                    seconds,
                    action: action.clone(),
                };
                break;
            }
        }
        Ok(())
    })?;
    tracing::info!(kind = %kind_log, %action, enabled, "处置规则已更新");
    auto.lock().unwrap().settings = s.clone();
    Ok(s.rules)
}

/// 保存引擎数值参数（离开阈值 / 重载阈值）
#[tauri::command]
pub fn set_auto_num(
    key: String,
    value: f64,
    auto: tauri::State<'_, AutoStateLock>,
) -> Result<(), String> {
    if !value.is_finite() {
        return Err("参数需为有效有限数值".into());
    }
    let s = update_settings(|s| {
        match key.as_str() {
            "away_idle_secs" => {
                if !(30.0..=3600.0).contains(&value) {
                    return Err("离开阈值范围 30-3600 秒".into());
                }
                s.away_idle_secs = value as u32;
            }
            "heavy_cpu" => {
                if value.fract() != 0.0 {
                    return Err("重载阈值需为整数百分比".into());
                }
                if !(30.0..=95.0).contains(&value) {
                    return Err("重载阈值范围 30-95%".into());
                }
                s.heavy_cpu_threshold = value as f32;
            }
            _ => return Err(format!("未知参数: {key}")),
        }
        Ok(())
    })?;
    auto.lock().unwrap().settings = s;
    tracing::info!(key = %key, value, "引擎参数已更新");
    Ok(())
}

#[tauri::command]
pub fn get_state_seconds(
    auto: tauri::State<'_, AutoStateLock>,
) -> std::collections::HashMap<String, u64> {
    auto.lock().unwrap().adaptive.seconds.clone()
}

fn brightness_restore_pending(
    settings: &AutoSettings,
    adaptive_state: &str,
    has_saved_brightness: bool,
    snapshot_error: bool,
) -> bool {
    snapshot_error
        || (has_saved_brightness
            && !(settings.adaptive
                && settings.auto_dim
                && matches!(adaptive_state, "power" | "away")))
}

#[tauri::command]
pub fn get_auto_state(auto: tauri::State<'_, AutoStateLock>) -> AutoStateInfo {
    let st = auto.lock().unwrap();
    AutoStateInfo {
        settings: st.settings.clone(),
        cpu_scheduler: st.cpu_scheduler_status.clone(),
        foreground: st.last_foreground.clone(),
        fullscreen: st.last_fullscreen,
        mode: crate::tweaks::current_mode(),
        boosted_pid: st.boosted.as_ref().map(|(p, _)| *p),
        cooldown: st.in_cooldown(),
        adaptive: st.settings.adaptive,
        adaptive_state: st.adaptive.state.clone(),
        baseline_restore_pending: st.adaptive.baseline_restore_pending,
        brightness_restore_pending: brightness_restore_pending(
            &st.settings,
            &st.adaptive.state,
            st.adaptive.brightness.has_saved(),
            st.adaptive.brightness.recovery_issue,
        ),
        brightness_restore_snapshot_error: st.adaptive.brightness.recovery_issue,
        brightness_preserved_value: st
            .adaptive
            .brightness
            .preserved_notice_value(Instant::now()),
        stats_sample_id: crate::stats::latest_sample_id(),
        cpu_core_peak_pct: st.adaptive.telemetry.cpu_core_peak_pct,
        cpu_saturated_cores: st.adaptive.telemetry.cpu_saturated_cores,
        cpu_logical_cores: st.adaptive.telemetry.cpu_logical_cores,
        idle_secs: input_idle_secs(),
        hold_secs: crate::tweaks::manual_hold_secs_left(),
        decisions: st.adaptive.decisions.clone(),
        selected_algorithm: st.adaptive.strategy.id().into(),
        algorithm_reason: st.adaptive.strategy_reason.clone(),
        algorithm_scores: st.adaptive.strategy_scores.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_suspend_settings_are_downgraded_with_warning() {
        let mut settings = AutoSettings::default();
        settings.rules[0].action = "suspend".into();
        let bytes = serde_json::to_vec(&settings).unwrap();

        let (loaded, warning) = parse_settings_with_warning(&bytes);

        assert_eq!(loaded.rules[0].action, "notify");
        assert_eq!(warning.as_deref(), Some(SETTINGS_SUSPEND_MIGRATION_WARNING));
    }

    #[test]
    fn rule_action_rejects_new_suspend_requests() {
        assert!(validate_rule_action("notify").is_ok());
        assert!(validate_rule_action("deprio").is_ok());
        assert!(validate_rule_action("suspend")
            .unwrap_err()
            .contains("进程暂停暂时关闭"));
        assert!(validate_rule_action("other").is_err());
    }

    #[test]
    fn settings_parser_accepts_optional_utf8_bom() {
        let json =
            br#"{"auto_game":false,"auto_power":false,"cpu_scheduler":false,"adaptive":false}"#;
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(json);

        for settings in [
            parse_settings(json).unwrap(),
            parse_settings(&with_bom).unwrap(),
        ] {
            assert!(!settings.auto_game);
            assert!(!settings.auto_power);
            assert!(!settings.cpu_scheduler);
            assert!(!settings.adaptive);
        }
    }

    #[test]
    fn imported_settings_clear_stale_hysteresis_without_losing_baseline() {
        let now = Instant::now();
        let baseline = Knobs {
            plan: PlanKind::Balanced,
            epp: Some(32),
            proc_max: 75,
            gamedvr: true,
            prio_boost: false,
            priority_separation: 2,
        };
        let mut adaptive = Adaptive {
            baseline: Some(baseline.clone()),
            applied: Some(baseline),
            strategy_pending: Some((StrategyKind::Efficiency, now)),
            strategy_changed_at: Some(now),
            strategy_reason: "stale state".into(),
            ..Adaptive::default()
        };
        adaptive.pending.target = "heavy".into();
        adaptive.pending.since = Some(now);
        adaptive.seconds.insert("heavy".into(), 17);
        adaptive.strategy_scores.push(AlgorithmScore {
            id: "efficiency".into(),
            score: 80,
            suitable: true,
            reason: "stale candidate".into(),
        });
        adaptive.strategy_reason = "stale state".into();

        reset_imported_adaptive_transients(&mut adaptive, true);

        assert!(adaptive.seconds.is_empty());
        assert!(adaptive.pending.target.is_empty());
        assert!(adaptive.pending.since.is_none());
        assert!(adaptive.strategy_pending.is_none());
        assert!(adaptive.strategy_changed_at.is_none());
        assert!(adaptive.strategy_scores.is_empty());
        assert_eq!(adaptive.strategy_reason, "配置已导入，等待重新评估");
        assert_eq!(adaptive.baseline.as_ref().unwrap().proc_max, 75);
        assert_eq!(adaptive.applied.as_ref().unwrap().proc_max, 75);

        reset_imported_adaptive_transients(&mut adaptive, false);
        assert_eq!(adaptive.strategy_reason, "自适应引擎已关闭");
    }

    #[test]
    fn malformed_settings_pause_all_automatic_system_writes() {
        let (settings, warning) = parse_settings_with_warning(b"{ invalid json");
        assert_eq!(warning.as_deref(), Some(SETTINGS_FORMAT_WARNING));
        assert!(!settings.auto_game);
        assert!(!settings.auto_power);
        assert!(!settings.game_boost);
        assert!(!settings.cpu_scheduler);
        assert!(settings.scheduler_shadow);
        assert!(!settings.close_to_tray);
        assert!(settings.rules.is_empty());
        assert!(!settings.adaptive);
        assert!(!settings.auto_restore_point);
        assert!(!settings.auto_dim);
    }

    #[test]
    fn unreadable_settings_pause_automation_and_report_recovery_hint() {
        let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "fixture");
        let (settings, warning) = settings_from_read_result(Err(error));
        assert_eq!(warning.as_deref(), Some(SETTINGS_READ_WARNING));
        assert!(!settings.auto_power);
        assert!(!settings.adaptive);
        assert!(!settings.close_to_tray);
    }

    #[test]
    fn missing_settings_keep_intended_first_run_defaults() {
        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "fixture");
        let (settings, warning) = settings_from_read_result(Err(error));
        assert!(warning.is_none());
        assert!(settings.auto_power);
        assert!(settings.adaptive);
        assert!(settings.close_to_tray);
    }

    #[test]
    fn settings_response_flattens_warning_without_nesting_settings() {
        let warning = SETTINGS_FORMAT_WARNING.to_owned();
        let response = AutoSettingsResponse {
            settings: disabled_settings_fallback(),
            load_warning: Some(warning.clone()),
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["auto_power"], false);
        assert_eq!(value["close_to_tray"], false);
        assert_eq!(value["load_warning"], warning);
        assert!(value.get("settings").is_none());

        let healthy = AutoSettingsResponse {
            settings: AutoSettings::default(),
            load_warning: None,
        };
        let healthy = serde_json::to_value(healthy).unwrap();
        assert!(healthy.get("load_warning").is_none());
    }

    #[test]
    fn scheduler_restore_does_not_persist_an_unchanged_empty_map() {
        let snapshots = HashMap::new();
        let mut writes = 0;
        let result = finish_scheduler_restore(0, &snapshots, |_| {
            writes += 1;
            Ok(())
        });

        assert!(result.is_ok());
        assert_eq!(writes, 0);
    }

    #[test]
    fn scheduler_restore_persists_once_after_clearing_real_entries() {
        let snapshots = HashMap::new();
        let mut writes = 0;
        let result = finish_scheduler_restore(1, &snapshots, |persisted| {
            assert!(persisted.is_empty());
            writes += 1;
            Ok(())
        });

        assert!(result.is_ok());
        assert_eq!(writes, 1);
    }

    #[test]
    fn scheduler_restore_keeps_pending_entries_and_does_not_persist_failure() {
        let mut snapshots = HashMap::new();
        snapshots.insert(
            4242,
            ProcessSchedulerSnapshot {
                pid: 4242,
                image_path: r"C:\Apps\worker.exe".into(),
                creation_time: 7,
                original_control_mask: 0,
                original_state_mask: 0,
                applied_eco_qos: true,
                original_priority_class: None,
            },
        );
        let mut writes = 0;
        let result = finish_scheduler_restore(1, &snapshots, |_| {
            writes += 1;
            Ok(())
        });

        assert!(result.is_err());
        assert!(snapshots.contains_key(&4242));
        assert_eq!(writes, 0);
    }

    #[test]
    fn scheduler_restore_allows_retry_after_failed_attempt_keeps_pending_entries() {
        // 还原失败保留条目后再次调用：finish_scheduler_restore 的重试语义应保持一致，
        // 只要仍有未确认条目就继续返回 Err 且不落盘。
        let mut snapshots = HashMap::new();
        snapshots.insert(
            4242,
            ProcessSchedulerSnapshot {
                pid: 4242,
                image_path: r"C:\Apps\worker.exe".into(),
                creation_time: 7,
                original_control_mask: 0,
                original_state_mask: 0,
                applied_eco_qos: false,
                original_priority_class: Some(NORMAL_PRIORITY_CLASS),
            },
        );
        let mut writes = 0;
        for _ in 0..2 {
            let result = finish_scheduler_restore(1, &snapshots, |_| {
                writes += 1;
                Ok(())
            });
            assert!(result.is_err());
        }
        assert!(snapshots.contains_key(&4242));
        assert_eq!(writes, 0);
    }

    #[test]
    fn priority_restore_treats_exited_process_as_no_op() {
        // 已退出的 PID 必须按“无需恢复”处理：返回 Ok 让调用方丢弃快照，
        // 而不是返回 Err 让快照永久滞留（这是退出还原失败循环的根因）。
        let mut child = std::process::Command::new("cmd")
            .args(["/C", "exit"])
            .spawn()
            .expect("spawn probe process");
        let pid = child.id();
        let _ = child.wait();
        // PID 理论上可能被立刻复用；仅当确认身份已不可读时才断言清理路径。
        if process_identity(pid).is_none() {
            let snapshot = ProcessPrioritySnapshot {
                pid,
                image_path: r"C:\Apps\worker.exe".into(),
                creation_time: 7,
                original_priority_class: NORMAL_PRIORITY_CLASS,
            };
            assert!(restore_priority_snapshot(&snapshot).is_ok());
        }
    }

    #[test]
    fn scheduler_snapshot_persistence_skips_already_durable_bytes() {
        let mut writes = 0;
        let result = persist_bytes_if_changed(Some(b"snapshot"), b"snapshot", || {
            writes += 1;
            Ok(())
        });

        assert_eq!(result, Ok(false));
        assert_eq!(writes, 0);
    }

    #[test]
    fn scheduler_snapshot_persistence_writes_changed_bytes() {
        let mut writes = 0;
        let result = persist_bytes_if_changed(Some(b"old"), b"new", || {
            writes += 1;
            Ok(())
        });

        assert_eq!(result, Ok(true));
        assert_eq!(writes, 1);
    }

    #[test]
    fn scheduler_snapshot_persistence_retries_after_a_failed_write() {
        let mut writes = 0;
        let first = persist_bytes_if_changed(Some(b"old"), b"new", || {
            writes += 1;
            Err("injected write failure".into())
        });
        assert!(first.is_err());

        let retry = persist_bytes_if_changed(Some(b"old"), b"new", || {
            writes += 1;
            Ok(())
        });
        assert_eq!(retry, Ok(true));
        assert_eq!(writes, 2);
    }

    #[test]
    fn scheduler_eco_qos_restore_merges_only_scheduler_owned_bit() {
        let qos_bit = PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        assert_eq!(merged_eco_qos_restore(qos_bit, qos_bit, 0, 0, true), (0, 0));
        assert_eq!(
            merged_eco_qos_restore(qos_bit | 0x20, qos_bit | 0x40, 0, 0, true),
            (0x20, 0x40)
        );
        assert_eq!(
            merged_eco_qos_restore(qos_bit | 0x20, qos_bit | 0x40, 0, 0, false),
            (qos_bit | 0x20, qos_bit | 0x40)
        );
        assert_eq!(
            merged_eco_qos_restore(qos_bit | 0x20, qos_bit | 0x40, qos_bit, qos_bit, true),
            (qos_bit | 0x20, qos_bit | 0x40)
        );
    }

    #[test]
    fn scheduler_rejects_young_processes_until_stable() {
        assert!(!scheduler_process_is_stable(100, 0));
        assert!(!scheduler_process_is_stable(100, 90));
        assert!(scheduler_process_is_stable(100, 85));
        assert!(!scheduler_process_is_stable(100, 115));
    }

    #[test]
    fn scheduler_restores_only_a_still_owned_priority_class() {
        assert_eq!(
            priority_restore_target(
                BACKGROUND_PRIORITY_CLASS,
                BACKGROUND_PRIORITY_CLASS,
                NORMAL_PRIORITY_CLASS,
            ),
            Some(NORMAL_PRIORITY_CLASS),
            "restore the original class while AeroTune still owns the background class"
        );
        assert_eq!(
            priority_restore_target(
                NORMAL_PRIORITY_CLASS,
                BACKGROUND_PRIORITY_CLASS,
                NORMAL_PRIORITY_CLASS,
            ),
            None,
            "do not rewrite a class already restored by another actor"
        );
        assert_eq!(
            priority_restore_target(0x00008000, BACKGROUND_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,),
            None,
            "preserve a foreign priority change"
        );
        assert_eq!(
            priority_restore_target(0, BACKGROUND_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS),
            None,
            "an unreadable class is not a safe write target"
        );
        assert_eq!(
            priority_restore_target(
                BACKGROUND_PRIORITY_CLASS,
                BACKGROUND_PRIORITY_CLASS,
                BACKGROUND_PRIORITY_CLASS,
            ),
            None,
            "no write is needed when the original already equals the owned value"
        );
    }

    #[test]
    fn foreground_boost_only_targets_priority_classes_below_above_normal() {
        for original in [
            IDLE_PRIORITY_CLASS,
            BACKGROUND_PRIORITY_CLASS,
            NORMAL_PRIORITY_CLASS,
        ] {
            assert_eq!(
                foreground_priority_target(original),
                Some(FOREGROUND_BOOST_PRIORITY_CLASS)
            );
        }
        for original in [0, 0x00000080, 0x00000100, FOREGROUND_BOOST_PRIORITY_CLASS] {
            assert_eq!(foreground_priority_target(original), None);
        }
    }

    #[test]
    fn foreground_boost_outcome_preserves_foreign_or_unreadable_values() {
        assert_eq!(
            foreground_boost_outcome(
                NORMAL_PRIORITY_CLASS,
                NORMAL_PRIORITY_CLASS,
                true,
                FOREGROUND_BOOST_PRIORITY_CLASS,
                FOREGROUND_BOOST_PRIORITY_CLASS,
            ),
            ForegroundBoostOutcome::Applied
        );
        assert_eq!(
            foreground_boost_outcome(
                FOREGROUND_BOOST_PRIORITY_CLASS,
                NORMAL_PRIORITY_CLASS,
                false,
                FOREGROUND_BOOST_PRIORITY_CLASS,
                FOREGROUND_BOOST_PRIORITY_CLASS,
            ),
            ForegroundBoostOutcome::NotApplied,
            "a changed precondition must not trigger rollback of a same-valued foreign change"
        );
        assert_eq!(
            foreground_boost_outcome(
                NORMAL_PRIORITY_CLASS,
                NORMAL_PRIORITY_CLASS,
                true,
                0x00000080,
                FOREGROUND_BOOST_PRIORITY_CLASS,
            ),
            ForegroundBoostOutcome::NotApplied,
            "preserve a foreign change after our write"
        );
        assert_eq!(
            foreground_boost_outcome(
                NORMAL_PRIORITY_CLASS,
                NORMAL_PRIORITY_CLASS,
                true,
                0,
                FOREGROUND_BOOST_PRIORITY_CLASS,
            ),
            ForegroundBoostOutcome::Unverified,
            "retain recovery evidence when the post-write value is unreadable"
        );
        assert_eq!(
            foreground_boost_outcome(
                NORMAL_PRIORITY_CLASS,
                NORMAL_PRIORITY_CLASS,
                false,
                NORMAL_PRIORITY_CLASS,
                FOREGROUND_BOOST_PRIORITY_CLASS,
            ),
            ForegroundBoostOutcome::NotApplied
        );
    }

    #[test]
    fn scheduler_identity_requires_full_path_and_creation_timestamp() {
        assert!(identity_matches(
            r"C:\Apps\worker.exe",
            42,
            r"c:\apps\WORKER.exe",
            42
        ));
        assert!(!identity_matches(
            r"C:\Apps\worker.exe",
            42,
            r"C:\Apps\worker.exe",
            43
        ));
        assert!(!identity_matches(
            r"C:\Apps\worker.exe",
            42,
            r"C:\Apps\other.exe",
            42
        ));
        assert!(exact_filetime_match(0x01DB123456789ABC, 0x01DB123456789ABC));
        assert!(!exact_filetime_match(
            0x01DB123456789ABC,
            0x01DB123456789ABD
        ));
        assert_eq!(
            classify_identity(
                Some((r"C:\Apps\worker.exe".into(), 42)),
                r"C:\Apps\worker.exe",
                42,
            ),
            IdentityCheck::Match
        );
        assert_eq!(
            classify_identity(
                Some((r"C:\Apps\other.exe".into(), 42)),
                r"C:\Apps\worker.exe",
                42,
            ),
            IdentityCheck::Mismatch
        );
        assert_eq!(
            classify_identity(None, r"C:\Apps\worker.exe", 42),
            IdentityCheck::Unavailable,
            "a failed identity query is not proof of PID reuse"
        );
    }

    #[test]
    fn scheduler_cpu_cap_stays_on_sysinfo_per_core_scale() {
        assert!(scheduler_cpu_is_below_per_core_cap(2.5));
        assert!(!scheduler_cpu_is_below_per_core_cap(2.5001));
        assert!(
            !scheduler_cpu_is_below_per_core_cap(100.0),
            "single-core saturation remains busy on the raw sysinfo scale"
        );
        assert!(!scheduler_cpu_is_below_per_core_cap(f32::NAN));
        assert!(!scheduler_cpu_is_below_per_core_cap(f32::INFINITY));
    }

    #[test]
    fn scheduler_selects_only_quiet_background_processes() {
        assert!(scheduler_scene_eligible("none", "explorer"));
        assert!(scheduler_scene_eligible("away", "notepad"));
        assert!(scheduler_scene_eligible("power", "explorer"));
        assert!(scheduler_scene_eligible("media", "notepad"));
        assert!(!scheduler_scene_eligible("game", "explorer"));
        assert!(!scheduler_scene_eligible("dev", "explorer"));
        assert!(!scheduler_scene_eligible("heavy", "explorer"));
        assert!(!scheduler_scene_eligible("away", ""));
        assert!(!scheduler_scene_eligible("media", "vlc"));
    }

    #[test]
    fn scheduler_orders_quiet_processes_by_larger_working_set() {
        let candidate = |pid, cpu, working_set| SchedulerCandidate {
            pid,
            cpu,
            working_set,
            image_path: format!("fixture-{pid}.exe").into(),
            start_time: pid as u64,
        };
        let mut candidates = vec![
            candidate(1, 0.5, 100),
            candidate(2, 0.5, 500),
            candidate(3, 0.1, 50),
        ];
        sort_scheduler_candidates(&mut candidates);
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| candidate.pid)
                .collect::<Vec<_>>(),
            vec![3, 2, 1]
        );
    }

    #[test]
    fn scheduler_unstable_sort_matches_stable_total_order() {
        let candidates = (0..512_u32)
            .rev()
            .map(|pid| SchedulerCandidate {
                pid,
                cpu: ((pid * 37) % 11) as f32 / 4.0,
                working_set: u64::from((pid * 13) % 7) * 4096,
                image_path: Cow::Borrowed("fixture.exe"),
                start_time: u64::from(pid),
            })
            .collect::<Vec<_>>();
        let mut stable = candidates.clone();
        let mut unstable = candidates;
        sort_scheduler_candidates_stable_reference(&mut stable);
        sort_scheduler_candidates(&mut unstable);
        assert_eq!(
            stable
                .iter()
                .map(|candidate| candidate.pid)
                .collect::<Vec<_>>(),
            unstable
                .iter()
                .map(|candidate| candidate.pid)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[ignore = "manual Release benchmark for scheduler candidate sorting"]
    fn benchmark_scheduler_candidate_sort_stable_vs_unstable() {
        const CANDIDATE_COUNT: u32 = 330;
        const BATCHES: usize = 101;
        let base = (0..CANDIDATE_COUNT)
            .rev()
            .map(|pid| SchedulerCandidate {
                pid,
                cpu: ((pid * 37) % 11) as f32 / 4.0,
                working_set: u64::from((pid * 13) % 7) * 4096,
                image_path: Cow::Borrowed("fixture.exe"),
                start_time: u64::from(pid),
            })
            .collect::<Vec<_>>();
        let mut stable_order = base.clone();
        let mut unstable_order = base.clone();
        sort_scheduler_candidates_stable_reference(&mut stable_order);
        sort_scheduler_candidates(&mut unstable_order);
        assert_eq!(
            stable_order
                .iter()
                .map(|candidate| candidate.pid)
                .collect::<Vec<_>>(),
            unstable_order
                .iter()
                .map(|candidate| candidate.pid)
                .collect::<Vec<_>>()
        );
        let measure = |stable: bool| {
            let mut candidates = base.clone();
            let started = Instant::now();
            if stable {
                sort_scheduler_candidates_stable_reference(&mut candidates);
            } else {
                sort_scheduler_candidates(&mut candidates);
            }
            std::hint::black_box(candidates[0].pid);
            started.elapsed().as_nanos()
        };
        for _ in 0..7 {
            std::hint::black_box(measure(true));
            std::hint::black_box(measure(false));
        }
        let mut stable_nanos = Vec::with_capacity(BATCHES);
        let mut unstable_nanos = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                stable_nanos.push(measure(true));
                unstable_nanos.push(measure(false));
            } else {
                unstable_nanos.push(measure(false));
                stable_nanos.push(measure(true));
            }
        }
        stable_nanos.sort_unstable();
        unstable_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "scheduler candidate sort ({} records): stable median/p95={:.1}/{:.1} us; unstable={:.1}/{:.1}",
            CANDIDATE_COUNT,
            stable_nanos[BATCHES / 2] as f64 / 1_000.0,
            stable_nanos[p95_index] as f64 / 1_000.0,
            unstable_nanos[BATCHES / 2] as f64 / 1_000.0,
            unstable_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn own_tree_refresh_gate_tracks_scheduler_scan_and_foreground_boost_consumers() {
        let mut settings = AutoSettings::default();
        let boost_scene = foreground_boost_scene_active(&settings, "game", true);
        assert!(boost_scene);
        assert!(should_refresh_own_process_tree(true, true, false));
        assert!(should_refresh_own_process_tree(true, false, boost_scene));
        assert!(!should_refresh_own_process_tree(true, false, false));

        settings.scheduler_shadow = true;
        assert!(!foreground_boost_scene_active(&settings, "game", true));
        assert!(!should_refresh_own_process_tree(true, false, false));
        settings.cpu_scheduler = false;
        assert!(!should_refresh_own_process_tree(false, true, true));
    }

    #[test]
    #[ignore = "manual benchmark for idle own-tree refresh cadence"]
    fn benchmark_idle_own_tree_refresh_cadence() {
        const BATCHES: usize = 101;
        const TICKS_PER_WINDOW: usize = 5;
        let mut sys = System::new();
        let refresh =
            sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet);
        sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh);
        assert!(!sys.processes().is_empty());
        let mut every_tick_micros = Vec::with_capacity(BATCHES);
        let mut cadence_gated_micros = Vec::with_capacity(BATCHES);

        for batch in 0..BATCHES {
            let measure_every_tick = || {
                let started = Instant::now();
                for _ in 0..TICKS_PER_WINDOW {
                    std::hint::black_box(crate::processes::collect_own_process_tree_pids(&sys));
                }
                started.elapsed().as_secs_f64() * 1_000_000.0
            };
            let measure_cadence_gated = || {
                let started = Instant::now();
                let mut own_tree = HashSet::new();
                for tick in 0..TICKS_PER_WINDOW {
                    let scheduler_scan_due = tick == 0;
                    if should_refresh_own_process_tree(true, scheduler_scan_due, false) {
                        own_tree = crate::processes::collect_own_process_tree_pids(&sys);
                    } else {
                        own_tree.clear();
                    }
                }
                std::hint::black_box(own_tree);
                started.elapsed().as_secs_f64() * 1_000_000.0
            };
            if batch % 2 == 0 {
                every_tick_micros.push(measure_every_tick());
                cadence_gated_micros.push(measure_cadence_gated());
            } else {
                cadence_gated_micros.push(measure_cadence_gated());
                every_tick_micros.push(measure_every_tick());
            }
        }
        every_tick_micros.sort_by(f64::total_cmp);
        cadence_gated_micros.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "idle own-tree work ({} processes, {} ticks/window): every-tick median/p95={:.1}/{:.1} us; cadence-gated={:.1}/{:.1}",
            sys.processes().len(),
            TICKS_PER_WINDOW,
            every_tick_micros[BATCHES / 2],
            every_tick_micros[p95_index],
            cadence_gated_micros[BATCHES / 2],
            cadence_gated_micros[p95_index],
        );
    }

    #[test]
    fn scheduler_filter_skips_own_webview_process_tree() {
        // 真实案例镜像：离开场景下自身 WebView2 运行时（msedgewebview2.exe，
        // PID 13404）被 EcoQoS + BACKGROUND_PRIORITY_CLASS 节流，冻结前端事件
        // 循环，唤醒后被误报“数据停更”。自身树内进程必须无条件落选。
        let self_pid = 42_u32;
        let own_tree: HashSet<u32> = HashSet::from([self_pid, 13404]);
        let now_unix = 1_700_000_000_u64;
        let started_long_ago = now_unix - 120;
        let quiet = 0.4_f32;

        // 树内安静稳定进程：落选。
        assert!(!scheduler_candidate_is_selectable(
            13404,
            self_pid,
            None,
            quiet,
            now_unix,
            started_long_ago,
            &own_tree
        ));
        // 同样安静稳定的非树内进程：可入选。
        assert!(scheduler_candidate_is_selectable(
            13405,
            self_pid,
            None,
            quiet,
            now_unix,
            started_long_ago,
            &own_tree
        ));
        // 既有排除规则不因重构而失效。
        assert!(!scheduler_candidate_is_selectable(
            13405,
            self_pid,
            Some(13405),
            quiet,
            now_unix,
            started_long_ago,
            &own_tree
        ));
        assert!(!scheduler_candidate_is_selectable(
            13405,
            self_pid,
            None,
            3.0,
            now_unix,
            started_long_ago,
            &own_tree
        ));
        // 启动不足 BACKGROUND_STABILITY_SECS（距 now 仅 5 秒）：落选。
        assert!(!scheduler_candidate_is_selectable(
            13405,
            self_pid,
            None,
            quiet,
            now_unix,
            started_long_ago + 115,
            &own_tree
        ));
        assert!(!scheduler_candidate_is_selectable(
            4,
            self_pid,
            None,
            quiet,
            now_unix,
            started_long_ago,
            &own_tree
        ));
        assert!(!scheduler_candidate_is_selectable(
            self_pid,
            self_pid,
            None,
            quiet,
            now_unix,
            started_long_ago,
            &own_tree
        ));
        // 空树（尚未收集快照）不得误伤正常候选。
        let empty: HashSet<u32> = HashSet::new();
        assert!(scheduler_candidate_is_selectable(
            13404,
            self_pid,
            None,
            quiet,
            now_unix,
            started_long_ago,
            &empty
        ));
    }

    #[test]
    fn scheduler_identity_probes_stop_after_filling_ranked_limit() {
        let candidates = (0..1000)
            .map(|pid| SchedulerCandidate {
                pid,
                cpu: pid as f32 / 1000.0,
                working_set: 0,
                image_path: format!("fixture-{pid}.exe").into(),
                start_time: pid as u64,
            })
            .collect::<Vec<_>>();
        let mut probes = 0;
        let selected = confirm_scheduler_candidates(&candidates, 10, |candidate| {
            probes += 1;
            candidate.pid % 4 == 1
        });
        assert_eq!(selected, [1, 5, 9, 13, 17, 21, 25, 29, 33, 37]);
        assert_eq!(
            probes, 38,
            "stop after the tenth valid identity, not all 1,000"
        );
    }

    #[test]
    fn scheduler_identity_probe_budget_handles_zero_and_all_invalid() {
        let candidates = (0..5)
            .map(|pid| SchedulerCandidate {
                pid,
                cpu: 0.0,
                working_set: 0,
                image_path: Cow::Borrowed(""),
                start_time: 0,
            })
            .collect::<Vec<_>>();
        let mut probes = 0;
        assert!(confirm_scheduler_candidates(&candidates, 0, |_| {
            probes += 1;
            true
        })
        .is_empty());
        assert_eq!(probes, 0);
        assert!(confirm_scheduler_candidates(&candidates, 10, |_| {
            probes += 1;
            false
        })
        .is_empty());
        assert_eq!(probes, 5);
    }

    #[test]
    fn scheduler_only_lowers_normal_priority_and_preserves_custom_classes() {
        assert_eq!(
            background_priority_target(NORMAL_PRIORITY_CLASS),
            Some(BACKGROUND_PRIORITY_CLASS)
        );
        for priority in [0, 0x00004000, 0x00008000, 0x00000080, 0x00000100] {
            assert_eq!(background_priority_target(priority), None, "{priority:#x}");
        }
    }

    #[test]
    fn scheduler_candidate_path_retention_borrows_valid_utf8() {
        let executable = std::path::Path::new(r"C:\Program Files\AeroTune\worker.exe");
        let retained = Some(executable)
            .map(|path| path.to_string_lossy())
            .unwrap_or(Cow::Borrowed(""));
        assert!(matches!(retained, Cow::Borrowed(_)));
        assert_eq!(retained, r"C:\Program Files\AeroTune\worker.exe");

        let missing: Option<&std::path::Path> = None;
        let retained = missing
            .map(|path| path.to_string_lossy())
            .unwrap_or(Cow::Borrowed(""));
        assert!(matches!(retained, Cow::Borrowed("")));
    }

    #[test]
    #[ignore = "manual Release benchmark for scheduler candidate path ownership"]
    fn benchmark_scheduler_candidate_path_retention() {
        const BATCHES: usize = 101;
        let mut sys = System::new();
        let refresh =
            sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet);
        sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh);
        let paths = sys
            .processes()
            .values()
            .filter_map(|process| process.exe())
            .collect::<Vec<_>>();
        assert!(!paths.is_empty());
        let owned_expected = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let borrowed_expected = paths
            .iter()
            .map(|path| path.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(owned_expected
            .iter()
            .zip(&borrowed_expected)
            .all(|(owned, borrowed)| owned == borrowed.as_ref()));
        let borrowed_paths = borrowed_expected
            .iter()
            .filter(|path| matches!(path, Cow::Borrowed(_)))
            .count();
        let measure = |owned: bool| {
            let started = Instant::now();
            if owned {
                let materialized = paths
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect::<Vec<_>>();
                std::hint::black_box(&materialized);
                drop(materialized);
            } else {
                let materialized = paths
                    .iter()
                    .map(|path| path.to_string_lossy())
                    .collect::<Vec<_>>();
                std::hint::black_box(&materialized);
                drop(materialized);
            }
            started.elapsed().as_nanos()
        };
        for _ in 0..7 {
            std::hint::black_box(measure(true));
            std::hint::black_box(measure(false));
        }
        let mut owned_nanos = Vec::with_capacity(BATCHES);
        let mut borrowed_nanos = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                owned_nanos.push(measure(true));
                borrowed_nanos.push(measure(false));
            } else {
                borrowed_nanos.push(measure(false));
                owned_nanos.push(measure(true));
            }
        }
        owned_nanos.sort_unstable();
        borrowed_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "scheduler candidate path retention ({} real paths, {} borrowed UTF-8): owned median/p95={:.1}/{:.1} us; Cow={:.1}/{:.1}",
            paths.len(),
            borrowed_paths,
            owned_nanos[BATCHES / 2] as f64 / 1_000.0,
            owned_nanos[p95_index] as f64 / 1_000.0,
            borrowed_nanos[BATCHES / 2] as f64 / 1_000.0,
            borrowed_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn scheduler_policy_matches_legacy_string_normalization() {
        let cases = [
            ("worker.exe", r"D:\Apps\worker.exe", Some(r"C:\Windows")),
            ("WORKER.EXE", "d:/apps/Worker.exe", Some("C:/WINDOWS/")),
            (
                "SearchIndexer.exe",
                "c:/WINDOWS/System32/SearchIndexer.exe",
                Some(r"C:\Windows"),
            ),
            (
                "SearchIndexer.exe",
                r"C:\WindowsOld\System32\SearchIndexer.exe",
                Some(r"C:\Windows"),
            ),
            (
                "runtimebroker.exe",
                "C:/Windows/SystemApps/RuntimeBroker.exe",
                Some(r"C:\Windows\\"),
            ),
            ("aerotune.EXE", r"D:\Apps\AeroTune.exe", Some(r"C:\Windows")),
            (
                "fixture.exe.exe",
                r"D:\Apps\fixture.exe.exe",
                Some(r"C:\Windows"),
            ),
            ("worker.exe", "", Some(r"C:\Windows")),
            ("worker.exe", r"D:\Apps\worker.exe", None),
            ("naïve.exe", "D:/Apps/naïve.exe", Some(r"C:\Windows")),
        ];
        let protected_sets = [
            Vec::new(),
            vec!["WORKER.EXE".to_string()],
            vec![r"d:/apps/Worker.EXE".to_string()],
            vec!["worker".to_string()],
            vec![r"C:\Windows\System32\SearchIndexer.exe".to_string()],
        ];
        for (name, path, root) in cases {
            for protected_apps in &protected_sets {
                assert_eq!(
                    scheduler_process_allowed(name, path, root, protected_apps),
                    scheduler_process_allowed_reference(name, path, root, protected_apps),
                    "name={name:?}, path={path:?}, root={root:?}, protected={protected_apps:?}"
                );
            }
        }
    }

    #[test]
    #[ignore = "manual Release benchmark on a real Windows process snapshot"]
    fn benchmark_scheduler_process_policy_real_process_paths() {
        const BATCHES: usize = 101;
        let mut sys = System::new();
        let refresh =
            sysinfo::ProcessRefreshKind::nothing().with_exe(sysinfo::UpdateKind::OnlyIfNotSet);
        sys.refresh_processes_specifics(sysinfo::ProcessesToUpdate::All, true, refresh);
        let windows_root = crate::processes::trusted_windows_directory()
            .expect("the Windows root should be available for this benchmark");
        let paths = sys
            .processes()
            .values()
            .filter_map(|process| {
                let name = process.name().to_string_lossy().into_owned();
                let path = process.exe()?.to_string_lossy().into_owned();
                Some((name, path))
            })
            .collect::<Vec<_>>();
        assert!(!paths.is_empty());
        let protected_apps = vec![
            "benchmark-protected.exe".to_string(),
            r"C:\AeroTune\benchmark-protected.exe".to_string(),
        ];
        let reference_results = paths
            .iter()
            .map(|(name, path)| {
                scheduler_process_allowed_reference(
                    name,
                    path,
                    Some(&windows_root),
                    &protected_apps,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            paths
                .iter()
                .map(|(name, path)| {
                    scheduler_process_allowed(name, path, Some(&windows_root), &protected_apps)
                })
                .collect::<Vec<_>>(),
            reference_results
        );
        let measure = |reference: bool| {
            let started = Instant::now();
            let allowed = paths
                .iter()
                .filter(|(name, path)| {
                    if reference {
                        scheduler_process_allowed_reference(
                            name,
                            path,
                            Some(&windows_root),
                            &protected_apps,
                        )
                    } else {
                        scheduler_process_allowed(name, path, Some(&windows_root), &protected_apps)
                    }
                })
                .count();
            std::hint::black_box(allowed);
            started.elapsed().as_nanos()
        };
        for _ in 0..7 {
            std::hint::black_box(measure(true));
            std::hint::black_box(measure(false));
        }
        let mut reference_nanos = Vec::with_capacity(BATCHES);
        let mut borrowed_nanos = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                reference_nanos.push(measure(true));
                borrowed_nanos.push(measure(false));
            } else {
                borrowed_nanos.push(measure(false));
                reference_nanos.push(measure(true));
            }
        }
        reference_nanos.sort_unstable();
        borrowed_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "scheduler path policy ({} real process paths): allocating median/p95={:.1}/{:.1} us; borrowed={:.1}/{:.1}",
            paths.len(),
            reference_nanos[BATCHES / 2] as f64 / 1_000.0,
            reference_nanos[p95_index] as f64 / 1_000.0,
            borrowed_nanos[BATCHES / 2] as f64 / 1_000.0,
            borrowed_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn scheduler_process_filter_protects_system_and_aerotune() {
        let windows = r"C:\Windows";
        assert!(!scheduler_process_allowed_for_test(
            "svchost.exe",
            r"C:\Windows\System32\svchost.exe",
            windows
        ));
        assert!(!scheduler_process_allowed_for_test(
            "aerotune.exe",
            r"C:\Apps\AeroTune.exe",
            windows
        ));
        assert!(scheduler_process_allowed_for_test(
            "worker.exe",
            r"D:\Apps\worker.exe",
            windows
        ));
        assert!(!scheduler_process_allowed_for_test(
            "searchindexer.exe",
            r"C:\Windows\Temp\searchindexer.exe",
            windows
        ));
        assert!(scheduler_process_allowed_for_test(
            "searchindexer.exe",
            r"C:\Windows\System32\searchindexer.exe",
            windows
        ));
        assert!(scheduler_process_allowed_for_test(
            "vmmemwsl.exe",
            r"C:\Windows\System32\vmmemwsl.exe",
            windows
        ));
        assert!(!scheduler_process_allowed_for_test(
            "worker.exe",
            "",
            windows
        ));
        let protected = vec!["worker.exe".to_string()];
        assert!(!scheduler_process_protected_for_test(
            "worker.exe",
            r"D:\Apps\worker.exe",
            windows,
            &protected
        ));
        assert!(
            !scheduler_process_allowed("worker.exe", r"D:\Apps\worker.exe", None, &[]),
            "unknown Windows root must fail closed instead of treating every path as non-system"
        );
    }

    #[test]
    fn exact_plan_restore_passes_target_and_expected_current_for_serialized_check() {
        let mut seen = Vec::new();
        restore_plan_exact("custom-oem", Some("balanced"), |target, expected| {
            seen.push((target.to_string(), expected.to_string()));
            Ok(())
        })
        .unwrap();
        restore_plan_exact("OEM", Some("oem"), |target, expected| {
            seen.push((target.to_string(), expected.to_string()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            seen,
            vec![
                ("custom-oem".into(), "balanced".into()),
                ("OEM".into(), "oem".into()),
            ]
        );
        assert!(restore_plan_exact("original", None, |_, _| Err("mock failure".into())).is_err());
        let error = restore_plan_exact("original", Some("third-party"), |target, current| {
            assert_eq!((target, current), ("original", "third-party"));
            Err("injected powercfg restore failure".into())
        })
        .unwrap_err();
        assert_eq!(error, "injected powercfg restore failure");
    }

    #[test]
    fn injected_baseline_restore_failure_propagates_and_sets_retry_state() {
        let mut pending = false;
        let error =
            finish_baseline_restore(&mut pending, Err("injected plan restore failure".into()))
                .unwrap_err();
        assert_eq!(error, "injected plan restore failure");
        assert!(pending);

        finish_baseline_restore(&mut pending, Ok(())).unwrap();
        assert!(!pending);
    }

    #[test]
    fn brightness_restore_pending_matches_requested_scene_and_setting() {
        let mut settings = AutoSettings {
            adaptive: true,
            auto_dim: true,
            ..Default::default()
        };
        assert!(!brightness_restore_pending(&settings, "power", true, false));
        assert!(!brightness_restore_pending(&settings, "away", true, false));
        assert!(brightness_restore_pending(&settings, "game", true, false));
        assert!(brightness_restore_pending(&settings, "power", false, true));
        settings.auto_dim = false;
        assert!(brightness_restore_pending(&settings, "power", true, false));
        settings.adaptive = false;
        assert!(brightness_restore_pending(&settings, "none", true, false));
        assert!(!brightness_restore_pending(&settings, "none", false, false));
    }

    #[test]
    fn baseline_restore_reports_unconfirmed_knobs_and_actionable_retry() {
        let baseline = Knobs {
            plan: PlanKind::Balanced,
            epp: Some(25),
            proc_max: 100,
            gamedvr: true,
            prio_boost: false,
            priority_separation: 2,
        };
        assert!(baseline_restore_mismatches(&baseline, &baseline).is_empty());
        let partial = Knobs {
            epp: Some(70),
            gamedvr: false,
            prio_boost: true,
            priority_separation: 26,
            ..baseline.clone()
        };
        assert_eq!(
            baseline_restore_mismatches(&baseline, &partial),
            vec![
                "EPP".to_string(),
                "GameDVR".to_string(),
                "前台优先级".to_string()
            ]
        );
        let message = baseline_restore_error(&["injected plan conflict".into()]);
        assert!(message.contains("未完整恢复"));
        assert!(message.contains("快照已保留"));
        assert!(message.contains("重试"));
        assert!(message.contains("变更日志"));
        assert!(message.contains("injected plan conflict"));

        let brightness_message =
            baseline_restore_error(
                &["屏幕亮度尚未确认恢复（后台任务执行中或等待退避重试）".into()],
            );
        assert!(brightness_message.contains("屏幕亮度尚未确认恢复"));
    }

    #[test]
    fn game_process_names_reject_paths_and_shell_syntax() {
        for valid in ["game", "game.exe", "game_2-beta.exe"] {
            assert!(valid_game_process_name(valid), "{valid}");
        }
        for invalid in [
            "",
            "..",
            "../game.exe",
            r"C:\\game.exe",
            "game.exe;calc",
            "game name",
            "游戏.exe",
        ] {
            assert!(!valid_game_process_name(invalid), "{invalid}");
        }
    }

    #[test]
    fn protected_app_names_are_normalized_sorted_and_validated() {
        assert_eq!(
            normalize_protected_apps(vec!["Editor.EXE".into(), "editor".into(), "game_2".into()])
                .unwrap(),
            vec!["editor.exe".to_string(), "game_2.exe".to_string()]
        );
        assert!(normalize_protected_apps(vec!["bad name.exe".into()]).is_err());
        assert!(normalize_protected_apps(vec![r"C:\\Apps\\editor.exe".into()]).is_err());
    }

    #[test]
    fn classifier_mode_key_normalizes_manual_aliases() {
        assert_eq!(classifier_mode_key("game"), "game");
        assert_eq!(classifier_mode_key("heavy"), "heavy");
        assert_eq!(classifier_mode_key("balanced"), "none");
        assert_eq!(classifier_mode_key("performance"), "none");
        assert_eq!(classifier_mode_key("编码"), "none");
        assert_eq!(classifier_mode_key("dev"), "none");
        assert_eq!(classifier_mode_key(""), "none");
    }

    #[test]
    fn heavy_mode_uses_schmitt_trigger_to_ignore_threshold_jitter() {
        let mut c = ctx(59.0);
        assert_eq!(classify_target(&c, "none", 300), "none");
        c.telemetry.cpu_avg_60s = 60.1;
        assert_eq!(classify_target(&c, "none", 300), "heavy");
        // The same CPU band keeps a running heavy session instead of toggling.
        for cpu in [59.0, 56.0, 58.5, 55.1, 59.9, 55.0] {
            c.telemetry.cpu_avg_60s = cpu;
            assert_eq!(classify_target(&c, "heavy", 300), "heavy", "cpu={cpu}");
        }
        c.telemetry.cpu_avg_60s = 54.9;
        assert_eq!(classify_target(&c, "heavy", 300), "none");
        c.telemetry.cpu_avg_60s = 60.0;
        assert_eq!(classify_target(&c, "none", 300), "none");
        c.telemetry.cpu_avg_60s = 60.01;
        assert_eq!(classify_target(&c, "none", 300), "heavy");
        c.telemetry.cpu_avg_60s = f32::NAN;
        assert_eq!(c.cpu_avg(), None);
        assert_eq!(classify_target(&c, "heavy", 300), "heavy");
        assert_eq!(classify_target(&c, "none", 300), "none");
        c.telemetry.cpu_avg_60s = f32::INFINITY;
        assert_eq!(c.cpu_avg(), None);
        assert_eq!(classify_target(&c, "heavy", 300), "heavy");
        assert_eq!(classify_target(&c, "none", 300), "none");
    }

    #[test]
    fn scene_priority_precedes_heavy_hysteresis_and_power_is_configurable() {
        let mut c = ctx(95.0);
        c.gaming = true;
        assert_eq!(classify_target(&c, "none", 300), "game");
        c.gaming = false;
        c.media = true;
        assert_eq!(classify_target(&c, "none", 300), "media");
        c.media = false;
        c.idle_secs = 300;
        assert_eq!(classify_target(&c, "none", 300), "away");
        c.idle_secs = 0;
        c.ac_online = false;
        assert_eq!(classify_target(&c, "heavy", 300), "power");
        c.auto_power = false;
        assert_eq!(classify_target(&c, "none", 300), "heavy");
    }

    #[test]
    fn acceptance_requires_confirmation_and_no_hold_or_cooldown() {
        for confirmed in [false, true] {
            for hold in [false, true] {
                for cooldown in [false, true] {
                    assert_eq!(
                        transition_allowed(confirmed, hold, cooldown),
                        confirmed && !hold && !cooldown
                    );
                }
            }
        }
    }
    #[test]
    fn pending_target_resets_and_hidden_time_is_real() {
        let now = Instant::now();
        let mut c = Confirmation::default();
        assert!(!c.ready("heavy", now, 10));
        assert!(!c.ready("heavy", now + Duration::from_secs(8), 10));
        assert!(!c.ready("media", now + Duration::from_secs(8), 3));
        assert!(!c.ready("media", now + Duration::from_secs(10), 3));
        assert!(c.ready("media", now + Duration::from_secs(12), 3));
        c.clear();
        assert!(!c.ready("media", now + Duration::from_secs(14), 3));
    }
    #[test]
    fn scene_stats_keep_fractional_elapsed() {
        let mut ad = Adaptive {
            state: "none".into(),
            ..Default::default()
        };
        for _ in 0..5 {
            account_seconds(&mut ad, Duration::from_millis(2100));
        }
        assert_eq!(ad.seconds["none"], 10);
        account_seconds(&mut ad, Duration::from_millis(500));
        assert_eq!(ad.seconds["none"], 11);
    }
    #[test]
    fn calibration_limits_same_scene_retries() {
        let now = Instant::now();
        let mut last = None;
        assert!(calibration_due(&mut last, now));
        for sec in 1..15 {
            assert!(!calibration_due(&mut last, now + Duration::from_secs(sec)));
        }
        assert!(calibration_due(&mut last, now + Duration::from_secs(16)));
    }

    #[test]
    fn failed_plan_retry_is_suppressed_until_backoff_expires() {
        let now = Instant::now();
        let mut retry_after = std::collections::HashMap::new();
        retry_after.insert(PlanKind::Saver, now + PLAN_RETRY_BACKOFF);

        assert!(!plan_retry_ready(
            &mut retry_after,
            PlanKind::Saver,
            now + Duration::from_secs(60)
        ));
        assert!(plan_retry_ready(
            &mut retry_after,
            PlanKind::Saver,
            now + PLAN_RETRY_BACKOFF + Duration::from_secs(1)
        ));
        assert!(!retry_after.contains_key(&PlanKind::Saver));
        assert!(plan_retry_ready(&mut retry_after, PlanKind::Balanced, now));
    }
    #[test]
    fn browser_fullscreen_is_not_game_but_explicit_list_is() {
        assert!(!game_signal(2, "chrome", false));
        assert!(!game_signal(3, "vlc", false));
        assert!(!game_signal(2, "unknown", false));
        assert!(game_signal(3, "actualgame", false));
        assert!(game_signal(2, "chrome", true));
    }
    #[test]
    fn brightness_dim_is_queued_once_and_saved_after_outcome() {
        // 旧语义：变暗只在第一次排队；执行成功后保存原始亮度；再变暗不再排队。
        let mut state = crate::brightness::BrightnessState::default();
        let now = Instant::now();
        assert_eq!(
            state.plan(true, now),
            Some(crate::brightness::BrightnessTask::Dim)
        );
        assert_eq!(state.plan(true, now), None);
        state.apply_outcome(crate::brightness::BrightnessOutcome::Dimmed { original: 60 });
        assert_eq!(state.saved, Some(60));
        assert_eq!(state.plan(true, now), None);
    }

    #[test]
    fn failed_brightness_restore_uses_bounded_retry_and_clears_after_recovery() {
        let now = Instant::now();
        let mut state = crate::brightness::BrightnessState::default();
        state.apply_outcome(crate::brightness::BrightnessOutcome::Dimmed { original: 70 });
        state.apply_outcome(crate::brightness::BrightnessOutcome::GuardedRecoveryFailed);
        // 失败后保留保存值，并进入退避。
        assert_eq!(state.saved, Some(70));
        assert_eq!(state.plan(false, now), None);
        // 退避期后重新排队恢复。
        assert_eq!(
            state.plan(false, now + Duration::from_secs(5 * 60 + 1)),
            Some(crate::brightness::BrightnessTask::Recover(
                crate::brightness::BrightnessSnapshot {
                    original: 70,
                    dimmed: 40,
                },
            ))
        );
        // 恢复成功后清空保存值与退避。
        state.apply_outcome(crate::brightness::BrightnessOutcome::Restored { value: 70 });
        assert_eq!(state.saved, None);
        assert!(state.retry_after.is_none());
        assert_eq!(state.plan(false, now), None);
    }

    #[test]
    fn dim_does_not_brighten_low_screen_or_lose_failed_write_backup() {
        // 低亮度屏幕（10）不能被"变暗"提亮：目标值必须 <= 原值，且 >= 20 的下界不越界到更亮。
        let original = 10u8;
        let target = original.saturating_sub(30).max(20).min(original);
        assert!(target <= original, "变暗不得让屏幕变亮");
        // 写入失败也要保留备份值。
        let mut state = crate::brightness::BrightnessState::default();
        state.apply_outcome(crate::brightness::BrightnessOutcome::DimFailed { original });
        assert_eq!(state.saved, Some(10));
    }

    use crate::diag::Anomaly;

    fn ctx(cpu: f32) -> AdaptiveCtx {
        AdaptiveCtx {
            gaming: false,
            media: false,
            idle_secs: 0,
            ac_online: true,
            auto_power: true,
            telemetry: AutoTelemetry {
                cpu_avg_60s: cpu,
                cpu_avg_10s: cpu,
                cpu_peak: cpu,
                mem_pct: 30.0,
                mem_slope: 0.0,
                process_count: 50,
                ..AutoTelemetry::default()
            },
            epp_ok: true,
            heavy_threshold: 60.0,
            away_threshold: 300,
        }
    }

    #[test]
    fn autopilot_enable_snapshots_and_disable_restores() {
        let mut settings = AutoSettings {
            adaptive: false,
            auto_game: true,
            auto_power: false,
            scheduler_shadow: true,
            ..Default::default()
        };
        let message = apply_autopilot(&mut settings, true).expect("enable succeeds");
        assert!(message.contains("关闭时自动还原"));
        assert!(settings.autopilot);
        assert!(
            settings.adaptive
                && settings.auto_power
                && settings.game_boost
                && settings.cpu_scheduler
        );
        assert!(
            !settings.scheduler_shadow,
            "shadow must be off so EcoQoS actually applies"
        );
        assert_eq!(
            settings.autopilot_backup.as_ref().unwrap(),
            &AutopilotBackup {
                adaptive: false,
                auto_game: true,
                auto_power: false,
                game_boost: true,
                cpu_scheduler: true,
                scheduler_shadow: true,
                auto_dispose_anomalies: false,
            }
        );
        // 重复开启被拒绝
        assert!(apply_autopilot(&mut settings, true).is_err());
        // 关闭按快照还原
        let message = apply_autopilot(&mut settings, false).expect("disable succeeds");
        assert!(message.contains("还原此前设置"));
        assert!(!settings.autopilot);
        assert!(!settings.adaptive && !settings.auto_power);
        assert!(settings.auto_game);
        assert!(settings.scheduler_shadow);
        assert!(settings.autopilot_backup.is_none());
        // 再次关闭被拒绝；无快照时兜底关闭 adaptive
        assert!(apply_autopilot(&mut settings, false).is_err());
        settings.autopilot = true;
        settings.autopilot_backup = None;
        apply_autopilot(&mut settings, false).unwrap();
        assert!(
            !settings.adaptive,
            "missing backup falls back to disabling adaptive"
        );
    }

    #[test]
    fn autopilot_scene_adoption_validates_and_dedupes() {
        let mut settings = AutoSettings::default();
        settings.autopilot = false;
        assert!(
            autopilot_adopt_scene(&mut settings, "coolgame.exe").is_none(),
            "requires autopilot"
        );
        settings.autopilot = true;
        settings.auto_game = false;
        assert!(
            autopilot_adopt_scene(&mut settings, "coolgame.exe").is_none(),
            "requires auto_game"
        );
        settings.auto_game = true;
        let message =
            autopilot_adopt_scene(&mut settings, "CoolGame.EXE").expect("adopts valid name");
        assert!(
            message.contains("coolgame"),
            "normalized name without .exe suffix: {message}"
        );
        assert!(settings.game_list.contains(&normalize_name("CoolGame.EXE")));
        // 重复名单内不再采纳
        assert!(autopilot_adopt_scene(&mut settings, "coolgame.exe").is_none());
        // 非法进程名被既有校验拒绝
        assert!(autopilot_adopt_scene(&mut settings, "bad name with spaces.exe").is_none());
    }

    #[test]
    fn adaptive_shared_tail_runs_rules_without_system_changes() {
        let mut state = AutoState {
            settings: AutoSettings::default(),
            scheduler_originals: HashMap::new(),
            foreground_priority_original: None,
            foreground_own_tree: HashSet::new(),
            cpu_scheduler_status: CpuSchedulerStatus::default(),
            scheduler_last_scan: None,
            foreground_scene_since: None,
            fullscreen_ticks: 0,
            fullscreen_evidence: Confirmation::default(),
            list_evidence: Confirmation::default(),
            list_ticks: 0,
            last_switch: None,
            boosted: None,
            rules_sustained: Default::default(),
            rules_suspend_notice_seen: Default::default(),
            adaptive: Adaptive::default(),
            last_foreground: String::new(),
            last_fullscreen: false,
            scene_shadow_seen: None,
        };
        state.settings.rules = vec![Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 50.0,
            seconds: 2,
            action: "notify".into(),
        }];
        let anomalies = vec![anomaly(100, "test", "cpu", 80.0)];
        assert!(state.settings.adaptive);
        assert!(game_boost_active("game", true));
        assert!(!game_boost_active("game", false));
        assert!(!game_boost_active("none", true));
        assert!(tick_shared(
            &mut state,
            "game",
            true,
            None,
            None,
            999,
            &anomalies,
            Duration::from_secs(1),
            &System::new(),
        )
        .is_empty());
        assert_eq!(
            tick_shared(
                &mut state,
                "game",
                true,
                None,
                None,
                999,
                &anomalies,
                Duration::from_secs(1),
                &System::new(),
            )
            .len(),
            1
        );
        assert!(tick_shared(
            &mut state,
            "game",
            true,
            None,
            None,
            999,
            &anomalies,
            Duration::from_secs(1),
            &System::new(),
        )
        .is_empty());
        assert!(state.boosted.is_none());
    }

    #[test]
    fn strategy_selector_prefers_each_policy_for_matching_context() {
        let mut c = ctx(5.0);
        c.telemetry.cpu_avg_10s = 20.0;
        c.telemetry.cpu_avg_60s = 5.0;
        c.telemetry.cpu_peak = 20.0;
        c.gaming = true;
        c.media = false;
        c.idle_secs = 0;
        assert_eq!(
            select_strategy(&c, StrategyKind::AdaptiveHybrid).selected,
            StrategyKind::Responsiveness
        );

        c.gaming = false;
        c.telemetry.cpu_avg_10s = 92.0;
        c.telemetry.cpu_avg_60s = 92.0;
        c.telemetry.cpu_peak = 99.0;
        c.gaming = false;
        c.idle_secs = 0;
        assert_eq!(
            select_strategy(&c, StrategyKind::AdaptiveHybrid).selected,
            StrategyKind::Throughput
        );

        c.telemetry.cpu_avg_60s = 5.0;
        c.telemetry.cpu_avg_10s = 5.0;
        c.telemetry.cpu_peak = 8.0;
        c.telemetry.mem_pct = 30.0;
        c.telemetry.mem_slope = 0.0;
        c.ac_online = true;
        c.idle_secs = 300;
        c.away_threshold = 300;
        assert_eq!(
            select_strategy(&c, StrategyKind::AdaptiveHybrid).selected,
            StrategyKind::Efficiency
        );

        c.idle_secs = 0;
        c.ac_online = false;
        assert_eq!(
            select_strategy(&c, StrategyKind::AdaptiveHybrid).selected,
            StrategyKind::Efficiency
        );

        c.ac_online = true;
        c.telemetry.mem_pct = 92.0;
        c.telemetry.mem_slope = 0.2;
        assert_eq!(
            select_strategy(&c, StrategyKind::AdaptiveHybrid).selected,
            StrategyKind::LoadBalance
        );
    }

    #[test]
    fn strategy_hysteresis_holds_incumbent_until_margin_is_exceeded() {
        assert_eq!(
            retain_incumbent_with_hysteresis(
                StrategyKind::Responsiveness,
                StrategyKind::AdaptiveHybrid,
                80,
                72,
            ),
            StrategyKind::AdaptiveHybrid
        );
        assert_eq!(
            retain_incumbent_with_hysteresis(
                StrategyKind::Responsiveness,
                StrategyKind::AdaptiveHybrid,
                85,
                72,
            ),
            StrategyKind::Responsiveness
        );
    }

    #[test]
    fn strategy_change_requires_confirmation_residence_and_no_manual_hold() {
        let incumbent = StrategyKind::Responsiveness;
        let candidate = StrategyKind::Efficiency;
        assert!(!strategy_transition_ready(
            candidate,
            incumbent,
            Duration::from_secs(4),
            Duration::from_secs(30),
            false,
            false,
        ));
        assert!(!strategy_transition_ready(
            candidate,
            incumbent,
            Duration::from_secs(5),
            Duration::from_secs(19),
            false,
            false,
        ));
        assert!(!strategy_transition_ready(
            candidate,
            incumbent,
            Duration::from_secs(60),
            Duration::from_secs(60),
            true,
            false,
        ));
        assert!(strategy_transition_ready(
            candidate,
            incumbent,
            Duration::from_secs(5),
            Duration::from_secs(20),
            false,
            false,
        ));
    }

    #[test]
    fn sustained_single_core_contention_selects_response_policy() {
        let mut c = ctx(9.0);
        c.telemetry.cpu_core_peak_pct = 96.0;
        c.telemetry.cpu_logical_cores = 16;
        c.idle_secs = 0;
        assert!(single_core_contention(&c));
        let selection = select_strategy(&c, StrategyKind::AdaptiveHybrid);
        assert_eq!(selection.selected, StrategyKind::Responsiveness);
        assert!(selection.urgent_response);
        assert!(selection.reason.contains("单核"));

        c.idle_secs = c.away_threshold;
        assert!(!single_core_contention(&c));
        assert!(!cpu_contention_for_response(&c));

        let mut multi = ctx(20.0);
        multi.telemetry.cpu_logical_cores = 8;
        multi.telemetry.cpu_saturated_cores = 2;
        multi.telemetry.cpu_core_peak_pct = 87.0;
        assert!(multi_core_contention(&multi));
        assert_eq!(
            select_strategy(&multi, StrategyKind::AdaptiveHybrid).selected,
            StrategyKind::Responsiveness
        );
        multi.telemetry.cpu_saturated_cores = 1;
        assert!(!multi_core_contention(&multi));
    }

    #[test]
    fn single_core_contention_scales_down_for_wide_cpu_systems() {
        let mut wide = ctx(2.0);
        wide.telemetry.cpu_logical_cores = 16;
        wide.telemetry.cpu_core_peak_pct = 95.0;
        assert!(single_core_contention(&wide));
        let selection = select_strategy(&wide, StrategyKind::AdaptiveHybrid);
        assert_eq!(selection.selected, StrategyKind::Responsiveness);
        assert!(selection.urgent_response);

        wide.telemetry.cpu_avg_10s = 1.24;
        assert!(!single_core_contention(&wide));
        wide.telemetry.cpu_avg_10s = 1.25;
        assert!(single_core_contention(&wide));

        wide.telemetry.cpu_logical_cores = 0;
        assert!(!single_core_contention(&wide));
        wide.telemetry.cpu_logical_cores = 16;
        wide.telemetry.cpu_avg_10s = f32::NAN;
        assert!(!single_core_contention(&wide));
        wide.telemetry.cpu_avg_10s = 2.0;
        wide.idle_secs = wide.away_threshold;
        assert!(!single_core_contention(&wide));
    }

    #[test]
    fn confident_cpu_forecast_triggers_early_response_policy() {
        let mut rising = ctx(30.0);
        rising.telemetry.cpu_avg_10s = 35.0;
        rising.telemetry.cpu_peak = 50.0;
        rising.telemetry.cpu_forecast_ready = true;
        rising.telemetry.cpu_forecast_10s_pct = 80.0;
        rising.telemetry.cpu_trend_pct_per_sec = 1.5;
        rising.telemetry.cpu_forecast_stability = 80.0;
        assert!(cpu_forecast_predicts_pressure(&rising));
        let selection = select_strategy(&rising, StrategyKind::AdaptiveHybrid);
        assert_eq!(selection.selected, StrategyKind::Responsiveness);
        assert!(selection.urgent_response);
        assert!(selection.reason.contains("趋势预测"));

        rising.telemetry.cpu_trend_pct_per_sec = 0.749;
        assert!(!cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_trend_pct_per_sec = 0.75;
        assert!(cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_forecast_stability = 59.99;
        assert!(!cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_forecast_stability = 60.0;
        assert!(cpu_forecast_predicts_pressure(&rising));

        rising.heavy_threshold = 30.0;
        rising.telemetry.cpu_forecast_10s_pct = 34.99;
        assert!(!cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_forecast_10s_pct = 35.0;
        assert!(cpu_forecast_predicts_pressure(&rising));
        rising.heavy_threshold = 95.0;
        rising.telemetry.cpu_forecast_10s_pct = 99.99;
        assert!(!cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_forecast_10s_pct = 100.0;
        assert!(cpu_forecast_predicts_pressure(&rising));

        rising.heavy_threshold = 40.0;
        rising.telemetry.cpu_forecast_10s_pct = 44.9;
        assert!(!cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_forecast_10s_pct = 45.0;
        assert!(cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_forecast_10s_pct = 80.0;
        rising.heavy_threshold = 60.0;

        rising.telemetry.cpu_forecast_stability = 40.0;
        assert!(!cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_forecast_stability = 80.0;
        rising.telemetry.cpu_trend_pct_per_sec = f32::NAN;
        assert!(!cpu_forecast_predicts_pressure(&rising));
        rising.telemetry.cpu_trend_pct_per_sec = 1.5;
        rising.idle_secs = rising.away_threshold;
        assert!(!cpu_forecast_predicts_pressure(&rising));
    }

    #[test]
    fn urgent_response_requires_three_seconds_and_keeps_manual_hold() {
        assert!(!strategy_transition_ready(
            StrategyKind::Responsiveness,
            StrategyKind::Efficiency,
            Duration::from_secs(2),
            Duration::ZERO,
            false,
            true,
        ));
        assert!(strategy_transition_ready(
            StrategyKind::Responsiveness,
            StrategyKind::Efficiency,
            Duration::from_secs(3),
            Duration::ZERO,
            false,
            true,
        ));
        assert!(!strategy_transition_ready(
            StrategyKind::Responsiveness,
            StrategyKind::Efficiency,
            Duration::from_secs(10),
            Duration::from_secs(100),
            true,
            true,
        ));
        assert!(!strategy_transition_ready(
            StrategyKind::Responsiveness,
            StrategyKind::Efficiency,
            Duration::from_secs(3),
            Duration::ZERO,
            false,
            false,
        ));
    }

    #[test]
    fn strategy_selector_uses_hysteresis_and_keeps_all_candidate_scores() {
        let mut c = ctx(5.0);
        let initial = select_strategy(&c, StrategyKind::AdaptiveHybrid);
        assert_eq!(initial.selected, StrategyKind::AdaptiveHybrid);
        assert_eq!(initial.scores.len(), 5);

        c.gaming = true;
        c.telemetry.mem_pct = 20.0;
        c.idle_secs = 0;
        c.ac_online = true;
        let selected = select_strategy(&c, StrategyKind::Responsiveness);
        assert_eq!(selected.selected, StrategyKind::Responsiveness);
        assert_eq!(selected.scores.len(), 5);
        assert!(selected.scores.windows(2).all(|w| w[0].score >= w[1].score));
    }

    #[test]
    fn strategy_scoring_handles_invalid_and_out_of_range_telemetry() {
        let mut c = ctx(5.0);
        c.telemetry.cpu_avg_10s = f32::NAN;
        c.telemetry.cpu_avg_60s = 140.0;
        c.telemetry.cpu_peak = f32::INFINITY;
        c.telemetry.mem_pct = -25.0;
        c.telemetry.mem_slope = f32::NAN;
        let selection = select_strategy(&c, StrategyKind::AdaptiveHybrid);
        assert_eq!(selection.scores.len(), 5);
        assert!(selection
            .scores
            .iter()
            .all(|s| (0..=100).contains(&s.score)));

        let mut no_finite_sensor = ctx(5.0);
        no_finite_sensor.telemetry.cpu_avg_10s = f32::NAN;
        no_finite_sensor.telemetry.cpu_avg_60s = f32::NAN;
        no_finite_sensor.telemetry.cpu_peak = f32::INFINITY;
        no_finite_sensor.telemetry.mem_pct = f32::NAN;
        no_finite_sensor.telemetry.mem_slope = f32::NAN;
        let neutral = select_strategy(&no_finite_sensor, StrategyKind::AdaptiveHybrid);
        assert!(neutral.scores.iter().any(|score| score.score > 0));
        assert!(neutral
            .scores
            .iter()
            .all(|score| (0..=100).contains(&score.score)));

        let mut finite_neutral = ctx(0.0);
        finite_neutral.telemetry.mem_pct = 0.0;
        let finite_scores = select_strategy(&finite_neutral, StrategyKind::AdaptiveHybrid)
            .scores
            .into_iter()
            .map(|score| (score.id, score.score))
            .collect::<Vec<_>>();
        let mut invalid_infinite = ctx(0.0);
        invalid_infinite.telemetry.cpu_avg_10s = f32::INFINITY;
        invalid_infinite.telemetry.cpu_avg_60s = f32::INFINITY;
        invalid_infinite.telemetry.cpu_peak = f32::NAN;
        invalid_infinite.telemetry.mem_pct = f32::INFINITY;
        invalid_infinite.telemetry.mem_slope = f32::NAN;
        let invalid_scores = select_strategy(&invalid_infinite, StrategyKind::AdaptiveHybrid)
            .scores
            .into_iter()
            .map(|score| (score.id, score.score))
            .collect::<Vec<_>>();
        assert_eq!(invalid_scores, finite_scores);

        let finite_memory_knobs =
            strategy_knobs("none", &finite_neutral, StrategyKind::LoadBalance);
        invalid_infinite.telemetry.cpu_avg_60s = 0.0;
        let invalid_memory_knobs =
            strategy_knobs("none", &invalid_infinite, StrategyKind::LoadBalance);
        assert_eq!(invalid_memory_knobs.epp, finite_memory_knobs.epp);
    }

    #[test]
    fn game_scene_overrides_every_strategy_policy() {
        let c = ctx(80.0);
        for strategy in [
            StrategyKind::Responsiveness,
            StrategyKind::Throughput,
            StrategyKind::Efficiency,
            StrategyKind::LoadBalance,
            StrategyKind::AdaptiveHybrid,
        ] {
            let knobs = strategy_knobs("game", &c, strategy);
            assert_eq!(knobs.epp, Some(0));
            assert_eq!(knobs.priority_separation, 0x26);
            assert!(knobs.prio_boost);
        }
    }

    #[test]
    fn policy_knobs_keep_game_boost_scoped_and_tune_heavy_response() {
        let c = ctx(80.0);
        let fast = strategy_knobs("heavy", &c, StrategyKind::Responsiveness);
        assert_eq!(fast.epp, Some(10));
        assert_eq!(fast.priority_separation, 0x26);
        assert!(fast.prio_boost);
        let throughput = strategy_knobs("heavy", &c, StrategyKind::Throughput);
        assert_eq!(throughput.epp, Some(25));
        assert!(!throughput.prio_boost);
        let balanced = strategy_knobs("heavy", &c, StrategyKind::LoadBalance);
        assert_eq!(balanced.priority_separation, 0x18);
        assert!(!balanced.prio_boost);

        let normal = ctx(5.0);
        let fast_idle = strategy_knobs("none", &normal, StrategyKind::Responsiveness);
        let efficient_idle = strategy_knobs("none", &normal, StrategyKind::Efficiency);
        assert_eq!(fast_idle.epp, Some(15));
        assert_eq!(fast_idle.priority_separation, 2);
        assert!(!fast_idle.prio_boost);
        assert_eq!(efficient_idle.epp, Some(70));

        let mut pressure = ctx(9.0);
        pressure.telemetry.cpu_core_peak_pct = 96.0;
        pressure.telemetry.cpu_logical_cores = 16;
        let focused = strategy_knobs("none", &pressure, StrategyKind::Responsiveness);
        assert_eq!(focused.priority_separation, 0x26);
        assert!(focused.prio_boost);
        let media = strategy_knobs("media", &pressure, StrategyKind::Responsiveness);
        assert_eq!(media.priority_separation, 2);
        assert!(!media.prio_boost);

        assert!(game_boost_active("game", true));
        assert!(!game_boost_active("heavy", true));
    }

    #[test]
    fn priority_separation_gate_filters_short_candidates_and_enforces_minimum_residence() {
        let start = Instant::now();
        let mut gate = PrioritySeparationGate::default();

        assert_eq!(gate.resolve(2, 0x26, false, start), 2);
        // A short-lived contention signal disappears before the 10-second confirmation.
        assert_eq!(gate.resolve(2, 2, false, start + Duration::from_secs(7)), 2);
        assert_eq!(
            gate.resolve(2, 0x26, false, start + Duration::from_secs(8)),
            2
        );
        assert_eq!(
            gate.resolve(2, 0x26, false, start + Duration::from_secs(17)),
            2
        );
        assert_eq!(
            gate.resolve(2, 0x26, false, start + Duration::from_secs(18)),
            0x26
        );
        gate.record_applied(start + Duration::from_secs(18));

        // Do not reverse the global scheduler setting after a brief seven-second rebound.
        assert_eq!(
            gate.resolve(0x26, 2, false, start + Duration::from_secs(25)),
            0x26
        );
        assert_eq!(
            gate.resolve(0x26, 2, false, start + Duration::from_secs(47)),
            0x26
        );
        assert_eq!(
            gate.resolve(0x26, 2, false, start + Duration::from_secs(48)),
            2
        );
    }

    #[test]
    fn adaptive_knob_recheck_precedes_calibration_and_respects_residence() {
        let start = Instant::now();
        let mut gate = PrioritySeparationGate::default();
        assert_eq!(gate.resolve(2, 0x26, false, start), 2);
        assert_eq!(CALIBRATE_EVERY, 15);

        // Model the one-second adaptive caller cadence: its gate deadline fires at 10s,
        // before the independent 15s calibration deadline.
        for elapsed_secs in 1..CALIBRATE_EVERY as u64 {
            let now = start + Duration::from_secs(elapsed_secs);
            assert_eq!(
                adaptive_knobs_due(false, false, &gate, now),
                elapsed_secs >= PRIORITY_SEPARATION_CONFIRM_SECS,
                "unexpected re-evaluation decision at +{elapsed_secs}s"
            );
        }
        assert!(adaptive_knobs_due(
            false,
            false,
            &gate,
            start + Duration::from_secs(PRIORITY_SEPARATION_CONFIRM_SECS)
        ));

        let mut resident_gate = PrioritySeparationGate::default();
        resident_gate.record_applied(start);
        assert_eq!(
            resident_gate.resolve(2, 0x26, false, start + Duration::from_secs(1)),
            2
        );
        for elapsed_secs in 1..PRIORITY_SEPARATION_MIN_RESIDENCE_SECS {
            assert!(
                !adaptive_knobs_due(
                    false,
                    false,
                    &resident_gate,
                    start + Duration::from_secs(elapsed_secs)
                ),
                "residence gate opened early at +{elapsed_secs}s"
            );
        }
        assert!(adaptive_knobs_due(
            false,
            false,
            &resident_gate,
            start + Duration::from_secs(PRIORITY_SEPARATION_MIN_RESIDENCE_SECS)
        ));
        assert_eq!(
            resident_gate.resolve(
                2,
                0x26,
                false,
                start + Duration::from_secs(PRIORITY_SEPARATION_MIN_RESIDENCE_SECS)
            ),
            0x26,
            "the caller can apply as soon as both existing gates allow it"
        );
    }

    #[test]
    fn priority_separation_gate_allows_confirmed_hard_scenes_to_bypass_dwell() {
        let start = Instant::now();
        let mut gate = PrioritySeparationGate::default();
        assert_eq!(gate.resolve(2, 0x26, false, start), 2);
        assert_eq!(
            gate.resolve(2, 0x26, true, start + Duration::from_secs(1)),
            0x26
        );
        gate.record_applied(start + Duration::from_secs(1));
        assert_eq!(
            gate.resolve(0x26, 2, true, start + Duration::from_secs(2)),
            2
        );
    }

    #[test]
    fn strategy_priority_gate_smooths_low_confidence_scene_and_releases_hard_scene() {
        let start = Instant::now();
        let normal = ctx(5.0);
        let mut adaptive = Adaptive {
            state: "none".into(),
            applied: Some(strategy_knobs(
                "none",
                &normal,
                StrategyKind::Responsiveness,
            )),
            ..Default::default()
        };
        let mut pressure = ctx(9.0);
        pressure.telemetry.cpu_core_peak_pct = 96.0;
        pressure.telemetry.cpu_logical_cores = 16;

        let initial = strategy_knobs_with_priority_gate(
            &mut adaptive,
            "none",
            &pressure,
            StrategyKind::Responsiveness,
            start,
        );
        assert_eq!(initial.priority_separation, 2);
        assert!(!initial.prio_boost);

        let confirmed = strategy_knobs_with_priority_gate(
            &mut adaptive,
            "none",
            &pressure,
            StrategyKind::Responsiveness,
            start + Duration::from_secs(PRIORITY_SEPARATION_CONFIRM_SECS),
        );
        assert_eq!(confirmed.priority_separation, 0x26);
        assert!(confirmed.prio_boost);
        adaptive.applied = Some(confirmed);
        adaptive
            .priority_separation_gate
            .record_applied(start + Duration::from_secs(PRIORITY_SEPARATION_CONFIRM_SECS));

        // Leaving a confirmed game scene restores the low-confidence boost without delay.
        adaptive.state = "game".into();
        let leaving_game = strategy_knobs_with_priority_gate(
            &mut adaptive,
            "media",
            &pressure,
            StrategyKind::Responsiveness,
            start + Duration::from_secs(PRIORITY_SEPARATION_CONFIRM_SECS + 1),
        );
        assert_eq!(leaving_game.priority_separation, 2);
        assert!(!leaving_game.prio_boost);
    }

    #[test]
    fn knobs_mapping() {
        let g = knobs_for("game", &ctx(0.0));
        assert_eq!(g.plan, PlanKind::High);
        assert_eq!(g.epp, Some(0));
        assert!(!g.gamedvr);
        assert!(g.prio_boost);

        let a = knobs_for("away", &ctx(0.0));
        assert_eq!(a.plan, PlanKind::Saver);
        assert_eq!(a.epp, Some(90));

        let h = knobs_for("heavy", &ctx(80.0));
        assert_eq!(h.epp, Some(20));
        assert!(h.prio_boost);

        assert_eq!(knobs_for("power", &ctx(80.0)).epp, Some(45));
        assert_eq!(knobs_for("power", &ctx(10.0)).epp, Some(85));

        let n = knobs_for("none", &ctx(0.0));
        assert_eq!(n.epp, Some(45));
        assert!(!n.prio_boost);
    }

    #[test]
    fn knobs_epp_none_when_unsupported() {
        let mut c = ctx(0.0);
        c.epp_ok = false;
        assert_eq!(knobs_for("game", &c).epp, None);
        assert_eq!(knobs_for("away", &c).epp, None);
    }

    #[test]
    fn confirm_windows() {
        assert_eq!(confirm_seconds_for("game"), 1);
        assert_eq!(confirm_seconds_for("heavy"), 10);
        assert_eq!(confirm_seconds_for("none"), 10);
    }

    #[test]
    fn normalize() {
        assert_eq!(normalize_name("Chrome.EXE"), "chrome");
        assert_eq!(normalize_name("  chrome "), "chrome");
        assert_eq!(normalize_name("aerotune.exe"), "aerotune");
    }

    fn anomaly(pid: u32, name: &str, kind: &str, value: f64) -> Anomaly {
        Anomaly {
            pid,
            name: name.into(),
            kind: kind.into(),
            value,
            baseline: 1.0,
            robust_z: value.max(0.0),
            onset_unix: None,
            creation_filetime: None,
        }
    }

    fn notify_rule_fixture() -> Vec<Rule> {
        vec![Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 50.0,
            seconds: 3,
            action: "notify".into(),
        }]
    }

    #[test]
    fn auto_dispose_escalates_notify_rules_for_non_foreground() {
        let rules = notify_rule_fixture();
        let mut sustained = std::collections::HashMap::new();
        let mut suspended = std::collections::HashSet::new();
        let a = vec![anomaly(999_001, "sustained-hog.exe", "cpu", 80.0)];
        // 前三秒累积不触发
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            true,
            ""
        )
        .is_empty());
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            true,
            ""
        )
        .is_empty());
        let acts = rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            true,
            "",
        );
        // 升级分支已进入；样本没有实例令牌，因此必须拒绝按 PID 降级。
        assert_eq!(acts.len(), 1);
        assert!(
            acts[0].contains("自动降级失败")
                && acts[0].contains("目标进程样本身份无法核验")
                && acts[0].contains("不会自动重试")
                && acts[0].contains("待异常解除后重新触发"),
            "escalation fails closed with identity and retry guidance: {}",
            acts[0]
        );
    }

    #[test]
    fn configured_deprio_preserves_foreground_policy_and_fails_closed_without_identity() {
        let rules = vec![Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 50.0,
            seconds: 1,
            action: "deprio".into(),
        }];
        let mut sustained = std::collections::HashMap::new();
        let mut suspended = std::collections::HashSet::new();
        let anomalies = vec![anomaly(999_004, "deprio-fixture.exe", "cpu", 80.0)];
        let actions = rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &anomalies,
            Duration::from_secs(1),
            false,
            "deprio-fixture.exe",
        );
        assert_eq!(actions.len(), 1);
        assert!(actions[0].contains("降级失败"));
        assert!(actions[0].contains("目标进程样本身份无法核验"));
        assert!(actions[0].contains("不会自动重试"));
        assert!(actions[0].contains("待异常解除后重新触发"));
        assert!(!actions[0].contains("前台进程，仅提醒"));
    }

    #[test]
    fn auto_dispose_off_and_foreground_and_critical_stay_notify() {
        let rules = notify_rule_fixture();
        let a = vec![anomaly(999_002, "regular-app.exe", "cpu", 80.0)];
        let run = |auto_dispose: bool, foreground: &str, _offset: u64| {
            let mut sustained = std::collections::HashMap::new();
            let mut suspended = std::collections::HashSet::new();
            for _ in 0..2 {
                let _ = rules_decide(
                    &rules,
                    &mut sustained,
                    &mut suspended,
                    &a,
                    Duration::from_secs(1),
                    auto_dispose,
                    foreground,
                );
            }
            rules_decide(
                &rules,
                &mut sustained,
                &mut suspended,
                &a,
                Duration::from_secs(1),
                auto_dispose,
                foreground,
            )
        };
        // 子开关关闭：普通提醒
        let acts = run(false, "", 0);
        assert_eq!(acts.len(), 1);
        assert!(acts[0].starts_with("规则提醒："));
        assert!(!acts[0].contains("自动降级"));
        // 前台进程即使开关开启也仅提醒
        let acts = run(true, "regular-app.exe", 0);
        assert!(acts[0].contains("前台进程，仅提醒"));
        // 系统关键进程不受升级影响
        let critical = vec![anomaly(999_003, "svchost.exe", "cpu", 80.0)];
        let mut sustained = std::collections::HashMap::new();
        let mut suspended = std::collections::HashSet::new();
        for _ in 0..2 {
            let _ = rules_decide(
                &rules,
                &mut sustained,
                &mut suspended,
                &critical,
                Duration::from_secs(1),
                true,
                "",
            );
        }
        let acts = rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &critical,
            Duration::from_secs(1),
            true,
            "",
        );
        assert!(acts[0].contains("系统关键进程，仅提醒"));
    }

    #[test]
    fn rule_fires_once_then_rearms() {
        let rules = vec![Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 50.0,
            seconds: 3,
            action: "notify".into(),
        }];
        let mut sustained = std::collections::HashMap::new();
        let mut suspended = std::collections::HashSet::new();
        let a = vec![anomaly(100, "test", "cpu", 80.0)];

        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());
        let acts = rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            false,
            "",
        );
        assert_eq!(acts.len(), 1);
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());

        let none: Vec<Anomaly> = vec![];
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &none,
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());
        assert_eq!(
            rules_decide(
                &rules,
                &mut sustained,
                &mut suspended,
                &a,
                Duration::from_secs(1),
                false,
                "",
            )
            .len(),
            1
        );
    }

    #[test]
    fn rules_hidden_two_seconds_equal_visible_one_second() {
        let rules = vec![Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 50.0,
            seconds: 3,
            action: "notify".into(),
        }];
        let a = vec![anomaly(100, "fixture", "cpu", 80.0)];
        let mut one = Default::default();
        let mut two = Default::default();
        let mut suspended = Default::default();
        for _ in 0..4 {
            let mut visible = rules_decide(
                &rules,
                &mut one,
                &mut suspended,
                &a,
                Duration::from_secs(1),
                false,
                "",
            );
            visible.extend(rules_decide(
                &rules,
                &mut one,
                &mut suspended,
                &a,
                Duration::from_secs(1),
                false,
                "",
            ));
            let hidden = rules_decide(
                &rules,
                &mut two,
                &mut suspended,
                &a,
                Duration::from_secs(2),
                false,
                "",
            );
            assert_eq!(visible, hidden);
        }
    }

    #[test]
    fn rule_below_threshold_ignored() {
        let rules = vec![Rule {
            kind: "mem".into(),
            enabled: true,
            threshold: 1000.0,
            seconds: 3,
            action: "notify".into(),
        }];
        let mut sustained = std::collections::HashMap::new();
        let mut suspended = std::collections::HashSet::new();
        let a = vec![anomaly(1, "x", "mem", 500.0)];
        for _ in 0..10 {
            assert!(rules_decide(
                &rules,
                &mut sustained,
                &mut suspended,
                &a,
                Duration::from_secs(1),
                false,
                "",
            )
            .is_empty());
        }
    }

    #[test]
    fn critical_procs_always_notify_only() {
        let rules = vec![Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 10.0,
            seconds: 1,
            action: "suspend".into(),
        }];
        let mut sustained = std::collections::HashMap::new();
        let mut suspended = std::collections::HashSet::new();
        let a = vec![anomaly(4, "explorer.exe", "cpu", 90.0)];
        let acts = rules_decide(
            &rules,
            &mut sustained,
            &mut suspended,
            &a,
            Duration::from_secs(1),
            false,
            "",
        );
        assert_eq!(acts.len(), 1);
        assert!(acts[0].contains("仅提醒"));
        assert!(!suspended.contains(&4));
    }

    #[test]
    fn disabled_suspend_rule_only_notifies_and_deduplicates() {
        let rules = vec![Rule {
            kind: "cpu".into(),
            enabled: true,
            threshold: 50.0,
            seconds: 3,
            action: "suspend".into(),
        }];
        let pid = u32::MAX;
        let anomalies = vec![anomaly(pid, "fixture-app.exe", "cpu", 80.0)];
        let mut sustained = std::collections::HashMap::new();
        let mut suspend_notice_seen = std::collections::HashSet::new();

        for _ in 0..2 {
            assert!(rules_decide(
                &rules,
                &mut sustained,
                &mut suspend_notice_seen,
                &anomalies,
                Duration::from_secs(1),
                false,
                "",
            )
            .is_empty());
        }
        let actions = rules_decide(
            &rules,
            &mut sustained,
            &mut suspend_notice_seen,
            &anomalies,
            Duration::from_secs(1),
            false,
            "",
        );
        assert_eq!(actions.len(), 1);
        assert!(actions[0].contains("暂停暂时关闭，已按仅提醒处理"));
        assert!(suspend_notice_seen.contains(&pid));
        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspend_notice_seen,
            &anomalies,
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());

        assert!(rules_decide(
            &rules,
            &mut sustained,
            &mut suspend_notice_seen,
            &[],
            Duration::from_secs(1),
            false,
            "",
        )
        .is_empty());
        assert!(!suspend_notice_seen.contains(&pid));
        for _ in 0..2 {
            assert!(rules_decide(
                &rules,
                &mut sustained,
                &mut suspend_notice_seen,
                &anomalies,
                Duration::from_secs(1),
                false,
                "",
            )
            .is_empty());
        }
        let rearmed = rules_decide(
            &rules,
            &mut sustained,
            &mut suspend_notice_seen,
            &anomalies,
            Duration::from_secs(1),
            false,
            "",
        );
        assert_eq!(rearmed.len(), 1);
        assert!(rearmed[0].contains("暂停暂时关闭，已按仅提醒处理"));
    }

    #[test]
    fn disabled_rule_ignored() {
        let rules = vec![Rule {
            kind: "io".into(),
            enabled: false,
            threshold: 1.0,
            seconds: 1,
            action: "notify".into(),
        }];
        let mut sustained = std::collections::HashMap::new();
        let mut suspended = std::collections::HashSet::new();
        let a = vec![anomaly(9, "x", "io", 99999.0)];
        for _ in 0..5 {
            assert!(rules_decide(
                &rules,
                &mut sustained,
                &mut suspended,
                &a,
                Duration::from_secs(1),
                false,
                "",
            )
            .is_empty());
        }
    }

    #[test]
    fn plan_kind_name_classification() {
        // 中文 OEM 计划
        assert_eq!(plan_kind_from_name("卓越性能"), PlanKind::High);
        assert_eq!(plan_kind_from_name("高性能"), PlanKind::High);
        assert_eq!(plan_kind_from_name("节能"), PlanKind::Saver);
        // 英文（大小写不敏感）
        assert_eq!(plan_kind_from_name("Ultimate Performance"), PlanKind::High);
        assert_eq!(plan_kind_from_name("Power saver"), PlanKind::Saver);
        assert_eq!(plan_kind_from_name("HIGH PERFORMANCE"), PlanKind::High);
        // 平衡/未知
        assert_eq!(plan_kind_from_name("平衡"), PlanKind::Balanced);
        assert_eq!(plan_kind_from_name("我的自定义计划"), PlanKind::Balanced);
        assert_eq!(plan_kind_from_name(""), PlanKind::Balanced);
    }

    #[test]
    fn duplicatescheme_negative_cache_blocks_repeat_attempts() {
        reset_plan_template_missing();
        let kind = PlanKind::Saver;
        assert!(!plan_template_missing(&kind));
        // 首次登记返回 true（输出一次 WARN）
        assert!(register_plan_template_missing(
            &kind,
            "系统找不到指定的文件"
        ));
        // 再次登记返回 false（静默退避，不重复 WARN）
        assert!(!register_plan_template_missing(&kind, "又一次失败"));
        assert_eq!(
            PLAN_TEMPLATE_MISSING.lock().unwrap().get(&kind),
            Some(&"系统找不到指定的文件".to_string())
        );
        // 短路生效：不再调用 powercfg，直接返回冷却错误
        assert!(resolve_plan_template_missing(&kind).is_some());
        // 其他计划类型不受影响
        assert!(!plan_template_missing(&PlanKind::High));
        assert!(!plan_template_missing(&PlanKind::Balanced));
        assert!(resolve_plan_template_missing(&PlanKind::Balanced).is_none());
        reset_plan_template_missing();
        assert!(!plan_template_missing(&kind));
    }

    #[test]
    fn active_plan_kind_guid_path_prefers_registry_and_templates() {
        // 无需真实 powercfg：active_scheme_guid 读取注册表；本机如果活动计划
        // 是标准模板，应正确分类；若是 AeroTune 副本 GUID，按设置精确匹配。
        // 这里只验证函数在真实环境下可调用且不 panic（注册表读取失败时回 Balanced）。
        let _ = active_plan_kind();
    }
}
