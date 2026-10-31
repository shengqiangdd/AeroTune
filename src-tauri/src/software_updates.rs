use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const QUERY_TIMEOUT: Duration = Duration::from_secs(90);
const MAX_DISPLAY_BYTES: usize = 512 * 1024;
const MAX_ERROR_BYTES: usize = 8 * 1024;
static QUERY_RUNNING: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
pub struct WingetQueryResult {
    pub output: String,
    pub checked_at_unix: u64,
    pub duration_ms: u64,
}

struct QueryGuard;

impl Drop for QueryGuard {
    fn drop(&mut self) {
        QUERY_RUNNING.store(false, Ordering::Release);
    }
}

fn bound_error_message(message: String) -> String {
    let mut clean = String::with_capacity(message.len().min(MAX_ERROR_BYTES));
    let mut truncated = false;
    for character in message.chars() {
        if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            continue;
        }
        if clean.len() + character.len_utf8() > MAX_ERROR_BYTES {
            truncated = true;
            break;
        }
        clean.push(character);
    }
    if truncated {
        clean.push_str("\n[错误输出已截断；查询仍按失败处理]");
    }
    if clean.trim().is_empty() {
        "WinGet 返回未知错误；未能确认更新状态".into()
    } else {
        clean
    }
}

fn prepare_display_output(output: String) -> Result<String, String> {
    let mut clean = String::with_capacity(output.len().min(MAX_DISPLAY_BYTES));
    for character in output.chars() {
        if character.is_control() && !matches!(character, '\n' | '\r' | '\t') {
            continue;
        }
        if clean.len() + character.len_utf8() > MAX_DISPLAY_BYTES {
            return Err("WinGet 输出超过 512 KiB 显示上限；不能确认查询是否完整".into());
        }
        clean.push(character);
    }
    if clean.trim().is_empty() {
        return Err("WinGet 查询成功退出但没有可读取的结果；不能据此判断是否有更新".into());
    }
    Ok(clean)
}

#[tauri::command]
pub fn check_winget_updates() -> Result<WingetQueryResult, String> {
    QUERY_RUNNING
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "WinGet 查询已在进行中".to_string())?;
    let _guard = QueryGuard;
    let started = Instant::now();

    // Deliberately do not pass --accept-source-agreements. Non-interactive execution
    // must fail if configured sources require consent; it must never silently consent.
    let output = crate::util::run_output_checked_with_timeout(
        "winget",
        &["list", "--upgrade-available", "--disable-interactivity"],
        QUERY_TIMEOUT,
    )
    .map_err(|error| {
        format!(
            "WinGet 查询失败；未能确认更新状态：{}",
            bound_error_message(error)
        )
    })?;
    let output = prepare_display_output(output)?;
    let checked_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "系统时间早于 Unix Epoch，无法为查询结果标注时间".to_string())?
        .as_secs();

    Ok(WingetQueryResult {
        output,
        checked_at_unix,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_or_control_only_output_is_not_a_zero_updates_result() {
        assert!(prepare_display_output(String::new()).is_err());
        assert!(prepare_display_output("\u{1b}\u{7}\u{1}".into()).is_err());
    }

    #[test]
    fn output_is_display_safe_but_keeps_tabs_and_line_breaks() {
        let output =
            prepare_display_output("Package\tVersion\r\nA\u{1b}[31mB\u{7}".into()).unwrap();
        assert_eq!(output, "Package\tVersion\r\nA[31mB");
    }

    #[test]
    fn failed_command_details_are_sanitized_and_bounded() {
        let message = bound_error_message(format!("{}\u{7}", "x".repeat(MAX_ERROR_BYTES + 16)));
        assert!(message.len() <= MAX_ERROR_BYTES + 64);
        assert!(message.contains("查询仍按失败处理"));
        assert!(!message.contains('\u{7}'));
    }

    #[test]
    fn output_limit_fails_closed_instead_of_truncating_results() {
        let output = "x".repeat(MAX_DISPLAY_BYTES + 1);
        let error = prepare_display_output(output).unwrap_err();
        assert!(error.contains("不能确认查询是否完整"));
    }
}
