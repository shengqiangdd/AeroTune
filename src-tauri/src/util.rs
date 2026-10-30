use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 解码控制台输出：JSON cmdlet 用 UTF-8，原生旧版工具仍可能输出 UTF-16LE/系统 ANSI。
/// UTF-8 先做严格验证；无效时再识别旧编码，避免把 PowerShell 管道 JSON 当作 GBK 损坏。
fn decode_process_output(b: &[u8]) -> String {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    if std::str::from_utf8(b).is_ok() {
        return String::from_utf8_lossy(b).into_owned();
    }
    decode_output(b)
}

/// 解码旧版控制台输出：powercfg 输出 UTF-16LE，Windows ANSI 输出系统代码页。
fn decode_output(b: &[u8]) -> String {
    // UTF-16LE BOM
    if b.starts_with(&[0xFF, 0xFE]) {
        return utf16_decode(&b[2..]);
    }
    // 启发式：奇数位大量 NUL → UTF-16LE（powercfg 全家）
    if b.len() >= 8 {
        let nulls = b
            .iter()
            .skip(1)
            .step_by(2)
            .take(b.len() / 2)
            .filter(|&&c| c == 0)
            .count();
        if nulls * 4 > b.len() {
            return utf16_decode(b);
        }
    }
    ansi_decode(b)
}

fn utf16_decode(b: &[u8]) -> String {
    let u: Vec<u16> = b
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u)
}

/// 用系统 ANSI 代码页（中文系统 = GBK/936）精确解码
fn ansi_decode(b: &[u8]) -> String {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    if b.is_empty() {
        return String::new();
    }
    unsafe {
        let need = MultiByteToWideChar(0, 0, b.as_ptr(), b.len() as i32, std::ptr::null_mut(), 0);
        if need <= 0 {
            return String::from_utf8_lossy(b).into_owned();
        }
        let mut buf = vec![0u16; need as usize];
        MultiByteToWideChar(0, 0, b.as_ptr(), b.len() as i32, buf.as_mut_ptr(), need);
        String::from_utf16_lossy(&buf)
    }
}

const MAX_COMMAND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const PIPE_CLOSE_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug)]
struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: CapturedStream,
    stderr: CapturedStream,
}

struct ChildGuard {
    child: Child,
    active: bool,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn drain_pipe(mut reader: impl Read) -> std::io::Result<CapturedStream> {
    let mut bytes = Vec::with_capacity(8192);
    let mut buffer = [0u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let remaining = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(bytes.len());
        let keep = remaining.min(count);
        bytes.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
    Ok(CapturedStream { bytes, truncated })
}

fn join_reader(
    reader: JoinHandle<std::io::Result<CapturedStream>>,
    grace: Duration,
) -> Result<CapturedStream, String> {
    let deadline = Instant::now() + grace;
    while !reader.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    if !reader.is_finished() {
        // Dropping JoinHandle detaches a reader if a descendant retained the pipe.
        return Err("子进程输出管道未按时关闭".into());
    }
    reader
        .join()
        .map_err(|_| "子进程输出读取线程异常退出".to_string())?
        .map_err(|error| error.to_string())
}

pub fn windows_directory() -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;

    let mut buffer = vec![0u16; 32_768];
    let length = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("无法解析 Windows 目录".into());
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

/// Resolve protected per-machine install roots without trusting inherited environment variables.
pub fn program_files_directories() -> Result<Vec<PathBuf>, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, SHGetKnownFolderPath},
    };

    let resolve = |folder_id, label: &str| -> Result<PathBuf, String> {
        let mut raw: *mut u16 = std::ptr::null_mut();
        let result = unsafe { SHGetKnownFolderPath(folder_id, 0, std::ptr::null_mut(), &mut raw) };
        if result < 0 || raw.is_null() {
            if !raw.is_null() {
                unsafe { CoTaskMemFree(raw.cast()) };
            }
            return Err(format!("无法解析 {label} 已知文件夹"));
        }
        let mut length = 0usize;
        unsafe {
            while length < 32_768 && *raw.add(length) != 0 {
                length += 1;
            }
        }
        if length == 32_768 {
            unsafe { CoTaskMemFree(raw.cast()) };
            return Err(format!("{label} 已知文件夹路径无效"));
        }
        let path =
            unsafe { PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(raw, length))) };
        unsafe { CoTaskMemFree(raw.cast()) };
        if !path.is_absolute() {
            return Err(format!("{label} 已知文件夹不是绝对路径"));
        }
        Ok(path)
    };

    let primary = resolve(&FOLDERID_ProgramFiles, "Program Files")?;
    let mut directories = vec![primary];
    if let Ok(x86) = resolve(&FOLDERID_ProgramFilesX86, "Program Files (x86)") {
        directories.push(x86);
    }
    Ok(directories)
}

fn windows_system_directory() -> Result<PathBuf, String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

    let mut buffer = vec![0u16; 32_768];
    let length = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if length == 0 || length >= buffer.len() {
        return Err("无法解析 Windows 系统程序目录".into());
    }
    Ok(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

fn verify_system_executable(path: PathBuf) -> Result<PathBuf, String> {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;

    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| format!("Windows 系统程序不可用: {}", path.display()))?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(format!(
            "拒绝启动无效或重解析的系统程序: {}",
            path.display()
        ));
    }
    Ok(path)
}

fn resolve_executable(program: &str) -> Result<PathBuf, String> {
    let name = program.to_ascii_lowercase();
    let system_directory = match name.as_str() {
        "powershell" | "powershell.exe" => {
            return verify_system_executable(
                windows_system_directory()?
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join("powershell.exe"),
            );
        }
        "takeown" | "takeown.exe" | "icacls" | "icacls.exe" | "cmd" | "cmd.exe" | "powercfg"
        | "powercfg.exe" | "ipconfig" | "ipconfig.exe" | "defrag" | "defrag.exe" | "schtasks"
        | "schtasks.exe" | "dism" | "dism.exe" | "compact" | "compact.exe" | "vssadmin"
        | "vssadmin.exe" => windows_system_directory()?,
        _ => return Err(format!("拒绝通过 PATH 启动未审核的命令: {program}")),
    };
    let executable = match name.as_str() {
        "takeown" | "takeown.exe" => "takeown.exe",
        "icacls" | "icacls.exe" => "icacls.exe",
        "cmd" | "cmd.exe" => "cmd.exe",
        "powercfg" | "powercfg.exe" => "powercfg.exe",
        "ipconfig" | "ipconfig.exe" => "ipconfig.exe",
        "defrag" | "defrag.exe" => "defrag.exe",
        "schtasks" | "schtasks.exe" => "schtasks.exe",
        "dism" | "dism.exe" => "dism.exe",
        "compact" | "compact.exe" => "compact.exe",
        "vssadmin" | "vssadmin.exe" => "vssadmin.exe",
        _ => unreachable!("only allowlisted system tools reach this branch"),
    };
    verify_system_executable(system_directory.join(executable))
}

fn default_command_timeout(program: &str) -> Duration {
    let name = PathBuf::from(program)
        .file_name()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match name.as_str() {
        "powercfg" | "powercfg.exe" => Duration::from_secs(3),
        "powershell" | "powershell.exe" => Duration::from_secs(15),
        _ => Duration::from_secs(30),
    }
}

fn command_output_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<CapturedOutput, String> {
    let child = Command::new(resolve_executable(program)?)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法启动 {program}: {error}"))?;
    let mut child = ChildGuard {
        child,
        active: true,
    };
    let stdout = child
        .child
        .stdout
        .take()
        .ok_or_else(|| format!("{program} stdout 管道不可用"))?;
    let stderr = child
        .child
        .stderr
        .take()
        .ok_or_else(|| format!("{program} stderr 管道不可用"))?;

    let stdout_reader = match thread::Builder::new()
        .name("aerotune-command-stdout".into())
        .spawn(move || drain_pipe(stdout))
    {
        Ok(reader) => reader,
        Err(error) => return Err(format!("启动 {program} stdout 读取线程失败: {error}")),
    };
    let stderr_reader = match thread::Builder::new()
        .name("aerotune-command-stderr".into())
        .spawn(move || drain_pipe(stderr))
    {
        Ok(reader) => reader,
        Err(error) => {
            let _ = join_reader(stdout_reader, PIPE_CLOSE_GRACE);
            return Err(format!("启动 {program} stderr 读取线程失败: {error}"));
        }
    };

    let started = Instant::now();
    let status = loop {
        match child.child.try_wait() {
            Ok(Some(status)) => {
                child.active = false;
                break status;
            }
            Ok(None) if started.elapsed() < timeout => {
                let remaining = timeout.saturating_sub(started.elapsed());
                thread::sleep(Duration::from_millis(10).min(remaining));
            }
            Ok(None) => {
                let _ = child.child.kill();
                let reap = child.child.wait();
                child.active = reap.is_err();
                let _ = join_reader(stdout_reader, PIPE_CLOSE_GRACE);
                let _ = join_reader(stderr_reader, PIPE_CLOSE_GRACE);
                if let Err(error) = reap {
                    return Err(format!("{program} 超时后回收子进程失败: {error}"));
                }
                return Err(format!("{program} 执行超时（{} ms）", timeout.as_millis()));
            }
            Err(error) => {
                let _ = child.child.kill();
                let reap = child.child.wait();
                child.active = reap.is_err();
                let _ = join_reader(stdout_reader, PIPE_CLOSE_GRACE);
                let _ = join_reader(stderr_reader, PIPE_CLOSE_GRACE);
                return Err(format!("等待 {program} 进程状态失败: {error}"));
            }
        }
    };

    let stdout = join_reader(stdout_reader, PIPE_CLOSE_GRACE)
        .map_err(|error| format!("读取 {program} stdout 失败: {error}"))?;
    let stderr = join_reader(stderr_reader, PIPE_CLOSE_GRACE)
        .map_err(|error| format!("读取 {program} stderr 失败: {error}"))?;
    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
    })
}

/// 运行外部命令，返回解码后的 stdout。失败或超过输出上限返回 None。
#[track_caller]
pub fn run(program: &str, args: &[&str]) -> Option<String> {
    run_with_timeout(program, args, default_command_timeout(program))
}

#[track_caller]
pub fn run_with_timeout(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let output = match command_output_with_timeout(program, args, timeout) {
        Ok(output) => output,
        Err(error) => {
            if error.contains("执行超时") {
                tracing::warn!(program, timeout_ms = timeout.as_millis(), caller = %std::panic::Location::caller(), %error, "外部命令超时");
            }
            return None;
        }
    };
    if output.stdout.truncated {
        return None;
    }
    let result = decode_process_output(&output.stdout.bytes)
        .trim()
        .to_string();
    (!result.is_empty()).then_some(result)
}

/// 运行外部命令并校验退出码，返回解码后的 stderr 文本作为错误信息。
pub fn run_checked(program: &str, args: &[&str]) -> Result<(), String> {
    run_output_checked_with_timeout(program, args, default_command_timeout(program)).map(|_| ())
}

pub fn run_checked_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<(), String> {
    run_output_checked_with_timeout(program, args, timeout).map(|_| ())
}

pub fn run_output_checked(program: &str, args: &[&str]) -> Result<String, String> {
    run_output_checked_with_timeout(program, args, default_command_timeout(program))
}

pub fn run_output_checked_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<String, String> {
    let out = command_output_with_timeout(program, args, timeout)?;
    if out.stdout.truncated || out.stderr.truncated {
        return Err(format!(
            "{program} 输出超过 {} MiB 上限",
            MAX_COMMAND_OUTPUT_BYTES / 1024 / 1024
        ));
    }
    if out.status.success() {
        Ok(decode_process_output(&out.stdout.bytes).trim().to_string())
    } else {
        let err = decode_process_output(&out.stderr.bytes).trim().to_string();
        let err = if err.is_empty() {
            let stdout = decode_process_output(&out.stdout.bytes).trim().to_string();
            if stdout.is_empty() {
                format!("{program} 退出码 {:?}", out.status.code())
            } else {
                format!("{program} 退出码 {:?}: {stdout}", out.status.code())
            }
        } else {
            err
        };
        Err(err)
    }
}

/// 运行 PowerShell 脚本（NoProfile / 非交互），用于获取 JSON 化的系统数据，
/// 规避本地化输出解析问题。默认限时 15 秒；长扫描可显式使用 `run_ps_with_timeout`。
#[track_caller]
pub fn run_ps(script: &str) -> Option<String> {
    run_ps_with_timeout(script, default_command_timeout("powershell"))
}

#[track_caller]
pub fn run_ps_with_timeout(script: &str, timeout: Duration) -> Option<String> {
    let utf8_script = format!("$OutputEncoding=[System.Text.UTF8Encoding]::new(); [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new(); {script}");
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        utf8_script.as_str(),
    ];
    let output = command_output_with_timeout("powershell", &args, timeout)
        .map_err(|error| {
            tracing::warn!(caller = %std::panic::Location::caller(), %error, "PowerShell 执行失败");
            error
        })
        .ok()?;
    if !output.status.success() {
        let stderr = decode_process_output(&output.stderr.bytes)
            .trim()
            .to_string();
        let stdout = decode_process_output(&output.stdout.bytes)
            .trim()
            .to_string();
        tracing::warn!(
            code = ?output.status.code(),
            caller = %std::panic::Location::caller(),
            stderr = %stderr,
            stdout = %stdout,
            stdout_truncated = output.stdout.truncated,
            stderr_truncated = output.stderr.truncated,
            "PowerShell 命令失败"
        );
        return None;
    }
    if output.stdout.truncated {
        tracing::warn!(caller = %std::panic::Location::caller(), "PowerShell stdout 超过采集上限");
        return None;
    }
    let result = decode_process_output(&output.stdout.bytes)
        .trim()
        .to_string();
    (!result.is_empty()).then_some(result)
}

/// Run a PowerShell query while preserving typed failures for diagnostic UI callers.
#[track_caller]
pub fn run_ps_with_timeout_result(script: &str, timeout: Duration) -> Result<String, String> {
    let utf8_script = format!("$OutputEncoding=[System.Text.UTF8Encoding]::new(); [Console]::OutputEncoding=[System.Text.UTF8Encoding]::new(); {script}");
    let args = [
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        utf8_script.as_str(),
    ];
    let output = command_output_with_timeout("powershell", &args, timeout).map_err(|error| {
        let timed_out = error.contains("超时");
        tracing::warn!(caller = %std::panic::Location::caller(), timed_out, "PowerShell 诊断查询启动或执行失败");
        if timed_out {
            "PowerShell 查询超时".to_string()
        } else {
            "PowerShell 启动或执行失败".to_string()
        }
    })?;
    if !output.status.success() {
        let code = output.status.code();
        tracing::warn!(caller = %std::panic::Location::caller(), ?code, "PowerShell 诊断查询返回非零状态");
        return Err(format!("PowerShell 查询失败（退出码 {code:?}）"));
    }
    if output.stdout.truncated {
        return Err("PowerShell 查询输出超过采集上限".into());
    }
    let result = decode_process_output(&output.stdout.bytes)
        .trim()
        .to_string();
    if result.is_empty() {
        return Err("PowerShell 查询返回空结果".into());
    }
    Ok(result)
}

pub fn is_admin() -> bool {
    unsafe { IsUserAnAdmin() != 0 }
}

/// Resolve the current token's roaming profile folder without trusting an inherited APPDATA value.
fn roaming_app_data_dir() -> Result<PathBuf, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_RoamingAppData, SHGetKnownFolderPath},
    };

    let mut raw: *mut u16 = std::ptr::null_mut();
    let result = unsafe {
        SHGetKnownFolderPath(&FOLDERID_RoamingAppData, 0, std::ptr::null_mut(), &mut raw)
    };
    if result < 0 || raw.is_null() {
        if !raw.is_null() {
            unsafe { CoTaskMemFree(raw.cast()) };
        }
        return Err("无法解析当前用户的 Roaming AppData 已知文件夹".into());
    }

    let mut length = 0usize;
    unsafe {
        while length < 32_768 && *raw.add(length) != 0 {
            length += 1;
        }
    }
    if length == 32_768 {
        unsafe { CoTaskMemFree(raw.cast()) };
        return Err("Roaming AppData 已知文件夹路径无效".into());
    }
    let wide = unsafe { std::slice::from_raw_parts(raw, length) };
    let path = PathBuf::from(OsString::from_wide(wide));
    unsafe { CoTaskMemFree(raw.cast()) };
    if !path.is_absolute() {
        return Err("Roaming AppData 已知文件夹不是绝对路径".into());
    }
    Ok(path)
}

/// 应用数据目录（变更日志 / 启动项备份），由当前 Windows token 的已知文件夹解析。
pub fn app_dir() -> PathBuf {
    let base = roaming_app_data_dir().unwrap_or_else(|error| {
        panic!("无法安全解析 AeroTune 用户数据目录：{error}");
    });
    let dir = base.join("AeroTune");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// 同目录临时文件 + flush + Windows MoveFileExW 原子替换，不先删除旧文件。
/// 不提供跨进程事务或磁盘硬件断电保证。
pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        #[link(name = "kernel32")]
        extern "system" {
            fn MoveFileExW(old: *const u16, new: *const u16, flags: u32) -> i32;
        }
        use std::os::windows::ffi::OsStrExt;
        let old: Vec<u16> = tmp.as_os_str().encode_wide().chain(Some(0)).collect();
        let new: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { MoveFileExW(old.as_ptr(), new.as_ptr(), 1 | 8) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

/// 把任意字符串编码为可做注册表值名 / 文件名的安全 ID。
pub fn safe_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roaming_app_data_resolves_through_the_windows_known_folder_api() {
        let directory =
            roaming_app_data_dir().expect("current user's roaming profile should resolve");
        assert!(
            directory.is_absolute(),
            "resolved profile path must be absolute"
        );
    }

    #[test]
    fn system_tools_resolve_to_verified_windows_system_paths() {
        let system_directory = windows_system_directory().unwrap();
        for (program, executable) in [
            ("takeown", "takeown.exe"),
            ("icacls.exe", "icacls.exe"),
            ("cmd", "cmd.exe"),
            ("powercfg", "powercfg.exe"),
            ("ipconfig.exe", "ipconfig.exe"),
            ("defrag", "defrag.exe"),
            ("schtasks.exe", "schtasks.exe"),
            ("Dism.exe", "dism.exe"),
            ("compact", "compact.exe"),
            ("vssadmin", "vssadmin.exe"),
        ] {
            assert_eq!(
                resolve_executable(program).unwrap(),
                system_directory.join(executable)
            );
        }
        assert!(resolve_executable("unreviewed-helper.exe").is_err());
        assert_eq!(
            resolve_executable("powershell").unwrap(),
            system_directory
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        );
    }

    #[test]
    fn powercfg_output_decode_is_clean() {
        let text = "电源方案 GUID";
        let mut bytes = vec![0xff, 0xfe];
        for u in text.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_output(&bytes), text);
    }

    #[test]
    fn utf8_json_output_preserves_cjk_for_powershell_adapter_data() {
        let bytes = r#"[{\"Name\":\"以太网\",\"Status\":\"Up\"}]"#.as_bytes();
        assert_eq!(
            decode_process_output(bytes),
            r#"[{\"Name\":\"以太网\",\"Status\":\"Up\"}]"#
        );
        assert_eq!(decode_process_output(&[0xEF, 0xBB, 0xBF, b'{', b'}']), "{}");
    }

    #[test]
    fn result_powershell_runner_exposes_distinct_timeout_and_exit_errors() {
        assert_eq!(
            run_ps_with_timeout_result("Write-Output 'ok'", Duration::from_secs(3)).unwrap(),
            "ok"
        );
        let exit_error = run_ps_with_timeout_result(
            "[Console]::Error.Write('ignored'); exit 7",
            Duration::from_secs(3),
        )
        .unwrap_err();
        assert!(exit_error.contains("退出码"));
        let timeout_error =
            run_ps_with_timeout_result("Start-Sleep -Seconds 5", Duration::from_millis(250))
                .unwrap_err();
        assert!(timeout_error.contains("超时"));
    }

    #[test]
    fn timed_runner_drains_stdout_and_stderr_concurrently() {
        let script = "[Console]::Out.Write(('O' * 200000)); [Console]::Error.Write(('E' * 200000))";
        let output = command_output_with_timeout(
            "powershell",
            &["-NoProfile", "-NonInteractive", "-Command", script],
            Duration::from_secs(15),
        )
        .expect("PowerShell should finish with both pipes drained");
        assert!(output.status.success());
        assert_eq!(output.stdout.bytes.len(), 200000);
        assert_eq!(output.stderr.bytes.len(), 200000);
        assert!(!output.stdout.truncated && !output.stderr.truncated);
    }

    #[test]
    fn timed_runner_kills_and_reaps_a_slow_direct_child() {
        let started = Instant::now();
        let error = command_output_with_timeout(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 5",
            ],
            Duration::from_millis(250),
        )
        .err()
        .expect("a five-second command should hit the test deadline");
        assert!(error.contains("执行超时"), "{error}");
        assert!(
            started.elapsed() < Duration::from_secs(4),
            "elapsed: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn timed_runner_preserves_stderr_and_exit_status() {
        let output = command_output_with_timeout(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "[Console]::Error.Write('expected-stderr'); exit 7",
            ],
            Duration::from_secs(3),
        )
        .expect("the command should exit normally with a non-zero status");
        assert!(!output.status.success());
        assert_eq!(
            decode_process_output(&output.stderr.bytes),
            "expected-stderr"
        );
    }

    #[test]
    fn explicit_command_timeout_allows_slow_successful_work() {
        let output = command_output_with_timeout(
            "powershell",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Milliseconds 200; 'done'",
            ],
            Duration::from_secs(3),
        )
        .expect("the explicit deadline should allow normal completion");
        assert!(output.status.success());
        assert!(decode_process_output(&output.stdout.bytes).contains("done"));
    }

    #[test]
    fn direct_windows_powershell_outputs_parseable_utf8_adapter_json() {
        let raw = run_ps("Get-NetAdapter | Select-Object Name,Status | ConvertTo-Json -Compress")
            .expect("Windows PowerShell output should be available");
        let json: serde_json::Value =
            serde_json::from_str(&raw).expect("PowerShell JSON must stay UTF-8");
        assert!(json.is_array() || json.is_object());
        assert!(json.to_string().contains("以太网"));
    }
}
