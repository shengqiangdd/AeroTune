//! Read-only suggestions derived exclusively from the existing monitoring snapshots.
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::diag::{
    Diagnostics, MEMORY_CRITICAL_PCT, MEMORY_FORECAST_MIN_SAMPLES, MEMORY_PRESSURE_ELEVATED_PCT,
    MEMORY_RISING_SLOPE_THRESHOLD,
};
use crate::stats::{Cache, Stats};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SmartRecommendation {
    pub id: String,
    pub title: String,
    pub reason: String,
    /// Frontend page to open; this command never runs the suggested operation.
    pub action: String,
    /// Higher values appear first (0..=100).
    pub priority: u8,
}

fn suggestion(
    id: impl Into<String>,
    title: &str,
    reason: String,
    action: &str,
    priority: u8,
) -> SmartRecommendation {
    SmartRecommendation {
        id: id.into(),
        title: title.into(),
        reason,
        action: action.into(),
        priority,
    }
}

/// Accept only an actual Windows drive root, not an empty/default mount or a relative path.
fn drive_root(value: &str) -> Option<char> {
    let root = value.trim().trim_end_matches(['\\', '/']);
    let bytes = root.as_bytes();
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some((bytes[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

/// Pure decision logic. Empty/default snapshots yield no suggestions.
pub fn recommend_from_snapshots(
    stats: &Stats,
    diag: &Diagnostics,
    system_drive: &str,
) -> Vec<SmartRecommendation> {
    let mut out = Vec::new();
    // score=0 is the engine's never-observed default, not a valid health score.
    if diag.score != 0
        && diag.mem_pct.is_finite()
        && (0.0..=100.0).contains(&diag.mem_pct)
        && diag.mem_pct >= MEMORY_PRESSURE_ELEVATED_PCT
    {
        if diag.mem_pct >= MEMORY_CRITICAL_PCT {
            out.push(suggestion(
                "memory-pressure",
                "检查高内存占用进程",
                format!(
                    "内存占用 {:.0}%，已接近容量上限；请检查进程并按需整理。",
                    diag.mem_pct
                ),
                "processes",
                90,
            ));
        } else if diag.mem_slope_ready
            && diag.mem_slope.is_finite()
            && diag.mem_slope > MEMORY_RISING_SLOPE_THRESHOLD
        {
            if let Some(eta_secs) = diag.mem_critical_eta_secs.filter(|eta| {
                (1..=60).contains(eta)
                    && diag.mem_slope_sample_count as usize >= MEMORY_FORECAST_MIN_SAMPLES
            }) {
                out.push(suggestion(
                    "memory-pressure",
                    "检查快速升高的内存压力",
                    format!(
                        "内存占用 {:.0}%，近 45 秒稳健趋势若延续，约 {eta_secs} 秒接近 90%；请检查进程。",
                        diag.mem_pct
                    ),
                    "processes",
                    85,
                ));
            } else {
                out.push(suggestion(
                    "memory-pressure",
                    "检查持续上涨的内存占用",
                    format!(
                        "内存占用 {:.0}%，近 45 秒每秒增长约 {:.2} 个百分点；请检查进程。",
                        diag.mem_pct, diag.mem_slope
                    ),
                    "processes",
                    78,
                ));
            }
        }
    }

    // Swap pressure can reveal sustained memory pressure despite moderate RAM percentage.
    if stats.swap_total_gb.is_finite()
        && stats.swap_used_gb.is_finite()
        && stats.swap_total_gb > 0.0
        && stats.swap_used_gb >= 0.0
        && stats.swap_used_gb <= stats.swap_total_gb
    {
        let used_pct = stats.swap_used_gb / stats.swap_total_gb * 100.0;
        if stats.swap_used_gb >= 2.0 && used_pct >= 80.0 {
            out.push(suggestion(
                "pagefile-pressure",
                "检查页面文件与内存占用",
                format!("页面文件已使用 {:.1}/{:.1} GB（{:.0}%）；可查看内存占用较高的应用并确认系统盘留有空间。", stats.swap_used_gb, stats.swap_total_gb, used_pct),
                "processes",
                if used_pct >= 95.0 { 88 } else { 74 },
            ));
        }
    }

    // GPU telemetry is optional; only emit advice for plausible, internally consistent measurements.
    if let Some(gpu) = &stats.gpu {
        if let (Some(used), Some(total)) = (gpu.mem_used_mb, gpu.mem_total_mb) {
            if total > 0 && used <= total {
                let used_pct = used as f64 / total as f64 * 100.0;
                if used_pct >= 90.0 {
                    out.push(suggestion(
                        "gpu-memory-pressure",
                        "检查显存占用较高的应用",
                        format!(
                            "显存已使用 {:.0}%（{:.0}/{:.0} MB）；可检查大型游戏、视频或图形应用。",
                            used_pct, used, total
                        ),
                        "processes",
                        if used_pct >= 97.0 { 90 } else { 78 },
                    ));
                }
            }
        }
        if let Some(temp) = gpu
            .temperature
            .filter(|t| t.is_finite() && (0.0..=130.0).contains(t))
        {
            if temp >= 85.0 {
                out.push(suggestion(
                    "gpu-temperature-high",
                    "留意显卡温度",
                    format!(
                        "显卡温度约 {:.0}°C；可检查当前图形负载与机箱/散热通风。",
                        temp
                    ),
                    "processes",
                    if temp >= 95.0 { 96 } else { 82 },
                ));
            }
        }
    }

    if let Some(disk) = drive_root(system_drive).and_then(|drive| {
        stats
            .disks
            .iter()
            .find(|d| drive_root(&d.mount) == Some(drive))
    }) {
        if disk.total_gb.is_finite()
            && disk.used_gb.is_finite()
            && disk.total_gb > 0.0
            && disk.used_gb >= 0.0
            && disk.used_gb <= disk.total_gb
        {
            let free = disk.total_gb - disk.used_gb;
            let free_pct = free / disk.total_gb * 100.0;
            if free < 10.0 || free_pct < 10.0 {
                out.push(suggestion(
                    "system-disk-low",
                    "清理系统盘空间",
                    format!(
                        "系统盘 {} 剩余约 {:.1} GB（{:.0}%）；请检查可清理文件。",
                        disk.mount, free, free_pct
                    ),
                    "cleaner",
                    if free < 5.0 || free_pct < 5.0 { 95 } else { 80 },
                ));
            }
        }
    }

    if let Some(battery) = &stats.battery {
        if !battery.ac_online {
            if let Some(percent) = battery.percent.filter(|&p| p <= 20) {
                out.push(suggestion(
                    "battery-low",
                    "连接电源或启用省电模式",
                    format!("电池仅剩 {percent}%，当前未接电源。"),
                    "power",
                    if percent <= 10 { 98 } else { 85 },
                ));
            }
        }
    }

    // Diagnostics confirms anomalies over multiple samples. Filter before taking the
    // highest-priority three, regardless of the incoming snapshot's order.
    let mut anomalies: Vec<_> = diag
        .anomalies
        .iter()
        .filter(|a| {
            matches!(a.kind.as_str(), "cpu" | "mem" | "io")
                && a.value.is_finite()
                && a.baseline.is_finite()
                && a.robust_z.is_finite()
                && a.value >= 0.0
                && a.baseline >= 0.0
                && a.robust_z > 0.0
                && a.value > a.baseline
        })
        .collect();
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
    for anomaly in anomalies.into_iter().take(3) {
        let (metric, unit) = match anomaly.kind.as_str() {
            "cpu" => ("CPU", "%"),
            "mem" => ("内存", " MB"),
            "io" => ("磁盘吞吐", " KB/s"),
            _ => continue,
        };
        if !anomaly.value.is_finite() || !anomaly.baseline.is_finite() {
            continue;
        }
        out.push(suggestion(
            format!("process-{}-{}", anomaly.pid, anomaly.kind),
            "检查异常进程",
            format!(
                "{}（PID {}）的{}约 {:.0}{}，高于常态 {:.0}{}；请核查该进程。",
                anomaly.name, anomaly.pid, metric, anomaly.value, unit, anomaly.baseline, unit
            ),
            "processes",
            match anomaly.kind.as_str() {
                "cpu" => 84,
                "mem" => 76,
                _ => 70,
            },
        ));
    }

    if diag.score != 0 && diag.deductions.iter().any(|d| d.item == "启动项过多") {
        out.push(suggestion(
            "startup-burden",
            "检查启动项",
            "诊断统计的启动项（含已禁用项）偏多；请检查实际启用项并决定是否调整。".into(),
            "startup",
            55,
        ));
    }

    // Keep policy priority across recommendation types, then preserve robust-z rank
    // among process anomalies that share the same category priority.
    let anomaly_scores = diag
        .anomalies
        .iter()
        .filter(|anomaly| anomaly.robust_z.is_finite())
        .map(|anomaly| {
            (
                format!("process-{}-{}", anomaly.pid, anomaly.kind),
                anomaly.robust_z,
            )
        })
        .collect::<HashMap<_, _>>();
    out.sort_by(|a, b| {
        b.priority
            .cmp(&a.priority)
            .then_with(
                || match (anomaly_scores.get(&a.id), anomaly_scores.get(&b.id)) {
                    (Some(a_score), Some(b_score)) => b_score.total_cmp(a_score),
                    _ => std::cmp::Ordering::Equal,
                },
            )
            .then_with(|| a.id.cmp(&b.id))
    });
    out
}

#[tauri::command]
pub fn get_smart_recommendations(
    cache: tauri::State<'_, Arc<Cache>>,
    diag: tauri::State<'_, Arc<Mutex<crate::diag::Engine>>>,
    auto: tauri::State<'_, crate::auto::AutoStateLock>,
) -> Vec<SmartRecommendation> {
    // Clone under short, separate locks; no filesystem traversal, process refresh, or writes.
    let stats = cache.stats.read().unwrap().clone();
    let diagnostics = diag.lock().unwrap().current.clone();
    let system_drive = if stats.system_drive.is_empty() {
        std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into())
    } else {
        stats.system_drive.clone()
    };
    let game_list = {
        let state = auto.lock().unwrap_or_else(|e| e.into_inner());
        state.settings.game_list.clone()
    };
    let residency = crate::residency::snapshot_residency();
    let scene = crate::scene_shadow::snapshot_scene(&game_list);
    let mut out = recommend_from_snapshots(&stats, &diagnostics, &system_drive);
    append_learning_recommendations(&mut out, &residency, &scene);
    out
}

/// 学习引擎建议在总列表中的上限，避免学习类条目挤掉核心遥测建议。
const LEARNING_RECOMMENDATION_CAP: usize = 2;

/// 纯函数：把常驻观察与场景候选作为低优先级"观察类"建议追加到列表尾部。
/// 学习建议始终排在既有遥测建议之后，只导航不执行，不改变原有条目。
pub fn append_learning_recommendations(
    out: &mut Vec<SmartRecommendation>,
    residency: &[crate::residency::ResidencySuggestion],
    scene: &[crate::scene_shadow::SceneSuggestion],
) {
    let mut added = 0usize;
    for entry in residency {
        if added >= LEARNING_RECOMMENDATION_CAP {
            break;
        }
        out.push(suggestion(
            format!("resident-app-{}", entry.name),
            "观察：长期驻留的低活跃进程",
            format!(
                "跨 {} 天观测、平均 {} MB 内存，{}% 的样本处于低活跃；仅为本机观察提示，不自动处置，可在进程管理中查看或忽略。",
                entry.resident_days,
                entry.avg_mem_mb as u64,
                (entry.low_cpu_ratio * 100.0).round() as u64
            ),
            "processes",
            42,
        ));
        added += 1;
    }
    for entry in scene {
        if added >= LEARNING_RECOMMENDATION_CAP {
            break;
        }
        out.push(suggestion(
            format!("scene-candidate-{}", entry.name),
            "候选：多次确认全屏但不在游戏名单",
            format!(
                "影子观测到 {} 次确认全屏会话、跨 {} 天；可在智能引擎页采纳或忽略，是否加入名单由你决定。",
                entry.total_hits, entry.distinct_days
            ),
            "auto",
            40,
        ));
        added += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::{Anomaly, Deduction};
    use crate::stats::{BatteryInfo, DiskInfo};

    #[test]
    fn empty_snapshot_has_no_suggestions() {
        assert!(
            recommend_from_snapshots(&Stats::default(), &Diagnostics::default(), "C:").is_empty()
        );
    }

    #[test]
    fn learning_entries_append_with_evidence_and_cap() {
        let residency = vec![crate::residency::ResidencySuggestion {
            name: "vendor-updater.exe".into(),
            role_hint: "updater-suspect".into(),
            resident_days: 4,
            avg_mem_mb: 186.5,
            low_cpu_ratio: 0.97,
            last_seen_unix: 1,
        }];
        let scene = vec![crate::scene_shadow::SceneSuggestion {
            name: "newgame.exe".into(),
            total_hits: 5,
            distinct_days: 3,
            last_seen_unix: 1,
        }];
        let mut out = vec![suggestion(
            "system-disk-low",
            "既有建议",
            "理由".into(),
            "cleaner",
            95,
        )];
        append_learning_recommendations(&mut out, &residency, &scene);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].id, "system-disk-low", "existing entries stay first");
        let resident = out
            .iter()
            .find(|entry| entry.id == "resident-app-vendor-updater.exe")
            .expect("resident entry");
        assert!(
            resident.reason.contains("跨 4 天")
                && resident.reason.contains("186 MB")
                && resident.reason.contains("97%")
        );
        assert_eq!(resident.action, "processes");
        assert_eq!(resident.priority, 42);
        let scene_entry = out
            .iter()
            .find(|entry| entry.id == "scene-candidate-newgame.exe")
            .expect("scene entry");
        assert!(scene_entry.reason.contains("5 次确认全屏会话"));
        assert_eq!(scene_entry.action, "auto");
    }

    #[test]
    fn learning_entries_are_capped() {
        let residency: Vec<_> = (0..4)
            .map(|index| crate::residency::ResidencySuggestion {
                name: format!("app{index}.exe"),
                role_hint: "app".into(),
                resident_days: 5,
                avg_mem_mb: 200.0,
                low_cpu_ratio: 0.9,
                last_seen_unix: 1,
            })
            .collect();
        let scene: Vec<_> = (0..2)
            .map(|index| crate::scene_shadow::SceneSuggestion {
                name: format!("game{index}.exe"),
                total_hits: 6,
                distinct_days: 3,
                last_seen_unix: 1,
            })
            .collect();
        let mut out = Vec::new();
        append_learning_recommendations(&mut out, &residency, &scene);
        assert_eq!(out.len(), LEARNING_RECOMMENDATION_CAP);
        assert!(
            out.iter()
                .all(|entry| entry.id.starts_with("resident-app-")),
            "earlier source list wins"
        );
    }

    #[test]
    fn learning_entries_absent_when_stores_empty() {
        let mut out = Vec::new();
        append_learning_recommendations(&mut out, &[], &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn memory_requires_pressure_and_rising_slope_or_critical_level() {
        let stats = Stats::default();
        let mut diag = Diagnostics {
            score: 95,
            mem_pct: 79.0,
            mem_slope: 0.06,
            mem_slope_ready: true,
            ..Default::default()
        };
        assert_eq!(
            recommend_from_snapshots(&stats, &diag, "C:")[0].id,
            "memory-pressure"
        );
        diag.mem_slope = 0.05;
        assert!(recommend_from_snapshots(&stats, &diag, "C:").is_empty());
        diag.mem_slope = 0.06;
        diag.mem_slope_ready = false;
        assert!(recommend_from_snapshots(&stats, &diag, "C:").is_empty());
        diag.mem_pct = 90.0;
        assert_eq!(
            recommend_from_snapshots(&stats, &diag, "C:")[0].priority,
            90
        );
        diag.mem_pct = f32::NAN;
        assert!(recommend_from_snapshots(&stats, &diag, "C:").is_empty());
    }

    #[test]
    fn near_critical_memory_forecast_raises_only_the_matching_priority() {
        let stats = Stats::default();
        let mut diag = Diagnostics {
            score: 95,
            mem_pct: 86.4,
            mem_slope: 0.06,
            mem_slope_ready: true,
            mem_slope_sample_count: MEMORY_FORECAST_MIN_SAMPLES as u8,
            mem_critical_eta_secs: Some(60),
            ..Default::default()
        };
        let find_memory = |diag: &Diagnostics| {
            recommend_from_snapshots(&stats, diag, "C:")
                .into_iter()
                .find(|item| item.id == "memory-pressure")
                .unwrap()
        };

        let imminent = find_memory(&diag);
        assert_eq!(imminent.priority, 85);
        assert!(imminent.reason.contains("约 60 秒接近 90%"));

        diag.mem_slope_sample_count = (MEMORY_FORECAST_MIN_SAMPLES - 1) as u8;
        assert_eq!(find_memory(&diag).priority, 78);
        diag.mem_slope_sample_count = MEMORY_FORECAST_MIN_SAMPLES as u8;
        diag.mem_critical_eta_secs = Some(61);
        assert_eq!(find_memory(&diag).priority, 78);
        diag.mem_critical_eta_secs = Some(0);
        assert_eq!(find_memory(&diag).priority, 78);
    }

    #[test]
    fn only_system_disk_triggers_and_bad_disk_data_is_ignored() {
        let mut stats = Stats {
            disks: vec![
                DiskInfo {
                    mount: "C:".into(),
                    total_gb: 100.0,
                    used_gb: 99.0,
                    ..Default::default()
                },
                DiskInfo {
                    mount: "D:".into(),
                    total_gb: 100.0,
                    used_gb: 50.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "D:").is_empty());
        assert_eq!(
            recommend_from_snapshots(&stats, &Diagnostics::default(), "C:\\")[0].priority,
            95
        );
        stats.disks[0].used_gb = 101.0;
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "C:").is_empty());
    }

    #[test]
    fn drive_roots_are_case_insensitive_but_empty_and_relative_mounts_do_not_match() {
        let mut stats = Stats::default();
        stats.disks.push(DiskInfo {
            mount: "c:/".into(),
            total_gb: 100.0,
            used_gb: 96.0,
            ..Default::default()
        });
        assert_eq!(
            recommend_from_snapshots(&stats, &Diagnostics::default(), " C:\\\\ ")[0].id,
            "system-disk-low"
        );
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "D:").is_empty());
        stats.disks[0].mount = "".into();
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "").is_empty());
        stats.disks[0].mount = "C:relative".into();
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "C:").is_empty());
    }

    #[test]
    fn battery_requires_known_low_level_and_unplugged() {
        let mut stats = Stats {
            battery: Some(BatteryInfo {
                ac_online: false,
                ac_connected: Some(false),
                percent: Some(10),
                secs_left: None,
            }),
            ..Default::default()
        };
        assert_eq!(
            recommend_from_snapshots(&stats, &Diagnostics::default(), "C:")[0].priority,
            98
        );
        stats.battery.as_mut().unwrap().ac_online = true;
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "C:").is_empty());
        stats.battery.as_mut().unwrap().ac_online = false;
        stats.battery.as_mut().unwrap().percent = None;
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "C:").is_empty());
    }

    #[test]
    fn pagefile_pressure_requires_valid_capacity_and_both_thresholds() {
        let stats = Stats {
            swap_used_gb: 4.8,
            swap_total_gb: 6.0,
            ..Default::default()
        };
        let items = recommend_from_snapshots(&stats, &Diagnostics::default(), "C:");
        assert_eq!(items[0].id, "pagefile-pressure");
        assert_eq!(items[0].priority, 74);
        assert!(items[0].reason.contains("80%"));

        let stats = Stats {
            swap_used_gb: 5.8,
            swap_total_gb: 6.0,
            ..Default::default()
        };
        assert_eq!(
            recommend_from_snapshots(&stats, &Diagnostics::default(), "C:")[0].priority,
            88
        );
        for stats in [
            Stats {
                swap_used_gb: 1.9,
                swap_total_gb: 2.0,
                ..Default::default()
            },
            Stats {
                swap_used_gb: 8.0,
                swap_total_gb: 6.0,
                ..Default::default()
            },
            Stats {
                swap_used_gb: 3.0,
                swap_total_gb: 0.0,
                ..Default::default()
            },
            Stats {
                swap_used_gb: f64::NAN,
                swap_total_gb: 6.0,
                ..Default::default()
            },
        ] {
            assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "C:").is_empty());
        }
    }

    #[test]
    fn gpu_suggestions_require_plausible_metrics_and_use_expected_thresholds() {
        let gpu = crate::stats::GpuInfo {
            mem_used_mb: Some(9500),
            mem_total_mb: Some(10000),
            temperature: Some(96.0),
            ..Default::default()
        };
        let stats = Stats {
            gpu: Some(gpu),
            ..Default::default()
        };
        let items = recommend_from_snapshots(&stats, &Diagnostics::default(), "C:");
        assert_eq!(
            items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["gpu-temperature-high", "gpu-memory-pressure"]
        );
        assert_eq!(items[0].priority, 96);
        assert!(items[0].reason.contains("96°C"));

        let stats = Stats {
            gpu: Some(crate::stats::GpuInfo {
                mem_used_mb: Some(10001),
                mem_total_mb: Some(10000),
                temperature: Some(131.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "C:").is_empty());

        let stats = Stats {
            gpu: Some(crate::stats::GpuInfo {
                mem_used_mb: Some(8500),
                mem_total_mb: Some(10000),
                temperature: Some(84.9),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(recommend_from_snapshots(&stats, &Diagnostics::default(), "C:").is_empty());
    }

    #[test]
    fn anomalies_and_startup_sort_by_priority_with_stable_ids() {
        let stats = Stats::default();
        let diag = Diagnostics {
            score: 95,
            deductions: vec![Deduction {
                item: "启动项过多".into(),
                cost: 5,
            }],
            anomalies: vec![Anomaly {
                pid: 42,
                name: "fixture".into(),
                kind: "cpu".into(),
                value: 80.0,
                baseline: 5.0,
                robust_z: 8.0,
                onset_unix: None,
                creation_filetime: None,
            }],
            ..Default::default()
        };
        let items = recommend_from_snapshots(&stats, &diag, "C:");
        assert_eq!(
            items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec!["process-42-cpu", "startup-burden"]
        );
        assert_eq!(items[0].action, "processes");
        assert_eq!(items[1].action, "startup");
    }

    #[test]
    fn invalid_or_unsorted_anomalies_do_not_displace_priority_top_three() {
        let anomaly = |pid, kind: &str, value, baseline, robust_z| Anomaly {
            pid,
            name: format!("p{pid}"),
            kind: kind.into(),
            value,
            baseline,
            robust_z,
            onset_unix: None,
            creation_filetime: None,
        };
        let diag = Diagnostics {
            anomalies: vec![
                anomaly(9, "unknown", 999.0, 1.0, 99.0),
                anomaly(8, "cpu", f64::NAN, 1.0, 100.0),
                anomaly(10, "cpu", 100.0, 10.0, f64::NAN),
                anomaly(7, "io", 500.0, 20.0, 12.0),
                anomaly(6, "mem", 800.0, 100.0, 3.0),
                anomaly(5, "cpu", 70.0, 10.0, 8.0),
                anomaly(4, "cpu", 90.0, 10.0, 4.0),
                anomaly(3, "cpu", 30.0, 40.0, 20.0),
            ],
            ..Default::default()
        };
        let ids = recommend_from_snapshots(&Stats::default(), &diag, "C:")
            .into_iter()
            .map(|r| r.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, ["process-5-cpu", "process-4-cpu", "process-6-mem"]);
    }

    #[test]
    fn default_diagnostics_do_not_emit_memory_or_startup_recommendations() {
        let diag = Diagnostics {
            mem_pct: 91.0,
            mem_slope: 0.3,
            deductions: vec![Deduction {
                item: "启动项过多".into(),
                cost: 5,
            }],
            ..Default::default()
        };
        assert!(recommend_from_snapshots(&Stats::default(), &diag, "C:").is_empty());
        let diag = Diagnostics {
            score: 80,
            mem_pct: 101.0,
            ..Default::default()
        };
        assert!(recommend_from_snapshots(&Stats::default(), &diag, "C:").is_empty());
    }

    #[test]
    fn sorts_combined_suggestions_by_priority() {
        let mut stats = Stats {
            battery: Some(BatteryInfo {
                ac_online: false,
                ac_connected: Some(false),
                percent: Some(8),
                secs_left: None,
            }),
            ..Default::default()
        };
        stats.disks.push(DiskInfo {
            mount: "C:".into(),
            total_gb: 100.0,
            used_gb: 96.0,
            ..Default::default()
        });
        let diag = Diagnostics {
            score: 90,
            mem_pct: 92.0,
            ..Default::default()
        };
        let stats = Stats {
            swap_used_gb: 5.8,
            swap_total_gb: 6.0,
            battery: stats.battery.clone(),
            disks: stats.disks.clone(),
            ..Default::default()
        };
        let items = recommend_from_snapshots(&stats, &diag, "C:");
        assert_eq!(
            items.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            vec![
                "battery-low",
                "system-disk-low",
                "memory-pressure",
                "pagefile-pressure"
            ]
        );
    }
}
