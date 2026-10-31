use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use sysinfo::{
    DiskRefreshKind, Disks, MemoryRefreshKind, Networks, ProcessRefreshKind, ProcessesToUpdate,
    System, UpdateKind,
};

use crate::auto::AutoState;
use crate::diag::Engine;

#[derive(Serialize, Clone, Debug, Default)]
pub struct GpuInfo {
    pub name: String,
    pub utilization: Option<f32>,
    pub mem_used_mb: Option<u64>,
    pub mem_total_mb: Option<u64>,
    pub temperature: Option<f32>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct BatteryInfo {
    /// Compatibility field: unknown AC state is conservatively treated as online.
    pub ac_online: bool,
    /// True = AC, false = battery, None = Windows could not determine the state.
    pub ac_connected: Option<bool>,
    pub percent: Option<u8>,
    pub secs_left: Option<u64>,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EffectivePowerMode {
    BatterySaver,
    BetterBattery,
    Balanced,
    HighPerformance,
    MaxPerformance,
    GameMode,
    MixedReality,
}

fn effective_power_mode_from_raw(mode: i32) -> Option<EffectivePowerMode> {
    match mode {
        0 => Some(EffectivePowerMode::BatterySaver),
        1 => Some(EffectivePowerMode::BetterBattery),
        2 => Some(EffectivePowerMode::Balanced),
        3 => Some(EffectivePowerMode::HighPerformance),
        4 => Some(EffectivePowerMode::MaxPerformance),
        5 => Some(EffectivePowerMode::GameMode),
        6 => Some(EffectivePowerMode::MixedReality),
        _ => None,
    }
}

const EFFECTIVE_POWER_MODE_UNSET: i32 = i32::MIN;
static EFFECTIVE_POWER_MODE_RAW: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(EFFECTIVE_POWER_MODE_UNSET);

#[cfg(windows)]
fn current_effective_power_mode() -> Option<EffectivePowerMode> {
    use std::sync::atomic::Ordering;

    let raw = EFFECTIVE_POWER_MODE_RAW.load(Ordering::Acquire);
    if raw == EFFECTIVE_POWER_MODE_UNSET {
        None
    } else {
        effective_power_mode_from_raw(raw)
    }
}

#[cfg(not(windows))]
fn current_effective_power_mode() -> Option<EffectivePowerMode> {
    None
}

#[cfg(windows)]
type RegisterEffectivePowerModeFn = unsafe extern "system" fn(
    u32,
    windows_sys::Win32::System::Power::EFFECTIVE_POWER_MODE_CALLBACK,
    *const std::ffi::c_void,
    *mut *mut std::ffi::c_void,
) -> windows_sys::core::HRESULT;

#[cfg(windows)]
type UnregisterEffectivePowerModeFn =
    unsafe extern "system" fn(*const std::ffi::c_void) -> windows_sys::core::HRESULT;

#[cfg(windows)]
struct EffectivePowerModeRegistration {
    module: windows_sys::Win32::Foundation::HMODULE,
    handle: *mut std::ffi::c_void,
    unregister: UnregisterEffectivePowerModeFn,
}

#[cfg(windows)]
impl Drop for EffectivePowerModeRegistration {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::FreeLibrary;

        let result = unsafe { (self.unregister)(self.handle.cast_const()) };
        if result == 0 {
            unsafe { FreeLibrary(self.module) };
        } else {
            // Keep PowrProf loaded if Windows could not quiesce the callback.
            tracing::warn!(
                result,
                "Windows effective power mode callback remains registered"
            );
        }
    }
}

#[cfg(windows)]
unsafe extern "system" fn effective_power_mode_changed(
    mode: windows_sys::Win32::System::Power::EFFECTIVE_POWER_MODE,
    _context: *const std::ffi::c_void,
) {
    use std::sync::atomic::Ordering;

    EFFECTIVE_POWER_MODE_RAW.store(mode, Ordering::Release);
}

#[cfg(windows)]
fn register_effective_power_mode_notifications() -> Option<EffectivePowerModeRegistration> {
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32,
    };
    use windows_sys::Win32::System::Power::{EFFECTIVE_POWER_MODE_V1, EFFECTIVE_POWER_MODE_V2};

    let dll_name = "PowrProf.dll\0".encode_utf16().collect::<Vec<_>>();
    // SAFETY: the DLL is resolved from System32 and the UTF-16 name is NUL-terminated.
    let module = unsafe {
        LoadLibraryExW(
            dll_name.as_ptr(),
            std::ptr::null_mut(),
            LOAD_LIBRARY_SEARCH_SYSTEM32,
        )
    };
    if module.is_null() {
        tracing::debug!("PowrProf.dll is unavailable; effective power mode will remain unknown");
        return None;
    }

    // SAFETY: both names are NUL-terminated and the loaded module is kept alive below.
    let register_proc = unsafe {
        GetProcAddress(
            module,
            c"PowerRegisterForEffectivePowerModeNotifications"
                .as_ptr()
                .cast(),
        )
    };
    let unregister_proc = unsafe {
        GetProcAddress(
            module,
            c"PowerUnregisterFromEffectivePowerModeNotifications"
                .as_ptr()
                .cast(),
        )
    };
    let (Some(register_proc), Some(unregister_proc)) = (register_proc, unregister_proc) else {
        unsafe { FreeLibrary(module) };
        tracing::debug!("Windows effective power mode API is unavailable");
        return None;
    };
    // SAFETY: GetProcAddress resolved these exact Win32 exports; aliases preserve the documented system ABI.
    let register: RegisterEffectivePowerModeFn = unsafe { std::mem::transmute(register_proc) };
    let unregister: UnregisterEffectivePowerModeFn =
        unsafe { std::mem::transmute(unregister_proc) };

    let callback: windows_sys::Win32::System::Power::EFFECTIVE_POWER_MODE_CALLBACK = Some(
        effective_power_mode_changed as unsafe extern "system" fn(i32, *const std::ffi::c_void),
    );
    for version in [EFFECTIVE_POWER_MODE_V2, EFFECTIVE_POWER_MODE_V1] {
        let mut handle = std::ptr::null_mut();
        // SAFETY: callback only performs an atomic store; this holder keeps the DLL loaded until unregister.
        let result = unsafe { register(version, callback, std::ptr::null(), &mut handle) };
        if result == 0 && !handle.is_null() {
            tracing::debug!(
                version,
                "Registered Windows effective power mode notifications"
            );
            return Some(EffectivePowerModeRegistration {
                module,
                handle,
                unregister,
            });
        }
        if !handle.is_null() {
            // A non-null handle may still own a callback even on an unexpected failure result.
            tracing::warn!(
                version,
                result,
                "Power-mode registration returned a handle with failure status"
            );
            return Some(EffectivePowerModeRegistration {
                module,
                handle,
                unregister,
            });
        }
        tracing::debug!(
            version,
            result,
            "Windows effective power mode registration attempt failed"
        );
    }

    unsafe { FreeLibrary(module) };
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcLineState {
    Online,
    Offline,
    Unknown,
}

fn battery_info_from_status(
    ac_line: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    battery_life_seconds: u32,
) -> Option<BatteryInfo> {
    if battery_flag & 128 != 0 {
        return None;
    }
    let ac_line = match ac_line {
        0 => AcLineState::Offline,
        1 => AcLineState::Online,
        _ => AcLineState::Unknown,
    };
    Some(BatteryInfo {
        // Keep the legacy field conservative; nullable state distinguishes AC/battery/unknown.
        ac_online: ac_line != AcLineState::Offline,
        ac_connected: match ac_line {
            AcLineState::Online => Some(true),
            AcLineState::Offline => Some(false),
            AcLineState::Unknown => None,
        },
        percent: battery_percent(battery_life_percent),
        secs_left: (battery_life_seconds != u32::MAX).then_some(battery_life_seconds as u64),
    })
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DiskInfo {
    pub mount: String,
    pub total_gb: f64,
    pub used_gb: f64,
    pub fs: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Stats {
    /// Monotonically increasing sample identifier; zero denotes uninitialized cache.
    pub sample_id: u64,
    pub cpu: f32,
    pub per_core: Vec<f32>,
    pub cpu_freq_mhz: u64,
    pub mem_used_gb: f64,
    pub mem_total_gb: f64,
    pub swap_used_gb: f64,
    pub swap_total_gb: f64,
    /// Windows system volume root used to align disk diagnostics and UI evidence.
    pub system_drive: String,
    pub disks: Vec<DiskInfo>,
    pub net_rx_kbps: f64,
    pub net_tx_kbps: f64,
    /// AeroTune 观测并跨进程累计的本地自然日接收流量（GB）。
    pub net_today_rx_gb: f64,
    /// AeroTune 观测并跨进程累计的本地自然日发送流量（GB）。
    pub net_today_tx_gb: f64,
    pub gpu: Option<GpuInfo>,
    pub battery: Option<BatteryInfo>,
    /// False means battery status sampling failed; true with battery=None means no battery found.
    pub battery_status_ok: bool,
    /// Windows-reported effective power policy; None means unavailable or unrecognized.
    pub effective_power_mode: Option<EffectivePowerMode>,
    pub uptime_secs: u64,
    pub process_count: usize,
}

fn memory_usage_pct(used_gb: f64, total_gb: f64) -> Option<f32> {
    if !used_gb.is_finite()
        || !total_gb.is_finite()
        || used_gb < 0.0
        || total_gb <= 0.0
        || used_gb > total_gb
    {
        return None;
    }
    let percent = (used_gb / total_gb * 100.0) as f32;
    percent.is_finite().then_some(percent)
}

static LATEST_SAMPLE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn latest_sample_id() -> u64 {
    LATEST_SAMPLE_ID.load(std::sync::atomic::Ordering::Acquire)
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq, Eq)]
struct NetworkDay {
    local_date: String,
    time_zone_id: Option<String>,
    rx_bytes: u64,
    tx_bytes: u64,
}

fn select_network_day(
    today: &str,
    time_zone_id: Option<&str>,
    saved: Option<NetworkDay>,
) -> NetworkDay {
    match (saved, time_zone_id) {
        (Some(day), Some(time_zone_id))
            if day.local_date == today && day.time_zone_id.as_deref() == Some(time_zone_id) =>
        {
            day
        }
        (_, time_zone_id) => NetworkDay {
            local_date: today.to_string(),
            time_zone_id: time_zone_id.map(str::to_string),
            rx_bytes: 0,
            tx_bytes: 0,
        },
    }
}

fn timezone_id_from_utf16(buffer: &[u16]) -> Option<String> {
    let end = buffer.iter().position(|unit| *unit == 0)?;
    let name = String::from_utf16(&buffer[..end]).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_ascii_lowercase())
}

fn timezone_id_from_api_result(status: u32, invalid_status: u32, buffer: &[u16]) -> Option<String> {
    (status != invalid_status)
        .then(|| timezone_id_from_utf16(buffer))
        .flatten()
}

const TIMEZONE_IDENTITY_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

fn timezone_identity_refresh_due(
    last_checked: std::time::Instant,
    now: std::time::Instant,
) -> bool {
    now.saturating_duration_since(last_checked) >= TIMEZONE_IDENTITY_REFRESH_INTERVAL
}

#[cfg(windows)]
fn local_timezone_identity() -> Option<String> {
    use windows_sys::Win32::System::Time::{
        GetDynamicTimeZoneInformation, DYNAMIC_TIME_ZONE_INFORMATION, TIME_ZONE_ID_INVALID,
    };

    let mut information: DYNAMIC_TIME_ZONE_INFORMATION = unsafe { std::mem::zeroed() };
    let status = unsafe { GetDynamicTimeZoneInformation(&mut information) };
    let id =
        timezone_id_from_api_result(status, TIME_ZONE_ID_INVALID, &information.TimeZoneKeyName)?;
    Some(format!(
        "windows-time-zone:{id}:dynamic-dst-disabled:{}",
        information.DynamicDaylightTimeDisabled != 0
    ))
}

#[cfg(not(windows))]
fn local_timezone_identity() -> Option<String> {
    None
}

fn load_network_day() -> NetworkDay {
    let today = chrono::Local::now().date_naive().to_string();
    let time_zone_id = local_timezone_identity();
    let saved = std::fs::read(crate::util::app_dir().join("network-day.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<NetworkDay>(&bytes).ok());
    select_network_day(&today, time_zone_id.as_deref(), saved)
}

fn persist_network_day(day: &NetworkDay) -> Result<(), String> {
    if day.time_zone_id.is_none() {
        return Ok(());
    }
    let bytes = serde_json::to_vec(day).map_err(|error| error.to_string())?;
    crate::util::atomic_write(&crate::util::app_dir().join("network-day.json"), &bytes)
        .map_err(|error| error.to_string())
}

pub struct Cache {
    pub stats: RwLock<Arc<Stats>>,
    tick: Mutex<u64>,
    /// AeroTune 观测到的本地自然日流量；缺少 Windows 时区标识时只在当前会话累计。
    net_day: Mutex<NetworkDay>,
    /// 采样器进程内前一轮网卡累计字节数；重置/计数器回退会从新基线继续
    net_totals: Mutex<HashMap<String, (u64, u64)>>,
}

impl Cache {
    pub fn new() -> Self {
        Self {
            stats: RwLock::new(Arc::new(Stats::default())),
            tick: Mutex::new(0),
            net_day: Mutex::new(load_network_day()),
            net_totals: Mutex::new(HashMap::new()),
        }
    }
}

fn publish_stats_snapshot(cache: &RwLock<Arc<Stats>>, stats: Stats) -> Arc<Stats> {
    let stats = Arc::new(stats);
    *cache.write().unwrap() = Arc::clone(&stats);
    stats
}

fn owned_stats_snapshot(cache: &Cache) -> Stats {
    cache.stats.read().unwrap().as_ref().clone()
}

fn contains_loopback_marker(name: &str) -> bool {
    const MARKER: &[u8] = b"loopback";
    name.as_bytes()
        .windows(MARKER.len())
        .any(|window| window.eq_ignore_ascii_case(MARKER))
}

fn network_delta(
    current: &HashMap<String, (u64, u64)>,
    previous: &mut HashMap<String, (u64, u64)>,
) -> (u64, u64) {
    let (mut rx, mut tx) = (0u64, 0u64);
    for (name, &(received, transmitted)) in current {
        if contains_loopback_marker(name) {
            continue;
        }
        if let Some(previous_counters) = previous.get_mut(name) {
            let (old_rx, old_tx) = *previous_counters;
            rx = rx.saturating_add(received.saturating_sub(old_rx));
            tx = tx.saturating_add(transmitted.saturating_sub(old_tx));
            *previous_counters = (received, transmitted);
        } else {
            previous.insert(name.clone(), (received, transmitted));
        }
    }
    previous.retain(|name, _| current.contains_key(name));
    (rx, tx)
}

fn battery_percent(value: u8) -> Option<u8> {
    (value <= 100).then_some(value)
}

fn cpu_frequency_is_plausible(frequency: u64) -> bool {
    (500..=10_000).contains(&frequency)
}

fn rounded_cpu_frequency_average(sum: u128, count: u128) -> u64 {
    (sum + count / 2)
        .checked_div(count)
        .map_or(0, |avg| avg.min(u64::MAX as u128) as u64)
}

#[cfg(test)]
fn aggregate_cpu_frequency_mhz(frequencies: impl IntoIterator<Item = u64>) -> u64 {
    let (sum, count) = frequencies
        .into_iter()
        .filter(|frequency| cpu_frequency_is_plausible(*frequency))
        .fold((0u128, 0u128), |(sum, count), frequency| {
            (sum + frequency as u128, count + 1)
        });
    rounded_cpu_frequency_average(sum, count)
}

fn battery_poll_interval_seconds() -> u64 {
    5
}

fn battery_query_succeeded(result: Option<&Result<Option<BatteryInfo>, u32>>) -> bool {
    matches!(result, Some(Ok(_)))
}

#[cfg(test)]
fn valid_cpu_frequency_samples(frequencies: &[u64], expected_count: usize) -> bool {
    expected_count > 0
        && frequencies.len() == expected_count
        && frequencies
            .iter()
            .any(|frequency| cpu_frequency_is_plausible(*frequency))
}

const CPU_FREQUENCY_REFRESH_INTERVAL: Duration = Duration::from_secs(5);
const MEMORY_SAMPLE_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct CpuFrequencyCache {
    average_mhz: u64,
    refreshed_at: Option<Instant>,
}

impl CpuFrequencyCache {
    fn refresh_due(&self, now: Instant) -> bool {
        self.refreshed_at.is_none_or(|last| {
            now.saturating_duration_since(last) >= CPU_FREQUENCY_REFRESH_INTERVAL
        })
    }

    #[cfg(test)]
    fn refresh(&mut self, now: Instant, frequencies: &[u64], expected_count: usize) -> bool {
        let valid = valid_cpu_frequency_samples(frequencies, expected_count);
        self.average_mhz = if valid {
            aggregate_cpu_frequency_mhz(frequencies.iter().copied())
        } else {
            0
        };
        // A failed sensor read still advances the polling cadence; don't hammer the API or show it as valid.
        self.refreshed_at = Some(now);
        valid
    }

    fn refresh_iter(
        &mut self,
        now: Instant,
        frequencies: impl IntoIterator<Item = u64>,
        expected_count: usize,
    ) -> bool {
        let mut sample_count = 0usize;
        let mut sum = 0u128;
        let mut valid_count = 0u128;
        for frequency in frequencies {
            sample_count += 1;
            if cpu_frequency_is_plausible(frequency) {
                sum += frequency as u128;
                valid_count += 1;
            }
        }
        let valid = expected_count > 0 && sample_count == expected_count && valid_count > 0;
        self.average_mhz = if valid {
            rounded_cpu_frequency_average(sum, valid_count)
        } else {
            0
        };
        // Invalid reads still advance cadence, matching the slice-backed reference path.
        self.refreshed_at = Some(now);
        valid
    }
}

fn memory_sample_refresh_due(last_sample_tick: Option<Instant>, tick_started_at: Instant) -> bool {
    last_sample_tick.is_none_or(|last| {
        tick_started_at.saturating_duration_since(last) >= MEMORY_SAMPLE_REFRESH_INTERVAL
    })
}

/// Use one nominal tick for the first diagnostics observation; only later samples can be judged
/// against a previous observation for genuine long-gap resets.
fn diagnostics_sample_elapsed(
    last_sample_at: Option<Instant>,
    sample_at: Instant,
    first_sample_interval: Duration,
) -> Duration {
    last_sample_at.map_or(first_sample_interval, |last| {
        sample_at.saturating_duration_since(last)
    })
}

fn process_refresh_interval(visible: bool) -> Duration {
    if visible {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(10)
    }
}

fn cpu_core_load_summary(per_core: &[f32]) -> (f32, usize, usize) {
    let mut peak = 0.0f32;
    let mut saturated = 0usize;
    let mut count = 0usize;
    for value in per_core.iter().copied().filter(|value| value.is_finite()) {
        let usage = value.clamp(0.0, 100.0);
        peak = peak.max(usage);
        saturated += usize::from(usage >= 85.0);
        count += 1;
    }
    (peak, saturated, count)
}

fn process_io_rate_kbps(read_delta: u64, write_delta: u64, elapsed: Duration) -> f64 {
    network_rate_kbps(read_delta.saturating_add(write_delta), elapsed)
}

fn process_io_sample_rate_kbps(
    read_delta: u64,
    write_delta: u64,
    elapsed: Duration,
    current_start_time: u64,
    previous_start_time: Option<u64>,
    baseline_established: bool,
) -> Option<f64> {
    (baseline_established && previous_start_time == Some(current_start_time))
        .then(|| process_io_rate_kbps(read_delta, write_delta, elapsed))
}

fn process_io_sample_payload(io_rate: Option<f64>) -> (f64, bool) {
    (io_rate.unwrap_or(0.0), io_rate.is_some())
}

struct ProcSampleUpdate<'a> {
    pid: u32,
    start_time: Option<u64>,
    name: std::borrow::Cow<'a, str>,
    cpu: f32,
    mem_mb: f64,
    io_kbps: f64,
    io_sample_fresh: bool,
}

fn update_proc_sample_entry(
    cache: &mut Vec<crate::diag::ProcSample>,
    index: usize,
    update: ProcSampleUpdate<'_>,
) {
    debug_assert!(index <= cache.len());
    if index == cache.len() {
        let name = match update.name {
            std::borrow::Cow::Borrowed(name) => name.to_owned(),
            std::borrow::Cow::Owned(name) => name,
        };
        cache.push(crate::diag::ProcSample {
            pid: update.pid,
            start_time: update.start_time,
            name,
            cpu: update.cpu,
            mem_mb: update.mem_mb,
            io_kbps: update.io_kbps,
            io_sample_fresh: update.io_sample_fresh,
        });
        return;
    }

    let sample = &mut cache[index];
    sample.pid = update.pid;
    sample.start_time = update.start_time;
    if sample.name != update.name.as_ref() {
        match update.name {
            std::borrow::Cow::Borrowed(name) => {
                sample.name.clear();
                sample.name.push_str(name);
            }
            std::borrow::Cow::Owned(name) => sample.name = name,
        }
    }
    sample.cpu = update.cpu;
    sample.mem_mb = update.mem_mb;
    sample.io_kbps = update.io_kbps;
    sample.io_sample_fresh = update.io_sample_fresh;
}

fn update_process_io_start_baseline(
    baselines: &mut HashMap<u32, u64>,
    live_pids: Option<&mut std::collections::HashSet<u32>>,
    pid: u32,
    start_time: u64,
    sampled_this_tick: bool,
    delta_valid: bool,
) -> Option<u64> {
    let previous_start_time = if sampled_this_tick && delta_valid {
        baselines.get(&pid).copied()
    } else {
        None
    };
    if let Some(live_pids) = live_pids {
        live_pids.insert(pid);
    }
    if sampled_this_tick {
        baselines.insert(pid, start_time);
    }
    previous_start_time
}

fn prune_process_io_start_baselines(
    baselines: &mut HashMap<u32, u64>,
    live_pids: Option<&std::collections::HashSet<u32>>,
) {
    if let Some(live_pids) = live_pids {
        baselines.retain(|pid, _| live_pids.contains(pid));
    }
}

fn network_rate_kbps(bytes: u64, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if !seconds.is_finite() || seconds <= 0.0 {
        0.0
    } else {
        bytes as f64 / seconds / 1024.0
    }
}

fn collect_disks(disks: &Disks) -> Vec<DiskInfo> {
    let mut result = Vec::with_capacity(disks.list().len());
    let mut seen = std::collections::HashSet::with_capacity(disks.list().len());
    for disk in disks.list() {
        let mount = disk
            .mount_point()
            .to_string_lossy()
            .trim_end_matches('\\')
            .to_uppercase();
        if mount.is_empty() || !seen.insert(mount.clone()) {
            continue;
        }
        let total = disk.total_space() as f64;
        if total <= 0.0 {
            continue;
        }
        let available = disk.available_space() as f64;
        result.push(DiskInfo {
            mount,
            total_gb: total / 1024.0 / 1024.0 / 1024.0,
            used_gb: (total - available) / 1024.0 / 1024.0 / 1024.0,
            fs: disk.file_system().to_string_lossy().to_string(),
        });
    }
    result
}

#[cfg(test)]
mod effective_power_mode_tests {
    use super::{effective_power_mode_from_raw, EffectivePowerMode, Stats};

    #[test]
    fn native_effective_power_mode_values_map_to_stable_wire_names() {
        let expected = [
            EffectivePowerMode::BatterySaver,
            EffectivePowerMode::BetterBattery,
            EffectivePowerMode::Balanced,
            EffectivePowerMode::HighPerformance,
            EffectivePowerMode::MaxPerformance,
            EffectivePowerMode::GameMode,
            EffectivePowerMode::MixedReality,
        ];
        for (raw, mode) in expected.into_iter().enumerate() {
            assert_eq!(effective_power_mode_from_raw(raw as i32), Some(mode));
        }
        assert_eq!(effective_power_mode_from_raw(-1), None);
        assert_eq!(effective_power_mode_from_raw(7), None);
    }

    #[test]
    fn absent_and_known_power_modes_serialize_explicitly() {
        let absent = serde_json::to_value(Stats::default()).expect("default Stats serializes");
        assert_eq!(absent["effective_power_mode"], serde_json::Value::Null);

        let known = Stats {
            effective_power_mode: Some(EffectivePowerMode::GameMode),
            ..Stats::default()
        };
        let known = serde_json::to_value(known).expect("known power mode serializes");
        assert_eq!(known["effective_power_mode"], "game_mode");
    }

    #[cfg(windows)]
    #[test]
    fn windows_power_mode_subscription_is_best_effort_and_publishes_state() {
        let registration = super::register_effective_power_mode_notifications();
        if registration.is_some() {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while super::current_effective_power_mode().is_none()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert!(
                super::current_effective_power_mode().is_some(),
                "registered callback should publish a recognized effective power mode"
            );
        }
    }
}

#[cfg(test)]
mod network_counter_tests {
    use super::{
        aggregate_cpu_frequency_mhz, battery_info_from_status, battery_percent,
        battery_poll_interval_seconds, battery_query_succeeded, collect_disks,
        cpu_core_load_summary, diagnostics_sample_elapsed, memory_sample_refresh_due,
        memory_usage_pct, network_delta, network_rate_kbps, process_io_rate_kbps,
        process_io_sample_payload, process_io_sample_rate_kbps, process_refresh_interval,
        prune_process_io_start_baselines, select_network_day, timezone_id_from_api_result,
        timezone_id_from_utf16, timezone_identity_refresh_due, update_proc_sample_entry,
        update_process_io_start_baseline, valid_cpu_frequency_samples, CpuFrequencyCache,
        NetworkDay, ProcSampleUpdate, CPU_FREQUENCY_REFRESH_INTERVAL,
        MEMORY_SAMPLE_REFRESH_INTERVAL,
    };
    use std::collections::{HashMap, HashSet};
    use std::time::{Duration, Instant};
    use sysinfo::{DiskRefreshKind, Disks, System};

    fn proc_sample(
        pid: u32,
        start_time: Option<u64>,
        name: &str,
        cpu: f32,
        mem_mb: f64,
        io_kbps: f64,
        io_sample_fresh: bool,
    ) -> crate::diag::ProcSample {
        crate::diag::ProcSample {
            pid,
            start_time,
            name: name.to_string(),
            cpu,
            mem_mb,
            io_kbps,
            io_sample_fresh,
        }
    }

    fn rebuild_proc_sample_cache(
        cache: &mut Vec<crate::diag::ProcSample>,
        samples: &[crate::diag::ProcSample],
    ) {
        cache.clear();
        cache.extend(samples.iter().cloned());
    }

    fn reuse_proc_sample_cache(
        cache: &mut Vec<crate::diag::ProcSample>,
        samples: &[crate::diag::ProcSample],
    ) {
        let additional = samples.len().saturating_sub(cache.len());
        cache.reserve(additional);
        for (index, sample) in samples.iter().enumerate() {
            update_proc_sample_entry(
                cache,
                index,
                ProcSampleUpdate {
                    pid: sample.pid,
                    start_time: sample.start_time,
                    name: std::borrow::Cow::Borrowed(&sample.name),
                    cpu: sample.cpu,
                    mem_mb: sample.mem_mb,
                    io_kbps: sample.io_kbps,
                    io_sample_fresh: sample.io_sample_fresh,
                },
            );
        }
        cache.truncate(samples.len());
    }

    fn assert_proc_samples_equal(
        actual: &[crate::diag::ProcSample],
        expected: &[crate::diag::ProcSample],
    ) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert_eq!(actual.pid, expected.pid);
            assert_eq!(actual.start_time, expected.start_time);
            assert_eq!(actual.name, expected.name);
            assert_eq!(actual.cpu.to_bits(), expected.cpu.to_bits());
            assert_eq!(actual.mem_mb.to_bits(), expected.mem_mb.to_bits());
            assert_eq!(actual.io_kbps.to_bits(), expected.io_kbps.to_bits());
            assert_eq!(actual.io_sample_fresh, expected.io_sample_fresh);
        }
    }

    fn counters(entries: &[(&str, u64, u64)]) -> HashMap<String, (u64, u64)> {
        entries
            .iter()
            .map(|(name, rx, tx)| ((*name).to_string(), (*rx, *tx)))
            .collect()
    }

    fn reference_network_delta(
        current: &HashMap<String, (u64, u64)>,
        previous: &mut HashMap<String, (u64, u64)>,
    ) -> (u64, u64) {
        let (mut rx, mut tx) = (0u64, 0u64);
        for (name, &(received, transmitted)) in current {
            // Isolate the previous get+clone+insert update strategy from filter costs.
            if super::contains_loopback_marker(name) {
                continue;
            }
            if let Some(&(old_rx, old_tx)) = previous.get(name) {
                rx = rx.saturating_add(received.saturating_sub(old_rx));
                tx = tx.saturating_add(transmitted.saturating_sub(old_tx));
            }
            previous.insert(name.clone(), (received, transmitted));
        }
        previous.retain(|name, _| current.contains_key(name));
        (rx, tx)
    }

    fn representative_stats_snapshot(sample_id: u64) -> super::Stats {
        super::Stats {
            sample_id,
            cpu: 31.5,
            per_core: (0..16).map(|index| 10.0 + index as f32).collect(),
            cpu_freq_mhz: 3_600,
            mem_used_gb: 12.5,
            mem_total_gb: 32.0,
            swap_used_gb: 0.5,
            swap_total_gb: 16.0,
            system_drive: "C:".into(),
            disks: ["C:", "D:", "E:"]
                .map(|mount| super::DiskInfo {
                    mount: mount.into(),
                    total_gb: 512.0,
                    used_gb: 256.0,
                    fs: "NTFS".into(),
                })
                .into(),
            net_rx_kbps: 1.5,
            net_tx_kbps: 2.5,
            net_today_rx_gb: 4.0,
            net_today_tx_gb: 2.0,
            gpu: Some(super::GpuInfo {
                name: "Example GPU".into(),
                utilization: Some(35.0),
                mem_used_mb: Some(2_048),
                mem_total_mb: Some(8_192),
                temperature: Some(55.0),
            }),
            battery: Some(super::BatteryInfo {
                ac_online: true,
                ac_connected: Some(true),
                percent: Some(85),
                secs_left: Some(3_600),
            }),
            battery_status_ok: true,
            effective_power_mode: Some(super::EffectivePowerMode::Balanced),
            uptime_secs: 86_400,
            process_count: 247,
        }
    }

    #[test]
    fn stats_arc_cache_handoff_preserves_payload_and_replacement_semantics() {
        let cache = super::Cache {
            stats: std::sync::RwLock::new(std::sync::Arc::new(super::Stats::default())),
            tick: std::sync::Mutex::new(0),
            net_day: std::sync::Mutex::new(network_day("2026-01-01", None, 0, 0)),
            net_totals: std::sync::Mutex::new(HashMap::new()),
        };
        let first = super::publish_stats_snapshot(&cache.stats, representative_stats_snapshot(10));
        let cached_first = cache.stats.read().unwrap().clone();
        assert!(std::sync::Arc::ptr_eq(&first, &cached_first));
        let first_payload = serde_json::to_vec(first.as_ref()).unwrap();
        assert_eq!(
            serde_json::to_vec(cached_first.as_ref()).unwrap(),
            first_payload
        );
        assert_eq!(
            serde_json::to_vec(&super::owned_stats_snapshot(&cache)).unwrap(),
            first_payload
        );

        let retained_first = std::sync::Arc::clone(&first);
        let second = super::publish_stats_snapshot(&cache.stats, representative_stats_snapshot(11));
        let cached_second = cache.stats.read().unwrap().clone();
        assert!(std::sync::Arc::ptr_eq(&second, &cached_second));
        assert_eq!(retained_first.sample_id, 10);
        assert_eq!(second.sample_id, 11);
        assert_eq!(
            serde_json::to_vec(cached_second.as_ref()).unwrap(),
            serde_json::to_vec(second.as_ref()).unwrap()
        );
    }

    #[test]
    #[ignore = "manual synthetic stats snapshot clone versus shared ownership benchmark"]
    fn benchmark_stats_snapshot_cache_clone_vs_arc_handoff() {
        const BATCHES: usize = 101;
        const SNAPSHOTS_PER_BATCH: usize = 128;
        let legacy_cache = std::sync::RwLock::new(super::Stats::default());
        let shared_cache = std::sync::RwLock::new(std::sync::Arc::new(super::Stats::default()));
        let mut clone_micros = Vec::with_capacity(BATCHES);
        let mut arc_micros = Vec::with_capacity(BATCHES);

        for warmup in 0..5 {
            let mut checksum = 0u64;
            for index in 0..SNAPSHOTS_PER_BATCH {
                let stats = representative_stats_snapshot(index as u64);
                *legacy_cache.write().unwrap() = stats.clone();
                checksum = checksum.wrapping_add(std::hint::black_box(stats.sample_id));
                let stats = super::publish_stats_snapshot(
                    &shared_cache,
                    representative_stats_snapshot(index as u64),
                );
                checksum = checksum.wrapping_add(std::hint::black_box(stats.sample_id));
            }
            std::hint::black_box((warmup, checksum));
        }

        for batch in 0..BATCHES {
            let measure_clone = || {
                let started = Instant::now();
                let mut checksum = 0u64;
                for index in 0..SNAPSHOTS_PER_BATCH {
                    let stats = representative_stats_snapshot(index as u64);
                    *legacy_cache.write().unwrap() = stats.clone();
                    checksum = checksum.wrapping_add(std::hint::black_box(stats.sample_id));
                }
                std::hint::black_box(checksum);
                started.elapsed().as_secs_f64() * 1_000_000.0 / SNAPSHOTS_PER_BATCH as f64
            };
            let measure_arc = || {
                let started = Instant::now();
                let mut checksum = 0u64;
                for index in 0..SNAPSHOTS_PER_BATCH {
                    let stats = super::publish_stats_snapshot(
                        &shared_cache,
                        representative_stats_snapshot(index as u64),
                    );
                    checksum = checksum.wrapping_add(std::hint::black_box(stats.sample_id));
                }
                std::hint::black_box(checksum);
                started.elapsed().as_secs_f64() * 1_000_000.0 / SNAPSHOTS_PER_BATCH as f64
            };
            if batch % 2 == 0 {
                clone_micros.push(measure_clone());
                arc_micros.push(measure_arc());
            } else {
                arc_micros.push(measure_arc());
                clone_micros.push(measure_clone());
            }
        }
        clone_micros.sort_by(f64::total_cmp);
        arc_micros.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "stats snapshot cache publication ({} representative snapshots/batch): RwLock<Stats> clone median/p95={:.3}/{:.3} us; RwLock<Arc<Stats>> handoff={:.3}/{:.3} us",
            SNAPSHOTS_PER_BATCH,
            clone_micros[BATCHES / 2],
            clone_micros[p95_index],
            arc_micros[BATCHES / 2],
            arc_micros[p95_index],
        );
    }

    fn network_day(
        local_date: &str,
        time_zone_id: Option<&str>,
        rx_bytes: u64,
        tx_bytes: u64,
    ) -> NetworkDay {
        NetworkDay {
            local_date: local_date.into(),
            time_zone_id: time_zone_id.map(str::to_string),
            rx_bytes,
            tx_bytes,
        }
    }

    #[test]
    fn daily_network_totals_restore_only_for_the_same_local_date_and_time_zone() {
        let utc = "windows-time-zone:utc:dynamic-dst-disabled:false";
        let gmt = "windows-time-zone:gmt standard time:dynamic-dst-disabled:false";
        let saved = network_day("2026-01-20", Some(utc), 100, 200);
        assert_eq!(
            select_network_day("2026-01-20", Some(utc), Some(saved.clone())),
            saved
        );
        // UTC and GMT Standard Time share the same winter offset but are distinct zones.
        assert_eq!(
            select_network_day("2026-01-20", Some(gmt), Some(saved.clone())),
            network_day("2026-01-20", Some(gmt), 0, 0)
        );
        assert_eq!(
            select_network_day("2026-01-21", Some(utc), Some(saved.clone())),
            network_day("2026-01-21", Some(utc), 0, 0)
        );
        assert_eq!(
            select_network_day("2026-01-20", Some(utc), None),
            network_day("2026-01-20", Some(utc), 0, 0)
        );
        assert_eq!(
            select_network_day("2026-01-20", None, Some(saved)),
            network_day("2026-01-20", None, 0, 0)
        );
    }

    #[test]
    fn legacy_network_day_tuple_is_discarded_as_unscoped_data() {
        let legacy = serde_json::to_vec(&("2026-01-20", 100_u64, 200_u64)).unwrap();
        let saved = serde_json::from_slice::<NetworkDay>(&legacy).ok();
        assert_eq!(
            select_network_day(
                "2026-01-20",
                Some("windows-time-zone:utc:dynamic-dst-disabled:false"),
                saved
            ),
            network_day(
                "2026-01-20",
                Some("windows-time-zone:utc:dynamic-dst-disabled:false"),
                0,
                0
            )
        );
    }

    #[test]
    fn unavailable_timezone_id_is_never_reused_as_persistent_identity() {
        let saved = network_day(
            "2026-01-20",
            Some("windows-time-zone:utc:dynamic-dst-disabled:false"),
            100,
            200,
        );
        assert_eq!(
            select_network_day("2026-01-20", None, Some(saved)),
            network_day("2026-01-20", None, 0, 0)
        );
    }

    #[test]
    fn timezone_key_decoding_requires_valid_nul_terminated_utf16() {
        let valid = "Pacific Standard Time\0".encode_utf16().collect::<Vec<_>>();
        assert_eq!(
            timezone_id_from_utf16(&valid).as_deref(),
            Some("pacific standard time")
        );
        assert_eq!(timezone_id_from_utf16(&[0]), None);
        assert_eq!(timezone_id_from_utf16(&[0xD800, 0]), None);
        assert_eq!(
            timezone_id_from_utf16(&[b'U' as u16, b'T' as u16, b'C' as u16]),
            None
        );
    }

    #[test]
    fn timezone_query_accepts_unknown_standard_and_daylight_statuses() {
        let id = "UTC\0".encode_utf16().collect::<Vec<_>>();
        for status in [0, 1, 2] {
            assert_eq!(
                timezone_id_from_api_result(status, u32::MAX, &id).as_deref(),
                Some("utc")
            );
        }
        assert_eq!(timezone_id_from_api_result(u32::MAX, u32::MAX, &id), None);
    }

    #[test]
    fn timezone_identity_cache_refreshes_at_one_minute_boundary() {
        let checked_at = Instant::now();
        assert!(!timezone_identity_refresh_due(
            checked_at,
            checked_at + Duration::from_secs(59)
        ));
        assert!(timezone_identity_refresh_due(
            checked_at,
            checked_at + Duration::from_secs(60)
        ));
        assert!(timezone_identity_refresh_due(
            checked_at,
            checked_at + Duration::from_secs(61)
        ));
    }

    #[test]
    fn battery_poll_period_is_five_seconds() {
        assert_eq!(battery_poll_interval_seconds(), 5);
    }

    #[test]
    fn diagnostics_first_sample_uses_one_tick_and_later_gaps_remain_visible() {
        let sampled_at = Instant::now();
        assert_eq!(
            diagnostics_sample_elapsed(None, sampled_at, Duration::from_secs(1),),
            Duration::from_secs(1),
        );
        assert_eq!(
            diagnostics_sample_elapsed(
                Some(sampled_at - Duration::from_secs(6)),
                sampled_at,
                Duration::from_secs(1),
            ),
            Duration::from_secs(6),
            "a real sampling interruption must still reach the diagnostic gap reset",
        );
    }

    #[test]
    fn memory_sensor_refresh_uses_tick_start_to_maintain_one_hz_cadence() {
        let tick_start = Instant::now();
        assert!(memory_sample_refresh_due(None, tick_start));
        assert!(!memory_sample_refresh_due(
            Some(tick_start),
            tick_start + Duration::from_millis(999)
        ));
        assert!(memory_sample_refresh_due(
            Some(tick_start),
            tick_start + MEMORY_SAMPLE_REFRESH_INTERVAL
        ));

        // Timestamping completion instead of the scheduled loop start misses the next
        // 1 Hz tick because the refresh itself happens after that start.
        let sensor_refresh_completed = tick_start + Duration::from_millis(5);
        assert!(!memory_sample_refresh_due(
            Some(sensor_refresh_completed),
            tick_start + MEMORY_SAMPLE_REFRESH_INTERVAL
        ));
    }

    #[test]
    #[ignore = "manual read-only RAM sensor refresh latency benchmark"]
    fn benchmark_ram_sensor_refresh_latency() {
        let mut system = System::new();
        let refresh_kind = sysinfo::MemoryRefreshKind::nothing().with_ram().with_swap();
        system.refresh_memory_specifics(refresh_kind);

        let mut samples = Vec::with_capacity(51);
        for _ in 0..51 {
            let started = Instant::now();
            system.refresh_memory_specifics(refresh_kind);
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        let p95_index = (samples.len() * 95).div_ceil(100) - 1;
        eprintln!(
            "RAM sensor refresh benchmark: samples={}, median={:?}, p95={:?}",
            samples.len(),
            samples[samples.len() / 2],
            samples[p95_index]
        );
    }

    #[test]
    fn process_inventory_refresh_is_slower_in_tray_mode() {
        assert_eq!(process_refresh_interval(true), Duration::from_secs(5));
        assert_eq!(process_refresh_interval(false), Duration::from_secs(10));
    }

    #[test]
    fn core_load_summary_clamps_invalid_samples_and_counts_hot_cores() {
        let (peak, saturated, count) = cpu_core_load_summary(&[30.0, 90.0, 120.0, -5.0, f32::NAN]);
        assert_eq!(peak, 100.0);
        assert_eq!(saturated, 2);
        assert_eq!(count, 4);
        assert_eq!(
            cpu_core_load_summary(&[f32::NAN, f32::INFINITY]),
            (0.0, 0, 0)
        );
    }

    #[test]
    fn unavailable_or_invalid_memory_capacity_is_not_reported_as_zero_percent() {
        assert_eq!(memory_usage_pct(0.0, 8.0), Some(0.0));
        assert_eq!(memory_usage_pct(4.0, 8.0), Some(50.0));
        assert_eq!(memory_usage_pct(8.0, 8.0), Some(100.0));
        assert_eq!(memory_usage_pct(0.0, 0.0), None);
        assert_eq!(memory_usage_pct(9.0, 8.0), None);
        assert_eq!(memory_usage_pct(-1.0, 8.0), None);
        assert_eq!(memory_usage_pct(1.0, f64::NAN), None);
    }

    #[test]
    fn battery_status_preserves_unknown_ac_instead_of_claiming_discharge() {
        assert_eq!(
            battery_info_from_status(2, 0, 65, 3600).and_then(|battery| battery.ac_connected),
            None
        );
        assert!(battery_info_from_status(2, 0, 65, 3600).is_some_and(|battery| battery.ac_online));
        assert_eq!(
            battery_info_from_status(0, 0, 65, u32::MAX).map(|battery| (
                battery.ac_connected,
                battery.ac_online,
                battery.secs_left
            )),
            Some((Some(false), false, None))
        );
        assert_eq!(
            battery_info_from_status(1, 0, 255, 0).map(|battery| (
                battery.ac_connected,
                battery.ac_online,
                battery.percent
            )),
            Some((Some(true), true, None))
        );
        assert!(battery_info_from_status(1, 128, 50, 10).is_none());
    }

    #[test]
    fn battery_percent_window_boundaries_are_explicit() {
        assert_eq!(battery_percent(0), Some(0));
        assert_eq!(battery_percent(100), Some(100));
        assert_eq!(battery_percent(255), None);
        assert!(battery_query_succeeded(Some(&Ok(None))));
        assert!(!battery_query_succeeded(Some(&Err(5))));
        assert!(!battery_query_succeeded(None));
    }

    #[test]
    fn cpu_frequency_averages_only_valid_per_core_samples() {
        assert_eq!(aggregate_cpu_frequency_mhz([2400, 2600, 0, 2800]), 2600);
        assert_eq!(aggregate_cpu_frequency_mhz([0, 0, 0]), 0);
        assert_eq!(aggregate_cpu_frequency_mhz([]), 0);
        assert_eq!(aggregate_cpu_frequency_mhz([u64::MAX, u64::MAX]), 0);
        assert_eq!(aggregate_cpu_frequency_mhz([200, 50_000]), 0);
        assert_eq!(aggregate_cpu_frequency_mhz([2400, 50_000]), 2400);
        assert!(valid_cpu_frequency_samples(&[2400, 50_000], 2));
        assert!(valid_cpu_frequency_samples(&[2400, 0, 2800], 3));
        assert!(!valid_cpu_frequency_samples(&[0, 0, 0], 3));
        assert!(!valid_cpu_frequency_samples(&[2400], 3));
        assert!(!valid_cpu_frequency_samples(&[2400], 0));
    }

    #[test]
    fn cpu_frequency_streaming_refresh_matches_slice_reference() {
        let cases: &[(&[u64], usize)] = &[
            (&[2400, 2600, 0], 3),
            (&[200, 50_000], 2),
            (&[2400], 3),
            (&[], 0),
            (&[500, 10_000], 2),
            (&[2400, 2601, 0], 3),
            (&[2400, 50_000], 2),
            (&[2400], 0),
        ];
        for &(frequencies, expected_count) in cases {
            let now = Instant::now();
            let mut slice_reference = CpuFrequencyCache::default();
            let mut streaming = CpuFrequencyCache::default();
            let expected_valid = slice_reference.refresh(now, frequencies, expected_count);
            let actual_valid =
                streaming.refresh_iter(now, frequencies.iter().copied(), expected_count);
            assert_eq!(actual_valid, expected_valid, "{frequencies:?}");
            assert_eq!(
                streaming.average_mhz, slice_reference.average_mhz,
                "{frequencies:?}"
            );
            assert_eq!(
                streaming.refreshed_at, slice_reference.refreshed_at,
                "{frequencies:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual Release benchmark for CPU-frequency streaming aggregation"]
    fn benchmark_cpu_frequency_streaming_refresh() {
        const BATCHES: usize = 101;
        for core_count in [8usize, 32, 128] {
            let frequencies = (0..core_count)
                .map(|index| {
                    if index % 17 == 0 {
                        0
                    } else {
                        2200 + (index % 11) as u64 * 100
                    }
                })
                .collect::<Vec<_>>();
            let mut reference_cache = CpuFrequencyCache::default();
            let mut streaming_cache = CpuFrequencyCache::default();
            let now = Instant::now();
            let reference_valid = reference_cache.refresh(now, &frequencies, core_count);
            let streaming_valid =
                streaming_cache.refresh_iter(now, frequencies.iter().copied(), core_count);
            assert_eq!(streaming_valid, reference_valid);
            assert_eq!(streaming_cache.average_mhz, reference_cache.average_mhz);

            let mut measure_reference = || {
                let started = Instant::now();
                let materialized = frequencies.iter().copied().collect::<Vec<_>>();
                let valid = reference_cache.refresh(started, &materialized, core_count);
                std::hint::black_box((valid, reference_cache.average_mhz));
                started.elapsed().as_nanos()
            };
            let mut measure_streaming = || {
                let started = Instant::now();
                let valid =
                    streaming_cache.refresh_iter(started, frequencies.iter().copied(), core_count);
                std::hint::black_box((valid, streaming_cache.average_mhz));
                started.elapsed().as_nanos()
            };
            for _ in 0..7 {
                std::hint::black_box(measure_reference());
                std::hint::black_box(measure_streaming());
            }
            let mut reference_nanos = Vec::with_capacity(BATCHES);
            let mut streaming_nanos = Vec::with_capacity(BATCHES);
            for batch in 0..BATCHES {
                if batch % 2 == 0 {
                    reference_nanos.push(measure_reference());
                    streaming_nanos.push(measure_streaming());
                } else {
                    streaming_nanos.push(measure_streaming());
                    reference_nanos.push(measure_reference());
                }
            }
            reference_nanos.sort_unstable();
            streaming_nanos.sort_unstable();
            let p95_index = (BATCHES * 95).div_ceil(100) - 1;
            eprintln!(
                "CPU frequency cache refresh ({} cores): Vec-reference median/p95={:.1}/{:.1} us; streaming={:.1}/{:.1}",
                core_count,
                reference_nanos[BATCHES / 2] as f64 / 1_000.0,
                reference_nanos[p95_index] as f64 / 1_000.0,
                streaming_nanos[BATCHES / 2] as f64 / 1_000.0,
                streaming_nanos[p95_index] as f64 / 1_000.0,
            );
        }
    }

    #[test]
    fn cpu_frequency_cache_reuses_value_until_five_second_refresh() {
        let now = Instant::now();
        let mut cache = CpuFrequencyCache::default();
        assert!(cache.refresh_due(now));
        assert!(cache.refresh(now, &[2400, 2600, 0], 3));
        assert_eq!(cache.average_mhz, 2500);
        assert!(!cache.refresh_due(now + Duration::from_secs(4)));
        assert_eq!(cache.average_mhz, 2500);
        assert!(cache.refresh_due(now + CPU_FREQUENCY_REFRESH_INTERVAL));
        assert!(cache.refresh(
            now + CPU_FREQUENCY_REFRESH_INTERVAL,
            &[2200, 50_000, 2600],
            3
        ));
        assert_eq!(cache.average_mhz, 2400);

        let invalid_at = now + Duration::from_secs(10);
        assert!(cache.refresh_due(invalid_at));
        assert!(!cache.refresh(invalid_at, &[200, 200], 2));
        assert_eq!(cache.average_mhz, 0);
        assert!(!cache.refresh_due(invalid_at + Duration::from_secs(4)));
        assert!(cache.refresh_due(invalid_at + CPU_FREQUENCY_REFRESH_INTERVAL));
    }

    #[test]
    fn cpu_frequency_refresh_cadence_anchors_to_tick_start() {
        let tick_start = Instant::now();
        let next_five_second_tick = tick_start + CPU_FREQUENCY_REFRESH_INTERVAL;
        let mut cache = CpuFrequencyCache::default();
        assert!(cache.refresh(tick_start, &[2400], 1));
        assert!(cache.refresh_due(next_five_second_tick));

        // A completion-time timestamp several milliseconds after the tick start
        // would miss the nominal five-second refresh tick.
        let mut completion_stamped_cache = CpuFrequencyCache::default();
        assert!(completion_stamped_cache.refresh(
            tick_start + Duration::from_millis(5),
            &[2400],
            1,
        ));
        assert!(!completion_stamped_cache.refresh_due(next_five_second_tick));
    }

    #[test]
    #[ignore = "manual read-only CPU frequency refresh latency benchmark"]
    fn benchmark_cpu_frequency_refresh_latency() {
        let mut system = System::new();
        system.refresh_cpu_frequency();
        assert!(!system.cpus().is_empty());

        let mut cache = CpuFrequencyCache::default();
        let mut samples = Vec::with_capacity(51);
        for _ in 0..51 {
            let started = Instant::now();
            system.refresh_cpu_frequency();
            let frequencies = system
                .cpus()
                .iter()
                .map(|cpu| cpu.frequency())
                .collect::<Vec<_>>();
            let _ = cache.refresh(started, &frequencies, system.cpus().len());
            samples.push(started.elapsed());
        }
        samples.sort_unstable();
        let p95_index = (samples.len() * 95).div_ceil(100) - 1;
        eprintln!(
            "CPU frequency refresh benchmark: samples={}, cpus={}, median={:?}, p95={:?}",
            samples.len(),
            system.cpus().len(),
            samples[samples.len() / 2],
            samples[p95_index]
        );
    }

    #[test]
    fn disk_inventory_uses_unique_mount_roots_and_cached_capacity() {
        let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing());
        let _ = collect_disks(&disks);
    }

    #[test]
    fn live_cpu_inventory_and_frequency_match_windows_power_query() {
        let mut sys = System::new();
        sys.refresh_cpu_frequency();
        let frequencies = sys
            .cpus()
            .iter()
            .map(|cpu| cpu.frequency())
            .collect::<Vec<_>>();
        let active_count = unsafe {
            windows_sys::Win32::System::Threading::GetActiveProcessorCount(
                windows_sys::Win32::System::Threading::ALL_PROCESSOR_GROUPS,
            )
        } as usize;
        assert!(
            active_count > 0,
            "Windows must report at least one active processor"
        );
        assert_eq!(frequencies.len(), active_count);
        assert!(
            !sys.cpus().is_empty(),
            "Windows must expose at least one logical CPU"
        );
        assert!(
            valid_cpu_frequency_samples(&frequencies, active_count),
            "sysinfo CallNtPowerInformation returned no plausible current frequency for the logical CPUs"
        );
        let average = aggregate_cpu_frequency_mhz(frequencies.iter().copied());
        assert!(
            (500..=10_000).contains(&average),
            "implausible CPU frequency {average} MHz"
        );
    }

    #[test]
    fn network_counters_count_deltas_once_and_ignore_first_baseline() {
        let mut previous = HashMap::new();
        assert_eq!(
            network_delta(&counters(&[("Ethernet", 1000, 2000)]), &mut previous),
            (0, 0)
        );
        assert_eq!(
            network_delta(&counters(&[("Ethernet", 1500, 2600)]), &mut previous),
            (500, 600)
        );
        assert_eq!(
            network_delta(&counters(&[("Ethernet", 1800, 2900)]), &mut previous),
            (300, 300)
        );
    }

    #[test]
    fn network_counter_in_place_refresh_matches_reference_across_resets_and_adapter_changes() {
        let samples = [
            counters(&[
                ("Ethernet", 100, 200),
                ("Wi-Fi", 50, 60),
                ("Loopback Pseudo-Interface", 5_000, 5_000),
            ]),
            counters(&[
                ("Ethernet", 150, 250),
                ("Wi-Fi", 70, 65),
                ("Loopback Pseudo-Interface", 7_000, 7_000),
            ]),
            counters(&[("Ethernet", 5, 1), ("USB Bridge", 100, 300)]),
            counters(&[("Ethernet", 25, 21), ("USB Bridge", 145, 330)]),
            counters(&[("USB Bridge", 160, 350), ("LOOPBACK Adapter", 9_000, 9_000)]),
        ];
        let mut optimized_previous = HashMap::new();
        let mut reference_previous = HashMap::new();
        for current in samples {
            assert_eq!(
                super::network_delta(&current, &mut optimized_previous),
                reference_network_delta(&current, &mut reference_previous)
            );
            assert_eq!(optimized_previous, reference_previous);
        }
    }

    #[test]
    #[ignore = "manual repeated network-counter refresh benchmark"]
    fn benchmark_network_counter_in_place_refresh() {
        const BATCHES: usize = 101;
        const SCANS_PER_BATCH: usize = 500;
        let current = counters(&[
            ("Ethernet", 10_000, 20_000),
            ("Wi-Fi", 30_000, 40_000),
            ("Loopback Pseudo-Interface", 50_000, 60_000),
            ("vEthernet (Default Switch)", 70_000, 80_000),
            ("USB Network Bridge", 90_000, 100_000),
            ("LOOPBACK Virtual Adapter", 110_000, 120_000),
            ("Bluetooth Network Connection", 130_000, 140_000),
            ("Ethernet 2", 150_000, 160_000),
        ]);
        let initial_previous = current.clone();
        let mut reference_micros = Vec::with_capacity(BATCHES);
        let mut in_place_micros = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            let measure_reference = || {
                let mut previous = initial_previous.clone();
                let started = Instant::now();
                let mut checksum = (0u64, 0u64);
                for _ in 0..SCANS_PER_BATCH {
                    let delta = reference_network_delta(&current, &mut previous);
                    checksum.0 = checksum.0.wrapping_add(delta.0);
                    checksum.1 = checksum.1.wrapping_add(delta.1);
                }
                std::hint::black_box((previous, checksum));
                started.elapsed().as_secs_f64() * 1_000_000.0 / SCANS_PER_BATCH as f64
            };
            let measure_in_place = || {
                let mut previous = initial_previous.clone();
                let started = Instant::now();
                let mut checksum = (0u64, 0u64);
                for _ in 0..SCANS_PER_BATCH {
                    let delta = super::network_delta(&current, &mut previous);
                    checksum.0 = checksum.0.wrapping_add(delta.0);
                    checksum.1 = checksum.1.wrapping_add(delta.1);
                }
                std::hint::black_box((previous, checksum));
                started.elapsed().as_secs_f64() * 1_000_000.0 / SCANS_PER_BATCH as f64
            };
            if batch % 2 == 0 {
                reference_micros.push(measure_reference());
                in_place_micros.push(measure_in_place());
            } else {
                in_place_micros.push(measure_in_place());
                reference_micros.push(measure_reference());
            }
        }
        reference_micros.sort_by(f64::total_cmp);
        in_place_micros.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "network counter refresh ({} interfaces/scan): clone+insert median/p95={:.3}/{:.3} us/scan; in-place update={:.3}/{:.3}",
            current.len(),
            reference_micros[BATCHES / 2],
            reference_micros[p95_index],
            in_place_micros[BATCHES / 2],
            in_place_micros[p95_index],
        );
    }

    #[test]
    fn loopback_name_filter_matches_ascii_case_insensitive_contains() {
        for name in [
            "loopback",
            "Loopback Pseudo-Interface",
            "Adapter-LOOPBACK-7",
            "网卡-LooPback-虚拟",
            "Ethernet",
            "loopbac",
            "",
        ] {
            assert_eq!(
                super::contains_loopback_marker(name),
                name.to_ascii_lowercase().contains("loopback"),
                "filter semantics must match the previous ASCII-lowercase substring check for {name:?}"
            );
        }
    }

    #[test]
    #[ignore = "manual synthetic network-adapter name-filter benchmark"]
    fn benchmark_network_loopback_filter_without_temporary_strings() {
        const BATCHES: usize = 101;
        const SCANS_PER_BATCH: usize = 400;
        const NAMES: [&str; 8] = [
            "Ethernet",
            "Wi-Fi",
            "Loopback Pseudo-Interface 1",
            "vEthernet (Default Switch)",
            "Adapter-2",
            "USB Network Bridge",
            "LOOPBACK Virtual Adapter",
            "蓝牙网络连接",
        ];
        let mut lowercase_micros = Vec::with_capacity(BATCHES);
        let mut direct_micros = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            let measure_lowercase = || {
                let started = Instant::now();
                let mut matches = 0usize;
                for _ in 0..SCANS_PER_BATCH {
                    for name in NAMES {
                        matches += std::hint::black_box(name)
                            .to_ascii_lowercase()
                            .contains("loopback") as usize;
                    }
                }
                std::hint::black_box(matches);
                started.elapsed().as_secs_f64() * 1_000_000.0 / SCANS_PER_BATCH as f64
            };
            let measure_direct = || {
                let started = Instant::now();
                let mut matches = 0usize;
                for _ in 0..SCANS_PER_BATCH {
                    for name in NAMES {
                        matches +=
                            super::contains_loopback_marker(std::hint::black_box(name)) as usize;
                    }
                }
                std::hint::black_box(matches);
                started.elapsed().as_secs_f64() * 1_000_000.0 / SCANS_PER_BATCH as f64
            };
            if batch % 2 == 0 {
                lowercase_micros.push(measure_lowercase());
                direct_micros.push(measure_direct());
            } else {
                direct_micros.push(measure_direct());
                lowercase_micros.push(measure_lowercase());
            }
        }
        lowercase_micros.sort_by(f64::total_cmp);
        direct_micros.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "network adapter name filter ({} names/scan): lowercase allocation median/p95={:.3}/{:.3} us/scan; direct ASCII search={:.3}/{:.3}",
            NAMES.len(),
            lowercase_micros[BATCHES / 2],
            lowercase_micros[p95_index],
            direct_micros[BATCHES / 2],
            direct_micros[p95_index],
        );
    }

    #[test]
    fn network_rate_uses_measurement_interval_and_handles_zero_elapsed() {
        assert_eq!(network_rate_kbps(1024, Duration::from_secs(2)), 0.5);
        assert_eq!(network_rate_kbps(1024, Duration::ZERO), 0.0);
        assert_eq!(
            process_io_sample_rate_kbps(1024, 1024, Duration::from_secs(30), 100, None, true),
            None,
            "first IO refresh establishes a baseline only"
        );
        assert_eq!(
            process_io_sample_rate_kbps(
                30 * 1024,
                30 * 1024,
                Duration::from_secs(30),
                100,
                Some(100),
                false,
            ),
            None,
            "baseline sample must be skipped"
        );
        assert_eq!(
            process_io_sample_rate_kbps(1024, 1024, Duration::from_secs(30), 101, Some(100), true),
            None,
            "PID reuse must establish a fresh baseline"
        );
        assert_eq!(
            process_io_sample_rate_kbps(
                30 * 1024,
                30 * 1024,
                Duration::from_secs(30),
                100,
                Some(100),
                true
            ),
            Some(2.0)
        );
        assert_eq!(
            process_io_rate_kbps(3 * 1024 * 1024, 1024 * 1024, Duration::from_secs(2)),
            2048.0
        );
        assert_eq!(
            process_io_rate_kbps(u64::MAX, u64::MAX, Duration::from_secs(1)),
            u64::MAX as f64 / 1024.0
        );
        assert_eq!(process_io_rate_kbps(100, 100, Duration::ZERO), 0.0);
    }

    #[test]
    fn process_io_baselines_reconcile_only_on_io_samples() {
        let original = HashMap::from([(42, 100_u64), (99, 200_u64)]);
        let mut baselines = original.clone();
        let mut current_pids: Option<HashSet<u32>> = None;

        assert_eq!(
            update_process_io_start_baseline(
                &mut baselines,
                current_pids.as_mut(),
                42,
                101,
                false,
                false,
            ),
            None
        );
        prune_process_io_start_baselines(&mut baselines, current_pids.as_ref());
        assert_eq!(
            baselines, original,
            "ordinary process refreshes do no I/O baseline work"
        );

        current_pids = Some(HashSet::new());
        assert_eq!(
            update_process_io_start_baseline(
                &mut baselines,
                current_pids.as_mut(),
                42,
                100,
                true,
                true,
            ),
            Some(100)
        );
        assert_eq!(current_pids.as_ref().unwrap(), &HashSet::from([42]));
        prune_process_io_start_baselines(&mut baselines, current_pids.as_ref());
        assert_eq!(
            baselines,
            HashMap::from([(42, 100)]),
            "exited PIDs are pruned on the next I/O sample"
        );

        current_pids = Some(HashSet::new());
        let previous_start = update_process_io_start_baseline(
            &mut baselines,
            current_pids.as_mut(),
            42,
            101,
            true,
            true,
        );
        assert_eq!(previous_start, Some(100));
        assert_eq!(
            process_io_sample_rate_kbps(
                30 * 1024,
                30 * 1024,
                Duration::from_secs(30),
                101,
                previous_start,
                true,
            ),
            None,
            "PID reuse still requires a fresh baseline"
        );
        assert_eq!(baselines.get(&42), Some(&101));
    }

    #[test]
    #[ignore = "manual synthetic process-I/O baseline bookkeeping benchmark; excludes sysinfo refresh"]
    fn benchmark_process_io_baseline_bookkeeping_by_io_cadence() {
        const PROCESS_REFRESHES_PER_MINUTE: usize = 12;
        const REPEATS: usize = 25;
        for process_count in [1_000usize, 5_000] {
            let entries = (0..process_count)
                .map(|index| (index as u32 + 100, index as u64 + 1))
                .collect::<Vec<_>>();
            let initial = entries.iter().copied().collect::<HashMap<_, _>>();

            let mut old_baselines = initial.clone();
            let old_started = Instant::now();
            let mut old_observed = 0usize;
            for _ in 0..REPEATS {
                for scan in 0..PROCESS_REFRESHES_PER_MINUTE {
                    let sampled = scan % 6 == 5;
                    let mut current_pids = HashSet::with_capacity(process_count);
                    for (pid, start_time) in &entries {
                        current_pids.insert(*pid);
                        let previous = old_baselines.get(pid).copied();
                        old_observed += usize::from(previous == Some(*start_time));
                        if sampled {
                            old_baselines.insert(*pid, *start_time);
                        }
                    }
                    old_baselines.retain(|pid, _| current_pids.contains(pid));
                }
            }
            let old_elapsed = old_started.elapsed();

            let mut new_baselines = initial;
            let new_started = Instant::now();
            let mut new_observed = 0usize;
            for _ in 0..REPEATS {
                for scan in 0..PROCESS_REFRESHES_PER_MINUTE {
                    let sampled = scan % 6 == 5;
                    let mut current_pids = sampled.then(|| HashSet::with_capacity(process_count));
                    for (pid, start_time) in &entries {
                        let previous = update_process_io_start_baseline(
                            &mut new_baselines,
                            current_pids.as_mut(),
                            *pid,
                            *start_time,
                            sampled,
                            sampled,
                        );
                        new_observed += usize::from(previous == Some(*start_time));
                    }
                    prune_process_io_start_baselines(&mut new_baselines, current_pids.as_ref());
                }
            }
            let new_elapsed = new_started.elapsed();

            assert_eq!(old_baselines, new_baselines);
            assert_eq!(
                old_observed,
                process_count * PROCESS_REFRESHES_PER_MINUTE * REPEATS
            );
            assert_eq!(
                new_observed,
                process_count * (PROCESS_REFRESHES_PER_MINUTE / 6) * REPEATS
            );
            println!(
                "{process_count} PIDs × {PROCESS_REFRESHES_PER_MINUTE} refreshes × {REPEATS} reps: old={old_elapsed:?}, IO-gated={new_elapsed:?}; bookkeeping only"
            );
        }
    }

    #[test]
    fn process_io_payload_preserves_freshness_without_a_pid_rate_cache() {
        assert_eq!(process_io_sample_payload(Some(12.5)), (12.5, true));
        assert_eq!(process_io_sample_payload(None), (0.0, false));
    }

    #[test]
    fn proc_sample_cache_reuse_matches_rebuild_across_reorder_growth_and_shrink() {
        let snapshots = [
            vec![
                proc_sample(
                    41,
                    Some(100),
                    "worker-long-name-initial.exe",
                    1.25,
                    100.25,
                    10.5,
                    false,
                ),
                proc_sample(17, Some(8), "程序-初始.exe", f32::NAN, f64::NAN, 0.0, true),
            ],
            vec![
                proc_sample(17, Some(8), "改名.exe", 2.5, 222.0, 30.0, false),
                proc_sample(
                    41,
                    Some(101),
                    "worker-long-name-initial.exe",
                    3.5,
                    333.0,
                    40.0,
                    true,
                ),
                proc_sample(99, None, "new-process.exe", 5.0, 99.0, 50.0, false),
            ],
            vec![
                proc_sample(99, Some(999), "new-process.exe", 6.0, 98.0, 60.0, true),
                proc_sample(41, Some(101), "renamed-程序.exe", -1.0, 77.0, 70.0, false),
                proc_sample(17, Some(8), "改名.exe", 0.0, 66.0, 80.0, true),
                proc_sample(7, Some(4), "grow.exe", 12.0, 7.0, 90.0, false),
            ],
            vec![proc_sample(
                7,
                None,
                "after-shrink.exe",
                0.5,
                6.0,
                0.0,
                true,
            )],
        ];
        let mut reused: Vec<crate::diag::ProcSample> = Vec::new();
        for (snapshot_index, snapshot) in snapshots.iter().enumerate() {
            let mut reference = Vec::new();
            rebuild_proc_sample_cache(&mut reference, snapshot);
            let prior_name_ptr = (snapshot_index == 1).then(|| reused[0].name.as_ptr());
            let prior_name_capacity = (snapshot_index == 1).then(|| reused[0].name.capacity());
            reuse_proc_sample_cache(&mut reused, snapshot);
            assert_proc_samples_equal(&reused, &reference);
            if let Some(name_ptr) = prior_name_ptr {
                assert_eq!(reused[0].name.as_ptr(), name_ptr);
                assert_eq!(reused[0].name.capacity(), prior_name_capacity.unwrap());
            }
        }
        assert_eq!(reused.len(), 1);
        assert_eq!(reused[0].pid, 7);
    }

    #[test]
    fn proc_sample_cache_preserves_owned_lossy_name_payloads() {
        let mut cache = vec![proc_sample(2, Some(1), "before.exe", 1.0, 2.0, 3.0, false)];
        update_proc_sample_entry(
            &mut cache,
            0,
            ProcSampleUpdate {
                pid: 9,
                start_time: None,
                name: std::borrow::Cow::Owned("lossy-程序.exe".to_string()),
                cpu: 4.0,
                mem_mb: 5.0,
                io_kbps: 6.0,
                io_sample_fresh: true,
            },
        );
        let expected = [proc_sample(9, None, "lossy-程序.exe", 4.0, 5.0, 6.0, true)];
        assert_proc_samples_equal(&cache, &expected);
    }

    #[test]
    #[ignore = "manual Release benchmark for ProcSample name-buffer reuse"]
    fn benchmark_proc_sample_name_buffer_reuse() {
        const BATCHES: usize = 101;
        fn generated_samples(count: usize, suffix: &str) -> Vec<crate::diag::ProcSample> {
            (1..=count)
                .map(|pid| {
                    proc_sample(
                        pid as u32,
                        Some((pid * 13) as u64),
                        &format!("process-{pid:04}-{suffix}.exe"),
                        (pid % 100) as f32,
                        ((pid * 37) % 500_000) as f64 / 1024.0,
                        ((pid * 11) % 10_000) as f64,
                        pid.is_multiple_of(3),
                    )
                })
                .collect()
        }
        fn measure_rebuild(
            cache: &mut Vec<crate::diag::ProcSample>,
            samples: &[crate::diag::ProcSample],
        ) -> u128 {
            let started = Instant::now();
            rebuild_proc_sample_cache(cache, samples);
            std::hint::black_box(cache.as_slice());
            started.elapsed().as_nanos()
        }
        fn measure_reuse(
            cache: &mut Vec<crate::diag::ProcSample>,
            samples: &[crate::diag::ProcSample],
        ) -> u128 {
            let started = Instant::now();
            reuse_proc_sample_cache(cache, samples);
            std::hint::black_box(cache.as_slice());
            started.elapsed().as_nanos()
        }

        for row_count in [500, 1000] {
            let steady = generated_samples(row_count, "baseline");
            let changed = generated_samples(row_count, "changed_");
            let mut reference_cache = Vec::with_capacity(row_count);
            let mut reused_cache = Vec::with_capacity(row_count);
            rebuild_proc_sample_cache(&mut reference_cache, &steady);
            reuse_proc_sample_cache(&mut reused_cache, &steady);
            assert_proc_samples_equal(&reused_cache, &reference_cache);

            for change_names in [false, true] {
                let mut reference_nanos = Vec::with_capacity(BATCHES);
                let mut reused_nanos = Vec::with_capacity(BATCHES);
                for warmup in 0..7 {
                    let samples = if change_names && warmup % 2 == 1 {
                        &changed
                    } else {
                        &steady
                    };
                    if warmup % 2 == 0 {
                        measure_rebuild(&mut reference_cache, samples);
                        measure_reuse(&mut reused_cache, samples);
                    } else {
                        measure_reuse(&mut reused_cache, samples);
                        measure_rebuild(&mut reference_cache, samples);
                    }
                    assert_proc_samples_equal(&reused_cache, &reference_cache);
                }
                for batch in 0..BATCHES {
                    let samples = if change_names && batch % 2 == 1 {
                        &changed
                    } else {
                        &steady
                    };
                    if batch % 2 == 0 {
                        reference_nanos.push(measure_rebuild(&mut reference_cache, samples));
                        reused_nanos.push(measure_reuse(&mut reused_cache, samples));
                    } else {
                        reused_nanos.push(measure_reuse(&mut reused_cache, samples));
                        reference_nanos.push(measure_rebuild(&mut reference_cache, samples));
                    }
                    assert_proc_samples_equal(&reused_cache, &reference_cache);
                }
                reference_nanos.sort_unstable();
                reused_nanos.sort_unstable();
                let p95_index = (BATCHES * 95).div_ceil(100) - 1;
                eprintln!(
                    "ProcSample refresh ({} rows, names {}): clear/rebuild median/p95={:.1}/{:.1} us; reused buffers={:.1}/{:.1} us",
                    row_count,
                    if change_names { "alternating" } else { "steady" },
                    reference_nanos[BATCHES / 2] as f64 / 1_000.0,
                    reference_nanos[p95_index] as f64 / 1_000.0,
                    reused_nanos[BATCHES / 2] as f64 / 1_000.0,
                    reused_nanos[p95_index] as f64 / 1_000.0,
                );
            }
        }
    }

    #[test]
    fn network_counter_reset_and_loopback_are_handled_without_spikes() {
        let mut previous = counters(&[
            ("Ethernet", 1000, 1000),
            ("Loopback Pseudo-Interface", 8000, 8000),
        ]);
        assert_eq!(
            network_delta(
                &counters(&[
                    ("Ethernet", 1200, 1300),
                    ("Loopback Pseudo-Interface", 9000, 9000)
                ]),
                &mut previous
            ),
            (200, 300)
        );
        assert_eq!(
            network_delta(&counters(&[("Ethernet", 5, 7)]), &mut previous),
            (0, 0)
        );
        assert_eq!(
            network_delta(&counters(&[("Ethernet", 25, 37)]), &mut previous),
            (20, 30)
        );
    }
}

fn merge_gpu(
    previous: Option<GpuInfo>,
    nvidia: Option<GpuInfo>,
    name: Option<String>,
    pdh: Option<crate::gpu::GpuPdh>,
) -> Option<GpuInfo> {
    // NVIDIA reports one device; PDH spans adapters and has no matching device identity.
    // Never attach aggregate load/memory (or registry capacity) to a named NVIDIA GPU.
    if let Some(info) = nvidia {
        return Some(info);
    }
    if let Some(sample) = pdh {
        return Some(GpuInfo {
            name: "GPU（全适配器聚合）".into(),
            utilization: sample
                .util_percent
                .is_finite()
                .then_some(sample.util_percent)
                .filter(|value| (0.0..=100.0).contains(value)),
            mem_used_mb: sample.dedicated_bytes.map(|bytes| bytes / 1024 / 1024),
            // The busiest 3D engine and summed dedicated usage are not one device.
            mem_total_mb: None,
            temperature: None,
        });
    }
    // Only retain a display name after failure; no stale sensor/capacity evidence.
    name.or_else(|| previous.map(|info| info.name))
        .map(|name| GpuInfo {
            name,
            ..Default::default()
        })
}

#[cfg(test)]
mod gpu_sample_tests {
    use super::*;

    fn nvidia() -> GpuInfo {
        parse_nvidia_sample("NVIDIA GPU, 12, 1024, 8192, 55").unwrap()
    }

    fn aggregate() -> crate::gpu::GpuPdh {
        crate::gpu::GpuPdh {
            util_percent: 89.0,
            dedicated_bytes: Some(4096 * 1024 * 1024),
        }
    }

    #[test]
    fn nvidia_device_is_never_overlaid_with_all_adapter_data() {
        let result = merge_gpu(None, Some(nvidia()), None, Some(aggregate())).unwrap();
        assert_eq!(result.name, "NVIDIA GPU");
        assert_eq!(result.utilization, Some(12.0));
        assert_eq!(result.mem_used_mb, Some(1024));
        assert_eq!(result.mem_total_mb, Some(8192));
        assert_eq!(result.temperature, Some(55.0));
    }

    #[test]
    fn aggregate_fallback_does_not_inherit_device_identity_or_capacity() {
        let result = merge_gpu(
            Some(nvidia()),
            None,
            Some("Intel GPU".into()),
            Some(aggregate()),
        )
        .unwrap();
        assert_eq!(result.name, "GPU（全适配器聚合）");
        assert_eq!(result.utilization, Some(89.0));
        assert_eq!(result.mem_used_mb, Some(4096));
        assert_eq!(result.mem_total_mb, None);
        assert_eq!(result.temperature, None);
        let failed = merge_gpu(Some(result), None, None, None).unwrap();
        assert_eq!(failed.utilization, None);
        assert_eq!(failed.mem_used_mb, None);
        assert_eq!(failed.mem_total_mb, None);
        assert_eq!(failed.temperature, None);
        let recovered = merge_gpu(Some(failed), Some(nvidia()), None, Some(aggregate())).unwrap();
        assert_eq!(recovered.name, "NVIDIA GPU");
        assert_eq!(recovered.temperature, Some(55.0));
    }

    #[test]
    fn failed_device_sample_and_name_only_probe_have_no_measurements() {
        let failed = merge_gpu(Some(nvidia()), None, None, None).unwrap();
        assert_eq!(failed.name, "NVIDIA GPU");
        assert_eq!(failed.mem_total_mb, None);
        assert_eq!(failed.temperature, None);
        assert_eq!(failed.utilization, None);
        assert_eq!(failed.mem_used_mb, None);
        assert!(merge_gpu(None, None, None, None).is_none());
        let name_only = merge_gpu(None, None, Some("Intel GPU".into()), None).unwrap();
        assert_eq!(name_only.name, "Intel GPU");
        assert_eq!(name_only.utilization, None);
    }

    #[test]
    fn unsupported_nvidia_fields_remain_unknown_without_aggregate_backfill() {
        let device = parse_nvidia_sample("GPU, N/A, [Not Supported], 8192, N/A").unwrap();
        let result = merge_gpu(None, Some(device), None, Some(aggregate())).unwrap();
        assert_eq!(result.utilization, None);
        assert_eq!(result.mem_used_mb, None);
        assert_eq!(result.temperature, None);
        assert_eq!(result.mem_total_mb, Some(8192));
    }

    #[test]
    fn nvidia_parser_rejects_invalid_values_and_keeps_rows_separate() {
        for text in ["GPU, NaN, 9000, 8192, inf", "GPU, 101, 9000, 8192, -1"] {
            let result = parse_nvidia_sample(text).unwrap();
            assert_eq!(result.utilization, None);
            assert_eq!(result.mem_used_mb, None);
            assert_eq!(result.temperature, None);
        }
        let result = parse_nvidia_sample("GPU, 0, 0, 0, 0\nOther GPU, 100, 5, 10, 90").unwrap();
        assert_eq!(result.name, "GPU");
        assert_eq!(result.utilization, Some(0.0));
        assert_eq!(result.mem_used_mb, Some(0));
        assert_eq!(result.mem_total_mb, None);
        assert_eq!(result.temperature, Some(0.0));
        for invalid in ["", "GPU, 1, 2", ", 0, 0, 1, 2", "GPU, 1, 2, 3, 4, 5"] {
            assert!(parse_nvidia_sample(invalid).is_none());
        }
    }
}

fn read_battery() -> Result<Option<BatteryInfo>, u32> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    unsafe {
        let mut status: SYSTEM_POWER_STATUS = std::mem::zeroed();
        if GetSystemPowerStatus(&mut status) == 0 {
            return Err(windows_sys::Win32::Foundation::GetLastError());
        }
        Ok(battery_info_from_status(
            status.ACLineStatus,
            status.BatteryFlag,
            status.BatteryLifePercent,
            status.BatteryLifeTime,
        ))
    }
}

fn query_nvidia() -> Option<GpuInfo> {
    let out = crate::util::run_with_timeout(
        "nvidia-smi",
        &[
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu",
            "--format=csv,noheader,nounits",
        ],
        Duration::from_secs(5),
    )?;
    parse_nvidia_sample(&out)
}

fn parse_nvidia_sample(output: &str) -> Option<GpuInfo> {
    let line = output.lines().find(|line| !line.trim().is_empty())?;
    let parts: Vec<&str> = line.split(',').map(str::trim).collect();
    if parts.len() != 5 || parts[0].is_empty() {
        return None;
    }
    let parse = |text: &str, max: f32| {
        text.parse::<f32>()
            .ok()
            .filter(|value| value.is_finite() && (0.0..=max).contains(value))
    };
    let mem_total_mb = parts[3].parse::<u64>().ok().filter(|total| *total > 0);
    let mem_used_mb = parts[2]
        .parse::<u64>()
        .ok()
        .filter(|used| mem_total_mb.is_none_or(|total| *used <= total));
    Some(GpuInfo {
        name: parts[0].to_string(),
        utilization: parse(parts[1], 100.0),
        mem_used_mb,
        mem_total_mb,
        temperature: parse(parts[4], 150.0),
    })
}

/// 无 nvidia-smi 时，退而求其次拿显卡名称（仅名称，无利用率）。
fn query_gpu_name_ps() -> Option<GpuInfo> {
    let out = crate::util::run_ps(
        "(Get-CimInstance Win32_VideoController | Select-Object -First 1 -ExpandProperty Name)",
    )?;
    let name = out.trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(GpuInfo {
            name,
            ..Default::default()
        })
    }
}

/// 监控轮询线程：1Hz 刷新，写入缓存并向前端推送 `stats` 事件；
/// 同时喂入诊断引擎（EWMA/评分）与自动模式状态机。
pub fn start(
    app: tauri::AppHandle,
    cache: Arc<Cache>,
    diag: Arc<Mutex<Engine>>,
    auto: Arc<Mutex<AutoState>>,
) {
    std::thread::spawn(move || {
        use tauri::{Emitter, Manager};
        #[cfg(windows)]
        let _effective_power_mode_registration = register_effective_power_mode_notifications();
        // Start empty: the monitor needs CPU, memory and process snapshots only;
        // loading unrelated refresh categories wastes startup work and retains state.
        let mut sys = System::new();
        let mut networks = Networks::new_with_refreshed_list();
        // Disk identity/filesystem is static; refresh capacity/free space only.
        let disk_refresh = DiskRefreshKind::nothing().with_storage();
        let mut disks = Disks::new_with_refreshed_list_specifics(disk_refresh);
        // Slow PDH/WMI/driver queries must never block CPU, memory and disk updates.
        let gpu_cache = Arc::new(RwLock::new(None::<GpuInfo>));
        let gpu_worker_cache = gpu_cache.clone();
        std::thread::spawn(move || {
            let mut cached: Option<GpuInfo> = None;
            let mut next_name_probe = std::time::Instant::now();
            loop {
                let started = std::time::Instant::now();
                let pdh_gpu = crate::gpu::poll();
                // Prefer one coherent NVIDIA-device sample; PDH is aggregate-only fallback.
                // WMI name discovery is static: retry only once per minute when absent.
                let nv = query_nvidia();
                let name = if cached.is_none() && nv.is_none() && started >= next_name_probe {
                    next_name_probe = started + Duration::from_secs(60);
                    query_gpu_name_ps().map(|g| g.name)
                } else {
                    None
                };
                cached = merge_gpu(cached.take(), nv, name, pdh_gpu);
                *gpu_worker_cache.write().unwrap() = cached.clone();
                // Account for query cost instead of adding it to the desired period.
                std::thread::sleep(Duration::from_secs(3).saturating_sub(started.elapsed()));
            }
        });
        let mut tick: u64 = 0;
        let mut last_startup_count: usize = 0;
        let mut last_tray_band: &'static str = "";
        let mut cpu_frequency_cache = CpuFrequencyCache::default();
        let mut epp_supported_cache: Option<(std::time::Instant, bool)> = None;
        let mut disk_cache: Vec<DiskInfo> = Vec::new();
        let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        let mut battery_cache: Option<(std::time::Instant, Result<Option<BatteryInfo>, u32>)> =
            None;
        let mut battery_error_reported = false;
        let mut disk_cache_refresh = std::time::Instant::now() - Duration::from_secs(10);
        let mut network_totals_cache: HashMap<String, (u64, u64)> = HashMap::new();
        let mut timezone_identity_cache = local_timezone_identity();
        let mut last_timezone_identity_refresh = std::time::Instant::now();
        let mut network_sample_at = std::time::Instant::now() - Duration::from_secs(3);
        let mut memory_cache: Option<(std::time::Instant, (f64, f64, f64, f64))> = None;
        let mut last_network_day_persist = std::time::Instant::now() - Duration::from_secs(60);
        let mut last_io_process_starts: HashMap<u32, u64> = HashMap::new();
        let mut proc_io_baseline_established = false;
        let mut proc_sample_cache: Vec<crate::diag::ProcSample> = Vec::new();
        let mut proc_creation_filetime_cache: HashMap<u32, u64> = HashMap::new();
        let mut process_image_query_buffer: Vec<u16> = Vec::new();
        let mut proc_sample_cpu_count = 1usize;
        let mut process_count = 0usize;
        let mut process_scan_ms = 0.0f64;
        let mut last_process_refresh = std::time::Instant::now() - Duration::from_secs(10);
        let mut last_diag_refresh: Option<std::time::Instant> = None;
        let mut last_io_refresh = std::time::Instant::now() - Duration::from_secs(60);
        let mut proc_io_pending_sample = false;
        let mut proc_io_interval = Duration::from_secs(30);
        // 常驻模式学习：低频观测（默认每 30 分钟用最新进程快照累积一次）。
        let mut next_residency_observe = std::time::Instant::now()
            + Duration::from_secs(crate::residency::RESIDENCY_OBSERVE_INTERVAL_SECS);

        let process_refresh = ProcessRefreshKind::nothing()
            .with_cpu()
            .with_memory()
            .with_exe(UpdateKind::OnlyIfNotSet);
        let process_io_refresh = process_refresh.with_disk_usage();
        tracing::info!("监控采样线程已启动");

        let mut last_instant = std::time::Instant::now();
        loop {
            // 主窗口隐藏（托盘驻留）时降频采样，降低自身功耗
            let visible = app
                .get_webview_window("main")
                .map(|w| w.is_visible().unwrap_or(true))
                .unwrap_or(true);
            let interval = if visible { 1000u64 } else { 2000 };
            // Target a start-to-start cadence; processing time belongs to the period.
            std::thread::sleep(
                Duration::from_millis(interval).saturating_sub(last_instant.elapsed()),
            );
            let dt_secs = last_instant.elapsed().as_secs_f64().max(0.5);
            last_instant = std::time::Instant::now();
            tick += 1;
            *cache.tick.lock().unwrap() = tick;

            if tick == 1 {
                tracing::info!("监控首次采样：刷新系统传感器");
            }
            // CPU utilization and frequency are separate sysinfo refresh kinds;
            // usage-only refresh leaves Cpu::frequency() at its zero-initialized value.
            sys.refresh_cpu_usage();
            if cpu_frequency_cache.refresh_due(last_instant) {
                sys.refresh_cpu_frequency();
                let cpus = sys.cpus();
                if !cpu_frequency_cache.refresh_iter(
                    last_instant,
                    cpus.iter().map(|cpu| cpu.frequency()),
                    cpus.len(),
                ) {
                    tracing::warn!(
                        cpu_count = cpus.len(),
                        "CPU frequency query returned no per-core readings"
                    );
                }
            }
            let memory_sample_fresh = memory_sample_refresh_due(
                memory_cache.as_ref().map(|(refreshed, _)| *refreshed),
                last_instant,
            );
            let memory_refresh_ms = if memory_sample_fresh {
                let refresh_started = Instant::now();
                sys.refresh_memory_specifics(MemoryRefreshKind::nothing().with_ram().with_swap());
                memory_cache = Some((
                    last_instant,
                    (
                        sys.used_memory() as f64 / 1024.0 / 1024.0 / 1024.0,
                        sys.total_memory() as f64 / 1024.0 / 1024.0 / 1024.0,
                        sys.used_swap() as f64 / 1024.0 / 1024.0 / 1024.0,
                        sys.total_swap() as f64 / 1024.0 / 1024.0 / 1024.0,
                    ),
                ));
                Some(refresh_started.elapsed().as_secs_f64() * 1000.0)
            } else {
                None
            };
            let memory = memory_cache.map(|(_, value)| value).unwrap_or_default();
            if battery_cache
                .as_ref()
                .map(|(refreshed, _)| {
                    refreshed.elapsed() >= Duration::from_secs(battery_poll_interval_seconds())
                })
                .unwrap_or(true)
            {
                battery_cache = Some((std::time::Instant::now(), read_battery()));
            }
            let battery_status_ok =
                battery_query_succeeded(battery_cache.as_ref().map(|(_, result)| result));
            let battery = battery_cache.as_ref().and_then(|(_, result)| match result {
                Ok(value) => {
                    battery_error_reported = false;
                    value.clone()
                }
                Err(error) => {
                    if !battery_error_reported {
                        tracing::warn!(code = error, "battery interface query failed");
                        battery_error_reported = true;
                    }
                    None
                }
            });
            // Process refresh is the most expensive system-wide scan; update less often than sensor samples.
            let refresh_processes_due =
                last_process_refresh.elapsed() >= process_refresh_interval(visible);
            if refresh_processes_due {
                let refresh_io = last_io_refresh.elapsed() >= Duration::from_secs(30);
                let kind = if refresh_io {
                    process_io_refresh
                } else {
                    process_refresh
                };
                let scan_started = std::time::Instant::now();
                sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
                process_scan_ms = scan_started.elapsed().as_secs_f64() * 1000.0;
                last_process_refresh = std::time::Instant::now();
                if refresh_io {
                    proc_io_interval = std::time::Instant::now()
                        .duration_since(last_io_refresh)
                        .max(Duration::from_millis(1));
                    last_io_refresh = std::time::Instant::now();
                    proc_io_pending_sample = true;
                }
                process_count = sys.processes().len();
            }
            let network_started_at = network_sample_at;
            let network_elapsed = network_started_at.elapsed();
            let refresh_networks = network_elapsed >= Duration::from_secs(1);
            if refresh_networks {
                let network_started = std::time::Instant::now();
                networks.refresh(true);
                network_totals_cache.clear();
                network_totals_cache.reserve(networks.iter().count());
                for (name, data) in networks.iter() {
                    network_totals_cache.insert(
                        name.clone(),
                        (data.total_received(), data.total_transmitted()),
                    );
                }
                if tick == 1 || tick.is_multiple_of(60) {
                    tracing::info!(
                        network_scan_ms = network_started.elapsed().as_secs_f64() * 1000.0,
                        "网络计数器采样完成"
                    );
                }
            }
            let (rx_bytes, tx_bytes) = if refresh_networks {
                let mut previous = cache.net_totals.lock().unwrap();
                network_delta(&network_totals_cache, &mut previous)
            } else {
                (0, 0)
            };
            let rate_interval = if refresh_networks {
                let interval = network_elapsed;
                network_sample_at = std::time::Instant::now();
                interval
            } else {
                Duration::from_secs(1)
            }
            .max(Duration::from_millis(1));
            if disk_cache_refresh.elapsed() >= Duration::from_secs(5) {
                let disk_started = std::time::Instant::now();
                disks.refresh_specifics(true, disk_refresh);
                disk_cache = collect_disks(&disks);
                disk_cache_refresh = std::time::Instant::now();
                tracing::debug!(
                    disk_scan_ms = disk_started.elapsed().as_secs_f64() * 1000.0,
                    "磁盘容量采样完成"
                );
            }

            let cpu = sys.global_cpu_usage();
            if tick == 1 && (sys.cpus().is_empty() || !cpu.is_finite()) {
                tracing::error!(
                    cpu_count = sys.cpus().len(),
                    cpu_usage = cpu,
                    "CPU usage telemetry initialization failed"
                );
            }
            let per_core: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
            let freq = cpu_frequency_cache.average_mhz;
            if tick == 1 || tick.is_multiple_of(60) {
                tracing::info!(
                    cpu_frequency_mhz = freq,
                    cpu_cores = sys.cpus().len(),
                    sample_id = tick,
                    "CPU frequency telemetry status"
                );
            }

            // ---- AeroTune 观测到的本地自然日流量；一分钟原子落盘一次 ----
            let timezone_check_now = std::time::Instant::now();
            if timezone_identity_refresh_due(last_timezone_identity_refresh, timezone_check_now) {
                timezone_identity_cache = local_timezone_identity();
                last_timezone_identity_refresh = timezone_check_now;
            }
            let day_snapshot = {
                let today = chrono::Local::now().date_naive().to_string();
                let mut day = cache.net_day.lock().unwrap();
                let timezone_changed =
                    day.time_zone_id.as_deref() != timezone_identity_cache.as_deref();
                if day.local_date != today || timezone_changed {
                    day.local_date = today;
                    day.time_zone_id = timezone_identity_cache.clone();
                    day.rx_bytes = 0;
                    day.tx_bytes = 0;
                }
                day.rx_bytes = day.rx_bytes.saturating_add(rx_bytes);
                day.tx_bytes = day.tx_bytes.saturating_add(tx_bytes);
                day.clone()
            };
            if last_network_day_persist.elapsed() >= Duration::from_secs(60) {
                match persist_network_day(&day_snapshot) {
                    Ok(()) => last_network_day_persist = std::time::Instant::now(),
                    Err(error) => tracing::warn!(%error, "今日网络累计落盘失败；后续采样将重试"),
                }
            }
            let (today_rx_bytes, today_tx_bytes) = (day_snapshot.rx_bytes, day_snapshot.tx_bytes);

            // GPU is independently sampled: a blocked driver query cannot freeze all sensors.
            let stats = publish_stats_snapshot(
                &cache.stats,
                Stats {
                    sample_id: tick,
                    cpu,
                    per_core,
                    cpu_freq_mhz: freq,
                    mem_used_gb: memory.0,
                    mem_total_gb: memory.1,
                    swap_used_gb: memory.2,
                    swap_total_gb: memory.3,
                    system_drive: system_drive.clone(),
                    disks: disk_cache.clone(),
                    net_rx_kbps: network_rate_kbps(rx_bytes, rate_interval),
                    net_tx_kbps: network_rate_kbps(tx_bytes, rate_interval),
                    net_today_rx_gb: today_rx_bytes as f64 / 1073741824.0,
                    net_today_tx_gb: today_tx_bytes as f64 / 1073741824.0,
                    gpu: gpu_cache.read().unwrap().clone(),
                    battery,
                    battery_status_ok,
                    effective_power_mode: current_effective_power_mode(),
                    uptime_secs: System::uptime(),
                    process_count,
                },
            );

            LATEST_SAMPLE_ID.store(tick, std::sync::atomic::Ordering::Release);
            if tick == 1 || tick.is_multiple_of(60) {
                let mem_used_pct = memory_usage_pct(stats.mem_used_gb, stats.mem_total_gb);
                let aerotune_working_set_mb = sys
                    .processes()
                    .get(&sysinfo::Pid::from_u32(std::process::id()))
                    .map(|process| process.memory() as f64 / 1024.0 / 1024.0)
                    .unwrap_or(0.0);
                tracing::info!(
                    tick,
                    cpu = stats.cpu,
                    mem_used_gb = stats.mem_used_gb,
                    mem_total_gb = stats.mem_total_gb,
                    mem_used_pct = ?mem_used_pct,
                    aerotune_working_set_mb,
                    processes = stats.process_count,
                    memory_sample_fresh,
                    memory_refresh_ms = ?memory_refresh_ms,
                    process_scan_ms,
                    "监控采样已更新"
                );
            }
            if let Err(error) = app.emit("stats", stats.as_ref()) {
                if tick == 1 || tick.is_multiple_of(60) {
                    tracing::warn!(%error, "监控事件推送失败；前端可使用轮询兜底");
                }
            }

            // ---- 喂入诊断引擎（EWMA 异常检测 / 内存斜率 / 健康评分）----
            if tick % 30 == 1 {
                last_startup_count = crate::startup::count_startup_items();
            }
            let mut io_sample_fresh = false;
            if refresh_processes_due {
                let diag_scan_started = std::time::Instant::now();
                let process_count = sys.processes().len();
                let capture_process_identities = {
                    let state = auto.lock().unwrap_or_else(|error| error.into_inner());
                    let has_enabled_rule = state.settings.rules.iter().any(|rule| rule.enabled);
                    state
                        .settings
                        .rules
                        .iter()
                        .any(|rule| rule.enabled && rule.action == "deprio")
                        || (has_enabled_rule
                            && state.settings.autopilot
                            && state.settings.auto_dispose_anomalies)
                };
                proc_creation_filetime_cache.clear();
                if capture_process_identities {
                    proc_creation_filetime_cache.reserve(process_count);
                }
                let proc_io_sampled_this_tick = proc_io_pending_sample;
                let proc_io_delta_valid = proc_io_sampled_this_tick && proc_io_baseline_established;
                io_sample_fresh = proc_io_delta_valid;
                if proc_io_sampled_this_tick {
                    proc_io_pending_sample = false;
                    proc_io_baseline_established = true;
                }
                let mut current_pids = proc_io_sampled_this_tick
                    .then(|| std::collections::HashSet::with_capacity(process_count));
                let additional_sample_capacity =
                    process_count.saturating_sub(proc_sample_cache.len());
                proc_sample_cache.reserve(additional_sample_capacity);
                let cpu_count = sys.cpus().len();
                proc_sample_cpu_count = cpu_count.max(1);
                let mut sample_index = 0;
                for (pid, process) in sys.processes() {
                    let pid = pid.as_u32();
                    let started_at = process.start_time();
                    let previous_started_at = update_process_io_start_baseline(
                        &mut last_io_process_starts,
                        current_pids.as_mut(),
                        pid,
                        started_at,
                        proc_io_sampled_this_tick,
                        proc_io_delta_valid,
                    );
                    let has_same_process_baseline = previous_started_at == Some(started_at);
                    let io_rate = if proc_io_delta_valid && has_same_process_baseline {
                        let usage = process.disk_usage();
                        process_io_sample_rate_kbps(
                            usage.read_bytes,
                            usage.written_bytes,
                            proc_io_interval,
                            started_at,
                            previous_started_at,
                            proc_io_baseline_established,
                        )
                    } else {
                        None
                    };
                    let (io_kbps, io_sample_fresh) = process_io_sample_payload(io_rate);
                    let name = process.name().to_string_lossy();
                    if capture_process_identities {
                        if let Some(executable) = process.exe() {
                            let expected_path = executable.to_string_lossy();
                            if let Some(creation_filetime) =
                                crate::processes::process_creation_filetime_for_sample(
                                    pid,
                                    started_at,
                                    name.as_ref(),
                                    expected_path.as_ref(),
                                    &mut process_image_query_buffer,
                                )
                            {
                                proc_creation_filetime_cache.insert(pid, creation_filetime);
                            }
                        }
                    }
                    update_proc_sample_entry(
                        &mut proc_sample_cache,
                        sample_index,
                        ProcSampleUpdate {
                            pid,
                            start_time: Some(started_at),
                            name,
                            cpu: crate::processes::normalize_sysinfo_cpu_usage(
                                process.cpu_usage(),
                                cpu_count,
                            ),
                            mem_mb: process.memory() as f64 / 1024.0 / 1024.0,
                            io_kbps,
                            io_sample_fresh,
                        },
                    );
                    sample_index += 1;
                }
                proc_sample_cache.truncate(sample_index);
                // PID-set construction and stale-baseline pruning only run on fresh I/O samples.
                prune_process_io_start_baselines(
                    &mut last_io_process_starts,
                    current_pids.as_ref(),
                );
                let diagnostic_process_scan_ms = diag_scan_started.elapsed().as_secs_f64() * 1000.0;
                process_scan_ms += diagnostic_process_scan_ms;
                tracing::debug!(
                    elapsed_ms = diagnostic_process_scan_ms,
                    process_count = proc_sample_cache.len(),
                    "诊断进程采样完成"
                );
            }
            // Reuse the most recent process snapshot on intervening 1 Hz samples.
            let c_used_pct = stats
                .disks
                .iter()
                .find(|d| d.mount.starts_with('C'))
                .map(|d| {
                    if d.total_gb > 0.0 {
                        d.used_gb / d.total_gb * 100.0
                    } else {
                        0.0
                    }
                })
                .unwrap_or(0.0);
            let mem_pct =
                memory_usage_pct(stats.mem_used_gb, stats.mem_total_gb).unwrap_or(f32::NAN);
            let (cpu_core_peak_pct, cpu_saturated_cores, cpu_logical_cores) =
                cpu_core_load_summary(&stats.per_core);
            // System sensors are refreshed every loop; do not hold CPU/RAM back to the
            // slower process-inventory cadence. There is no prior diagnostics sample at startup,
            // so seed the first observation with one normal tick rather than a fabricated gap.
            let diag_sample_at = std::time::Instant::now();
            let diag_elapsed = diagnostics_sample_elapsed(
                last_diag_refresh,
                diag_sample_at,
                Duration::from_millis(interval),
            );
            let process_sample = (stats.cpu, mem_pct, stats.swap_used_gb, stats.uptime_secs);
            last_diag_refresh = Some(diag_sample_at);
            // Observe the sample and derive the controller snapshot under one lock.
            // Re-locking this non-reentrant mutex in the same iteration deadlocked
            // the sampler after the first successful sample.
            let (anomalies, telemetry) = {
                let mut d = diag.lock().unwrap_or_else(|e| e.into_inner());
                d.note_wallclock(chrono::Utc::now().timestamp().max(0) as u64);
                d.set_process_cpu_count(proc_sample_cpu_count);
                if refresh_processes_due {
                    d.set_process_creation_filetimes(&proc_creation_filetime_cache);
                }
                d.observe_elapsed_with_memory_sample_freshness(
                    diag_elapsed,
                    process_sample.0,
                    process_sample.1,
                    &proc_sample_cache,
                    last_startup_count,
                    process_sample.2,
                    c_used_pct,
                    process_sample.3,
                    refresh_processes_due,
                    io_sample_fresh,
                    memory_sample_fresh,
                );
                (
                    d.current.anomalies.clone(),
                    crate::auto::AutoTelemetry {
                        cpu_avg_60s: d.cpu_avg(60),
                        cpu_avg_10s: d.cpu_avg(10),
                        cpu_peak: d.cpu_peak(10),
                        cpu_forecast_10s_pct: d.current.cpu_forecast_10s_pct,
                        cpu_trend_pct_per_sec: d.current.cpu_trend_pct_per_sec,
                        cpu_forecast_stability: d.current.cpu_forecast_stability,
                        cpu_forecast_ready: d.current.cpu_forecast_ready,
                        cpu_core_peak_pct,
                        cpu_saturated_cores,
                        cpu_logical_cores,
                        mem_pct: d.current.mem_pct,
                        mem_slope: d.current.mem_slope,
                        process_count: stats.process_count,
                    },
                )
            };

            // ---- 常驻模式学习：低频观测（30 分钟一次，用最新进程快照） ----
            if std::time::Instant::now() >= next_residency_observe {
                if !proc_sample_cache.is_empty() {
                    crate::residency::observe_from_sampler(&proc_sample_cache);
                }
                next_residency_observe = std::time::Instant::now()
                    + Duration::from_secs(crate::residency::RESIDENCY_OBSERVE_INTERVAL_SECS);
            }

            // ---- 自动模式状态机 + 处置规则引擎 + 自适应控制器 ----
            let mode = crate::tweaks::current_mode();
            let ac_online = stats.battery.as_ref().map(|b| b.ac_online).unwrap_or(true);
            // EPP support is static registry metadata; avoid opening the same key every second.
            let epp_ok = epp_supported_cache
                .filter(|(checked, _)| checked.elapsed() < Duration::from_secs(60))
                .map(|(_, supported)| supported)
                .unwrap_or_else(|| {
                    let supported = crate::power::epp_supported();
                    epp_supported_cache = Some((std::time::Instant::now(), supported));
                    supported
                });
            let auto_tick_started = std::time::Instant::now();
            let (actions, memory_relief_enabled) = {
                let mut st = auto.lock().unwrap();
                let enabled = st.settings.cpu_scheduler;
                let actions = crate::auto::tick(
                    &mut st,
                    &sys,
                    &mode,
                    ac_online,
                    &anomalies,
                    epp_ok,
                    telemetry,
                    Duration::from_secs_f64(dt_secs),
                );
                (actions, enabled)
            };
            let auto_tick_elapsed = auto_tick_started.elapsed();
            if auto_tick_elapsed >= Duration::from_millis(250) {
                tracing::warn!(
                    elapsed_ms = auto_tick_elapsed.as_millis(),
                    "自动控制器处理偏慢（包含系统操作与 AutoState 锁持有时间）"
                );
            }
            let mem_used_pct = memory_usage_pct(stats.mem_used_gb, stats.mem_total_gb);
            if memory_relief_enabled
                && tick.is_multiple_of(60)
                && mem_used_pct.is_some_and(|percent| percent >= 88.0)
            {
                if let Err(error) = std::thread::Builder::new()
                    .name("aerotune-memory-relief".into())
                    .spawn(
                        || match crate::processes::auto_trim_background_memory_if_needed() {
                            Ok(Some(report)) if report.processed > 0 => tracing::info!(
                                processed = report.processed,
                                estimated_reclaimed_mb = report.estimated_reclaimed_mb,
                                available_before_mb = report.available_before_mb,
                                available_after_mb = ?report.available_after_mb,
                                "高内存压力下轻度回收后台进程工作集"
                            ),
                            Ok(_) => {}
                            Err(error) => tracing::warn!(%error, "自动内存缓解任务失败"),
                        },
                    )
                {
                    tracing::warn!(%error, "无法启动自动内存缓解线程");
                }
            }
            if !actions.is_empty() {
                let _ = app.emit("auto_actions", &actions);
                // 主窗口隐藏（托盘驻留）时走 Windows 原生通知
                let visible = app
                    .get_webview_window("main")
                    .map(|w| w.is_visible().unwrap_or(false))
                    .unwrap_or(false);
                if !visible {
                    use tauri_plugin_notification::NotificationExt;
                    let _ = app
                        .notification()
                        .builder()
                        .title("AeroTune 智能引擎")
                        .body(actions.join("\n"))
                        .show();
                }
            }

            // ---- 托盘动态图标与提示（每 5 秒）----
            if tick.is_multiple_of(5) {
                let score = diag.lock().unwrap().current.score;
                update_tray(&app, score, &mode, &mut last_tray_band);
            }
        }
    });
}

fn update_tray(app: &tauri::AppHandle, score: u8, mode: &str, last_band: &mut &'static str) {
    let Some(tray) = app.tray_by_id("main-tray") else {
        return;
    };
    let band: &'static str = if score >= 85 {
        "ok"
    } else if score >= 65 {
        "warn"
    } else {
        "bad"
    };
    if band != *last_band {
        *last_band = band;
        let bytes: &[u8] = match band {
            "ok" => include_bytes!("../icons/tray-ok.png"),
            "warn" => include_bytes!("../icons/tray-warn.png"),
            _ => include_bytes!("../icons/tray-bad.png"),
        };
        if let Ok(img) = tauri::image::Image::from_bytes(bytes) {
            let _ = tray.set_icon(Some(img));
        }
    }
    let mode_cn = match mode {
        "game" => "游戏",
        "power" => "省电",
        "dev" => "编码",
        _ => "无",
    };
    let _ = tray.set_tooltip(Some(&format!(
        "AeroTune · 健康 {score} 分\n模式：{mode_cn}"
    )));
}

#[tauri::command]
pub fn get_stats(cache: tauri::State<'_, Arc<Cache>>) -> Stats {
    owned_stats_snapshot(&cache)
}

#[tauri::command]
pub fn get_diagnostics(diag: tauri::State<'_, Arc<Mutex<Engine>>>) -> crate::diag::Diagnostics {
    diag.lock().unwrap().current.clone()
}
