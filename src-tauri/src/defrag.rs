// 磁盘优化：碎片整理（HDD）/ TRIM（SSD）—— defrag /O 按介质自动选择正确动作
// 定期执行复用系统内置计划任务 \\Microsoft\\Windows\\Defrag\\ScheduledDefrag
use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default)]
pub struct VolumeOpt {
    pub drive: String,
    /// HDD / SSD / Unspecified
    pub media: String,
    /// 碎片化百分比（按需分析后填充）
    pub fragmentation: Option<u8>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DefragStatus {
    pub volumes: Vec<VolumeOpt>,
    pub task_enabled: bool,
    pub last_run: String,
    pub next_run: String,
}

fn normalize_drive(drive: &str) -> Result<String, String> {
    let d = drive.trim().to_ascii_uppercase();
    let b = d.as_bytes();
    if matches!(b.len(), 1..=3)
        && b[0].is_ascii_alphabetic()
        && (b.len() == 1 || b[1] == b':')
        && (b.len() < 3 || b[2] == b'\\')
    {
        Ok(d[..1].to_string())
    } else {
        Err("盘符不合法".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drive_validation_rejects_paths_and_repeated_separators() {
        for input in ["d", "D:", " d:\\ "] {
            assert_eq!(normalize_drive(input).unwrap(), "D");
        }
        for input in ["", "D::", "D\\", "D:\\folder", "é", "D:;whoami"] {
            assert!(normalize_drive(input).is_err(), "{input}");
        }
    }

    #[test]
    fn status_parser_preserves_task_state_and_normalizes_drive_letters() {
        let status = parse_defrag_status(
            r#"{"volumes":[{"drive":"c","media":"SSD"},{"drive":"D:","media":"HDD"}],"task_enabled":true,"last_run":"today","next_run":"tomorrow"}"#,
        ).unwrap();
        assert_eq!(status.volumes.len(), 2);
        assert_eq!(status.volumes[0].drive, "C:\\");
        assert_eq!(status.volumes[0].media, "SSD");
        assert_eq!(status.volumes[1].drive, "D:\\");
        assert!(status.task_enabled);
        assert_eq!(status.last_run, "today");
        assert_eq!(status.next_run, "tomorrow");
    }

    #[test]
    fn status_parser_rejects_invalid_json_and_missing_volume_array() {
        assert!(parse_defrag_status("not json").is_err());
        assert!(parse_defrag_status(r#"{"task_enabled":false}"#).is_err());
    }
}

const DEFRAG_STATUS_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$mediaByDisk = @{}
Get-PhysicalDisk | ForEach-Object { $mediaByDisk[[string]$_.DeviceId] = [string]$_.MediaType }
$mediaByDrive = @{}
Get-Partition | Where-Object DriveLetter | ForEach-Object { $mediaByDrive[[string]$_.DriveLetter] = $mediaByDisk[[string]$_.DiskNumber] }
$volumes = @(Get-Volume | Where-Object DriveLetter | ForEach-Object {
  $drive = [string]$_.DriveLetter
  $media = [string]$mediaByDrive[$drive]
  if ([string]::IsNullOrWhiteSpace($media)) { $media = 'Unspecified' }
  [pscustomobject]@{ drive = $drive; media = $media }
})
$t = Get-ScheduledTask -TaskPath '\Microsoft\Windows\Defrag\' -TaskName 'ScheduledDefrag' -ErrorAction SilentlyContinue
$i = if ($t) { $t | Get-ScheduledTaskInfo -ErrorAction SilentlyContinue } else { $null }
$enabled = $false
if ($t -and ([string]$t.State -ne 'Disabled')) { $enabled = $true }
[pscustomobject]@{
  volumes = $volumes
  task_enabled = $enabled
  last_run = if ($i) { [string]$i.LastRunTime } else { '' }
  next_run = if ($i) { [string]$i.NextRunTime } else { '' }
} | ConvertTo-Json -Depth 4 -Compress
"#;

fn parse_defrag_status(raw: &str) -> Result<DefragStatus, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("解析磁盘状态失败：{error}"))?;
    let rows = value
        .get("volumes")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "磁盘状态格式无效".to_string())?;
    let volumes = rows
        .iter()
        .filter_map(|row| {
            let drive = row.get("drive").and_then(serde_json::Value::as_str)?;
            let drive = normalize_drive(drive).ok()?;
            let media = row
                .get("media")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty())
                .unwrap_or("Unspecified")
                .to_string();
            Some(VolumeOpt {
                drive: format!("{drive}:\\"),
                media,
                fragmentation: None,
            })
        })
        .collect();
    Ok(DefragStatus {
        volumes,
        task_enabled: value
            .get("task_enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        last_run: value
            .get("last_run")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
        next_run: value
            .get("next_run")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string(),
    })
}

#[tauri::command]
pub async fn defrag_status() -> Result<DefragStatus, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let raw = crate::util::run_ps_with_timeout_result(
            DEFRAG_STATUS_SCRIPT,
            std::time::Duration::from_secs(15),
        )?;
        parse_defrag_status(&raw)
    })
    .await
    .map_err(|error| format!("磁盘状态查询线程失败：{error}"))?
}

/// 立即优化：HDD 走碎片整理，SSD 自动 TRIM（defrag /O 按介质自行判定）
#[tauri::command]
pub async fn defrag_optimize(drive: String) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("磁盘优化需要管理员权限".into());
    }
    let d = normalize_drive(&drive)?;
    if d == "C" {
        return Err("系统盘优化请使用「系统维护」的内置计划（避免与系统任务冲突）".into());
    }
    let drive_arg = format!("{d}:");
    tauri::async_runtime::spawn_blocking(move || {
        tracing::info!(drive = %drive_arg, "磁盘优化开始（defrag /O）");
        crate::util::run_checked_with_timeout(
            "defrag.exe",
            &[&drive_arg, "/O", "/U", "/V"],
            std::time::Duration::from_secs(2 * 60 * 60),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    crate::journal::record(
        &format!("磁盘优化完成（{d}: /O，HDD 碎片整理 / SSD TRIM）"),
        crate::journal::Revert::None,
    )?;
    tracing::info!(drive = %d, "磁盘优化完成");
    Ok(())
}

/// 碎片分析：返回碎片化百分比（SSD 通常返回“不需要”→ None）
#[tauri::command]
pub async fn defrag_analyze(drive: String) -> Result<Option<u8>, String> {
    let d = normalize_drive(&drive)?;
    let drive_arg = format!("{d}:");
    tauri::async_runtime::spawn_blocking(move || {
        let out = crate::util::run_output_checked_with_timeout(
            "defrag.exe",
            &[&drive_arg, "/A", "/U", "/V"],
            std::time::Duration::from_secs(10 * 60),
        )?;
        for line in out.lines() {
            let t = line.trim();
            let lower = t.to_lowercase();
            if (lower.contains("总碎片")
                || lower.contains("total fragmentation")
                || lower.contains("碎片化"))
                && t.contains('%')
            {
                // 提取百分号前的数字
                if let Some(pct) = t.split('%').next().and_then(|s| {
                    s.split(|c: char| !c.is_ascii_digit() && c != '.')
                        .next_back()
                        .and_then(|n| n.parse::<u8>().ok())
                }) {
                    return Ok(Some(pct.min(100)));
                }
            }
        }
        Ok(None)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 配置内置计划任务：禁用走原生 schtasks，避免 PowerShell 模块启动延迟
#[tauri::command]
pub async fn set_defrag_schedule(enable: bool, day: String, time: String) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("修改优化计划需要管理员权限".into());
    }
    if enable {
        let allowed_days = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        if !allowed_days.contains(&day.as_str()) {
            return Err("星期参数不合法".into());
        }
        if !regex::Regex::new(r"^([01]\d|2[0-3]):[0-5]\d$")
            .unwrap()
            .is_match(&time)
        {
            return Err("时间参数不合法（HH:mm）".into());
        }
    }

    let scheduled_day = day.clone();
    let scheduled_time = time.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if enable {
            let script = format!(r#"$ErrorActionPreference = 'Stop'; $t = Get-ScheduledTask -TaskPath '\Microsoft\Windows\Defrag\' -TaskName 'ScheduledDefrag'; $t.Triggers[0] = New-ScheduledTaskTrigger -Weekly -DaysOfWeek {scheduled_day} -At '{scheduled_time}'; Set-ScheduledTask -InputObject $t | Out-Null; Enable-ScheduledTask -TaskPath '\Microsoft\Windows\Defrag\' -TaskName 'ScheduledDefrag' | Out-Null"#);
            crate::util::run_checked_with_timeout(
                "powershell",
                &["-NoProfile", "-NonInteractive", "-Command", &script],
                std::time::Duration::from_secs(15),
            )
        } else {
            crate::util::run_checked_with_timeout(
                "schtasks.exe",
                &["/Change", "/TN", r"\Microsoft\Windows\Defrag\ScheduledDefrag", "/Disable"],
                std::time::Duration::from_secs(10),
            )
        }
    })
    .await
    .map_err(|error| format!("计划任务工作线程失败：{error}"))??;

    crate::journal::record(
        &format!(
            "磁盘优化计划 {}（系统内置 Defrag 任务）",
            if enable {
                format!("→ 每周{day} {time}")
            } else {
                "关闭".into()
            }
        ),
        crate::journal::Revert::None,
    )?;
    tracing::info!(enable, day = %day, time = %time, "磁盘优化计划已配置");
    Ok(())
}
