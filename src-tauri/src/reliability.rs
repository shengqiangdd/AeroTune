use serde::Serialize;

const MAX_EVENTS: usize = 200;
const ALLOWED_KINDS: [&str; 6] = [
    "app_crash",
    "app_hang",
    "unexpected_shutdown",
    "planned_shutdown",
    "update_installed",
    "update_failed",
];
const EXPECTED_SOURCES: [&str; 3] = ["application", "system", "updates"];

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ReliabilityEvent {
    pub ts: i64,
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub source: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ReliabilitySourceStatus {
    pub id: String,
    pub label: String,
    pub available: bool,
    pub fetched: usize,
    pub limit_reached: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ReliabilitySnapshot {
    pub window_days: u16,
    /// Number of event records collected from readable sources before the response cap.
    pub candidate_count: usize,
    /// At least this many collected records were omitted by the response cap.
    pub omitted_estimate: usize,
    /// Returned rows with unexpected/missing fields that were discarded.
    pub invalid_rows: usize,
    pub events: Vec<ReliabilityEvent>,
    pub sources: Vec<ReliabilitySourceStatus>,
}

fn looks_like_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(3).any(|part| {
        part[0].is_ascii_alphabetic() && part[1] == b':' && matches!(part[2], b'\\' | b'/')
    }) || text.contains("\\\\")
}

fn raw_text(value: &serde_json::Value, max_chars: usize) -> String {
    value
        .as_str()
        .unwrap_or_default()
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_string()
}

fn bounded_text(value: &serde_json::Value, max_chars: usize) -> String {
    if value.as_str().is_some_and(looks_like_path) {
        return "[路径信息已省略]".into();
    }
    raw_text(value, max_chars)
}

fn field_text(data: &serde_json::Value, key: &str, max_chars: usize) -> String {
    bounded_text(&data[key], max_chars)
}

fn path_leaf(value: &str) -> String {
    value
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(value)
        .trim()
        .to_string()
}

fn first_text(data: &serde_json::Value, keys: &[&str], max_chars: usize) -> String {
    keys.iter()
        .map(|key| field_text(data, key, max_chars))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn first_leaf(data: &serde_json::Value, keys: &[&str], max_chars: usize) -> String {
    keys.iter()
        .map(|key| path_leaf(&raw_text(&data[key], max_chars)))
        .find(|value| !value.is_empty())
        .unwrap_or_default()
}

fn map_event(row: &serde_json::Value) -> Option<ReliabilityEvent> {
    let ts = row["ts"].as_i64()?;
    if ts <= 0 {
        return None;
    }
    let event_id = row["event_id"].as_u64()?;
    let source_id = row["source"].as_str()?;
    let data = &row["data"];

    let (kind, title, detail, source) = match (source_id, event_id) {
        ("application", 1000) => {
            let mut app = first_leaf(data, &["AppName", "ApplicationName", "ProcessName"], 256);
            if app.is_empty() {
                app = first_leaf(data, &["AppPath"], 512);
            }
            if app.is_empty() {
                app = "未知应用".into();
            }
            let module = first_leaf(data, &["ModuleName"], 512);
            let exception = first_text(data, &["ExceptionCode"], 48);
            let mut details = Vec::new();
            if !module.is_empty() {
                details.push(format!("故障模块 {module}"));
            }
            if !exception.is_empty() {
                details.push(format!("异常码 {exception}"));
            }
            ("app_crash", app, details.join(" · "), "Application")
        }
        ("application", 1002) => {
            let mut app = first_leaf(data, &["AppName", "ApplicationName", "ProcessName"], 256);
            if app.is_empty() {
                app = first_leaf(data, &["AppPath"], 512);
            }
            if app.is_empty() {
                app = "未知应用".into();
            }
            let hang = first_text(data, &["HangType"], 120);
            let detail = if hang.is_empty() {
                "Windows 记录应用无响应事件".into()
            } else {
                format!("Windows 记录应用挂起：{hang}")
            };
            ("app_hang", app, detail, "Application")
        }
        ("system", 41) => (
            "unexpected_shutdown",
            "系统非正常关机 / 重启".into(),
            "Kernel-Power 事件 41：上次系统会话未正常结束。".into(),
            "System",
        ),
        ("system", 1074) => (
            "planned_shutdown",
            "系统计划关机 / 重启".into(),
            "Windows 记录了计划关机或重启事件（ID 1074）。".into(),
            "System",
        ),
        ("updates", 19) | ("updates", 20) => {
            let description = first_text(data, &["UpdateTitle", "Title"], 240);
            let (kind, title, fallback) = if event_id == 19 {
                (
                    "update_installed",
                    "Windows 更新安装成功",
                    "Windows Update 记录安装成功事件。",
                )
            } else {
                (
                    "update_failed",
                    "Windows 更新安装失败",
                    "Windows Update 记录安装失败事件。",
                )
            };
            let detail = if description.is_empty() {
                fallback.to_string()
            } else {
                description
            };
            (kind, title.into(), detail, "Windows Update")
        }
        _ => return None,
    };

    if !ALLOWED_KINDS.contains(&kind) || title.trim().is_empty() {
        return None;
    }
    Some(ReliabilityEvent {
        ts,
        kind: kind.into(),
        title: bounded_text(&serde_json::Value::String(title), 160),
        detail: bounded_text(&serde_json::Value::String(detail), 240),
        source: source.into(),
    })
}

fn validate_reliability_window(days: u16) -> Result<u16, String> {
    if (1..=90).contains(&days) {
        Ok(days)
    } else {
        Err("可靠性事件查询范围必须为 1–90 天".into())
    }
}

fn parse_source_status(value: &serde_json::Value) -> Option<ReliabilitySourceStatus> {
    let id = bounded_text(&value["id"], 24);
    let label = bounded_text(&value["label"], 48);
    let available = value["available"].as_bool()?;
    let fetched = usize::try_from(value["fetched"].as_u64()?).ok()?;
    let limit_reached = value["limit_reached"].as_bool()?;
    if !EXPECTED_SOURCES.contains(&id.as_str()) || label.is_empty() {
        return None;
    }
    let error = match value.get("error") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(_)) => {
            let text = bounded_text(&value["error"], 120);
            (!text.is_empty()).then_some(text)
        }
        _ => return None,
    };
    Some(ReliabilitySourceStatus {
        id,
        label,
        available,
        fetched,
        limit_reached,
        error,
    })
}

fn object_rows(value: &serde_json::Value, field: &str) -> Result<Vec<serde_json::Value>, String> {
    match value {
        serde_json::Value::Array(rows) => Ok(rows.clone()),
        serde_json::Value::Object(_) => Ok(vec![value.clone()]),
        _ => Err(format!("可靠性事件响应中的 {field} 字段格式异常")),
    }
}

fn parse_reliability_snapshot(
    raw: &str,
    expected_days: u16,
) -> Result<ReliabilitySnapshot, String> {
    let value: serde_json::Value =
        serde_json::from_str(raw).map_err(|error| format!("可靠性事件 JSON 解析失败: {error}"))?;
    let root = value
        .as_object()
        .ok_or_else(|| "可靠性事件响应不是预期对象".to_string())?;
    let window_days = u16::try_from(
        root.get("window_days")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "可靠性事件响应缺少查询范围".to_string())?,
    )
    .map_err(|_| "可靠性事件查询范围无效".to_string())?;
    if window_days != expected_days {
        return Err("可靠性事件响应范围与请求不匹配".into());
    }
    let candidate_count = usize::try_from(
        root.get("candidate_count")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| "可靠性事件响应缺少候选总数".to_string())?,
    )
    .map_err(|_| "可靠性事件数量超出支持范围".to_string())?;
    let script_invalid_rows = usize::try_from(
        root.get("invalid_rows")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
    )
    .unwrap_or(usize::MAX);

    let raw_events = object_rows(
        root.get("events")
            .ok_or_else(|| "可靠性事件响应缺少事件列表".to_string())?,
        "events",
    )?;
    if candidate_count < raw_events.len() {
        return Err("可靠性事件候选数量小于返回事件数量".into());
    }

    let raw_sources = object_rows(
        root.get("sources")
            .ok_or_else(|| "可靠性事件响应缺少来源状态".to_string())?,
        "sources",
    )?;
    let mut sources = raw_sources
        .iter()
        .filter_map(parse_source_status)
        .collect::<Vec<_>>();
    if sources.len() != raw_sources.len()
        || sources.len() != EXPECTED_SOURCES.len()
        || EXPECTED_SOURCES
            .iter()
            .any(|id| !sources.iter().any(|source| source.id == *id))
    {
        return Err("可靠性事件来源状态不完整或格式异常".into());
    }
    sources.sort_by_key(|source| {
        EXPECTED_SOURCES
            .iter()
            .position(|id| *id == source.id)
            .unwrap_or(usize::MAX)
    });

    let mut invalid_rows = script_invalid_rows;
    let mut events = Vec::with_capacity(raw_events.len().min(MAX_EVENTS));
    for row in &raw_events {
        if let Some(event) = map_event(row) {
            events.push(event);
        } else {
            invalid_rows = invalid_rows.saturating_add(1);
        }
    }
    events.sort_by_key(|event| std::cmp::Reverse(event.ts));
    let over_response_cap = events.len().saturating_sub(MAX_EVENTS);
    events.truncate(MAX_EVENTS);
    let omitted_estimate = candidate_count
        .saturating_sub(raw_events.len())
        .saturating_add(over_response_cap);

    Ok(ReliabilitySnapshot {
        window_days,
        candidate_count,
        omitted_estimate,
        invalid_rows,
        events,
        sources,
    })
}

#[tauri::command]
pub async fn get_reliability_events(days: u16) -> Result<ReliabilitySnapshot, String> {
    let days = validate_reliability_window(days)?;
    tauri::async_runtime::spawn_blocking(move || {
        let script = r#"
$days = __DAYS__
$since = (Get-Date).AddDays(-$days)
$out = New-Object System.Collections.ArrayList
$script:invalidRows = 0
function Read-EventData($event) {
  $data = @{}
  $xml = [xml]$event.ToXml()
  foreach ($item in $xml.Event.EventData.Data) {
    if ($item.Name) { $data[[string]$item.Name] = [string]$item.'#text' }
  }
  return $data
}
function Query-Channel([string]$LogName, [int[]]$Ids, [int]$Limit, [string]$Id, [string]$Label) {
  $channelEvents = @()
  $available = $true
  $message = $null
  try {
    $descriptor = Get-WinEvent -ListLog $LogName -ErrorAction Stop
    if (-not $descriptor.IsEnabled) {
      $available = $false
      $message = '事件通道当前已禁用'
    } else {
      $queryErrors = @()
      $channelEvents = @(Get-WinEvent -FilterHashtable @{ LogName=$LogName; Id=$Ids; StartTime=$since } -MaxEvents $Limit -ErrorAction SilentlyContinue -ErrorVariable +queryErrors)
      $queryFailures = @($queryErrors | Where-Object { $_.FullyQualifiedErrorId -notmatch 'NoMatchingEventsFound' })
      if ($queryFailures.Count -gt 0) {
        $available = $false
        $message = '事件通道查询失败或访问受限'
        $channelEvents = @()
      }
    }
  } catch {
    $available = $false
    $message = '事件通道不可访问或不存在'
    $channelEvents = @()
  }
  $status = [pscustomobject]@{
    id = $Id
    label = $Label
    available = [bool]$available
    fetched = [int]$channelEvents.Count
    limit_reached = [bool]($channelEvents.Count -ge $Limit)
    error = $message
  }
  return [pscustomobject]@{ status=$status; events=@($channelEvents) }
}
function Add-RawEvent($event, [string]$source, $data) {
  try {
    $ts = ([DateTimeOffset]$event.TimeCreated).ToUnixTimeSeconds()
    [void]$out.Add([pscustomobject]@{ ts=$ts; event_id=[int]$event.Id; source=$source; data=$data })
  } catch {
    $script:invalidRows++
  }
}
$appQuery = Query-Channel 'Application' @(1000,1002) 200 'application' '应用程序日志'
foreach ($event in $appQuery.events) {
  try {
    $data = Read-EventData $event
    $fields = [pscustomobject]@{
      AppName = $data['AppName']; ApplicationName = $data['ApplicationName']; ProcessName = $data['ProcessName']
      AppPath = $data['AppPath']; ModuleName = $data['ModuleName']; ExceptionCode = $data['ExceptionCode']; HangType = $data['HangType']
    }
    Add-RawEvent $event 'application' $fields
  } catch { $script:invalidRows++ }
}
$systemQuery = Query-Channel 'System' @(41,1074) 200 'system' '系统日志'
foreach ($event in $systemQuery.events) { Add-RawEvent $event 'system' ([pscustomobject]@{}) }
$updateQuery = Query-Channel 'Microsoft-Windows-WindowsUpdateClient/Operational' @(19,20) 200 'updates' 'Windows 更新日志'
foreach ($event in $updateQuery.events) {
  try {
    $data = Read-EventData $event
    $fields = [pscustomobject]@{ UpdateTitle = $data['UpdateTitle']; Title = $data['Title'] }
    Add-RawEvent $event 'updates' $fields
  } catch { $script:invalidRows++ }
}
$sourceStatuses = @($appQuery.status, $systemQuery.status, $updateQuery.status)
$ordered = @($out | Sort-Object -Property ts -Descending)
$candidateCount = $ordered.Count
$selected = @($ordered | Select-Object -First 200)
$payload = [pscustomobject]@{
  window_days = $days
  candidate_count = $candidateCount
  invalid_rows = $script:invalidRows
  sources = @($sourceStatuses)
  events = @($selected)
}
$payload | ConvertTo-Json -Depth 6 -Compress
"#
        .replace("__DAYS__", &days.to_string());
        let raw = crate::util::run_ps_with_timeout_result(&script, std::time::Duration::from_secs(30))?;
        parse_reliability_snapshot(&raw, days)
    })
    .await
    .map_err(|_| "可靠性事件查询任务未能完成".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn source(id: &str, available: bool, fetched: usize, limit_reached: bool) -> Value {
        json!({
            "id": id,
            "label": id,
            "available": available,
            "fetched": fetched,
            "limit_reached": limit_reached,
            "error": if available { Value::Null } else { json!("事件通道不可访问") },
        })
    }

    fn source_statuses() -> Value {
        json!([
            source("application", true, 2, false),
            source("system", true, 1, false),
            source("updates", true, 1, false)
        ])
    }

    fn raw_event(ts: i64, source: &str, event_id: u64, data: Value) -> Value {
        json!({"ts": ts, "source": source, "event_id": event_id, "data": data})
    }

    fn raw_snapshot(events: Value, candidate_count: usize, sources: Value) -> String {
        json!({
            "window_days": 30,
            "candidate_count": candidate_count,
            "invalid_rows": 0,
            "sources": sources,
            "events": events,
        })
        .to_string()
    }

    #[test]
    fn query_window_is_bounded_to_one_through_ninety_days() {
        for days in [1, 7, 30, 90] {
            assert_eq!(validate_reliability_window(days), Ok(days));
        }
        assert!(validate_reliability_window(0).is_err());
        assert!(validate_reliability_window(91).is_err());
    }

    #[test]
    fn maps_supported_windows_event_ids_using_only_allowlisted_fields() {
        let cases = [
            raw_event(
                1700000001,
                "application",
                1000,
                json!({
                    "AppName": "C:\\Program Files\\Editor\\editor.exe",
                    "ModuleName": "C:\\Windows\\System32\\fault.dll",
                    "ExceptionCode": "0xc0000005"
                }),
            ),
            raw_event(
                1700000002,
                "application",
                1002,
                json!({"AppName":"editor.exe","HangType":"Cross-thread"}),
            ),
            raw_event(1700000003, "system", 41, json!({})),
            raw_event(1700000004, "system", 1074, json!({})),
            raw_event(
                1700000005,
                "updates",
                19,
                json!({"UpdateTitle":"Feature update"}),
            ),
            raw_event(
                1700000006,
                "updates",
                20,
                json!({"param1":"C:\\private\\path","Title":""}),
            ),
        ];
        let mapped = cases.iter().filter_map(map_event).collect::<Vec<_>>();
        assert_eq!(mapped.len(), 6);
        assert_eq!(mapped[0].title, "editor.exe");
        assert!(mapped[0].detail.contains("fault.dll"));
        assert!(!mapped[0].detail.contains("C:\\Windows"));
        assert_eq!(mapped[2].kind, "unexpected_shutdown");
        assert_eq!(mapped[3].kind, "planned_shutdown");
        assert_eq!(mapped[4].kind, "update_installed");
        assert_eq!(mapped[5].detail, "Windows Update 记录安装失败事件。");
        assert!(map_event(&raw_event(
            1700000007,
            "updates",
            20,
            json!({"param1":"private"})
        ))
        .is_some());
        assert_eq!(
            map_event(&raw_event(
                1700000008,
                "updates",
                20,
                json!({"UpdateTitle":"C:\\private\\path"})
            ))
            .unwrap()
            .detail,
            "[路径信息已省略]"
        );
    }

    #[test]
    fn snapshot_reports_partial_sources_and_single_event_objects() {
        let sources = json!([
            source("application", true, 0, false),
            source("system", false, 0, false),
            source("updates", true, 1, false)
        ]);
        let event = raw_event(
            1700000001,
            "updates",
            19,
            json!({"UpdateTitle":"Quality update"}),
        );
        let raw = raw_snapshot(event, 1, sources);
        let snapshot = parse_reliability_snapshot(&raw, 30).unwrap();
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.events[0].title, "Windows 更新安装成功");
        assert!(!snapshot.sources[1].available);
        assert_eq!(snapshot.sources[1].id, "system");
    }

    #[test]
    fn parser_rejects_invalid_top_level_and_missing_source_status() {
        assert!(parse_reliability_snapshot("null", 30).is_err());
        assert!(parse_reliability_snapshot("[]", 30).is_err());
        let raw = raw_snapshot(json!([]), 0, json!([]));
        assert!(parse_reliability_snapshot(&raw, 30).is_err());
        let mismatch = raw_snapshot(json!([]), 0, source_statuses());
        assert!(parse_reliability_snapshot(&mismatch, 7).is_err());
    }

    #[test]
    #[ignore = "live read-only check of the Windows Event Log query"]
    fn live_windows_query_returns_a_bounded_typed_snapshot() {
        let snapshot = tauri::async_runtime::block_on(get_reliability_events(7))
            .expect("PowerShell should return a valid reliability snapshot");
        assert_eq!(snapshot.window_days, 7);
        assert_eq!(snapshot.sources.len(), EXPECTED_SOURCES.len());
        assert!(snapshot.events.len() <= MAX_EVENTS);
        assert!(snapshot.candidate_count >= snapshot.events.len());
    }

    #[test]
    fn snapshot_counts_invalid_rows_and_capped_records() {
        let events = (1..=205)
            .map(|ts| raw_event(ts, "application", 1002, json!({"AppName":"app.exe"})))
            .chain(std::iter::once(
                json!({"ts":0,"source":"application","event_id":1000,"data":{}}),
            ))
            .collect::<Vec<_>>();
        let raw = raw_snapshot(json!(events), 206, source_statuses());
        let snapshot = parse_reliability_snapshot(&raw, 30).unwrap();
        assert_eq!(snapshot.events.len(), MAX_EVENTS);
        assert_eq!(snapshot.events[0].ts, 205);
        assert_eq!(snapshot.invalid_rows, 1);
        assert_eq!(snapshot.omitted_estimate, 5);
    }
}
