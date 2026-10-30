use serde::{Deserialize, Serialize};

#[derive(Serialize, Clone)]
pub struct Plan {
    pub guid: String,
    pub name: String,
    pub active: bool,
}

/// 电源计划输出中的 GUID 匹配表达式。
const GUID_RE: &str =
    r"([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})";

pub(crate) fn is_valid_power_plan_guid(guid: &str) -> bool {
    let parts = guid.split('-').collect::<Vec<_>>();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(parts).all(|(size, part)| {
            part.len() == *size && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

pub(crate) fn epp_value_valid(value: u32) -> bool {
    value <= 100
}

fn parse_plans(out: &str) -> Vec<Plan> {
    let re = regex::Regex::new(GUID_RE).unwrap();
    let mut plans = Vec::new();
    for line in out.lines() {
        if let Some(m) = re.find(line) {
            let active = line.contains('*');
            // 活动计划行尾带 '*'，先剥掉再提取括号内名称
            let clean = line.trim_end().trim_end_matches('*').trim_end();
            let name = clean
                .rsplit_once('(')
                .map(|(_, r)| r.trim_end_matches(')').trim().to_string())
                .unwrap_or_default();
            plans.push(Plan {
                guid: m.as_str().to_lowercase(),
                name,
                active,
            });
        }
    }
    plans
}

#[tauri::command]
pub fn list_power_plans() -> Result<Vec<Plan>, String> {
    let out = crate::util::run("powercfg", &["/list"]).ok_or("无法读取电源计划")?;
    let mut plans = parse_plans(&out);
    for p in &mut plans {
        if p.name.is_empty() {
            p.name = p.guid.clone();
        }
    }
    Ok(plans)
}

/// `powercfg /list` 的短 TTL 缓存。
///
/// 电源计划集合极少变化；而 AutoState 持锁路径（active_plan_kind/plan_exists/resolve_plan）
/// 每次都要同步起一个 powercfg 子进程（最多 3 秒超时），会把共用同一把锁的 UI 命令
/// 一起卡住。缓存命中时返回 `None`，由调用方决定是否回退到真实子进程。
static PLAN_CACHE_LAST_FETCH_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static PLAN_CACHE: std::sync::Mutex<Option<Vec<Plan>>> = std::sync::Mutex::new(None);
const PLAN_CACHE_TTL_MS: u64 = 5_000;

fn cached_plan_list() -> Option<Vec<Plan>> {
    use std::sync::atomic::Ordering;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = PLAN_CACHE_LAST_FETCH_MS.load(Ordering::Relaxed);
    if last != 0 && now.saturating_sub(last) < PLAN_CACHE_TTL_MS {
        if let Ok(guard) = PLAN_CACHE.lock() {
            if let Some(plans) = guard.as_ref() {
                return Some(plans.clone());
            }
        }
        return None;
    }
    let out = crate::util::run("powercfg", &["/list"])?;
    let mut plans = parse_plans(&out);
    for p in &mut plans {
        if p.name.is_empty() {
            p.name = p.guid.clone();
        }
    }
    if let Ok(mut guard) = PLAN_CACHE.lock() {
        *guard = Some(plans.clone());
    }
    PLAN_CACHE_LAST_FETCH_MS.store(now, Ordering::Relaxed);
    Some(plans)
}

/// 带短 TTL 缓存的电源计划读取；失败时回退到真实调用。
///
/// 仅内部（auto / power 热路径）使用；前端 `list_power_plans` 命令保持直连，保证
/// 用户手动操作后立即读到最新状态。
pub(crate) fn cached_plans() -> Vec<Plan> {
    cached_plan_list().unwrap_or_else(list_power_plans_ok)
}

/// 主动失效计划缓存（刚创建/切换计划后调用，避免短 TTL 内读到旧集合）。
pub(crate) fn invalidate_plan_cache() {
    use std::sync::atomic::Ordering;
    PLAN_CACHE_LAST_FETCH_MS.store(0, Ordering::Relaxed);
}

fn list_power_plans_ok() -> Vec<Plan> {
    list_power_plans().unwrap_or_default()
}

fn plan_list() -> Vec<Plan> {
    list_power_plans().unwrap_or_default()
}

/// 找不到名称匹配的计划时用模板复制一份再找。
fn ensure_plan(matcher: impl Fn(&Plan) -> bool, template: &str) -> Result<Plan, String> {
    let plans = plan_list();
    if let Some(p) = plans.iter().find(|p| matcher(p)) {
        return Ok(p.clone());
    }
    crate::util::run_checked("powercfg", &["/duplicatescheme", template])?;
    let plans = plan_list();
    plans
        .into_iter()
        .find(|p| matcher(p))
        .ok_or_else(|| "创建电源计划失败".into())
}

#[tauri::command]
pub fn set_power_plan(guid: String) -> Result<(), String> {
    if !is_valid_power_plan_guid(&guid) {
        return Err("电源计划 GUID 不合法".into());
    }
    let old_guid = active_scheme_guid()
        .filter(|current| is_valid_power_plan_guid(current))
        .ok_or_else(|| "无法确认当前电源计划；拒绝执行未记录的切换".to_string())?;
    if !list_power_plans()?
        .iter()
        .any(|plan| plan.guid.eq_ignore_ascii_case(&old_guid))
    {
        return Err("当前电源计划不在系统计划列表中；拒绝执行不可安全撤销的切换".into());
    }
    crate::journal::change_power_plan_with_journal(
        old_guid.clone(),
        guid.clone(),
        &format!("手动切换电源计划 → {guid}"),
        || set_plan_raw_if_current(&guid, &old_guid),
    )
}

fn validate_existing_power_plan(guid: &str) -> Result<(), String> {
    if !is_valid_power_plan_guid(guid) {
        return Err("电源计划恢复 GUID 不合法；保留恢复记录".into());
    }
    if !list_power_plans()?
        .iter()
        .any(|plan| plan.guid.eq_ignore_ascii_case(guid))
    {
        return Err("目标电源计划已不存在；保留恢复记录".into());
    }
    Ok(())
}

fn verify_active_scheme_guid(
    expected: &str,
    current: Option<&str>,
    phase: &str,
) -> Result<(), String> {
    let current = current.ok_or_else(|| format!("无法{phase}当前活动电源计划；保留恢复记录"))?;
    if current.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(format!("{phase}活动电源计划与预期不符；保留恢复记录"))
    }
}

/// Validate the target, recheck before powercfg, then verify the applied active scheme.
pub fn set_plan_raw_if_current(guid: &str, expected_current: &str) -> Result<(), String> {
    validate_existing_power_plan(guid)?;
    if !is_valid_power_plan_guid(expected_current) {
        return Err("预期活动电源计划 GUID 不合法；拒绝覆盖当前状态".into());
    }
    verify_active_scheme_guid(expected_current, active_scheme_guid().as_deref(), "复核")?;
    crate::util::run_checked("powercfg", &["/setactive", guid])?;
    verify_active_scheme_guid(guid, active_scheme_guid().as_deref(), "确认切换后")?;
    invalidate_plan_cache();
    Ok(())
}

pub fn balanced_plan() -> Result<Plan, String> {
    ensure_plan(
        |p| {
            let n = p.name.to_lowercase();
            n.contains("平衡") || n.contains("balanced")
        },
        "381b4222-f694-41f0-9685-ff5bb260df2e",
    )
}

/// 生成电池报告 HTML 到桌面，返回路径。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct PowerDiagnostics {
    pub requests: Vec<String>,
    pub last_wake: Vec<String>,
    pub wake_timers: Vec<String>,
    pub collected_at_unix: u64,
}

/// Read-only Windows sleep blockers and wake-source snapshot.
#[tauri::command]
pub async fn power_diagnostics() -> Result<PowerDiagnostics, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let script = r#"
function Read-Lines([string]$Argument) {
  $items = @(& powercfg.exe $Argument 2>&1 | ForEach-Object { $_.ToString().Trim() } | Where-Object { $_ })
  return @($items | Select-Object -First 40)
}
$requests = @(Read-Lines '/requests'); $lastWake = @(Read-Lines '/lastwake'); $wakeTimers = @(Read-Lines '/waketimers')
[pscustomobject]@{
  requests = $requests
  last_wake = $lastWake
  wake_timers = $wakeTimers
  collected_at_unix = [DateTimeOffset]::Now.ToUnixTimeSeconds()
} | ConvertTo-Json -Compress -Depth 3
"#;
        let raw = crate::util::run_ps_with_timeout(script, std::time::Duration::from_secs(20))
            .ok_or("读取睡眠/唤醒诊断失败")?;
        serde_json::from_str(&raw).map_err(|error| format!("睡眠/唤醒结果解析失败: {error}"))
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
pub fn battery_report() -> Result<String, String> {
    let home = std::env::var("USERPROFILE").map_err(|_| "无法定位用户目录".to_string())?;
    let path = format!("{}\\Desktop\\AeroTune-电池报告.html", home);
    crate::util::run_checked_with_timeout(
        "powercfg",
        &["/batteryreport", "/output", &path],
        std::time::Duration::from_secs(60),
    )?;
    Ok(path)
}

// ---------- EPP（Intel Speed Shift）：注册表直读真值，不依赖 powercfg 本地化输出 ----------

const SUB_PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
const PERFEPP: &str = "36687f9e-e3a5-4dbf-b1dc-b15b817ce8c4";

/// 当前活动电源计划 GUID（注册表权威值）
pub fn active_scheme_guid() -> Option<String> {
    let k = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes")
        .ok()?;
    k.get_value::<String, _>("ActivePowerScheme").ok()
}

/// 平台是否提供 EPP 设置（定义层存在即支持；OEM 阉割机上为 false）
pub fn epp_supported() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(format!(
            r"SYSTEM\CurrentControlSet\Control\Power\PowerSettings\{}\{}",
            SUB_PROCESSOR, PERFEPP
        ))
        .is_ok()
}

/// 读取 EPP 当前值（AC 索引）。未设置/不支持返回 None。
pub fn get_epp_value() -> Option<u32> {
    let active = active_scheme_guid()?;
    let path = format!(
        r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes\{}\{}\{}",
        active, SUB_PROCESSOR, PERFEPP
    );
    let k = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(path)
        .ok()?;
    k.get_value::<u32, _>("ACSettingIndex").ok()
}

pub fn set_epp_raw(v: Option<u32>) -> Result<(), String> {
    let Some(v) = v else {
        return Ok(());
    };
    if !epp_value_valid(v) {
        return Err("EPP 取值范围 0-100".into());
    }
    if !epp_supported() {
        return Err("此平台未提供 EPP（perfepp）设置".into());
    }
    crate::util::run_checked(
        "powercfg",
        &[
            "/setacvalueindex",
            "scheme_current",
            "sub_processor",
            "perfepp",
            &v.to_string(),
        ],
    )?;
    crate::util::run_checked("powercfg", &["/setactive", "scheme_current"])
}

#[tauri::command]
pub fn get_epp() -> Option<u32> {
    get_epp_value()
}

/// 设置能效偏好并在写入前持久化可验证的恢复状态。
#[tauri::command]
pub fn set_epp(v: u32) -> Result<(), String> {
    if !epp_value_valid(v) {
        return Err("EPP 取值范围 0-100".into());
    }
    if !crate::util::is_admin() {
        return Err("EPP 调优需要管理员权限，请在「设置」页提权后重试".into());
    }
    if !epp_supported() {
        return Err("此平台未提供 EPP（perfepp）设置".into());
    }
    let old = get_epp_value()
        .ok_or_else(|| "无法读取当前 EPP；为避免无快照修改，拒绝设置".to_string())?;
    let outcome = crate::journal::set_epp_with_journal(
        &format!("能效偏好 EPP → {v}（0=极致性能 / 100=极致节能）"),
        old,
        v,
        || set_epp_raw(Some(v)),
        || Ok(get_epp_value()),
    )?;
    match outcome {
        crate::journal::EppWriteOutcome::Applied => {}
        crate::journal::EppWriteOutcome::AppliedWithWarning(warning) => {
            tracing::warn!(epp = v, %warning, "EPP 目标值已读回，但 powercfg 曾返回错误");
            return Err(format!("{warning}；恢复记录已保留，请刷新并核对当前 EPP"));
        }
        crate::journal::EppWriteOutcome::Unchanged(reason) if old == v => {
            tracing::info!(epp = v, %reason, "EPP 已处于请求值");
        }
        crate::journal::EppWriteOutcome::Unchanged(reason) => return Err(reason),
    }
    tracing::info!(epp = v, "EPP 已设置（手动）");
    Ok(())
}

// ---------- 休眠文件管理 ----------

#[derive(serde::Serialize, Clone, Debug, Default)]
pub struct HibernateInfo {
    pub supported: bool,
    pub enabled: bool,
    pub size_gb: f64,
}

#[tauri::command]
pub fn hibernate_info() -> HibernateInfo {
    let meta = std::fs::metadata(r"C:\hiberfil.sys");
    let enabled = meta.is_ok();
    let size_gb = meta
        .ok()
        .map(|m| m.len() as f64 / 1024.0 / 1024.0 / 1024.0)
        .unwrap_or(0.0);
    let supported = crate::util::run("powercfg", &["/a"])
        .map(|o| o.to_lowercase().contains("hibernat") || o.contains("休眠"))
        .unwrap_or(true);
    HibernateInfo {
        supported,
        enabled,
        size_gb,
    }
}

/// 开/关休眠（关闭立即释放 hiberfil.sys 空间）
#[tauri::command]
pub fn set_hibernate(on: bool) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("休眠管理需要管理员权限".into());
    }
    crate::util::run_checked("powercfg", &["/h", if on { "on" } else { "off" }])?;
    crate::journal::record(
        &format!(
            "{}系统休眠（{}）",
            if on { "开启" } else { "关闭" },
            if on {
                "重新生成 hiberfil.sys"
            } else {
                "释放 hiberfil.sys 空间"
            }
        ),
        crate::journal::Revert::None,
    )?;
    tracing::info!(on, "休眠已切换");
    Ok(())
}

// ---------- 电源计划冗余治理 ----------

#[derive(serde::Serialize, Clone)]
pub struct PlanDup {
    pub guid: String,
    pub name: String,
    pub active: bool,
}

/// 同名计划的非活动副本（AeroTune 早期版本/用户误操作可能堆积）
#[tauri::command]
pub fn list_redundant_plans() -> Vec<PlanDup> {
    let plans = match list_power_plans() {
        Ok(p) => p,
        Err(_) => return vec![],
    };
    let mut out: Vec<PlanDup> = Vec::new();
    for p in &plans {
        // 同名组里保留活动项，其余视为冗余
        let same_name_count = plans.iter().filter(|q| q.name == p.name).count();
        if same_name_count > 1 && !p.active {
            out.push(PlanDup {
                guid: p.guid.clone(),
                name: p.name.clone(),
                active: false,
            });
        }
    }
    out
}

/// 删除电源计划：先导出 .pow 备份（可重新导入还原），再删除
#[tauri::command]
pub fn remove_plan(guid: String) -> Result<String, String> {
    if !crate::util::is_admin() {
        return Err("删除电源计划需要管理员权限".into());
    }
    let re = regex::Regex::new(
        r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$",
    )
    .unwrap();
    if !re.is_match(&guid) {
        return Err("GUID 不合法".into());
    }
    let plans = list_power_plans()?;
    let plan = plans
        .iter()
        .find(|p| p.guid == guid.to_lowercase())
        .ok_or("电源计划不存在")?;
    if plan.active {
        return Err("不能删除正在使用的电源计划".into());
    }
    let dir = crate::util::app_dir().join("plans_backup");
    let _ = std::fs::create_dir_all(&dir);
    let backup = dir.join(format!("{}.pow", plan.guid));
    crate::util::run_checked(
        "powercfg",
        &["/export", backup.to_string_lossy().as_ref(), &plan.guid],
    )?;
    crate::util::run_checked("powercfg", &["/delete", &plan.guid])?;
    crate::journal::record(
        &format!(
            "删除电源计划「{}」（备份：{}）",
            plan.name,
            backup.display()
        ),
        crate::journal::Revert::None,
    )?;
    tracing::info!(guid = %plan.guid, name = %plan.name, "冗余电源计划已删除（已备份）");
    Ok(backup.to_string_lossy().into_owned())
}

// ---------- 亮度（WMI 显示器brightness，笔记本屏生效） ----------

fn parse_brightness_level(raw: &str) -> Option<u8> {
    let level = raw.trim().parse::<u8>().ok()?;
    (level <= 100).then_some(level)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrightnessCompareWriteResult {
    Applied,
    CurrentChanged(u8),
    Failed,
}

fn build_brightness_compare_write_script(expected: u8, target: u8) -> Option<String> {
    if expected > 100 || target > 100 {
        return None;
    }
    Some(format!(
        "$ErrorActionPreference='Stop'; try {{ $c=@(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightness); if ($c.Count -ne 1) {{ 'AEROTUNE_UNSUPPORTED'; exit 0 }}; $current=[int]$c[0].CurrentBrightness; if ($current -lt 0 -or $current -gt 100) {{ 'AEROTUNE_FAILED:invalid-current'; exit 0 }}; if ($current -ne {expected}) {{ 'AEROTUNE_CHANGED:' + $current; exit 0 }}; $m=@(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods | Where-Object Active); if ($m.Count -eq 0) {{ 'AEROTUNE_UNSUPPORTED'; exit 0 }}; foreach ($x in $m) {{ $r=$x.WmiSetBrightness(1,{target}); if ($r.ReturnValue -ne 0) {{ 'AEROTUNE_FAILED:' + $r.ReturnValue; exit 0 }} }}; 'AEROTUNE_OK' }} catch {{ 'AEROTUNE_ERROR:' + $_.Exception.HResult; exit 0 }}"
    ))
}

fn parse_brightness_compare_write_result(raw: &str) -> BrightnessCompareWriteResult {
    let status = raw.trim();
    if status == "AEROTUNE_OK" {
        BrightnessCompareWriteResult::Applied
    } else if let Some(level) = status.strip_prefix("AEROTUNE_CHANGED:") {
        parse_brightness_level(level)
            .map(BrightnessCompareWriteResult::CurrentChanged)
            .unwrap_or(BrightnessCompareWriteResult::Failed)
    } else {
        BrightnessCompareWriteResult::Failed
    }
}

pub fn set_brightness_if_current(expected: u8, target: u8) -> BrightnessCompareWriteResult {
    let Some(script) = build_brightness_compare_write_script(expected, target) else {
        return BrightnessCompareWriteResult::Failed;
    };
    let Some(out) = crate::util::run_ps_with_timeout(&script, std::time::Duration::from_secs(3))
    else {
        return BrightnessCompareWriteResult::Failed;
    };
    let result = parse_brightness_compare_write_result(&out);
    if result == BrightnessCompareWriteResult::Failed {
        tracing::debug!(status = %out.trim(), "屏幕亮度条件恢复未成功");
    }
    result
}

pub fn get_brightness() -> Option<u8> {
    let out = crate::util::run_ps_with_timeout(
        "(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightness).CurrentBrightness",
        std::time::Duration::from_secs(3),
    )?;
    parse_brightness_level(&out)
}

fn brightness_write_ok(output: Option<&str>) -> bool {
    matches!(output.map(str::trim), Some("AEROTUNE_OK"))
}

pub fn set_brightness(v: u8) -> bool {
    let script = format!(
        "$ErrorActionPreference='Stop'; try {{ $m=@(Get-WmiObject -Namespace root/WMI -Class WmiMonitorBrightnessMethods | Where-Object Active); if ($m.Count -eq 0) {{ 'AEROTUNE_UNSUPPORTED'; exit 0 }}; foreach ($x in $m) {{ $r=$x.WmiSetBrightness(1,{v}); if ($r.ReturnValue -ne 0) {{ 'AEROTUNE_FAILED:' + $r.ReturnValue; exit 0 }} }}; 'AEROTUNE_OK' }} catch {{ 'AEROTUNE_ERROR:' + $_.Exception.HResult; exit 0 }}"
    );
    let Some(out) = crate::util::run_ps_with_timeout(&script, std::time::Duration::from_secs(3))
    else {
        return false;
    };
    if !brightness_write_ok(Some(&out)) {
        tracing::debug!(status = %out.trim(), "屏幕亮度接口不支持或写入未成功");
    }
    brightness_write_ok(Some(&out))
}

// ---------- 处理器最大频率（PROCTHROTTLEMAX） ----------

const PROCTHROTTLEMAX: &str = "bc5038f7-23e0-4960-96da-33abaf5935ec";

/// 读取当前计划的最大处理器状态（AC 索引，未设置时视为 100）
pub fn get_proc_max() -> Option<u32> {
    let active = active_scheme_guid()?;
    let path = format!(
        r"SYSTEM\CurrentControlSet\Control\Power\User\PowerSchemes\{}\{}\{}",
        active, SUB_PROCESSOR, PROCTHROTTLEMAX
    );
    let k = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey(path)
        .ok()?;
    k.get_value::<u32, _>("ACSettingIndex").ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_input_accepts_only_the_wmi_percent_range() {
        assert_eq!(parse_brightness_level("0\r\n"), Some(0));
        assert_eq!(parse_brightness_level("100"), Some(100));
        for raw in ["101", "255", "-1", "unknown", ""] {
            assert_eq!(parse_brightness_level(raw), None, "accepted {raw:?}");
        }
    }

    #[test]
    fn guarded_brightness_write_checks_current_before_mutating() {
        let script = build_brightness_compare_write_script(40, 70).unwrap();
        let read = script.find("WmiMonitorBrightness)").unwrap();
        let compare = script.find("$current -ne 40").unwrap();
        let write = script.find("WmiSetBrightness(1,70)").unwrap();
        assert!(read < compare && compare < write);
        assert!(build_brightness_compare_write_script(101, 70).is_none());
        assert!(build_brightness_compare_write_script(40, 255).is_none());
        assert_eq!(
            parse_brightness_compare_write_result("AEROTUNE_OK"),
            BrightnessCompareWriteResult::Applied
        );
        assert_eq!(
            parse_brightness_compare_write_result("AEROTUNE_CHANGED:55"),
            BrightnessCompareWriteResult::CurrentChanged(55)
        );
        assert_eq!(
            parse_brightness_compare_write_result("AEROTUNE_CHANGED:255"),
            BrightnessCompareWriteResult::Failed
        );
    }

    #[test]
    fn brightness_write_status_distinguishes_success_and_unavailable() {
        assert!(brightness_write_ok(Some("AEROTUNE_OK")));
        assert!(!brightness_write_ok(Some("AEROTUNE_UNSUPPORTED")));
        assert!(!brightness_write_ok(Some("AEROTUNE_FAILED:1")));
        assert!(!brightness_write_ok(Some("AEROTUNE_ERROR:-2147024891")));
        assert!(!brightness_write_ok(None));
    }

    #[test]
    fn processor_max_guid_is_complete() {
        assert_eq!(PROCTHROTTLEMAX, "bc5038f7-23e0-4960-96da-33abaf5935ec");
        assert!(regex::Regex::new(&format!("^{GUID_RE}$"))
            .unwrap()
            .is_match(PROCTHROTTLEMAX));
    }

    #[test]
    fn persisted_power_plan_and_epp_values_are_bounded() {
        assert!(is_valid_power_plan_guid(
            "381b4222-f694-41f0-9685-ff5bb260df2e"
        ));
        assert!(is_valid_power_plan_guid(
            "DB99F211-6CCC-451F-848C-066D5C85927C"
        ));
        assert!(!is_valid_power_plan_guid("scheme_min"));
        assert!(!is_valid_power_plan_guid(
            "381b4222-f694-41f0-9685-ff5bb260df2e /h"
        ));
        assert!(epp_value_valid(0));
        assert!(epp_value_valid(100));
        assert!(!epp_value_valid(101));
        assert!(!epp_value_valid(u32::MAX));
    }

    #[test]
    fn active_scheme_verification_is_case_insensitive_and_fail_closed() {
        let guid = "381b4222-f694-41f0-9685-ff5bb260df2e";
        assert!(verify_active_scheme_guid(guid, Some(&guid.to_uppercase()), "复核").is_ok());
        assert!(verify_active_scheme_guid(guid, None, "确认切换后").is_err());
        assert!(verify_active_scheme_guid(
            guid,
            Some("a1841308-3541-4fab-bc81-f71556f20b4a"),
            "确认切换后"
        )
        .is_err());
    }

    #[test]
    fn parse_plans_handles_active_marker_and_chinese() {
        let raw = "现有电源使用方案 (* Active)\r\n\
                   电源方案 GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (平衡) *\r\n\
                   电源方案 GUID: db99f211-6ccc-451f-848c-066d5c85927c  (高性能)\r\n";
        let plans = parse_plans(raw);
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0].name, "平衡");
        assert!(plans[0].active);
        assert_eq!(plans[1].name, "高性能");
        assert!(!plans[1].active);
    }
}
