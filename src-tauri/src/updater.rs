use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;
use url::Url;

pub const UPDATE_ONLY_ARGUMENT: &str = "--aerotune-update-only";
// Keep hard-disabled until rollback and privileged Windows tests are completed and reviewed.
const UPDATE_PIPELINE_READY: bool = false;
// The local updater-plugin patch caps manifests at 256 KiB; this prototype remains disabled pending review.
const UPDATE_FEED_RESPONSE_BOUNDED_READY: bool = false;
// DNS destinations must be constrained against private-network targets and rebinding before enabling.
const UPDATE_DNS_DESTINATION_POLICY_READY: bool = false;
// The selected policy preserves Windows system proxy; keep hard-off until a disposable VM validates it.
const UPDATE_PROXY_POLICY_READY: bool = false;
// 64 MiB compressed and 128 MiB expanded limits are prototypes; cap approval against signed
// release artifacts, protected staging, and end-to-end Windows validation are still incomplete.
const UPDATE_BOUNDED_DOWNLOAD_READY: bool = false;
// Runtime action/DACL verification is implemented, but stays gated until a real-task Windows integration test passes.
const TASK_DEFINITION_ATTESTATION_READY: bool = false;
pub(crate) const RECOVERY_STORE_REQUIRED: bool =
    UPDATE_PIPELINE_READY || TASK_DEFINITION_ATTESTATION_READY;
const UPDATE_ENDPOINT: Option<&str> = option_env!("AEROTUNE_UPDATE_ENDPOINT");
const UPDATE_PUBLIC_KEY: Option<&str> = option_env!("AEROTUNE_UPDATE_PUBLIC_KEY");
const UPDATE_GATE_NAME: &str = "Global\\AeroTune.SignedUpdateGate";
const UPDATE_GATE_WAIT_MS: u32 = 60_000;

#[cfg(windows)]
pub struct UpdateGateGuard {
    handle: windows_sys::Win32::Foundation::HANDLE,
    _thread_bound: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(windows)]
impl Drop for UpdateGateGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.handle);
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

#[cfg(not(windows))]
pub struct UpdateGateGuard;

#[cfg(windows)]
fn acquire_update_gate(timeout_ms: u32) -> Result<UpdateGateGuard, String> {
    acquire_named_update_gate(UPDATE_GATE_NAME, timeout_ms)
}

#[cfg(windows)]
fn acquire_named_update_gate(name: &str, timeout_ms: u32) -> Result<UpdateGateGuard, String> {
    use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};

    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    if handle.is_null() {
        return Err("无法创建 AeroTune 更新互斥锁。".into());
    }
    let result = unsafe { WaitForSingleObject(handle, timeout_ms) };
    if result == WAIT_ABANDONED {
        // WAIT_ABANDONED grants ownership, but the prior owner may have died mid-transaction.
        // Keep this thread's ownership/handle until the fail-closed caller terminates; releasing
        // normally would let a queued waiter acquire WAIT_OBJECT_0 and lose the recovery signal.
        // This is only a baton for current waiters, not a durable journal for later launches.
        return Err("AeroTune 更新互斥锁的前一持有者异常退出；恢复状态未验证，已拒绝继续。".into());
    }
    if result != WAIT_OBJECT_0 {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return Err(if result == WAIT_TIMEOUT {
            "AeroTune 正在更新；本次启动未继续，请稍后重试。".into()
        } else {
            "无法取得 AeroTune 更新互斥锁。".into()
        });
    }
    Ok(UpdateGateGuard {
        handle,
        _thread_bound: std::marker::PhantomData,
    })
}

#[cfg(not(windows))]
fn acquire_update_gate(_timeout_ms: u32) -> Result<UpdateGateGuard, String> {
    Ok(UpdateGateGuard)
}

/// The normal app fails closed instead of starting while an update owns the process gate.
pub fn try_acquire_app_update_gate() -> Result<UpdateGateGuard, String> {
    let gate = acquire_update_gate(0)?;
    crate::update_recovery::ensure_no_pending_journal(RECOVERY_STORE_REQUIRED)?;
    Ok(gate)
}

#[derive(Debug, Serialize)]
pub struct UpdateReadiness {
    pub enabled: bool,
    pub task_registered: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AvailableUpdate {
    pub current_version: String,
    pub available_version: String,
}

fn url_uses_ip_literal(url: &Url) -> bool {
    matches!(
        url.host(),
        Some(url::Host::Ipv4(_)) | Some(url::Host::Ipv6(_))
    )
}

fn normalized_url_host(url: &Url) -> String {
    url.host_str()
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

fn valid_tauri_public_key(public_key: &str) -> bool {
    use base64::{engine::general_purpose::STANDARD, Engine as _};

    if public_key.is_empty() || public_key.len() > 4096 {
        return false;
    }
    let Ok(decoded) = STANDARD.decode(public_key) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(&decoded) else {
        return false;
    };
    let mut lines = text.lines();
    let Some(comment) = lines.next() else {
        return false;
    };
    let Some(key_material) = lines.next() else {
        return false;
    };
    comment.starts_with("untrusted comment:")
        && minisign_verify::PublicKey::from_base64(key_material).is_ok()
}

const RESERVED_UPDATE_HOST_SUFFIXES: &[&str] = &[
    "localhost",
    "local",
    "internal",
    "test",
    "invalid",
    "example",
    "example.com",
    "example.org",
    "example.net",
    "onion",
    "lan",
    "corp",
    "intranet",
    "private",
    "localdomain",
];

fn valid_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

/// Validates URL syntax only; DNS answer pinning/rebinding policy is a separate hard-off gate.
fn validate_https_dns_url(url: &Url, allow_query: bool) -> Result<(), String> {
    let raw_host = url.host_str().unwrap_or_default();
    let host = normalized_url_host(url);
    if url.scheme() != "https"
        || host.is_empty()
        || url_uses_ip_literal(url)
        || raw_host.ends_with('.')
        || url.port().is_some()
        || url.username() != ""
        || url.password().is_some()
    {
        return Err(
            "URL must use HTTPS with a default-port DNS hostname and no credentials.".into(),
        );
    }
    let labels: Vec<_> = host.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|label| !valid_dns_label(label)) {
        return Err("URL must use a fully qualified DNS hostname with valid labels.".into());
    }
    if RESERVED_UPDATE_HOST_SUFFIXES
        .iter()
        .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
    {
        return Err(
            "URL hostname is reserved or internal; public update hosts are required.".into(),
        );
    }
    if url.fragment().is_some() {
        return Err("URL fragments are not accepted.".into());
    }
    if !allow_query && url.query().is_some() {
        return Err("Feed URL query strings are not accepted.".into());
    }
    Ok(())
}

fn validate_update_inputs(
    endpoint: &str,
    public_key: &str,
    pipeline_ready: bool,
) -> Result<(Url, String), String> {
    if !pipeline_ready {
        return Err("受保护更新器尚未完成回滚与安全审计，更新功能保持关闭。".into());
    }
    let endpoint = endpoint.trim();
    if endpoint.is_empty() || endpoint.contains('<') || endpoint.contains('>') {
        return Err("安全更新源尚未配置。".into());
    }
    let endpoint = Url::parse(endpoint).map_err(|_| "安全更新源地址无效。".to_string())?;
    validate_https_dns_url(&endpoint, false)?;

    let public_key = public_key.trim();
    let lower_key = public_key.to_ascii_lowercase();
    if public_key.is_empty()
        || lower_key.contains("replace")
        || lower_key.contains("placeholder")
        || lower_key.contains("private key")
        || public_key.contains('<')
        || public_key.contains('>')
    {
        return Err("Tauri 更新公钥尚未配置。".into());
    }
    if !valid_tauri_public_key(public_key) {
        return Err("Tauri 更新公钥不是有效的 Minisign 公钥格式。".into());
    }
    Ok((endpoint, public_key.to_string()))
}

fn validate_runtime_update_inputs(
    endpoint: Option<&str>,
    public_key: Option<&str>,
    feed_response_bounded_ready: bool,
    dns_destination_policy_ready: bool,
    proxy_policy_ready: bool,
    bounded_download_ready: bool,
    task_definition_attestation_ready: bool,
    pipeline_ready: bool,
) -> Result<(Url, String), String> {
    let endpoint = endpoint.ok_or_else(|| "安全更新源尚未配置。".to_string())?;
    let public_key = public_key.ok_or_else(|| "Tauri 更新公钥尚未配置。".to_string())?;
    if !feed_response_bounded_ready {
        return Err("更新清单大小上限尚未通过 release review；更新功能保持关闭。".into());
    }
    if !dns_destination_policy_ready {
        return Err("更新源 DNS 目的地址策略尚未通过审核；更新功能保持关闭。".into());
    }
    if !proxy_policy_ready {
        return Err("Windows 系统代理保留策略尚未通过 VM 验证；更新功能保持关闭。".into());
    }
    if !bounded_download_ready {
        return Err("有界签名下载和受保护暂存尚未完成；更新功能保持关闭。".into());
    }
    if !task_definition_attestation_ready {
        return Err("固定更新任务定义和访问控制尚未通过运行时验收；更新功能保持关闭。".into());
    }
    validate_update_inputs(endpoint, public_key, pipeline_ready)
}

fn configured_update_inputs() -> Result<(Url, String), String> {
    validate_runtime_update_inputs(
        UPDATE_ENDPOINT,
        UPDATE_PUBLIC_KEY,
        UPDATE_FEED_RESPONSE_BOUNDED_READY,
        UPDATE_DNS_DESTINATION_POLICY_READY,
        UPDATE_PROXY_POLICY_READY,
        UPDATE_BOUNDED_DOWNLOAD_READY,
        TASK_DEFINITION_ATTESTATION_READY,
        UPDATE_PIPELINE_READY,
    )
}

#[cfg(windows)]
fn trusted_program_files_directory() -> Result<std::path::PathBuf, String> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
    use windows_sys::Win32::{
        System::Com::CoTaskMemFree,
        UI::Shell::{FOLDERID_ProgramFiles, SHGetKnownFolderPath},
    };

    let mut path_ptr: *mut u16 = std::ptr::null_mut();
    let result = unsafe {
        SHGetKnownFolderPath(
            &FOLDERID_ProgramFiles,
            0,
            std::ptr::null_mut(),
            &mut path_ptr,
        )
    };
    if result < 0 || path_ptr.is_null() {
        if !path_ptr.is_null() {
            unsafe { CoTaskMemFree(path_ptr.cast()) };
        }
        return Err("无法解析系统 Program Files 目录。".into());
    }

    let mut length = 0usize;
    unsafe {
        while *path_ptr.add(length) != 0 {
            length += 1;
        }
    }
    let directory = unsafe {
        PathBuf::from(OsString::from_wide(std::slice::from_raw_parts(
            path_ptr, length,
        )))
    };
    unsafe { CoTaskMemFree(path_ptr.cast()) };
    if !directory.is_absolute() {
        return Err("系统 Program Files 路径不是绝对路径。".into());
    }
    Ok(directory)
}

#[cfg(not(windows))]
fn trusted_program_files_directory() -> Result<std::path::PathBuf, String> {
    Err("Windows Program Files 目录仅适用于 Windows。".into())
}

// This is only a path/reparse-point check. It does not attest ACLs; runtime ACL/task attestation
// remains a release blocker even though the setup script checks ACLs during provisioning.
fn validate_protected_executable() -> Result<(), String> {
    let program_files = trusted_program_files_directory()?;
    let expected = program_files.join("AeroTune").join("AeroTune.exe");
    let actual = std::env::current_exe().map_err(|_| "无法解析更新器自身路径。".to_string())?;
    let install_dir = expected
        .parent()
        .ok_or_else(|| "Program Files/AeroTune 安装目录无效。".to_string())?;
    for path in [program_files.as_path(), install_dir, actual.as_path()] {
        let metadata =
            std::fs::symlink_metadata(path).map_err(|_| "无法验证更新器安装路径。".to_string())?;
        if metadata.file_type().is_symlink() {
            return Err("更新器路径包含符号链接，已拒绝。".into());
        }
    }
    let expected = expected
        .canonicalize()
        .map_err(|_| "受保护 AeroTune 安装不存在。".to_string())?;
    let actual = actual
        .canonicalize()
        .map_err(|_| "无法规范化更新器路径。".to_string())?;
    if !actual
        .to_string_lossy()
        .eq_ignore_ascii_case(&expected.to_string_lossy())
    {
        return Err("更新器拒绝从 Program Files/AeroTune 之外的副本运行。".into());
    }
    Ok(())
}

#[cfg(windows)]
fn windows_system_directory() -> Option<std::path::PathBuf> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf};
    let mut buffer = vec![0u16; 32_768];
    let length = unsafe {
        windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if length == 0 || length >= buffer.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&buffer[..length])))
}

// Execute only fixed read-only/Run task actions from the embedded verifier. The Run action
// verifies and starts the same COM task object, avoiding a check-then-schtasks race.
#[cfg(windows)]
fn run_embedded_task_action(action: &'static str) -> bool {
    use std::{os::windows::process::CommandExt, process::Command};

    if !matches!(action, "Verify" | "Run") {
        return false;
    }
    let Some(system_directory) = windows_system_directory() else {
        return false;
    };
    let powershell = system_directory
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let verifier = include_str!("../../scripts/signed-updater-task.ps1");
    let command = format!(
        "try {{ & {{\n{verifier}\n}} -Action {action} *> $null; exit 0 }} catch {{ exit 1 }}"
    );
    Command::new(powershell)
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command"])
        .arg(command)
        .creation_flags(crate::util::CREATE_NO_WINDOW)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn task_is_registered() -> bool {
    run_embedded_task_action("Verify")
}

#[cfg(not(windows))]
fn task_is_registered() -> bool {
    false
}

#[tauri::command]
pub fn get_signed_update_readiness() -> UpdateReadiness {
    match configured_update_inputs() {
        Ok(_) => {
            let task_registered = TASK_DEFINITION_ATTESTATION_READY && task_is_registered();
            let reason = if !TASK_DEFINITION_ATTESTATION_READY {
                Some("受保护任务的完整定义与 DACL 尚未通过运行时验证。".to_string())
            } else if !task_registered {
                Some("尚未配置受保护的管理员更新任务。".to_string())
            } else {
                None
            };
            UpdateReadiness {
                enabled: task_registered,
                task_registered,
                reason,
            }
        }
        // Do not inspect or invoke the task while the update pipeline is disabled/unconfigured.
        Err(reason) => UpdateReadiness {
            enabled: false,
            task_registered: false,
            reason: Some(reason),
        },
    }
}

fn ensure_newer_version(current: &str, candidate: &str) -> Result<semver::Version, String> {
    let current = semver::Version::parse(current).map_err(|_| "本机版本格式无效。".to_string())?;
    let candidate =
        semver::Version::parse(candidate).map_err(|_| "更新源返回的版本格式无效。".to_string())?;
    if candidate.cmp_precedence(&current) != std::cmp::Ordering::Greater {
        return Err("更新版本未高于当前版本，已拒绝。".into());
    }
    Ok(candidate)
}

fn validate_artifact_url(url: &Url) -> Result<(), String> {
    validate_https_dns_url(url, true)
}

const MAX_UPDATER_REDIRECT_HOPS: usize = 3;
const UPDATER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
// Prototype ceiling only; not release-approved until measured against the signed target artifact.
const PROPOSED_MAX_UPDATE_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const PROPOSED_MAX_UPDATE_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_UPDATE_ARCHIVE_ENTRIES: usize = 1;

fn updater_redirect_is_safe(previous_urls: &[Url], next_url: &Url) -> bool {
    !previous_urls.is_empty()
        && previous_urls.len() <= MAX_UPDATER_REDIRECT_HOPS
        && previous_urls
            .iter()
            .all(|url| validate_artifact_url(url).is_ok())
        && validate_artifact_url(next_url).is_ok()
}

fn updater_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if updater_redirect_is_safe(attempt.previous(), attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("redirect blocked by AeroTune updater policy")
        }
    })
}

fn configure_updater_client(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    builder
        .timeout(UPDATER_REQUEST_TIMEOUT)
        .redirect(updater_redirect_policy())
}

fn validate_system_proxy_policy(
    no_proxy: bool,
    explicit_proxy_configured: bool,
) -> Result<(), String> {
    if no_proxy || explicit_proxy_configured {
        return Err(
            "更新策略要求保留 Windows 系统代理；显式代理/直连覆盖未通过审核，已拒绝下载。".into(),
        );
    }
    Ok(())
}

fn validate_artifact_request_headers(headers: &reqwest::header::HeaderMap) -> Result<(), String> {
    if !headers.is_empty() {
        return Err("更新工件请求不接受未审核的自定义 HTTP 头；已拒绝下载。".into());
    }
    Ok(())
}

fn build_bounded_update_request(
    update: &tauri_plugin_updater::Update,
) -> Result<reqwest::RequestBuilder, String> {
    validate_system_proxy_policy(update.no_proxy, update.proxy.is_some())?;
    validate_artifact_url(&update.download_url)?;
    validate_artifact_request_headers(&update.headers)?;
    let mut builder =
        reqwest::Client::builder().user_agent(concat!("AeroTune/", env!("CARGO_PKG_VERSION")));
    if let Some(timeout) = update.timeout {
        builder = builder.timeout(timeout);
    }
    let client = configure_updater_client(builder)
        .build()
        .map_err(|_| "无法创建受限更新下载客户端。".to_string())?;
    Ok(client
        .get(update.download_url.clone())
        .headers(update.headers.clone()))
}

async fn collect_bounded_update_response(
    response: reqwest::Response,
    byte_limit: usize,
) -> Result<Vec<u8>, String> {
    if !response.status().is_success() {
        return Err("更新工件服务器返回失败状态；已拒绝下载。".into());
    }
    if response
        .content_length()
        .is_some_and(|length| length > byte_limit as u64)
    {
        return Err("更新工件超过配置的字节上限；已拒绝下载。".into());
    }

    let mut bytes = Vec::new();
    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "更新工件流读取失败；已拒绝下载。".to_string())?
    {
        let next_len = bytes
            .len()
            .checked_add(chunk.len())
            .ok_or_else(|| "更新工件长度溢出；已拒绝下载。".to_string())?;
        if next_len > byte_limit {
            return Err("更新工件超过配置的字节上限；已拒绝下载。".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn validate_pe_installer_header(header: &[u8]) -> Result<(), String> {
    if header.len() < 64 || header.get(..2) != Some(b"MZ") {
        return Err("更新归档不是有效的 Windows PE 安装程序；已拒绝处理。".into());
    }
    let pe_offset = u32::from_le_bytes(
        header[0x3c..0x40]
            .try_into()
            .map_err(|_| "更新安装程序 PE 头无效。".to_string())?,
    ) as usize;
    let pe_header_end = pe_offset
        .checked_add(6)
        .ok_or_else(|| "更新安装程序 PE 头偏移溢出。".to_string())?;
    if pe_header_end > header.len()
        || header.get(pe_offset..pe_offset + 4) != Some(&[b'P', b'E', 0, 0][..])
    {
        return Err("更新归档缺少有效的 Windows PE 签名；已拒绝处理。".into());
    }
    let machine = u16::from_le_bytes([header[pe_offset + 4], header[pe_offset + 5]]);
    let expected_machine = if cfg!(target_arch = "x86_64") {
        0x8664
    } else if cfg!(target_arch = "aarch64") {
        0xaa64
    } else if cfg!(target_arch = "x86") {
        0x014c
    } else {
        return Err("当前处理器架构不受更新器支持。".into());
    };
    if machine != expected_machine {
        return Err("更新安装程序 PE 架构与当前程序不一致；已拒绝处理。".into());
    }
    Ok(())
}

fn validate_update_archive_payload(bytes: &[u8], target_version: &str) -> Result<(), String> {
    validate_update_archive_with_limits(
        bytes,
        target_version,
        PROPOSED_MAX_UPDATE_ARTIFACT_BYTES,
        PROPOSED_MAX_UPDATE_EXPANDED_BYTES,
        MAX_UPDATE_ARCHIVE_ENTRIES,
    )
}

fn validate_update_archive_with_limits(
    bytes: &[u8],
    target_version: &str,
    compressed_limit: usize,
    expanded_limit: u64,
    entry_limit: usize,
) -> Result<(), String> {
    use std::io::Read;

    if bytes.len() > compressed_limit {
        return Err("更新归档超过压缩字节上限；已拒绝处理。".into());
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| "更新归档格式无效；已拒绝处理。".to_string())?;
    if archive.is_empty() || archive.len() > entry_limit {
        return Err("更新归档文件数不符合限制；已拒绝处理。".into());
    }

    let mut total_expanded = 0u64;
    let mut actual_total_expanded = 0u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| "更新归档目录无效；已拒绝处理。".to_string())?;
        let name = entry.name();
        if entry.encrypted()
            || entry.is_dir()
            || entry.is_symlink()
            || !entry.is_file()
            || entry.enclosed_name().is_none()
            || name.contains('/')
            || name.contains('\\')
            || name.contains(':')
            || !name.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-')
            })
        {
            return Err("更新归档包含加密、目录、链接或非根路径条目；已拒绝处理。".into());
        }
        let lower_name = name.to_ascii_lowercase();
        if !lower_name.starts_with("aerotune_") || !lower_name.ends_with("-setup.exe") {
            return Err(
                "更新归档仅允许一个根目录 AeroTune NSIS -setup.exe 安装包；已拒绝处理。".into(),
            );
        }
        let target = semver::Version::parse(target_version)
            .map_err(|_| "更新归档目标版本无效；已拒绝处理。".to_string())?;
        let expected_prefix = format!("aerotune_{}_", target);
        if !lower_name.starts_with(&expected_prefix) {
            return Err("更新归档文件名版本与签名更新版本不一致；已拒绝处理。".into());
        }
        let expected_arch_suffix = if cfg!(target_arch = "x86_64") {
            "_x64-setup.exe"
        } else if cfg!(target_arch = "aarch64") {
            "_arm64-setup.exe"
        } else if cfg!(target_arch = "x86") {
            "_x86-setup.exe"
        } else {
            return Err("当前处理器架构不受更新器支持。".into());
        };
        if !lower_name.ends_with(expected_arch_suffix) {
            return Err("更新归档安装器架构与当前程序不一致；已拒绝处理。".into());
        }
        if entry.compressed_size() > compressed_limit as u64 {
            return Err("更新归档成员超过压缩字节上限；已拒绝处理。".into());
        }
        let declared_size = entry.size();
        total_expanded = total_expanded
            .checked_add(declared_size)
            .ok_or_else(|| "更新归档展开长度溢出；已拒绝处理。".to_string())?;
        if total_expanded > expanded_limit {
            return Err("更新归档超过展开字节上限；已拒绝处理。".into());
        }

        let mut actual_size = 0u64;
        let mut installer_header = Vec::with_capacity(4096);
        let mut scratch = [0u8; 8192];
        loop {
            let read = entry
                .read(&mut scratch)
                .map_err(|_| "更新归档展开或 CRC 校验失败；已拒绝处理。".to_string())?;
            if read == 0 {
                break;
            }
            actual_size = actual_size
                .checked_add(read as u64)
                .ok_or_else(|| "更新归档展开长度溢出；已拒绝处理。".to_string())?;
            if installer_header.len() < 4096 {
                let take = (4096 - installer_header.len()).min(read);
                installer_header.extend_from_slice(&scratch[..take]);
            }
            actual_total_expanded = actual_total_expanded
                .checked_add(read as u64)
                .ok_or_else(|| "更新归档总展开长度溢出；已拒绝处理。".to_string())?;
            if actual_total_expanded > expanded_limit {
                return Err("更新归档实际总展开内容超过字节上限；已拒绝处理。".into());
            }
            if actual_size > expanded_limit {
                return Err("更新归档实际展开内容超过字节上限；已拒绝处理。".into());
            }
        }
        if actual_size != declared_size {
            return Err("更新归档声明大小与实际展开大小不一致；已拒绝处理。".into());
        }
        validate_pe_installer_header(&installer_header)?;
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct VerifiedUpdateArtifact {
    bytes: Vec<u8>,
    target_version: semver::Version,
    sha256: [u8; 32],
}

impl VerifiedUpdateArtifact {
    fn new(bytes: Vec<u8>, target_version: semver::Version) -> Self {
        use sha2::{Digest, Sha256};

        let sha256 = Sha256::digest(&bytes).into();
        Self {
            bytes,
            target_version,
            sha256,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn target_version(&self) -> &semver::Version {
        &self.target_version
    }

    pub(crate) fn sha256_hex(&self) -> String {
        let mut encoded = String::with_capacity(64);
        for byte in self.sha256 {
            encoded.push_str(&format!("{byte:02x}"));
        }
        encoded
    }

    #[allow(dead_code)]
    pub(crate) fn verify_staged_bytes(&self, staged_bytes: &[u8]) -> Result<(), String> {
        use sha2::{Digest, Sha256};

        if staged_bytes.len() != self.bytes.len() {
            return Err("受保护暂存内容长度与已验证更新工件不一致；已拒绝继续。".into());
        }
        let staged_digest: [u8; 32] = Sha256::digest(staged_bytes).into();
        if staged_digest != self.sha256 {
            return Err("受保护暂存内容摘要与已验证更新工件不一致；已拒绝继续。".into());
        }
        Ok(())
    }
}

fn verify_update_signature(
    update: &tauri_plugin_updater::Update,
    bytes: &[u8],
) -> Result<(), String> {
    update
        .verify(bytes)
        .map_err(|_| "更新工件签名或绑定版本验证失败；已拒绝更新。".to_string())
}

fn verify_and_wrap_update_artifact(
    bytes: Vec<u8>,
    target_version: &str,
    verify_signature: impl FnOnce(&[u8]) -> Result<(), String>,
    validate_archive: impl FnOnce(&[u8], &str) -> Result<(), String>,
) -> Result<VerifiedUpdateArtifact, String> {
    verify_signature(&bytes)?;
    let target_version = semver::Version::parse(target_version)
        .map_err(|_| "更新工件目标版本无效；已拒绝更新。".to_string())?;
    validate_archive(&bytes, &target_version.to_string())?;
    Ok(VerifiedUpdateArtifact::new(bytes, target_version))
}

async fn download_and_verify_bounded_update(
    update: &tauri_plugin_updater::Update,
) -> Result<VerifiedUpdateArtifact, String> {
    if !UPDATE_PIPELINE_READY {
        return Err("更新处理流水线尚未通过验收；已拒绝下载。".into());
    }
    if !UPDATE_DNS_DESTINATION_POLICY_READY {
        return Err("更新源 DNS 目的地址策略尚未通过审核；已拒绝下载。".into());
    }
    if !UPDATE_PROXY_POLICY_READY {
        return Err("Windows 系统代理保留策略尚未通过 VM 验证；已拒绝下载。".into());
    }
    if !TASK_DEFINITION_ATTESTATION_READY {
        return Err("固定更新任务及其安全属性尚未通过验收；已拒绝下载。".into());
    }
    if !UPDATE_BOUNDED_DOWNLOAD_READY {
        return Err("有界更新下载尚未通过完整性与容量审核；已拒绝下载。".into());
    }
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| "本机版本格式无效。".to_string())?;
    ensure_newer_version(&current.to_string(), &update.version)?;
    let response = build_bounded_update_request(update)?
        .send()
        .await
        .map_err(|_| "更新工件下载失败；已拒绝更新。".to_string())?;
    let bytes =
        collect_bounded_update_response(response, PROPOSED_MAX_UPDATE_ARTIFACT_BYTES).await?;
    verify_and_wrap_update_artifact(
        bytes,
        &update.version,
        |bytes| verify_update_signature(update, bytes),
        |bytes, version| validate_update_archive_payload(bytes, version),
    )
}

fn build_updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    let (endpoint, public_key) = configured_update_inputs()?;
    app.updater_builder()
        .pubkey(public_key)
        .endpoints(vec![endpoint])
        .map_err(|_| "安全更新源配置无效。".to_string())?
        .configure_client(configure_updater_client)
        .restart_after_install(false)
        .build()
        .map_err(|_| "安全更新器初始化失败。".to_string())
}

#[tauri::command]
pub async fn check_signed_update(_app: AppHandle) -> Result<Option<AvailableUpdate>, String> {
    let readiness = get_signed_update_readiness();
    if !readiness.enabled {
        return Err(readiness
            .reason
            .unwrap_or_else(|| "安全更新功能不可用。".into()));
    }
    // The standard UI intentionally does not register Tauri's updater plugin. Once the
    // external protected host is complete, update checks must be routed through that host.
    Err("标准界面未加载更新插件；请由经过审计的受保护更新主机提供更新检查。".into())
}

#[cfg(windows)]
#[tauri::command]
pub fn request_signed_update(app: AppHandle) -> Result<(), String> {
    let readiness = get_signed_update_readiness();
    if !readiness.enabled {
        return Err(readiness
            .reason
            .unwrap_or_else(|| "安全更新功能不可用。".into()));
    }
    if !run_embedded_task_action("Run") {
        return Err("受保护的更新任务定义/DACL 验证或启动失败。".into());
    }
    app.exit(0);
    Ok(())
}

#[cfg(not(windows))]
#[tauri::command]
pub fn request_signed_update(_app: AppHandle) -> Result<(), String> {
    Err("安全更新仅适用于 Windows。".into())
}

fn wait_for_other_aerotune_instances() -> Result<(), String> {
    use std::time::{Duration, Instant};
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let current_exe = std::env::current_exe().map_err(|_| "无法识别更新器路径。".to_string())?;
    let current_path = current_exe.to_string_lossy().to_ascii_lowercase();
    let current_pid = std::process::id();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut system = System::new();
    loop {
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_exe(UpdateKind::OnlyIfNotSet),
        );
        let mut another_instance = false;
        let mut unknown_aerotune_process = false;
        for process in system.processes().values() {
            if process.pid().as_u32() == current_pid {
                continue;
            }
            match process.exe() {
                Some(path) if path.to_string_lossy().eq_ignore_ascii_case(&current_path) => {
                    another_instance = true
                }
                None if process
                    .name()
                    .to_string_lossy()
                    .eq_ignore_ascii_case("AeroTune.exe") =>
                {
                    unknown_aerotune_process = true
                }
                _ => {}
            }
        }
        if unknown_aerotune_process {
            return Err(
                "无法确认另一个 AeroTune 进程的路径；为避免覆盖运行中的文件，更新已取消。".into(),
            );
        }
        if !another_instance {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("AeroTune 主窗口尚未退出；为避免覆盖运行中的文件，更新已取消。".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

async fn apply_signed_update(app: AppHandle) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("更新任务未以管理员令牌运行；未请求或绕过 UAC。".into());
    }
    let updater = build_updater(&app)?;
    if let Some(update) = updater
        .check()
        .await
        .map_err(|_| "无法检查签名更新。".to_string())?
    {
        validate_artifact_url(&update.download_url)?;
        let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .map_err(|_| "本机版本格式无效。".to_string())?;
        ensure_newer_version(&current.to_string(), &update.version)?;
        if !UPDATE_BOUNDED_DOWNLOAD_READY {
            return Err("有界更新下载尚未通过完整性与容量审核；已拒绝下载。".into());
        }
        let _verified_artifact = download_and_verify_bounded_update(&update).await?;
        // Do not invoke Tauri's Windows install path here: it stages the NSIS payload under
        // the user temp directory before launching it. Protected staging, post-copy verification,
        // installer-result validation, and rollback must be implemented before enabling installs.
        return Err("受保护暂存、安装结果确认和回滚尚未完成；已拒绝安装更新。".into());
    }
    Ok(())
}

fn log_updater_error(message: impl std::fmt::Display) {
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "AeroTune updater: {message}");
}

/// Restricted update-only mode intended for the fixed-action task; it never requests or bypasses UAC.
pub fn run_update_only() -> i32 {
    use std::sync::{
        atomic::{AtomicI32, Ordering},
        Arc,
    };

    if !crate::util::is_admin() {
        log_updater_error("elevated task required; refusing to request UAC.");
        return 1;
    }
    if let Err(reason) = validate_protected_executable() {
        log_updater_error(reason);
        return 1;
    }
    if let Err(reason) = configured_update_inputs() {
        log_updater_error(format!("disabled: {reason}"));
        return 1;
    }
    let _update_gate = match acquire_update_gate(UPDATE_GATE_WAIT_MS) {
        Ok(gate) => gate,
        Err(reason) => {
            log_updater_error(reason);
            return 1;
        }
    };
    if let Err(reason) = crate::update_recovery::ensure_no_pending_journal(RECOVERY_STORE_REQUIRED)
    {
        log_updater_error(reason);
        return 1;
    }

    let exit_code = Arc::new(AtomicI32::new(0));
    let task_exit_code = exit_code.clone();
    let app = match tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(move |app| {
            let handle = app.handle().clone();
            let exit_code = task_exit_code.clone();
            tauri::async_runtime::spawn(async move {
                let result =
                    tauri::async_runtime::spawn_blocking(wait_for_other_aerotune_instances)
                        .await
                        .map_err(|_| "无法等待 AeroTune 退出。".to_string())
                        .and_then(|result| result);
                if let Err(reason) = result {
                    log_updater_error(reason);
                    exit_code.store(1, Ordering::SeqCst);
                    handle.exit(1);
                    return;
                }
                if let Err(reason) = apply_signed_update(handle.clone()).await {
                    log_updater_error(reason);
                    exit_code.store(1, Ordering::SeqCst);
                    handle.exit(1);
                } else {
                    handle.exit(0);
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!("tauri.updater-helper.conf.json"))
    {
        Ok(app) => app,
        Err(error) => {
            log_updater_error(format!("failed to initialize: {error}"));
            return 1;
        }
    };
    app.run(|_, _| {});
    exit_code.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_TEST_TAURI_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXkgRTc2MjBGMTg0MkI0RTgxRgpSV1FmNkxSQ0dBOWk1M21sWWVjTzRJelQ1MVRHUHB2V3VjTlNDaDFDQk0wUVRhTG43M1k3R0ZPMw==";
    const VALID_TAURI_GENERATED_PUBLIC_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEE0OUVGNDc0NkZBNkU2RDcKUldUWDVxWnZkUFNlcERHV2cyYno4bjZBclFuQk13Z1FLMWdlVm9zb3JjYkx6Y3VuRCtLSitjWWU=";

    #[test]
    fn selected_system_proxy_policy_rejects_direct_and_explicit_proxy_overrides() {
        assert!(validate_system_proxy_policy(false, false).is_ok());
        assert!(validate_system_proxy_policy(true, false).is_err());
        assert!(validate_system_proxy_policy(false, true).is_err());
        assert!(validate_system_proxy_policy(true, true).is_err());
    }

    #[test]
    fn artifact_download_rejects_unreviewed_custom_headers() {
        let empty = reqwest::header::HeaderMap::new();
        assert!(validate_artifact_request_headers(&empty).is_ok());

        let mut custom = reqwest::header::HeaderMap::new();
        custom.insert(
            "x-aerotune-token",
            reqwest::header::HeaderValue::from_static("test-only"),
        );
        let error = validate_artifact_request_headers(&custom).unwrap_err();
        assert!(error.contains("未审核的自定义 HTTP 头"));
    }

    #[test]
    fn verified_artifact_binds_exact_bytes_target_version_and_digest() {
        let artifact = verify_and_wrap_update_artifact(
            b"abc".to_vec(),
            "1.5.10",
            |bytes| {
                assert_eq!(bytes, b"abc");
                Ok(())
            },
            |bytes, version| {
                assert_eq!(bytes, b"abc");
                assert_eq!(version, "1.5.10");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(artifact.bytes(), b"abc");
        assert_eq!(artifact.target_version().to_string(), "1.5.10");
        assert_eq!(
            artifact.sha256_hex(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(artifact.verify_staged_bytes(b"abc").is_ok());
        assert!(artifact.verify_staged_bytes(b"abd").is_err());
        assert!(artifact.verify_staged_bytes(b"ab").is_err());
    }

    #[test]
    fn failed_signature_check_never_produces_verified_artifact() {
        let archive_validation_ran = std::cell::Cell::new(false);
        let result = verify_and_wrap_update_artifact(
            b"tampered".to_vec(),
            "1.5.10",
            |_| Err("signature invalid".into()),
            |_, _| {
                archive_validation_ran.set(true);
                Ok(())
            },
        );

        assert!(result.is_err());
        assert!(!archive_validation_ran.get());
    }

    #[test]
    fn journal_factory_derives_update_and_rollback_hashes_from_verified_artifacts() {
        let update_artifact = verify_and_wrap_update_artifact(
            b"update payload".to_vec(),
            "1.5.10",
            |_| Ok(()),
            |_, _| Ok(()),
        )
        .unwrap();
        let rollback_artifact = verify_and_wrap_update_artifact(
            b"rollback payload".to_vec(),
            "1.5.9",
            |_| Ok(()),
            |_, _| Ok(()),
        )
        .unwrap();
        let journal = crate::update_recovery::UpdateTransactionJournal::from_verified_artifacts(
            "0123456789abcdef0123456789abcdef".into(),
            "1.5.9".into(),
            &update_artifact,
            &rollback_artifact,
        )
        .unwrap();
        let encoded: serde_json::Value =
            serde_json::from_slice(&journal.encode().unwrap()).unwrap();

        assert_eq!(
            encoded["update_artifact_sha256"],
            update_artifact.sha256_hex()
        );
        assert_eq!(
            encoded["rollback_artifact_sha256"],
            rollback_artifact.sha256_hex()
        );
        assert_eq!(encoded["target_version"], "1.5.10");

        let mismatched_rollback = verify_and_wrap_update_artifact(
            b"wrong rollback payload".to_vec(),
            "1.5.8",
            |_| Ok(()),
            |_, _| Ok(()),
        )
        .unwrap();
        assert!(
            crate::update_recovery::UpdateTransactionJournal::from_verified_artifacts(
                "fedcba9876543210fedcba9876543210".into(),
                "1.5.9".into(),
                &update_artifact,
                &mismatched_rollback,
            )
            .is_err()
        );
    }

    #[test]
    fn failed_archive_check_never_produces_verified_artifact() {
        let result = verify_and_wrap_update_artifact(
            b"signed but invalid archive".to_vec(),
            "1.5.10",
            |_| Ok(()),
            |_, _| Err("invalid archive".into()),
        );
        assert!(result.is_err());
    }

    #[cfg(windows)]
    #[test]
    fn trusted_program_files_root_resolves_to_existing_absolute_known_folder() {
        let directory = trusted_program_files_directory().unwrap();
        assert!(directory.is_absolute());
        assert!(directory.is_dir());
    }

    #[test]
    fn tauri_public_key_must_decode_to_a_valid_minisign_key() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};

        assert!(valid_tauri_public_key(VALID_TEST_TAURI_PUBLIC_KEY));
        assert!(valid_tauri_public_key(VALID_TAURI_GENERATED_PUBLIC_KEY));
        let decoded_key = STANDARD.decode(VALID_TEST_TAURI_PUBLIC_KEY).unwrap();
        let decoded_key = std::str::from_utf8(&decoded_key).unwrap();
        let windows_line_endings = STANDARD.encode(decoded_key.replace('\n', "\r\n"));
        assert!(valid_tauri_public_key(&windows_line_endings));
        assert!(!valid_tauri_public_key(""));
        assert!(!valid_tauri_public_key("RWQPublicKey"));
        assert!(!valid_tauri_public_key("@@not-base64@@"));
        let malformed = STANDARD.encode("untrusted comment: test key\nAAAA");
        assert!(!valid_tauri_public_key(&malformed));

        let error = validate_update_inputs(
            "https://updates.aerotune.net/latest.json",
            "RWQPublicKey",
            true,
        )
        .unwrap_err();
        assert!(error.contains("不是有效的 Minisign 公钥格式"));
    }

    #[test]
    fn update_inputs_fail_closed_without_pipeline_flag_or_real_https_feed() {
        assert!(
            validate_update_inputs("https://updates.example.invalid/latest.json", "", false)
                .is_err()
        );
        assert!(validate_update_inputs(
            "https://updates.Example.invalid/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
        assert!(validate_update_inputs(
            "https://localhost/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
        assert!(validate_update_inputs(
            "https://localhost./latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
        assert!(validate_update_inputs(
            "https://updates.localhost./latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
        assert!(validate_update_inputs(
            "https://127.0.0.1/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
        assert!(validate_update_inputs(
            "https://[::1]/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
        for endpoint in [
            "https://2130706433/latest.json",
            "https://0x7f000001/latest.json",
            "https://0177.0.0.1/latest.json",
            "https://127.1/latest.json",
            "https://127.0.0.1./latest.json",
        ] {
            assert!(
                validate_update_inputs(endpoint, VALID_TEST_TAURI_PUBLIC_KEY, true).is_err(),
                "non-canonical IP endpoint must be rejected: {endpoint}"
            );
        }
        assert!(validate_update_inputs(
            "http://updates.aerotune.net/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
        for endpoint in [
            "https://updates.aerotune.net:8443/latest.json",
            "https://updates.aerotune.net/latest.json?channel=stable",
            "https://updates.aerotune.net/latest.json#release",
            "https://updates.aerotune.net./latest.json",
            "https://updates.aerotune.local/latest.json",
            "https://updates.aerotune.internal/latest.json",
            "https://updates.example.net/latest.json",
            "https://updates/latest.json",
        ] {
            assert!(
                validate_update_inputs(endpoint, VALID_TEST_TAURI_PUBLIC_KEY, true).is_err(),
                "unsafe feed endpoint must be rejected: {endpoint}"
            );
        }
        assert!(validate_update_inputs(
            "https://updates.aerotune.net:443/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_ok());
        assert!(validate_update_inputs(
            "https://updates.aerotune.net/latest.json",
            "REPLACE_WITH_PUBLIC_KEY",
            true
        )
        .is_err());
        assert!(validate_update_inputs(
            "https://updates.aerotune.net/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            false
        )
        .is_err());
    }

    #[test]
    fn missing_feed_or_public_key_fails_closed_without_network_access() {
        let no_feed = validate_runtime_update_inputs(
            None,
            Some(VALID_TEST_TAURI_PUBLIC_KEY),
            true,
            true,
            true,
            true,
            true,
            true,
        )
        .expect_err("missing feed URL must disable updater inputs");
        assert!(no_feed.contains("安全更新源尚未配置"));

        let no_public_key = validate_runtime_update_inputs(
            Some("https://updates.aerotune.net/latest.json"),
            None,
            true,
            true,
            true,
            true,
            true,
            true,
        )
        .expect_err("missing public key must disable updater inputs");
        assert!(no_public_key.contains("Tauri 更新公钥尚未配置"));
    }

    #[test]
    fn dns_destination_gate_blocks_configured_feed_until_policy_is_approved() {
        let endpoint = Some("https://updates.aerotune.net/latest.json");
        let public_key = Some(VALID_TEST_TAURI_PUBLIC_KEY);
        let feed_error = validate_runtime_update_inputs(
            endpoint, public_key, false, true, true, true, true, true,
        )
        .expect_err("unreviewed feed-size cap must keep updates disabled");
        assert!(feed_error.contains("更新清单大小上限尚未通过 release review"));
        let dns_error = validate_runtime_update_inputs(
            endpoint, public_key, true, false, true, true, true, true,
        )
        .expect_err("an unreviewed DNS destination policy must keep updates disabled");
        assert!(dns_error.contains("DNS 目的地址策略尚未通过审核"));
        let proxy_error = validate_runtime_update_inputs(
            endpoint, public_key, true, true, false, true, true, true,
        )
        .expect_err("unreviewed system-proxy routing must keep updates disabled");
        assert!(proxy_error.contains("系统代理保留策略尚未通过 VM 验证"));
        let download_error = validate_runtime_update_inputs(
            endpoint, public_key, true, true, true, false, true, true,
        )
        .expect_err("unbounded artifact handling must keep updates disabled");
        assert!(download_error.contains("有界签名下载和受保护暂存尚未完成"));
        let task_error = validate_runtime_update_inputs(
            endpoint, public_key, true, true, true, true, false, true,
        )
        .expect_err("unattested task configuration must keep updates disabled");
        assert!(task_error.contains("固定更新任务定义和访问控制尚未通过运行时验收"));
        assert!(validate_runtime_update_inputs(
            endpoint, public_key, true, true, true, true, true, false,
        )
        .is_err());
        assert!(validate_runtime_update_inputs(
            endpoint, public_key, true, true, true, true, true, true,
        )
        .is_ok());
    }

    #[test]
    fn readiness_stays_disabled_when_security_prerequisites_are_not_ready() {
        let readiness = get_signed_update_readiness();
        assert!(!readiness.enabled);
        assert!(!readiness.task_registered);
        assert!(readiness.reason.is_some());
    }

    #[test]
    fn update_inputs_accept_only_configured_https_without_credentials() {
        assert!(validate_update_inputs(
            "https://updates.aerotune.net/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_ok());
        assert!(validate_update_inputs(
            "https://user:pass@updates.aerotune.net/latest.json",
            VALID_TEST_TAURI_PUBLIC_KEY,
            true
        )
        .is_err());
    }

    #[test]
    fn artifact_downloads_require_public_https_without_credentials() {
        assert!(validate_artifact_url(
            &Url::parse("https://cdn.aerotune.net/aerotune.nsis.zip").unwrap()
        )
        .is_ok());
        assert!(validate_artifact_url(
            &Url::parse("https://cdn.aerotune.net/aerotune.nsis.zip?token=signed-value").unwrap()
        )
        .is_ok());
        for artifact in [
            "https://cdn.aerotune.net:8443/aerotune.nsis.zip",
            "https://cdn.aerotune.net./aerotune.nsis.zip",
            "https://cdn.aerotune.local/aerotune.nsis.zip",
            "https://cdn.aerotune.internal/aerotune.nsis.zip",
            "https://updates.example.net/aerotune.nsis.zip",
            "https://cdn/aerotune.nsis.zip",
            "https://cdn.aerotune.net/aerotune.nsis.zip#installer",
        ] {
            assert!(
                validate_artifact_url(&Url::parse(artifact).unwrap()).is_err(),
                "unsafe artifact URL must be rejected: {artifact}"
            );
        }
        assert!(validate_artifact_url(
            &Url::parse("http://cdn.aerotune.net/aerotune.nsis.zip").unwrap()
        )
        .is_err());
        assert!(validate_artifact_url(
            &Url::parse("https://user:pass@cdn.aerotune.net/aerotune.nsis.zip").unwrap()
        )
        .is_err());
        assert!(
            validate_artifact_url(&Url::parse("https://localhost/aerotune.nsis.zip").unwrap())
                .is_err()
        );
        assert!(validate_artifact_url(
            &Url::parse("https://localhost./aerotune.nsis.zip").unwrap()
        )
        .is_err());
        assert!(validate_artifact_url(
            &Url::parse("https://updates.localhost./aerotune.nsis.zip").unwrap()
        )
        .is_err());
        assert!(
            validate_artifact_url(&Url::parse("https://127.0.0.1/aerotune.nsis.zip").unwrap())
                .is_err()
        );
        assert!(
            validate_artifact_url(&Url::parse("https://[::1]/aerotune.nsis.zip").unwrap()).is_err()
        );
        for artifact in [
            "https://2130706433/aerotune.nsis.zip",
            "https://0x7f000001/aerotune.nsis.zip",
            "https://0177.0.0.1/aerotune.nsis.zip",
            "https://127.1/aerotune.nsis.zip",
            "https://127.0.0.1./aerotune.nsis.zip",
        ] {
            let parsed = Url::parse(artifact).unwrap();
            assert!(
                validate_artifact_url(&parsed).is_err(),
                "non-canonical IP artifact URL must be rejected: {artifact}"
            );
        }
    }

    #[test]
    fn updater_redirects_are_bounded_and_remain_https_dns_urls() {
        let origin = Url::parse("https://updates.aerotune.net/latest.json").unwrap();
        let cdn = Url::parse("https://cdn.aerotune.net/aerotune.nsis.zip").unwrap();
        assert!(updater_redirect_is_safe(
            std::slice::from_ref(&origin),
            &cdn
        ));
        assert!(updater_redirect_is_safe(
            &vec![origin.clone(); MAX_UPDATER_REDIRECT_HOPS],
            &cdn
        ));
        assert!(!updater_redirect_is_safe(&[], &cdn));
        assert!(!updater_redirect_is_safe(
            &vec![origin.clone(); MAX_UPDATER_REDIRECT_HOPS + 1],
            &cdn
        ));
        let insecure_previous = Url::parse("http://updates.aerotune.net/latest.json").unwrap();
        assert!(!updater_redirect_is_safe(&[insecure_previous], &cdn));
        for target in [
            "http://cdn.aerotune.net/aerotune.nsis.zip",
            "https://user:pass@cdn.aerotune.net/aerotune.nsis.zip",
            "https://127.0.0.1/aerotune.nsis.zip",
            "https://2130706433/aerotune.nsis.zip",
            "https://127.0.0.1./aerotune.nsis.zip",
            "https://localhost/aerotune.nsis.zip",
            "https://localhost./aerotune.nsis.zip",
            "https://updates.localhost./aerotune.nsis.zip",
        ] {
            let target = Url::parse(target).unwrap();
            assert!(
                !updater_redirect_is_safe(std::slice::from_ref(&origin), &target),
                "unsafe updater redirect must be rejected: {target}"
            );
        }
    }

    #[test]
    fn updater_http_redirect_is_blocked_by_the_configured_client_policy() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/redirected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
        });

        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = {
            let _runtime_guard = runtime.enter();
            configure_updater_client(reqwest::Client::builder())
                .no_proxy()
                .build()
                .unwrap()
        };
        let error = runtime
            .block_on(async { client.get(format!("http://{address}/start")).send().await })
            .expect_err("the custom policy must reject the HTTP redirect");
        server.join().unwrap();
        assert!(
            error.is_redirect(),
            "expected a redirect-policy error, got: {error}"
        );
    }

    fn spawn_update_response_server(
        response: &'static [u8],
    ) -> (String, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};

        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::ring::default_provider().install_default();
        }
        use std::net::TcpListener;

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(response);
        });
        (format!("http://{address}/artifact"), server)
    }

    fn fake_pe_installer() -> Vec<u8> {
        let mut bytes = vec![0u8; 128];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..68].copy_from_slice(&[b'P', b'E', 0, 0]);
        let machine = if cfg!(target_arch = "x86_64") {
            0x8664u16
        } else if cfg!(target_arch = "aarch64") {
            0xaa64u16
        } else {
            0x014cu16
        };
        bytes[68..70].copy_from_slice(&machine.to_le_bytes());
        bytes
    }

    fn make_test_update_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;

        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn make_test_stored_update_zip(name: &str, content: &[u8]) -> Vec<u8> {
        use std::io::Write;

        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(content).unwrap();
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn update_archive_rejects_crc_mismatch_after_reading_member_to_eof() {
        let installer = fake_pe_installer();
        let mut archive = make_test_stored_update_zip("AeroTune_1.5.9_x64-setup.exe", &installer);
        let offset = archive
            .windows(installer.len())
            .position(|window| window == installer)
            .unwrap();
        archive[offset + 96] ^= 1;
        let error = validate_update_archive_with_limits(&archive, "1.5.9", 1024, 4096, 1)
            .expect_err("member CRC mismatch must be rejected after draining to EOF");
        assert!(error.contains("CRC 校验失败"));
    }

    #[test]
    fn update_archive_allows_one_safe_root_installer_with_expanded_limit_and_matching_pe() {
        let installer = fake_pe_installer();
        let archive = make_test_update_zip(&[("AeroTune_1.5.9_x64-setup.exe", &installer)]);
        validate_update_archive_with_limits(&archive, "1.5.9", 1024, 1024, 1).unwrap();
    }

    #[test]
    fn update_archive_rejects_invalid_pe_headers_and_wrong_machine() {
        let mut cases = Vec::new();
        let mut invalid_dos = fake_pe_installer();
        invalid_dos[0] = b'X';
        cases.push((invalid_dos, "有效的 Windows PE"));

        let mut invalid_offset = fake_pe_installer();
        invalid_offset[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
        cases.push((invalid_offset, "PE 签名"));

        let mut invalid_signature = fake_pe_installer();
        invalid_signature[64] = b'X';
        cases.push((invalid_signature, "PE 签名"));

        let mut wrong_machine = fake_pe_installer();
        let machine = if cfg!(target_arch = "x86_64") {
            0xaa64u16
        } else {
            0x8664u16
        };
        wrong_machine[68..70].copy_from_slice(&machine.to_le_bytes());
        cases.push((wrong_machine, "PE 架构与当前程序不一致"));

        for (installer, expected) in cases {
            let archive = make_test_update_zip(&[("AeroTune_1.5.9_x64-setup.exe", &installer)]);
            let error = validate_update_archive_with_limits(&archive, "1.5.9", 4096, 4096, 1)
                .expect_err("invalid PE headers and architecture must be rejected");
            assert!(
                error.contains(expected),
                "unexpected PE validation error: {error}"
            );
        }
    }

    #[test]
    fn update_archive_rejects_msi_non_setup_and_wrong_product_executables() {
        for name in [
            "AeroTune_1.5.9_x64.msi",
            "AeroTune.exe",
            "AeroTuneEvil_1.5.9_x64-setup.exe",
        ] {
            let fake = fake_pe_installer();
            let archive = make_test_update_zip(&[(name, &fake)]);
            let error = validate_update_archive_with_limits(&archive, "1.5.9", 1024, 1024, 1)
                .expect_err("per-machine release policy requires a named NSIS setup executable");
            assert!(error.contains("NSIS -setup.exe"));
        }
    }

    #[test]
    fn update_archive_rejects_version_mismatch_and_wrong_architecture() {
        let fake = fake_pe_installer();
        let wrong_version = make_test_update_zip(&[("AeroTune_1.5.8_x64-setup.exe", &fake)]);
        let error = validate_update_archive_with_limits(&wrong_version, "1.5.9", 1024, 1024, 1)
            .unwrap_err();
        assert!(error.contains("文件名版本与签名更新版本不一致"));

        let wrong_arch = "AeroTune_1.5.9_arm64-setup.exe";
        let installer = fake_pe_installer();
        let archive = make_test_update_zip(&[(wrong_arch, &installer)]);
        let error =
            validate_update_archive_with_limits(&archive, "1.5.9", 1024, 1024, 1).unwrap_err();
        assert!(error.contains("架构与当前程序不一致"));
    }

    #[test]
    fn update_archive_rejects_path_traversal_and_multiple_entries() {
        let traversal = make_test_update_zip(&[("../AeroTune.exe", b"installer")]);
        assert!(validate_update_archive_with_limits(&traversal, "1.5.9", 1024, 1024, 1).is_err());
        let multiple = make_test_update_zip(&[
            ("AeroTune_1.5.9_x64-setup.exe", &fake_pe_installer()),
            ("Other_1.5.9_x64-setup.exe", &fake_pe_installer()),
        ]);
        assert!(validate_update_archive_with_limits(&multiple, "1.5.9", 1024, 1024, 1).is_err());
        let wrong_product = make_test_update_zip(&[("SomethingElse.exe", b"installer")]);
        assert!(
            validate_update_archive_with_limits(&wrong_product, "1.5.9", 1024, 1024, 1).is_err()
        );
    }

    #[test]
    fn update_archive_rejects_symlink_entries() {
        let cursor = std::io::Cursor::new(Vec::new());
        let mut writer = zip::ZipWriter::new(cursor);
        writer
            .add_symlink(
                "AeroTune_1.5.9_x64-setup.exe",
                "C:\\outside.exe",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        let archive = writer.finish().unwrap().into_inner();
        let error = validate_update_archive_with_limits(&archive, "1.5.9", 1024, 1024, 1)
            .expect_err("installer archives must never contain symlink entries");
        assert!(error.contains("链接"));
    }

    #[test]
    fn update_archive_rejects_expanded_size_over_limit() {
        let archive = make_test_update_zip(&[("AeroTune_1.5.9_x64-setup.exe", &[b'A'; 256])]);
        let error = validate_update_archive_with_limits(&archive, "1.5.9", 1024, 128, 1)
            .expect_err("expanded-size cap must apply regardless of compression ratio");
        assert!(error.contains("展开字节上限"));
    }

    #[test]
    fn bounded_artifact_accepts_body_exactly_at_the_byte_limit() {
        let (url, server) = spawn_update_response_server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345",
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = {
            let _runtime_guard = runtime.enter();
            reqwest::Client::builder().no_proxy().build().unwrap()
        };
        let bytes = runtime
            .block_on(async {
                let response = client.get(url).send().await.unwrap();
                collect_bounded_update_response(response, 5).await
            })
            .unwrap();
        server.join().unwrap();
        assert_eq!(bytes, b"12345");
    }

    #[test]
    fn bounded_artifact_rejects_oversized_content_length_before_reading_body() {
        let (url, server) = spawn_update_response_server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\n123456",
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = {
            let _runtime_guard = runtime.enter();
            reqwest::Client::builder().no_proxy().build().unwrap()
        };
        let error = runtime
            .block_on(async {
                let response = client.get(url).send().await.unwrap();
                collect_bounded_update_response(response, 5).await
            })
            .expect_err("known oversized body must be rejected before buffering");
        server.join().unwrap();
        assert!(error.contains("超过配置的字节上限"));
    }

    #[test]
    fn bounded_artifact_rejects_chunked_body_over_limit_without_content_length() {
        let (url, server) = spawn_update_response_server(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n6\r\n123456\r\n0\r\n\r\n",
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = {
            let _runtime_guard = runtime.enter();
            reqwest::Client::builder().no_proxy().build().unwrap()
        };
        let error = runtime
            .block_on(async {
                let response = client.get(url).send().await.unwrap();
                assert_eq!(response.content_length(), None);
                collect_bounded_update_response(response, 5).await
            })
            .expect_err("unknown-length stream must be rejected at the streaming cap");
        server.join().unwrap();
        assert!(error.contains("超过配置的字节上限"));
    }

    #[test]
    fn bounded_artifact_rejects_non_success_http_status() {
        let (url, server) = spawn_update_response_server(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad",
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = {
            let _runtime_guard = runtime.enter();
            reqwest::Client::builder().no_proxy().build().unwrap()
        };
        let error = runtime
            .block_on(async {
                let response = client.get(url).send().await.unwrap();
                collect_bounded_update_response(response, 5).await
            })
            .expect_err("non-success HTTP responses must not be treated as artifacts");
        server.join().unwrap();
        assert!(error.contains("返回失败状态"));
    }

    #[test]
    fn bounded_artifact_rejects_truncated_content_length_response() {
        let (url, server) = spawn_update_response_server(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nabc",
        );
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let client = {
            let _runtime_guard = runtime.enter();
            reqwest::Client::builder().no_proxy().build().unwrap()
        };
        let error = runtime
            .block_on(async {
                let response = client.get(url).send().await.unwrap();
                collect_bounded_update_response(response, 5).await
            })
            .expect_err("truncated bodies must fail closed");
        server.join().unwrap();
        assert!(error.contains("流读取失败"));
    }

    #[test]
    fn versions_must_be_valid_semver_and_strictly_newer() {
        assert_eq!(
            ensure_newer_version("1.5.9", "1.5.10").unwrap(),
            semver::Version::new(1, 5, 10)
        );
        assert!(ensure_newer_version("1.5.9", "1.5.9").is_err());
        assert!(ensure_newer_version("1.5.9", "1.5.8").is_err());
        assert!(ensure_newer_version("1.5.9", "not-a-version").is_err());
        assert!(ensure_newer_version("1.5.9", "1.05.10").is_err());
        assert!(ensure_newer_version("1.5.9+build.1", "1.5.9+build.2").is_err());
        assert!(ensure_newer_version("1.5.9", "1.5.9+build.2").is_err());
        assert!(ensure_newer_version("1.5.9+build.2", "1.5.9").is_err());
        assert!(ensure_newer_version("1.5.9-rc.2", "1.5.9-rc.1").is_err());
        assert_eq!(
            ensure_newer_version("1.5.9-rc.1", "1.5.9").unwrap(),
            semver::Version::new(1, 5, 9)
        );
        assert!(ensure_newer_version("1.5.9", "1.5.10-rc.1").is_ok());
    }

    #[cfg(windows)]
    static UPDATE_GATE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[cfg(windows)]
    fn test_update_gate_name() -> String {
        format!(
            "Local\\AeroTune.Test.SignedUpdateGate.{}",
            std::process::id()
        )
    }

    #[cfg(windows)]
    #[test]
    fn named_update_gate_excludes_other_threads_and_releases_cleanly() {
        let _test_lock = UPDATE_GATE_TEST_LOCK.lock().unwrap();
        let gate_name = test_update_gate_name();
        let guard = acquire_named_update_gate(&gate_name, 0).unwrap();
        let waiter_gate_name = gate_name.clone();
        let blocked =
            std::thread::spawn(move || acquire_named_update_gate(&waiter_gate_name, 0).is_err())
                .join()
                .unwrap();
        assert!(blocked);
        drop(guard);
        assert!(acquire_named_update_gate(&gate_name, 0).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn abandoned_update_gate_rejects_and_propagates_to_queued_waiters() {
        use windows_sys::Win32::Foundation::{HANDLE, WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{
            CreateMutexW, ReleaseMutex, WaitForSingleObject,
        };

        let _test_lock = UPDATE_GATE_TEST_LOCK.lock().unwrap();
        let gate_name = test_update_gate_name();
        let name: Vec<u16> = gate_name.encode_utf16().chain(std::iter::once(0)).collect();
        let anchor = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        assert!(!anchor.is_null());
        let abandoned_owner_handle = std::thread::spawn(move || unsafe {
            let handle = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
            assert!(!handle.is_null());
            assert_eq!(WaitForSingleObject(handle, 0), WAIT_OBJECT_0);
            handle as usize
        })
        .join()
        .unwrap();

        let first_gate_name = gate_name.clone();
        let first_waiter_error = std::thread::spawn(move || {
            acquire_named_update_gate(&first_gate_name, 0)
                .err()
                .unwrap()
        })
        .join()
        .unwrap();
        let second_gate_name = gate_name;
        let second_waiter_error = std::thread::spawn(move || {
            acquire_named_update_gate(&second_gate_name, 0)
                .err()
                .unwrap()
        })
        .join()
        .unwrap();
        assert!(first_waiter_error.contains("恢复状态未验证"));
        assert!(second_waiter_error.contains("恢复状态未验证"));
        // Neither waiter released normally: the anchor sees the propagated abandonment too.
        assert_eq!(unsafe { WaitForSingleObject(anchor, 0) }, WAIT_ABANDONED);
        unsafe {
            ReleaseMutex(anchor);
            windows_sys::Win32::Foundation::CloseHandle(abandoned_owner_handle as HANDLE);
            windows_sys::Win32::Foundation::CloseHandle(anchor);
        }
    }

    #[test]
    fn update_only_argument_is_fixed() {
        assert_eq!(UPDATE_ONLY_ARGUMENT, "--aerotune-update-only");
    }
}
