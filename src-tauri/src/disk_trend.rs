// SMART 磁盘健康趋势：按需读取当前健康快照，低频追加到本机历史，
// 用纯函数从历史计算磨损/错误/通电时长的变化趋势。
// 红线：只读查询；趋势基于 AeroTune 运行期间的有限样本，不做寿命预测；
// 字段可用性取决于硬件与驱动；数据只存本机 disk-health-history.json。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct DiskHealthSample {
    pub ts_unix: u64,
    pub health: String,
    pub temp: Option<f64>,
    pub wear: Option<f64>,
    pub hours: Option<f64>,
    pub read_errors: Option<f64>,
    pub write_errors: Option<f64>,
}

pub type DiskTrendHistory = HashMap<String, Vec<DiskHealthSample>>;

/// 同盘两次存储的最小间隔：SMART 变化极慢，更密的样本没有信息量。
pub const SAMPLE_MIN_INTERVAL_SECS: u64 = 3600;
/// 每盘样本上限（120 小时样本 ≈ 数月跨度）。
pub const SAMPLES_PER_DISK_CAP: usize = 120;
/// 30 天无新样本的磁盘从趋势历史中淘汰。
const STALE_DISK_SECS: u64 = 30 * 24 * 3600;

#[derive(Serialize, Clone, Debug)]
pub struct DiskTrend {
    pub name: String,
    pub samples: usize,
    pub span_days: Option<f64>,
    /// 磨损相对上一样本的百分点变化（SSD；不可用为 None）。
    pub wear_delta: Option<f64>,
    /// 每 1000 通电小时的磨损增速（需要 ≥2 个含磨损与通电时长的样本）。
    pub wear_rate_per_1000h: Option<f64>,
    pub hours_delta: Option<f64>,
    /// 未纠正读/写错误相对上一样本的增量（增长为正）。
    pub error_delta: Option<f64>,
    /// error-rise（新错误）/ wear-moving（磨损上升）/ stable / insufficient
    pub tone: String,
    pub summary: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct DiskTrendReport {
    pub trends: Vec<DiskTrend>,
    pub stored: bool,
    /// 因最小间隔被跳过的盘数（快照仍用于展示）。
    pub skipped_unchanged: usize,
    pub generated_unix: u64,
}

pub struct DiskTrendStore {
    data: DiskTrendHistory,
    path: PathBuf,
}

impl DiskTrendStore {
    pub fn load() -> Self {
        Self::load_from(crate::util::app_dir().join("disk-health-history.json"))
    }

    pub fn load_from(path: PathBuf) -> Self {
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<DiskTrendHistory>(&bytes).ok())
            .unwrap_or_default();
        Self { data, path }
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("磁盘趋势目录创建失败: {e}"))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.data)
            .map_err(|e| format!("磁盘趋势序列化失败: {e}"))?;
        crate::util::atomic_write(&self.path, &bytes)
    }

    pub fn dismiss(&mut self, name: &str) -> bool {
        self.data.remove(name).is_some()
    }
}

fn sample_from_health(health: &crate::disk_analyzer::DiskHealth, ts_unix: u64) -> DiskHealthSample {
    DiskHealthSample {
        ts_unix,
        health: health.health.clone(),
        temp: health.temp,
        wear: health.wear,
        hours: health.power_on_hours,
        read_errors: health.read_errors,
        write_errors: health.write_errors,
    }
}

/// 纯函数：把当前快照按最小间隔追加进历史（拷贝进参，返回新历史与是否存储）。
/// 超过容量的盘丢弃最旧样本；30 天无样本的整盘淘汰。
pub fn record_samples(
    mut data: DiskTrendHistory,
    snapshot: &[crate::disk_analyzer::DiskHealth],
    now_unix: u64,
) -> (DiskTrendHistory, bool, usize) {
    let mut skipped = 0usize;
    let mut stored = false;
    for health in snapshot {
        let name = health.name.trim();
        if name.is_empty() {
            continue;
        }
        let entry = data.entry(name.to_string()).or_default();
        if entry
            .last()
            .is_some_and(|last| now_unix.saturating_sub(last.ts_unix) < SAMPLE_MIN_INTERVAL_SECS)
        {
            skipped += 1;
            continue;
        }
        entry.push(sample_from_health(health, now_unix));
        while entry.len() > SAMPLES_PER_DISK_CAP {
            entry.remove(0);
        }
        stored = true;
    }
    // 淘汰长期无样本的磁盘，防止换盘/重装后残留。
    let stale: Vec<String> = data
        .iter()
        .filter(|(_, samples)| {
            samples
                .last()
                .is_none_or(|last| now_unix.saturating_sub(last.ts_unix) > STALE_DISK_SECS)
        })
        .map(|(name, _)| name.clone())
        .collect();
    for name in stale {
        data.remove(&name);
    }
    (data, stored, skipped)
}

fn option_delta(earlier: Option<f64>, later: Option<f64>) -> Option<f64> {
    Some(later? - earlier?)
}

/// 纯函数：从历史计算每盘趋势；历史不足时如实输出 insufficient。
pub fn analyze_trends(data: &DiskTrendHistory) -> Vec<DiskTrend> {
    let mut trends: Vec<DiskTrend> = data
        .iter()
        .map(|(name, samples)| {
            let mut trend = DiskTrend {
                name: name.clone(),
                samples: samples.len(),
                span_days: None,
                wear_delta: None,
                wear_rate_per_1000h: None,
                hours_delta: None,
                error_delta: None,
                tone: "insufficient".into(),
                summary: String::new(),
            };
            let Some(first) = samples.first() else {
                trend.summary = "暂无历史样本".into();
                return trend;
            };
            let Some(last) = samples.last() else {
                trend.summary = "暂无历史样本".into();
                return trend;
            };
            if samples.len() < 2 {
                trend.summary = "仅一次观测，趋势需至少两次不同时间的样本".into();
                return trend;
            }
            let span_secs = last.ts_unix.saturating_sub(first.ts_unix);
            if span_secs < 3600 {
                trend.summary = "两次观测间隔过近，不足以形成趋势".into();
                return trend;
            }
            trend.span_days = Some((span_secs as f64 / 86400.0 * 10.0).round() / 10.0);
            trend.wear_delta = option_delta(first.wear, last.wear);
            trend.hours_delta = option_delta(first.hours, last.hours);
            let error_sum =
                |sample: &DiskHealthSample| match (sample.read_errors, sample.write_errors) {
                    (Some(read), Some(write)) => Some(read + write),
                    (Some(value), None) | (None, Some(value)) => Some(value),
                    (None, None) => None,
                };
            trend.error_delta = option_delta(error_sum(first), error_sum(last));
            if let (Some(first_wear), Some(last_wear), Some(first_hours), Some(last_hours)) =
                (first.wear, last.wear, first.hours, last.hours)
            {
                let hours_span = last_hours - first_hours;
                if hours_span > 0.0 {
                    trend.wear_rate_per_1000h = Some(
                        ((last_wear - first_wear) / hours_span * 1000.0 * 100.0).round() / 100.0,
                    );
                }
            }

            if trend.error_delta.is_some_and(|delta| delta > 0.0) {
                trend.tone = "error-rise".into();
                trend.summary = format!(
                    "未纠正 I/O 错误较上次观测增加 {}；建议关注该磁盘并保持备份",
                    trend.error_delta.unwrap()
                );
            } else if trend.wear_delta.is_some_and(|delta| delta >= 1.0) {
                trend.tone = "wear-moving".into();
                trend.summary =
                    format!("磨损较首次观测上升 {} 个百分点", trend.wear_delta.unwrap());
            } else {
                trend.tone = "stable".into();
                trend.summary = "历史样本内未见磨损上升或新的未纠正错误".into();
            }
            trend
        })
        .collect();
    trends.sort_by(|a, b| a.name.cmp(&b.name));
    trends
}

static DISK_TREND_STORE: OnceLock<Mutex<DiskTrendStore>> = OnceLock::new();

fn store() -> &'static Mutex<DiskTrendStore> {
    DISK_TREND_STORE.get_or_init(|| Mutex::new(DiskTrendStore::load()))
}

/// 按需命令：读取当前健康快照 → 低频追加历史 → 返回趋势分析。
/// PowerShell 只在用户查看时运行；存储失败不影响趋势展示。
#[tauri::command]
pub async fn get_disk_health_trends() -> Result<DiskTrendReport, String> {
    let snapshot = crate::disk_analyzer::disk_health().await;
    let now_unix = chrono::Utc::now().timestamp().max(0) as u64;
    let (data, stored, skipped) = {
        let guard = store().lock().unwrap_or_else(|e| e.into_inner());
        record_samples(guard.data.clone(), &snapshot, now_unix)
    };
    {
        let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
        guard.data = data.clone();
        if stored {
            guard.save()?;
        }
    }
    Ok(DiskTrendReport {
        trends: analyze_trends(&data),
        stored,
        skipped_unchanged: skipped,
        generated_unix: now_unix,
    })
}

/// 忽略命令：清除一块磁盘的本地趋势历史。
#[tauri::command]
pub fn dismiss_disk_trend(name: String) -> Result<bool, String> {
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
    use crate::disk_analyzer::DiskHealth;

    fn health(
        name: &str,
        wear: Option<f64>,
        hours: Option<f64>,
        rerr: Option<f64>,
        werr: Option<f64>,
    ) -> DiskHealth {
        DiskHealth {
            name: name.into(),
            media: "SSD".into(),
            health: "Healthy".into(),
            size_gb: 500.0,
            temp: Some(40.0),
            wear,
            power_on_hours: hours,
            read_errors: rerr,
            write_errors: werr,
        }
    }

    #[test]
    fn recording_respects_min_interval_and_cap() {
        let mut data = DiskTrendHistory::new();
        let (next, stored, skipped) = record_samples(
            data,
            &[health("C", Some(5.0), Some(1000.0), Some(0.0), Some(0.0))],
            1000,
        );
        data = next;
        assert!(stored);
        assert_eq!(skipped, 0);
        // 一小时内的重复读取不追加
        let (next, stored, skipped) = record_samples(
            data,
            &[health("C", Some(5.0), Some(1005.0), Some(0.0), Some(0.0))],
            1000 + SAMPLE_MIN_INTERVAL_SECS - 10,
        );
        data = next;
        assert!(!stored);
        assert_eq!(skipped, 1);
        // 超过间隔才追加
        let (next, stored, _) = record_samples(
            data,
            &[health("C", Some(6.0), Some(2000.0), Some(0.0), Some(0.0))],
            1000 + SAMPLE_MIN_INTERVAL_SECS + 10,
        );
        data = next;
        assert!(stored);
        assert_eq!(data["C"].len(), 2);
        // 容量上限：灌满后最旧样本被丢弃
        for index in 0..(SAMPLES_PER_DISK_CAP + 5) {
            let (next, _, _) = record_samples(
                data,
                &[health(
                    "C",
                    Some(7.0),
                    Some(3000.0 + index as f64),
                    Some(0.0),
                    Some(0.0),
                )],
                5000 + index as u64 * (SAMPLE_MIN_INTERVAL_SECS + 1),
            );
            data = next;
        }
        assert_eq!(data["C"].len(), SAMPLES_PER_DISK_CAP);
    }

    #[test]
    fn empty_names_are_skipped_and_stale_disks_pruned() {
        let mut data = DiskTrendHistory::new();
        let (next, stored, _) =
            record_samples(data, &[health("", Some(1.0), None, None, None)], 1000);
        assert!(!stored);
        data = next;
        let (next, _, _) =
            record_samples(data, &[health("old", Some(1.0), None, None, None)], 1000);
        let mut data = next;
        let now = 1000 + STALE_DISK_SECS + 10;
        let (next, _, _) =
            record_samples(data, &[health("fresh", Some(1.0), None, None, None)], now);
        data = next;
        assert!(!data.contains_key("old"), "stale disk pruned");
        assert!(data.contains_key("fresh"));
    }

    #[test]
    fn trend_tones_match_evidence() {
        let mut data = DiskTrendHistory::new();
        // 错误增长 → error-rise
        data.insert(
            "bad".into(),
            vec![
                DiskHealthSample {
                    ts_unix: 0,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(5.0),
                    hours: Some(1000.0),
                    read_errors: Some(0.0),
                    write_errors: Some(0.0),
                },
                DiskHealthSample {
                    ts_unix: 5 * 86400,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(5.0),
                    hours: Some(2000.0),
                    read_errors: Some(2.0),
                    write_errors: Some(0.0),
                },
            ],
        );
        // 磨损上升 → wear-moving（同时有磨损速率）
        data.insert(
            "wearing".into(),
            vec![
                DiskHealthSample {
                    ts_unix: 0,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(10.0),
                    hours: Some(1000.0),
                    read_errors: Some(0.0),
                    write_errors: Some(0.0),
                },
                DiskHealthSample {
                    ts_unix: 10 * 86400,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(12.0),
                    hours: Some(3000.0),
                    read_errors: Some(0.0),
                    write_errors: Some(0.0),
                },
            ],
        );
        // 稳定 → stable
        data.insert(
            "calm".into(),
            vec![
                DiskHealthSample {
                    ts_unix: 0,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(5.0),
                    hours: Some(1000.0),
                    read_errors: Some(0.0),
                    write_errors: Some(0.0),
                },
                DiskHealthSample {
                    ts_unix: 7 * 86400,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(5.0),
                    hours: Some(2000.0),
                    read_errors: Some(0.0),
                    write_errors: Some(0.0),
                },
            ],
        );
        let trends = analyze_trends(&data);
        let by_name = |name: &str| trends.iter().find(|t| t.name == name).unwrap();
        let bad = by_name("bad");
        assert_eq!(bad.tone, "error-rise");
        assert!(bad.summary.contains("增加 2"));
        assert_eq!(bad.span_days, Some(5.0));
        let wearing = by_name("wearing");
        assert_eq!(wearing.tone, "wear-moving");
        assert_eq!(
            wearing.wear_rate_per_1000h,
            Some(1.0),
            "2 pct / 2000h = 1 per 1000h"
        );
        assert_eq!(by_name("calm").tone, "stable");
    }

    #[test]
    fn single_sample_and_close_samples_are_insufficient() {
        let mut data = DiskTrendHistory::new();
        data.insert(
            "one".into(),
            vec![DiskHealthSample {
                ts_unix: 0,
                health: "Healthy".into(),
                temp: None,
                wear: Some(1.0),
                hours: Some(10.0),
                read_errors: None,
                write_errors: None,
            }],
        );
        data.insert(
            "close".into(),
            vec![
                DiskHealthSample {
                    ts_unix: 0,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(1.0),
                    hours: Some(10.0),
                    read_errors: None,
                    write_errors: None,
                },
                DiskHealthSample {
                    ts_unix: 60,
                    health: "Healthy".into(),
                    temp: None,
                    wear: Some(9.0),
                    hours: Some(11.0),
                    read_errors: None,
                    write_errors: None,
                },
            ],
        );
        let trends = analyze_trends(&data);
        let by_name = |name: &str| trends.iter().find(|t| t.name == name).unwrap();
        assert_eq!(by_name("one").tone, "insufficient");
        assert_eq!(by_name("close").tone, "insufficient");
        assert!(by_name("one").summary.contains("至少两次"));
    }

    #[test]
    fn store_roundtrip_and_dismiss() {
        let path = std::env::temp_dir()
            .join(format!("aerotune-disk-trend-{}.json", std::process::id()))
            .join("disk-health-history.json");
        let mut store = DiskTrendStore::load_from(path.clone());
        let (data, _, _) = record_samples(
            store.data.clone(),
            &[health("C", Some(5.0), Some(1000.0), Some(0.0), Some(0.0))],
            1000,
        );
        store.data = data;
        store.save().unwrap();
        let mut reloaded = DiskTrendStore::load_from(path.clone());
        assert!(reloaded.data.contains_key("C"));
        assert!(reloaded.dismiss("C"));
        reloaded.save().unwrap();
        assert!(DiskTrendStore::load_from(path).data.is_empty());
    }
}
