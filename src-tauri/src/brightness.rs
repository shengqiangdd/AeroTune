// 屏幕亮度执行器：把 WMI/PowerShell 调用从 AutoState 锁内移出。
//
// 背景：`Get-WmiObject` 一次调用约 300-400ms（其中 PowerShell 进程启动本身就占 ~230ms），
// 而自适应引擎的 tick 与所有 UI 命令共用同一把 `AutoState` 锁。
// 锁内做 WMI 会把 UI 命令一起卡住数百毫秒，表现为"界面偶尔一卡"。
//
// 方案：锁内只做纯内存决策（记录/清除 `brightness_saved`、安排退避），
// 真正的 WMI 读写交给本模块的专用工作线程串行执行；
// 工作线程把结果回传，由下一次 tick（或调用方）在锁内合并状态。

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 亮度任务类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrightnessTask {
    /// 进入省电类场景：读取当前亮度、先持久化快照，再降低 30（不低于 20）。
    Dim,
    /// 离开省电类场景：恢复先前保存的亮度。
    Restore(u8),
    /// 仅在当前亮度仍符合 AeroTune 上次变暗结果时恢复。
    Recover(BrightnessSnapshot),
}

enum WorkerCommand {
    Task(BrightnessTask),
    Shutdown(Sender<()>),
}

enum WorkerLifecycle {
    Running,
    Stopped,
    Failed(String),
}

/// 亮度恢复快照；`dimmed` 用于识别用户/其他软件的后续改动。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BrightnessSnapshot {
    pub original: u8,
    pub dimmed: u8,
}

fn snapshot_for_original(original: u8) -> BrightnessSnapshot {
    BrightnessSnapshot {
        original,
        dimmed: original.saturating_sub(30).max(20).min(original),
    }
}

/// 工作线程回报的执行结果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrightnessOutcome {
    /// 变暗成功，返回已保存的原始亮度。
    Dimmed { original: u8 },
    /// 读到原始亮度但变暗写入失败：保留备份值，不再重复变暗。
    DimFailed { original: u8 },
    /// 恢复成功。
    Restored { value: u8 },
    /// 带目标值核验的恢复失败：保留快照，后续重试仍需通过当前值匹配校验。
    GuardedRecoveryFailed,
    /// 快照写入失败；未执行变暗，稍后再试。
    SnapshotWriteFailed,
    /// 发现屏幕亮度已被用户或其他软件改动，保留当前值并清除旧快照。
    RecoverySuperseded { current: u8 },
    /// 该显示器不支持亮度控制。
    Unsupported,
}

/// 供锁内调用的句柄：只做状态决策，不做任何 I/O。
pub struct BrightnessHandle {
    tx: Sender<WorkerCommand>,
    rx: Mutex<Receiver<BrightnessOutcome>>,
    lifecycle: Mutex<WorkerLifecycle>,
}

impl BrightnessHandle {
    /// 取回工作线程已完成的结果（非阻塞）。返回后结果即被消费。
    pub fn take_outcome(&self) -> Option<BrightnessOutcome> {
        let rx = self.rx.lock().unwrap_or_else(|error| error.into_inner());
        rx.try_recv().ok()
    }

    /// 排队一个亮度任务；退出屏障关闭或工作线程不可用时返回 false。
    pub fn request(&self, task: BrightnessTask) -> bool {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !matches!(*lifecycle, WorkerLifecycle::Running) {
            return false;
        }
        if self.tx.send(WorkerCommand::Task(task)).is_ok() {
            true
        } else {
            *lifecycle = WorkerLifecycle::Failed("亮度工作线程已退出".into());
            false
        }
    }

    /// 排空所有先前提交的亮度任务并关闭工作线程；之后拒绝新任务。
    fn shutdown_and_drain(&self) -> Result<(), String> {
        self.shutdown_and_drain_with(|| {})
    }

    fn shutdown_and_drain_with(&self, after_enqueue: impl FnOnce()) -> Result<(), String> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match &*lifecycle {
            WorkerLifecycle::Stopped => return Ok(()),
            WorkerLifecycle::Failed(error) => return Err(error.clone()),
            WorkerLifecycle::Running => {}
        }
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        if self.tx.send(WorkerCommand::Shutdown(done_tx)).is_err() {
            let error = "亮度工作线程已退出；快照保留供下次启动恢复".to_string();
            *lifecycle = WorkerLifecycle::Failed(error.clone());
            return Err(error);
        }
        after_enqueue();
        match done_rx.recv() {
            Ok(()) => {
                *lifecycle = WorkerLifecycle::Stopped;
                Ok(())
            }
            Err(_) => {
                let error = "亮度工作线程未能确认排空；快照保留供下次启动恢复".to_string();
                *lifecycle = WorkerLifecycle::Failed(error.clone());
                Err(error)
            }
        }
    }
}

/// 全局单例句柄。首个调用者负责启动工作线程。
pub fn handle() -> &'static BrightnessHandle {
    static HANDLE: OnceLock<BrightnessHandle> = OnceLock::new();
    HANDLE.get_or_init(start_worker)
}

const SNAPSHOT_FILE: &str = "brightness.snapshot";
const SNAPSHOT_FORMAT: &str = "aerotune-brightness-v1";
const SNAPSHOT_MAX_BYTES: usize = 64;

fn snapshot_path() -> std::path::PathBuf {
    crate::util::app_dir().join(SNAPSHOT_FILE)
}

fn encode_snapshot(snapshot: BrightnessSnapshot) -> String {
    format!(
        "{SNAPSHOT_FORMAT}\n{}\n{}\n",
        snapshot.original, snapshot.dimmed
    )
}

fn parse_snapshot(bytes: &[u8]) -> Result<BrightnessSnapshot, String> {
    if bytes.len() > SNAPSHOT_MAX_BYTES {
        return Err("亮度恢复快照超出大小限制".into());
    }
    let content = std::str::from_utf8(bytes).map_err(|_| "亮度恢复快照不是有效 UTF-8")?;
    let mut lines = content.lines();
    if lines.next() != Some(SNAPSHOT_FORMAT) {
        return Err("亮度恢复快照版本无效".into());
    }
    let original = lines
        .next()
        .ok_or("亮度恢复快照缺少原始值")?
        .parse::<u8>()
        .map_err(|_| "亮度恢复快照原始值无效")?;
    let dimmed = lines
        .next()
        .ok_or("亮度恢复快照缺少变暗值")?
        .parse::<u8>()
        .map_err(|_| "亮度恢复快照变暗值无效")?;
    let expected_dimmed = original.saturating_sub(30).max(20).min(original);
    if lines.next().is_some() || original > 100 || dimmed != expected_dimmed {
        return Err("亮度恢复快照包含越界、非预期亮度值或额外数据".into());
    }
    Ok(BrightnessSnapshot { original, dimmed })
}

fn load_snapshot() -> Result<Option<BrightnessSnapshot>, String> {
    use std::io::Read;
    let path = snapshot_path();
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("无法读取亮度恢复快照: {error}")),
    };
    let mut bytes = Vec::with_capacity(SNAPSHOT_MAX_BYTES);
    file.take((SNAPSHOT_MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("无法读取亮度恢复快照: {error}"))?;
    parse_snapshot(&bytes).map(Some)
}

fn persist_snapshot(snapshot: BrightnessSnapshot) -> Result<(), String> {
    crate::util::atomic_write(&snapshot_path(), encode_snapshot(snapshot).as_bytes())
}

fn clear_snapshot() -> Result<(), String> {
    let path = snapshot_path();
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法清除亮度恢复快照: {error}")),
    }
}

/// 载入并安排一次启动恢复；快照不可读时 fail closed，不更改屏幕亮度。
pub fn schedule_startup_recovery(state: &mut BrightnessState) -> Result<bool, String> {
    let snapshot = match load_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => {
            state.recovery_issue = true;
            tracing::warn!(%error, "启动时亮度恢复快照不可读；保持当前屏幕亮度");
            return Err("亮度恢复快照不可读；未更改屏幕亮度，请手动核对".into());
        }
    };
    state.recovery_issue = false;
    let Some(snapshot) = snapshot else {
        state.saved = None;
        state.restore_pending = false;
        state.dim_pending = false;
        state.guarded_recovery = false;
        return Ok(false);
    };
    state.saved = Some(snapshot.original);
    state.dim_pending = false;
    state.restore_pending = true;
    state.guarded_recovery = true;
    state.retry_after = None;
    if !handle().request(BrightnessTask::Recover(snapshot)) {
        state.restore_pending = false;
        return Err("亮度恢复工作线程不可用；磁盘快照仍保留，可稍后重试".into());
    }
    Ok(true)
}

fn dim_with_snapshot(
    snapshot: BrightnessSnapshot,
    persist: impl FnOnce(BrightnessSnapshot) -> Result<(), String>,
    set_brightness: impl FnOnce(u8) -> bool,
) -> BrightnessOutcome {
    if persist(snapshot).is_err() {
        return BrightnessOutcome::SnapshotWriteFailed;
    }
    if set_brightness(snapshot.dimmed) {
        BrightnessOutcome::Dimmed {
            original: snapshot.original,
        }
    } else {
        // Keep the durable snapshot: a failed WMI write can still have partially applied.
        BrightnessOutcome::DimFailed {
            original: snapshot.original,
        }
    }
}

fn recover_snapshot_with(
    snapshot: BrightnessSnapshot,
    current: Option<u8>,
    set_if_current: impl FnOnce(u8, u8) -> crate::power::BrightnessCompareWriteResult,
    clear: impl FnOnce() -> Result<(), String>,
) -> BrightnessOutcome {
    let Some(current) = current else {
        return BrightnessOutcome::GuardedRecoveryFailed;
    };
    if current == snapshot.original {
        return if clear().is_ok() {
            BrightnessOutcome::Restored {
                value: snapshot.original,
            }
        } else {
            BrightnessOutcome::GuardedRecoveryFailed
        };
    }
    if current == snapshot.dimmed {
        return match set_if_current(snapshot.dimmed, snapshot.original) {
            crate::power::BrightnessCompareWriteResult::Applied => {
                if clear().is_ok() {
                    BrightnessOutcome::Restored {
                        value: snapshot.original,
                    }
                } else {
                    BrightnessOutcome::GuardedRecoveryFailed
                }
            }
            crate::power::BrightnessCompareWriteResult::CurrentChanged(current) => {
                if clear().is_err() {
                    BrightnessOutcome::GuardedRecoveryFailed
                } else if current == snapshot.original {
                    BrightnessOutcome::Restored {
                        value: snapshot.original,
                    }
                } else {
                    BrightnessOutcome::RecoverySuperseded { current }
                }
            }
            crate::power::BrightnessCompareWriteResult::Failed => {
                BrightnessOutcome::GuardedRecoveryFailed
            }
        };
    }
    if clear().is_ok() {
        BrightnessOutcome::RecoverySuperseded { current }
    } else {
        BrightnessOutcome::GuardedRecoveryFailed
    }
}

fn execute_guarded_recovery(snapshot: BrightnessSnapshot) -> BrightnessOutcome {
    let outcome = recover_snapshot_with(
        snapshot,
        crate::power::get_brightness(),
        crate::power::set_brightness_if_current,
        clear_snapshot,
    );
    if let BrightnessOutcome::RecoverySuperseded { current } = outcome {
        tracing::info!(
            current,
            expected_dimmed = snapshot.dimmed,
            "亮度恢复发现亮度已被外部改动；保留当前值并清除旧快照"
        );
    } else if outcome == BrightnessOutcome::GuardedRecoveryFailed {
        tracing::warn!("自动亮度恢复未完成；保留快照供校验重试");
    }
    outcome
}

fn execute_brightness_task(task: BrightnessTask) -> BrightnessOutcome {
    match task {
        BrightnessTask::Dim => match crate::power::get_brightness() {
            Some(original) => {
                let snapshot = snapshot_for_original(original);
                let outcome =
                    dim_with_snapshot(snapshot, persist_snapshot, crate::power::set_brightness);
                if outcome == BrightnessOutcome::SnapshotWriteFailed {
                    tracing::warn!("未能持久化屏幕亮度恢复快照；本次未执行自动变暗");
                }
                outcome
            }
            None => BrightnessOutcome::Unsupported,
        },
        BrightnessTask::Restore(value) => execute_guarded_recovery(snapshot_for_original(value)),
        BrightnessTask::Recover(snapshot) => execute_guarded_recovery(snapshot),
    }
}

fn run_worker_loop(
    task_rx: Receiver<WorkerCommand>,
    out_tx: Sender<BrightnessOutcome>,
    mut execute: impl FnMut(BrightnessTask) -> BrightnessOutcome,
) {
    while let Ok(command) = task_rx.recv() {
        match command {
            WorkerCommand::Task(task) => {
                if out_tx.send(execute(task)).is_err() {
                    break;
                }
            }
            WorkerCommand::Shutdown(done) => {
                let _ = done.send(());
                break;
            }
        }
    }
}

/// 启动专用工作线程。
///
/// 单独线程的理由：WMI 调用慢且要串行（同一台显示器上并发写亮度会互相覆盖）；
/// 放在这里既避免占用 AutoState 锁，也不影响监控线程的 1Hz 心跳。
fn start_worker() -> BrightnessHandle {
    let (tx, task_rx) = std::sync::mpsc::channel::<WorkerCommand>();
    let (out_tx, out_rx) = std::sync::mpsc::channel::<BrightnessOutcome>();
    std::thread::Builder::new()
        .name("aerotune-brightness".into())
        .spawn(move || run_worker_loop(task_rx, out_tx, execute_brightness_task))
        .expect("无法启动亮度工作线程");
    BrightnessHandle {
        tx,
        rx: Mutex::new(out_rx),
        lifecycle: Mutex::new(WorkerLifecycle::Running),
    }
}

/// 锁内使用的纯状态机：决定"要不要排队任务"，并合并工作线程回报的结果。
///
/// 与旧的 `brightness_transition_with_backoff` 语义保持一致：
/// - 变暗前先原子持久化原始/目标亮度，只有写前快照成功才触碰显示器；
/// - 启动恢复只在当前值仍匹配上次目标时写回，外部改动优先保留；
/// - 恢复失败时保留 `saved` 并进入 5 分钟退避，成功后清空快照与退避。
#[derive(Default)]
pub struct BrightnessState {
    /// 进入省电类场景时保存的屏幕亮度。
    pub saved: Option<u8>,
    /// 亮度恢复失败后退避时间。
    pub retry_after: Option<Instant>,
    /// 是否已经排队过"变暗"任务（防止每个 tick 重复排队）。
    pub dim_pending: bool,
    /// 是否已经排队过恢复任务（避免等待 WMI 时重复排队）。
    pub restore_pending: bool,
    /// 平台不提供亮度控制：不再排队任何亮度任务。
    pub unsupported: bool,
    /// 启动恢复记录不可读时禁止猜测恢复值，等待用户显式重试读取。
    pub recovery_issue: bool,
    /// 恢复须核对预期变暗值，避免无条件覆盖用户后续改动。
    pub guarded_recovery: bool,
    /// 最近一次保留的不同亮度值及提示到期时间。
    pub preserved_notice: Option<(u8, Instant)>,
}

const RETRY_BACKOFF: Duration = Duration::from_secs(5 * 60);
const PRESERVED_NOTICE_DURATION: Duration = Duration::from_secs(30);

impl BrightnessState {
    pub fn preserved_notice_value(&self, now: Instant) -> Option<u8> {
        match &self.preserved_notice {
            Some((value, expires)) if now < *expires => Some(*value),
            _ => None,
        }
    }

    /// 合并工作线程回报的结果。
    pub fn apply_outcome(&mut self, outcome: BrightnessOutcome) {
        self.preserved_notice = match outcome {
            BrightnessOutcome::RecoverySuperseded { current } => {
                Some((current, Instant::now() + PRESERVED_NOTICE_DURATION))
            }
            _ => None,
        };
        match outcome {
            BrightnessOutcome::Dimmed { original } => {
                self.saved = Some(original);
                self.dim_pending = false;
                self.restore_pending = false;
                self.retry_after = None;
                self.recovery_issue = false;
                self.guarded_recovery = true;
            }
            BrightnessOutcome::DimFailed { original } => {
                // WMI 失败时可能已部分生效：保留备份并在恢复前核对预期亮度。
                self.saved = Some(original);
                self.dim_pending = false;
                self.restore_pending = false;
                self.recovery_issue = false;
                self.guarded_recovery = true;
            }
            BrightnessOutcome::Restored { .. } => {
                self.saved = None;
                self.dim_pending = false;
                self.restore_pending = false;
                self.retry_after = None;
                self.recovery_issue = false;
                self.guarded_recovery = false;
            }
            BrightnessOutcome::RecoverySuperseded { .. } => {
                self.saved = None;
                self.dim_pending = false;
                self.restore_pending = false;
                self.retry_after = None;
                self.recovery_issue = false;
                self.guarded_recovery = false;
            }
            BrightnessOutcome::SnapshotWriteFailed => {
                self.saved = None;
                self.dim_pending = false;
                self.restore_pending = false;
                self.retry_after = Some(Instant::now() + RETRY_BACKOFF);
                self.guarded_recovery = false;
            }
            BrightnessOutcome::GuardedRecoveryFailed => {
                self.dim_pending = false;
                self.restore_pending = false;
                self.guarded_recovery = true;
                if self.saved.is_some() {
                    self.retry_after = Some(Instant::now() + RETRY_BACKOFF);
                }
            }
            BrightnessOutcome::Unsupported => {
                // 平台不支持：不再排队，避免每个 tick 都起 PowerShell。
                self.dim_pending = false;
                self.restore_pending = false;
                self.saved = None;
                self.unsupported = true;
                self.recovery_issue = false;
                self.guarded_recovery = false;
            }
        }
    }

    /// 场景切换后调用：返回是否需要排队亮度任务。
    ///
    /// `dim = true` 表示进入省电类场景需要变暗；`false` 表示需要恢复。
    /// 返回 `None` 表示当前无需动作。
    pub fn plan(&mut self, dim: bool, now: Instant) -> Option<BrightnessTask> {
        if self.unsupported || self.recovery_issue {
            return None;
        }
        // 先合并已完成的结果，避免基于过期状态排队。
        if dim {
            if self.saved.is_some() || self.dim_pending {
                return None;
            }
            if self.retry_after.is_some_and(|deadline| now < deadline) {
                return None;
            }
            self.preserved_notice = None;
            self.dim_pending = true;
            Some(BrightnessTask::Dim)
        } else {
            let original = self.saved?;
            if self.restore_pending || self.retry_after.is_some_and(|deadline| now < deadline) {
                return None;
            }
            self.restore_pending = true;
            if self.guarded_recovery {
                Some(BrightnessTask::Recover(snapshot_for_original(original)))
            } else {
                Some(BrightnessTask::Restore(original))
            }
        }
    }

    /// 是否还有待恢复的亮度（用于"存在保存值才尝试恢复"的判定）。
    pub fn has_saved(&self) -> bool {
        self.saved.is_some()
    }
}

/// 兼容旧字段名的构造：从 `Option<u8>` 初始化。
impl From<Option<u8>> for BrightnessState {
    fn from(saved: Option<u8>) -> Self {
        BrightnessState {
            guarded_recovery: saved.is_some(),
            saved,
            ..Default::default()
        }
    }
}

/// 退出时同步恢复亮度。
///
/// 退出路径没有"卡顿"顾虑，同步核验实时值后按快照条件恢复；
/// 否则应用在变暗场景中退出时，屏幕会一直保持被降低的亮度。
pub fn restore_on_exit(saved: Option<u8>) {
    if let Err(error) = handle().shutdown_and_drain() {
        tracing::error!(%error, "退出时无法确认亮度工作线程已排空；保留恢复快照供下次启动处理");
        return;
    }
    let (snapshot, source) = match load_snapshot() {
        Ok(Some(snapshot)) => (Some(snapshot), "磁盘快照"),
        Ok(None) => (saved.map(snapshot_for_original), "进程内快照"),
        Err(error) => {
            tracing::warn!(%error, "退出时无法读取亮度恢复快照；改用进程内快照并核验实时亮度");
            (saved.map(snapshot_for_original), "进程内快照")
        }
    };
    let Some(snapshot) = snapshot else {
        return;
    };
    match recover_snapshot_with(
        snapshot,
        crate::power::get_brightness(),
        crate::power::set_brightness_if_current,
        clear_snapshot,
    ) {
        BrightnessOutcome::Restored { .. } => {}
        BrightnessOutcome::RecoverySuperseded { current } => {
            tracing::info!(
                current,
                source,
                "退出时检测到屏幕亮度已被外部改动；保留当前值"
            );
        }
        BrightnessOutcome::GuardedRecoveryFailed => {
            tracing::warn!(
                source,
                "退出时未能核验或恢复屏幕亮度；不做无条件覆盖，请手动核对当前值"
            );
        }
        _ => tracing::warn!(source, "退出时未能确认屏幕亮度恢复；请手动核对当前值"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_is_queued_once_and_saved_after_outcome() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        assert_eq!(state.plan(true, now), Some(BrightnessTask::Dim));
        // 排队后不再重复排队。
        assert_eq!(state.plan(true, now), None);
        state.apply_outcome(BrightnessOutcome::Dimmed { original: 60 });
        assert_eq!(state.saved, Some(60));
        assert_eq!(state.plan(true, now), None);
    }

    #[test]
    fn restore_is_queued_only_when_saved_exists() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        assert_eq!(state.plan(false, now), None);
        state.apply_outcome(BrightnessOutcome::Dimmed { original: 70 });
        assert_eq!(
            state.plan(false, now),
            Some(BrightnessTask::Recover(BrightnessSnapshot {
                original: 70,
                dimmed: 40,
            }))
        );
    }

    #[test]
    fn legacy_saved_backup_and_retry_stay_guarded() {
        let mut state = BrightnessState::from(Some(70));
        let now = Instant::now();
        let expected = Some(BrightnessTask::Recover(BrightnessSnapshot {
            original: 70,
            dimmed: 40,
        }));
        assert!(state.guarded_recovery);
        assert_eq!(state.plan(false, now), expected);

        state.apply_outcome(BrightnessOutcome::GuardedRecoveryFailed);
        assert!(state.guarded_recovery);
        assert_eq!(state.plan(false, now), None);
        assert_eq!(
            state.plan(
                false,
                Instant::now() + RETRY_BACKOFF + Duration::from_secs(1)
            ),
            expected
        );
    }

    #[test]
    fn live_guarded_restore_preserves_manual_brightness_changes() {
        use std::cell::Cell;

        let mut state = BrightnessState::default();
        state.apply_outcome(BrightnessOutcome::Dimmed { original: 70 });
        let Some(BrightnessTask::Recover(snapshot)) = state.plan(false, Instant::now()) else {
            panic!("successful dim must schedule a guarded restore");
        };
        assert_eq!(
            snapshot,
            BrightnessSnapshot {
                original: 70,
                dimmed: 40
            }
        );

        let changed = Cell::new(false);
        let cleared = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(55),
            |_, _| {
                changed.set(true);
                crate::power::BrightnessCompareWriteResult::Applied
            },
            || {
                cleared.set(true);
                Ok(())
            },
        );
        assert_eq!(
            outcome,
            BrightnessOutcome::RecoverySuperseded { current: 55 }
        );
        state.apply_outcome(outcome);
        assert!(
            !changed.get(),
            "a manual brightness change must not be overwritten"
        );
        assert!(cleared.get(), "the superseded snapshot should be cleared");
        assert_eq!(state.saved, None);
        assert_eq!(state.plan(false, Instant::now()), None);
    }

    #[test]
    fn restore_is_not_queued_twice_while_worker_is_pending() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        state.apply_outcome(BrightnessOutcome::Dimmed { original: 70 });
        assert_eq!(
            state.plan(false, now),
            Some(BrightnessTask::Recover(BrightnessSnapshot {
                original: 70,
                dimmed: 40,
            }))
        );
        assert!(state.restore_pending);
        assert_eq!(state.plan(false, now), None);

        state.apply_outcome(BrightnessOutcome::GuardedRecoveryFailed);
        assert!(!state.restore_pending);
        assert!(state.has_saved());
    }

    #[test]
    fn failed_restore_keeps_saved_and_enters_backoff() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        state.apply_outcome(BrightnessOutcome::Dimmed { original: 70 });
        state.apply_outcome(BrightnessOutcome::GuardedRecoveryFailed);
        assert_eq!(state.saved, Some(70));
        // 退避期内不再排队恢复。
        assert_eq!(state.plan(false, now), None);
        // 退避期后重新排队。
        let later = now + RETRY_BACKOFF + Duration::from_secs(1);
        assert_eq!(
            state.plan(false, later),
            Some(BrightnessTask::Recover(BrightnessSnapshot {
                original: 70,
                dimmed: 40,
            }))
        );
    }

    #[test]
    fn successful_restore_clears_saved_and_backoff() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        state.apply_outcome(BrightnessOutcome::Dimmed { original: 70 });
        state.apply_outcome(BrightnessOutcome::GuardedRecoveryFailed);
        state.apply_outcome(BrightnessOutcome::Restored { value: 70 });
        assert_eq!(state.saved, None);
        assert!(state.retry_after.is_none());
        assert_eq!(state.plan(false, now), None);
    }

    #[test]
    fn failed_dim_write_keeps_backup_and_uses_guarded_recovery() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        state.apply_outcome(BrightnessOutcome::DimFailed { original: 70 });
        assert_eq!(state.saved, Some(70));
        assert!(state.guarded_recovery);
        assert_eq!(state.plan(true, now), None);
        assert_eq!(
            state.plan(false, now),
            Some(BrightnessTask::Recover(BrightnessSnapshot {
                original: 70,
                dimmed: 40,
            }))
        );
    }

    #[test]
    fn failed_partial_dim_does_not_overwrite_a_later_manual_brightness() {
        use std::cell::Cell;

        let mut state = BrightnessState::default();
        let now = Instant::now();
        state.apply_outcome(BrightnessOutcome::DimFailed { original: 70 });
        let Some(BrightnessTask::Recover(snapshot)) = state.plan(false, now) else {
            panic!("failed dim must use guarded recovery");
        };
        let written = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(55),
            |_, _| {
                written.set(true);
                crate::power::BrightnessCompareWriteResult::Applied
            },
            || Ok(()),
        );
        assert_eq!(
            outcome,
            BrightnessOutcome::RecoverySuperseded { current: 55 }
        );
        state.apply_outcome(outcome);
        assert!(!written.get(), "later manual value must remain untouched");
        assert_eq!(state.saved, None);
        assert_eq!(state.preserved_notice_value(Instant::now()), Some(55));
    }

    #[test]
    fn snapshot_format_is_bounded_and_rejects_invalid_values() {
        let snapshot = BrightnessSnapshot {
            original: 70,
            dimmed: 40,
        };
        assert_eq!(
            parse_snapshot(encode_snapshot(snapshot).as_bytes()),
            Ok(snapshot)
        );
        assert!(parse_snapshot(b"wrong-version\n70\n40\n").is_err());
        assert!(parse_snapshot(b"aerotune-brightness-v1\n101\n40\n").is_err());
        assert!(parse_snapshot(b"aerotune-brightness-v1\n40\n70\n").is_err());
        assert!(parse_snapshot(b"aerotune-brightness-v1\n70\n39\n").is_err());
        assert!(parse_snapshot(&[b'x'; SNAPSHOT_MAX_BYTES + 1]).is_err());
    }

    #[test]
    fn dim_persists_snapshot_before_touching_display() {
        use std::cell::Cell;

        let persisted = Cell::new(false);
        let snapshot = BrightnessSnapshot {
            original: 70,
            dimmed: 40,
        };
        let outcome = dim_with_snapshot(
            snapshot,
            |_| {
                persisted.set(true);
                Ok(())
            },
            |_| {
                assert!(
                    persisted.get(),
                    "brightness must not change before durable snapshot"
                );
                true
            },
        );
        assert_eq!(outcome, BrightnessOutcome::Dimmed { original: 70 });

        let persisted_after_failed_write = Cell::new(false);
        let outcome = dim_with_snapshot(
            snapshot,
            |_| {
                persisted_after_failed_write.set(true);
                Ok(())
            },
            |_| false,
        );
        assert_eq!(outcome, BrightnessOutcome::DimFailed { original: 70 });
        assert!(
            persisted_after_failed_write.get(),
            "possibly partial dim writes retain their durable backup"
        );

        let changed = Cell::new(false);
        let outcome = dim_with_snapshot(
            snapshot,
            |_| Err("injected snapshot write failure".into()),
            |_| {
                changed.set(true);
                true
            },
        );
        assert_eq!(outcome, BrightnessOutcome::SnapshotWriteFailed);
        assert!(
            !changed.get(),
            "failed write-ahead persistence must prevent dimming"
        );

        let mut state = BrightnessState::default();
        state.apply_outcome(outcome);
        assert!(!state.has_saved());
        assert_eq!(state.plan(true, Instant::now()), None);
    }

    #[test]
    fn recovery_matrix_restores_only_matching_dim_targets() {
        use std::cell::Cell;

        for original in 0..=100 {
            let snapshot = snapshot_for_original(original);
            for current in 0..=100 {
                let written = Cell::new(None);
                let cleared = Cell::new(false);
                let outcome = recover_snapshot_with(
                    snapshot,
                    Some(current),
                    |_, value| {
                        written.set(Some(value));
                        crate::power::BrightnessCompareWriteResult::Applied
                    },
                    || {
                        cleared.set(true);
                        Ok(())
                    },
                );
                if current == snapshot.original {
                    assert_eq!(outcome, BrightnessOutcome::Restored { value: original });
                    assert_eq!(
                        written.get(),
                        None,
                        "original value should not be rewritten"
                    );
                } else if current == snapshot.dimmed {
                    assert_eq!(outcome, BrightnessOutcome::Restored { value: original });
                    assert_eq!(written.get(), Some(original));
                } else {
                    assert_eq!(outcome, BrightnessOutcome::RecoverySuperseded { current });
                    assert_eq!(
                        written.get(),
                        None,
                        "unmatched value must not be overwritten"
                    );
                }
                assert!(cleared.get());
            }
        }
    }

    #[test]
    fn startup_recovery_restores_only_the_expected_dimmed_value() {
        use std::cell::Cell;

        let snapshot = BrightnessSnapshot {
            original: 70,
            dimmed: 40,
        };
        let restored_value = Cell::new(None);
        let cleared = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(40),
            |_, value| {
                restored_value.set(Some(value));
                crate::power::BrightnessCompareWriteResult::Applied
            },
            || {
                cleared.set(true);
                Ok(())
            },
        );
        assert_eq!(outcome, BrightnessOutcome::Restored { value: 70 });
        assert_eq!(restored_value.get(), Some(70));
        assert!(cleared.get());

        let changed = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(55),
            |_, _| {
                changed.set(true);
                crate::power::BrightnessCompareWriteResult::Applied
            },
            || Ok(()),
        );
        assert_eq!(
            outcome,
            BrightnessOutcome::RecoverySuperseded { current: 55 }
        );
        assert!(
            !changed.get(),
            "external brightness must not be overwritten"
        );

        let removed = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(40),
            |_, _| crate::power::BrightnessCompareWriteResult::Failed,
            || {
                removed.set(true);
                Ok(())
            },
        );
        assert_eq!(outcome, BrightnessOutcome::GuardedRecoveryFailed);
        assert!(
            !removed.get(),
            "failed restore must keep the durable snapshot"
        );
    }

    #[test]
    fn guarded_restore_preserves_a_change_since_the_initial_sample() {
        use std::cell::Cell;

        let snapshot = BrightnessSnapshot {
            original: 70,
            dimmed: 40,
        };
        let cleared = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(40),
            |expected, target| {
                assert_eq!(expected, 40);
                assert_eq!(target, 70);
                crate::power::BrightnessCompareWriteResult::CurrentChanged(55)
            },
            || {
                cleared.set(true);
                Ok(())
            },
        );
        assert_eq!(
            outcome,
            BrightnessOutcome::RecoverySuperseded { current: 55 }
        );
        assert!(cleared.get());
    }

    #[test]
    fn guarded_restore_accepts_original_value_seen_by_recheck() {
        use std::cell::Cell;

        let snapshot = BrightnessSnapshot {
            original: 70,
            dimmed: 40,
        };
        let rechecked = Cell::new(false);
        let cleared = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(40),
            |expected, target| {
                assert_eq!(expected, 40);
                assert_eq!(target, 70);
                rechecked.set(true);
                crate::power::BrightnessCompareWriteResult::CurrentChanged(70)
            },
            || {
                cleared.set(true);
                Ok(())
            },
        );
        assert_eq!(outcome, BrightnessOutcome::Restored { value: 70 });
        assert!(rechecked.get());
        assert!(cleared.get());
    }

    #[test]
    fn startup_retries_remain_guarded_against_external_brightness_changes() {
        let mut state = BrightnessState::from(Some(70));
        state.apply_outcome(BrightnessOutcome::GuardedRecoveryFailed);
        assert!(state.guarded_recovery);
        let after_backoff = Instant::now() + RETRY_BACKOFF + Duration::from_secs(1);
        assert_eq!(
            state.plan(false, after_backoff),
            Some(BrightnessTask::Recover(BrightnessSnapshot {
                original: 70,
                dimmed: 40,
            }))
        );
    }

    #[test]
    fn preserved_brightness_notice_expires_and_clears_before_a_new_dim() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        state.apply_outcome(BrightnessOutcome::RecoverySuperseded { current: 55 });
        assert_eq!(state.preserved_notice_value(now), Some(55));
        assert_eq!(
            state.preserved_notice_value(now + PRESERVED_NOTICE_DURATION + Duration::from_secs(1)),
            None
        );
        assert_eq!(state.plan(true, now), Some(BrightnessTask::Dim));
        assert_eq!(state.preserved_notice_value(now), None);
    }

    #[test]
    fn exit_barrier_drains_inflight_dim_before_closing_worker() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let (task_tx, task_rx) = std::sync::mpsc::channel();
        let (out_tx, out_rx) = std::sync::mpsc::channel();
        let handle = Arc::new(BrightnessHandle {
            tx: task_tx,
            rx: Mutex::new(out_rx),
            lifecycle: Mutex::new(WorkerLifecycle::Running),
        });
        let (task_started_tx, task_started_rx) = std::sync::mpsc::channel();
        let (release_task_tx, release_task_rx) = std::sync::mpsc::channel();
        let dim_finished = Arc::new(AtomicBool::new(false));
        let dim_finished_worker = Arc::clone(&dim_finished);
        let worker = std::thread::spawn(move || {
            run_worker_loop(task_rx, out_tx, move |task| {
                assert_eq!(task, BrightnessTask::Dim);
                task_started_tx.send(()).unwrap();
                release_task_rx.recv().unwrap();
                dim_finished_worker.store(true, Ordering::SeqCst);
                BrightnessOutcome::Dimmed { original: 70 }
            });
        });

        assert!(handle.request(BrightnessTask::Dim));
        task_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("worker must enter the in-flight dim task");

        let (barrier_queued_tx, barrier_queued_rx) = std::sync::mpsc::channel();
        let (shutdown_result_tx, shutdown_result_rx) = std::sync::mpsc::channel();
        let shutdown_handle = Arc::clone(&handle);
        let shutdown = std::thread::spawn(move || {
            let result = shutdown_handle.shutdown_and_drain_with(|| {
                barrier_queued_tx.send(()).unwrap();
            });
            shutdown_result_tx.send(result).unwrap();
        });
        barrier_queued_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown barrier must be queued behind the in-flight task");
        assert!(shutdown_result_rx
            .recv_timeout(Duration::from_millis(25))
            .is_err());
        assert!(!dim_finished.load(Ordering::SeqCst));

        release_task_tx.send(()).unwrap();
        shutdown_result_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("shutdown completes only after the in-flight task")
            .unwrap();
        assert!(dim_finished.load(Ordering::SeqCst));
        assert!(!handle.request(BrightnessTask::Restore(70)));
        worker.join().unwrap();
        shutdown.join().unwrap();
    }

    #[test]
    fn unreadable_snapshot_blocks_automatic_brightness_writes() {
        let mut state = BrightnessState {
            recovery_issue: true,
            ..Default::default()
        };
        let now = Instant::now();
        assert_eq!(state.plan(true, now), None);
        assert_eq!(state.plan(false, now), None);
    }

    #[test]
    fn recovery_of_already_restored_value_only_clears_snapshot() {
        use std::cell::Cell;

        let snapshot = BrightnessSnapshot {
            original: 70,
            dimmed: 40,
        };
        let changed = Cell::new(false);
        let cleared = Cell::new(false);
        let outcome = recover_snapshot_with(
            snapshot,
            Some(70),
            |_, _| {
                changed.set(true);
                crate::power::BrightnessCompareWriteResult::Applied
            },
            || {
                cleared.set(true);
                Ok(())
            },
        );
        assert_eq!(outcome, BrightnessOutcome::Restored { value: 70 });
        assert!(!changed.get());
        assert!(cleared.get());
    }

    #[test]
    fn recovery_superseded_clears_stale_in_memory_backup() {
        let mut state = BrightnessState::from(Some(70));
        state.restore_pending = true;
        state.apply_outcome(BrightnessOutcome::RecoverySuperseded { current: 55 });
        assert!(!state.has_saved());
        assert!(!state.restore_pending);
        assert!(!state.recovery_issue);
    }

    #[test]
    fn unsupported_platform_stops_queuing() {
        let mut state = BrightnessState::default();
        let now = Instant::now();
        assert_eq!(state.plan(true, now), Some(BrightnessTask::Dim));
        state.apply_outcome(BrightnessOutcome::Unsupported);
        assert_eq!(state.plan(true, now), None);
        assert!(!state.has_saved());
    }
}
