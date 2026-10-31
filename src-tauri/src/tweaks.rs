use serde::Serialize;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const GAME_CONFIG_STORE: &str = r"HKCU\System\GameConfigStore";
const GAMEDVR_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\GameDVR";
const GRAPHICS_DRIVERS: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers";
const PRIORITY_CONTROL: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\PriorityControl";

/// 手动模式操作（托盘/页面/命令）临时覆盖自适应引擎的窗口
static MANUAL_HOLD: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));
const MANUAL_HOLD_SECS: u64 = 300;

pub fn manual_hold_active() -> bool {
    MANUAL_HOLD
        .lock()
        .unwrap()
        .map(|t| t.elapsed() < Duration::from_secs(MANUAL_HOLD_SECS))
        .unwrap_or(false)
}

pub fn manual_hold_secs_left() -> u64 {
    MANUAL_HOLD
        .lock()
        .unwrap()
        .map(|t| MANUAL_HOLD_SECS.saturating_sub(t.elapsed().as_secs()))
        .unwrap_or(0)
}

fn mode_path() -> std::path::PathBuf {
    crate::util::app_dir().join("mode.json")
}

pub fn current_mode() -> String {
    std::fs::read_to_string(mode_path())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "none".into())
}

fn write_mode_with(
    mode: &str,
    persist: impl FnOnce(&[u8]) -> Result<(), String>,
) -> Result<(), String> {
    persist(mode.as_bytes()).map_err(|error| format!("模式状态保存失败：{error}"))
}

pub(crate) fn write_mode(mode: &str) -> Result<(), String> {
    write_mode_with(mode, |bytes| crate::util::atomic_write(&mode_path(), bytes))
}

#[derive(Serialize, Clone)]
pub struct TweakStates {
    pub gamedvr: bool,
    pub hags: Option<bool>,
    pub priority_boost: bool,
}

pub(crate) fn set_gamedvr(on: bool) -> Result<(), String> {
    let v: u32 = if on { 1 } else { 0 };
    let desc = if on {
        "恢复 Xbox Game Bar / GameDVR".to_string()
    } else {
        "关闭 GameDVR（游戏录制后台开销）".to_string()
    };
    crate::journal::set_reg_dword(GAME_CONFIG_STORE, "GameDVR_Enabled", v, &desc)?;
    crate::journal::set_reg_dword(GAMEDVR_KEY, "AppCaptureEnabled", v, &desc)
}

fn set_hags(on: bool) -> Result<(), String> {
    let v: u32 = if on { 2 } else { 1 };
    crate::journal::set_reg_dword(
        GRAPHICS_DRIVERS,
        "HwSchMode",
        v,
        &format!(
            "硬件加速 GPU 计划（HAGS）{}",
            if on { "开启" } else { "关闭" }
        ),
    )
}

pub(crate) fn set_priority_boost(on: bool) -> Result<(), String> {
    set_priority_separation(if on { 0x26 } else { 2 }, "自动适配前台响应")
}

pub(crate) fn set_priority_separation(value: u32, label: &str) -> Result<(), String> {
    if !matches!(value, 0x18 | 0x26 | 0x28 | 0x2a | 2) {
        return Err("前台响应策略值不在可用档位内".into());
    }
    crate::journal::set_reg_dword(
        PRIORITY_CONTROL,
        "Win32PrioritySeparation",
        value,
        &format!("{label} (0x{value:02x})"),
    )
}

/// 读取各调优项当前状态（用于渲染开关）
pub(crate) fn read_states() -> TweakStates {
    let gamedvr = crate::journal::read_reg_dword(GAME_CONFIG_STORE, "GameDVR_Enabled")
        .map(|v| v != 0)
        .unwrap_or(true)
        && crate::journal::read_reg_dword(GAMEDVR_KEY, "AppCaptureEnabled")
            .map(|v| v != 0)
            .unwrap_or(true);
    let hags = crate::journal::read_reg_dword(GRAPHICS_DRIVERS, "HwSchMode").map(|v| v == 2);
    let priority_boost =
        crate::journal::read_reg_dword(PRIORITY_CONTROL, "Win32PrioritySeparation")
            .map(|v| v == 0x26)
            .unwrap_or(false);
    TweakStates {
        gamedvr,
        hags,
        priority_boost,
    }
}

#[tauri::command]
pub fn get_tweak_states() -> TweakStates {
    read_states()
}

/// 单项调优开关（自动记录可还原变更；同步自适应引擎的旋钮快照）
fn validate_tweak_access(key: &str, is_admin: bool) -> Result<(), String> {
    if !is_admin {
        let reason = match key {
            "hags" => Some("HAGS 调优需要管理员权限"),
            "priority_boost" => Some("前台响应优先调优需要管理员权限"),
            _ => None,
        };
        if let Some(reason) = reason {
            return Err(format!("{reason}，请在「设置」页提权后重试"));
        }
    }
    Ok(())
}

#[tauri::command]
pub fn set_tweak(
    key: String,
    on: bool,
    auto: tauri::State<'_, crate::auto::AutoStateLock>,
) -> Result<&'static str, String> {
    validate_tweak_access(&key, crate::util::is_admin())?;
    match key.as_str() {
        "gamedvr" => set_gamedvr(on)?,
        "hags" => {
            // HAGS 需重启生效且影响图形栈，属"重大变更"
            crate::admin::auto_restore_point("修改 HAGS（硬件加速 GPU 计划）");
            set_hags(on)?;
        }
        "priority_boost" => set_priority_boost(on)?,
        _ => return Err(format!("未知调优项: {key}")),
    }
    // 引擎快照跟随真实改动，避免下次切换误判
    if let Ok(mut st) = auto.try_lock() {
        if let Some(applied) = st.adaptive.applied.as_mut() {
            match key.as_str() {
                "gamedvr" => applied.gamedvr = on,
                "priority_boost" => applied.prio_boost = on,
                _ => {}
            }
        }
    }
    tracing::info!(key = %key, on, "手动调优已应用");
    Ok("已应用，可在「设置 → 变更日志」还原")
}

/// 尝试设置 EPP 并在写入前持久化可验证的恢复状态。
fn try_epp(v: u32, label: &str, applied: &mut Vec<String>) -> Result<(), String> {
    if !crate::power::epp_supported() {
        applied.push("当前平台不支持 EPP，已跳过该项".into());
        return Ok(());
    }
    let Some(old) = crate::power::get_epp_value() else {
        applied.push("无法读取当前 EPP；为避免无快照修改，已跳过该项".into());
        return Ok(());
    };
    let description = format!("能效偏好 EPP → {v}（{label}）");
    let outcome = crate::journal::set_epp_with_journal(
        &description,
        old,
        v,
        || crate::power::set_epp_raw(Some(v)),
        || Ok(crate::power::get_epp_value()),
    )?;
    match outcome {
        crate::journal::EppWriteOutcome::Applied => applied.push(description),
        crate::journal::EppWriteOutcome::AppliedWithWarning(warning) => {
            applied.push(format!("{description}（{warning}）"));
        }
        crate::journal::EppWriteOutcome::Unchanged(reason) => {
            applied.push(format!("EPP 未变更：{reason}"));
        }
    }
    Ok(())
}

fn set_plan_with_journal(old_guid: String, guid: String, name: &str) -> Result<(), String> {
    let expected_current = old_guid.clone();
    let target_guid = guid.clone();
    crate::journal::change_power_plan_with_journal(
        old_guid,
        guid,
        &format!("切换电源计划 → {name}"),
        || crate::power::set_plan_raw_if_current(&target_guid, &expected_current),
    )
}

fn validate_mode_access(mode: &str, is_admin: bool) -> Result<(), String> {
    if !matches!(mode, "game" | "power" | "dev" | "none") {
        return Err(format!("未知模式: {mode}"));
    }
    if mode == "dev" && !is_admin {
        return Err("编码模式的前台响应优先调优需要管理员权限，请在「设置」页提权后重试".into());
    }
    Ok(())
}

fn apply_mode_steps(mode: &str, applied: &mut Vec<String>) -> Result<(), String> {
    match mode {
        "game" => {
            let cur = crate::power::list_power_plans()?
                .into_iter()
                .find(|p| p.active)
                .map(|p| p.guid)
                .unwrap_or_default();
            let (guid, name) = crate::auto::resolve_plan(&crate::auto::PlanKind::High)?;
            set_plan_with_journal(cur, guid, &name)?;
            applied.push(format!("电源计划 → {}", name));
            set_gamedvr(false)?;
            applied.push("GameDVR 已关闭".into());
            try_epp(0, "极致性能", applied)?;
        }
        "power" => {
            let cur = crate::power::list_power_plans()?
                .into_iter()
                .find(|p| p.active)
                .map(|p| p.guid)
                .unwrap_or_default();
            let (guid, name) = crate::auto::resolve_plan(&crate::auto::PlanKind::Saver)?;
            set_plan_with_journal(cur, guid, &name)?;
            applied.push(format!("电源计划 → {}", name));
            set_gamedvr(false)?;
            applied.push("GameDVR 已关闭（降低后台负载）".into());
            try_epp(85, "续航优先", applied)?;
        }
        "dev" => {
            let cur = crate::power::list_power_plans()?
                .into_iter()
                .find(|p| p.active)
                .map(|p| p.guid)
                .unwrap_or_default();
            let (guid, name) = crate::auto::resolve_plan(&crate::auto::PlanKind::Balanced)?;
            set_plan_with_journal(cur, guid, &name)?;
            applied.push(format!("电源计划 → {}", name));
            set_priority_boost(true)?;
            applied.push("前台响应优先已开启（IDE/编译更跟手）".into());
            try_epp(25, "响应优先", applied)?;
        }
        "none" => applied.push("已退出场景模式（历史变更可在设置中还原）".into()),
        _ => return Err(format!("未知模式: {mode}")),
    }
    Ok(())
}

fn run_mode_group<T>(
    apply: impl FnOnce() -> Result<T, String>,
    rollback: impl FnOnce() -> Result<usize, String>,
) -> Result<T, String> {
    match apply() {
        Ok(value) => Ok(value),
        Err(apply_error) => match rollback() {
            Ok(restored) if restored > 0 => Err(format!(
                "{apply_error}；已自动回滚 {restored} 项已记录更改"
            )),
            Ok(_) => Err(format!(
                "{apply_error}；没有已记录的变更需要回滚，请核对当前系统状态"
            )),
            Err(rollback_error) if rollback_error.contains("该恢复组没有可还原的变更") => Err(
                format!("{apply_error}；没有已记录的变更需要回滚，请核对当前系统状态"),
            ),
            Err(rollback_error) => Err(format!(
                "{apply_error}；自动回滚未能完全确认：{rollback_error}。未解决的恢复记录已保留，请检查系统状态与变更日志"
            )),
        },
    }
}

fn persist_mode_with_recovery(
    mode: &str,
    group_id: Option<u64>,
    persist: impl FnOnce() -> Result<(), String>,
    rollback: impl FnOnce(u64) -> Result<usize, String>,
) -> Result<(), String> {
    match persist() {
        Ok(()) => Ok(()),
        Err(persist_error) => {
            let Some(group_id) = group_id else {
                return Err(format!(
                    "场景状态「{mode}」保存失败；本次请求没有可回滚的已记录场景变更：{persist_error}"
                ));
            };
            match rollback(group_id) {
                Ok(restored) if restored > 0 => Err(format!(
                    "场景状态「{mode}」保存失败，已自动回滚 {restored} 项本次场景变更：{persist_error}"
                )),
                Ok(_) => Err(format!(
                    "场景状态「{mode}」保存失败；没有已记录的变更需要回滚，请核对当前系统状态：{persist_error}"
                )),
                Err(rollback_error) => Err(format!(
                    "场景状态「{mode}」保存失败；自动回滚未能完全确认：{rollback_error}。未解决的恢复记录已保留，请检查系统状态与变更日志。原始错误：{persist_error}"
                )),
            }
        }
    }
}

/// 场景模式编排核心（同步）。game=游戏 / power=省电 / dev=编码 / none=无。
/// 供前端命令与自动引擎（auto.rs）共同调用。
pub fn apply_mode_core(mode: &str) -> Result<Vec<String>, String> {
    validate_mode_access(mode, crate::util::is_admin())?;
    let mode_label = match mode {
        "game" => "游戏",
        "power" => "省电",
        "dev" => "编码",
        _ => mode,
    };
    let mut applied: Vec<String> = Vec::new();
    let mode_group_id = if mode == "none" {
        apply_mode_steps(mode, &mut applied)?;
        None
    } else {
        let group = crate::journal::begin_change_group(&format!("场景模式：{mode_label}"))?;
        let group_id = group.id();
        let mut group_guard = Some(group);
        run_mode_group(
            || apply_mode_steps(mode, &mut applied),
            || {
                // End the thread-local group before restoring so compensation cannot be
                // mistaken for a new member of the failing group.
                drop(group_guard.take());
                crate::journal::rollback_change_group(group_id)
            },
        )?;
        drop(group_guard.take());
        Some(group_id)
    };
    persist_mode_with_recovery(
        mode,
        mode_group_id,
        || write_mode(mode),
        crate::journal::rollback_change_group,
    )?;
    // A failed or rejected manual request must not suppress the automatic policy for five minutes.
    *MANUAL_HOLD.lock().unwrap() = Some(Instant::now());
    Ok(applied)
}

/// 场景模式编排。game=游戏 / power=省电 / dev=编码 / none=无
#[tauri::command]
pub async fn apply_mode(mode: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || apply_mode_core(&mode))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_mode_state() -> String {
    current_mode()
}

// ---------- GPU 应用偏好（双显卡指定） ----------

const GPU_PREF_KEY: &str = r"HKCU\Software\Microsoft\DirectX\UserGpuPreferences";

#[derive(Serialize, Clone)]
pub struct GpuPref {
    pub exe: String,
    pub pref: u8,
}

#[tauri::command]
pub fn list_gpu_prefs() -> Vec<GpuPref> {
    let Ok(key) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\DirectX\UserGpuPreferences")
    else {
        return vec![];
    };
    key.enum_values()
        .flatten()
        .filter_map(|(exe, _)| {
            let v: String = key.get_value(&exe).ok()?;
            let pref = v
                .split(';')
                .next()
                .and_then(|s| s.strip_prefix("GpuPreference="))
                .and_then(|s| s.parse::<u8>().ok())
                .unwrap_or(0);
            Some(GpuPref { exe, pref })
        })
        .collect()
}

/// pref: 0 自动 / 1 节能（核显）/ 2 高性能（独显）
#[tauri::command]
pub fn set_gpu_pref(exe: String, pref: u8) -> Result<(), String> {
    let exe = exe.trim().trim_matches('"').to_string();
    let lower = exe.to_lowercase();
    if !lower.ends_with(".exe") || !lower.contains(":\\") {
        return Err("请填写完整的 .exe 绝对路径".into());
    }
    let label = match pref {
        0 => "自动选择",
        1 => "节能（核显）",
        2 => "高性能（独显）",
        _ => return Err("无效的 GPU 偏好".into()),
    };
    if pref == 0 {
        crate::journal::del_reg_value(
            GPU_PREF_KEY,
            &exe,
            &format!("GPU 偏好 → 自动选择（{exe}）"),
        )?;
    } else {
        crate::journal::set_reg_sz(
            GPU_PREF_KEY,
            &exe,
            &format!("GpuPreference={pref};"),
            &format!("GPU 偏好 → {label}（{exe}）"),
        )?;
    }
    tracing::info!(exe = %exe, pref, "GPU 应用偏好已设置");
    Ok(())
}

// ---------- Windows 更新控制 ----------

const WU_POLICY: &str = r"HKLM\SOFTWARE\Policies\Microsoft\Windows\WindowsUpdate\AU";

#[tauri::command]
pub fn set_update_pause(paused: bool) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("更新控制需要管理员权限".into());
    }
    if paused {
        crate::journal::set_reg_dword(
            WU_POLICY,
            "NoAutoUpdate",
            1,
            "暂停 Windows 自动更新（策略，可还原）",
        )?;
    } else {
        crate::journal::del_reg_value(WU_POLICY, "NoAutoUpdate", "恢复 Windows 自动更新")?;
    }
    tracing::info!(paused, "Windows 更新策略已切换");
    Ok(())
}

#[tauri::command]
pub fn get_update_pause() -> bool {
    crate::journal::read_reg_dword(WU_POLICY, "NoAutoUpdate")
        .map(|v| v == 1)
        .unwrap_or(false)
}

// ---------- 遥测治理 ----------

const TELEMETRY_POLICY: &str = r"HKLM\SOFTWARE\Policies\Microsoft\Windows\DataCollection";
const DIAGTRACK_SVC: &str = r"HKLM\SYSTEM\CurrentControlSet\Services\DiagTrack";

#[tauri::command]
pub fn set_telemetry_minimal(on: bool) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("遥测治理需要管理员权限".into());
    }
    if on {
        crate::journal::set_reg_dword(
            TELEMETRY_POLICY,
            "AllowTelemetry",
            0,
            "遥测策略 → 最低级别",
        )?;
        crate::journal::set_reg_dword(DIAGTRACK_SVC, "Start", 4, "禁用 DiagTrack（诊断跟踪）服务")?;
    } else {
        return Err("不猜测原始遥测/DiagTrack 配置；请在设置 → 变更日志逆序恢复对应记录。没有快照时需人工核对。".into());
    }
    tracing::info!(on, "遥测治理已切换");
    Ok(())
}

#[tauri::command]
pub fn get_telemetry_minimal() -> bool {
    crate::journal::read_reg_dword(TELEMETRY_POLICY, "AllowTelemetry")
        .map(|v| v == 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod privilege_tests {
    use super::{
        persist_mode_with_recovery, run_mode_group, validate_mode_access, validate_tweak_access,
        write_mode_with,
    };

    #[test]
    fn standard_user_is_rejected_from_dev_mode_before_writes() {
        let error = validate_mode_access("dev", false).unwrap_err();
        assert!(error.contains("管理员权限"));
        assert!(error.contains("设置"));
        assert!(validate_mode_access("dev", true).is_ok());
        assert!(validate_mode_access("power", false).is_ok());
        assert!(validate_mode_access("none", false).is_ok());
        assert!(validate_mode_access("unknown", false).is_err());
    }

    #[test]
    fn standard_user_may_change_user_scope_but_not_machine_scope_tweaks() {
        assert!(validate_tweak_access("gamedvr", false).is_ok());
        assert!(validate_tweak_access("hags", false)
            .unwrap_err()
            .contains("管理员权限"));
        assert!(validate_tweak_access("priority_boost", false)
            .unwrap_err()
            .contains("管理员权限"));
        assert!(validate_tweak_access("hags", true).is_ok());
        assert!(validate_tweak_access("priority_boost", true).is_ok());
    }

    #[test]
    fn scenario_group_rolls_back_every_prior_step_failure() {
        let scenarios = [
            ("game", ["power plan", "GameDVR", "EPP"]),
            ("power", ["power plan", "GameDVR", "EPP"]),
            ("dev", ["power plan", "priority boost", "EPP"]),
        ];
        for (mode, steps) in scenarios {
            for fail_after in 1..=steps.len() {
                let state = std::cell::RefCell::new(Vec::new());
                let result = run_mode_group(
                    || {
                        for step in steps.iter().copied() {
                            state.borrow_mut().push(step);
                            if state.borrow().len() == fail_after {
                                return Err(format!("injected {mode} {step} failure"));
                            }
                        }
                        Ok(())
                    },
                    || {
                        let mut state = state.borrow_mut();
                        let restored = state.len();
                        state.clear();
                        Ok(restored)
                    },
                );
                let error = result.unwrap_err();
                assert!(error.contains("injected"));
                assert!(error.contains("已自动回滚"));
                assert!(
                    state.borrow().is_empty(),
                    "failure in {mode} at step {fail_after} left partial state"
                );
            }
        }
    }

    #[test]
    fn scenario_group_preserves_failure_and_recovery_record_when_compensation_conflicts() {
        let state = vec!["power plan", "GameDVR"];
        let result = run_mode_group(
            || Err::<(), _>("injected EPP state conflict".into()),
            || Err("已还原 0 条；当前状态冲突，组内恢复记录保留".into()),
        );
        let error = result.unwrap_err();
        assert!(error.contains("injected EPP state conflict"));
        assert!(error.contains("自动回滚未能完全确认"));
        assert!(error.contains("恢复记录已保留"));
        assert_eq!(state, vec!["power plan", "GameDVR"]);
    }

    #[test]
    fn mode_status_write_failure_is_returned_with_context() {
        let error = write_mode_with("dev", |_| Err("injected disk failure".into())).unwrap_err();
        assert!(error.contains("模式状态保存失败"));
        assert!(error.contains("injected disk failure"));
    }

    #[test]
    fn mode_marker_failure_rolls_back_the_committed_scenario_group() {
        let mut rolled_back = None;
        let error = persist_mode_with_recovery(
            "game",
            Some(42),
            || Err("injected marker write failure".into()),
            |group_id| {
                rolled_back = Some(group_id);
                Ok(3)
            },
        )
        .unwrap_err();
        assert_eq!(rolled_back, Some(42));
        assert!(error.contains("已自动回滚 3 项"));
        assert!(error.contains("injected marker write failure"));
    }

    #[test]
    fn mode_marker_failure_keeps_unresolved_group_recovery_visible() {
        let error = persist_mode_with_recovery(
            "power",
            Some(7),
            || Err("injected marker write failure".into()),
            |_| Err("rollback conflict; recovery record retained".into()),
        )
        .unwrap_err();
        assert!(error.contains("自动回滚未能完全确认"));
        assert!(error.contains("恢复记录已保留"));
    }

    #[test]
    fn successful_scenario_group_does_not_run_rollback() {
        let mut rollback_called = false;
        let value = run_mode_group(
            || Ok::<_, String>("applied"),
            || {
                rollback_called = true;
                Ok(0)
            },
        )
        .unwrap();
        assert_eq!(value, "applied");
        assert!(!rollback_called);
    }
}
