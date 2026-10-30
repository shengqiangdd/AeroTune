// 智能诊断引擎：多维度滚动 median/MAD 异常检测 / 内存压力回归 / 系统健康评分
//
// - 每个进程的 CPU、内存、磁盘 IO 维护 31 点固定窗口；以 median 为中心、1.4826×MAD 为稳健尺度，
//   中位数中心对少数离群点稳健。持续超过中心+3×尺度+维度阈值才候选，CPU/内存/IO 分别连续 4/5/6 点确认。
// - 内存占用率用近 45 秒有效观测的 Theil–Sen pairwise 中位斜率预测趋势，抵抗短时尖峰并保留桶间隔。
// - 健康评分 = 100 − Σ 多维加权扣分（CPU 持续负载 / 内存水位 / 交换分区 / C 盘 / 进程数 / 开机时长 / 启动项）。
use serde::Serialize;
use std::collections::HashMap;

const HIST_CAP: usize = 180;
const ROBUST_WINDOW: usize = 31;
const CPU_FORECAST_WINDOW: usize = 15;
const CPU_FORECAST_MIN_SAMPLES: usize = 10;
const CPU_FORECAST_MIN_SPAN_SECS: f32 = 5.0;
const CPU_FORECAST_HORIZON_SECS: f32 = 10.0;
const CPU_FORECAST_MAX_SLOPES: usize = CPU_FORECAST_WINDOW * (CPU_FORECAST_WINDOW - 1) / 2;
const MAD_TO_SIGMA: f32 = 1.4826;
const SCALE_FLOOR_RATIO: f32 = 0.1;
const MEMORY_SLOPE_WINDOW: usize = 45;
const MEMORY_SLOPE_MIN_SAMPLES: usize = 10;
const MEMORY_SLOPE_MAX_PAIRS: usize = MEMORY_SLOPE_WINDOW * (MEMORY_SLOPE_WINDOW - 1) / 2;
pub const MEMORY_RISING_SLOPE_THRESHOLD: f32 = 0.05;
pub const MEMORY_PRESSURE_ELEVATED_PCT: f32 = 78.0;
pub const MEMORY_CRITICAL_PCT: f32 = 90.0;
pub const MEMORY_FORECAST_MIN_SAMPLES: usize = 20;
const MEMORY_FORECAST_MAX_SECONDS: u32 = 180;
const MEMORY_TREND_WARMUP_BUCKETS: usize = 15;
const PID_TRACK_MIN_SHRINK_CAPACITY: usize = 1024;
const PID_TRACK_SHRINK_RATIO: usize = 4;

fn valid_memory_pct(value: f32) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

fn valid_memory_observation_count(hist: &[f32], observations: &[bool], n: usize) -> usize {
    let len = n
        .min(MEMORY_SLOPE_WINDOW)
        .min(hist.len())
        .min(observations.len());
    hist[hist.len() - len..]
        .iter()
        .zip(&observations[observations.len() - len..])
        .filter(|(value, is_observation)| **is_observation && valid_memory_pct(**value))
        .count()
}

fn memory_critical_eta_secs(mem_pct: f32, slope: f32, sample_count: usize) -> Option<u32> {
    if sample_count < MEMORY_FORECAST_MIN_SAMPLES
        || !valid_memory_pct(mem_pct)
        || !(MEMORY_PRESSURE_ELEVATED_PCT..MEMORY_CRITICAL_PCT).contains(&mem_pct)
        || !slope.is_finite()
        || slope <= MEMORY_RISING_SLOPE_THRESHOLD
    {
        return None;
    }

    let eta_secs = ((MEMORY_CRITICAL_PCT - mem_pct) / slope).ceil();
    (eta_secs >= 1.0 && eta_secs <= MEMORY_FORECAST_MAX_SECONDS as f32).then_some(eta_secs as u32)
}

#[derive(Serialize, Clone, Debug)]
pub struct Anomaly {
    pub pid: u32,
    pub name: String,
    /// cpu | mem | io
    pub kind: String,
    pub value: f64,
    pub baseline: f64,
    /// Deviation in robust-scale units; it is not a probability.
    pub robust_z: f64,
    /// Wall-clock time when the confirming hot streak began; used only for
    /// local onset correlation and stays `None` while the wall clock is unknown.
    pub onset_unix: Option<u64>,
    /// Internal sampled FILETIME for safe automatic process actions; never serialized to the renderer.
    #[serde(skip_serializing)]
    pub(crate) creation_filetime: Option<u64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct Deduction {
    pub item: String,
    pub cost: u8,
}

/// 监控线程每秒提供的进程采样
#[derive(Clone)]
pub struct ProcSample {
    pub pid: u32,
    /// Process creation time in sysinfo's timestamp units; `None` means identity is unavailable.
    pub start_time: Option<u64>,
    pub name: String,
    /// Process share normalized to whole-machine capacity (0–100%).
    pub cpu: f32,
    pub mem_mb: f64,
    pub io_kbps: f64,
    /// True only when this process received a new counter-delta rate in this inventory scan.
    pub io_sample_fresh: bool,
}

#[derive(Default)]
struct Dim {
    values: [f32; ROBUST_WINDOW],
    len: usize,
    next: usize,
    center: f32,
    scale: f32,
    baseline_threshold: Option<f32>,
    streak: u32,
    /// Wall clock captured when the current hot streak began (0 streak → 1 transition).
    hot_since_unix: Option<u64>,
}

impl Dim {
    fn median(values: &[f32; ROBUST_WINDOW], len: usize) -> f32 {
        let mut ordered = *values;
        ordered[..len].select_nth_unstable_by(len / 2, f32::total_cmp);
        ordered[len / 2]
    }

    /// Bounded, allocation-free median/MAD baseline. The fixed odd-sized window
    /// keeps fewer than half-window outliers from moving the center materially.
    fn robust_location_scale(&self, threshold: f32) -> (f32, f32) {
        let center = Self::median(&self.values, self.len);
        let mut deviations = [0.0; ROBUST_WINDOW];
        for (index, value) in self.values[..self.len].iter().enumerate() {
            deviations[index] = (*value - center).abs();
        }
        let mad = Self::median(&deviations, self.len);
        let scale_floor = threshold.abs() * SCALE_FLOOR_RATIO;
        let scaled_mad = (MAD_TO_SIGMA * mad).min(f32::MAX);
        (center, scaled_mad.max(scale_floor).max(f32::EPSILON))
    }

    fn push(&mut self, value: f32) {
        self.values[self.next] = value;
        self.next = (self.next + 1) % ROBUST_WINDOW;
        self.len = (self.len + 1).min(ROBUST_WINDOW);
    }

    fn robust_z(&self, value: f32) -> f64 {
        let score = (value - self.center) / self.scale.max(f32::EPSILON);
        score.clamp(-f32::MAX, f32::MAX) as f64
    }

    /// Confirm anomalies against a rolling median/MAD baseline; confirmed hot
    /// samples stay out of the reference window so they cannot normalize themselves.
    fn update(&mut self, x: f32, threshold: f32, confirm: u32, floor: f32, now_unix: u64) -> bool {
        if !x.is_finite() {
            self.streak = 0;
            self.hot_since_unix = None;
            return false;
        }
        if self.len < ROBUST_WINDOW {
            self.push(x);
            if self.len == ROBUST_WINDOW {
                (self.center, self.scale) = self.robust_location_scale(threshold);
                self.baseline_threshold = Some(threshold);
            }
            self.streak = 0;
            self.hot_since_unix = None;
            return false;
        }

        // Hot samples do not enter the baseline ring, so its median/MAD remains
        // unchanged throughout a streak. Reuse that baseline unless the threshold changed.
        if self.streak == 0 || self.baseline_threshold != Some(threshold) {
            (self.center, self.scale) = self.robust_location_scale(threshold);
            self.baseline_threshold = Some(threshold);
        }
        let hot = x > floor && x > self.center + 3.0 * self.scale + threshold;
        if hot {
            if self.streak == 0 && now_unix > 0 {
                self.hot_since_unix = Some(now_unix);
            }
            self.streak = self.streak.saturating_add(1);
        } else {
            self.streak = 0;
            self.hot_since_unix = None;
            // The next tick rebuilds statistics from the updated ring; avoid sorting the window twice.
            self.push(x);
        }
        self.streak >= confirm
    }
}

#[derive(Default)]
struct PidTrack {
    cpu: Dim,
    mem: Dim,
    io: Dim,
    last_tick: u64,
    start_time: Option<u64>,
    creation_filetime: Option<u64>,
    /// Latest observed process name, kept for read-only onset correlation hints.
    name: String,
}

fn refresh_tracked_name(current: &mut String, sampled: &str) {
    if current != sampled {
        current.clear();
        current.push_str(sampled);
    }
}

fn process_identity_matches(previous: Option<u64>, current: Option<u64>) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => previous == current,
        _ => true,
    }
}

fn process_creation_filetime_matches(previous: Option<u64>, current: Option<u64>) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => previous == current,
        (None, None) => true,
        _ => false,
    }
}

fn should_shrink_pid_tracks(len: usize, capacity: usize) -> bool {
    capacity >= PID_TRACK_MIN_SHRINK_CAPACITY
        && len.saturating_mul(PID_TRACK_SHRINK_RATIO) < capacity
}

fn shrink_sparse_pid_tracks(tracks: &mut HashMap<u32, PidTrack>) {
    if should_shrink_pid_tracks(tracks.len(), tracks.capacity()) {
        tracks.shrink_to_fit();
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct Diagnostics {
    pub score: u8,
    pub deductions: Vec<Deduction>,
    pub anomalies: Vec<Anomaly>,
    /// True after at least one fresh process inventory has been ingested.
    pub process_sample_ready: bool,
    /// Current processes whose per-dimension robust windows are still warming.
    pub cpu_baseline_warming_processes: usize,
    pub mem_baseline_warming_processes: usize,
    pub io_baseline_warming_processes: usize,
    pub mem_slope: f32,
    pub mem_pct: f32,
    /// True when the 45-second window contains enough valid readings for a trend decision.
    pub mem_slope_ready: bool,
    /// Distinct valid memory sensor readings in the 45-second trend window (0–45).
    pub mem_slope_sample_count: u8,
    /// Linear ETA to 90% RAM pressure; present only for >=20 distinct readings, 78–<90%, rising trends, and <=3 minutes.
    pub mem_critical_eta_secs: Option<u32>,
    /// Robust CPU trend in percentage points per second; finite only after warm-up.
    pub cpu_trend_pct_per_sec: f32,
    /// Theil–Sen projection ten seconds ahead, clamped to the physical 0–100% range.
    pub cpu_forecast_10s_pct: f32,
    /// Robust residual stability index (0–100), not a probability.
    pub cpu_forecast_stability: f32,
    /// Distinct monotonic samples currently available in the CPU forecast window (0–15).
    pub cpu_forecast_sample_count: u8,
    /// Seconds spanned by valid samples in the current 15-sample CPU forecast window.
    pub cpu_forecast_observed_span_secs: f32,
    pub cpu_forecast_ready: bool,
}

pub struct Engine {
    tick: u64,
    /// Latest wall-clock second noted by the sampler; 0 until the first note.
    wallclock_unix: u64,
    process_cpu_count: usize,
    remainder: std::time::Duration,
    cpu_hist: Vec<f32>,
    cpu_forecast_hist: Vec<CpuForecastSample>,
    cpu_forecast_clock_secs: f64,
    mem_hist: Vec<f32>,
    mem_observation_hist: Vec<bool>,
    process_creation_filetimes: HashMap<u32, u64>,
    procs: HashMap<u32, PidTrack>,
    pub current: Diagnostics,
}

#[derive(Clone, Copy, Debug, Default)]
struct CpuForecast {
    slope_pct_per_sec: f32,
    projected_10s_pct: f32,
    stability: f32,
    sample_count: u8,
    observed_span_secs: f32,
    ready: bool,
}

#[derive(Clone, Copy, Debug)]
struct CpuForecastSample {
    at_secs: f64,
    cpu_pct: f32,
}

fn median_f32(values: &mut [f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let len = values.len();
    let middle = len / 2;
    let (lower, pivot, _) = values.select_nth_unstable_by(middle, f32::total_cmp);
    let upper = *pivot;
    if len % 2 == 1 {
        upper
    } else {
        let lower_middle = lower
            .iter()
            .copied()
            .max_by(f32::total_cmp)
            .unwrap_or(upper);
        (lower_middle + upper) * 0.5
    }
}

/// Fixed-window Theil–Sen forecast over real sensor timestamps: median pairwise
/// slope resists spikes, while median intercept/residuals provide a stability index.
/// Warm-up requires ten distinct monotonic times spanning five seconds so duplicate polls
/// and compressed bursts do not add forecast confidence.
fn forecast_cpu(samples: &[CpuForecastSample]) -> CpuForecast {
    if !samples
        .last()
        .is_some_and(|sample| sample.at_secs.is_finite() && sample.cpu_pct.is_finite())
    {
        return CpuForecast::default();
    }
    let window = samples.len().min(CPU_FORECAST_WINDOW);
    if window == 0 {
        return CpuForecast::default();
    }

    let mut x = [0.0f32; CPU_FORECAST_WINDOW];
    let mut y = [0.0f32; CPU_FORECAST_WINDOW];
    let start = samples.len() - window;
    let origin = samples[start..]
        .iter()
        .find(|sample| sample.at_secs.is_finite() && sample.cpu_pct.is_finite())
        .map(|sample| sample.at_secs);
    let Some(origin) = origin else {
        return CpuForecast::default();
    };
    let mut count = 0usize;
    for sample in &samples[start..] {
        if sample.at_secs.is_finite() && sample.cpu_pct.is_finite() {
            let relative_time = (sample.at_secs - origin) as f32;
            if !relative_time.is_finite()
                || relative_time < 0.0
                || (count > 0 && relative_time < x[count - 1])
            {
                return CpuForecast::default();
            }
            x[count] = relative_time;
            y[count] = sample.cpu_pct.clamp(0.0, 100.0);
            count += 1;
        }
    }
    if count == 0 {
        return CpuForecast::default();
    }
    let mut independent_times = 1usize;
    for index in 1..count {
        if x[index] > x[index - 1] {
            independent_times += 1;
        }
    }
    let observed_span = x[count - 1] - x[0];
    let progress = CpuForecast {
        sample_count: independent_times as u8,
        observed_span_secs: observed_span,
        ..CpuForecast::default()
    };
    if independent_times < CPU_FORECAST_MIN_SAMPLES || observed_span < CPU_FORECAST_MIN_SPAN_SECS {
        return progress;
    }

    let mut slopes = [0.0f32; CPU_FORECAST_MAX_SLOPES];
    let mut slope_count = 0usize;
    for left in 0..count - 1 {
        for right in left + 1..count {
            let delta_time = x[right] - x[left];
            if delta_time <= 0.0 {
                continue;
            }
            let slope = (y[right] - y[left]) / delta_time;
            if slope.is_finite() {
                slopes[slope_count] = slope;
                slope_count += 1;
            }
        }
    }
    if slope_count == 0 {
        return progress;
    }
    let slope = median_f32(&mut slopes[..slope_count]);

    let mut intercepts = [0.0f32; CPU_FORECAST_WINDOW];
    for index in 0..count {
        intercepts[index] = y[index] - slope * x[index];
    }
    let intercept = median_f32(&mut intercepts[..count]);

    let mut residuals = [0.0f32; CPU_FORECAST_WINDOW];
    for index in 0..count {
        residuals[index] = (y[index] - (intercept + slope * x[index])).abs();
    }
    let median_residual = median_f32(&mut residuals[..count]);
    let projected =
        (intercept + slope * (x[count - 1] + CPU_FORECAST_HORIZON_SECS)).clamp(0.0, 100.0);
    CpuForecast {
        slope_pct_per_sec: slope,
        projected_10s_pct: projected,
        stability: (100.0 - median_residual * 4.0).clamp(0.0, 100.0),
        sample_count: independent_times as u8,
        observed_span_secs: observed_span,
        ready: true,
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            tick: 0,
            wallclock_unix: 0,
            process_cpu_count: 1,
            remainder: std::time::Duration::ZERO,
            cpu_hist: Vec::new(),
            cpu_forecast_hist: Vec::with_capacity(CPU_FORECAST_WINDOW),
            cpu_forecast_clock_secs: 0.0,
            mem_hist: Vec::new(),
            mem_observation_hist: Vec::new(),
            process_creation_filetimes: HashMap::new(),
            procs: HashMap::new(),
            current: Diagnostics::default(),
        }
    }

    pub(crate) fn set_process_cpu_count(&mut self, cpu_count: usize) {
        self.process_cpu_count = cpu_count.max(1);
    }

    /// Replace the exact identities captured alongside the current sysinfo process snapshot.
    pub(crate) fn set_process_creation_filetimes(&mut self, filetimes: &HashMap<u32, u64>) {
        self.process_creation_filetimes.clone_from(filetimes);
    }

    /// Record the current wall-clock second so anomaly streaks can stamp a
    /// human-correlatable onset; called by the sampler next to each observation.
    pub fn note_wallclock(&mut self, now_unix: u64) {
        self.wallclock_unix = now_unix;
    }

    /// Snapshot of tracked process identities for onset correlation.
    pub fn tracked_process_starts(&self) -> Vec<(u32, String, Option<u64>)> {
        self.procs
            .iter()
            .map(|(pid, track)| (*pid, track.name.clone(), track.start_time))
            .collect()
    }

    /// Keep the test helper for callers that provide a fresh process snapshot on every call.
    #[cfg(test)]
    pub fn observe_elapsed(
        &mut self,
        elapsed: std::time::Duration,
        cpu: f32,
        mem: f32,
        procs: &[ProcSample],
        startup: usize,
        swap: f64,
        disk: f64,
        uptime: u64,
    ) {
        self.observe_elapsed_with_process_fresh(
            elapsed, cpu, mem, procs, startup, swap, disk, uptime, true,
        );
    }

    /// Compatibility path for tests that provide fresh CPU, memory, and I/O process data together.
    #[cfg(test)]
    pub fn observe_elapsed_with_process_fresh(
        &mut self,
        elapsed: std::time::Duration,
        cpu: f32,
        mem: f32,
        procs: &[ProcSample],
        startup: usize,
        swap: f64,
        disk: f64,
        uptime: u64,
        process_sample_fresh: bool,
    ) {
        self.observe_elapsed_with_sample_freshness(
            elapsed,
            cpu,
            mem,
            procs,
            startup,
            swap,
            disk,
            uptime,
            process_sample_fresh,
            process_sample_fresh,
        );
    }

    /// Global CPU/memory histories fill one-second buckets; memory-trend confidence counts only
    /// the real reading on the latest bucket, not values repeated into elapsed buckets. The CPU
    /// forecast separately retains each real sensor sample with its elapsed timestamp. Process baselines advance only when
    /// the caller supplies a newly refreshed process inventory; I/O baselines advance only when
    /// a new counter-delta sample is available. Reused data is never counted as another observation.
    /// A fresh process sample is applied once, on the newest elapsed bucket.
    /// Gaps over five seconds clear derived state instead of extrapolating stale data.
    #[cfg(test)]
    pub fn observe_elapsed_with_sample_freshness(
        &mut self,
        elapsed: std::time::Duration,
        cpu: f32,
        mem: f32,
        procs: &[ProcSample],
        startup: usize,
        swap: f64,
        disk: f64,
        uptime: u64,
        process_sample_fresh: bool,
        io_sample_fresh: bool,
    ) {
        self.observe_elapsed_with_memory_sample_freshness(
            elapsed,
            cpu,
            mem,
            procs,
            startup,
            swap,
            disk,
            uptime,
            process_sample_fresh,
            io_sample_fresh,
            true,
        );
    }

    pub(crate) fn observe_elapsed_with_memory_sample_freshness(
        &mut self,
        elapsed: std::time::Duration,
        cpu: f32,
        mem: f32,
        procs: &[ProcSample],
        startup: usize,
        swap: f64,
        disk: f64,
        uptime: u64,
        process_sample_fresh: bool,
        io_sample_fresh: bool,
        memory_sample_fresh: bool,
    ) {
        if elapsed > std::time::Duration::from_secs(5) {
            self.procs.clear();
            self.cpu_hist.clear();
            self.cpu_forecast_hist.clear();
            self.cpu_forecast_clock_secs = 0.0;
            self.mem_hist.clear();
            self.mem_observation_hist.clear();
            self.remainder = std::time::Duration::ZERO;
            self.current = Diagnostics::default();
            return;
        }
        if cpu.is_finite() {
            self.cpu_forecast_clock_secs += elapsed.as_secs_f64();
            if self.cpu_forecast_hist.len() == CPU_FORECAST_WINDOW {
                self.cpu_forecast_hist.remove(0);
            }
            self.cpu_forecast_hist.push(CpuForecastSample {
                at_secs: self.cpu_forecast_clock_secs,
                cpu_pct: cpu.clamp(0.0, 100.0),
            });
        } else {
            self.cpu_forecast_hist.clear();
            self.cpu_forecast_clock_secs = 0.0;
        }
        let cpu_forecast = forecast_cpu(&self.cpu_forecast_hist);
        self.current.cpu_trend_pct_per_sec = cpu_forecast.slope_pct_per_sec;
        self.current.cpu_forecast_10s_pct = cpu_forecast.projected_10s_pct;
        self.current.cpu_forecast_stability = cpu_forecast.stability;
        self.current.cpu_forecast_sample_count = cpu_forecast.sample_count;
        self.current.cpu_forecast_observed_span_secs = cpu_forecast.observed_span_secs;
        self.current.cpu_forecast_ready = cpu_forecast.ready;
        self.remainder += elapsed;
        while self.remainder >= std::time::Duration::from_secs(1) {
            self.remainder -= std::time::Duration::from_secs(1);
            let latest_bucket = self.remainder < std::time::Duration::from_secs(1);
            let fresh_on_this_bucket = process_sample_fresh && latest_bucket;
            self.observe_samples(
                cpu,
                mem,
                procs,
                startup,
                swap,
                disk,
                uptime,
                fresh_on_this_bucket,
                io_sample_fresh && fresh_on_this_bucket,
                memory_sample_fresh && latest_bucket,
            );
        }
    }

    fn slope(hist: &[f32], n: usize) -> f32 {
        let len = n.min(MEMORY_SLOPE_WINDOW).min(hist.len());
        if len < MEMORY_SLOPE_MIN_SAMPLES {
            return 0.0;
        }
        let samples = &hist[hist.len() - len..];
        let mut bucket_indices = [0usize; MEMORY_SLOPE_WINDOW];
        let mut values = [0.0f32; MEMORY_SLOPE_WINDOW];
        let mut count = 0usize;
        for (index, value) in samples.iter().enumerate() {
            if valid_memory_pct(*value) {
                bucket_indices[count] = index;
                values[count] = *value;
                count += 1;
            }
        }
        if count < MEMORY_SLOPE_MIN_SAMPLES {
            return 0.0;
        }

        // Pairwise slopes retain elapsed bucket distances across gaps and resist isolated spikes.
        let mut slopes = [0.0f32; MEMORY_SLOPE_MAX_PAIRS];
        let mut slope_count = 0usize;
        for first in 0..count {
            for second in (first + 1)..count {
                let elapsed = bucket_indices[second] - bucket_indices[first];
                slopes[slope_count] = (values[second] - values[first]) / elapsed as f32;
                slope_count += 1;
            }
        }
        let slope = median_f32(&mut slopes[..slope_count]);
        if slope.is_finite() {
            slope
        } else {
            0.0
        }
    }

    fn push(hist: &mut Vec<f32>, v: f32) {
        if hist.len() >= HIST_CAP {
            hist.remove(0);
        }
        hist.push(v);
    }

    fn push_observation_flag(hist: &mut Vec<bool>, observed: bool) {
        if hist.len() >= HIST_CAP {
            hist.remove(0);
        }
        hist.push(observed);
    }

    fn mean_finite(samples: &[f32]) -> Option<f32> {
        let (sum, count) = samples
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .fold((0.0f32, 0usize), |(sum, count), value| {
                (sum + value, count + 1)
            });
        (count > 0).then_some(sum / count as f32)
    }

    /// 每秒喂入一次：全局指标 + 全部进程的多维采样 + 启动项数量（供纯算法测试使用）
    #[cfg(test)]
    pub fn observe(
        &mut self,
        cpu: f32,
        mem_pct: f32,
        procs: Vec<ProcSample>,
        startup_count: usize,
        swap_gb: f64,
        c_used_pct: f64,
        uptime_secs: u64,
    ) {
        self.observe_elapsed_with_process_fresh(
            std::time::Duration::from_secs(1),
            cpu,
            mem_pct,
            &procs,
            startup_count,
            swap_gb,
            c_used_pct,
            uptime_secs,
            true,
        );
    }

    /// Shared borrowed path for direct samples and elapsed-second buckets.
    fn observe_samples(
        &mut self,
        cpu: f32,
        mem_pct: f32,
        procs: &[ProcSample],
        startup_count: usize,
        swap_gb: f64,
        c_used_pct: f64,
        uptime_secs: u64,
        process_sample_fresh: bool,
        io_sample_fresh: bool,
        memory_observation_fresh: bool,
    ) {
        self.tick += 1;
        let cpu_sample = if cpu.is_finite() {
            cpu.clamp(0.0, 100.0)
        } else {
            f32::NAN
        };
        Self::push(&mut self.cpu_hist, cpu_sample);
        Self::push(&mut self.mem_hist, mem_pct);
        Self::push_observation_flag(&mut self.mem_observation_hist, memory_observation_fresh);

        // ---- 三维滚动 median/MAD 基线与异常检测 ----
        if process_sample_fresh {
            let now_unix = self.wallclock_unix;
            let mut anomalies: Vec<Anomaly> = self
                .current
                .anomalies
                .iter()
                .filter(|anomaly| {
                    let previous_start_time = self
                        .procs
                        .get(&anomaly.pid)
                        .and_then(|track| track.start_time);
                    let previous_creation_filetime = self
                        .procs
                        .get(&anomaly.pid)
                        .and_then(|track| track.creation_filetime);
                    let current_creation_filetime =
                        self.process_creation_filetimes.get(&anomaly.pid).copied();
                    anomaly.kind == "io"
                        && procs.iter().any(|sample| {
                            sample.pid == anomaly.pid
                                && process_identity_matches(previous_start_time, sample.start_time)
                                && process_creation_filetime_matches(
                                    previous_creation_filetime,
                                    current_creation_filetime,
                                )
                                && (!io_sample_fresh || !sample.io_sample_fresh)
                        })
                })
                .cloned()
                .collect();
            let (mut cpu_warming, mut mem_warming, mut io_warming) = (0usize, 0usize, 0usize);
            for s in procs {
                let sample_creation_filetime = self.process_creation_filetimes.get(&s.pid).copied();
                let t = self.procs.entry(s.pid).or_default();
                let coarse_identity_changed = s.start_time.is_some_and(|start_time| {
                    t.start_time.is_some_and(|previous| previous != start_time)
                });
                let exact_identity_changed = !process_creation_filetime_matches(
                    t.creation_filetime,
                    sample_creation_filetime,
                );
                let identity_changed = coarse_identity_changed || exact_identity_changed;
                if identity_changed {
                    *t = PidTrack::default();
                }
                if let Some(start_time) = s.start_time {
                    t.start_time = Some(start_time);
                }
                t.creation_filetime = sample_creation_filetime;
                refresh_tracked_name(&mut t.name, &s.name);
                t.last_tick = self.tick;
                if identity_changed {
                    // Metrics and exact FILETIME are queried separately. Never attribute this
                    // transition scan's metrics to the newly observed PID identity.
                    cpu_warming += 1;
                    mem_warming += 1;
                    io_warming += 1;
                    continue;
                }
                let cpu_per_core = s.cpu * self.process_cpu_count as f32;
                if t.cpu.update(
                    cpu_per_core,
                    ANOMALY_CPU_THRESHOLD_PER_CORE,
                    4,
                    ANOMALY_CPU_FLOOR,
                    now_unix,
                ) {
                    anomalies.push(Anomaly {
                        pid: s.pid,
                        name: s.name.clone(),
                        kind: "cpu".into(),
                        value: s.cpu as f64,
                        baseline: (t.cpu.center / self.process_cpu_count as f32).max(0.0) as f64,
                        robust_z: t.cpu.robust_z(cpu_per_core),
                        onset_unix: t.cpu.hot_since_unix,
                        creation_filetime: t.creation_filetime,
                    });
                }
                let mem_mb = s.mem_mb as f32;
                if t.mem.update(mem_mb, 64.0, 5, ANOMALY_MEM_FLOOR, now_unix) {
                    anomalies.push(Anomaly {
                        pid: s.pid,
                        name: s.name.clone(),
                        kind: "mem".into(),
                        value: s.mem_mb,
                        baseline: t.mem.center.max(0.0) as f64,
                        robust_z: t.mem.robust_z(mem_mb),
                        onset_unix: t.mem.hot_since_unix,
                        creation_filetime: t.creation_filetime,
                    });
                }
                let io_kbps = s.io_kbps as f32;
                if io_sample_fresh
                    && s.io_sample_fresh
                    && t.io.update(io_kbps, 2048.0, 6, ANOMALY_IO_FLOOR, now_unix)
                {
                    anomalies.push(Anomaly {
                        pid: s.pid,
                        name: s.name.clone(),
                        kind: "io".into(),
                        value: s.io_kbps,
                        baseline: t.io.center.max(0.0) as f64,
                        robust_z: t.io.robust_z(io_kbps),
                        onset_unix: t.io.hot_since_unix,
                        creation_filetime: t.creation_filetime,
                    });
                }
                cpu_warming += (t.cpu.len < ROBUST_WINDOW) as usize;
                mem_warming += (t.mem.len < ROBUST_WINDOW) as usize;
                io_warming += (t.io.len < ROBUST_WINDOW) as usize;
            }
            // Sort by severity, then robust-scale deviation.
            let rank = |kind: &str| match kind {
                "cpu" => 0,
                "mem" => 1,
                _ => 2,
            };
            anomalies.sort_by(|a, b| {
                rank(&a.kind)
                    .cmp(&rank(&b.kind))
                    .then_with(|| b.robust_z.total_cmp(&a.robust_z))
                    .then_with(|| a.pid.cmp(&b.pid))
            });
            anomalies.truncate(6);
            // Age entries by global seconds but only refresh their last-seen tick on a new process scan.
            self.procs.retain(|_, t| {
                self.tick
                    .checked_sub(t.last_tick)
                    .is_some_and(|age| age < 60)
            });
            // Avoid retaining a transient high-water PID table after stale tracks expire.
            shrink_sparse_pid_tracks(&mut self.procs);
            self.current.process_sample_ready = true;
            self.current.cpu_baseline_warming_processes = cpu_warming;
            self.current.mem_baseline_warming_processes = mem_warming;
            self.current.io_baseline_warming_processes = io_warming;
            self.current.anomalies = anomalies;
        }

        // ---- 内存压力斜率（%/s，近 45 秒）----
        self.current.mem_pct = mem_pct;
        let memory_samples = valid_memory_observation_count(
            &self.mem_hist,
            &self.mem_observation_hist,
            MEMORY_SLOPE_WINDOW,
        );
        self.current.mem_slope_sample_count = memory_samples as u8;
        self.current.mem_slope_ready = memory_samples >= MEMORY_SLOPE_MIN_SAMPLES;
        self.current.mem_slope = if self.current.mem_slope_ready {
            Self::slope(&self.mem_hist, MEMORY_SLOPE_WINDOW)
        } else {
            0.0
        };
        self.current.mem_critical_eta_secs =
            memory_critical_eta_secs(mem_pct, self.current.mem_slope, memory_samples);

        // ---- 健康评分：多维加权扣分 ----
        let mut score: f32 = 100.0;
        let mut ded: Vec<Deduction> = Vec::new();
        let mut deduct = |item: &str, cost: f32, cap: f32, ded: &mut Vec<Deduction>| {
            let c = cost.min(cap).round() as u8;
            if c >= 1 {
                ded.push(Deduction {
                    item: item.into(),
                    cost: c,
                });
            }
            score -= c as f32;
        };

        let cpu_avg_1min = self.cpu_avg(60);
        if cpu_avg_1min > 70.0 {
            deduct(
                "CPU 持续高负载",
                (cpu_avg_1min - 70.0) * 0.8,
                25.0,
                &mut ded,
            );
        }
        if mem_pct > 80.0 {
            deduct("内存水位偏高", (mem_pct - 80.0) * 1.2, 25.0, &mut ded);
        }
        if swap_gb > 0.5 {
            deduct("已使用交换分区", 8.0, 8.0, &mut ded);
        }
        if c_used_pct > 90.0 {
            deduct("C 盘空间告急", 15.0, 15.0, &mut ded);
        } else if c_used_pct > 80.0 {
            deduct("C 盘空间偏低", 8.0, 8.0, &mut ded);
        }
        if startup_count > 10 {
            deduct("启动项过多", 5.0, 5.0, &mut ded);
        }
        if uptime_secs > 7 * 86400 {
            deduct("长时间未重启", 5.0, 5.0, &mut ded);
        }

        self.current.score = score.round().clamp(5.0, 100.0) as u8;
        self.current.deductions = ded;
    }

    /// 最近 N 秒 CPU 峰值，供负载选型识别短时饱和。
    pub fn cpu_peak(&self, n: usize) -> f32 {
        let k = n.min(self.cpu_hist.len());
        self.cpu_hist[self.cpu_hist.len().saturating_sub(k)..]
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .fold(0.0, f32::max)
    }

    /// 近 N 个采样时间桶的有效 CPU 均值；全窗口无效时返回 NaN（自适应仲裁用）
    pub fn cpu_avg(&self, n: usize) -> f32 {
        let k = n.min(self.cpu_hist.len());
        if k == 0 {
            return 0.0;
        }
        Self::mean_finite(&self.cpu_hist[self.cpu_hist.len() - k..]).unwrap_or(f32::NAN)
    }

    /// smart_trim 决策依据：高水位 且（急涨 或 已逼近极限）
    pub fn trim_needed(&self) -> Result<(), &'static str> {
        if self.mem_hist.len() < MEMORY_TREND_WARMUP_BUCKETS {
            return Err("正在积累观测数据（15 秒）");
        }
        if !valid_memory_pct(self.current.mem_pct) {
            return Err("内存水位数据暂不可用");
        }
        if self.current.mem_pct < MEMORY_PRESSURE_ELEVATED_PCT {
            return Err("内存水位充裕");
        }
        if self.current.mem_pct >= MEMORY_CRITICAL_PCT {
            return Ok(());
        }
        if valid_memory_observation_count(
            &self.mem_hist,
            &self.mem_observation_hist,
            MEMORY_SLOPE_WINDOW,
        ) < MEMORY_SLOPE_MIN_SAMPLES
        {
            return Err("内存趋势有效样本不足（至少 10 个）");
        }
        if self.current.mem_slope > MEMORY_RISING_SLOPE_THRESHOLD {
            Ok(())
        } else {
            Err("内存平稳，无需干预")
        }
    }
}

// Preserve the prior per-core-scale detection behavior while reporting whole-machine percentages.
const ANOMALY_CPU_THRESHOLD_PER_CORE: f32 = 2.0;
const ANOMALY_CPU_FLOOR: f32 = 15.0;
const ANOMALY_MEM_FLOOR: f32 = 400.0; // MB
const ANOMALY_IO_FLOOR: f32 = 30_000.0; // KB/s ≈ 30MB/s

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_samples_at_interval(values: &[f32], interval_secs: f64) -> Vec<CpuForecastSample> {
        values
            .iter()
            .copied()
            .enumerate()
            .map(|(index, cpu_pct)| CpuForecastSample {
                at_secs: index as f64 * interval_secs,
                cpu_pct,
            })
            .collect()
    }

    #[test]
    fn pid_track_table_shrinks_only_after_a_large_sparse_collapse() {
        assert!(!should_shrink_pid_tracks(
            256,
            PID_TRACK_MIN_SHRINK_CAPACITY
        ));
        assert!(should_shrink_pid_tracks(255, PID_TRACK_MIN_SHRINK_CAPACITY));
        assert!(!should_shrink_pid_tracks(
            0,
            PID_TRACK_MIN_SHRINK_CAPACITY - 1
        ));

        let mut tracks = HashMap::with_capacity(PID_TRACK_MIN_SHRINK_CAPACITY * 4);
        for pid in 0..32 {
            tracks.insert(
                pid,
                PidTrack {
                    last_tick: pid as u64 + 1,
                    ..PidTrack::default()
                },
            );
        }
        let before_capacity = tracks.capacity();
        assert!(should_shrink_pid_tracks(tracks.len(), before_capacity));

        shrink_sparse_pid_tracks(&mut tracks);

        assert_eq!(tracks.len(), 32);
        assert!(tracks.capacity() < before_capacity);
        let released_slot_bytes =
            (before_capacity - tracks.capacity()) * std::mem::size_of::<(u32, PidTrack)>();
        assert!(
            released_slot_bytes >= 1_000_000,
            "expected to release at least 1 MB of PID-track slot capacity, got {released_slot_bytes} bytes"
        );
        assert_eq!(tracks.get(&7).unwrap().last_tick, 8);
    }

    #[test]
    fn rolling_median_mad_resists_sparse_outliers_and_confirms_on_fourth() {
        let mut clean = Dim::default();
        let mut contaminated = Dim::default();
        for index in 0..ROBUST_WINDOW {
            assert!(!clean.update(10.0, 2.0, 4, 15.0, 0));
            let sample = if index == 7 || index == 18 {
                1000.0
            } else {
                10.0
            };
            assert!(!contaminated.update(sample, 2.0, 4, 15.0, 0));
        }
        assert_eq!(clean.center, 10.0);
        assert_eq!(contaminated.center, 10.0);
        assert!((contaminated.scale - 0.2).abs() < 0.001);
        for _ in 0..3 {
            assert!(!contaminated.update(80.0, 2.0, 4, 15.0, 0));
        }
        assert!(contaminated.update(80.0, 2.0, 4, 15.0, 0));
        assert_eq!(contaminated.center, 10.0);
        assert!(contaminated.robust_z(80.0) > 300.0);
        for _ in 0..40 {
            clean.update(10.0, 2.0, 4, 15.0, 0);
        }
        assert!(!clean.update(10.0, 2.0, 4, 15.0, 0));
    }

    fn reference_dim_update_full_recompute(
        dim: &mut Dim,
        x: f32,
        threshold: f32,
        confirm: u32,
        floor: f32,
        now_unix: u64,
    ) -> bool {
        if !x.is_finite() {
            dim.streak = 0;
            dim.hot_since_unix = None;
            return false;
        }
        if dim.len < ROBUST_WINDOW {
            dim.push(x);
            if dim.len == ROBUST_WINDOW {
                (dim.center, dim.scale) = dim.robust_location_scale(threshold);
            }
            dim.streak = 0;
            dim.hot_since_unix = None;
            return false;
        }

        (dim.center, dim.scale) = dim.robust_location_scale(threshold);
        let hot = x > floor && x > dim.center + 3.0 * dim.scale + threshold;
        if hot {
            if dim.streak == 0 && now_unix > 0 {
                dim.hot_since_unix = Some(now_unix);
            }
            dim.streak = dim.streak.saturating_add(1);
        } else {
            dim.streak = 0;
            dim.hot_since_unix = None;
            dim.push(x);
        }
        dim.streak >= confirm
    }

    #[test]
    fn cached_hot_baseline_matches_full_recompute_across_threshold_changes() {
        let mut cached = Dim::default();
        let mut reference = Dim::default();
        let mut samples = vec![(10.0, 2.0, 4, 15.0, 1_000); ROBUST_WINDOW];
        samples.extend([
            (80.0, 2.0, 4, 15.0, 1_100),
            (80.0, 2.0, 4, 15.0, 1_101),
            (80.0, 4.0, 4, 15.0, 1_102),
            (80.0, 4.0, 4, 15.0, 1_103),
            (10.0, 4.0, 4, 15.0, 1_104),
            (80.0, 4.0, 4, 15.0, 1_105),
            (f32::NAN, 4.0, 4, 15.0, 1_106),
            (80.0, 3.0, 4, 15.0, 1_107),
            (80.0, 3.0, 4, 15.0, 1_108),
            (10.0, 3.0, 4, 15.0, 1_109),
            (10.1, 2.0, 4, 15.0, 1_110),
            (80.0, 2.0, 4, 15.0, 1_111),
        ]);

        for (x, threshold, confirm, floor, now_unix) in samples {
            let cached_result = cached.update(x, threshold, confirm, floor, now_unix);
            let reference_result = reference_dim_update_full_recompute(
                &mut reference,
                x,
                threshold,
                confirm,
                floor,
                now_unix,
            );
            assert_eq!(cached_result, reference_result);
            assert_eq!(cached.values, reference.values);
            assert_eq!(cached.len, reference.len);
            assert_eq!(cached.next, reference.next);
            assert_eq!(cached.center, reference.center);
            assert_eq!(cached.scale, reference.scale);
            assert_eq!(cached.streak, reference.streak);
            assert_eq!(cached.hot_since_unix, reference.hot_since_unix);
        }
    }

    #[test]
    #[ignore = "manual synthetic robust-baseline hot-streak benchmark"]
    fn benchmark_dim_hot_streak_baseline_cache() {
        const DIM_COUNT: usize = 256;
        const BATCHES: usize = 101;
        let make_hot_streak_dim = || {
            let mut dim = Dim::default();
            for _ in 0..ROBUST_WINDOW {
                dim.update(10.0, 2.0, 4, 15.0, 1_000);
            }
            assert!(!dim.update(80.0, 2.0, 4, 15.0, 1_001));
            dim
        };
        let mut cached = (0..DIM_COUNT)
            .map(|_| make_hot_streak_dim())
            .collect::<Vec<_>>();
        let mut full_recompute = (0..DIM_COUNT)
            .map(|_| make_hot_streak_dim())
            .collect::<Vec<_>>();
        let mut cached_micros = Vec::with_capacity(BATCHES);
        let mut reference_micros = Vec::with_capacity(BATCHES);

        for batch in 0..BATCHES {
            let mut run_cached = || {
                let started = std::time::Instant::now();
                for dim in &mut cached {
                    std::hint::black_box(dim.update(80.0, 2.0, 4, 15.0, 1_002));
                }
                started.elapsed().as_secs_f64() * 1_000_000.0 / DIM_COUNT as f64
            };
            let mut run_reference = || {
                let started = std::time::Instant::now();
                for dim in &mut full_recompute {
                    std::hint::black_box(reference_dim_update_full_recompute(
                        dim, 80.0, 2.0, 4, 15.0, 1_002,
                    ));
                }
                started.elapsed().as_secs_f64() * 1_000_000.0 / DIM_COUNT as f64
            };
            if batch % 2 == 0 {
                cached_micros.push(run_cached());
                reference_micros.push(run_reference());
            } else {
                reference_micros.push(run_reference());
                cached_micros.push(run_cached());
            }
        }
        assert_eq!(cached[0].center, full_recompute[0].center);
        assert_eq!(cached[0].scale, full_recompute[0].scale);
        assert_eq!(cached[0].streak, full_recompute[0].streak);
        cached_micros.sort_by(f64::total_cmp);
        reference_micros.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        let median_reduction = 1.0 - cached_micros[BATCHES / 2] / reference_micros[BATCHES / 2];
        let p95_reduction = 1.0 - cached_micros[p95_index] / reference_micros[p95_index];
        eprintln!(
            "Dim hot-streak cache benchmark ({} dimensions/batch): full recompute median/p95={:.3}/{:.3} us/dimension; cached={:.3}/{:.3}; reduction={:.1}%/{:.1}%",
            DIM_COUNT,
            reference_micros[BATCHES / 2],
            reference_micros[p95_index],
            cached_micros[BATCHES / 2],
            cached_micros[p95_index],
            median_reduction * 100.0,
            p95_reduction * 100.0,
        );
    }

    #[test]
    fn tracked_name_refresh_preserves_and_updates_sampled_name() {
        let mut tracked = String::new();
        refresh_tracked_name(&mut tracked, "browser.exe");
        assert_eq!(tracked, "browser.exe");
        let stable_capacity = tracked.capacity();
        refresh_tracked_name(&mut tracked, "browser.exe");
        assert_eq!(tracked, "browser.exe");
        assert_eq!(tracked.capacity(), stable_capacity);

        refresh_tracked_name(&mut tracked, "browser-helper.exe");
        assert_eq!(tracked, "browser-helper.exe");
        refresh_tracked_name(&mut tracked, "应用.exe");
        assert_eq!(tracked, "应用.exe");
        refresh_tracked_name(&mut tracked, "");
        assert!(tracked.is_empty());
    }

    #[test]
    #[ignore = "manual synthetic steady-snapshot process-name refresh benchmark"]
    fn benchmark_tracked_name_refresh_steady_snapshot() {
        const PROCESS_COUNT: usize = 512;
        const BATCHES: usize = 101;
        let names = (0..PROCESS_COUNT)
            .map(|pid| format!("browser-worker-{pid}.exe"))
            .collect::<Vec<_>>();
        let mut always_clone = names.clone();
        let mut conditional_reuse = names.clone();
        for _ in 0..5 {
            for (tracked, sampled) in always_clone.iter_mut().zip(&names) {
                *tracked = sampled.clone();
            }
            for (tracked, sampled) in conditional_reuse.iter_mut().zip(&names) {
                refresh_tracked_name(tracked, sampled);
            }
        }

        let mut clone_micros = Vec::with_capacity(BATCHES);
        let mut reuse_micros = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            let mut run_clone = || {
                let started = std::time::Instant::now();
                for (tracked, sampled) in always_clone.iter_mut().zip(&names) {
                    *tracked = std::hint::black_box(sampled.clone());
                }
                started.elapsed().as_secs_f64() * 1_000_000.0 / PROCESS_COUNT as f64
            };
            let mut run_reuse = || {
                let started = std::time::Instant::now();
                for (tracked, sampled) in conditional_reuse.iter_mut().zip(&names) {
                    refresh_tracked_name(tracked, std::hint::black_box(sampled));
                }
                started.elapsed().as_secs_f64() * 1_000_000.0 / PROCESS_COUNT as f64
            };
            if batch % 2 == 0 {
                clone_micros.push(run_clone());
                reuse_micros.push(run_reuse());
            } else {
                reuse_micros.push(run_reuse());
                clone_micros.push(run_clone());
            }
        }
        assert_eq!(always_clone, conditional_reuse);
        clone_micros.sort_by(f64::total_cmp);
        reuse_micros.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "process-name steady-snapshot benchmark ({} names/batch): clone median/p95={:.3}/{:.3} us/name; reuse={:.3}/{:.3}",
            PROCESS_COUNT,
            clone_micros[BATCHES / 2],
            clone_micros[p95_index],
            reuse_micros[BATCHES / 2],
            reuse_micros[p95_index],
        );
    }

    #[test]
    fn hot_streak_records_onset_and_resets_with_streak() {
        let mut dim = Dim::default();
        for _ in 0..ROBUST_WINDOW {
            dim.update(10.0, 2.0, 4, 15.0, 1_000);
        }
        // First hot sample starts the streak at the noted wall clock; confirmation lands on the fourth.
        assert!(!dim.update(80.0, 2.0, 4, 15.0, 1_700));
        assert_eq!(dim.hot_since_unix, Some(1_700));
        assert!(!dim.update(80.0, 2.0, 4, 15.0, 1_705));
        assert!(!dim.update(80.0, 2.0, 4, 15.0, 1_710));
        assert_eq!(dim.hot_since_unix, Some(1_700));
        assert!(dim.update(80.0, 2.0, 4, 15.0, 1_715));
        assert_eq!(dim.hot_since_unix, Some(1_700));
        // A cool sample clears the streak and the onset stamp.
        dim.update(10.0, 2.0, 4, 15.0, 1_712);
        assert_eq!(dim.streak, 0);
        assert_eq!(dim.hot_since_unix, None);
        // A zero wall clock (never noted) must not fabricate an onset.
        dim.update(80.0, 2.0, 4, 15.0, 0);
        assert_eq!(dim.hot_since_unix, None);
    }

    #[test]
    fn confirmed_anomaly_carries_streak_onset_time_and_internal_identity() {
        let mut engine = Engine::new();
        let sampled_filetime = 0x01dc_1234_5678_9abcu64;
        engine.set_process_creation_filetimes(&HashMap::from([(31, sampled_filetime)]));
        let calm = ProcSample {
            pid: 31,
            start_time: Some(900),
            name: "onset-probe.exe".into(),
            cpu: 5.0,
            mem_mb: 100.0,
            io_kbps: 0.0,
            io_sample_fresh: true,
        };
        let hot = ProcSample {
            cpu: 80.0,
            ..calm.clone()
        };
        for _ in 0..ROBUST_WINDOW + 1 {
            engine.note_wallclock(1_000);
            engine.observe_elapsed_with_process_fresh(
                std::time::Duration::from_secs(1),
                10.0,
                50.0,
                std::slice::from_ref(&calm),
                0,
                0.0,
                0.0,
                0,
                true,
            );
        }
        for second in [1_100, 1_101, 1_102, 1_103] {
            engine.note_wallclock(second);
            engine.observe_elapsed_with_process_fresh(
                std::time::Duration::from_secs(1),
                10.0,
                50.0,
                std::slice::from_ref(&hot),
                0,
                0.0,
                0.0,
                0,
                true,
            );
        }
        let cpu = engine
            .current
            .anomalies
            .iter()
            .find(|anomaly| anomaly.kind == "cpu")
            .expect("cpu anomaly should confirm on the fourth hot sample");
        assert_eq!(cpu.onset_unix, Some(1_100));
        assert_eq!(cpu.creation_filetime, Some(sampled_filetime));
        let serialized = serde_json::to_value(&engine.current).unwrap();
        let serialized_cpu = serialized["anomalies"]
            .as_array()
            .unwrap()
            .iter()
            .find(|anomaly| anomaly["kind"] == "cpu")
            .unwrap();
        assert!(serialized_cpu.get("creation_filetime").is_none());
        assert_eq!(engine.procs[&31].name, "onset-probe.exe");
        assert_eq!(
            engine
                .tracked_process_starts()
                .into_iter()
                .find(|(pid, _, _)| *pid == 31)
                .and_then(|(_, _, start)| start),
            Some(900)
        );
    }

    #[test]
    fn rolling_baseline_tracks_gradual_drift_and_ignores_subfloor_spikes() {
        let mut dim = Dim::default();
        for _ in 0..40 {
            assert!(!dim.update(10.0, 2.0, 4, 15.0, 0));
        }
        assert!(!dim.update(14.0, 2.0, 4, 15.0, 0));
        assert_eq!(dim.center, 10.0);
        for step in 1..=40 {
            assert!(!dim.update(10.0 + step as f32 * 0.1, 2.0, 4, 15.0, 0));
        }
        assert!(!dim.update(14.0, 2.0, 4, 15.0, 0));
        assert!(
            dim.center > 12.4,
            "rolling median should follow gradual drift: {}",
            dim.center
        );
    }

    #[test]
    fn cpu_memory_and_io_keep_their_four_five_six_sample_confirmation() {
        let mut engine = Engine::new();
        let sample = |cpu, mem_mb, io_kbps| ProcSample {
            pid: 77,
            start_time: None,
            name: "fixture".into(),
            cpu,
            mem_mb,
            io_kbps,
            io_sample_fresh: true,
        };
        for _ in 0..ROBUST_WINDOW {
            engine.observe(0.0, 50.0, vec![sample(5.0, 100.0, 1_000.0)], 0, 0.0, 0.0, 0);
        }
        for confirmation in 1..=6 {
            engine.observe(
                0.0,
                50.0,
                vec![sample(80.0, 2_000.0, 50_000.0)],
                0,
                0.0,
                0.0,
                0,
            );
            let kinds = engine
                .current
                .anomalies
                .iter()
                .map(|item| item.kind.as_str())
                .collect::<Vec<_>>();
            assert_eq!(kinds.contains(&"cpu"), confirmation >= 4);
            assert_eq!(kinds.contains(&"mem"), confirmation >= 5);
            assert_eq!(kinds.contains(&"io"), confirmation >= 6);
        }
    }

    #[test]
    fn high_core_process_cpu_detection_preserves_per_core_gate_and_reports_system_percent() {
        let mut engine = Engine::new();
        engine.set_process_cpu_count(8);
        let baseline = ProcSample {
            pid: 79,
            start_time: None,
            name: "single-thread-fixture".into(),
            cpu: 5.0 / 8.0,
            mem_mb: 100.0,
            io_kbps: 0.0,
            io_sample_fresh: true,
        };
        for _ in 0..ROBUST_WINDOW {
            engine.observe(0.0, 50.0, vec![baseline.clone()], 0, 0.0, 0.0, 0);
        }

        // A topology/count change alters the display scale but not the detector's per-core history.
        engine.set_process_cpu_count(16);
        let unchanged_per_core_load = ProcSample {
            cpu: 5.0 / 16.0,
            ..baseline.clone()
        };
        for _ in 0..4 {
            engine.observe(
                0.0,
                50.0,
                vec![unchanged_per_core_load.clone()],
                0,
                0.0,
                0.0,
                0,
            );
            assert!(!engine
                .current
                .anomalies
                .iter()
                .any(|item| item.kind == "cpu"));
        }

        let spike = ProcSample {
            cpu: 80.0 / 16.0,
            ..baseline
        };
        for confirmation in 1..=4 {
            engine.observe(0.0, 50.0, vec![spike.clone()], 0, 0.0, 0.0, 0);
            let anomaly = engine
                .current
                .anomalies
                .iter()
                .find(|item| item.kind == "cpu");
            assert_eq!(anomaly.is_some(), confirmation == 4);
        }

        let anomaly = engine
            .current
            .anomalies
            .iter()
            .find(|item| item.kind == "cpu")
            .unwrap();
        assert!(
            (anomaly.value - 5.0).abs() < 1e-6,
            "report whole-machine CPU share"
        );
        assert!(
            (anomaly.baseline - (5.0 / 16.0)).abs() < 1e-6,
            "report baseline on the same whole-machine scale"
        );
    }

    #[test]
    fn process_baseline_readiness_tracks_warmup_per_dimension() {
        let mut engine = Engine::new();
        let sample = ProcSample {
            pid: 77,
            start_time: None,
            name: "fixture".into(),
            cpu: f32::NAN,
            mem_mb: 100.0,
            io_kbps: 1_000.0,
            io_sample_fresh: true,
        };
        assert!(!engine.current.process_sample_ready);

        for _ in 0..ROBUST_WINDOW - 1 {
            engine.observe(0.0, 50.0, vec![sample.clone()], 0, 0.0, 0.0, 0);
        }
        assert!(engine.current.process_sample_ready);
        assert_eq!(engine.current.cpu_baseline_warming_processes, 1);
        assert_eq!(engine.current.mem_baseline_warming_processes, 1);
        assert_eq!(engine.current.io_baseline_warming_processes, 1);

        engine.observe(0.0, 50.0, vec![sample.clone()], 0, 0.0, 0.0, 0);
        assert_eq!(engine.current.cpu_baseline_warming_processes, 1);
        assert_eq!(engine.current.mem_baseline_warming_processes, 0);
        assert_eq!(engine.current.io_baseline_warming_processes, 0);

        let new_process = ProcSample {
            pid: 88,
            cpu: 5.0,
            ..sample.clone()
        };
        engine.observe(0.0, 50.0, vec![sample.clone(), new_process], 0, 0.0, 0.0, 0);
        assert_eq!(engine.current.cpu_baseline_warming_processes, 2);
        assert_eq!(engine.current.mem_baseline_warming_processes, 1);
        assert_eq!(engine.current.io_baseline_warming_processes, 1);

        engine.observe_elapsed_with_process_fresh(
            std::time::Duration::from_secs(6),
            0.0,
            50.0,
            &[sample],
            0,
            0.0,
            0.0,
            0,
            true,
        );
        assert!(!engine.current.process_sample_ready);
        assert_eq!(engine.current.cpu_baseline_warming_processes, 0);
        assert_eq!(engine.current.mem_baseline_warming_processes, 0);
        assert_eq!(engine.current.io_baseline_warming_processes, 0);
    }

    #[test]
    fn cached_process_snapshot_does_not_mark_baselines_ready() {
        let mut engine = Engine::new();
        let sample = ProcSample {
            pid: 77,
            start_time: None,
            name: "fixture".into(),
            cpu: 5.0,
            mem_mb: 100.0,
            io_kbps: 1_000.0,
            io_sample_fresh: true,
        };
        for _ in 0..ROBUST_WINDOW {
            engine.observe_elapsed_with_process_fresh(
                std::time::Duration::from_secs(1),
                0.0,
                50.0,
                std::slice::from_ref(&sample),
                0,
                0.0,
                0.0,
                0,
                false,
            );
        }
        assert!(!engine.current.process_sample_ready);
        assert_eq!(engine.current.cpu_baseline_warming_processes, 0);
        assert_eq!(engine.current.mem_baseline_warming_processes, 0);
        assert_eq!(engine.current.io_baseline_warming_processes, 0);
        assert!(engine.procs.is_empty());

        engine.observe_elapsed_with_process_fresh(
            std::time::Duration::from_secs(1),
            0.0,
            50.0,
            std::slice::from_ref(&sample),
            0,
            0.0,
            0.0,
            0,
            true,
        );
        assert!(engine.current.process_sample_ready);
        assert_eq!(engine.current.cpu_baseline_warming_processes, 1);
        assert_eq!(engine.current.mem_baseline_warming_processes, 1);
        assert_eq!(engine.current.io_baseline_warming_processes, 1);
    }

    #[test]
    fn cached_process_snapshot_does_not_advance_anomaly_confirmation() {
        let mut engine = Engine::new();
        let baseline = ProcSample {
            pid: 77,
            start_time: None,
            name: "fixture".into(),
            cpu: 5.0,
            mem_mb: 100.0,
            io_kbps: 1_000.0,
            io_sample_fresh: true,
        };
        for _ in 0..ROBUST_WINDOW {
            engine.observe(0.0, 50.0, vec![baseline.clone()], 0, 0.0, 0.0, 0);
        }
        let spike = ProcSample {
            cpu: 80.0,
            mem_mb: 2_000.0,
            io_kbps: 50_000.0,
            io_sample_fresh: true,
            ..baseline.clone()
        };
        engine.observe(0.0, 50.0, vec![spike.clone()], 0, 0.0, 0.0, 0);
        let cpu_streak = engine.procs[&77].cpu.streak;
        for _ in 0..5 {
            engine.observe_elapsed_with_process_fresh(
                std::time::Duration::from_secs(1),
                0.0,
                50.0,
                std::slice::from_ref(&spike),
                0,
                0.0,
                0.0,
                0,
                false,
            );
        }
        assert_eq!(engine.procs[&77].cpu.streak, cpu_streak);
        assert!(engine.current.anomalies.is_empty());
    }

    #[test]
    fn cached_io_rate_does_not_advance_anomaly_confirmation() {
        let mut engine = Engine::new();
        let baseline = ProcSample {
            pid: 88,
            start_time: None,
            name: "io-fixture".into(),
            cpu: 5.0,
            mem_mb: 100.0,
            io_kbps: 1_000.0,
            io_sample_fresh: true,
        };
        for _ in 0..ROBUST_WINDOW {
            engine.observe_elapsed_with_sample_freshness(
                std::time::Duration::from_secs(1),
                0.0,
                50.0,
                std::slice::from_ref(&baseline),
                0,
                0.0,
                0.0,
                0,
                true,
                true,
            );
        }

        let spike = ProcSample {
            io_kbps: 50_000.0,
            io_sample_fresh: true,
            ..baseline.clone()
        };
        engine.observe_elapsed_with_sample_freshness(
            std::time::Duration::from_secs(1),
            0.0,
            50.0,
            std::slice::from_ref(&spike),
            0,
            0.0,
            0.0,
            0,
            true,
            true,
        );
        assert_eq!(engine.procs[&88].io.streak, 1);
        let io_history_len = engine.procs[&88].io.len;
        let cached_io = ProcSample {
            io_kbps: 0.0,
            io_sample_fresh: false,
            ..spike.clone()
        };

        // The process inventory refreshes every few seconds, but I/O has no new counter delta.
        for _ in 0..12 {
            engine.observe_elapsed_with_sample_freshness(
                std::time::Duration::from_secs(1),
                0.0,
                50.0,
                std::slice::from_ref(&cached_io),
                0,
                0.0,
                0.0,
                0,
                true,
                false,
            );
        }
        assert_eq!(engine.procs[&88].io.streak, 1);
        assert_eq!(engine.procs[&88].io.len, io_history_len);
        assert!(!engine
            .current
            .anomalies
            .iter()
            .any(|item| item.kind == "io"));

        for _ in 0..4 {
            engine.observe_elapsed_with_sample_freshness(
                std::time::Duration::from_secs(1),
                0.0,
                50.0,
                std::slice::from_ref(&spike),
                0,
                0.0,
                0.0,
                0,
                true,
                true,
            );
        }
        assert_eq!(engine.procs[&88].io.streak, 5);
        assert!(!engine
            .current
            .anomalies
            .iter()
            .any(|item| item.kind == "io"));

        engine.observe_elapsed_with_sample_freshness(
            std::time::Duration::from_secs(1),
            0.0,
            50.0,
            std::slice::from_ref(&spike),
            0,
            0.0,
            0.0,
            0,
            true,
            true,
        );
        assert_eq!(engine.procs[&88].io.streak, 6);
        assert!(engine
            .current
            .anomalies
            .iter()
            .any(|item| item.kind == "io"));
        engine.observe_elapsed_with_sample_freshness(
            std::time::Duration::from_secs(1),
            0.0,
            50.0,
            std::slice::from_ref(&cached_io),
            0,
            0.0,
            0.0,
            0,
            true,
            false,
        );
        assert_eq!(engine.procs[&88].io.streak, 6);
        assert!(engine
            .current
            .anomalies
            .iter()
            .any(|item| item.kind == "io"));
    }

    #[test]
    fn transition_sample_is_discarded_when_exact_identity_first_appears() {
        let mut engine = Engine::new();
        let creation_filetime = 0x01dc_1234_5678_9abcu64;
        engine.set_process_creation_filetimes(&HashMap::from([(76, creation_filetime)]));
        let sample = ProcSample {
            pid: 76,
            start_time: Some(100),
            name: "new-process.exe".into(),
            cpu: 99.0,
            mem_mb: 4_096.0,
            io_kbps: 50_000.0,
            io_sample_fresh: true,
        };

        engine.observe_elapsed_with_sample_freshness(
            std::time::Duration::from_secs(1),
            0.0,
            50.0,
            std::slice::from_ref(&sample),
            0,
            0.0,
            0.0,
            0,
            true,
            true,
        );

        let track = &engine.procs[&76];
        assert_eq!(track.creation_filetime, Some(creation_filetime));
        assert_eq!(track.cpu.len, 0);
        assert_eq!(track.mem.len, 0);
        assert_eq!(track.io.len, 0);
        assert!(engine.current.anomalies.is_empty());
        assert_eq!(engine.current.cpu_baseline_warming_processes, 1);
        assert_eq!(engine.current.mem_baseline_warming_processes, 1);
        assert_eq!(engine.current.io_baseline_warming_processes, 1);
    }

    #[test]
    fn pid_reuse_resets_metric_baselines_and_drops_stale_io_alerts() {
        assert!(process_identity_matches(Some(100), Some(100)));
        assert!(!process_identity_matches(Some(100), Some(200)));
        assert!(process_identity_matches(Some(100), None));
        assert!(process_identity_matches(None, Some(200)));
        assert!(process_creation_filetime_matches(None, None));
        assert!(!process_creation_filetime_matches(None, Some(1)));
        assert!(!process_creation_filetime_matches(Some(1), None));
        assert!(!process_creation_filetime_matches(Some(1), Some(2)));

        let mut engine = Engine::new();
        let original_filetime = 0x01dc_1234_5678_9abcu64;
        engine.set_process_creation_filetimes(&HashMap::from([(77, original_filetime)]));
        let original = ProcSample {
            pid: 77,
            start_time: Some(100),
            name: "original".into(),
            cpu: 5.0,
            mem_mb: 100.0,
            io_kbps: 1_000.0,
            io_sample_fresh: true,
        };
        for _ in 0..=ROBUST_WINDOW {
            engine.observe_elapsed_with_sample_freshness(
                std::time::Duration::from_secs(1),
                0.0,
                50.0,
                std::slice::from_ref(&original),
                0,
                0.0,
                0.0,
                0,
                true,
                true,
            );
        }
        assert_eq!(engine.procs[&77].cpu.len, ROBUST_WINDOW);
        assert_eq!(engine.procs[&77].mem.len, ROBUST_WINDOW);
        assert_eq!(engine.procs[&77].io.len, ROBUST_WINDOW);
        engine.current.anomalies.push(Anomaly {
            pid: 77,
            name: "original".into(),
            kind: "io".into(),
            value: 50_000.0,
            baseline: 1_000.0,
            robust_z: 5.0,
            onset_unix: None,
            creation_filetime: None,
        });

        let replacement_filetime = original_filetime + 1;
        engine.set_process_creation_filetimes(&HashMap::from([(77, replacement_filetime)]));
        let reused_pid = ProcSample {
            pid: 77,
            start_time: Some(100),
            name: "replacement".into(),
            cpu: 90.0,
            mem_mb: 2_000.0,
            io_kbps: 0.0,
            io_sample_fresh: false,
        };
        engine.observe_elapsed_with_sample_freshness(
            std::time::Duration::from_secs(1),
            0.0,
            50.0,
            std::slice::from_ref(&reused_pid),
            0,
            0.0,
            0.0,
            0,
            true,
            false,
        );

        let track = &engine.procs[&77];
        assert_eq!(
            track.start_time,
            Some(100),
            "coarse sysinfo time remains the same second"
        );
        assert_eq!(track.creation_filetime, Some(replacement_filetime));
        assert_eq!(track.cpu.len, 0, "transition-scan CPU is discarded");
        assert_eq!(track.mem.len, 0, "transition-scan memory is discarded");
        assert_eq!(track.io.len, 0, "no stale IO baseline is inherited");
        assert!(engine.current.anomalies.is_empty());
    }

    #[test]
    fn process_without_io_delta_does_not_advance_on_other_process_sample() {
        let mut engine = Engine::new();
        let baseline = ProcSample {
            pid: 88,
            start_time: None,
            name: "cached-io".into(),
            cpu: 5.0,
            mem_mb: 100.0,
            io_kbps: 1_000.0,
            io_sample_fresh: true,
        };
        let other = ProcSample {
            pid: 89,
            name: "fresh-io".into(),
            ..baseline.clone()
        };
        for _ in 0..ROBUST_WINDOW {
            engine.observe_elapsed_with_sample_freshness(
                std::time::Duration::from_secs(1),
                0.0,
                50.0,
                &[baseline.clone(), other.clone()],
                0,
                0.0,
                0.0,
                0,
                true,
                true,
            );
        }

        let cached = ProcSample {
            io_kbps: 0.0,
            io_sample_fresh: false,
            ..baseline
        };
        let fresh = ProcSample {
            io_kbps: 50_000.0,
            ..other
        };
        engine.observe_elapsed_with_sample_freshness(
            std::time::Duration::from_secs(1),
            0.0,
            50.0,
            &[cached, fresh],
            0,
            0.0,
            0.0,
            0,
            true,
            true,
        );
        assert_eq!(engine.procs[&88].io.len, ROBUST_WINDOW);
        assert_eq!(engine.procs[&88].io.streak, 0);
        assert_eq!(engine.procs[&89].io.streak, 1);
    }

    #[test]
    fn cold_high_load_is_baseline_and_intervals_equivalent() {
        let mut one = Engine::new();
        let mut two = Engine::new();
        for value in [80.0, 80.0, 10.0, 90.0] {
            for _ in 0..40 {
                let samples = || {
                    vec![ProcSample {
                        pid: 7,
                        start_time: None,
                        name: "fixture".into(),
                        cpu: value,
                        mem_mb: 900.0,
                        io_kbps: 50_000.0,
                        io_sample_fresh: true,
                    }]
                };
                one.observe_elapsed(
                    std::time::Duration::from_secs(1),
                    value,
                    85.0,
                    &samples(),
                    0,
                    0.0,
                    0.0,
                    0,
                );
                one.observe_elapsed(
                    std::time::Duration::from_secs(1),
                    value,
                    85.0,
                    &samples(),
                    0,
                    0.0,
                    0.0,
                    0,
                );
                two.observe_elapsed(
                    std::time::Duration::from_secs(2),
                    value,
                    85.0,
                    &samples(),
                    0,
                    0.0,
                    0.0,
                    0,
                );
                // Global histories are elapsed-time buckets; per-process confirmation
                // depends on actual inventory refresh cadence and may differ here.
                assert_eq!(one.cpu_avg(60), two.cpu_avg(60));
                assert_eq!(one.current.mem_slope, two.current.mem_slope);
                if value == 80.0 {
                    assert!(one.current.anomalies.is_empty());
                }
            }
        }
    }

    #[test]
    fn fractional_elapsed_only_advances_complete_second_buckets() {
        let mut e = Engine::new();
        for _ in 0..3 {
            e.observe_elapsed(
                std::time::Duration::from_millis(250),
                10.0,
                50.0,
                &[],
                0,
                0.0,
                0.0,
                0,
            );
        }
        assert_eq!(e.tick, 0);
        e.observe_elapsed(
            std::time::Duration::from_millis(250),
            20.0,
            60.0,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        assert_eq!(e.tick, 1);
        assert_eq!(e.cpu_hist, vec![20.0]);
        assert_eq!(e.cpu_peak(1), 20.0);
        e.observe_elapsed(
            std::time::Duration::from_millis(2750),
            30.0,
            70.0,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        assert_eq!(e.tick, 3);
        assert_eq!(e.remainder, std::time::Duration::from_millis(750));
        e.observe_elapsed(std::time::Duration::ZERO, 99.0, 99.0, &[], 0, 0.0, 0.0, 0);
        assert_eq!(e.tick, 3);
        e.observe_elapsed(
            std::time::Duration::from_secs(6),
            0.0,
            0.0,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        assert_eq!(e.remainder, std::time::Duration::ZERO);
        e.observe_elapsed(
            std::time::Duration::from_millis(250),
            0.0,
            0.0,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        assert_eq!(e.tick, 3);
    }

    #[test]
    fn pid_cleanup_handles_tick_saturation_without_retaining_dead_tracks() {
        let mut engine = Engine::new();
        engine.tick = u64::MAX - 1;
        engine.procs.insert(
            1,
            PidTrack {
                last_tick: u64::MAX - 60,
                ..Default::default()
            },
        );
        engine.procs.insert(
            2,
            PidTrack {
                last_tick: u64::MAX - 59,
                ..Default::default()
            },
        );
        engine.observe_samples(0.0, 0.0, &[], 0, 0.0, 0.0, 0, true, true, true);
        assert!(!engine.procs.contains_key(&1));
        assert!(engine.procs.contains_key(&2));
    }

    #[test]
    fn five_second_buckets_match_direct_global_and_process_diagnostics() {
        let mut direct = Engine::new();
        let mut elapsed = Engine::new();
        for high in [false, true, false] {
            for _ in 0..8 {
                let samples = || {
                    vec![ProcSample {
                        pid: 7,
                        start_time: None,
                        name: "fixture".into(),
                        cpu: if high { 90.0 } else { 10.0 },
                        mem_mb: if high { 2000.0 } else { 100.0 },
                        io_kbps: if high { 100_000.0 } else { 100.0 },
                        io_sample_fresh: true,
                    }]
                };
                for bucket in 0..5 {
                    direct.observe_samples(
                        80.0,
                        92.0,
                        &samples(),
                        12,
                        1.0,
                        95.0,
                        800_000,
                        bucket == 4,
                        bucket == 4,
                        bucket == 4,
                    );
                }
                elapsed.observe_elapsed_with_process_fresh(
                    std::time::Duration::from_secs(5),
                    80.0,
                    92.0,
                    &samples(),
                    12,
                    1.0,
                    95.0,
                    800_000,
                    true,
                );
                assert_eq!(direct.tick, elapsed.tick);
                assert_eq!(direct.cpu_hist, elapsed.cpu_hist);
                assert_eq!(direct.cpu_peak(60), elapsed.cpu_peak(60));
                assert_eq!(direct.mem_hist, elapsed.mem_hist);
                assert_eq!(direct.mem_observation_hist, elapsed.mem_observation_hist);
                // Direct bucket fixtures mark one real memory reading at the final bucket,
                // matching the batched five-second sensor observation.
                assert_eq!(
                    direct.current.mem_slope_sample_count,
                    elapsed.current.mem_slope_sample_count
                );
                assert_eq!(direct.current.score, elapsed.current.score);
                assert_eq!(
                    format!("{:?}", direct.current.deductions),
                    format!("{:?}", elapsed.current.deductions)
                );
                assert_eq!(
                    format!("{:?}", direct.current.anomalies),
                    format!("{:?}", elapsed.current.anomalies)
                );
                assert_eq!(direct.current.mem_slope, elapsed.current.mem_slope);
                assert_eq!(direct.current.mem_pct, elapsed.current.mem_pct);
                let a = &direct.procs[&7];
                let b = &elapsed.procs[&7];
                for (x, y) in [(&a.cpu, &b.cpu), (&a.mem, &b.mem), (&a.io, &b.io)] {
                    assert_eq!(
                        (x.values, x.len, x.next, x.center, x.scale, x.streak),
                        (y.values, y.len, y.next, y.center, y.scale, y.streak)
                    );
                }
            }
        }
    }

    #[test]
    fn invalid_global_cpu_samples_do_not_poison_or_stall_rolling_windows() {
        let mut engine = Engine::new();
        let observe_cpu = |engine: &mut Engine, cpu| {
            engine.observe(cpu, 50.0, Vec::new(), 0, 0.0, 0.0, 0);
        };
        for _ in 0..60 {
            observe_cpu(&mut engine, 80.0);
        }
        assert_eq!(engine.cpu_avg(10), 80.0);
        assert!(engine
            .current
            .deductions
            .iter()
            .any(|deduction| deduction.item == "CPU 持续高负载"));

        observe_cpu(&mut engine, f32::NAN);
        assert!(engine.cpu_hist.last().unwrap().is_nan());
        assert_eq!(engine.cpu_avg(10), 80.0);
        assert_eq!(engine.cpu_avg(60), 80.0);
        assert_eq!(engine.cpu_peak(60), 80.0);
        assert!(!engine.current.cpu_forecast_ready);
        assert!(engine
            .current
            .deductions
            .iter()
            .any(|deduction| deduction.item == "CPU 持续高负载"));

        for _ in 0..9 {
            observe_cpu(&mut engine, f32::INFINITY);
        }
        assert!(engine.cpu_avg(10).is_nan());
        assert_eq!(engine.cpu_avg(60), 80.0);
        for _ in 0..50 {
            observe_cpu(&mut engine, f32::NEG_INFINITY);
        }
        assert!(engine.cpu_avg(60).is_nan());
        assert_eq!(engine.cpu_peak(60), 0.0);

        observe_cpu(&mut engine, 120.0);
        assert_eq!(engine.cpu_avg(1), 100.0);
        assert_eq!(engine.cpu_peak(1), 100.0);
    }

    #[test]
    fn cpu_peak_shared_history_matches_legacy_across_rollover_and_gap() {
        let mut engine = Engine::new();
        let mut legacy_peak_history = Vec::new();
        let inputs = [
            0.0,
            25.0,
            150.0,
            -5.0,
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            80.0,
        ];
        let check_windows = |engine: &Engine, legacy: &[f32]| {
            for window in [0, 1, 10, 60, HIST_CAP, usize::MAX] {
                let count = window.min(legacy.len());
                let expected = legacy[legacy.len().saturating_sub(count)..]
                    .iter()
                    .copied()
                    .filter(|sample| sample.is_finite())
                    .fold(0.0, f32::max);
                assert_eq!(engine.cpu_peak(window), expected, "window={window}");
            }
        };

        for index in 0..(HIST_CAP + 29) {
            let cpu = inputs[index % inputs.len()];
            engine.observe_elapsed(
                std::time::Duration::from_secs(1),
                cpu,
                50.0,
                &[],
                0,
                0.0,
                0.0,
                0,
            );
            if legacy_peak_history.len() >= HIST_CAP {
                legacy_peak_history.remove(0);
            }
            legacy_peak_history.push(if cpu.is_finite() {
                cpu.clamp(0.0, 100.0)
            } else {
                f32::NAN
            });
            assert_eq!(engine.cpu_hist.len(), legacy_peak_history.len());
            assert!(engine
                .cpu_hist
                .iter()
                .zip(&legacy_peak_history)
                .all(|(actual, expected)| if expected.is_nan() {
                    actual.is_nan()
                } else {
                    actual.to_bits() == expected.to_bits()
                }));
            check_windows(&engine, &legacy_peak_history);
        }

        engine.observe_elapsed(
            std::time::Duration::from_secs(6),
            90.0,
            50.0,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        legacy_peak_history.clear();
        assert_eq!(engine.cpu_avg(HIST_CAP), 0.0);
        check_windows(&engine, &legacy_peak_history);

        engine.observe_elapsed(
            std::time::Duration::from_secs(1),
            75.0,
            50.0,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        legacy_peak_history.push(75.0);
        assert_eq!(engine.cpu_avg(1), 75.0);
        check_windows(&engine, &legacy_peak_history);
    }

    #[test]
    #[ignore = "manual Release microbenchmark for duplicate CPU history storage"]
    fn benchmark_cpu_history_shared_storage() {
        const BATCHES: usize = 101;
        const CALLS_PER_BATCH: usize = 20_000;
        let initial = vec![50.0; HIST_CAP];
        let samples = (0..CALLS_PER_BATCH)
            .map(|index| match index % 101 {
                0 => f32::NAN,
                1 => 100.0,
                value => value as f32,
            })
            .collect::<Vec<_>>();
        let measure_legacy = || {
            let mut cpu_history = initial.clone();
            let mut peak_history = initial.clone();
            let started = std::time::Instant::now();
            for sample in &samples {
                Engine::push(&mut cpu_history, *sample);
                Engine::push(&mut peak_history, *sample);
            }
            std::hint::black_box((&cpu_history, &peak_history));
            started.elapsed().as_nanos()
        };
        let measure_shared = || {
            let mut cpu_history = initial.clone();
            let started = std::time::Instant::now();
            for sample in &samples {
                Engine::push(&mut cpu_history, *sample);
            }
            std::hint::black_box(&cpu_history);
            started.elapsed().as_nanos()
        };
        for _ in 0..7 {
            std::hint::black_box(measure_legacy());
            std::hint::black_box(measure_shared());
        }
        let mut legacy_nanos = Vec::with_capacity(BATCHES);
        let mut shared_nanos = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                legacy_nanos.push(measure_legacy());
                shared_nanos.push(measure_shared());
            } else {
                shared_nanos.push(measure_shared());
                legacy_nanos.push(measure_legacy());
            }
        }
        legacy_nanos.sort_unstable();
        shared_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "CPU history push/rollover ({} samples, cap={}): two histories median/p95={:.1}/{:.1} us; shared={:.1}/{:.1}",
            CALLS_PER_BATCH,
            HIST_CAP,
            legacy_nanos[BATCHES / 2] as f64 / 1_000.0,
            legacy_nanos[p95_index] as f64 / 1_000.0,
            shared_nanos[BATCHES / 2] as f64 / 1_000.0,
            shared_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn centered_slope_preserves_small_trends_and_window_selection() {
        let hist: Vec<f32> = (0..180).map(|i| 90.0 + i as f32 / 1024.0).collect();
        assert!((Engine::slope(&hist, 45) - 1.0 / 1024.0).abs() < 1e-7);
        assert_eq!(Engine::slope(&[95.0; 45], 45), 0.0);
        assert_eq!(Engine::slope(&hist, 9), 0.0);
        let mut window = vec![0.0; 135];
        window.extend((0..45).map(|i| 90.0 - i as f32 * 0.25));
        assert_eq!(Engine::slope(&window, 45), -0.25);
    }

    #[test]
    fn theil_sen_cpu_forecast_resists_a_sparse_spike() {
        let mut rising: Vec<f32> = (0..CPU_FORECAST_WINDOW)
            .map(|index| 20.0 + index as f32 * 2.0)
            .collect();
        rising[6] = 100.0;
        let forecast = forecast_cpu(&cpu_samples_at_interval(&rising, 1.0));
        assert!(forecast.ready);
        assert!((forecast.slope_pct_per_sec - 2.0).abs() < 0.01);
        assert!((forecast.projected_10s_pct - 68.0).abs() < 0.1);
        assert!(forecast.stability >= 80.0);

        let mut steady = vec![45.0; CPU_FORECAST_WINDOW];
        steady[4] = 100.0;
        let forecast = forecast_cpu(&cpu_samples_at_interval(&steady, 1.0));
        assert!(forecast.slope_pct_per_sec.abs() < f32::EPSILON);
        assert!((forecast.projected_10s_pct - 45.0).abs() < f32::EPSILON);
        assert!(forecast.stability >= 80.0);
    }

    #[test]
    fn cpu_forecast_warms_up_safely_and_clamps_physical_range() {
        let short = [20.0; CPU_FORECAST_MIN_SAMPLES - 1];
        let short_forecast = forecast_cpu(&cpu_samples_at_interval(&short, 1.0));
        assert!(!short_forecast.ready);
        assert_eq!(short_forecast.sample_count, 9);
        assert_eq!(short_forecast.observed_span_secs, 8.0);
        let mut stale_tail = cpu_samples_at_interval(&[20.0; CPU_FORECAST_WINDOW], 1.0);
        stale_tail.last_mut().unwrap().cpu_pct = f32::NAN;
        assert!(!forecast_cpu(&stale_tail).ready);
        let mut invalid_head = cpu_samples_at_interval(&[30.0; CPU_FORECAST_WINDOW], 1.0);
        invalid_head[0].at_secs = f64::NAN;
        let recovered_head = forecast_cpu(&invalid_head);
        assert!(recovered_head.ready);
        assert_eq!(recovered_head.sample_count, 14);
        assert_eq!(recovered_head.observed_span_secs, 13.0);
        let rising: Vec<f32> = (0..CPU_FORECAST_WINDOW)
            .map(|index| 75.0 + index as f32 * 3.0)
            .collect();
        assert_eq!(
            forecast_cpu(&cpu_samples_at_interval(&rising, 1.0)).projected_10s_pct,
            100.0
        );
        let falling: Vec<f32> = (0..CPU_FORECAST_WINDOW)
            .map(|index| 25.0 - index as f32 * 3.0)
            .collect();
        assert_eq!(
            forecast_cpu(&cpu_samples_at_interval(&falling, 1.0)).projected_10s_pct,
            0.0
        );
        let mut even = [1.0, 3.0, 5.0, 7.0];
        assert_eq!(median_f32(&mut even), 4.0);
    }

    #[test]
    fn cpu_forecast_uses_independent_samples_and_elapsed_seconds() {
        let mut engine = Engine::new();
        for index in 0..5 {
            engine.observe_elapsed(
                std::time::Duration::from_secs(2),
                20.0 + index as f32 * 4.0,
                40.0,
                &[],
                0,
                0.0,
                0.0,
                0,
            );
        }
        assert_eq!(engine.cpu_hist.len(), 10);
        assert_eq!(engine.cpu_forecast_hist.len(), 5);
        assert!(!engine.current.cpu_forecast_ready);
        assert_eq!(engine.current.cpu_forecast_sample_count, 5);
        assert_eq!(engine.current.cpu_forecast_observed_span_secs, 8.0);
        let warming = serde_json::to_value(&engine.current).unwrap();
        assert_eq!(warming["cpu_forecast_sample_count"].as_u64(), Some(5));
        assert_eq!(
            warming["cpu_forecast_observed_span_secs"].as_f64(),
            Some(8.0)
        );

        for index in 5..10 {
            engine.observe_elapsed(
                std::time::Duration::from_secs(2),
                20.0 + index as f32 * 4.0,
                40.0,
                &[],
                0,
                0.0,
                0.0,
                0,
            );
        }
        assert!(engine.current.cpu_forecast_ready);
        assert_eq!(engine.current.cpu_forecast_sample_count, 10);
        assert_eq!(engine.current.cpu_forecast_observed_span_secs, 18.0);
        let ready = serde_json::to_value(&engine.current).unwrap();
        assert_eq!(ready["cpu_forecast_sample_count"].as_u64(), Some(10));
        assert_eq!(
            ready["cpu_forecast_observed_span_secs"].as_f64(),
            Some(18.0)
        );
        assert!((engine.current.cpu_trend_pct_per_sec - 2.0).abs() < 0.01);
        assert!((engine.current.cpu_forecast_10s_pct - 76.0).abs() < 0.1);
    }

    #[test]
    fn cpu_forecast_requires_independent_monotonic_timestamps() {
        let repeated: Vec<CpuForecastSample> = (0..CPU_FORECAST_WINDOW)
            .map(|index| {
                let at_secs = (index / 2) as f64;
                CpuForecastSample {
                    at_secs,
                    cpu_pct: 20.0 + at_secs as f32 * 5.0,
                }
            })
            .collect();
        let repeated_forecast = forecast_cpu(&repeated);
        assert!(!repeated_forecast.ready);
        assert_eq!(repeated_forecast.sample_count, 8);
        assert_eq!(repeated_forecast.observed_span_secs, 7.0);

        let ten_distinct: Vec<CpuForecastSample> = (0..CPU_FORECAST_WINDOW)
            .map(|index| {
                let at_secs = if index < 10 {
                    (index / 2) as f64
                } else {
                    (index - 5) as f64
                };
                CpuForecastSample {
                    at_secs,
                    cpu_pct: 20.0 + at_secs as f32 * 2.0,
                }
            })
            .collect();
        let ten_forecast = forecast_cpu(&ten_distinct);
        assert!(
            ten_forecast.ready,
            "ten independent times meet the warm-up gate"
        );
        assert_eq!(ten_forecast.sample_count, 10);

        let mut out_of_order = cpu_samples_at_interval(&[20.0; CPU_FORECAST_WINDOW], 1.0);
        out_of_order.swap(5, 6);
        assert!(
            !forecast_cpu(&out_of_order).ready,
            "out-of-order timestamps invalidate the forecast"
        );
    }

    #[test]
    fn cpu_forecast_requires_a_minimum_observation_span() {
        let compressed_values: Vec<f32> = (0..CPU_FORECAST_WINDOW)
            .map(|index| 20.0 + index as f32 * 0.1)
            .collect();
        let compressed = cpu_samples_at_interval(&compressed_values, 0.1);
        let compressed_forecast = forecast_cpu(&compressed);
        assert!(!compressed_forecast.ready);
        assert_eq!(compressed_forecast.sample_count, 15);
        assert!((compressed_forecast.observed_span_secs - 1.4).abs() < 0.00001);

        let boundary_values: Vec<f32> = (0..=CPU_FORECAST_MIN_SAMPLES)
            .map(|index| 30.0 + index as f32 * 0.5)
            .collect();
        let boundary = cpu_samples_at_interval(&boundary_values, 0.5);
        let boundary_forecast = forecast_cpu(&boundary);
        assert!(
            boundary_forecast.ready,
            "five seconds of independent samples meets the span gate"
        );
        assert_eq!(boundary_forecast.sample_count, 11);
        assert_eq!(
            boundary_forecast.observed_span_secs,
            CPU_FORECAST_MIN_SPAN_SECS
        );
    }

    #[test]
    fn floor_blocks_low_value_spikes() {
        let mut d = Dim::default();
        for _ in 0..40 {
            d.update(1.0, 2.0, 4, 15.0, 0);
        }
        // 5% 虽然远超基线，但低于地板值 15%，不算异常
        for _ in 0..10 {
            assert!(!d.update(5.0, 2.0, 4, 15.0, 0));
        }
    }

    #[test]
    fn slope_tracks_rising_memory() {
        let mut e = Engine::new();
        // 水位从 60% 爬升到 ~86%：既有斜率又过阈值
        for i in 0..90 {
            e.observe(0.0, 60.0 + i as f32 * 0.3, vec![], 0, 0.0, 0.0, 0);
        }
        assert!(e.current.mem_slope > 0.2, "slope = {}", e.current.mem_slope);
        // 高水位 + 急涨 → 智能整理应放行
        assert!(e.trim_needed().is_ok());
    }

    #[test]
    fn memory_slope_caps_window_at_pair_array_capacity() {
        let samples = (0..100)
            .map(|bucket| 40.0 + bucket as f32 * 0.1)
            .collect::<Vec<_>>();
        let slope = Engine::slope(&samples, usize::MAX);
        assert!((slope - 0.1).abs() < 0.001, "slope = {slope}");
    }

    #[test]
    #[ignore = "manual synthetic maximum-window memory-slope benchmark"]
    fn benchmark_memory_slope_45_sample_window() {
        const BATCHES: usize = 101;
        const CALLS_PER_BATCH: usize = 500;
        let values = (0..MEMORY_SLOPE_WINDOW)
            .map(|index| 65.0 + index as f32 * 0.25 + ((index * 17) % 7) as f32 * 0.01)
            .collect::<Vec<_>>();

        for _ in 0..5 {
            for _ in 0..CALLS_PER_BATCH {
                std::hint::black_box(Engine::slope(
                    std::hint::black_box(&values),
                    MEMORY_SLOPE_WINDOW,
                ));
            }
        }

        let mut micros_per_call = Vec::with_capacity(BATCHES);
        let mut sink = 0.0f32;
        for _ in 0..BATCHES {
            let started = std::time::Instant::now();
            for _ in 0..CALLS_PER_BATCH {
                sink += std::hint::black_box(Engine::slope(
                    std::hint::black_box(&values),
                    MEMORY_SLOPE_WINDOW,
                ));
            }
            micros_per_call
                .push(started.elapsed().as_secs_f64() * 1_000_000.0 / CALLS_PER_BATCH as f64);
        }
        std::hint::black_box(sink);
        micros_per_call.sort_by(f64::total_cmp);
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "memory slope 45-sample benchmark: median={:.3} us/call, p95={:.3} us/call",
            micros_per_call[BATCHES / 2],
            micros_per_call[p95_index],
        );
    }

    #[test]
    fn memory_slope_resists_a_two_bucket_in_range_pressure_spike() {
        let mut engine = Engine::new();
        for second in 0..45 {
            let mem_pct = if second >= 43 { 89.0 } else { 78.0 };
            engine.observe(0.0, mem_pct, vec![], 0, 0.0, 0.0, 0);
        }
        assert!(engine.current.mem_slope_ready);
        assert_eq!(engine.current.mem_pct, 89.0);
        assert!(
            engine.current.mem_slope.abs() < 0.0001,
            "slope = {}",
            engine.current.mem_slope
        );
        assert_eq!(engine.trim_needed(), Err("内存平稳，无需干预"));
    }

    #[test]
    fn memory_slope_ignores_invalid_samples_without_compressing_time() {
        let mut reference = Engine::new();
        let mut with_gaps = Engine::new();
        for second in 0..45 {
            let valid = 55.0 + second as f32 * 0.1;
            reference.observe(0.0, valid, vec![], 0, 0.0, 0.0, 0);
            let sample = match second {
                12 => f32::NAN,
                25 => f32::INFINITY,
                36 => 100.5,
                _ => valid,
            };
            with_gaps.observe(0.0, sample, vec![], 0, 0.0, 0.0, 0);
        }
        assert!((reference.current.mem_slope - 0.1).abs() < 0.001);
        assert!((with_gaps.current.mem_slope - reference.current.mem_slope).abs() < 0.001);
        assert!(reference.current.mem_slope_ready);
        assert!(with_gaps.current.mem_slope_ready);
        assert_eq!(reference.current.mem_slope_sample_count, 45);
        assert_eq!(with_gaps.current.mem_slope_sample_count, 42);
    }

    #[test]
    fn unavailable_memory_capacity_does_not_create_a_false_rising_eta_after_recovery() {
        let mut engine = Engine::new();
        for _ in 0..25 {
            engine.observe(0.0, f32::NAN, vec![], 0, 0.0, 0.0, 0);
        }
        assert_eq!(engine.current.mem_slope_sample_count, 0);
        assert!(!engine.current.mem_slope_ready);
        assert_eq!(engine.current.mem_critical_eta_secs, None);
        assert!(serde_json::to_value(&engine.current).unwrap()["mem_pct"].is_null());

        for _ in 0..20 {
            engine.observe(0.0, 80.0, vec![], 0, 0.0, 0.0, 0);
        }
        assert_eq!(engine.current.mem_pct, 80.0);
        assert_eq!(engine.current.mem_slope_sample_count, 20);
        assert!(engine.current.mem_slope_ready);
        assert!(engine.current.mem_slope.abs() < 0.0001);
        assert_eq!(engine.current.mem_critical_eta_secs, None);
    }

    #[test]
    fn trim_reports_insufficient_valid_memory_trend_samples() {
        let mut engine = Engine::new();
        for bucket in 0..15 {
            let sample = match bucket {
                0..=7 => 80.0 + bucket as f32 * 0.5,
                14 => 84.0,
                _ => f32::NAN,
            };
            engine.observe(0.0, sample, vec![], 0, 0.0, 0.0, 0);
        }
        assert_eq!(
            valid_memory_observation_count(
                &engine.mem_hist,
                &engine.mem_observation_hist,
                MEMORY_SLOPE_WINDOW,
            ),
            9
        );
        assert_eq!(engine.current.mem_slope_sample_count, 9);
        assert!(!engine.current.mem_slope_ready);
        assert_eq!(engine.current.mem_slope, 0.0);
        assert_eq!(
            engine.trim_needed(),
            Err("内存趋势有效样本不足（至少 10 个）")
        );

        engine.observe(0.0, 84.5, vec![], 0, 0.0, 0.0, 0);
        assert_eq!(
            valid_memory_observation_count(
                &engine.mem_hist,
                &engine.mem_observation_hist,
                MEMORY_SLOPE_WINDOW,
            ),
            10
        );
        assert_eq!(engine.current.mem_slope_sample_count, 10);
        assert!(engine.current.mem_slope_ready);
        assert!(engine.current.mem_slope > 0.05);
        assert!(engine.trim_needed().is_ok());
    }

    #[test]
    fn diagnostics_json_exposes_memory_sample_progress_and_readiness() {
        let mut engine = Engine::new();
        for sample in 0..9 {
            engine.observe(0.0, 78.0 + sample as f32 * 0.1, vec![], 0, 0.0, 0.0, 0);
        }
        let warming = serde_json::to_value(&engine.current).unwrap();
        assert_eq!(
            warming
                .get("mem_slope_sample_count")
                .and_then(serde_json::Value::as_u64),
            Some(9)
        );
        assert_eq!(
            warming
                .get("mem_slope_ready")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );

        engine.observe(0.0, 78.9, vec![], 0, 0.0, 0.0, 0);
        let ready = serde_json::to_value(&engine.current).unwrap();
        assert_eq!(
            ready
                .get("mem_slope_sample_count")
                .and_then(serde_json::Value::as_u64),
            Some(10)
        );
        assert_eq!(
            ready
                .get("mem_slope_ready")
                .and_then(serde_json::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn memory_critical_eta_is_bounded_to_confident_near_term_rises() {
        assert_eq!(memory_critical_eta_secs(84.0, 0.25, 20), Some(24));
        assert_eq!(memory_critical_eta_secs(84.0, 0.25, 19), None);
        assert_eq!(memory_critical_eta_secs(81.0, 0.051, 20), Some(177));
        assert_eq!(memory_critical_eta_secs(78.0, 0.06, 20), None);
        assert_eq!(memory_critical_eta_secs(81.0, 0.05, 20), None);
        assert_eq!(memory_critical_eta_secs(77.99, 0.25, 20), None);
        assert_eq!(memory_critical_eta_secs(90.0, 0.25, 20), None);
        assert_eq!(memory_critical_eta_secs(f32::NAN, 0.25, 20), None);
        assert_eq!(memory_critical_eta_secs(84.0, f32::INFINITY, 20), None);
        assert_eq!(memory_critical_eta_secs(84.0, -0.25, 20), None);
    }

    #[test]
    fn memory_critical_eta_waits_for_twenty_samples_and_serializes() {
        let mut engine = Engine::new();
        for sample in 0..19 {
            engine.observe(0.0, 80.0 + sample as f32 * 0.25, vec![], 0, 0.0, 0.0, 0);
        }
        assert_eq!(engine.current.mem_slope_sample_count, 19);
        assert_eq!(engine.current.mem_critical_eta_secs, None);
        let warming = serde_json::to_value(&engine.current).unwrap();
        assert!(warming["mem_critical_eta_secs"].is_null());

        engine.observe(0.0, 84.75, vec![], 0, 0.0, 0.0, 0);
        assert_eq!(engine.current.mem_slope_sample_count, 20);
        assert!(engine.current.mem_slope_ready);
        assert_eq!(engine.current.mem_critical_eta_secs, Some(21));
        let ready = serde_json::to_value(&engine.current).unwrap();
        assert_eq!(ready["mem_critical_eta_secs"].as_u64(), Some(21));

        for _ in 0..MEMORY_SLOPE_WINDOW {
            engine.observe(0.0, 84.75, vec![], 0, 0.0, 0.0, 0);
        }
        assert_eq!(engine.current.mem_slope, 0.0);
        assert_eq!(engine.current.mem_critical_eta_secs, None);
    }

    #[test]
    fn memory_eta_requires_distinct_readings_across_two_second_tray_buckets() {
        let mut engine = Engine::new();
        for sample in 0..10 {
            engine.observe_elapsed(
                std::time::Duration::from_secs(2),
                0.0,
                79.0 + sample as f32 * 0.5,
                &[],
                0,
                0.0,
                0.0,
                0,
            );
        }
        assert_eq!(
            engine.mem_hist.len(),
            20,
            "two elapsed buckets are retained per tray sample"
        );
        assert_eq!(
            engine.current.mem_slope_sample_count, 10,
            "duplicate buckets do not count as new readings"
        );
        assert!(engine.current.mem_slope_ready);
        assert_eq!(engine.current.mem_critical_eta_secs, None);

        for sample in 10..19 {
            engine.observe_elapsed(
                std::time::Duration::from_secs(2),
                0.0,
                79.0 + sample as f32 * 0.5,
                &[],
                0,
                0.0,
                0.0,
                0,
            );
        }
        assert_eq!(engine.current.mem_slope_sample_count, 19);
        assert_eq!(engine.current.mem_critical_eta_secs, None);

        engine.observe_elapsed(
            std::time::Duration::from_secs(2),
            0.0,
            88.5,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        assert_eq!(engine.current.mem_slope_sample_count, 20);
        assert!(engine.current.mem_slope > MEMORY_RISING_SLOPE_THRESHOLD);
        assert!(engine.current.mem_critical_eta_secs.is_some());
    }

    #[test]
    fn memory_trend_confidence_ignores_reused_sensor_cache_values() {
        let mut engine = Engine::new();
        for sample in 0..10 {
            let mem_pct = 79.0 + sample as f32 * 0.5;
            engine.observe_elapsed_with_memory_sample_freshness(
                std::time::Duration::from_secs(1),
                0.0,
                mem_pct,
                &[],
                0,
                0.0,
                0.0,
                0,
                true,
                true,
                true,
            );
            engine.observe_elapsed_with_memory_sample_freshness(
                std::time::Duration::from_secs(1),
                0.0,
                mem_pct,
                &[],
                0,
                0.0,
                0.0,
                0,
                true,
                true,
                false,
            );
        }
        assert_eq!(engine.mem_hist.len(), 20);
        assert_eq!(engine.current.mem_slope_sample_count, 10);
        assert_eq!(engine.current.mem_critical_eta_secs, None);
    }

    #[test]
    fn trim_rejects_invalid_current_memory_even_with_a_rising_history() {
        let mut engine = Engine::new();
        for second in 0..20 {
            engine.observe(0.0, 80.0 + second as f32 * 0.3, vec![], 0, 0.0, 0.0, 0);
        }
        assert!(engine.current.mem_slope > 0.05);
        engine.observe(0.0, f32::NAN, vec![], 0, 0.0, 0.0, 0);
        assert!(engine.current.mem_slope_ready);
        assert!(engine.current.mem_slope > 0.05);
        assert_eq!(engine.trim_needed(), Err("内存水位数据暂不可用"));
    }

    #[test]
    fn trim_skipped_when_plenty() {
        let mut e = Engine::new();
        for _ in 0..60 {
            e.observe(0.0, 40.0, vec![], 0, 0.0, 0.0, 0);
        }
        assert!(e.trim_needed().is_err());
    }

    #[test]
    fn first_cpu_and_memory_observation_is_kept_after_startup() {
        let mut engine = Engine::new();
        engine.observe_elapsed(
            std::time::Duration::from_secs(1),
            32.0,
            61.5,
            &[],
            0,
            0.0,
            0.0,
            0,
        );

        assert_eq!(engine.current.mem_pct, 61.5);
        assert_eq!(engine.current.mem_slope_sample_count, 1);
        assert_eq!(engine.mem_observation_hist, vec![true]);
        assert_eq!(engine.current.cpu_forecast_sample_count, 1);
    }

    #[test]
    fn sampling_gap_invalidates_all_derived_diagnostics() {
        let mut e = Engine::new();
        for sample in 0..45 {
            e.observe(
                30.0,
                80.0 + sample as f32 * 0.1,
                vec![],
                20,
                2.0,
                97.0,
                3600,
            );
        }
        assert!(e.current.score > 0);
        assert!((78.0..90.0).contains(&e.current.mem_pct));
        assert!(e.current.mem_slope_ready);
        assert!(e.current.mem_slope > 0.05);
        let eta_before_gap = e
            .current
            .mem_critical_eta_secs
            .expect("confident memory ETA");
        let serialized_before_gap = serde_json::to_value(&e.current).unwrap();
        assert_eq!(
            serialized_before_gap["mem_critical_eta_secs"].as_u64(),
            Some(u64::from(eta_before_gap))
        );
        assert!(!e.current.deductions.is_empty());
        assert!(e.current.cpu_forecast_ready);
        assert!(e.current.cpu_forecast_sample_count > 0);
        assert!(e.current.cpu_forecast_observed_span_secs > 0.0);
        e.observe_elapsed(
            std::time::Duration::from_secs(6),
            0.0,
            0.0,
            &[],
            0,
            0.0,
            0.0,
            0,
        );
        assert_eq!(e.current.score, 0);
        assert_eq!(e.current.mem_pct, 0.0);
        assert_eq!(e.current.mem_slope, 0.0);
        assert!(!e.current.mem_slope_ready);
        assert_eq!(e.current.mem_slope_sample_count, 0);
        assert_eq!(e.current.mem_critical_eta_secs, None);
        assert!(e.mem_hist.is_empty());
        assert!(e.mem_observation_hist.is_empty());
        assert!(!e.current.cpu_forecast_ready);
        assert_eq!(e.current.cpu_forecast_sample_count, 0);
        assert_eq!(e.current.cpu_forecast_observed_span_secs, 0.0);
        assert_eq!(e.current.cpu_forecast_10s_pct, 0.0);
        assert!(e.cpu_forecast_hist.is_empty());
        assert!(e.current.deductions.is_empty());
        assert!(e.current.anomalies.is_empty());
        let serialized_gap = serde_json::to_value(&e.current).unwrap();
        assert_eq!(serialized_gap["cpu_forecast_ready"].as_bool(), Some(false));
        assert_eq!(
            serialized_gap["cpu_forecast_sample_count"].as_u64(),
            Some(0)
        );
        assert_eq!(
            serialized_gap["cpu_forecast_observed_span_secs"].as_f64(),
            Some(0.0)
        );
        assert!(serialized_gap["mem_critical_eta_secs"].is_null());
        assert!(e.trim_needed().is_err());

        for _ in 0..10 {
            e.observe(30.0, 92.0, vec![], 20, 2.0, 97.0, 3600);
        }
        assert!(e.current.cpu_forecast_ready);
        assert!(e.current.cpu_forecast_sample_count >= 10);
        assert!(e.current.cpu_forecast_observed_span_secs >= 5.0);
        let serialized_recovery = serde_json::to_value(&e.current).unwrap();
        assert_eq!(
            serialized_recovery["cpu_forecast_ready"].as_bool(),
            Some(true)
        );
    }

    #[test]
    fn health_score_deducts_disk_pressure() {
        let mut e = Engine::new();
        for _ in 0..10 {
            e.observe(10.0, 50.0, vec![], 0, 0.0, 95.0, 3600);
        }
        assert!(e.current.score < 95, "score = {}", e.current.score);
        assert!(e.current.deductions.iter().any(|d| d.item.contains("C 盘")));
    }
}
