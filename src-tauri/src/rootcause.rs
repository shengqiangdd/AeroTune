// 异常根因引导：把确认异常的起点时间与同期本地事件（进程启动、变更日志、
// 引擎模式切换）做时间关联，输出"相关提示"。关联只是提示可能起因，
// 不证明因果；所有数据均为本机已有记录，不产生新的系统查询副作用。
use serde::Serialize;
use std::sync::{Arc, Mutex};

#[derive(Serialize, Clone, Debug)]
pub struct AnomalyHint {
    /// recent（自身刚启动）/ correlation（他进程时间相近）/ context（此前事件）/ none（空态）
    pub tone: String,
    pub text: String,
}

/// 时间线上的单个事件，供关联图表渲染；与提示文字同源同窗。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct TimelineEvent {
    pub at_unix: u64,
    /// onset | process-start | journal | decision
    pub kind: String,
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct AnomalyContext {
    pub pid: u32,
    pub name: String,
    pub kind: String,
    pub onset_unix: Option<u64>,
    pub age_secs: Option<u64>,
    pub hints: Vec<AnomalyHint>,
    pub timeline: Vec<TimelineEvent>,
}

/// 关联输入：进程创建时间引用（pid, 名称, Unix 秒）。
pub struct ProcessStartRef {
    pub pid: u32,
    pub name: String,
    pub start_unix: Option<u64>,
}

/// 关联输入：变更日志条目（Unix 秒, 描述）。
pub struct JournalRef {
    pub ts: u64,
    pub desc: String,
}

/// 关联输入：引擎决策（Unix 秒, from, to）。
pub struct DecisionRef {
    pub ts: u64,
    pub from: String,
    pub to: String,
}

/// 异常进程自身"启动后即高负载"的最大间隔。
const OWN_START_WINDOW_SECS: u64 = 180;
/// 其他进程启动与异常起点的时间窗（前后各留少量余量）。
const OTHER_START_BEFORE_SECS: u64 = 60;
const OTHER_START_AFTER_SECS: u64 = 10;
/// 变更日志与引擎决策回看的窗口。
const CONTEXT_LOOKBACK_SECS: u64 = 300;
const MAX_NEARBY_STARTS: usize = 3;
const MAX_CONTEXT_EVENTS: usize = 2;

fn hint(tone: &str, text: String) -> AnomalyHint {
    AnomalyHint {
        tone: tone.to_string(),
        text,
    }
}

fn duration_label(secs: u64) -> String {
    if secs < 60 {
        format!("{secs} 秒")
    } else if secs < 3600 {
        format!("{} 分钟", secs / 60)
    } else {
        format!("{} 小时 {} 分", secs / 3600, secs % 3600 / 60)
    }
}

/// 纯函数：为单个异常生成关联提示。`now_unix` 用于计算起点距今。
pub fn correlate(
    pid: u32,
    onset_unix: Option<u64>,
    now_unix: u64,
    own_start_unix: Option<u64>,
    starts: &[ProcessStartRef],
    journal: &[JournalRef],
    decisions: &[DecisionRef],
) -> Vec<AnomalyHint> {
    let Some(onset) = onset_unix else {
        return vec![hint(
            "none",
            "未记录异常起点时间（可能在采样中断后重新确认）".into(),
        )];
    };
    if now_unix < onset {
        return vec![hint("none", "起点时间异常（时钟可能已调整）".into())];
    }
    let mut hints = Vec::new();

    if let Some(own) = own_start_unix {
        if own <= onset && onset.saturating_sub(own) <= OWN_START_WINDOW_SECS {
            let gap = onset.saturating_sub(own);
            hints.push(hint(
                "recent",
                format!("该进程启动后 {} 即进入高负载", duration_label(gap)),
            ));
        }
    }

    let mut nearby: Vec<&ProcessStartRef> = starts
        .iter()
        .filter(|reference| {
            reference.pid != pid
                && reference.start_unix.is_some_and(|start| {
                    start + OTHER_START_BEFORE_SECS >= onset
                        && start.saturating_sub(OTHER_START_AFTER_SECS) <= onset
                })
        })
        .collect();
    nearby.sort_by_key(|reference| {
        reference
            .start_unix
            .map(|start| start.abs_diff(onset))
            .unwrap_or(u64::MAX)
    });
    if !nearby.is_empty() {
        let names: Vec<String> = nearby
            .iter()
            .take(MAX_NEARBY_STARTS)
            .map(|reference| format!("{}（PID {}）", reference.name, reference.pid))
            .collect();
        let suffix = if nearby.len() > MAX_NEARBY_STARTS {
            format!(" 等 {} 个", nearby.len())
        } else {
            String::new()
        };
        hints.push(hint(
            "correlation",
            format!("与 {} 的启动时间相近", names.join("、")) + &suffix,
        ));
    }

    let context_events: Vec<String> = journal
        .iter()
        .filter(|entry| {
            entry.ts <= onset && onset.saturating_sub(entry.ts) <= CONTEXT_LOOKBACK_SECS
        })
        .map(|entry| format!("「{}」", entry.desc))
        .chain(
            decisions
                .iter()
                .filter(|decision| {
                    decision.ts <= onset
                        && onset.saturating_sub(decision.ts) <= CONTEXT_LOOKBACK_SECS
                })
                .map(|decision| format!("引擎切换 {} → {}", decision.from, decision.to)),
        )
        .collect();
    let shown: Vec<String> = context_events
        .iter()
        .take(MAX_CONTEXT_EVENTS)
        .cloned()
        .collect();
    if !shown.is_empty() {
        let suffix = if context_events.len() > MAX_CONTEXT_EVENTS {
            format!(" 等 {} 项", context_events.len())
        } else {
            String::new()
        };
        hints.push(hint(
            "context",
            format!(
                "异常开始前 {} 内应用过：{}",
                duration_label(CONTEXT_LOOKBACK_SECS),
                shown.join("、")
            ) + &suffix,
        ));
    }

    if hints.is_empty() {
        hints.push(hint("none", "未发现同期的本地事件".into()));
    }
    hints
}

/// 时间线事件上限（不含起点标记），避免长日志把图表挤爆。
const TIMELINE_MAX_EVENTS: usize = 12;

/// 纯函数：按与 `correlate` 相同的窗口组装时间线事件，供前端图表渲染。
/// 无起点或窗口内无事件时只返回起点标记（或空）。
pub fn build_timeline(
    pid: u32,
    onset_unix: Option<u64>,
    starts: &[ProcessStartRef],
    journal: &[JournalRef],
    decisions: &[DecisionRef],
) -> Vec<TimelineEvent> {
    let Some(onset) = onset_unix else {
        return Vec::new();
    };
    let mut events = vec![TimelineEvent {
        at_unix: onset,
        kind: "onset".into(),
        label: "异常起点".into(),
    }];
    let mut candidates: Vec<TimelineEvent> = Vec::new();
    for reference in starts {
        if reference.pid == pid {
            continue;
        }
        if let Some(start) = reference.start_unix {
            if start + OTHER_START_BEFORE_SECS >= onset
                && start.saturating_sub(OTHER_START_AFTER_SECS) <= onset
            {
                candidates.push(TimelineEvent {
                    at_unix: start,
                    kind: "process-start".into(),
                    label: format!("{} 启动", reference.name),
                });
            }
        }
    }
    for entry in journal {
        if entry.ts <= onset && onset.saturating_sub(entry.ts) <= CONTEXT_LOOKBACK_SECS {
            candidates.push(TimelineEvent {
                at_unix: entry.ts,
                kind: "journal".into(),
                label: truncate_label(&entry.desc),
            });
        }
    }
    for decision in decisions {
        if decision.ts <= onset && onset.saturating_sub(decision.ts) <= CONTEXT_LOOKBACK_SECS {
            candidates.push(TimelineEvent {
                at_unix: decision.ts,
                kind: "decision".into(),
                label: format!("切换 {} → {}", decision.from, decision.to),
            });
        }
    }
    candidates.sort_by_key(|event| event.at_unix);
    candidates.truncate(TIMELINE_MAX_EVENTS);
    events.extend(candidates);
    // 整体按时间排序：起点标记按自身时间落位（通常最晚），前端按 kind 识别。
    events.sort_by_key(|event| event.at_unix);
    events
}

fn truncate_label(text: &str) -> String {
    const MAX_LABEL_CHARS: usize = 18;
    let mut clean: String = text.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() > MAX_LABEL_CHARS {
        clean = clean.chars().take(MAX_LABEL_CHARS).collect();
        clean.push('…');
    }
    clean
}

/// 只读命令：为当前确认的每个异常生成根因提示。异常为空时返回空列表。
#[tauri::command]
pub fn get_anomaly_context(
    diag: tauri::State<'_, Arc<Mutex<crate::diag::Engine>>>,
    auto: tauri::State<'_, crate::auto::AutoStateLock>,
) -> Result<Vec<AnomalyContext>, String> {
    let now_unix = chrono::Utc::now().timestamp().max(0) as u64;
    let (anomalies, starts) = {
        let engine = diag.lock().unwrap_or_else(|e| e.into_inner());
        (
            engine.current.anomalies.clone(),
            engine
                .tracked_process_starts()
                .into_iter()
                .map(|(pid, name, start_unix)| ProcessStartRef {
                    pid,
                    name,
                    start_unix,
                })
                .collect::<Vec<_>>(),
        )
    };
    if anomalies.is_empty() {
        return Ok(Vec::new());
    }
    let decisions: Vec<DecisionRef> = {
        let state = auto.lock().unwrap_or_else(|e| e.into_inner());
        state
            .adaptive
            .decisions
            .iter()
            .map(|log| DecisionRef {
                ts: log.ts,
                from: log.from.clone(),
                to: log.to.clone(),
            })
            .collect()
    };
    let journal: Vec<JournalRef> = crate::journal::list_changes()
        .unwrap_or_default()
        .into_iter()
        .map(|change| JournalRef {
            ts: change.ts,
            desc: change.desc,
        })
        .collect();
    Ok(anomalies
        .into_iter()
        .map(|anomaly| {
            let own_start = starts
                .iter()
                .find(|reference| reference.pid == anomaly.pid)
                .and_then(|reference| reference.start_unix);
            let hints = correlate(
                anomaly.pid,
                anomaly.onset_unix,
                now_unix,
                own_start,
                &starts,
                &journal,
                &decisions,
            );
            let timeline = build_timeline(
                anomaly.pid,
                anomaly.onset_unix,
                &starts,
                &journal,
                &decisions,
            );
            AnomalyContext {
                pid: anomaly.pid,
                name: anomaly.name,
                kind: anomaly.kind,
                onset_unix: anomaly.onset_unix,
                age_secs: anomaly
                    .onset_unix
                    .and_then(|onset| now_unix.checked_sub(onset)),
                hints,
                timeline,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(pid: u32, name: &str, at: u64) -> ProcessStartRef {
        ProcessStartRef {
            pid,
            name: name.to_string(),
            start_unix: Some(at),
        }
    }

    #[test]
    fn missing_onset_reports_honest_none_state() {
        let hints = correlate(1, None, 1000, None, &[], &[], &[]);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].tone, "none");
        assert!(hints[0].text.contains("未记录"));
    }

    #[test]
    fn own_recent_start_is_reported_first() {
        let hints = correlate(7, Some(1000), 1060, Some(940), &[], &[], &[]);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].tone, "recent");
        assert!(hints[0].text.contains("启动后 1 分钟"));
    }

    #[test]
    fn own_start_outside_window_is_ignored() {
        let hints = correlate(7, Some(5000), 5100, Some(1000), &[], &[], &[]);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].tone, "none");
    }

    #[test]
    fn nearby_other_process_start_is_correlated_and_capped() {
        let starts: Vec<ProcessStartRef> = vec![
            start(7, "self.exe", 900),
            start(11, "a.exe", 1000),
            start(12, "b.exe", 980),
            start(13, "c.exe", 990),
            start(14, "d.exe", 1005),
            start(15, "far.exe", 1500),
        ];
        let hints = correlate(7, Some(1000), 1100, Some(900), &starts, &[], &[]);
        let correlation = hints
            .iter()
            .find(|h| h.tone == "correlation")
            .expect("correlation hint");
        assert!(correlation.text.contains("a.exe"));
        assert!(correlation.text.contains("等"));
        assert!(!correlation.text.contains("far.exe"));
    }

    #[test]
    fn journal_and_decision_events_share_the_context_window() {
        let journal = vec![JournalRef {
            ts: 900,
            desc: "开启游戏模式".into(),
        }];
        let decisions = vec![DecisionRef {
            ts: 950,
            from: "normal".into(),
            to: "game".into(),
        }];
        let old_journal = vec![JournalRef {
            ts: 4000,
            desc: "太晚".into(),
        }];
        let hints = correlate(7, Some(1000), 1200, None, &[], &journal, &decisions);
        let context = hints
            .iter()
            .find(|h| h.tone == "context")
            .expect("context hint");
        assert!(context.text.contains("开启游戏模式"));
        assert!(context.text.contains("normal → game"));
        let hints = correlate(7, Some(1000), 1200, None, &[], &old_journal, &[]);
        assert!(hints.iter().all(|h| h.tone != "context"));
    }

    #[test]
    fn onset_in_the_future_is_rejected() {
        let hints = correlate(7, Some(2000), 1000, None, &[], &[], &[]);
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].tone, "none");
        assert!(hints[0].text.contains("时钟"));
    }

    #[test]
    fn timeline_marks_onset_and_same_window_events_sorted() {
        let starts = vec![start(11, "a.exe", 980)];
        let journal = vec![JournalRef {
            ts: 900,
            desc: "开启游戏模式".into(),
        }];
        let decisions = vec![DecisionRef {
            ts: 950,
            from: "normal".into(),
            to: "game".into(),
        }];
        let events = build_timeline(7, Some(1000), &starts, &journal, &decisions);
        assert!(events
            .iter()
            .any(|e| e.kind == "onset" && e.at_unix == 1000));
        assert!(events
            .iter()
            .any(|e| e.kind == "process-start" && e.label.contains("a.exe")));
        assert!(events.iter().any(|e| e.kind == "journal"));
        assert!(events.iter().any(|e| e.kind == "decision"));
        assert!(events.windows(2).all(|w| w[0].at_unix <= w[1].at_unix));
    }

    #[test]
    fn timeline_excludes_self_out_of_window_and_unknown_onset() {
        let starts = vec![start(7, "self.exe", 900), start(9, "far.exe", 2000)];
        let events = build_timeline(7, Some(1000), &starts, &[], &[]);
        assert_eq!(events.len(), 1, "only the onset marker remains");
        assert_eq!(build_timeline(7, None, &starts, &[], &[]).len(), 0);
    }

    #[test]
    fn timeline_caps_events_and_truncates_labels() {
        let journal: Vec<JournalRef> = (0..20)
            .map(|i| JournalRef {
                ts: 990 - i,
                desc: format!("变更 {i:04}"),
            })
            .collect();
        let events = build_timeline(7, Some(1000), &[], &journal, &[]);
        assert_eq!(events.len(), 1 + TIMELINE_MAX_EVENTS);
        let long = build_timeline(
            7,
            Some(1000),
            &[],
            &[JournalRef {
                ts: 990,
                desc: "很长的变更描述".repeat(8),
            }],
            &[],
        );
        let journal_event = long
            .iter()
            .find(|e| e.kind == "journal")
            .expect("journal event present");
        assert!(journal_event.label.ends_with('…'));
        assert!(journal_event.label.chars().count() <= 19);
    }
}
