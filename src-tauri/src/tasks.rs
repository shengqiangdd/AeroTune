// 计划任务：第三方任务枚举与启停
// Task Scheduler COM 触发器类型使用固定枚举值，避免依赖本地化 PowerShell 类名。
use serde::Serialize;

#[derive(Serialize, Clone, Debug)]
pub struct TaskInfo {
    pub path: String,
    pub name: String,
    pub state: String,
    pub author: String,
    pub logon: bool,
    pub boot: bool,
    pub action: String,
    pub third_party: bool,
}

fn parse_task_list(raw: &str) -> Result<Vec<TaskInfo>, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("解析计划任务列表失败：{error}"))?;
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        object @ serde_json::Value::Object(_) => vec![object],
        _ => return Err("计划任务列表格式无效".into()),
    };
    let mut tasks = Vec::with_capacity(rows.len());
    for row in &rows {
        for field in ["path", "name", "state"] {
            if row
                .get(field)
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
            {
                return Err(format!("计划任务字段缺失或为空：{field}"));
            }
        }
        let task = map_task(row);
        if task.third_party {
            tasks.push(task);
        }
    }
    Ok(tasks)
}

const TASK_ENUMERATION_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$service = New-Object -ComObject 'Schedule.Service'
$service.Connect()
$script:taskRows = New-Object 'System.Collections.Generic.List[object]'
function Visit-TaskFolder($folder) {
  $folderPath = [string]$folder.Path
  if ($folderPath -like '\Microsoft*') { return }
  $tasks = $folder.GetTasks(1)
  for ($taskIndex = 1; $taskIndex -le $tasks.Count; $taskIndex++) {
    $task = $tasks.Item($taskIndex)
    $definition = $task.Definition
    $hasLogon = $false
    $hasBoot = $false
    for ($triggerIndex = 1; $triggerIndex -le $definition.Triggers.Count; $triggerIndex++) {
      $triggerType = [int]$definition.Triggers.Item($triggerIndex).Type
      if ($triggerType -eq 9) { $hasLogon = $true }
      if ($triggerType -eq 8) { $hasBoot = $true }
    }
    $actionPath = ''
    if ($definition.Actions.Count -gt 0) {
      $action = $definition.Actions.Item(1)
      if ([int]$action.Type -eq 0) { $actionPath = [string]$action.Path }
    }
    $state = switch ([int]$task.State) {
      1 { 'Disabled' }
      2 { 'Queued' }
      3 { 'Ready' }
      4 { 'Running' }
      default { 'Unknown' }
    }
    $script:taskRows.Add([pscustomobject]@{
      path = $folderPath
      name = [string]$task.Name
      state = [string]$state
      author = [string]$definition.RegistrationInfo.Author
      logon = [bool]$hasLogon
      boot = [bool]$hasBoot
      action = $actionPath
    })
  }
  $subfolders = $folder.GetFolders(0)
  for ($folderIndex = 1; $folderIndex -le $subfolders.Count; $folderIndex++) {
    Visit-TaskFolder $subfolders.Item($folderIndex)
  }
}
Visit-TaskFolder $service.GetFolder('\')
ConvertTo-Json -InputObject @($script:taskRows.ToArray()) -Depth 3 -Compress
"#;

fn map_task(v: &serde_json::Value) -> TaskInfo {
    let path = v["path"].as_str().unwrap_or("").to_string();
    let lower = path.to_lowercase();
    TaskInfo {
        path,
        name: v["name"].as_str().unwrap_or("").to_string(),
        state: v["state"].as_str().unwrap_or("").to_string(),
        author: v["author"].as_str().unwrap_or("").to_string(),
        logon: v["logon"].as_bool().unwrap_or(false),
        boot: v["boot"].as_bool().unwrap_or(false),
        action: v["action"].as_str().unwrap_or("").to_string(),
        third_party: !lower.starts_with("\\microsoft"),
    }
}

/// 非微软计划任务（含已禁用，便于恢复）；COM 枚举避免加载 ScheduledTasks PowerShell 模块
#[tauri::command]
pub async fn list_logon_tasks() -> Result<Vec<TaskInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let raw = crate::util::run_ps_with_timeout_result(
            TASK_ENUMERATION_SCRIPT,
            std::time::Duration::from_secs(20),
        )?;
        parse_task_list(&raw)
    })
    .await
    .map_err(|error| format!("计划任务枚举线程失败：{error}"))?
}

fn valid_task_part(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 260
        && !s.contains('\'')
        && !s.contains('"')
        && !s.chars().any(char::is_control)
}

fn valid_task_target(path: &str, name: &str) -> bool {
    if !valid_task_part(path)
        || !valid_task_part(name)
        || !path.starts_with('\\')
        || !path.ends_with('\\')
        || path.contains('/')
        || name.contains('\\')
        || name.contains('/')
        || matches!(name, "." | "..")
        || path.to_ascii_lowercase().starts_with("\\microsoft")
    {
        return false;
    }
    if path.len() > 1 {
        let interior = &path[1..path.len() - 1];
        if interior
            .split('\\')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
        {
            return false;
        }
    }
    true
}

/// 启用/禁用计划任务（原生 schtasks，避免 PowerShell 模块启动延迟）
#[tauri::command]
pub async fn set_task_state(
    task_path: String,
    task_name: String,
    enable: bool,
) -> Result<(), String> {
    if !valid_task_target(&task_path, &task_name) {
        return Err("只允许修改格式有效的非 Microsoft 计划任务".into());
    }
    let full_name = format!("{task_path}{task_name}");
    let operation = if enable { "/Enable" } else { "/Disable" };
    tauri::async_runtime::spawn_blocking(move || {
        crate::util::run_checked_with_timeout(
            "schtasks.exe",
            &["/Change", "/TN", &full_name, operation],
            std::time::Duration::from_secs(10),
        )
    })
    .await
    .map_err(|error| format!("计划任务工作线程失败：{error}"))??;
    crate::journal::record(
        &format!(
            "{}计划任务 {}{}（可在「开机全景」恢复）",
            if enable { "启用" } else { "禁用" },
            task_path,
            task_name
        ),
        crate::journal::Revert::None,
    )?;
    tracing::info!(path = %task_path, name = %task_name, enable, "计划任务状态已修改");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_mapping() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"path":"\\NvTmRep\\","name":"NvTmRep_1","state":"Ready","author":"NVIDIA",
                "logon":true,"boot":false,"action":"C:\\Program Files\\NVIDIA\\nvtegra\\nvcontainer.exe"}"#,
        )
        .unwrap();
        let t = map_task(&v);
        assert!(t.third_party);
        assert!(t.logon);
        assert!(!t.boot);
        assert_eq!(t.state, "Ready");
        assert!(t.action.contains("nvcontainer"));
    }

    #[test]
    fn microsoft_tasks_excluded_at_filter_level() {
        let mut v = serde_json::json!({"path":"\\Microsoft\\Windows\\Update\\x","name":"x","state":"Ready","author":"","logon":true,"boot":false,"action":"a"});
        v["path"] = serde_json::json!("\\Microsoft\\Windows\\Update\\x");
        let t = map_task(&v);
        assert!(!t.third_party);
        v["path"] = serde_json::json!("\\MicrosoftVendor\\");
        assert!(!map_task(&v).third_party);
    }

    #[test]
    fn task_state_rejects_microsoft_and_malformed_scheduler_paths() {
        for (path, name) in [
            ("\\Microsoft\\Windows\\Update\\", "Orchestrator"),
            ("\\mIcRoSoFtVendor\\", "Task"),
            ("\\Vendor\\..\\Microsoft\\", "Task"),
            ("\\Vendor\\\\Nested\\", "Task"),
            ("Vendor\\", "Task"),
            ("\\Vendor", "Task"),
            ("\\Vendor\\", "nested\\Task"),
            ("\\Vendor\\", ".."),
            ("\\Vendor/Other\\", "Task"),
        ] {
            assert!(
                !valid_task_target(path, name),
                "accepted task target {path:?} / {name:?}"
            );
        }
        assert!(valid_task_target("\\Vendor\\", "BackupTask"));
        assert!(valid_task_target("\\", "RootThirdPartyTask"));
    }

    #[test]
    fn task_list_parser_distinguishes_empty_from_invalid_payload() {
        assert!(parse_task_list("[]").unwrap().is_empty());
        assert!(parse_task_list("not json").is_err());
        assert!(parse_task_list(r#"{"unexpected":true}"#).is_err());
    }

    #[test]
    fn validation() {
        assert!(valid_task_part("\\NvTmRep\\"));
        assert!(!valid_task_part("has'quote"));
        assert!(!valid_task_part(""));
    }
}
