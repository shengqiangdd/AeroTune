// 实验一致性影子分析：对「优化对比实验」的历史记录做纯函数分析，
// 判断同一模式在多次实验中是否呈现一致的方向与幅度。这是参数自学习的
// 观察层——只输出"是否构成一致性证据"的结论，绝不自动调整任何参数；
// 真正的调参（若有）必须由用户基于结论自行决定。无状态、无 IO。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 可分析的最少完整实验数（低于此数明确输出"样本不足"）。
pub const MIN_EXPERIMENTS_PER_MODE: usize = 5;
/// 方向一致性比例门槛。
pub const CONSISTENCY_RATIO: f64 = 0.8;
/// 有意义的最小中位数幅度（全机 CPU 百分点 / 内存 GB）。
pub const MIN_CPU_DELTA_PCT: f64 = 2.0;
pub const MIN_MEM_DELTA_GB: f64 = 0.3;
/// 双窗口采样覆盖率门槛：任一窗口覆盖率低于此值即不可信。
pub const MIN_WINDOW_COVERAGE: f64 = 0.7;

/// 与前端摘要同构；旧 JSON 可缺失上下文，但不能成为可信证据。
#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PowerContextInput {
    pub sample_count: f64,
    pub power_source_counts: HashMap<String, f64>,
    pub effective_power_mode_counts: Vec<PowerModeCountInput>,
    pub effective_power_mode_unknown_count: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct PowerModeCountInput {
    pub mode: String,
    pub count: f64,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ExperimentWindowInput {
    #[serde(default)]
    pub sample_count: f64,
    pub cpu_pct: Option<f64>,
    pub memory_used_gb: Option<f64>,
    #[serde(default)]
    pub coverage: HashMap<String, f64>,
    #[serde(default)]
    pub power_context: Option<PowerContextInput>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ExperimentRecordInput {
    pub mode: String,
    /// complete | interrupted —— 只有 complete 参与分析。
    pub status: String,
    pub baseline: ExperimentWindowInput,
    #[serde(default)]
    pub after: Option<ExperimentWindowInput>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ModeConsistency {
    pub mode: String,
    pub analyzed: usize,
    pub cpu_direction: String,
    pub cpu_median_delta: Option<f64>,
    pub mem_direction: String,
    pub mem_median_delta: Option<f64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ExperimentConsistencyReport {
    pub modes: Vec<ModeConsistency>,
    /// ready（存在达标模式）/ insufficient（样本不足）/ inconsistent（方向矛盾）/ empty
    pub verdict: String,
    pub summary: String,
}

fn stable_power_source(window: &ExperimentWindowInput) -> Option<&str> {
    let context = window.power_context.as_ref()?;
    let count = |value: f64| {
        value.is_finite()
            && value >= 0.0
            && value.fract() == 0.0
            && value <= 9_007_199_254_740_991.0
    };
    if !count(context.sample_count)
        || context.sample_count < 3.0
        || context.sample_count != window.sample_count
        || !count(context.effective_power_mode_unknown_count)
        || context.power_source_counts.len() != 3
    {
        return None;
    }
    let sources = &context.power_source_counts;
    let ac = *sources.get("ac")?;
    let battery = *sources.get("battery")?;
    let unknown = *sources.get("unknown")?;
    if ![ac, battery, unknown].into_iter().all(count)
        || ac + battery + unknown != context.sample_count
        || unknown != 0.0
    {
        return None;
    }
    let mut seen = std::collections::HashSet::new();
    let mut mode_total = context.effective_power_mode_unknown_count;
    for item in &context.effective_power_mode_counts {
        if !matches!(
            item.mode.as_str(),
            "battery_saver"
                | "better_battery"
                | "balanced"
                | "high_performance"
                | "max_performance"
                | "game_mode"
                | "mixed_reality"
        ) || !seen.insert(item.mode.as_str())
            || !count(item.count)
            || item.count == 0.0
        {
            return None;
        }
        mode_total += item.count;
    }
    if mode_total != context.sample_count {
        return None;
    }
    if ac == context.sample_count {
        Some("ac")
    } else if battery == context.sample_count {
        Some("battery")
    } else {
        None
    }
}

fn window_trustable(window: &ExperimentWindowInput) -> bool {
    let coverage_ok = ["cpu", "memory"]
        .iter()
        .all(|key| window.coverage.get(*key).copied().unwrap_or(0.0) >= MIN_WINDOW_COVERAGE);
    coverage_ok && window.sample_count >= 3.0
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    Some(values[values.len() / 2])
}

fn direction_of(delta: Option<f64>, meaningful: f64) -> String {
    match delta {
        Some(value) if value <= -meaningful => "lower".into(),
        Some(value) if value >= meaningful => "higher".into(),
        Some(_) => "flat".into(),
        None => "unknown".into(),
    }
}

/// 纯函数：按模式聚合分析。只统计 complete、双窗口可信且供电来源已知、稳定、相同的记录。
/// 此校验位于共享分析入口，不能通过绕过前端过滤使旧记录或不匹配上下文成为证据。
pub fn analyze_consistency(records: &[ExperimentRecordInput]) -> ExperimentConsistencyReport {
    let mut by_mode: HashMap<&str, Vec<(&ExperimentWindowInput, &ExperimentWindowInput)>> =
        HashMap::new();
    for record in records {
        if record.status != "complete" {
            continue;
        }
        let (Some(after), true) = (record.after.as_ref(), window_trustable(&record.baseline))
        else {
            continue;
        };
        if !window_trustable(after) {
            continue;
        }
        let source = stable_power_source(&record.baseline);
        if source.is_none() || source != stable_power_source(after) {
            continue;
        }
        by_mode
            .entry(record.mode.as_str())
            .or_default()
            .push((&record.baseline, after));
    }

    let mut modes = Vec::new();
    for (mode, pairs) in by_mode {
        let analyzed = pairs.len();
        let mut cpu_deltas: Vec<f64> = pairs
            .iter()
            .filter_map(
                |(baseline, after)| match (baseline.cpu_pct, after.cpu_pct) {
                    (Some(base), Some(value)) => Some(value - base),
                    _ => None,
                },
            )
            .collect();
        let mut mem_deltas: Vec<f64> = pairs
            .iter()
            .filter_map(
                |(baseline, after)| match (baseline.memory_used_gb, after.memory_used_gb) {
                    (Some(base), Some(value)) => Some(value - base),
                    _ => None,
                },
            )
            .collect();
        let cpu_median = median(&mut cpu_deltas);
        let mem_median = median(&mut mem_deltas);
        let cpu_direction = direction_of(cpu_median, MIN_CPU_DELTA_PCT);
        let mem_direction = direction_of(mem_median, MIN_MEM_DELTA_GB);

        // 方向一致性：偏离 "flat/unknown" 的记录里，同向占比 ≥ CONSISTENCY_RATIO。
        let direction_consistent = |deltas: &[f64], meaningful: f64| -> bool {
            let signed: Vec<f64> = deltas
                .iter()
                .copied()
                .filter(|d| d.abs() >= meaningful)
                .collect();
            if signed.len() < 3 {
                return false;
            }
            let positives = signed.iter().filter(|d| **d > 0.0).count();
            let negatives = signed.len() - positives;
            let ratio = positives.max(negatives) as f64 / signed.len() as f64;
            ratio >= CONSISTENCY_RATIO
        };
        let cpu_consistent = direction_consistent(&cpu_deltas, MIN_CPU_DELTA_PCT);
        let mem_consistent = direction_consistent(&mem_deltas, MIN_MEM_DELTA_GB);

        modes.push(ModeConsistency {
            mode: mode.to_string(),
            analyzed,
            cpu_direction: if cpu_consistent {
                cpu_direction
            } else {
                "mixed".into()
            },
            cpu_median_delta: cpu_median.map(|value| (value * 10.0).round() / 10.0),
            mem_direction: if mem_consistent {
                mem_direction
            } else {
                "mixed".into()
            },
            mem_median_delta: mem_median.map(|value| (value * 100.0).round() / 100.0),
        });
    }
    modes.sort_by(|a, b| a.mode.cmp(&b.mode));

    let verdict = if modes.is_empty() {
        "empty"
    } else if modes.iter().any(|mode| {
        mode.analyzed >= MIN_EXPERIMENTS_PER_MODE
            && (mode.cpu_direction == "lower" || mode.mem_direction == "lower")
    }) {
        "ready"
    } else if modes
        .iter()
        .all(|mode| mode.analyzed < MIN_EXPERIMENTS_PER_MODE)
    {
        "insufficient"
    } else {
        "inconsistent"
    };
    let summary = match verdict {
        "ready" => "部分模式的多次实验呈现一致方向；这是观察结论，AeroTune 不会据此自动调整参数。".into(),
        "insufficient" => format!("完整实验样本不足（每种模式至少需要 {MIN_EXPERIMENTS_PER_MODE} 次可信实验），暂不构成任何调整依据。"),
        "inconsistent" => "实验结果方向不一致，不构成调整依据；建议在相近负载下重做实验。".into(),
        _ => "暂无可分析的完整实验记录。".into(),
    };
    ExperimentConsistencyReport {
        modes,
        verdict: verdict.into(),
        summary,
    }
}

/// 只读命令：前端把本地实验记录传入，返回一致性分析（无状态，不落盘）。
#[tauri::command]
pub fn analyze_experiment_consistency(
    records: Vec<ExperimentRecordInput>,
) -> ExperimentConsistencyReport {
    analyze_consistency(&records)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(cpu: f64, mem: f64, coverage: f64) -> ExperimentWindowInput {
        ExperimentWindowInput {
            sample_count: 60.0,
            cpu_pct: Some(cpu),
            memory_used_gb: Some(mem),
            coverage: HashMap::from([("cpu".into(), coverage), ("memory".into(), coverage)]),
            power_context: Some(PowerContextInput {
                sample_count: 60.0,
                power_source_counts: HashMap::from([
                    ("ac".into(), 60.0),
                    ("battery".into(), 0.0),
                    ("unknown".into(), 0.0),
                ]),
                effective_power_mode_counts: vec![PowerModeCountInput {
                    mode: "balanced".into(),
                    count: 60.0,
                }],
                effective_power_mode_unknown_count: 0.0,
            }),
        }
    }

    fn record(mode: &str, base_cpu: f64, after_cpu: f64) -> ExperimentRecordInput {
        ExperimentRecordInput {
            mode: mode.into(),
            status: "complete".into(),
            baseline: window(base_cpu, 8.0, 1.0),
            after: Some(window(after_cpu, 8.0, 1.0)),
        }
    }

    #[test]
    fn insufficient_samples_report_verdict_without_claims() {
        let records: Vec<_> = (0..MIN_EXPERIMENTS_PER_MODE - 1)
            .map(|index| record("game", 30.0, 20.0 + index as f64 * 0.1))
            .collect();
        let report = analyze_consistency(&records);
        assert_eq!(report.verdict, "insufficient");
        assert!(report.summary.contains("样本不足"));
        assert!(report.summary.contains("不构成任何调整依据"));
    }

    #[test]
    fn consistent_lower_cpu_reaches_ready() {
        let records: Vec<_> = (0..MIN_EXPERIMENTS_PER_MODE + 1)
            .map(|index| record("game", 30.0, 22.0 + index as f64 * 0.2))
            .collect();
        let report = analyze_consistency(&records);
        assert_eq!(report.verdict, "ready");
        let game = &report.modes[0];
        assert_eq!(game.cpu_direction, "lower");
        assert!(game.cpu_median_delta.unwrap() <= -MIN_CPU_DELTA_PCT);
        assert!(report.summary.contains("不会据此自动调整"));
    }

    #[test]
    fn mixed_directions_are_reported_as_mixed() {
        let deltas = [21.0, 21.0, 21.0, 45.0, 45.0, 45.0];
        let records: Vec<_> = deltas
            .iter()
            .map(|after| record("game", 30.0, *after))
            .collect();
        let report = analyze_consistency(&records);
        assert_eq!(report.verdict, "inconsistent");
        assert_eq!(report.modes[0].cpu_direction, "mixed");
        assert!(report.summary.contains("不构成调整依据"));
    }

    #[test]
    fn interrupted_and_low_coverage_records_are_excluded() {
        let mut records: Vec<_> = (0..MIN_EXPERIMENTS_PER_MODE + 1)
            .map(|_| record("game", 30.0, 20.0))
            .collect();
        records[0].status = "interrupted".into();
        records[1].baseline.coverage = HashMap::from([("cpu".into(), 0.3), ("memory".into(), 0.3)]);
        records[2].after = None;
        let report = analyze_consistency(&records);
        assert_eq!(report.modes[0].analyzed, MIN_EXPERIMENTS_PER_MODE + 1 - 3);
    }

    #[test]
    fn legacy_json_deserializes_but_command_does_not_trust_it() {
        let legacy = serde_json::json!({
            "mode": "game", "status": "complete",
            "baseline": {"sample_count": 60, "cpu_pct": 30, "memory_used_gb": 8, "coverage": {"cpu": 1, "memory": 1}},
            "after": {"sample_count": 60, "cpu_pct": 20, "memory_used_gb": 8, "coverage": {"cpu": 1, "memory": 1}}
        });
        let records = serde_json::from_value(serde_json::Value::Array(vec![legacy; 6])).unwrap();
        assert_eq!(analyze_experiment_consistency(records).verdict, "empty");
    }

    #[test]
    fn frontend_payload_context_deserializes_and_is_checked() {
        let context = serde_json::json!({
            "sampleCount": 60,
            "powerSourceCounts": {"ac": 60, "battery": 0, "unknown": 0},
            "effectivePowerModeCounts": [{"mode": "balanced", "count": 60}],
            "effectivePowerModeUnknownCount": 0
        });
        let mut value = serde_json::json!({
            "mode": "game", "status": "complete",
            "baseline": {"sample_count": 60, "cpu_pct": 30, "memory_used_gb": 8, "coverage": {"cpu": 1, "memory": 1}, "power_context": context},
            "after": {"sample_count": 60, "cpu_pct": 20, "memory_used_gb": 8, "coverage": {"cpu": 1, "memory": 1}, "power_context": context}
        });
        let records =
            serde_json::from_value(serde_json::Value::Array(vec![value.clone(); 6])).unwrap();
        assert_eq!(analyze_experiment_consistency(records).verdict, "ready");
        value["after"]["power_context"] = serde_json::Value::Null;
        let records = serde_json::from_value(serde_json::Value::Array(vec![value; 6])).unwrap();
        assert_eq!(analyze_experiment_consistency(records).verdict, "empty");
    }

    #[test]
    fn power_context_is_fail_closed_at_command_boundary() {
        for case in 0..17 {
            let mut item = record("game", 30.0, 20.0);
            if case == 0 {
                item.baseline.power_context = None;
            } else if case == 1 {
                item.after.as_mut().unwrap().power_context = None;
            } else {
                let context = item.after.as_mut().unwrap().power_context.as_mut().unwrap();
                match case {
                    2 => {
                        context.power_source_counts.insert("ac".into(), 0.0);
                        context.power_source_counts.insert("unknown".into(), 60.0);
                    }
                    3 => {
                        context.power_source_counts.insert("ac".into(), 30.0);
                        context.power_source_counts.insert("battery".into(), 30.0);
                    }
                    4 => {
                        context.power_source_counts.insert("ac".into(), 0.0);
                        context.power_source_counts.insert("battery".into(), 60.0);
                    }
                    5 => context.sample_count = 59.0,
                    6 => {
                        context.power_source_counts.remove("unknown");
                    }
                    7 => {
                        context.power_source_counts.insert("ac".into(), 59.0);
                    }
                    8 => {
                        context
                            .power_source_counts
                            .insert("ac".into(), f64::INFINITY);
                    }
                    9 => {
                        context.power_source_counts.insert("battery".into(), -1.0);
                    }
                    10 => {
                        context.power_source_counts.insert("battery".into(), 0.5);
                    }
                    11 => context.effective_power_mode_counts[0].count = 59.0,
                    12 => context.effective_power_mode_counts[0].mode = "future_mode".into(),
                    13 => context
                        .effective_power_mode_counts
                        .push(context.effective_power_mode_counts[0].clone()),
                    14 => context.effective_power_mode_unknown_count = f64::NAN,
                    15 => {
                        context
                            .power_source_counts
                            .insert("future_source".into(), 0.0);
                    }
                    16 => context.effective_power_mode_counts[0].count = 0.0,
                    _ => unreachable!(),
                }
            }
            assert_eq!(
                analyze_experiment_consistency(vec![item; 6]).verdict,
                "empty",
                "case {case}"
            );
        }
    }

    #[test]
    fn stable_battery_and_changed_effective_modes_remain_comparable() {
        let mut item = record("game", 30.0, 20.0);
        for window in [&mut item.baseline, item.after.as_mut().unwrap()] {
            let context = window.power_context.as_mut().unwrap();
            context.power_source_counts.insert("ac".into(), 0.0);
            context.power_source_counts.insert("battery".into(), 60.0);
        }
        // Effective mode is descriptive, not the supply; optimization can change it.
        item.after
            .as_mut()
            .unwrap()
            .power_context
            .as_mut()
            .unwrap()
            .effective_power_mode_counts[0]
            .mode = "high_performance".into();
        assert_eq!(
            analyze_experiment_consistency(vec![item; 6]).verdict,
            "ready"
        );
    }

    #[test]
    fn short_power_context_is_not_evidence() {
        let mut item = record("game", 30.0, 20.0);
        item.baseline.sample_count = 2.0;
        let context = item.baseline.power_context.as_mut().unwrap();
        context.sample_count = 2.0;
        context.power_source_counts.insert("ac".into(), 2.0);
        context.effective_power_mode_counts[0].count = 2.0;
        assert_eq!(
            analyze_experiment_consistency(vec![item; 6]).verdict,
            "empty"
        );
    }

    #[test]
    fn empty_input_is_honest_empty() {
        let report = analyze_consistency(&[]);
        assert_eq!(report.verdict, "empty");
        assert!(report.modes.is_empty());
    }

    #[test]
    fn flat_changes_never_count_as_direction() {
        let records: Vec<_> = (0..MIN_EXPERIMENTS_PER_MODE + 2)
            .map(|index| record("power", 30.0, 30.0 + (index % 2) as f64 * 0.2))
            .collect();
        let report = analyze_consistency(&records);
        assert_eq!(report.modes[0].cpu_direction, "mixed");
        assert_eq!(report.verdict, "inconsistent");
    }
}
