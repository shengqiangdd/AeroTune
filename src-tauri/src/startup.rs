use serde::{Deserialize, Serialize};
use std::sync::Mutex;

static DELAY_LOCK: Mutex<()> = Mutex::new(());
use std::path::{Path, PathBuf};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use winreg::HKEY;

#[derive(Serialize, Clone)]
pub struct StartupItem {
    pub id: String,
    pub name: String,
    pub cmd: String,
    pub source: String,
    pub enabled: bool,
    pub signature_status: String,
    pub signer_subject: Option<String>,
}

const RUN_USER: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_MACHINE: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const BACKUP_KEY: &str = r"Software\AeroTune\StartupBackup";

fn run_values(hive: HKEY, sub: &str, source: &str, enabled: bool) -> Vec<StartupItem> {
    let mut out = Vec::new();
    let Ok(key) = winreg::RegKey::predef(hive).open_subkey_with_flags(sub, winreg::enums::KEY_READ)
    else {
        return out;
    };
    for name in key.enum_values().flatten().map(|(n, _)| n) {
        let cmd = key.get_value::<String, _>(&name).unwrap_or_default();
        out.push(StartupItem {
            id: format!(
                "reg|{}|{}",
                if source.contains("HKLM") {
                    "HKLM"
                } else {
                    "HKCU"
                },
                name
            ),
            name: name.clone(),
            cmd,
            source: source.to_string(),
            enabled,
            signature_status: "unknown".into(),
            signer_subject: None,
        });
    }
    out
}

fn folder_items(dir: PathBuf, source: &str, enabled: bool) -> Vec<StartupItem> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if ext != "lnk" && ext != "exe" && ext != "bat" {
            continue;
        }
        let name = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        out.push(StartupItem {
            id: format!("file|{}", p.to_string_lossy()),
            name,
            cmd: p.to_string_lossy().into_owned(),
            source: source.to_string(),
            enabled,
            signature_status: "unknown".into(),
            signer_subject: None,
        });
    }
    out
}

fn disabled_dir() -> PathBuf {
    let d = crate::util::app_dir().join("startup_disabled");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn encode_disabled_name(original: &str) -> String {
    format!("{}_{}", crate::util::safe_id(original), original)
}

// Existing disabled files use <safe_id(original)>_<original>. The safe ID can
// itself contain underscores, so look for the separator whose suffix actually
// produces the prefix. Plain files already in the disabled directory keep their name.
fn decode_disabled_name(stored: &str) -> &str {
    stored
        .rmatch_indices('_')
        .find_map(|(at, _)| {
            let (prefix, suffix) = stored.split_at(at);
            let original = &suffix[1..];
            (crate::util::safe_id(original) == prefix).then_some(original)
        })
        .unwrap_or(stored)
}

fn valid_startup_basename(name: &str) -> bool {
    if name.is_empty()
        || name.len() > 255
        || name.ends_with(' ')
        || name.ends_with('.')
        || name
            .chars()
            .any(|character| character.is_control() || "\\/:*?\"<>|".contains(character))
    {
        return false;
    }
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "lnk" | "exe" | "bat"
            )
        })
        .unwrap_or(false)
}

/// Accept only a regular, supported startup file that is a direct child of `root`.
/// Canonical parent equality avoids prefix/`..`/junction escapes; all failures are closed.
fn validated_startup_file(path: &Path, root: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("启动文件路径必须是绝对路径".into());
    }
    let canonical_root =
        std::fs::canonicalize(root).map_err(|_| "无法验证受信任的启动目录".to_string())?;
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "启动文件不存在或不可访问".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("只允许移动普通启动文件".into());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "启动文件名无法安全解析".to_string())?;
    if !valid_startup_basename(name) {
        return Err("只允许处理 .lnk、.exe 或 .bat 启动文件".into());
    }
    let canonical_file =
        std::fs::canonicalize(path).map_err(|_| "无法验证启动文件路径".to_string())?;
    if canonical_file.parent() != Some(canonical_root.as_path()) {
        return Err("启动文件必须是受信任目录中的直接子项".into());
    }
    let canonical_metadata = std::fs::symlink_metadata(&canonical_file)
        .map_err(|_| "启动文件在验证期间已变化".to_string())?;
    if !canonical_metadata.is_file() || canonical_metadata.file_type().is_symlink() {
        return Err("只允许移动普通启动文件".into());
    }
    Ok(canonical_file)
}

fn path_entry_exists(path: &Path) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// 启动项总数（含已禁用），供健康评分使用
pub fn count_startup_items() -> usize {
    fn count_key(hive: HKEY, sub: &str) -> usize {
        winreg::RegKey::predef(hive)
            .open_subkey_with_flags(sub, winreg::enums::KEY_READ)
            .map(|k| k.enum_values().flatten().count())
            .unwrap_or(0)
    }
    let folder = |d: Option<PathBuf>| -> usize {
        d.map(|d| {
            std::fs::read_dir(d)
                .map(|rd| rd.flatten().count())
                .unwrap_or(0)
        })
        .unwrap_or(0)
    };
    count_key(HKEY_CURRENT_USER, RUN_USER)
        + count_key(HKEY_LOCAL_MACHINE, RUN_MACHINE)
        + count_key(HKEY_CURRENT_USER, BACKUP_KEY)
        + folder(user_startup_dir())
        + folder(common_startup_dir())
}

// ---------- 开机耗时分析（Windows 诊断-性能事件 100） ----------

#[derive(serde::Serialize, Clone, Debug)]
pub struct BootRecord {
    pub ts: i64,
    /// 主路径开机时间（秒）：到登录界面
    pub main_secs: f64,
    /// 后开机时间（秒）：登录到系统就绪
    pub post_secs: f64,
    /// 总开机时间（秒）
    pub total_secs: f64,
    /// 事件级别：1 严重 / 2 异常 / 3 偏慢 / 4 正常
    pub level: u8,
}

#[tauri::command]
pub async fn boot_history() -> Result<Vec<BootRecord>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // 用事件 XML 的 Data Name 取值，不依赖本地化文案
        let script = r#"
$b = Get-WinEvent -FilterHashtable @{LogName='Microsoft-Windows-Diagnostics-Performance/Operational'; Id=100} -MaxEvents 5 -ErrorAction SilentlyContinue
$out = foreach ($e in $b) {
  $x = [xml]$e.ToXml()
  $d = @{}
  foreach ($p in $x.Event.EventData.Data) { $d[$p.Name] = $p.'#text' }
  [pscustomobject]@{
    ts    = [DateTimeOffset]::Parse($e.TimeCreated).ToUnixTimeSeconds()
    main  = [math]::Round([double]$d['MainPathBootTime'] / 1000, 1)
    post  = [math]::Round([double]$d['PostBootTime'] / 1000, 1)
    total = [math]::Round([double]$d['BootTime'] / 1000, 1)
    lvl   = $e.Level
  }
}
if ($out) { $out | ConvertTo-Json -Compress } else { '[]' }
"#;
        let raw = crate::util::run_ps_with_timeout(script, std::time::Duration::from_secs(60))
            .ok_or("无法读取开机事件日志（可能需要管理员或日志被清理）")?;
        let v: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("开机事件解析失败: {e}"))?;
        let arr: Vec<serde_json::Value> = match v {
            serde_json::Value::Array(a) => a,
            obj @ serde_json::Value::Object(_) => vec![obj],
            _ => vec![],
        };
        let records: Vec<BootRecord> = arr
            .into_iter()
            .map(|r| BootRecord {
                ts: r["ts"].as_i64().unwrap_or(0),
                main_secs: r["main"].as_f64().unwrap_or(0.0),
                post_secs: r["post"].as_f64().unwrap_or(0.0),
                total_secs: r["total"].as_f64().unwrap_or(0.0),
                level: r["lvl"].as_u64().unwrap_or(4) as u8,
            })
            .collect();
        tracing::info!(count = records.len(), "开机耗时分析完成");
        Ok(records)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize, Clone, Debug)]
pub struct StartupImpact {
    pub ts: u64,
    pub name: String,
    pub path: String,
    pub total_ms: u64,
    pub degradation_ms: u64,
}

fn parse_startup_impacts(raw: &str) -> Result<Vec<StartupImpact>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("启动影响事件解析失败: {error}"))?;
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        serde_json::Value::Object(row) => vec![serde_json::Value::Object(row)],
        _ => Vec::new(),
    };
    let mut impacts = rows
        .into_iter()
        .filter_map(|row| {
            let name = row["name"].as_str().unwrap_or_default().trim().to_string();
            let total = row["total_ms"].as_f64()?;
            let degradation = row["degradation_ms"].as_f64().unwrap_or(0.0);
            if name.is_empty()
                || !total.is_finite()
                || !degradation.is_finite()
                || !(0.0..=600_000.0).contains(&total)
                || !(0.0..=600_000.0).contains(&degradation)
            {
                return None;
            }
            Some(StartupImpact {
                ts: row["ts"].as_u64().unwrap_or(0),
                name,
                path: row["path"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(1024)
                    .collect(),
                total_ms: total.round() as u64,
                degradation_ms: degradation.round() as u64,
            })
        })
        .collect::<Vec<_>>();
    impacts.sort_by(|a, b| {
        b.degradation_ms
            .cmp(&a.degradation_ms)
            .then_with(|| b.total_ms.cmp(&a.total_ms))
    });
    impacts.truncate(30);
    Ok(impacts)
}

/// 启动影响短 TTL 缓存：数据源自历史启动事件日志，短时间内不变；
/// 缓存避免每次进页都同步跑一次 Get-WinEvent（30 秒超时）。
static IMPACTS_CACHE: std::sync::LazyLock<
    std::sync::Mutex<Option<(std::time::Instant, Vec<StartupImpact>)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
const IMPACTS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(600);

#[tauri::command]
pub async fn startup_impacts() -> Result<Vec<StartupImpact>, String> {
    if let Ok(guard) = IMPACTS_CACHE.lock() {
        if let Some((at, cached)) = guard.as_ref() {
            if at.elapsed() < IMPACTS_CACHE_TTL {
                return Ok(cached.clone());
            }
        }
    }
    let impacts = tauri::async_runtime::spawn_blocking(|| {
        let script = r#"
$events = Get-WinEvent -FilterHashtable @{LogName='Microsoft-Windows-Diagnostics-Performance/Operational'; Id=101} -MaxEvents 30 -ErrorAction SilentlyContinue
$out = foreach ($event in $events) {
  $xml = [xml]$event.ToXml(); $data = @{}
  foreach ($item in $xml.Event.EventData.Data) { $data[$item.Name] = $item.'#text' }
  $name = [string]$data['FriendlyName']; if (-not $name) { $name = [string]$data['Name'] }
  $total = 0.0; $delay = 0.0
  [double]::TryParse([string]$data['TotalTime'], [ref]$total) | Out-Null
  [double]::TryParse([string]$data['DegradationTime'], [ref]$delay) | Out-Null
  if ($name -and $total -gt 0) { $ts = ([DateTimeOffset]$event.TimeCreated).ToUnixTimeSeconds(); [pscustomobject]@{ ts=$ts; name=$name; path=[string]$data['Path']; total_ms=$total; degradation_ms=$delay } }
}
if ($out) { $out | ConvertTo-Json -Compress -Depth 3 } else { '[]' }
"#;
        let raw = crate::util::run_ps_with_timeout(script, std::time::Duration::from_secs(30))
            .ok_or("读取启动应用诊断事件失败（事件日志可能未启用或已清理）")?;
        parse_startup_impacts(&raw)
    })
    .await
    .map_err(|error| error.to_string())??;
    if let Ok(mut guard) = IMPACTS_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), impacts.clone()));
    }
    Ok(impacts)
}

fn user_startup_dir() -> Option<PathBuf> {
    std::env::var("APPDATA")
        .ok()
        .map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\Startup"))
}

fn common_startup_dir() -> Option<PathBuf> {
    std::env::var("ProgramData")
        .ok()
        .map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Start Menu\Programs\StartUp"))
}

#[tauri::command]
pub async fn list_startup() -> Result<Vec<StartupItem>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut items = collect_startup_items();
        attach_startup_signature_context(&mut items);
        items
    })
    .await
    .map_err(|error| format!("读取启动项信任上下文失败: {error}"))
}

/// Lightweight startup inventory used by process-role classification; it does not verify files.
pub fn collect_startup_items() -> Vec<StartupItem> {
    let mut items = Vec::new();
    items.extend(run_values(
        HKEY_CURRENT_USER,
        RUN_USER,
        "注册表 · HKCU",
        true,
    ));
    items.extend(run_values(
        HKEY_LOCAL_MACHINE,
        RUN_MACHINE,
        "注册表 · HKLM（管理员）",
        true,
    ));
    if let Some(d) = user_startup_dir() {
        items.extend(folder_items(d, "启动文件夹 · 用户", true));
    }
    if let Some(d) = common_startup_dir() {
        items.extend(folder_items(d, "启动文件夹 · 所有用户（管理员）", true));
    }
    // 已禁用的项（HKCU 备份键 + 本地禁用文件夹）
    items.extend(run_values(
        HKEY_CURRENT_USER,
        BACKUP_KEY,
        "AeroTune 已禁用",
        false,
    ));
    items.extend(folder_items(disabled_dir(), "AeroTune 已禁用", false));
    items.sort_by(|a, b| b.enabled.cmp(&a.enabled).then(a.name.cmp(&b.name)));
    items
}

#[tauri::command]
pub fn toggle_startup(id: String, enable: bool) -> Result<(), String> {
    let mut parts = id.splitn(2, '|');
    let kind = parts.next().unwrap_or("");
    let rest = parts.next().unwrap_or("");
    match (kind, enable) {
        ("reg", false) => {
            // HKCU Run → 备份键 → 删除
            let (hive, name) = rest.split_once('|').ok_or("无效 ID")?;
            if hive != "HKCU" {
                return Err("HKLM 启动项需要管理员权限，请在「设置」提权后重试".into());
            }
            let user = winreg::RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey_with_flags(
                    RUN_USER,
                    winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
                )
                .map_err(|e| e.to_string())?;
            let cmd: String = user
                .get_value(name)
                .map_err(|e| format!("读取启动项失败: {e}"))?;
            let (backup, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
                .create_subkey(BACKUP_KEY)
                .map_err(|e| e.to_string())?;
            backup.set_value(name, &cmd).map_err(|e| e.to_string())?;
            user.delete_value(name).map_err(|e| e.to_string())?;
            Ok(())
        }
        ("reg", true) => {
            let (hive, name) = rest.split_once('|').ok_or("无效 ID")?;
            if hive != "HKCU" {
                return Err("HKLM 启动项需要管理员权限".into());
            }
            let backup = winreg::RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey_with_flags(
                    BACKUP_KEY,
                    winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
                )
                .map_err(|e| e.to_string())?;
            let cmd: String = backup
                .get_value(name)
                .map_err(|e| format!("备份不存在: {e}"))?;
            let user = winreg::RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey_with_flags(
                    RUN_USER,
                    winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
                )
                .map_err(|e| e.to_string())?;
            user.set_value(name, &cmd).map_err(|e| e.to_string())?;
            let _ = backup.delete_value(name);
            Ok(())
        }
        ("file", false) => {
            let supplied = PathBuf::from(rest);
            let mut source = None;
            if let Some(user_root) = user_startup_dir() {
                if let Ok(validated) = validated_startup_file(&supplied, &user_root) {
                    source = Some(validated);
                }
            }
            if source.is_none() {
                if let Some(common_root) = common_startup_dir() {
                    if let Ok(validated) = validated_startup_file(&supplied, &common_root) {
                        if !crate::util::is_admin() {
                            return Err("所有用户启动项需要管理员权限".into());
                        }
                        source = Some(validated);
                    }
                }
            }
            let source = source.ok_or("只能禁用用户或所有用户启动目录中的普通启动文件")?;
            let original = source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("无效文件名")?;
            let disabled_root = disabled_dir();
            std::fs::create_dir_all(&disabled_root).map_err(|e| e.to_string())?;
            let disabled_root = std::fs::canonicalize(disabled_root).map_err(|e| e.to_string())?;
            let target = disabled_root.join(encode_disabled_name(original));
            if path_entry_exists(&target)? {
                return Err("禁用目录已有同名文件，未移动原文件".into());
            }
            std::fs::rename(&source, &target).map_err(|e| format!("移动失败: {e}"))
        }
        ("file", true) => {
            let supplied = PathBuf::from(rest);
            let disabled_root = disabled_dir();
            std::fs::create_dir_all(&disabled_root).map_err(|e| e.to_string())?;
            let source = validated_startup_file(&supplied, &disabled_root)?;
            let fname = source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("无效禁用文件名")?;
            let original = decode_disabled_name(fname);
            if !valid_startup_basename(original) {
                return Err("禁用目录中的启动文件名无效".into());
            }
            let user_root = user_startup_dir().ok_or("无法定位启动文件夹")?;
            let user_root = std::fs::canonicalize(user_root).map_err(|_| "无法验证用户启动目录")?;
            let destination = user_root.join(original);
            if path_entry_exists(&destination)? {
                return Err("启动文件夹已有同名文件，未覆盖；禁用文件保留".into());
            }
            std::fs::rename(&source, &destination).map_err(|e| format!("还原失败: {e}"))
        }
        _ => Err("无效的启动项".into()),
    }
}

// ---------- 启动项延迟启动器 ----------
// 原理：创建 \AeroTune\ 下的登录触发计划任务（带延迟），同时从 Run 键移除原值。
// 原项先持久备份，专用恢复协调 Run 与任务；不宣称跨资源原子性。

#[derive(Serialize, Deserialize, Clone)]
pub struct DelayBackup {
    pub name: String,
    pub task: String,
    token: String,
    bytes: Vec<u8>,
    vtype: u32,
}

fn delay_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA").ok_or("APPDATA 不存在，拒绝使用相对备份目录")?;
    let dir = PathBuf::from(base).join("AeroTune").join("startup_delays");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn delay_task_name(name: &str) -> String {
    // 注入式 UTF-8 十六进制编码，不使用有碰撞的字符替换。
    format!(
        "AeroTuneDelay2_{}",
        name.as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn checked_ps(script: &str) -> Result<(), String> {
    crate::util::run_checked(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("$ErrorActionPreference='Stop'; {script}"),
        ],
    )
}

// 最小安全范围：绝对本地 exe，无参数；不猜测 Windows 模糊路径/引号/环境扩展语义。
fn parse_delay_command(cmd: &str) -> Result<String, String> {
    let exe = if cmd.starts_with('"') && cmd.ends_with('"') && cmd.len() >= 2 {
        &cmd[1..cmd.len() - 1]
    } else {
        if cmd.chars().any(char::is_whitespace) {
            return Err("含参数或未引用空格路径，拒绝延迟；请保留直接启动".into());
        }
        cmd
    };
    if exe.len() < 7
        || exe.as_bytes()[1] != b':'
        || exe.as_bytes()[2] != b'\\'
        || !exe.as_bytes()[0].is_ascii_alphabetic()
        || !exe.to_ascii_lowercase().ends_with(".exe")
        || exe[2..].contains(':')
        || exe.chars().any(|c| c.is_control() || "\"%&|<>".contains(c))
    {
        return Err("仅支持明确绝对本地 exe 且无参数的启动命令；此命令无法安全转换".into());
    }
    Ok(exe.into())
}

/// Parse only the same narrow direct-executable form accepted by delayed-startup actions.
/// Parameters, environment expansion, shortcuts, scripts, relative paths, and device/UNC forms stay unknown.
fn startup_signature_candidate(cmd: &str) -> Option<PathBuf> {
    let exe = parse_delay_command(cmd).ok()?;
    let path = PathBuf::from(exe);
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(path)
}

/// Refuse missing files and any reparse point in the path before invoking WinVerifyTrust.
fn is_safe_local_signature_target(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let ancestors = path.ancestors().collect::<Vec<_>>();
    if ancestors.is_empty() {
        return false;
    }
    for ancestor in ancestors.iter().rev() {
        let Ok(metadata) = std::fs::symlink_metadata(ancestor) else {
            return false;
        };
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
        if *ancestor == path {
            if !metadata.is_file() {
                return false;
            }
        } else if !metadata.is_dir() {
            return false;
        }
    }
    true
}

const STARTUP_SIGNATURE_BUDGET_PER_LIST: usize = 8;
const STARTUP_SIGNATURE_CANDIDATE_CAP_PER_LIST: usize = 32;

fn attach_startup_signature_context(items: &mut [StartupItem]) {
    let mut budget = STARTUP_SIGNATURE_BUDGET_PER_LIST;
    let mut candidates = 0usize;
    for item in items {
        let Some(path) = startup_signature_candidate(&item.cmd) else {
            continue;
        };
        candidates += 1;
        if candidates > STARTUP_SIGNATURE_CANDIDATE_CAP_PER_LIST {
            break;
        }
        if !is_safe_local_signature_target(&path) {
            continue;
        }
        let path_text = path.to_string_lossy();
        let evidence = crate::profile::verify_signature_evidence_cached(&path_text, &mut budget);
        item.signature_status = evidence.status.as_str().to_string();
        item.signer_subject = evidence.signer_subject;
    }
}

#[tauri::command]
pub fn list_delayed_startup() -> Result<Vec<DelayBackup>, String> {
    let mut items = Vec::new();
    for entry in std::fs::read_dir(delay_dir()?).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let b = std::fs::read(&p).map_err(|e| e.to_string())?;
        items.push(
            serde_json::from_slice(&b).map_err(|e| format!("备份损坏 {}: {e}", p.display()))?,
        );
    }
    Ok(items)
}

/// 把 HKCU 启动项转为延迟启动（创建计划任务 + 移除 Run 值，双双可逆）
#[tauri::command]
pub fn delay_startup_item(name: String, delay_secs: u32) -> Result<(), String> {
    let _guard = DELAY_LOCK.lock().map_err(|e| e.to_string())?;
    if name.is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
        return Err("启动项名称不合法".into());
    }
    if !(10..=600).contains(&delay_secs) {
        return Err("延迟范围 10-600 秒".into());
    }
    let user = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_USER, winreg::enums::KEY_READ | winreg::enums::KEY_WRITE)
        .map_err(|e| e.to_string())?;
    let raw = user.get_raw_value(&name).map_err(|e| e.to_string())?;
    if raw.vtype != winreg::enums::RegType::REG_SZ {
        return Err("仅支持 REG_SZ，环境扩展类型拒绝转换".into());
    }
    let cmd: String = user.get_value(&name).map_err(|e| e.to_string())?;
    let exe = parse_delay_command(&cmd)?;
    if !PathBuf::from(&exe).is_file() {
        return Err("可执行文件不存在".into());
    }
    let tname = delay_task_name(&name);
    let path = delay_dir()?.join(format!("{tname}.json"));
    let token = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    );
    let backup = DelayBackup {
        name: name.clone(),
        task: tname.clone(),
        token,
        bytes: raw.bytes.clone(),
        vtype: raw.vtype.clone() as u32,
    };
    let bytes = serde_json::to_vec(&backup).map_err(|e| e.to_string())?;
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| format!("无法保存备份（已有恢复记录时请先恢复）: {e}"))?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("备份写入失败，未改启动项: {e}"))?;
    let script = format!(
        "$sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; \
         $act=New-ScheduledTaskAction -Execute {}; \
         $tr=New-ScheduledTaskTrigger -AtLogOn -User $sid; $tr.Delay='PT{delay_secs}S'; \
         $p=New-ScheduledTaskPrincipal -UserId $sid -LogonType Interactive -RunLevel Limited; \
         Register-ScheduledTask -TaskName {} -TaskPath '\\AeroTune\\' -Action $act -Trigger $tr -Principal $p -Description {} | Out-Null",
        ps_quote(&exe), ps_quote(&tname), ps_quote(&delay_marker(&backup)));
    // 不使用 Force；创建失败保持 Run 和备份，恢复入口可重试清理。
    if let Err(e) = checked_ps(&script) {
        let cleanup = delete_delay_task(&backup);
        return Err(format!(
            "任务创建未确认成功: {e}；原启动项未删除；任务补偿 {cleanup:?}；恢复记录保留"
        ));
    }
    let now = user.get_raw_value(&name).map_err(|e| e.to_string());
    if !matches!(&now, Ok(v) if v.bytes == raw.bytes && v.vtype == raw.vtype) {
        let cleanup = delete_delay_task(&backup);
        return Err(format!(
            "原启动项已变化，未删除；任务补偿结果 {cleanup:?}，备份保留"
        ));
    }
    if let Err(e) = user.delete_value(&name) {
        let cleanup = delete_delay_task(&backup);
        return Err(format!(
            "Run 删除失败: {e}；任务补偿结果 {cleanup:?}，备份保留"
        ));
    }
    crate::journal::record(
        &format!("启动项「{name}」延迟 {delay_secs} 秒；日志不可撤销，请用开机全景的恢复直接启动"),
        crate::journal::Revert::None,
    )?;
    Ok(())
}

fn delay_marker(b: &DelayBackup) -> String {
    format!("AeroTune delay backup {}", b.token)
}

fn delete_delay_task(b: &DelayBackup) -> Result<(), String> {
    checked_ps(&format!(
        "$t=@(Get-ScheduledTask | Where-Object {{$_.TaskPath -eq '\\AeroTune\\' -and $_.TaskName -eq {}}}); \
         if ($t.Count -gt 0) {{ if ($t.Count -ne 1 -or $t[0].Description -ne {}) {{ throw '任务所有权不匹配，拒绝删除' }}; \
         $sid=[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value; \
         if ($t[0].Principal.UserId -ne $sid) {{ throw '任务用户不匹配' }}; \
         $t[0] | Unregister-ScheduledTask -Confirm:$false }}",
        ps_quote(&b.task), ps_quote(&delay_marker(b))))
}

/// 取消延迟：只从持久备份恢复，不从任务参数猜测命令
#[tauri::command]
pub fn undelay_startup_item(name: String) -> Result<(), String> {
    let _guard = DELAY_LOCK.lock().map_err(|e| e.to_string())?;
    let tname = delay_task_name(&name);
    let path = delay_dir()?.join(format!("{tname}.json"));
    let backup: DelayBackup = serde_json::from_slice(
        &std::fs::read(&path).map_err(|e| format!("无持久备份，旧版任务拒绝自动猜测恢复: {e}"))?,
    )
    .map_err(|e| e.to_string())?;
    if backup.name != name || backup.task != tname || backup.vtype != 1 {
        return Err("备份身份或类型异常".into());
    }
    let (user, _) = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .create_subkey(RUN_USER)
        .map_err(|e| e.to_string())?;
    let original = winreg::RegValue {
        bytes: backup.bytes.clone(),
        vtype: winreg::enums::RegType::REG_SZ,
    };
    let existed = match user.get_raw_value(&name) {
        Ok(v) if v.bytes == original.bytes && v.vtype == original.vtype => true,
        Ok(_) => return Err("同名 Run 值已变化，拒绝覆盖；备份保留".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.to_string()),
    };
    if !existed {
        user.set_raw_value(&name, &original)
            .map_err(|e| e.to_string())?;
    }
    if let Err(e) = delete_delay_task(&backup) {
        // 删除失败时撤销本次 Run 写入，避免同时直接/延迟启动；记录始终保留。
        let compensation = if !existed {
            user.delete_value(&name).map_err(|e| e.to_string())
        } else {
            Ok(())
        };
        return Err(format!(
            "任务删除失败: {e}；Run 补偿 {compensation:?}；备份保留，请重试恢复"
        ));
    }
    std::fs::remove_file(&path).map_err(|e| format!("已恢复，但备份清理失败，可重试: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_filename_round_trip() {
        for original in [
            "my_app.lnk",
            "a_b_c.exe",
            "report.v2_test.bat",
            "a__b.lnk",
            "应用_启动.lnk",
        ] {
            let stored = encode_disabled_name(original);
            assert_eq!(decode_disabled_name(&stored), original);
        }
    }

    #[test]
    fn disabled_filename_compatibility() {
        // Existing files used the same encoding; a plain disabled file keeps its name.
        assert_eq!(decode_disabled_name("my_app_lnk_my_app.lnk"), "my_app.lnk");
        assert_eq!(decode_disabled_name("plain_file.lnk"), "plain_file.lnk");
    }

    struct TempStartupDirs(PathBuf);

    impl TempStartupDirs {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "aerotune-startup-path-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(root.join("user")).unwrap();
            std::fs::create_dir_all(root.join("disabled")).unwrap();
            Self(root)
        }

        fn user(&self) -> PathBuf {
            self.0.join("user")
        }

        fn disabled(&self) -> PathBuf {
            self.0.join("disabled")
        }
    }

    impl Drop for TempStartupDirs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn startup_file_validation_requires_a_direct_regular_allowed_child() {
        let dirs = TempStartupDirs::new();
        let allowed = dirs.user().join("allowed.lnk");
        std::fs::write(&allowed, b"link").unwrap();
        let canonical = validated_startup_file(&allowed, &dirs.user()).unwrap();
        assert_eq!(
            canonical.parent(),
            Some(std::fs::canonicalize(dirs.user()).unwrap().as_path())
        );

        let unsupported = dirs.user().join("notes.txt");
        std::fs::write(&unsupported, b"text").unwrap();
        assert!(validated_startup_file(&unsupported, &dirs.user()).is_err());

        let nested_dir = dirs.user().join("nested");
        std::fs::create_dir_all(&nested_dir).unwrap();
        let nested = nested_dir.join("nested.exe");
        std::fs::write(&nested, b"exe").unwrap();
        assert!(validated_startup_file(&nested, &dirs.user()).is_err());

        let sibling = dirs.0.join("user-evil");
        std::fs::create_dir_all(&sibling).unwrap();
        let sibling_file = sibling.join("evil.bat");
        std::fs::write(&sibling_file, b"bat").unwrap();
        assert!(validated_startup_file(&sibling_file, &dirs.user()).is_err());
        assert!(validated_startup_file(&PathBuf::from("relative.exe"), &dirs.user()).is_err());
        assert!(validated_startup_file(&dirs.user(), &dirs.user()).is_err());
        assert!(validated_startup_file(&dirs.user().join("missing.exe"), &dirs.user()).is_err());

        #[cfg(windows)]
        {
            let symlink = dirs.user().join("link.exe");
            if std::os::windows::fs::symlink_file(&allowed, &symlink).is_ok() {
                assert!(validated_startup_file(&symlink, &dirs.user()).is_err());
            }
        }
    }

    #[test]
    fn restore_source_is_confined_but_keeps_legacy_disabled_names() {
        let dirs = TempStartupDirs::new();
        for original in ["encoded_app.lnk", "legacy_app.exe"] {
            let stored = if original.starts_with("encoded") {
                encode_disabled_name(original)
            } else {
                original.to_string()
            };
            let path = dirs.disabled().join(&stored);
            std::fs::write(&path, b"startup").unwrap();
            let validated = validated_startup_file(&path, &dirs.disabled()).unwrap();
            let leaf = validated.file_name().unwrap().to_str().unwrap();
            assert_eq!(decode_disabled_name(leaf), original);
        }
        assert!(!valid_startup_basename("foo:stream.lnk"));
        assert!(!valid_startup_basename("unsupported.txt"));

        let outside = dirs.0.join("outside.lnk");
        std::fs::write(&outside, b"outside").unwrap();
        assert!(validated_startup_file(&outside, &dirs.disabled()).is_err());
    }

    #[test]
    fn delay_task_name_sanitized() {
        let n = delay_task_name("OneDrive Startup");
        assert!(n.starts_with("AeroTuneDelay2_"));
        assert_ne!(delay_task_name("a b"), delay_task_name("a_b"));
        assert!(!n.contains(' '));
        assert!(n.len() < 80);
    }

    #[test]
    fn delay_command_refuses_ambiguous_semantics() {
        assert_eq!(
            parse_delay_command(r#""C:\Program Files\App\app.exe""#).unwrap(),
            r"C:\Program Files\App\app.exe"
        );
        assert!(parse_delay_command(r"C:\app.exe --silent").is_err());
        assert!(parse_delay_command(r"%APPDATA%\app.exe").is_err());
        assert!(parse_delay_command(r"cmd.exe /c app.exe").is_err());
        assert!(parse_delay_command(r"C:\app.exe&calc.exe").is_err());
        assert!(parse_delay_command(r"\\server\app.exe").is_err());
        assert!(parse_delay_command(r#""C:\app.exe" "arg""#).is_err());
        assert_eq!(ps_quote("a'b"), "'a''b'");
    }

    #[test]
    fn startup_signature_candidate_is_limited_to_direct_local_executables() {
        assert_eq!(
            startup_signature_candidate(r#""C:\Program Files\App\app.exe""#),
            Some(PathBuf::from(r"C:\Program Files\App\app.exe"))
        );
        for cmd in [
            "app.exe",
            r#""C:\App\app.exe" --silent"#,
            r"%APPDATA%\app.exe",
            r"\\server\share\app.exe",
            r"\\?\C:\App\app.exe",
            r"C:\App\app.exe:stream",
            r"C:\App\folder\..\app.exe",
            r"C:\App\app.lnk",
            r"C:\App\app.bat",
        ] {
            assert_eq!(startup_signature_candidate(cmd), None, "{cmd}");
        }
    }

    #[test]
    fn startup_impact_parser_bounds_and_orders_system_events() {
        let raw = r#"[
          {"ts":20,"name":"slow.exe","path":"C:\\Apps\\slow.exe","total_ms":5000,"degradation_ms":2400},
          {"ts":30,"name":"fast.exe","path":"","total_ms":1000,"degradation_ms":300},
          {"ts":40,"name":"bad.exe","total_ms":900000,"degradation_ms":1},
          {"ts":50,"name":"","total_ms":1,"degradation_ms":1}
        ]"#;
        let impacts = parse_startup_impacts(raw).unwrap();
        assert_eq!(impacts.len(), 2);
        assert_eq!(impacts[0].name, "slow.exe");
        assert_eq!(impacts[0].degradation_ms, 2400);
        assert_eq!(impacts[1].name, "fast.exe");
    }
}
