// 常驻模式学习：按进程名低频累积观测样本（内存聚合 / CPU 活跃度 / 更新器标记），
// 跨天后识别"长期驻留、低活跃、占用大"的进程并给出建议。
// 红线：只统计 AeroTune 运行期间的观测（不代表全天行为）；只建议不处置，
// 任何动作由用户在进程管理中自行决定；数据只存本机 residency-patterns.json。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ResidencyTrack {
    /// "updater-suspect"（名称启发式）或 "app"。
    pub role_hint: String,
    /// 本地日期（yyyy-mm-dd）→ 当日观测次数。
    pub days: HashMap<String, u32>,
    pub total_samples: u64,
    /// 聚合 CPU ≤ 阈值的样本数（低活跃）。
    pub low_cpu_samples: u64,
    pub sum_mem_mb: f64,
    pub max_mem_mb: f64,
    pub last_seen_unix: u64,
}

pub type ResidencyData = HashMap<String, ResidencyTrack>;

pub const RESIDENCY_OBSERVE_INTERVAL_SECS: u64 = 30 * 60;
/// 建议门槛：跨天数、样本数、平均内存、低活跃占比同时达标。
pub const SUGGESTION_MIN_DAYS: usize = 3;
pub const SUGGESTION_MIN_SAMPLES: u64 = 8;
pub const SUGGESTION_MIN_AVG_MEM_MB: f64 = 128.0;
pub const SUGGESTION_MIN_LOW_CPU_RATIO: f64 = 0.8;
/// 单次观测判定的低活跃 CPU 上限（全机口径，聚合多实例取最大）。
const LOW_CPU_THRESHOLD_PCT: f32 = 2.0;
const SUGGESTION_MAX_ITEMS: usize = 5;
const STORE_MAX_ENTRIES: usize = 128;
const STALE_ENTRY_SECS: u64 = 30 * 24 * 3600;

#[derive(Serialize, Clone, Debug)]
pub struct ResidencySuggestion {
    pub name: String,
    pub role_hint: String,
    pub resident_days: u32,
    pub avg_mem_mb: f64,
    pub low_cpu_ratio: f64,
    pub last_seen_unix: u64,
}

pub struct ResidencyStore {
    data: ResidencyData,
    path: PathBuf,
}

impl ResidencyStore {
    pub fn load() -> Self {
        Self::load_from(crate::util::app_dir().join("residency-patterns.json"))
    }

    pub fn load_from(path: PathBuf) -> Self {
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<ResidencyData>(&bytes).ok())
            .unwrap_or_default();
        let mut store = Self { data, path };
        store.enforce_capacity();
        store
    }

    fn enforce_capacity(&mut self) {
        while self.data.len() > STORE_MAX_ENTRIES {
            let oldest = self
                .data
                .iter()
                .min_by_key(|(_, track)| track.last_unix_key())
                .map(|(name, _)| name.clone());
            match oldest {
                Some(name) => {
                    self.data.remove(&name);
                }
                None => break,
            }
        }
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("常驻观测目录创建失败: {e}"))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.data)
            .map_err(|e| format!("常驻观测序列化失败: {e}"))?;
        crate::util::atomic_write(&self.path, &bytes)
    }

    pub fn dismiss(&mut self, name: &str) -> bool {
        self.data.remove(name).is_some()
    }
}

impl ResidencyTrack {
    fn last_unix_key(&self) -> u64 {
        self.last_seen_unix
    }

    fn observe(&mut self, mem_mb: f64, cpu_pct: f32, updater_hint: bool, day: &str, now_unix: u64) {
        if self.role_hint != "updater-suspect" && updater_hint {
            self.role_hint = "updater-suspect".into();
        }
        *self.days.entry(day.to_string()).or_insert(0) += 1;
        self.total_samples = self.total_samples.saturating_add(1);
        if cpu_pct <= LOW_CPU_THRESHOLD_PCT {
            self.low_cpu_samples = self.low_cpu_samples.saturating_add(1);
        }
        self.sum_mem_mb += mem_mb.max(0.0);
        self.max_mem_mb = self.max_mem_mb.max(mem_mb);
        self.last_seen_unix = now_unix;
    }
}

fn is_updater_name(normalized: &str) -> bool {
    ["update", "setup", "install", "upgrade"]
        .iter()
        .any(|marker| normalized.contains(marker))
}

/// 纯函数：把一次进程快照按名称聚合（内存求和、CPU 取最大），更新观测表。
/// 系统名单、AeroTune 自身与空名跳过；返回是否有变化。
pub fn observe_samples(
    data: &mut ResidencyData,
    samples: &[crate::diag::ProcSample],
    day: &str,
    now_unix: u64,
) -> bool {
    // 聚合键用归一化名称：同名不同大小写的实例合并为一条观测。
    let mut aggregated: HashMap<String, (f64, f32, bool)> = HashMap::new();
    for sample in samples {
        let normalized = crate::processes::normalized_process_name(&sample.name);
        if normalized.is_empty()
            || normalized.starts_with("aerotune")
            || crate::processes::PROTECTED_PROCESSES.contains(&normalized.as_str())
        {
            continue;
        }
        let entry = aggregated
            .entry(normalized.clone())
            .or_insert((0.0, f32::MIN, false));
        entry.0 += sample.mem_mb.max(0.0);
        entry.1 = entry.1.max(sample.cpu.max(0.0));
        entry.2 = entry.2 || is_updater_name(&normalized);
    }
    let mut changed = false;
    for (name, (mem_mb, cpu, updater)) in aggregated {
        data.entry(name)
            .or_default()
            .observe(mem_mb, cpu, updater, day, now_unix);
        changed = true;
    }
    if changed {
        enforce_capacity_on(data);
    }
    changed
}

fn enforce_capacity_on(data: &mut ResidencyData) {
    while data.len() > STORE_MAX_ENTRIES {
        let oldest = data
            .iter()
            .min_by_key(|(_, track)| track.last_seen_unix)
            .map(|(name, _)| name.clone());
        match oldest {
            Some(name) => {
                data.remove(&name);
            }
            None => break,
        }
    }
}

/// 纯函数：建议规则——跨 ≥3 天、≥8 次观测、平均内存 ≥128MB、低活跃占比 ≥80%。
pub fn eligible_suggestions(data: &ResidencyData, now_unix: u64) -> Vec<ResidencySuggestion> {
    let mut suggestions: Vec<ResidencySuggestion> = data
        .iter()
        .filter(|(_, track)| {
            let avg = if track.total_samples > 0 {
                track.sum_mem_mb / track.total_samples as f64
            } else {
                0.0
            };
            let low_ratio = if track.total_samples > 0 {
                track.low_cpu_samples as f64 / track.total_samples as f64
            } else {
                0.0
            };
            track.days.len() >= SUGGESTION_MIN_DAYS
                && track.total_samples >= SUGGESTION_MIN_SAMPLES
                && avg >= SUGGESTION_MIN_AVG_MEM_MB
                && low_ratio >= SUGGESTION_MIN_LOW_CPU_RATIO
                && now_unix.saturating_sub(track.last_seen_unix) <= STALE_ENTRY_SECS
        })
        .map(|(name, track)| {
            let avg = track.sum_mem_mb / track.total_samples.max(1) as f64;
            let low_ratio = track.low_cpu_samples as f64 / track.total_samples.max(1) as f64;
            ResidencySuggestion {
                name: name.clone(),
                role_hint: track.role_hint.clone(),
                resident_days: track.days.len() as u32,
                avg_mem_mb: (avg * 10.0).round() / 10.0,
                low_cpu_ratio: (low_ratio * 100.0).round() / 100.0,
                last_seen_unix: track.last_seen_unix,
            }
        })
        .collect();
    suggestions.sort_by(|a, b| {
        b.avg_mem_mb
            .total_cmp(&a.avg_mem_mb)
            .then_with(|| a.name.cmp(&b.name))
    });
    suggestions.truncate(SUGGESTION_MAX_ITEMS);
    suggestions
}

/// 淘汰 30 天无新观测的条目；返回淘汰数。
pub fn prune_stale(data: &mut ResidencyData, now_unix: u64) -> usize {
    let stale: Vec<String> = data
        .iter()
        .filter(|(_, track)| now_unix.saturating_sub(track.last_seen_unix) > STALE_ENTRY_SECS)
        .map(|(name, _)| name.clone())
        .collect();
    let removed = stale.len();
    for name in stale {
        data.remove(&name);
    }
    removed
}

static RESIDENCY_STORE: OnceLock<Mutex<ResidencyStore>> = OnceLock::new();

fn store() -> &'static Mutex<ResidencyStore> {
    RESIDENCY_STORE.get_or_init(|| Mutex::new(ResidencyStore::load()))
}

/// 采样线程的低频观测入口（默认每 30 分钟一次）。
/// 落盘失败只记日志，绝不影响采样主流程。
pub fn observe_from_sampler(samples: &[crate::diag::ProcSample]) {
    let now_unix = chrono::Utc::now().timestamp().max(0) as u64;
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let changed = observe_samples(&mut guard.data, samples, &day, now_unix);
    let pruned = prune_stale(&mut guard.data, now_unix);
    if changed || pruned > 0 {
        if let Err(error) = guard.save() {
            tracing::warn!(error = %error, "常驻观测持久化失败（内存数据仍有效）");
        }
    }
}

/// 只读命令：当前可建议关注的长期驻留进程。
#[tauri::command]
pub fn get_residency_suggestions() -> Vec<ResidencySuggestion> {
    snapshot_residency()
}

/// 供智能建议管线复用的快照（与命令同一门槛，无副作用）。
pub(crate) fn snapshot_residency() -> Vec<ResidencySuggestion> {
    let now_unix = chrono::Utc::now().timestamp().max(0) as u64;
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    eligible_suggestions(&guard.data, now_unix)
}

/// 忽略命令：移除一条常驻观测（用户不想要的建议）。
#[tauri::command]
pub fn dismiss_residency_suggestion(name: String) -> Result<bool, String> {
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let removed = guard.dismiss(&name);
    if removed {
        guard.save()?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(pid: u32, name: &str, mem_mb: f64, cpu: f32) -> crate::diag::ProcSample {
        crate::diag::ProcSample {
            pid,
            start_time: None,
            name: name.into(),
            cpu,
            mem_mb,
            io_kbps: 0.0,
            io_sample_fresh: false,
        }
    }

    #[test]
    fn observe_aggregates_same_name_and_skips_protected() {
        let mut data = ResidencyData::new();
        let samples = vec![
            sample(1, "Chrome.exe", 800.0, 1.0),
            sample(2, "chrome.exe", 400.0, 0.5),
            sample(3, "svchost.exe", 300.0, 0.0),
            sample(4, "aerotune.exe", 100.0, 2.0),
        ];
        assert!(observe_samples(&mut data, &samples, "2026-10-01", 1000));
        assert_eq!(data.len(), 1, "only chrome survives");
        let chrome = &data["chrome.exe"];
        assert_eq!(chrome.days["2026-10-01"], 1);
        assert!(
            (chrome.sum_mem_mb - 1200.0).abs() < 0.01,
            "memory aggregates across instances"
        );
        assert_eq!(chrome.low_cpu_samples, 1, "max cpu 1.0 counts as low");
    }

    #[test]
    fn updater_hint_is_sticky_and_low_cpu_threshold_uses_max() {
        let mut data = ResidencyData::new();
        observe_samples(
            &mut data,
            &[sample(1, "tool.exe", 500.0, 0.5)],
            "2026-10-01",
            1000,
        );
        observe_samples(
            &mut data,
            &[sample(1, "tool.exe", 500.0, 9.0)],
            "2026-10-02",
            2000,
        );
        let tool = &data["tool.exe"];
        assert_eq!(tool.low_cpu_samples, 1, "9% cpu sample is not low");
        assert_eq!(tool.total_samples, 2);
        observe_samples(
            &mut data,
            &[sample(1, "tool updater.exe", 500.0, 0.1)],
            "2026-10-03",
            3000,
        );
        let _ = data.remove("tool.exe");
        let updater = &data["tool updater.exe"];
        assert_eq!(updater.role_hint, "updater-suspect");
    }

    #[test]
    fn suggestions_require_all_thresholds() {
        let mut data = ResidencyData::new();
        // 差一天
        for day in ["2026-10-01", "2026-10-02"] {
            for _ in 0..4 {
                observe_samples(
                    &mut data,
                    &[sample(1, "few-days.exe", 300.0, 0.5)],
                    day,
                    2000,
                );
            }
        }
        // 内存不足
        for (index, day) in ["2026-10-01", "2026-10-02", "2026-10-03"]
            .iter()
            .enumerate()
        {
            for _ in 0..3 {
                observe_samples(
                    &mut data,
                    &[sample(1, "small.exe", 60.0, 0.5)],
                    day,
                    1000 + index as u64,
                );
            }
        }
        // CPU 活跃
        for (index, day) in ["2026-10-01", "2026-10-02", "2026-10-03"]
            .iter()
            .enumerate()
        {
            for _ in 0..3 {
                observe_samples(
                    &mut data,
                    &[sample(1, "busy.exe", 900.0, 15.0)],
                    day,
                    1000 + index as u64,
                );
            }
        }
        // 达标
        for (index, day) in ["2026-10-01", "2026-10-02", "2026-10-03"]
            .iter()
            .enumerate()
        {
            for _ in 0..3 {
                observe_samples(
                    &mut data,
                    &[sample(1, "resident.exe", 600.0, 0.5)],
                    day,
                    1000 + index as u64,
                );
            }
        }
        let suggestions = eligible_suggestions(&data, 1000 + STALE_ENTRY_SECS);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "resident.exe");
        assert_eq!(suggestions[0].resident_days, 3);
        assert!(suggestions[0].avg_mem_mb >= SUGGESTION_MIN_AVG_MEM_MB);
    }

    #[test]
    fn stale_entries_are_pruned_and_not_suggested() {
        let mut data = ResidencyData::new();
        for (index, day) in ["2026-08-01", "2026-08-02", "2026-08-03"]
            .iter()
            .enumerate()
        {
            observe_samples(
                &mut data,
                &[sample(1, "old.exe", 500.0, 0.5)],
                day,
                100 + index as u64,
            );
        }
        let now = 100 + STALE_ENTRY_SECS + 10;
        assert!(eligible_suggestions(&data, now).is_empty());
        assert_eq!(prune_stale(&mut data, now), 1);
        assert!(data.is_empty());
    }

    #[test]
    fn store_roundtrip_and_dismiss() {
        let path = std::env::temp_dir()
            .join(format!("aerotune-residency-{}.json", std::process::id()))
            .join("residency-patterns.json");
        let mut store = ResidencyStore::load_from(path.clone());
        observe_samples(
            &mut store.data,
            &[sample(1, "roundtrip.exe", 400.0, 0.5)],
            "2026-10-01",
            1000,
        );
        store.save().unwrap();
        let mut reloaded = ResidencyStore::load_from(path.clone());
        assert!(reloaded.data.contains_key("roundtrip.exe"));
        assert!(reloaded.dismiss("roundtrip.exe"));
        reloaded.save().unwrap();
        assert!(ResidencyStore::load_from(path).data.is_empty());
    }
}
