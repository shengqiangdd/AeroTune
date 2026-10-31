use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;
use url::Url;

// Keep the standard updater disabled until transport bounds, host policy, and Windows acceptance pass.
const UPDATE_FEED_RESPONSE_BOUNDED_READY: bool = true;
const UPDATE_PROXY_POLICY_READY: bool = true;
const UPDATE_BOUNDED_DOWNLOAD_READY: bool = true;
const GITHUB_RELEASE_HOST_POLICY_REVIEWED: bool = true;
// Owner-approved early access; cancellation, concurrent-install, and repair cases remain unverified.
const GITHUB_RELEASE_WINDOWS_ACCEPTED: bool = true;
const UPDATE_ENDPOINT: Option<&str> = option_env!("AEROTUNE_UPDATE_ENDPOINT");
const UPDATE_PUBLIC_KEY: Option<&str> = option_env!("AEROTUNE_UPDATE_PUBLIC_KEY");
const GITHUB_RELEASE_ENDPOINT: &str =
    "https://github.com/shengqiangdd/AeroTune/releases/latest/download/latest.json";
const GITHUB_RELEASE_DOWNLOAD_PATH: &str = "/shengqiangdd/AeroTune/releases/download/";
const GITHUB_RELEASE_LATEST_PATH: &str =
    "/shengqiangdd/AeroTune/releases/latest/download/latest.json";
const UPDATE_GATE_NAME: &str = "Global\\AeroTune.SignedUpdateGate";

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
        // The prior process may have terminated while holding the launch/update gate. Fail closed
        // so this process cannot race any remaining installer work; a later manual launch can retry.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(handle);
        }
        return Err("AeroTune 更新互斥锁的前一持有者异常退出；已拒绝并发启动。".into());
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
    acquire_update_gate(0)
}

#[derive(Debug, Serialize)]
pub struct UpdateReadiness {
    pub enabled: bool,
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
        return Err("更新功能尚未通过发布安全验收。".into());
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

fn is_github_release_endpoint(url: &Url) -> bool {
    normalized_url_host(url) == "github.com"
        && url.path() == GITHUB_RELEASE_LATEST_PATH
        && url.query().is_none()
        && url.fragment().is_none()
}

fn validate_github_release_inputs(
    endpoint: Option<&str>,
    public_key: Option<&str>,
) -> Result<(Url, String), String> {
    let endpoint = endpoint.unwrap_or(GITHUB_RELEASE_ENDPOINT).trim();
    let public_key = public_key.ok_or_else(|| "Tauri 更新公钥尚未配置。".to_string())?;
    let (endpoint, public_key) = validate_update_inputs(endpoint, public_key, true)?;
    if !is_github_release_endpoint(&endpoint) {
        return Err("更新源必须是固定的 AeroTune GitHub Releases latest.json 地址。".into());
    }
    Ok((endpoint, public_key))
}

fn configured_github_release_inputs() -> Result<(Url, String), String> {
    validate_github_release_inputs(UPDATE_ENDPOINT, UPDATE_PUBLIC_KEY)
}

fn github_release_update_readiness_error() -> Option<String> {
    if !UPDATE_FEED_RESPONSE_BOUNDED_READY {
        return Some("GitHub 更新清单大小限制尚未完成 release review。".into());
    }
    if !GITHUB_RELEASE_HOST_POLICY_REVIEWED {
        return Some("GitHub HTTPS 主机与重定向策略尚未完成 release review。".into());
    }
    if !UPDATE_PROXY_POLICY_READY {
        return Some("Windows 系统代理行为尚未通过本机实测。".into());
    }
    if !UPDATE_BOUNDED_DOWNLOAD_READY {
        return Some("更新包下载/解压上限尚未完成测量和 Windows 验证。".into());
    }
    if !GITHUB_RELEASE_WINDOWS_ACCEPTED {
        return Some(
            "标准用户 UAC、取消、中断/并发安装和修复行为尚未通过本机 Windows 安装验收。".into(),
        );
    }
    None
}

#[tauri::command]
pub fn get_signed_update_readiness() -> UpdateReadiness {
    match configured_github_release_inputs() {
        Ok(_) => match github_release_update_readiness_error() {
            Some(reason) => UpdateReadiness {
                enabled: false,
                reason: Some(reason),
            },
            None => UpdateReadiness {
                enabled: true,
                reason: None,
            },
        },
        Err(reason) => UpdateReadiness {
            enabled: false,
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

fn github_redirect_url_is_allowed(url: &Url) -> bool {
    if validate_https_dns_url(url, true).is_err() {
        return false;
    }
    match normalized_url_host(url).as_str() {
        "github.com" => {
            url.path() == GITHUB_RELEASE_LATEST_PATH
                || url.path().starts_with(GITHUB_RELEASE_DOWNLOAD_PATH)
        }
        "release-assets.githubusercontent.com" | "objects.githubusercontent.com" => {
            !url.path().is_empty()
        }
        _ => false,
    }
}

fn github_release_asset_url_is_allowed(url: &Url) -> bool {
    let host = normalized_url_host(url);
    let github_asset = host == "github.com" && url.path().starts_with(GITHUB_RELEASE_DOWNLOAD_PATH);
    validate_https_dns_url(url, true).is_ok()
        && github_asset
        && url.path().ends_with(".nsis.zip")
        && url.query().is_none()
        && url.fragment().is_none()
}

fn validate_github_release_asset_url(url: &Url) -> Result<(), String> {
    if github_release_asset_url_is_allowed(url) {
        Ok(())
    } else {
        Err(
            "更新工件必须是 AeroTune GitHub Releases 中的 HTTPS NSIS 更新包及其官方资产 URL。"
                .into(),
        )
    }
}

const MAX_UPDATER_REDIRECT_HOPS: usize = 3;
const UPDATER_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
// Prototype ceiling only; not release-approved until measured against the signed target artifact.
const PROPOSED_MAX_UPDATE_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;
const PROPOSED_MAX_UPDATE_EXPANDED_BYTES: u64 = 128 * 1024 * 1024;
const MAX_UPDATE_ARCHIVE_ENTRIES: usize = 1;

fn github_updater_redirect_is_safe(previous_urls: &[Url], next_url: &Url) -> bool {
    !previous_urls.is_empty()
        && previous_urls.len() <= MAX_UPDATER_REDIRECT_HOPS
        && previous_urls.iter().all(github_redirect_url_is_allowed)
        && github_redirect_url_is_allowed(next_url)
}

fn github_updater_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if github_updater_redirect_is_safe(attempt.previous(), attempt.url()) {
            attempt.follow()
        } else {
            attempt.error("redirect blocked by AeroTune GitHub release policy")
        }
    })
}

fn configure_github_updater_client(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    builder
        .timeout(UPDATER_REQUEST_TIMEOUT)
        .redirect(github_updater_redirect_policy())
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
    // NSIS setup.exe is commonly a 32-bit PE bootstrapper even for x64/ARM64 bundles.
    // The signed update archive and the target-specific *_x64/_arm64 filename are validated
    // separately, so accept the x86 launcher while still rejecting other mismatched machines.
    if machine != expected_machine && machine != 0x014c {
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
}

impl VerifiedUpdateArtifact {
    fn new(bytes: Vec<u8>, target_version: semver::Version) -> Self {
        Self {
            bytes,
            target_version,
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn matches_target_version(&self, version: &str) -> bool {
        self.target_version.to_string() == version
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

fn build_github_bounded_update_request(
    update: &tauri_plugin_updater::Update,
) -> Result<reqwest::RequestBuilder, String> {
    validate_system_proxy_policy(update.no_proxy, update.proxy.is_some())?;
    validate_github_release_asset_url(&update.download_url)?;
    validate_artifact_request_headers(&update.headers)?;
    let mut builder =
        reqwest::Client::builder().user_agent(concat!("AeroTune/", env!("CARGO_PKG_VERSION")));
    if let Some(timeout) = update.timeout {
        builder = builder.timeout(timeout);
    }
    let client = configure_github_updater_client(builder)
        .build()
        .map_err(|_| "无法创建 GitHub Releases 更新下载客户端。".to_string())?;
    Ok(client
        .get(update.download_url.clone())
        .headers(update.headers.clone()))
}

async fn download_and_verify_github_update(
    update: &tauri_plugin_updater::Update,
) -> Result<VerifiedUpdateArtifact, String> {
    if let Some(reason) = github_release_update_readiness_error() {
        return Err(reason);
    }
    validate_github_release_asset_url(&update.download_url)?;
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| "本机版本格式无效。".to_string())?;
    ensure_newer_version(&current.to_string(), &update.version)?;
    let response = build_github_bounded_update_request(update)?
        .send()
        .await
        .map_err(|_| "GitHub Releases 更新工件下载失败；已拒绝更新。".to_string())?;
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
    let (endpoint, public_key) = configured_github_release_inputs()?;
    app.updater_builder()
        .pubkey(public_key)
        .endpoints(vec![endpoint])
        .map_err(|_| "GitHub Releases 更新源配置无效。".to_string())?
        .configure_client(configure_github_updater_client)
        .restart_after_install(false)
        .build()
        .map_err(|_| "GitHub Releases 更新器初始化失败。".to_string())
}

#[tauri::command]
pub async fn check_signed_update(app: AppHandle) -> Result<Option<AvailableUpdate>, String> {
    let readiness = get_signed_update_readiness();
    if !readiness.enabled {
        return Err(readiness
            .reason
            .unwrap_or_else(|| "GitHub Releases 更新功能不可用。".into()));
    }
    let Some(update) = build_updater(&app)?
        .check()
        .await
        .map_err(|_| "无法检查 GitHub Releases 更新。".to_string())?
    else {
        return Ok(None);
    };
    validate_github_release_asset_url(&update.download_url)?;
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|_| "本机版本格式无效。".to_string())?;
    ensure_newer_version(&current.to_string(), &update.version)?;
    Ok(Some(AvailableUpdate {
        current_version: current.to_string(),
        available_version: update.version,
    }))
}

#[cfg(windows)]
#[tauri::command]
pub async fn request_signed_update(app: AppHandle, expected_version: String) -> Result<(), String> {
    let readiness = get_signed_update_readiness();
    if !readiness.enabled {
        return Err(readiness
            .reason
            .unwrap_or_else(|| "GitHub Releases 更新功能不可用。".into()));
    }
    let Some(update) = build_updater(&app)?
        .check()
        .await
        .map_err(|_| "无法重新验证 GitHub Releases 更新。".to_string())?
    else {
        return Err("GitHub Releases 中已没有待安装的新版本；请重新检查更新。".into());
    };
    if update.version != expected_version {
        return Err("GitHub Releases 的版本已变化；请重新检查后再安装。".into());
    }
    let artifact = download_and_verify_github_update(&update).await?;
    if !artifact.matches_target_version(&update.version) {
        return Err("已验证的更新工件与当前确认的版本不匹配；已拒绝安装。".into());
    }
    update
        .install(artifact.bytes())
        .map_err(|_| "已验证的更新未能启动 Windows 安装器。".to_string())
}

#[cfg(not(windows))]
#[tauri::command]
pub async fn request_signed_update(
    _app: AppHandle,
    _expected_version: String,
) -> Result<(), String> {
    Err("安全更新仅适用于 Windows。".into())
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
    fn verified_artifact_binds_exact_bytes_and_target_version() {
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
        assert!(artifact.matches_target_version("1.5.10"));
        assert!(!artifact.matches_target_version("1.5.11"));
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
    fn failed_archive_check_never_produces_verified_artifact() {
        let result = verify_and_wrap_update_artifact(
            b"signed but invalid archive".to_vec(),
            "1.5.10",
            |_| Ok(()),
            |_, _| Err("invalid archive".into()),
        );
        assert!(result.is_err());
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
    fn readiness_stays_disabled_without_a_real_release_public_key() {
        let readiness = get_signed_update_readiness();
        assert!(!readiness.enabled);
        assert!(readiness.reason.is_some());
    }

    #[test]
    fn github_updater_release_gates_are_owner_accepted_for_early_access() {
        assert!(github_release_update_readiness_error().is_none());
        assert!(UPDATE_FEED_RESPONSE_BOUNDED_READY);
        assert!(UPDATE_PROXY_POLICY_READY);
        assert!(UPDATE_BOUNDED_DOWNLOAD_READY);
        assert!(GITHUB_RELEASE_HOST_POLICY_REVIEWED);
        assert!(GITHUB_RELEASE_WINDOWS_ACCEPTED);
    }

    #[test]
    fn github_release_inputs_require_the_exact_public_feed_and_valid_public_key() {
        assert!(validate_github_release_inputs(
            Some(GITHUB_RELEASE_ENDPOINT),
            Some(VALID_TEST_TAURI_PUBLIC_KEY),
        )
        .is_ok());
        assert!(validate_github_release_inputs(None, Some(VALID_TEST_TAURI_PUBLIC_KEY)).is_ok());
        assert!(validate_github_release_inputs(Some(GITHUB_RELEASE_ENDPOINT), None).is_err());
        for endpoint in [
            "https://github.com/attacker/AeroTune/releases/latest/download/latest.json",
            "https://github.com/shengqiangdd/Other/releases/latest/download/latest.json",
            "https://github.com/shengqiangdd/AeroTune/releases/latest/download/latest.json?token=x",
            "http://github.com/shengqiangdd/AeroTune/releases/latest/download/latest.json",
            "https://api.github.com/repos/shengqiangdd/AeroTune/releases/latest",
        ] {
            assert!(
                validate_github_release_inputs(Some(endpoint), Some(VALID_TEST_TAURI_PUBLIC_KEY))
                    .is_err(),
                "non-canonical GitHub feed must be rejected: {endpoint}"
            );
        }
    }

    #[test]
    fn github_release_artifacts_and_redirects_are_host_and_path_allowlisted() {
        for url in [
            "https://github.com/shengqiangdd/AeroTune/releases/download/v1.5.10/AeroTune_1.5.10_x64-setup.nsis.zip",
        ] {
            assert!(
                validate_github_release_asset_url(&Url::parse(url).unwrap()).is_ok(),
                "expected approved GitHub release URL: {url}"
            );
        }
        for url in [
            GITHUB_RELEASE_ENDPOINT,
            "http://github.com/shengqiangdd/AeroTune/releases/download/v1.5.10/aerotune.nsis.zip",
            "https://github.com/attacker/AeroTune/releases/download/v1.5.10/aerotune.nsis.zip",
            "https://github.com/shengqiangdd/AeroTune/archive/refs/tags/v1.5.10.zip",
            "https://release-assets.githubusercontent.com/github-production-release-asset/abc/AeroTune.nsis.zip?token=x",
            "https://evil.example/aerotune.nsis.zip",
            "https://release-assets.githubusercontent.com.evil.example/file",
            "https://user:pass@release-assets.githubusercontent.com/file",
            "https://release-assets.githubusercontent.com:8443/file",
        ] {
            assert!(
                validate_github_release_asset_url(&Url::parse(url).unwrap()).is_err(),
                "unsafe GitHub release URL must be rejected: {url}"
            );
        }
        let github = Url::parse(GITHUB_RELEASE_ENDPOINT).unwrap();
        let asset_cdn =
            Url::parse("https://release-assets.githubusercontent.com/asset?token=x").unwrap();
        assert!(github_updater_redirect_is_safe(
            std::slice::from_ref(&github),
            &asset_cdn
        ));
        assert!(!github_updater_redirect_is_safe(
            &vec![github.clone(); MAX_UPDATER_REDIRECT_HOPS + 1],
            &asset_cdn,
        ));
        let attacker = Url::parse("https://evil.example/asset").unwrap();
        assert!(!github_updater_redirect_is_safe(
            std::slice::from_ref(&github),
            &attacker
        ));
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
    fn github_updater_http_redirect_is_blocked_by_the_configured_client_policy() {
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
            configure_github_updater_client(reqwest::Client::builder())
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
    fn update_archive_allows_x86_nsis_bootstrap_for_x64_bundle() {
        if !cfg!(target_arch = "x86_64") {
            return;
        }
        let mut installer = fake_pe_installer();
        installer[68..70].copy_from_slice(&0x014cu16.to_le_bytes());
        let archive = make_test_update_zip(&[("AeroTune_1.5.9_x64-setup.exe", &installer)]);
        validate_update_archive_with_limits(&archive, "1.5.9", 4096, 4096, 1).unwrap();
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
        assert!(first_waiter_error.contains("前一持有者异常退出"));
        assert!(second_waiter_error.contains("前一持有者异常退出"));
        // Neither waiter released normally: the anchor sees the propagated abandonment too.
        assert_eq!(unsafe { WaitForSingleObject(anchor, 0) }, WAIT_ABANDONED);
        unsafe {
            ReleaseMutex(anchor);
            windows_sys::Win32::Foundation::CloseHandle(abandoned_owner_handle as HANDLE);
            windows_sys::Win32::Foundation::CloseHandle(anchor);
        }
    }
}
