use std::os::windows::process::CommandExt;
use tauri::AppHandle;
use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

fn parse_local_open_path(input: &str) -> Result<(std::path::PathBuf, char), String> {
    let path = if input.starts_with('"') || input.ends_with('"') {
        if !(input.starts_with('"') && input.ends_with('"')) || input.len() < 3 {
            return Err("只接受本地文件或文件夹路径".into());
        }
        &input[1..input.len() - 1]
    } else {
        input
    };
    let bytes = path.as_bytes();
    if path.is_empty()
        || path.contains('\0')
        || path.contains('"')
        || bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || !matches!(bytes[2], b'\\' | b'/')
        || path[2..].contains(':')
        || path[3..]
            .split(['\\', '/'])
            .any(|component| matches!(component, "." | ".."))
    {
        return Err("只接受规范的本地绝对路径（不支持网络、设备或 ADS 路径）".into());
    }
    let candidate = std::path::PathBuf::from(path);
    if !candidate.is_absolute() {
        return Err("只接受本地绝对路径".into());
    }
    Ok((candidate, bytes[0] as char))
}

fn resolve_local_open_path(input: &str) -> Result<(std::path::PathBuf, bool), String> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, FILE_ATTRIBUTE_REPARSE_POINT};
    use windows_sys::Win32::System::WindowsProgramming::{
        DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOVABLE,
    };

    let (candidate, drive) = parse_local_open_path(input)?;
    let drive_root = format!("{}:\\", drive.to_ascii_uppercase());
    let mut drive_root_wide: Vec<u16> = std::ffi::OsStr::new(&drive_root).encode_wide().collect();
    drive_root_wide.push(0);
    let drive_type = unsafe { GetDriveTypeW(drive_root_wide.as_ptr()) };
    if !matches!(
        drive_type,
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_CDROM | DRIVE_RAMDISK
    ) {
        return Err("拒绝访问网络或未知驱动器上的路径".into());
    }

    let is_reparse_point = |metadata: &std::fs::Metadata| {
        metadata.file_type().is_symlink()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    };
    let mut current = std::path::PathBuf::from(&drive_root);
    let mut target_metadata =
        std::fs::symlink_metadata(&current).map_err(|_| "本地驱动器不可用".to_string())?;
    if is_reparse_point(&target_metadata) {
        return Err("拒绝通过重解析点打开路径".into());
    }
    let components: Vec<_> = candidate
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(name) => Some(name.to_os_string()),
            std::path::Component::Prefix(_) | std::path::Component::RootDir => None,
            std::path::Component::CurDir | std::path::Component::ParentDir => Some("..".into()),
        })
        .collect();
    for (index, component) in components.iter().enumerate() {
        if component == ".." {
            return Err("不支持包含相对跳转的路径".into());
        }
        current.push(component);
        target_metadata = std::fs::symlink_metadata(&current)
            .map_err(|_| "本地文件或文件夹不存在或不可访问".to_string())?;
        if is_reparse_point(&target_metadata) {
            return Err("拒绝通过重解析点打开路径".into());
        }
        if index + 1 < components.len() && !target_metadata.is_dir() {
            return Err("路径中的父项不是文件夹".into());
        }
    }
    if !target_metadata.is_dir() && !target_metadata.is_file() {
        return Err("只允许打开普通文件或文件夹".into());
    }
    let canonical =
        std::fs::canonicalize(&current).map_err(|_| "无法验证本地文件或文件夹".to_string())?;
    Ok((canonical, target_metadata.is_dir()))
}

fn system_explorer_path() -> Result<std::path::PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;

    let mut buffer = vec![0u16; 32_768];
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("无法解析 Windows 系统目录".into());
    }
    let path =
        std::path::PathBuf::from(OsString::from_wide(&buffer[..length])).join("explorer.exe");
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| "无法验证 Windows 资源管理器程序".to_string())?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("Windows 资源管理器目标无效".into());
    }
    Ok(path)
}

/// Open a verified local file/folder without probing renderer-supplied network paths.
#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    use std::process::Command;

    let (canonical, is_dir) = resolve_local_open_path(&path)?;
    let explorer = system_explorer_path()?;
    if is_dir {
        Command::new(&explorer)
            .arg(&canonical)
            .creation_flags(crate::util::CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("打开文件夹失败: {error}"))?;
    } else {
        // Explorer's /select argument is assembled only after local-path validation.
        let path = canonical.to_string_lossy().replace('"', "");
        Command::new(&explorer)
            .raw_arg(format!("/select,\"{path}\""))
            .creation_flags(crate::util::CREATE_NO_WINDOW)
            .spawn()
            .map_err(|error| format!("在资源管理器中定位文件失败: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod open_path_tests {
    use super::parse_local_open_path;

    #[test]
    fn open_path_rejects_unc_devices_relative_paths_and_ads_lexically() {
        for path in [
            r"\\server\share\file.txt",
            r"//server/share/file.txt",
            r"\\?\C:\Windows",
            r"\\?\UNC\server\share",
            r"\\.\pipe\name",
            r"\??\C:\Windows",
            r"C:Windows\explorer.exe",
            r"relative\file.exe",
            r"C:\Windows\..\Windows\win.ini",
            r"C:\Users\user\file.txt:secret",
            "ms-settings:display",
        ] {
            assert!(parse_local_open_path(path).is_err(), "accepted {path:?}");
        }
    }

    #[test]
    fn open_path_accepts_only_drive_rooted_absolute_local_candidates() {
        let (path, drive) =
            parse_local_open_path(r#""C:\Program Files\AeroTune\AeroTune.exe""#).unwrap();
        assert_eq!(drive, 'C');
        assert!(path.is_absolute());
        assert!(parse_local_open_path(r#"C:\Program Files\AeroTune\app"quoted".exe"#).is_err());
    }
}

/// Open one of the built-in Windows Settings pages. The allowlist prevents
/// this convenience command from becoming an arbitrary URI/command launcher.
fn settings_uri(page: &str) -> Option<&'static str> {
    match page {
        "display" => Some("ms-settings:display"),
        "sound" => Some("ms-settings:sound"),
        "network" => Some("ms-settings:network-status"),
        "windows_update" => Some("ms-settings:windowsupdate"),
        "startup" => Some("ms-settings:startupapps"),
        "storage" => Some("ms-settings:storagesense"),
        "apps" => Some("ms-settings:appsfeatures"),
        "bluetooth" => Some("ms-settings:bluetooth"),
        "night_light" => Some("ms-settings:nightlight"),
        _ => None,
    }
}

#[tauri::command]
pub fn open_settings_page(page: String) -> Result<(), String> {
    let uri = settings_uri(&page).ok_or_else(|| "不支持的 Windows 设置页面".to_string())?;
    let explorer = system_explorer_path()?;
    std::process::Command::new(&explorer)
        .arg(uri)
        .creation_flags(crate::util::CREATE_NO_WINDOW)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("打开 Windows 设置失败: {e}"))
}

#[cfg(test)]
mod settings_page_tests {
    use super::settings_uri;

    #[test]
    fn settings_shortcuts_are_allowlisted_and_exact() {
        assert_eq!(settings_uri("display"), Some("ms-settings:display"));
        assert_eq!(
            settings_uri("windows_update"),
            Some("ms-settings:windowsupdate")
        );
        assert_eq!(settings_uri("apps"), Some("ms-settings:appsfeatures"));
        assert_eq!(settings_uri("night_light"), Some("ms-settings:nightlight"));
        assert_eq!(settings_uri("ms-settings:display"), None);
        assert_eq!(settings_uri("../../anything"), None);
        assert_eq!(settings_uri(""), None);
    }
}

#[tauri::command]
pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

#[tauri::command]
pub fn is_silent_start() -> bool {
    std::env::args().any(|argument| argument == "--silent")
}

/// UAC 提权重启应用
#[tauri::command]
pub fn relaunch_as_admin(app: AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Start-Process failures (including UAC cancellation) must keep the current app alive.
    // Escape PowerShell's single-quoted literal syntax for unusual install paths.
    let quoted = exe.to_string_lossy().replace('\'', "''");
    let script = format!("$ErrorActionPreference='Stop'; Start-Process -FilePath '{quoted}' -Verb RunAs -ErrorAction Stop");
    crate::util::run_checked_with_timeout(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
        std::time::Duration::from_secs(5 * 60),
    )
    .map_err(|e| format!("提权启动失败，原窗口已保留: {e}"))?;
    app.exit(0);
    Ok(())
}

#[derive(serde::Serialize)]
pub struct SysInfo {
    pub os: String,
    pub version: String,
    pub arch: String,
    pub host: String,
    pub cpu_model: String,
    pub cores: usize,
    pub admin: bool,
    pub exe: String,
    pub silent: bool,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct DiagnosticReport {
    pub schema_version: u8,
    pub app_version: String,
    pub generated_at_utc: String,
    pub generated_at_local: String,
    pub system: SysInfoSnapshot,
    pub runtime: RuntimeSnapshot,
    pub notes: Vec<String>,
}

#[derive(serde::Serialize, Clone, Debug, PartialEq, Eq)]
pub struct SysInfoSnapshot {
    pub os: String,
    pub version: String,
    pub arch: String,
    pub host: String,
    pub cpu_model: String,
    pub cores: usize,
    pub admin: bool,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct RuntimeSnapshot {
    pub cpu_pct: f32,
    pub memory_used_gb: f64,
    pub memory_total_gb: f64,
    pub pagefile_used_gb: f64,
    pub pagefile_total_gb: f64,
    pub process_count: usize,
    pub uptime_secs: u64,
    pub sample_id: u64,
    pub health_score: u8,
    pub cpu_frequency_mhz: u64,
    pub memory_pressure_pct: Option<f64>,
    pub pagefile_pressure_pct: Option<f64>,
    pub swap_in_use: bool,
    pub has_sample: bool,
    pub network_rx_kbps: f64,
    pub network_tx_kbps: f64,
    pub network_today_rx_gb: f64,
    pub network_today_tx_gb: f64,
    pub battery_percent: Option<u8>,
    pub on_ac_power: Option<bool>,
    pub gpu_name: Option<String>,
    pub gpu_utilization_pct: Option<f32>,
    pub gpu_temperature_c: Option<f32>,
    pub disks: Vec<crate::stats::DiskInfo>,
    pub anomaly_count: usize,
    pub anomaly_categories: AnomalyCounts,
    pub health_deductions: Vec<HealthDeduction>,
    pub recommendation_categories: Vec<String>,
    pub recommendation_count: usize,
}

#[derive(serde::Serialize, Clone, Debug, Default)]
pub struct AnomalyCounts {
    pub cpu: usize,
    pub memory: usize,
    pub disk_io: usize,
}

#[derive(serde::Serialize, Clone, Debug)]
pub struct HealthDeduction {
    pub category: String,
    pub points: u8,
}

fn health_category(label: &str) -> Option<&'static str> {
    match label {
        "CPU 持续高负载" => Some("cpu-sustained-load"),
        "内存水位偏高" => Some("memory-pressure"),
        "已使用交换分区" => Some("pagefile-in-use"),
        "C 盘空间告急" => Some("system-disk-critical"),
        "C 盘空间偏低" => Some("system-disk-low"),
        "启动项过多" => Some("startup-burden"),
        "长时间未重启" => Some("long-uptime"),
        _ => None,
    }
}

fn recommendation_category(id: &str) -> Option<&str> {
    match id {
        "memory-pressure" => Some("memory-pressure"),
        "pagefile-pressure" => Some("pagefile-pressure"),
        "system-disk-low" => Some("system-disk-low"),
        "battery-low" => Some("battery-low"),
        "startup-burden" => Some("startup-burden"),
        "gpu-memory-pressure" => Some("gpu-memory-pressure"),
        "gpu-temperature-high" => Some("gpu-temperature-high"),
        _ => None,
    }
}

fn anomaly_counts(anomalies: &[crate::diag::Anomaly]) -> AnomalyCounts {
    let mut counts = AnomalyCounts::default();
    for anomaly in anomalies {
        match anomaly.kind.as_str() {
            "cpu" => counts.cpu += 1,
            "mem" => counts.memory += 1,
            "io" => counts.disk_io += 1,
            _ => {}
        }
    }
    counts
}

fn finite_nonnegative(value: f64) -> f64 {
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

fn ratio_percent(used: f64, total: f64) -> Option<f64> {
    if !used.is_finite() || !total.is_finite() || used < 0.0 || total <= 0.0 || used > total {
        return None;
    }
    Some((used / total * 100.0).clamp(0.0, 100.0))
}

pub fn assemble_diagnostic_report(
    system: SysInfoSnapshot,
    stats: &crate::stats::Stats,
    diagnostics: &crate::diag::Diagnostics,
    recommendations: Vec<crate::recommendations::SmartRecommendation>,
    generated_at_utc: String,
) -> DiagnosticReport {
    let recommendation_categories: Vec<String> = recommendations
        .into_iter()
        .filter_map(|item| recommendation_category(&item.id).map(str::to_owned))
        .collect();
    let recommendation_count = recommendation_categories.len();
    let anomaly_categories = anomaly_counts(&diagnostics.anomalies);
    let anomaly_count =
        anomaly_categories.cpu + anomaly_categories.memory + anomaly_categories.disk_io;
    DiagnosticReport {
        schema_version: 1,
        app_version: env!("CARGO_PKG_VERSION").into(),
        generated_at_utc,
        generated_at_local: chrono::Local::now()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
        system,
        runtime: RuntimeSnapshot {
            cpu_pct: if stats.cpu.is_finite() {
                stats.cpu.clamp(0.0, 100.0)
            } else {
                0.0
            },
            memory_used_gb: if stats.mem_used_gb.is_finite() && stats.mem_used_gb >= 0.0 {
                stats.mem_used_gb
            } else {
                0.0
            },
            memory_total_gb: if stats.mem_total_gb.is_finite() && stats.mem_total_gb > 0.0 {
                stats.mem_total_gb
            } else {
                0.0
            },
            pagefile_used_gb: if stats.swap_used_gb.is_finite() && stats.swap_used_gb >= 0.0 {
                stats.swap_used_gb
            } else {
                0.0
            },
            pagefile_total_gb: if stats.swap_total_gb.is_finite() && stats.swap_total_gb > 0.0 {
                stats.swap_total_gb
            } else {
                0.0
            },
            process_count: stats.process_count,
            uptime_secs: stats.uptime_secs,
            sample_id: stats.sample_id,
            health_score: diagnostics.score,
            has_sample: stats.sample_id > 0,
            cpu_frequency_mhz: stats.cpu_freq_mhz,
            memory_pressure_pct: ratio_percent(stats.mem_used_gb, stats.mem_total_gb),
            pagefile_pressure_pct: ratio_percent(stats.swap_used_gb, stats.swap_total_gb),
            swap_in_use: stats.swap_used_gb.is_finite() && stats.swap_used_gb > 0.0,
            network_rx_kbps: finite_nonnegative(stats.net_rx_kbps),
            network_tx_kbps: finite_nonnegative(stats.net_tx_kbps),
            network_today_rx_gb: finite_nonnegative(stats.net_today_rx_gb),
            network_today_tx_gb: finite_nonnegative(stats.net_today_tx_gb),
            battery_percent: stats
                .battery
                .as_ref()
                .and_then(|b| b.percent)
                .filter(|&p| p <= 100),
            on_ac_power: stats.battery.as_ref().map(|b| b.ac_online),
            gpu_name: stats.gpu.as_ref().map(|g| g.name.clone()),
            gpu_utilization_pct: stats
                .gpu
                .as_ref()
                .and_then(|g| g.utilization)
                .filter(|v| v.is_finite())
                .map(|v| v.clamp(0.0, 100.0)),
            gpu_temperature_c: stats
                .gpu
                .as_ref()
                .and_then(|g| g.temperature)
                .filter(|v| v.is_finite() && (0.0..=130.0).contains(v)),
            disks: stats
                .disks
                .iter()
                .filter(|d| {
                    d.total_gb.is_finite()
                        && d.used_gb.is_finite()
                        && d.total_gb >= 0.0
                        && d.used_gb >= 0.0
                        && d.used_gb <= d.total_gb
                })
                .cloned()
                .collect(),
            anomaly_count,
            anomaly_categories,
            health_deductions: diagnostics
                .deductions
                .iter()
                .filter_map(|d| {
                    (d.cost > 0)
                        .then(|| {
                            health_category(&d.item).map(|category| HealthDeduction {
                                category: category.into(),
                                points: d.cost,
                            })
                        })
                        .flatten()
                })
                .collect(),
            recommendation_categories,
            recommendation_count,
        },
        notes: vec![
            "此报告仅包含系统概况与聚合遥测，不包含进程名称/路径、Wi-Fi 密码或网络凭据。".into(),
            "保存前请在预览中检查最终 JSON；设备标识和详细遥测可分别选择是否包含。".into(),
        ],
    }
}

#[tauri::command]
pub fn export_diagnostic_report(
    cache: tauri::State<'_, std::sync::Arc<crate::stats::Cache>>,
    diag: tauri::State<'_, std::sync::Arc<std::sync::Mutex<crate::diag::Engine>>>,
) -> Result<String, String> {
    let sys = get_system_info();
    let stats = cache.stats.read().map_err(|e| e.to_string())?.clone();
    let diagnostics = diag.lock().map_err(|e| e.to_string())?.current.clone();
    let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let recommendations =
        crate::recommendations::recommend_from_snapshots(&stats, &diagnostics, &system_drive);
    let report = assemble_diagnostic_report(
        SysInfoSnapshot {
            os: sys.os,
            version: sys.version,
            arch: sys.arch,
            host: sys.host,
            cpu_model: sys.cpu_model,
            cores: sys.cores,
            admin: sys.admin,
        },
        &stats,
        &diagnostics,
        recommendations,
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    );
    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_system_info() -> SysInfo {
    use sysinfo::System;
    let sys = System::new_all();
    SysInfo {
        os: System::name().unwrap_or_else(|| "Windows".into()),
        version: System::os_version().unwrap_or_default(),
        arch: std::env::consts::ARCH.replace("x86_64", "x64"),
        host: System::host_name().unwrap_or_default(),
        cpu_model: sys
            .cpus()
            .first()
            .map(|c| c.brand().trim().to_string())
            .unwrap_or_default(),
        cores: sys.physical_core_count().unwrap_or(0),
        admin: is_admin(),
        exe: std::env::current_exe()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default(),
        silent: std::env::args().any(|a| a == "--silent"),
    }
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

/// 开机自启（HKCU Run，无需管理员），带 --silent 静默驻留托盘
#[tauri::command]
pub fn set_autostart(enable: bool) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let hkcu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_READ | winreg::enums::KEY_WRITE)
        .map_err(|e| e.to_string())?;
    if enable {
        hkcu.set_value("AeroTune", &format!("\"{}\" --silent", exe.display()))
            .map_err(|e| e.to_string())?;
    } else {
        let _ = hkcu.delete_value("AeroTune");
    }
    Ok(())
}

#[tauri::command]
pub fn get_autostart() -> bool {
    match winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_READ)
    {
        Ok(k) => k.get_value::<String, _>("AeroTune").is_ok(),
        Err(_) => false,
    }
}

const MAX_TEXT_EXPORT_BYTES: usize = 32 * 1024 * 1024;

fn validate_text_export_request(
    filename: &str,
    content_bytes: usize,
) -> Result<&'static str, String> {
    if content_bytes > MAX_TEXT_EXPORT_BYTES {
        return Err("导出内容超过安全上限，请缩小范围后重试。".into());
    }
    if filename.len() > 128
        || !filename.is_ascii()
        || !filename.starts_with("AeroTune-")
        || !filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err("导出文件名不安全。".into());
    }
    let Some((stem, extension)) = filename.rsplit_once('.') else {
        return Err("导出文件必须使用 CSV 或 JSON 扩展名。".into());
    };
    if stem.len() <= "AeroTune-".len() {
        return Err("导出文件名无效。".into());
    }
    match extension.to_ascii_lowercase().as_str() {
        "csv" => Ok("csv"),
        "json" => Ok("json"),
        _ => Err("仅允许导出 CSV 或 JSON 文件。".into()),
    }
}

/// Open a native Save dialog and write only to the path selected there.
#[tauri::command]
pub async fn save_text_export(
    app: AppHandle,
    filename: String,
    contents: String,
) -> Result<bool, String> {
    let extension = validate_text_export_request(&filename, contents.len())?.to_string();
    let dialog_extension = extension.clone();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;

        let filter_name = if dialog_extension == "csv" {
            "CSV 文件"
        } else {
            "JSON 文件"
        };
        let extensions = [dialog_extension.as_str()];
        app.dialog()
            .file()
            .set_title("保存 AeroTune 导出")
            .set_file_name(filename)
            .add_filter(filter_name, &extensions)
            .blocking_save_file()
            .map(|file| {
                file.into_path()
                    .map_err(|_| "所选保存路径不可用。".to_string())
            })
            .transpose()
    })
    .await
    .map_err(|error| format!("保存对话框失败: {error}"))??;
    let Some(path) = selected else {
        return Ok(false);
    };
    if !path.is_absolute() || path.is_dir() {
        return Err("所选保存目标无效。".into());
    }
    if let Some(extension_on_disk) = path.extension().and_then(|value| value.to_str()) {
        if !extension_on_disk.eq_ignore_ascii_case(&extension) {
            return Err("保存文件扩展名与导出格式不符。".into());
        }
    } else {
        return Err("所选文件缺少有效扩展名。".into());
    }
    if std::fs::symlink_metadata(&path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err("为避免覆盖链接目标，不支持将导出写入符号链接。".into());
    }
    crate::util::atomic_write(&path, contents.as_bytes())?;
    tracing::info!(extension, bytes = contents.len(), "用户导出已安全保存");
    Ok(true)
}

/// 导出配置到桌面（自动命名）
#[tauri::command]
pub fn export_config() -> Result<String, String> {
    let src = crate::util::app_dir().join("auto_settings.json");
    if !src.exists() {
        return Err("配置文件尚未生成".into());
    }
    let home = std::env::var("USERPROFILE").map_err(|_| "无法定位用户目录".to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let dst = std::path::PathBuf::from(home)
        .join("Desktop")
        .join(format!("AeroTune-config-{stamp}.json"));
    std::fs::copy(&src, &dst).map_err(|e| format!("导出失败: {e}"))?;
    tracing::info!(to = %dst.display(), "配置已导出");
    Ok(dst.to_string_lossy().into_owned())
}

fn validate_import(mut s: crate::auto::AutoSettings) -> Result<crate::auto::AutoSettings, String> {
    if !(30..=3600).contains(&s.away_idle_secs) || !(30.0..=95.0).contains(&s.heavy_cpu_threshold) {
        return Err("引擎阈值超出安全范围".into());
    }
    if s.rules.len() > 3 || s.game_list.len() > 256 || s.history.len() > 7 {
        return Err("配置列表超出限制".into());
    }
    let mut kinds = std::collections::HashSet::new();
    for r in &s.rules {
        let max = match r.kind.as_str() {
            "cpu" => 100.0,
            "mem" => 1_048_576.0,
            "io" => 1_073_741_824.0,
            _ => return Err("未知规则维度".into()),
        };
        if r.action == "suspend" {
            return Err("配置中的自动挂起规则已停用；请改为“仅提醒”后重新导入。".into());
        }
        if !kinds.insert(&r.kind)
            || !r.threshold.is_finite()
            || r.threshold <= 0.0
            || r.threshold > max
            || !(3..=600).contains(&r.seconds)
            || !["notify", "deprio"].contains(&r.action.as_str())
        {
            return Err("规则参数不合法".into());
        }
    }
    for name in &mut s.game_list {
        if name.is_empty()
            || name.len() > 260
            || name
                .chars()
                .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        {
            return Err("游戏名单只接受进程名，不接受路径或控制字符".into());
        }
        *name = crate::auto::normalize_name(name);
        if name.is_empty() {
            return Err("游戏进程名不能为空".into());
        }
    }
    s.game_list.sort();
    s.game_list.dedup();
    let guid_re =
        regex::Regex::new(r"^[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}$").unwrap();
    for guid in [&s.high_plan_guid, &s.saver_plan_guid]
        .into_iter()
        .flatten()
    {
        if !guid_re.is_match(guid) {
            return Err("电源计划 GUID 不合法".into());
        }
    }
    // 导出无可信机器身份：始终丢弃机器本地 GUID 与运行统计，重新解析本机计划。
    s.high_plan_guid = None;
    s.saver_plan_guid = None;
    s.state_seconds.clear();
    s.history.clear();
    Ok(s)
}

const MAX_CONFIG_IMPORT_BYTES: u64 = 1_048_576;

fn validate_config_import_path(path: &std::path::Path) -> Result<(), String> {
    let path_text = path.to_string_lossy();
    if !path.is_absolute() || path_text.starts_with(r"\\") {
        return Err("配置必须是绝对本地文件路径".into());
    }
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        return Err("请选择 .json 配置文件".into());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "配置文件不存在或不可访问")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("配置导入只接受本地普通文件，不接受目录或链接".into());
    }
    if metadata.len() > MAX_CONFIG_IMPORT_BYTES {
        return Err("配置文件超过 1 MiB".into());
    }
    Ok(())
}

fn read_config_import_file(path: &std::path::Path) -> Result<Vec<u8>, String> {
    use std::io::Read;

    validate_config_import_path(path)?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| "无法读取所选配置文件".to_string())?
        .take(MAX_CONFIG_IMPORT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取所选配置文件".to_string())?;
    if bytes.len() as u64 > MAX_CONFIG_IMPORT_BYTES {
        return Err("配置文件超过 1 MiB".into());
    }
    Ok(bytes)
}

fn parse_imported_config(bytes: &[u8]) -> Result<crate::auto::AutoSettings, String> {
    let json = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let settings =
        serde_json::from_slice(json).map_err(|error| format!("配置文件格式不正确: {error}"))?;
    validate_import(settings)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AdaptiveImportTransition {
    Start,
    Stop,
    Keep,
}

fn adaptive_import_transition(current: bool, imported: bool) -> AdaptiveImportTransition {
    match (current, imported) {
        (false, true) => AdaptiveImportTransition::Start,
        (true, false) => AdaptiveImportTransition::Stop,
        _ => AdaptiveImportTransition::Keep,
    }
}

/// 在 Rust 内打开原生选择器、读取并导入本地配置；不把文件路径交还渲染器。
#[tauri::command]
pub async fn import_config(
    app: AppHandle,
    auto: tauri::State<'_, crate::auto::AutoStateLock>,
) -> Result<bool, String> {
    let selected = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;

        app.dialog()
            .file()
            .set_title("导入 AeroTune 配置")
            .add_filter("JSON 配置文件", &["json"])
            .blocking_pick_file()
            .map(|file| {
                file.into_path()
                    .map_err(|_| "所选配置文件路径不可用".to_string())
            })
            .transpose()
    })
    .await
    .map_err(|error| format!("配置选择器失败: {error}"))??;
    let Some(path) = selected else {
        return Ok(false);
    };
    let bytes = tauri::async_runtime::spawn_blocking(move || read_config_import_file(&path))
        .await
        .map_err(|error| error.to_string())??;
    let parsed = parse_imported_config(&bytes)?;
    let mut st = auto
        .lock()
        .map_err(|_| "配置运行时状态不可用".to_string())?;
    if st.adaptive.baseline_restore_pending {
        return Err("系统基线恢复尚未完成；请先在智能引擎页重试恢复，再导入配置".into());
    }
    let transition = adaptive_import_transition(st.settings.adaptive, parsed.adaptive);
    if transition == AdaptiveImportTransition::Stop {
        st.restore_baseline()
            .map_err(|error| format!("配置未导入：{error}"))?;
    }
    crate::auto::save_settings_pub(&parsed)?;
    st.settings = parsed.clone();
    match transition {
        AdaptiveImportTransition::Start => {
            let cur = crate::auto::knobs_current_pub();
            st.adaptive.baseline_plan_guid = crate::power::active_scheme_guid();
            st.adaptive.baseline = Some(cur.clone());
            st.adaptive.applied = Some(cur);
            st.adaptive.state = crate::auto::classifier_mode_key(&crate::tweaks::current_mode());
            st.adaptive.pending.clear();
        }
        AdaptiveImportTransition::Stop | AdaptiveImportTransition::Keep => {}
    }
    st.sync_after_settings_import();
    tracing::info!(bytes = bytes.len(), "用户配置已导入");
    Ok(true)
}

#[cfg(test)]
mod import_tests {
    use crate::stats::{DiskInfo, GpuInfo, Stats};

    #[test]
    fn diagnostic_report_contains_aggregates_and_filters_invalid_telemetry() {
        let system = SysInfoSnapshot {
            os: "Windows".into(),
            version: "11".into(),
            arch: "x64".into(),
            host: "fixture".into(),
            cpu_model: "CPU".into(),
            cores: 8,
            admin: false,
        };
        let stats = Stats {
            cpu: 140.0,
            mem_used_gb: 8.0,
            mem_total_gb: 16.0,
            cpu_freq_mhz: 3600,
            swap_used_gb: f64::NAN,
            swap_total_gb: 8.0,
            process_count: 42,
            uptime_secs: 120,
            net_rx_kbps: 12.5,
            net_tx_kbps: 4.5,
            net_today_rx_gb: 1.25,
            net_today_tx_gb: 0.75,
            battery_status_ok: true,
            battery: Some(crate::stats::BatteryInfo {
                ac_online: false,
                ac_connected: Some(false),
                percent: Some(48),
                secs_left: Some(3600),
            }),
            disks: vec![
                DiskInfo {
                    mount: "C:".into(),
                    total_gb: 100.0,
                    used_gb: 80.0,
                    ..Default::default()
                },
                DiskInfo {
                    mount: "D:".into(),
                    total_gb: 50.0,
                    used_gb: 60.0,
                    ..Default::default()
                },
            ],
            gpu: Some(GpuInfo {
                name: "Fixture GPU".into(),
                utilization: Some(150.0),
                temperature: Some(999.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        let diagnostics = crate::diag::Diagnostics {
            score: 82,
            anomalies: vec![
                crate::diag::Anomaly {
                    pid: 123,
                    name: "private-process-name".into(),
                    kind: "cpu".into(),
                    value: 90.0,
                    baseline: 5.0,
                    robust_z: 8.0,
                    onset_unix: None,
                    creation_filetime: None,
                },
                crate::diag::Anomaly {
                    pid: 456,
                    name: "another-process".into(),
                    kind: "mem".into(),
                    value: 1000.0,
                    baseline: 20.0,
                    robust_z: 7.0,
                    onset_unix: None,
                    creation_filetime: None,
                },
                crate::diag::Anomaly {
                    pid: 789,
                    name: "io-process".into(),
                    kind: "io".into(),
                    value: 100.0,
                    baseline: 1.0,
                    robust_z: 6.0,
                    onset_unix: None,
                    creation_filetime: None,
                },
            ],
            deductions: vec![crate::diag::Deduction {
                item: "内存水位偏高".into(),
                cost: 8,
            }],
            ..Default::default()
        };
        let recommendations = vec![
            crate::recommendations::SmartRecommendation {
                id: "process-123-cpu".into(),
                title: "private process".into(),
                reason: "secret process detail".into(),
                action: "processes".into(),
                priority: 80,
            },
            crate::recommendations::SmartRecommendation {
                id: "memory-pressure".into(),
                title: "memory".into(),
                reason: "aggregated pressure".into(),
                action: "processes".into(),
                priority: 70,
            },
            crate::recommendations::SmartRecommendation {
                id: "future-unknown-category".into(),
                title: "unknown".into(),
                reason: "future detail".into(),
                action: "processes".into(),
                priority: 60,
            },
        ];
        let report = super::assemble_diagnostic_report(
            system,
            &stats,
            &diagnostics,
            recommendations,
            "2026-01-01T00:00:00Z".into(),
        );
        assert_eq!(report.schema_version, 1);
        assert_eq!(report.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(report.runtime.cpu_pct, 100.0);
        assert_eq!(report.runtime.memory_pressure_pct, Some(50.0));
        assert_eq!(report.runtime.pagefile_pressure_pct, None);
        assert_eq!(report.runtime.cpu_frequency_mhz, 3600);
        assert_eq!(report.runtime.network_rx_kbps, 12.5);
        assert_eq!(report.runtime.battery_percent, Some(48));
        assert_eq!(report.runtime.on_ac_power, Some(false));
        assert_eq!(report.runtime.sample_id, 0);
        assert!(!report.runtime.has_sample);
        assert_eq!(report.runtime.anomaly_count, 3);
        assert_eq!(report.runtime.anomaly_categories.cpu, 1);
        assert_eq!(report.runtime.anomaly_categories.memory, 1);
        assert_eq!(report.runtime.anomaly_categories.disk_io, 1);
        assert_eq!(
            report.runtime.health_deductions[0].category,
            "memory-pressure"
        );
        assert_eq!(report.runtime.health_deductions[0].points, 8);
        assert!(!serde_json::to_string(&report)
            .unwrap()
            .contains("内存水位偏高"));
        assert_eq!(report.runtime.recommendation_count, 1);
        assert_eq!(
            report.runtime.recommendation_categories,
            vec!["memory-pressure"]
        );
        assert!(!report.generated_at_local.is_empty());
        assert_eq!(report.runtime.pagefile_used_gb, 0.0);
        assert_eq!(report.runtime.gpu_utilization_pct, Some(100.0));
        assert_eq!(report.runtime.gpu_temperature_c, None);
        assert_eq!(report.runtime.disks.len(), 1);
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("process_name"));
        assert!(!json.contains("private-process-name"));
        assert!(!json.contains("another-process"));
        assert!(!json.contains("secret process detail"));
        assert!(!json.contains("future-unknown-category"));
        assert_eq!(
            report.runtime.anomaly_categories.cpu
                + report.runtime.anomaly_categories.memory
                + report.runtime.anomaly_categories.disk_io,
            report.runtime.anomaly_count
        );
        assert!(json.contains("不包含进程名称"));
        assert!(!json.contains("\"exe\""));
    }

    use super::*;

    #[test]
    fn config_picker_reader_accepts_only_bounded_local_json_files() {
        let root =
            std::env::temp_dir().join(format!("aerotune-import-path-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let json_path = root.join("settings.JSON");
        std::fs::write(&json_path, b"{}").unwrap();
        assert!(validate_config_import_path(&json_path).is_ok());
        assert_eq!(read_config_import_file(&json_path).unwrap(), b"{}");

        let text_path = root.join("settings.txt");
        std::fs::write(&text_path, b"{}").unwrap();
        assert!(validate_config_import_path(&text_path).is_err());
        let directory_path = root.join("directory.json");
        std::fs::create_dir_all(&directory_path).unwrap();
        assert!(validate_config_import_path(&directory_path).is_err());
        assert!(
            validate_config_import_path(std::path::Path::new(r"\\server\share\settings.json"))
                .is_err()
        );

        let large_path = root.join("large.json");
        std::fs::write(&large_path, vec![0; MAX_CONFIG_IMPORT_BYTES as usize + 1]).unwrap();
        assert!(read_config_import_file(&large_path).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn imported_config_accepts_optional_utf8_bom() {
        assert!(parse_imported_config(b"not JSON").is_err());
        let settings = crate::auto::AutoSettings::default();
        let json = serde_json::to_vec(&settings).unwrap();
        let mut with_bom = vec![0xEF, 0xBB, 0xBF];
        with_bom.extend_from_slice(&json);

        let plain = parse_imported_config(&json).unwrap();
        let bom = parse_imported_config(&with_bom).unwrap();
        assert_eq!(plain.auto_power, bom.auto_power);
        assert_eq!(plain.adaptive, bom.adaptive);
        assert_eq!(plain.close_to_tray, bom.close_to_tray);
        assert!(bom.high_plan_guid.is_none());
    }

    #[test]
    fn imported_adaptive_transition_preserves_or_restores_baseline() {
        assert_eq!(
            adaptive_import_transition(false, true),
            AdaptiveImportTransition::Start
        );
        assert_eq!(
            adaptive_import_transition(true, false),
            AdaptiveImportTransition::Stop
        );
        assert_eq!(
            adaptive_import_transition(true, true),
            AdaptiveImportTransition::Keep
        );
        assert_eq!(
            adaptive_import_transition(false, false),
            AdaptiveImportTransition::Keep
        );
    }

    #[test]
    fn rejects_unsafe_ranges_rules_and_paths() {
        let s = crate::auto::AutoSettings {
            heavy_cpu_threshold: f32::NAN,
            ..Default::default()
        };
        assert!(validate_import(s).is_err());
        let s = crate::auto::AutoSettings {
            game_list: vec![r"C:\game.exe".into()],
            ..Default::default()
        };
        assert!(validate_import(s).is_err());
        let mut s = crate::auto::AutoSettings::default();
        s.rules[0].threshold = f64::INFINITY;
        assert!(validate_import(s).is_err());
        let s = crate::auto::AutoSettings {
            high_plan_guid: Some("bad-guid".into()),
            ..Default::default()
        };
        assert!(validate_import(s).is_err());

        let mut legacy_suspend = crate::auto::AutoSettings::default();
        legacy_suspend.rules[0].action = "suspend".into();
        assert!(matches!(
            validate_import(legacy_suspend),
            Err(error) if error.contains("自动挂起规则已停用")
        ));
    }
    #[test]
    fn migration_drops_local_guids_and_normalizes_names() {
        let mut s = crate::auto::AutoSettings {
            high_plan_guid: Some("381b4222-f694-41f0-9685-ff5bb260df2e".into()),
            game_list: vec!["Game.EXE".into(), "game".into()],
            ..Default::default()
        };
        s.state_seconds.insert("game".into(), 123);
        let s = validate_import(s).unwrap();
        assert!(s.high_plan_guid.is_none());
        assert!(s.state_seconds.is_empty());
        assert_eq!(s.game_list, vec!["game"]);
    }
}

#[cfg(test)]
mod text_export_tests {
    use super::{validate_text_export_request, MAX_TEXT_EXPORT_BYTES};

    #[test]
    fn accepts_only_bounded_aerotune_csv_and_json_names() {
        assert_eq!(
            validate_text_export_request("AeroTune-report-2026-10-20.json", 100),
            Ok("json")
        );
        assert_eq!(
            validate_text_export_request("AeroTune-processes-2026-10-20.csv", 100),
            Ok("csv")
        );
        assert!(validate_text_export_request("report.json", 100).is_err());
        assert!(validate_text_export_request("AeroTune-report.html", 100).is_err());
        assert!(validate_text_export_request("AeroTune-..\\escape.json", 100).is_err());
        assert!(validate_text_export_request("AeroTune-report-恶意.json", 100).is_err());
        assert!(validate_text_export_request("AeroTune-.json", 100).is_err());
        assert!(
            validate_text_export_request("AeroTune-report.json", MAX_TEXT_EXPORT_BYTES + 1)
                .is_err()
        );
    }
}

// ---------- 系统还原点 ----------

/// 同步创建还原点（手动与"重大变更前自动打点"共用）
pub fn create_restore_point_sync(desc: &str) -> Result<(), String> {
    let clean: String = desc
        .chars()
        .filter(|c| *c != '\'' && *c != '"')
        .take(80)
        .collect();
    let script = format!(
        "Checkpoint-Computer -Description 'AeroTune: {clean}' -RestorePointType MODIFY_SETTINGS"
    );
    crate::util::run_checked_with_timeout(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
        std::time::Duration::from_secs(10 * 60),
    )
}

#[tauri::command]
pub async fn create_restore_point(desc: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        create_restore_point_sync(&desc)?;
        tracing::info!(desc = %desc, "系统还原点已创建");
        Ok("还原点已创建".into())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 重大调优前自动打点（后台静默，失败仅记日志不阻塞主操作）
pub fn auto_restore_point(desc: &str) {
    if crate::auto::auto_restore_point_enabled() {
        let desc = desc.to_string();
        std::thread::spawn(move || match create_restore_point_sync(&desc) {
            Ok(()) => tracing::info!(desc = %desc, "变更前自动还原点已创建"),
            Err(e) => tracing::warn!(
                desc = %desc,
                error = %e,
                "自动还原点创建失败（系统保护未开启或频率受限）"
            ),
        });
    }
}

// ---------- 还原点存储治理 ----------

#[derive(serde::Serialize, Clone, Debug)]
pub struct ShadowStore {
    pub drive: String,
    pub used_gb: f64,
    pub allocated_gb: f64,
    pub max_gb: f64,
}

/// 各卷还原点存储占用（Win32_ShadowStorage，本地化免疫）
#[tauri::command]
pub async fn list_shadow_storage() -> Vec<ShadowStore> {
    tauri::async_runtime::spawn_blocking(|| {
        let script = r#"
$vols = @{}
Get-CimInstance Win32_Volume -ErrorAction SilentlyContinue | ForEach-Object { $vols[$_.DeviceID] = [string]$_.DriveLetter }
Get-CimInstance Win32_ShadowStorage -ErrorAction SilentlyContinue | ForEach-Object {
  if ($_.Volume -match 'Volume\{(.+?)\}') {
    $dl = $vols["\?\Volume{$($Matches[1])}\"]
    [pscustomobject]@{
      drive = [string]$dl
      used  = [math]::Round($_.UsedSpace / 1GB, 2)
      alloc = [math]::Round($_.AllocatedSpace / 1GB, 2)
      max   = [math]::Round($_.MaxSpace / 1GB, 1)
    }
  }
} | ConvertTo-Json -Compress
"#;
        let Some(raw) = crate::util::run_ps(script) else {
            return Vec::new();
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return Vec::new();
        };
        let arr: Vec<serde_json::Value> = match v {
            serde_json::Value::Array(a) => a,
            obj @ serde_json::Value::Object(_) => vec![obj],
            _ => vec![],
        };
        arr.into_iter()
            .map(|r| ShadowStore {
                drive: r["drive"].as_str().unwrap_or("").into(),
                used_gb: r["used"].as_f64().unwrap_or(0.0),
                allocated_gb: r["alloc"].as_f64().unwrap_or(0.0),
                max_gb: r["max"].as_f64().unwrap_or(0.0),
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 删除指定卷最旧的还原点
#[tauri::command]
pub fn delete_oldest_shadow(drive: String) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("需要管理员权限".into());
    }
    let d = drive.trim().trim_end_matches('\\').to_uppercase();
    let b = d.as_bytes();
    if b.len() != 2 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
        return Err("盘符不合法".into());
    }
    crate::util::run_checked(
        "vssadmin",
        &[
            "delete",
            "shadows",
            &format!("/for={d}"),
            "/oldest",
            "/quiet",
        ],
    )?;
    crate::journal::record(
        &format!("删除 {d} 盘最旧的还原点"),
        crate::journal::Revert::None,
    )?;
    tracing::info!(drive = %d, "最旧还原点已删除");
    Ok(())
}

/// 限制还原点存储配额
#[tauri::command]
pub fn set_shadow_max(drive: String, max_gb: f64) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("需要管理员权限".into());
    }
    let d = drive.trim().trim_end_matches('\\').to_uppercase();
    if d.len() != 2 || !(1.0..=100.0).contains(&max_gb) {
        return Err("参数不合法（盘符 / 配额 1-100GB）".into());
    }
    crate::util::run_checked(
        "vssadmin",
        &[
            "resize",
            "shadowstorage",
            &format!("/for={d}"),
            &format!("/on={d}"),
            &format!("/maxsize={max_gb}GB"),
        ],
    )?;
    crate::journal::record(
        &format!("限制 {d} 盘还原点存储配额 → {max_gb}GB"),
        crate::journal::Revert::None,
    )?;
    tracing::info!(drive = %d, max_gb, "还原点配额已调整");
    Ok(())
}
