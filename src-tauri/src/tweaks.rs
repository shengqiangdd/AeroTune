use serde::Serialize;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const GAME_CONFIG_STORE: &str = r"HKCU\System\GameConfigStore";
const GAMEDVR_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\GameDVR";
const GRAPHICS_DRIVERS: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers";
const PRIORITY_CONTROL: &str = r"HKLM\SYSTEM\CurrentControlSet\Control\PriorityControl";

/// 手动模式操作（托盘/页面/命令）临时覆盖自适应引擎的窗口
static MANUAL_HOLD: LazyLock<Mutex<Option<Instant>>> = LazyLock::new(|| Mutex::new(None));
static MODE_OPERATION_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
const MANUAL_HOLD_SECS: u64 = 300;

pub(crate) fn lock_mode_operations() -> std::sync::MutexGuard<'static, ()> {
    MODE_OPERATION_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
    pub gamedvr: Option<bool>,
    pub hags: Option<bool>,
    pub priority_boost: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_preview: Option<GameModePreview>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_preview_error: Option<String>,
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

fn set_gamedvr_if_current(
    on: bool,
    expected_store: Option<u32>,
    expected_capture: Option<u32>,
) -> Result<(), String> {
    let v: u32 = if on { 1 } else { 0 };
    let desc = if on {
        "恢复 Xbox Game Bar / GameDVR".to_string()
    } else {
        "关闭 GameDVR（游戏录制后台开销）".to_string()
    };
    crate::journal::set_reg_dword_if_current(
        GAME_CONFIG_STORE,
        "GameDVR_Enabled",
        v,
        expected_store,
        &desc,
    )?;
    crate::journal::set_reg_dword_if_current(
        GAMEDVR_KEY,
        "AppCaptureEnabled",
        v,
        expected_capture,
        &desc,
    )
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

fn gamedvr_state_from_reads(
    game_config_store: Result<Option<u32>, String>,
    app_capture: Result<Option<u32>, String>,
) -> Option<bool> {
    let game_config_store = game_config_store
        .ok()?
        .map(|value| value != 0)
        .unwrap_or(true);
    let app_capture = app_capture.ok()?.map(|value| value != 0).unwrap_or(true);
    Some(game_config_store && app_capture)
}

/// 读取各调优项当前状态（用于渲染开关）
pub(crate) fn read_states() -> TweakStates {
    let gamedvr = gamedvr_state_from_reads(
        crate::journal::read_reg_dword_detailed(GAME_CONFIG_STORE, "GameDVR_Enabled"),
        crate::journal::read_reg_dword_detailed(GAMEDVR_KEY, "AppCaptureEnabled"),
    );
    let hags = crate::journal::read_reg_dword(GRAPHICS_DRIVERS, "HwSchMode").map(|v| v == 2);
    let priority_boost =
        crate::journal::read_reg_dword(PRIORITY_CONTROL, "Win32PrioritySeparation")
            .map(|v| v == 0x26)
            .unwrap_or(false);
    TweakStates {
        gamedvr,
        hags,
        priority_boost,
        mode_preview: None,
        mode_preview_error: None,
    }
}

#[tauri::command]
pub async fn get_tweak_states(preview_mode: Option<String>) -> Result<TweakStates, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _operation_guard = lock_mode_operations();
        let mut states = read_states();
        if let Some(mode) = preview_mode {
            if mode != "game" {
                return Err("当前仅提供游戏模式的状态化预检".into());
            }
            match preview_game_mode_core() {
                Ok(preview) => states.mode_preview = Some(preview),
                Err(error) => states.mode_preview_error = Some(error),
            }
        }
        Ok(states)
    })
    .await
    .map_err(|error| error.to_string())?
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
    let mut auto_state = auto.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let _operation_guard = lock_mode_operations();
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
    // 引擎快照跟随真实改动，避免下一轮自动校准使用过期状态。
    if let Some(applied) = auto_state.adaptive.applied.as_mut() {
        match key.as_str() {
            "gamedvr" => applied.gamedvr = on,
            "priority_boost" => applied.prio_boost = on,
            _ => {}
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
    write_epp_with_snapshot(old, v, label, applied, None)
}

fn active_plan_matches(expected: &str, current: Option<&str>) -> bool {
    current.is_some_and(|current| current.eq_ignore_ascii_case(expected))
}

fn ensure_active_plan_matches(expected: &str) -> Result<(), String> {
    let current = crate::power::active_scheme_guid();
    if !active_plan_matches(expected, current.as_deref()) {
        return Err("活动电源计划在场景应用期间变化；拒绝对非预检计划写入 EPP".into());
    }
    Ok(())
}

fn try_epp_expected(
    expected: u32,
    v: u32,
    label: &str,
    applied: &mut Vec<String>,
    target_plan_guid: &str,
) -> Result<(), String> {
    ensure_active_plan_matches(target_plan_guid)?;
    if !crate::power::epp_supported() {
        return Err("目标计划 EPP 支持状态在预检后变化；场景事务已拒绝继续".into());
    }
    let old = crate::power::get_epp_value()
        .ok_or_else(|| "目标计划 EPP 在预检后变为不可读；场景事务已拒绝继续".to_string())?;
    if old != expected {
        return Err(format!(
            "目标计划 EPP 在预检后变化（当前 {old}，预期 {expected}）；未执行 EPP 写入"
        ));
    }
    write_epp_with_snapshot(old, v, label, applied, Some(target_plan_guid))
}

fn try_epp_on_plan(
    v: u32,
    label: &str,
    applied: &mut Vec<String>,
    target_plan_guid: &str,
) -> Result<(), String> {
    ensure_active_plan_matches(target_plan_guid)?;
    if !crate::power::epp_supported() {
        applied.push("当前平台不支持 EPP，已跳过该项".into());
        return Ok(());
    }
    let Some(old) = crate::power::get_epp_value() else {
        applied.push("无法读取当前 EPP；为避免无快照修改，已跳过该项".into());
        return Ok(());
    };
    write_epp_with_snapshot(old, v, label, applied, Some(target_plan_guid))
}

fn write_epp_with_snapshot(
    old: u32,
    v: u32,
    label: &str,
    applied: &mut Vec<String>,
    expected_plan_guid: Option<&str>,
) -> Result<(), String> {
    let description = format!("能效偏好 EPP → {v}（{label}）");
    let outcome = crate::journal::set_epp_with_journal(
        &description,
        old,
        v,
        || {
            if let Some(expected) = expected_plan_guid {
                ensure_active_plan_matches(expected)?;
            }
            crate::power::set_epp_raw(Some(v))
        },
        || {
            if let Some(expected) = expected_plan_guid {
                ensure_active_plan_matches(expected)?;
            }
            Ok(crate::power::get_epp_value())
        },
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

#[derive(Clone)]
enum PreviewGamePlan {
    Existing {
        guid: String,
        name: String,
        epp: EppSnapshot,
        active_plan_guid: String,
        game_config_store: Option<u32>,
        gamedvr_app_capture: Option<u32>,
    },
    MissingConfirmed {
        active_plan_guid: String,
        game_config_store: Option<u32>,
        gamedvr_app_capture: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PreviewEppAction {
    ExpectedValue(u32),
    Unsupported,
    Unset,
    Unknown,
    Invalid(u32),
    FreshSnapshot,
}

fn preview_epp_action(plan: Option<&PreviewGamePlan>) -> PreviewEppAction {
    match plan {
        None | Some(PreviewGamePlan::MissingConfirmed { .. }) => PreviewEppAction::FreshSnapshot,
        Some(PreviewGamePlan::Existing { epp, .. }) => match epp {
            EppSnapshot::Value(value) if crate::power::epp_value_valid(*value) => {
                PreviewEppAction::ExpectedValue(*value)
            }
            EppSnapshot::Value(value) => PreviewEppAction::Invalid(*value),
            EppSnapshot::Unsupported => PreviewEppAction::Unsupported,
            EppSnapshot::Unset => PreviewEppAction::Unset,
            EppSnapshot::Unknown | EppSnapshot::TargetPlanMissing => PreviewEppAction::Unknown,
        },
    }
}

fn set_gamedvr_for_preview(plan: Option<&PreviewGamePlan>) -> Result<(), String> {
    match plan {
        Some(PreviewGamePlan::Existing {
            game_config_store,
            gamedvr_app_capture,
            ..
        })
        | Some(PreviewGamePlan::MissingConfirmed {
            game_config_store,
            gamedvr_app_capture,
            ..
        }) => set_gamedvr_if_current(false, *game_config_store, *gamedvr_app_capture),
        None => set_gamedvr(false),
    }
}

fn apply_preview_epp_action(
    action: PreviewEppAction,
    v: u32,
    label: &str,
    target_plan_guid: &str,
    applied: &mut Vec<String>,
) -> Result<(), String> {
    match action {
        PreviewEppAction::ExpectedValue(expected) => {
            try_epp_expected(expected, v, label, applied, target_plan_guid)
        }
        PreviewEppAction::Unsupported => {
            ensure_active_plan_matches(target_plan_guid)?;
            if crate::power::epp_supported() {
                return Err("目标计划的 EPP 支持状态在预检后变化；未继续应用场景".into());
            }
            applied.push("预检确认平台不支持 EPP，已跳过该项".into());
            Ok(())
        }
        PreviewEppAction::Unset => {
            ensure_active_plan_matches(target_plan_guid)?;
            if !crate::power::epp_supported() || crate::power::get_epp_value().is_some() {
                return Err("目标计划的 EPP 状态在预检后变化；未执行 EPP 写入".into());
            }
            applied.push("预检确认目标计划未设置 AC EPP；为避免无恢复快照写入，已跳过".into());
            Ok(())
        }
        PreviewEppAction::Unknown => {
            ensure_active_plan_matches(target_plan_guid)?;
            applied.push("目标计划 EPP 预检状态未知，为避免基于未知状态写入，已跳过".into());
            Ok(())
        }
        PreviewEppAction::Invalid(value) => Err(format!(
            "目标计划 AC EPP 值 {value} 超出 0-100 范围；未应用游戏模式"
        )),
        PreviewEppAction::FreshSnapshot => try_epp_on_plan(v, label, applied, target_plan_guid),
    }
}

fn resolve_game_plan_after_confirmed_missing() -> Result<(String, String, bool), String> {
    let (high_plan_guid, saver_plan_guid) = crate::auto::plan_guids_for_preview()?;
    let plans_before = crate::power::list_power_plans()?;
    if crate::auto::preview_existing_plan(
        &crate::auto::PlanKind::High,
        high_plan_guid.as_deref(),
        saver_plan_guid.as_deref(),
        &plans_before,
    )
    .is_some()
    {
        return Err("预检后发现高性能计划状态已变化；请刷新预检，未应用场景更改".into());
    }

    crate::power::invalidate_plan_cache();
    let (guid, _) = match crate::auto::resolve_plan(&crate::auto::PlanKind::High) {
        Ok(resolved) => resolved,
        Err(error) => {
            let maybe_created = crate::power::list_power_plans()
                .map(|after| {
                    after.iter().any(|plan| {
                        !plans_before
                            .iter()
                            .any(|prior| prior.guid.eq_ignore_ascii_case(&plan.guid))
                    })
                })
                .unwrap_or(true);
            return if maybe_created {
                Err(format!(
                    "{error}；解析期间可能已创建高性能计划，该计划不会自动删除，请在电源设置中核对"
                ))
            } else {
                Err(error)
            };
        }
    };
    if plans_before
        .iter()
        .any(|plan| plan.guid.eq_ignore_ascii_case(&guid))
    {
        return Err("计划解析器选中了预检未列出的既有计划；请刷新预检，未应用场景更改".into());
    }
    let plans_after = crate::power::list_power_plans().map_err(|error| {
        format!("无法确认计划解析/创建结果：{error}；新计划可能已保留，请在电源设置中核对")
    })?;
    let verified = plans_after
        .into_iter()
        .find(|plan| plan.guid.eq_ignore_ascii_case(&guid))
        .ok_or_else(|| {
            "计划解析/创建后无法在系统计划列表中确认目标；新计划可能已保留，请在电源设置中核对"
                .to_string()
        })?;
    Ok((verified.guid, verified.name, true))
}

fn resolve_game_plan_automatic() -> Result<(String, String, bool), String> {
    let plans_before = crate::power::list_power_plans()?;
    let (guid, _) = match crate::auto::resolve_plan(&crate::auto::PlanKind::High) {
        Ok(resolved) => resolved,
        Err(error) => {
            let maybe_created = crate::power::list_power_plans()
                .map(|after| {
                    after.iter().any(|plan| {
                        !plans_before
                            .iter()
                            .any(|prior| prior.guid.eq_ignore_ascii_case(&plan.guid))
                    })
                })
                .unwrap_or(true);
            return if maybe_created {
                Err(format!(
                    "{error}；自动解析期间可能已创建高性能计划，该计划不会自动删除，请在电源设置中核对"
                ))
            } else {
                Err(error)
            };
        }
    };
    let plans_after = crate::power::list_power_plans().map_err(|error| {
        format!("无法确认自动解析后的电源计划：{error}；新计划可能已保留，请在电源设置中核对")
    })?;
    let may_remain = !plans_before
        .iter()
        .any(|plan| plan.guid.eq_ignore_ascii_case(&guid));
    let verified = plans_after
        .into_iter()
        .find(|plan| plan.guid.eq_ignore_ascii_case(&guid))
        .ok_or_else(|| {
            "自动解析后无法确认目标计划；新计划可能已保留，请在电源设置中核对".to_string()
        })?;
    Ok((verified.guid, verified.name, may_remain))
}

fn apply_mode_steps(
    mode: &str,
    applied: &mut Vec<String>,
    preview_game_plan: Option<&PreviewGamePlan>,
    plan_may_remain: &mut bool,
) -> Result<(), String> {
    match mode {
        "game" => {
            let epp_action = preview_epp_action(preview_game_plan);
            if let PreviewEppAction::Invalid(value) = epp_action {
                return Err(format!(
                    "目标计划 AC EPP 值 {value} 超出 0-100 范围；未应用游戏模式"
                ));
            }
            let cur = match preview_game_plan {
                Some(PreviewGamePlan::Existing {
                    active_plan_guid, ..
                })
                | Some(PreviewGamePlan::MissingConfirmed {
                    active_plan_guid, ..
                }) => active_plan_guid.clone(),
                None => crate::power::list_power_plans()?
                    .into_iter()
                    .find(|p| p.active)
                    .map(|p| p.guid)
                    .unwrap_or_default(),
            };
            let (guid, name, resolved_plan_may_remain) = match preview_game_plan {
                Some(PreviewGamePlan::Existing { guid, name, .. }) => {
                    (guid.clone(), name.clone(), false)
                }
                Some(PreviewGamePlan::MissingConfirmed { .. }) => {
                    let (guid, name, may_remain) = resolve_game_plan_after_confirmed_missing()?;
                    (guid, name, may_remain)
                }
                None => resolve_game_plan_automatic()?,
            };
            *plan_may_remain = resolved_plan_may_remain;
            set_plan_with_journal(cur, guid.clone(), &name)?;
            applied.push(format!("电源计划 → {}", name));
            set_gamedvr_for_preview(preview_game_plan)?;
            applied.push("GameDVR 已关闭".into());
            apply_preview_epp_action(epp_action, 0, "极致性能", &guid, applied)?;
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

const GAME_MODE_PREVIEW_TTL: Duration = Duration::from_secs(120);
const MAX_GAME_MODE_PREVIEW_TOKENS: usize = 32;

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ModePreviewStatus {
    WillChange,
    Unchanged,
    RequiresCreation,
    Unsupported,
    Unknown,
}

#[derive(Serialize, Clone)]
pub struct ModePreviewStep {
    pub id: String,
    pub label: String,
    pub current: String,
    pub target: String,
    pub status: ModePreviewStatus,
    pub note: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct GameModePreview {
    pub mode: String,
    pub observed_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub preview_id: String,
    pub requires_confirmation: bool,
    pub steps: Vec<ModePreviewStep>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EppSnapshot {
    TargetPlanMissing,
    Unsupported,
    Unset,
    Value(u32),
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct GameModePreviewSnapshot {
    active_plan_guid: String,
    target_plan_guid: Option<String>,
    game_config_store: Option<u32>,
    gamedvr_app_capture: Option<u32>,
    target_plan_epp: EppSnapshot,
}

struct GameModeInspection {
    snapshot: GameModePreviewSnapshot,
    current_plan: crate::power::Plan,
    target_plan: Option<crate::power::Plan>,
}

struct StoredGameModePreview {
    snapshot: GameModePreviewSnapshot,
    created_at: Instant,
}

#[derive(Default)]
struct GameModePreviewTokenStore {
    tokens: HashMap<String, StoredGameModePreview>,
}

impl GameModePreviewTokenStore {
    fn insert_at(&mut self, id: String, snapshot: GameModePreviewSnapshot, now: Instant) {
        self.tokens
            .retain(|_, token| now.duration_since(token.created_at) < GAME_MODE_PREVIEW_TTL);
        while self.tokens.len() >= MAX_GAME_MODE_PREVIEW_TOKENS {
            let oldest = self
                .tokens
                .iter()
                .min_by_key(|(_, token)| token.created_at)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.tokens.remove(&oldest);
            } else {
                break;
            }
        }
        self.tokens.insert(
            id,
            StoredGameModePreview {
                snapshot,
                created_at: now,
            },
        );
    }

    fn consume_at(&mut self, id: &str, now: Instant) -> Result<GameModePreviewSnapshot, String> {
        let token = self
            .tokens
            .remove(id)
            .ok_or_else(|| "场景预检已过期或已使用，请刷新预检后重试".to_string())?;
        if now.duration_since(token.created_at) >= GAME_MODE_PREVIEW_TTL {
            return Err("场景预检已过期，请刷新预检后重试".into());
        }
        Ok(token.snapshot)
    }
}

static GAME_MODE_PREVIEW_TOKENS: LazyLock<Mutex<GameModePreviewTokenStore>> =
    LazyLock::new(|| Mutex::new(GameModePreviewTokenStore::default()));

fn epp_snapshot(setting: crate::power::EppPlanSetting) -> EppSnapshot {
    match setting {
        crate::power::EppPlanSetting::Unsupported => EppSnapshot::Unsupported,
        crate::power::EppPlanSetting::Unset => EppSnapshot::Unset,
        crate::power::EppPlanSetting::Value(value) => EppSnapshot::Value(value),
        crate::power::EppPlanSetting::Unknown => EppSnapshot::Unknown,
    }
}

fn inspect_game_mode_state() -> Result<GameModeInspection, String> {
    let plans = crate::power::list_power_plans()?;
    let current_plan = plans
        .iter()
        .find(|plan| plan.active)
        .cloned()
        .ok_or_else(|| "无法确定当前活动电源计划；预检未签发，可刷新后重试".to_string())?;
    let active_guid = crate::power::active_scheme_guid()
        .ok_or_else(|| "无法读取当前活动电源计划 GUID；预检未签发".to_string())?
        .to_ascii_lowercase();
    if !current_plan.guid.eq_ignore_ascii_case(&active_guid) {
        return Err("电源计划列表与注册表当前状态不一致；预检未签发，请刷新后重试".into());
    }
    let (high_plan_guid, saver_plan_guid) = crate::auto::plan_guids_for_preview()?;
    let target_plan = crate::auto::preview_existing_plan(
        &crate::auto::PlanKind::High,
        high_plan_guid.as_deref(),
        saver_plan_guid.as_deref(),
        &plans,
    );
    let game_config_store =
        crate::journal::read_reg_dword_detailed(GAME_CONFIG_STORE, "GameDVR_Enabled")?;
    let gamedvr_app_capture =
        crate::journal::read_reg_dword_detailed(GAMEDVR_KEY, "AppCaptureEnabled")?;
    let target_plan_epp = target_plan
        .as_ref()
        .map(|plan| epp_snapshot(crate::power::inspect_epp_plan_setting(&plan.guid)))
        .unwrap_or(EppSnapshot::TargetPlanMissing);
    let snapshot = GameModePreviewSnapshot {
        active_plan_guid: active_guid,
        target_plan_guid: target_plan
            .as_ref()
            .map(|plan| plan.guid.to_ascii_lowercase()),
        game_config_store,
        gamedvr_app_capture,
        target_plan_epp,
    };
    Ok(GameModeInspection {
        snapshot,
        current_plan,
        target_plan,
    })
}

fn display_dword(value: Option<u32>) -> String {
    match value {
        Some(0) => "0（关闭）".into(),
        Some(1) => "1（开启）".into(),
        Some(other) => format!("{other}（非标准值）"),
        None => "未设置".into(),
    }
}

fn game_dvr_step(id: &str, label: &str, value: Option<u32>) -> ModePreviewStep {
    ModePreviewStep {
        id: id.into(),
        label: label.into(),
        current: display_dword(value),
        target: "0（关闭）".into(),
        status: if value == Some(0) {
            ModePreviewStatus::Unchanged
        } else {
            ModePreviewStatus::WillChange
        },
        note: Some("这是注册表配置值，不保证 Windows 或组策略的最终录制行为。".into()),
    }
}

fn game_mode_preview_steps(inspection: &GameModeInspection) -> Vec<ModePreviewStep> {
    let active_guid = &inspection.snapshot.active_plan_guid;
    let target_plan = inspection.target_plan.as_ref();
    let plan_step = match target_plan {
        Some(target) => ModePreviewStep {
            id: "power_plan".into(),
            label: "活动电源计划".into(),
            current: format!("{} ({})", inspection.current_plan.name, active_guid),
            target: format!("{} ({})", target.name, target.guid),
            status: if target.guid.eq_ignore_ascii_case(active_guid) {
                ModePreviewStatus::Unchanged
            } else {
                ModePreviewStatus::WillChange
            },
            note: None,
        },
        None => ModePreviewStep {
            id: "power_plan".into(),
            label: "活动电源计划".into(),
            current: format!("{} ({})", inspection.current_plan.name, active_guid),
            target: "未发现现成的高性能计划".into(),
            status: ModePreviewStatus::RequiresCreation,
            note: Some("应用时可能复制系统电源计划模板；预检本身不会创建或持久化计划。".into()),
        },
    };
    let epp_step = match inspection.snapshot.target_plan_epp {
        EppSnapshot::TargetPlanMissing => ModePreviewStep {
            id: "epp_ac".into(),
            label: "目标计划 AC EPP".into(),
            current: "目标计划尚不存在".into(),
            target: "0（性能优先）".into(),
            status: ModePreviewStatus::Unknown,
            note: Some("无法预检新计划的 EPP；应用时会在获取可验证恢复值后才尝试写入。".into()),
        },
        EppSnapshot::Unsupported => ModePreviewStep {
            id: "epp_ac".into(),
            label: "目标计划 AC EPP".into(),
            current: "平台不支持".into(),
            target: "0（性能优先）".into(),
            status: ModePreviewStatus::Unsupported,
            note: Some("EPP 在此平台会跳过；仅涉及交流电（AC）索引。".into()),
        },
        EppSnapshot::Unset => ModePreviewStep {
            id: "epp_ac".into(),
            label: "目标计划 AC EPP".into(),
            current: "未设置，无法建立恢复快照".into(),
            target: "0（性能优先）".into(),
            status: ModePreviewStatus::Unknown,
            note: Some("为避免无恢复值写入，应用时会跳过此项；仅检查 AC 索引。".into()),
        },
        EppSnapshot::Value(current) if !crate::power::epp_value_valid(current) => ModePreviewStep {
            id: "epp_ac".into(),
            label: "目标计划 AC EPP".into(),
            current: format!("{current}（超出 0-100 范围）"),
            target: "0（性能优先）".into(),
            status: ModePreviewStatus::Unknown,
            note: Some(
                "该值超出受支持范围；后端会拒绝应用游戏模式，避免改动计划、GameDVR 或 EPP。".into(),
            ),
        },
        EppSnapshot::Value(current) => ModePreviewStep {
            id: "epp_ac".into(),
            label: "目标计划 AC EPP".into(),
            current: current.to_string(),
            target: "0（性能优先）".into(),
            status: if current == 0 {
                ModePreviewStatus::Unchanged
            } else {
                ModePreviewStatus::WillChange
            },
            note: Some("只检查目标计划的交流电（AC）索引，不代表电池（DC）设置。".into()),
        },
        EppSnapshot::Unknown => ModePreviewStep {
            id: "epp_ac".into(),
            label: "目标计划 AC EPP".into(),
            current: "无法读取".into(),
            target: "0（性能优先）".into(),
            status: ModePreviewStatus::Unknown,
            note: Some("无法确认目标值；为避免基于未知状态写入，应用时会跳过 EPP。".into()),
        },
    };
    vec![
        plan_step,
        game_dvr_step(
            "game_config_store",
            r"GameConfigStore\GameDVR_Enabled",
            inspection.snapshot.game_config_store,
        ),
        game_dvr_step(
            "app_capture",
            r"GameDVR\AppCaptureEnabled",
            inspection.snapshot.gamedvr_app_capture,
        ),
        epp_step,
    ]
}

fn unix_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn issue_game_mode_preview(snapshot: GameModePreviewSnapshot) -> Result<(String, u64), String> {
    let now = Instant::now();
    let mut token_bytes = [0u8; 16];
    getrandom::fill(&mut token_bytes).map_err(|_| "无法生成安全的场景预检标识".to_string())?;
    token_bytes[6] = (token_bytes[6] & 0x0f) | 0x40;
    token_bytes[8] = (token_bytes[8] & 0x3f) | 0x80;
    let id = uuid::Uuid::from_bytes(token_bytes).to_string();
    let mut tokens = GAME_MODE_PREVIEW_TOKENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    tokens.insert_at(id.clone(), snapshot, now);
    let expires_at_unix_ms =
        unix_time_ms().saturating_add(GAME_MODE_PREVIEW_TTL.as_millis() as u64);
    Ok((id, expires_at_unix_ms))
}

fn preview_game_mode_core() -> Result<GameModePreview, String> {
    let inspection = inspect_game_mode_state()?;
    let requires_confirmation = inspection.target_plan.is_none();
    let steps = game_mode_preview_steps(&inspection);
    let (preview_id, expires_at_unix_ms) = issue_game_mode_preview(inspection.snapshot)?;
    Ok(GameModePreview {
        mode: "game".into(),
        observed_at_unix_ms: unix_time_ms(),
        expires_at_unix_ms,
        preview_id,
        requires_confirmation,
        steps,
    })
}

fn consume_game_mode_preview(id: &str) -> Result<GameModePreviewSnapshot, String> {
    GAME_MODE_PREVIEW_TOKENS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .consume_at(id, Instant::now())
}

fn apply_if_preview_matches<T>(
    expected: &GameModePreviewSnapshot,
    current: &GameModePreviewSnapshot,
    apply: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    if expected != current {
        return Err("预检后系统状态已变化；未应用任何场景更改，请刷新预检".into());
    }
    apply()
}

fn require_preview_confirmation(
    snapshot: &GameModePreviewSnapshot,
    missing_plan_confirmed: bool,
) -> Result<(), String> {
    if snapshot.target_plan_guid.is_none() && !missing_plan_confirmed {
        return Err("预检显示需要解析/创建高性能计划，必须显式确认；未应用更改".into());
    }
    Ok(())
}

fn apply_mode_with_preview_core(
    mode: &str,
    preview_id: &str,
    missing_plan_confirmed: bool,
) -> Result<Vec<String>, String> {
    let _operation_guard = lock_mode_operations();
    if mode != "game" {
        return Err("当前预检令牌仅适用于游戏模式".into());
    }
    let expected = consume_game_mode_preview(preview_id)?;
    require_preview_confirmation(&expected, missing_plan_confirmed)?;
    let current = inspect_game_mode_state()?;
    let current_snapshot = current.snapshot.clone();
    let target_epp = current_snapshot.target_plan_epp;
    let plan_resolution = match current.target_plan {
        Some(plan) => PreviewGamePlan::Existing {
            guid: plan.guid,
            name: plan.name,
            epp: target_epp,
            active_plan_guid: current_snapshot.active_plan_guid.clone(),
            game_config_store: current_snapshot.game_config_store,
            gamedvr_app_capture: current_snapshot.gamedvr_app_capture,
        },
        None => PreviewGamePlan::MissingConfirmed {
            active_plan_guid: current_snapshot.active_plan_guid.clone(),
            game_config_store: current_snapshot.game_config_store,
            gamedvr_app_capture: current_snapshot.gamedvr_app_capture,
        },
    };
    apply_if_preview_matches(&expected, &current_snapshot, || {
        apply_mode_core_with_preview_plan(mode, Some(plan_resolution), true)
    })
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
    let _operation_guard = lock_mode_operations();
    if mode == "game" {
        let inspection = inspect_game_mode_state()?;
        let Some(target) = inspection.target_plan else {
            return Err("未发现现成高性能计划；请到游戏模式页查看只读预检并确认后再应用".into());
        };
        let snapshot = inspection.snapshot;
        let plan = PreviewGamePlan::Existing {
            guid: target.guid,
            name: target.name,
            epp: snapshot.target_plan_epp,
            active_plan_guid: snapshot.active_plan_guid,
            game_config_store: snapshot.game_config_store,
            gamedvr_app_capture: snapshot.gamedvr_app_capture,
        };
        apply_mode_core_with_preview_plan(mode, Some(plan), true)
    } else {
        apply_mode_core_with_preview_plan(mode, None, true)
    }
}

pub(crate) fn apply_mode_core_automatic(mode: &str) -> Result<Vec<String>, String> {
    let _operation_guard = lock_mode_operations();
    if manual_hold_active() {
        return Err("手动操作保持期间跳过已排队的自动场景决策".into());
    }
    apply_mode_core_with_preview_plan(mode, None, false)
}

fn apply_mode_core_with_preview_plan(
    mode: &str,
    preview_game_plan: Option<PreviewGamePlan>,
    mark_manual_hold: bool,
) -> Result<Vec<String>, String> {
    validate_mode_access(mode, crate::util::is_admin())?;
    let mode_label = match mode {
        "game" => "游戏",
        "power" => "省电",
        "dev" => "编码",
        _ => mode,
    };
    let mut plan_may_remain = false;
    let result = (|| {
        let mut applied: Vec<String> = Vec::new();
        let mode_group_id = if mode == "none" {
            apply_mode_steps(
                mode,
                &mut applied,
                preview_game_plan.as_ref(),
                &mut plan_may_remain,
            )?;
            None
        } else {
            let group = crate::journal::begin_change_group(&format!("场景模式：{mode_label}"))?;
            let group_id = group.id();
            let mut group_guard = Some(group);
            run_mode_group(
                || {
                    apply_mode_steps(
                        mode,
                        &mut applied,
                        preview_game_plan.as_ref(),
                        &mut plan_may_remain,
                    )
                },
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
        if mark_manual_hold {
            *MANUAL_HOLD.lock().unwrap() = Some(Instant::now());
        }
        Ok(applied)
    })();
    match result {
        Err(error) if plan_may_remain => Err(format!(
            "{error}；预检后解析/创建的高性能计划不会随场景组自动删除，请在电源设置中核对是否仍需要"
        )),
        other => other,
    }
}

/// 场景模式编排。游戏模式必须携带最新的一次性预检令牌。
#[tauri::command]
pub async fn apply_mode(
    mode: String,
    preview_id: Option<String>,
    preview_confirmed: Option<bool>,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        match (mode.as_str(), preview_id, preview_confirmed) {
            ("game", Some(preview_id), confirmed) => {
                apply_mode_with_preview_core(&mode, &preview_id, confirmed.unwrap_or(false))
            }
            ("game", None, _) => Err("请先读取并确认游戏模式预检；未应用任何更改".into()),
            (_, Some(_), _) => Err("预检令牌仅适用于游戏模式".into()),
            (_, None, Some(true)) => Err("计划创建确认仅适用于游戏模式预检".into()),
            _ => apply_mode_core(&mode),
        }
    })
    .await
    .map_err(|error| error.to_string())?
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
    use super::*;

    fn preview_snapshot() -> GameModePreviewSnapshot {
        GameModePreviewSnapshot {
            active_plan_guid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            target_plan_guid: Some("11111111-2222-3333-4444-555555555555".into()),
            game_config_store: Some(1),
            gamedvr_app_capture: None,
            target_plan_epp: EppSnapshot::Value(85),
        }
    }

    #[test]
    fn active_plan_guard_is_case_insensitive_and_rejects_unknown() {
        let guid = "11111111-2222-3333-4444-555555555555";
        assert!(active_plan_matches(guid, Some(&guid.to_ascii_uppercase())));
        assert!(!active_plan_matches(guid, None));
        assert!(!active_plan_matches(
            guid,
            Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee")
        ));
    }

    #[test]
    fn gamedvr_state_does_not_turn_registry_read_errors_into_a_guess() {
        assert_eq!(gamedvr_state_from_reads(Ok(None), Ok(None)), Some(true));
        assert_eq!(gamedvr_state_from_reads(Ok(Some(0)), Ok(None)), Some(false));
        assert_eq!(
            gamedvr_state_from_reads(Err("denied".into()), Ok(Some(1))),
            None
        );
    }

    #[test]
    fn preview_epp_actions_bind_values_and_fail_closed_on_unknowns() {
        let make_plan = |epp| PreviewGamePlan::Existing {
            guid: "11111111-2222-3333-4444-555555555555".into(),
            name: "High Performance".into(),
            epp,
            active_plan_guid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            game_config_store: Some(1),
            gamedvr_app_capture: None,
        };
        assert_eq!(
            preview_epp_action(Some(&make_plan(EppSnapshot::Value(85)))),
            PreviewEppAction::ExpectedValue(85)
        );
        assert_eq!(
            preview_epp_action(Some(&make_plan(EppSnapshot::Value(101)))),
            PreviewEppAction::Invalid(101)
        );
        assert_eq!(
            preview_epp_action(Some(&make_plan(EppSnapshot::Unknown))),
            PreviewEppAction::Unknown
        );
        assert_eq!(
            preview_epp_action(Some(&PreviewGamePlan::MissingConfirmed {
                active_plan_guid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
                game_config_store: None,
                gamedvr_app_capture: None,
            })),
            PreviewEppAction::FreshSnapshot
        );
    }

    #[test]
    fn missing_plan_requires_backend_confirmation_but_existing_target_does_not() {
        let mut missing = preview_snapshot();
        missing.target_plan_guid = None;
        assert!(require_preview_confirmation(&missing, false)
            .unwrap_err()
            .contains("必须显式确认"));
        assert!(require_preview_confirmation(&missing, true).is_ok());
        assert!(require_preview_confirmation(&preview_snapshot(), false).is_ok());
    }

    #[test]
    fn game_mode_preview_tokens_are_single_use_and_expire() {
        let now = Instant::now();
        let expected = preview_snapshot();
        let mut store = GameModePreviewTokenStore::default();
        store.insert_at("one-use".into(), expected.clone(), now);
        assert_eq!(store.consume_at("one-use", now).unwrap(), expected);
        assert!(store.consume_at("one-use", now).is_err());

        store.insert_at("expired".into(), preview_snapshot(), now);
        assert!(store
            .consume_at("expired", now + GAME_MODE_PREVIEW_TTL)
            .unwrap_err()
            .contains("过期"));
    }

    #[test]
    fn stale_game_mode_preview_is_rejected_before_apply_callback() {
        let expected = preview_snapshot();
        let mut changes = Vec::new();
        let mut active_plan_changed = expected.clone();
        active_plan_changed.active_plan_guid = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".into();
        changes.push(active_plan_changed);
        let mut target_plan_changed = expected.clone();
        target_plan_changed.target_plan_guid = None;
        changes.push(target_plan_changed);
        let mut game_config_store_changed = expected.clone();
        game_config_store_changed.game_config_store = Some(0);
        changes.push(game_config_store_changed);
        let mut app_capture_changed = expected.clone();
        app_capture_changed.gamedvr_app_capture = Some(0);
        changes.push(app_capture_changed);
        let mut epp_changed = expected.clone();
        epp_changed.target_plan_epp = EppSnapshot::Value(25);
        changes.push(epp_changed);

        for changed in changes {
            let mut apply_called = false;
            let result = apply_if_preview_matches(&expected, &changed, || {
                apply_called = true;
                Ok(())
            });
            assert!(result.unwrap_err().contains("状态已变化"));
            assert!(!apply_called);
        }

        let mut apply_called = false;
        let applied = apply_if_preview_matches(&expected, &expected, || {
            apply_called = true;
            Ok("applied")
        })
        .unwrap();
        assert_eq!(applied, "applied");
        assert!(apply_called);
    }

    #[test]
    fn game_mode_preview_reports_diverged_registry_values_and_missing_plan() {
        let current = crate::power::Plan {
            guid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            name: "Balanced".into(),
            active: true,
        };
        let inspection = GameModeInspection {
            snapshot: GameModePreviewSnapshot {
                active_plan_guid: current.guid.clone(),
                target_plan_guid: None,
                game_config_store: Some(0),
                gamedvr_app_capture: Some(7),
                target_plan_epp: EppSnapshot::TargetPlanMissing,
            },
            current_plan: current,
            target_plan: None,
        };
        let steps = game_mode_preview_steps(&inspection);
        assert!(matches!(
            steps[0].status,
            ModePreviewStatus::RequiresCreation
        ));
        assert!(matches!(steps[1].status, ModePreviewStatus::Unchanged));
        assert!(matches!(steps[2].status, ModePreviewStatus::WillChange));
        assert!(matches!(steps[3].status, ModePreviewStatus::Unknown));
        assert!(steps[2].current.contains("非标准值"));
        assert!(steps[3].note.as_deref().unwrap().contains("无法预检"));
    }

    #[test]
    fn game_mode_preview_uses_target_plan_epp_and_marks_ac_only() {
        let current = crate::power::Plan {
            guid: "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".into(),
            name: "Balanced".into(),
            active: true,
        };
        let target = crate::power::Plan {
            guid: "11111111-2222-3333-4444-555555555555".into(),
            name: "High Performance".into(),
            active: false,
        };
        let inspection = GameModeInspection {
            snapshot: GameModePreviewSnapshot {
                active_plan_guid: current.guid.clone(),
                target_plan_guid: Some(target.guid.clone()),
                game_config_store: Some(1),
                gamedvr_app_capture: Some(0),
                target_plan_epp: EppSnapshot::Value(85),
            },
            current_plan: current,
            target_plan: Some(target),
        };
        let steps = game_mode_preview_steps(&inspection);
        assert!(matches!(steps[0].status, ModePreviewStatus::WillChange));
        assert!(matches!(steps[1].status, ModePreviewStatus::WillChange));
        assert!(matches!(steps[2].status, ModePreviewStatus::Unchanged));
        assert!(matches!(steps[3].status, ModePreviewStatus::WillChange));
        assert!(steps[3].note.as_deref().unwrap().contains("交流电"));

        let mut invalid = inspection;
        invalid.snapshot.target_plan_epp = EppSnapshot::Value(101);
        let mut invalid_steps = game_mode_preview_steps(&invalid);
        let invalid_epp = invalid_steps.remove(3);
        assert!(matches!(invalid_epp.status, ModePreviewStatus::Unknown));
        assert!(invalid_epp.current.contains("超出"));
        assert!(invalid_epp.note.as_deref().unwrap().contains("拒绝应用"));
    }

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
