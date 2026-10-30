// 进程画像引擎：用纯本地静态证据（路径、已知名单、自启来源、数字签名）给进程分类。
// 只做只读分类与展示，不联网查询、不驱动任何自动处置；分类可能出错，动作始终由用户决定。
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

pub const ROLE_SELF: &str = "self";
pub const ROLE_SYSTEM_CORE: &str = "system-core";
pub const ROLE_WINDOWS_BACKGROUND: &str = "windows-background";
pub const ROLE_AUTOSTART: &str = "autostart";
pub const ROLE_UPDATER_SUSPECT: &str = "updater-suspect";
pub const ROLE_USER_APP: &str = "user-app";
pub const ROLE_UNKNOWN: &str = "unknown";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ProcessRole {
    pub id: &'static str,
    pub label: &'static str,
    pub detail: String,
}

const UPDATER_NAME_MARKERS: &[&str] = &["update", "setup", "install", "upgrade"];

fn is_updater_suspect_name(normalized_name: &str) -> bool {
    UPDATER_NAME_MARKERS
        .iter()
        .any(|marker| normalized_name.contains(marker))
}

/// Classify one process from local static evidence only. `autostart_names` holds
/// normalized executable names sourced from AeroTune's startup enumeration.
pub fn classify_process(name: &str, path: &str, autostart_names: &HashSet<String>) -> ProcessRole {
    let normalized = crate::processes::normalized_process_name(name);
    if normalized.starts_with("aerotune") {
        return ProcessRole {
            id: ROLE_SELF,
            label: "AeroTune 自身",
            detail: "AeroTune 进程".into(),
        };
    }
    let empty_path = path.trim().is_empty();
    if crate::processes::PROTECTED_PROCESSES.contains(&normalized.as_str()) {
        return ProcessRole {
            id: ROLE_SYSTEM_CORE,
            label: "系统核心",
            detail: "Windows 核心/常驻进程名单命中".into(),
        };
    }
    let mut normalized_path = None;
    match crate::processes::trusted_windows_directory() {
        Some(windows_root) if !empty_path => {
            let path = crate::processes::normalize_windows_path(path);
            let root = crate::processes::normalize_windows_root(windows_root);
            if crate::processes::is_windows_system_path_normalized(&path, &root) {
                if crate::processes::is_allowed_windows_background_process_normalized(
                    &normalized,
                    &path,
                    &root,
                ) {
                    return ProcessRole {
                        id: ROLE_WINDOWS_BACKGROUND,
                        label: "Windows 后台",
                        detail: "位于系统目录的后台组件白名单".into(),
                    };
                }
                return ProcessRole {
                    id: ROLE_SYSTEM_CORE,
                    label: "系统组件",
                    detail: "可执行文件位于 Windows 系统目录".into(),
                };
            }
            normalized_path = Some(path);
        }
        None if !empty_path => {
            return ProcessRole {
                id: ROLE_UNKNOWN,
                label: "未知",
                detail: "无法确认 Windows 系统目录，按未知来源处理".into(),
            };
        }
        _ => {}
    }
    if !empty_path && autostart_names.contains(&normalized) {
        return ProcessRole {
            id: ROLE_AUTOSTART,
            label: "自启动",
            detail: "出现在开机启动项清单中".into(),
        };
    }
    if is_updater_suspect_name(&normalized) {
        return ProcessRole {
            id: ROLE_UPDATER_SUSPECT,
            label: "更新器嫌疑",
            detail: "名称包含 update/setup/install 等标记（启发式，可能误判）".into(),
        };
    }
    if empty_path {
        return ProcessRole {
            id: ROLE_UNKNOWN,
            label: "未知",
            detail: "路径不可读（权限不足或进程已退出）".into(),
        };
    }
    let lowered = normalized_path.unwrap_or_else(|| crate::processes::normalize_windows_path(path));
    let known_root = lowered.starts_with("c:\\program files")
        || lowered.starts_with("c:\\program files (x86)")
        || lowered.contains("\\windowsapps\\")
        || lowered.contains("\\users\\")
        || lowered.starts_with("d:\\")
        || lowered.starts_with("e:\\")
        || lowered.starts_with("f:\\");
    ProcessRole {
        id: if known_root {
            ROLE_USER_APP
        } else {
            ROLE_UNKNOWN
        },
        label: if known_root { "用户应用" } else { "未知" },
        detail: if known_root {
            "安装在常规程序或用户目录".into()
        } else {
            "可执行文件位于非常规路径，建议留意".into()
        },
    }
}

/// Extract the normalized executable leaf name from a startup command line.
pub fn startup_command_exe_name(cmd: &str) -> Option<String> {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = if let Some(stripped) = trimmed.strip_prefix('"') {
        stripped.split('"').next().unwrap_or("")
    } else if trimmed.contains(' ') {
        trimmed.split_whitespace().next().unwrap_or("")
    } else {
        trimmed
    };
    let leaf = candidate.rsplit(['\\', '/']).next().unwrap_or(candidate);
    let leaf = leaf.trim();
    (!leaf.is_empty()).then(|| leaf.to_ascii_lowercase())
}

/// Cached startup-executable names with their refresh timestamp.
type AutostartCache = Mutex<Option<(std::time::Instant, HashSet<String>)>>;

pub(crate) fn autostart_snapshot() -> HashSet<String> {
    static CACHE: OnceLock<AutostartCache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = guard
        .as_ref()
        .is_some_and(|(at, _)| at.elapsed() < std::time::Duration::from_secs(60));
    if fresh {
        return guard
            .as_ref()
            .map(|(_, set)| set.clone())
            .unwrap_or_default();
    }
    let mut names = HashSet::new();
    for item in crate::startup::collect_startup_items() {
        if let Some(exe) = startup_command_exe_name(&item.cmd) {
            names.insert(exe);
        }
    }
    *guard = Some((std::time::Instant::now(), names.clone()));
    names
}

#[cfg(test)]
fn role_for(name: &str, path: &str) -> ProcessRole {
    classify_process(name, path, &autostart_snapshot())
}

// ================= 数字签名（WinVerifyTrust，按路径缓存） =================

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignatureStatus {
    Signed,
    Unsigned,
    Unknown,
}

impl SignatureStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            SignatureStatus::Signed => "signed",
            SignatureStatus::Unsigned => "unsigned",
            SignatureStatus::Unknown => "unknown",
        }
    }
}

const SIGNATURE_CACHE_CAP: usize = 512;
const SIGNATURE_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);
/// Upper bound on fresh WinVerifyTrust calls per command so one request can never stall.
const SIGNATURE_BUDGET_PER_CALL: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureEvidence {
    pub status: SignatureStatus,
    pub signer_subject: Option<String>,
}

impl SignatureEvidence {
    fn unknown() -> Self {
        Self {
            status: SignatureStatus::Unknown,
            signer_subject: None,
        }
    }
}

#[derive(Clone)]
struct CachedSignatureEvidence {
    checked_at: std::time::Instant,
    evidence: SignatureEvidence,
}

fn signature_cache() -> &'static Mutex<HashMap<String, CachedSignatureEvidence>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedSignatureEvidence>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sanitize_signer_subject(value: String) -> Option<String> {
    let cleaned = value
        .chars()
        .filter(|character| !character.is_control())
        .take(160)
        .collect::<String>()
        .trim()
        .to_string();
    (!cleaned.is_empty()).then_some(cleaned)
}

#[cfg(windows)]
mod wintrust {
    use super::{sanitize_signer_subject, SignatureEvidence, SignatureStatus};
    use windows_sys::Win32::Foundation::{FARPROC, HANDLE, HWND, TRUST_E_NOSIGNATURE};
    use windows_sys::Win32::Security::Cryptography::{
        CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE,
    };
    use windows_sys::Win32::Security::WinTrust::{
        WinVerifyTrust, CRYPT_PROVIDER_CERT, CRYPT_PROVIDER_DATA, CRYPT_PROVIDER_SGNR,
        WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_FILE_INFO, WTD_CHOICE_FILE,
        WTD_REVOKE_NONE, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    const INVALID_WINDOW_HANDLE: HWND = -1isize as HANDLE as HWND;

    type GetProviderData = unsafe extern "system" fn(HANDLE) -> *mut CRYPT_PROVIDER_DATA;
    type GetProviderSigner = unsafe extern "system" fn(
        *mut CRYPT_PROVIDER_DATA,
        u32,
        i32,
        u32,
    ) -> *mut CRYPT_PROVIDER_SGNR;
    type GetProviderCert =
        unsafe extern "system" fn(*mut CRYPT_PROVIDER_SGNR, u32) -> *mut CRYPT_PROVIDER_CERT;

    /// Resolve the unstable WinTrust helper APIs dynamically and fail closed if any are absent.
    /// The provider state remains live until the caller closes WINTRUST_DATA.
    unsafe fn signer_subject_from_state(state: HANDLE) -> Option<String> {
        let module = GetModuleHandleW(windows_sys::core::w!("wintrust.dll"));
        if module.is_null() {
            return None;
        }
        let data_proc: FARPROC =
            GetProcAddress(module, b"WTHelperProvDataFromStateData\0".as_ptr());
        let signer_proc: FARPROC =
            GetProcAddress(module, b"WTHelperGetProvSignerFromChain\0".as_ptr());
        let cert_proc: FARPROC = GetProcAddress(module, b"WTHelperGetProvCertFromChain\0".as_ptr());
        let data_proc = data_proc?;
        let signer_proc = signer_proc?;
        let cert_proc = cert_proc?;
        let get_data: GetProviderData = std::mem::transmute(data_proc);
        let get_signer: GetProviderSigner = std::mem::transmute(signer_proc);
        let get_cert: GetProviderCert = std::mem::transmute(cert_proc);

        let provider = get_data(state);
        if provider.is_null() {
            return None;
        }
        // Primary signer, not a timestamp countersigner; certificate zero is the leaf.
        let signer = get_signer(provider, 0, 0, 0);
        if signer.is_null() || (*signer).dwError != 0 {
            return None;
        }
        let provider_cert = get_cert(signer, 0);
        if provider_cert.is_null() || (*provider_cert).dwError != 0 {
            return None;
        }
        let cert = (*provider_cert).pCert;
        if cert.is_null() {
            return None;
        }
        let needed = CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
            0,
        );
        if !(2..=512).contains(&needed) {
            return None;
        }
        let mut buffer = vec![0u16; needed as usize];
        let written = CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            0,
            std::ptr::null(),
            buffer.as_mut_ptr(),
            needed,
        );
        if written < 2 || written > needed {
            return None;
        }
        let end = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        sanitize_signer_subject(String::from_utf16_lossy(&buffer[..end]))
    }

    /// One stateless Authenticode check: capture the primary signer subject before always closing state.
    pub(super) fn verify_once(path: &str) -> SignatureEvidence {
        let mut wide: Vec<u16> = path.encode_utf16().collect();
        wide.push(0);
        unsafe {
            let file_info = WINTRUST_FILE_INFO {
                cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
                pcwszFilePath: wide.as_ptr(),
                hFile: std::ptr::null_mut(),
                pgKnownSubject: std::ptr::null_mut(),
            };
            let mut data: WINTRUST_DATA = std::mem::zeroed();
            data.cbStruct = std::mem::size_of::<WINTRUST_DATA>() as u32;
            data.dwUIChoice = WTD_UI_NONE;
            data.fdwRevocationChecks = WTD_REVOKE_NONE;
            data.dwUnionChoice = WTD_CHOICE_FILE;
            data.Anonymous.pFile = &file_info as *const WINTRUST_FILE_INFO as *mut _;
            data.dwStateAction = WTD_STATEACTION_VERIFY;
            let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
            let status = WinVerifyTrust(
                INVALID_WINDOW_HANDLE,
                &mut action as *mut _,
                &mut data as *mut _ as *mut core::ffi::c_void,
            );
            let signer_subject = if status == 0 {
                signer_subject_from_state(data.hWVTStateData)
            } else {
                None
            };
            // Always release provider state, even when dynamic lookup or extraction failed.
            data.dwStateAction = WTD_STATEACTION_CLOSE;
            WinVerifyTrust(
                INVALID_WINDOW_HANDLE,
                &mut action as *mut _,
                &mut data as *mut _ as *mut core::ffi::c_void,
            );
            let status = if status == 0 {
                SignatureStatus::Signed
            } else if status == TRUST_E_NOSIGNATURE {
                SignatureStatus::Unsigned
            } else {
                SignatureStatus::Unknown
            };
            SignatureEvidence {
                status,
                signer_subject,
            }
        }
    }
}

/// Cached Authenticode status and, when available, the verified primary signer subject.
/// `budget` counts fresh checks; unknown results are not cached so refresh can retry.
/// Checks embedded signatures only: 仅目录签名（catalog-signed）的 OS 组件会呈现为 unsigned，这不是恶意结论。
pub fn verify_signature_evidence_cached(path: &str, budget: &mut usize) -> SignatureEvidence {
    let key = path.replace('/', "\\").to_ascii_lowercase();
    if key.is_empty() {
        return SignatureEvidence::unknown();
    }
    let cache = signature_cache();
    {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(hit) = guard.get(&key) {
            if hit.checked_at.elapsed() < SIGNATURE_CACHE_TTL {
                return hit.evidence.clone();
            }
        }
    }
    if *budget == 0 {
        return SignatureEvidence::unknown();
    }
    *budget -= 1;
    #[cfg(windows)]
    let verified = wintrust::verify_once(path);
    #[cfg(not(windows))]
    let verified = {
        let _ = path;
        SignatureEvidence::unknown()
    };
    if verified.status != SignatureStatus::Unknown {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= SIGNATURE_CACHE_CAP {
            guard.clear();
        }
        guard.insert(
            key,
            CachedSignatureEvidence {
                checked_at: std::time::Instant::now(),
                evidence: verified.clone(),
            },
        );
    }
    verified
}

pub fn verify_signature_cached(path: &str, budget: &mut usize) -> SignatureStatus {
    verify_signature_evidence_cached(path, budget).status
}

// ================= 后台常驻洞察命令 =================

#[derive(Serialize, Clone, Debug)]
pub struct ProcessInsightEntry {
    pub pid: u32,
    pub name: String,
    pub path: String,
    pub role_id: String,
    pub role_label: String,
    pub role_detail: String,
    pub mem_mb: f64,
    pub cpu: f32,
    pub start_age_secs: Option<u64>,
    pub signature: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProcessInsights {
    pub generated_unix: u64,
    pub entries: Vec<ProcessInsightEntry>,
    /// Paths skipped this round because the verification budget ran out.
    pub signature_deferred: usize,
}

const INSIGHT_MAX_ENTRIES: usize = 12;

/// Read-only snapshot of the heaviest non-system resident processes with role
/// and signature evidence. Heavy work (inventory scan + WinVerifyTrust) runs on
/// the blocking pool; nothing here mutates system state.
#[tauri::command]
pub async fn get_process_insights() -> Result<ProcessInsights, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let details = crate::processes::process_details_snapshot();
        let now_unix = chrono::Utc::now().timestamp().max(0) as u64;
        let mut candidates: Vec<_> = details
            .into_iter()
            .filter(|detail| {
                !matches!(
                    detail.role.id,
                    ROLE_SYSTEM_CORE | ROLE_WINDOWS_BACKGROUND | ROLE_SELF
                )
            })
            .collect();
        candidates.sort_by(|a, b| {
            b.info
                .mem_mb
                .total_cmp(&a.info.mem_mb)
                .then_with(|| a.info.pid.cmp(&b.info.pid))
        });
        candidates.truncate(INSIGHT_MAX_ENTRIES);
        let mut budget = SIGNATURE_BUDGET_PER_CALL;
        let mut deferred = 0usize;
        let entries = candidates
            .into_iter()
            .map(|detail| {
                let signature = if detail.info.path.trim().is_empty() {
                    SignatureStatus::Unknown
                } else {
                    let status = verify_signature_cached(&detail.info.path, &mut budget);
                    if matches!(status, SignatureStatus::Unknown)
                        && !signature_cache()
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .contains_key(&detail.info.path.replace('/', "\\").to_ascii_lowercase())
                    {
                        deferred += 1;
                    }
                    status
                };
                ProcessInsightEntry {
                    pid: detail.info.pid,
                    name: detail.info.name.clone(),
                    path: detail.info.path.clone(),
                    role_id: detail.role.id.to_string(),
                    role_label: detail.role.label.to_string(),
                    role_detail: detail.role.detail.clone(),
                    mem_mb: detail.info.mem_mb,
                    cpu: detail.info.cpu,
                    start_age_secs: detail
                        .start_unix
                        .and_then(|start| now_unix.checked_sub(start)),
                    signature: signature.as_str().to_string(),
                }
            })
            .collect();
        Ok(ProcessInsights {
            generated_unix: now_unix,
            entries,
            signature_deferred: deferred,
        })
    })
    .await
    .map_err(|e| format!("生成进程洞察失败: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn autostart() -> HashSet<String> {
        HashSet::from(["autostartapp.exe".to_string()])
    }

    fn classify_process_before_path_reuse(
        name: &str,
        path: &str,
        autostart_names: &HashSet<String>,
    ) -> ProcessRole {
        let normalized = crate::processes::normalized_process_name(name);
        if normalized.starts_with("aerotune") {
            return ProcessRole {
                id: ROLE_SELF,
                label: "AeroTune 自身",
                detail: "AeroTune 进程".into(),
            };
        }
        let empty_path = path.trim().is_empty();
        if crate::processes::PROTECTED_PROCESSES.contains(&normalized.as_str()) {
            return ProcessRole {
                id: ROLE_SYSTEM_CORE,
                label: "系统核心",
                detail: "Windows 核心/常驻进程名单命中".into(),
            };
        }
        if let Some(windows_root) = crate::processes::trusted_windows_directory() {
            if crate::processes::is_windows_system_path(path, windows_root) {
                if crate::processes::is_allowed_windows_background_process(name, path, windows_root)
                {
                    return ProcessRole {
                        id: ROLE_WINDOWS_BACKGROUND,
                        label: "Windows 后台",
                        detail: "位于系统目录的后台组件白名单".into(),
                    };
                }
                return ProcessRole {
                    id: ROLE_SYSTEM_CORE,
                    label: "系统组件",
                    detail: "可执行文件位于 Windows 系统目录".into(),
                };
            }
        } else if !empty_path {
            return ProcessRole {
                id: ROLE_UNKNOWN,
                label: "未知",
                detail: "无法确认 Windows 系统目录，按未知来源处理".into(),
            };
        }
        if !empty_path && autostart_names.contains(&normalized) {
            return ProcessRole {
                id: ROLE_AUTOSTART,
                label: "自启动",
                detail: "出现在开机启动项清单中".into(),
            };
        }
        if is_updater_suspect_name(&normalized) {
            return ProcessRole {
                id: ROLE_UPDATER_SUSPECT,
                label: "更新器嫌疑",
                detail: "名称包含 update/setup/install 等标记（启发式，可能误判）".into(),
            };
        }
        if empty_path {
            return ProcessRole {
                id: ROLE_UNKNOWN,
                label: "未知",
                detail: "路径不可读（权限不足或进程已退出）".into(),
            };
        }
        let lowered = path.replace('/', "\\").to_ascii_lowercase();
        let known_root = lowered.starts_with("c:\\program files")
            || lowered.starts_with("c:\\program files (x86)")
            || lowered.contains("\\windowsapps\\")
            || lowered.contains("\\users\\")
            || lowered.starts_with("d:\\")
            || lowered.starts_with("e:\\")
            || lowered.starts_with("f:\\");
        ProcessRole {
            id: if known_root {
                ROLE_USER_APP
            } else {
                ROLE_UNKNOWN
            },
            label: if known_root { "用户应用" } else { "未知" },
            detail: if known_root {
                "安装在常规程序或用户目录".into()
            } else {
                "可执行文件位于非常规路径，建议留意".into()
            },
        }
    }

    #[test]
    fn normalized_path_classification_matches_previous_precedence_and_boundaries() {
        let windows_root =
            crate::processes::trusted_windows_directory().expect("Windows directory API succeeds");
        let mixed_case_system_path = format!(
            "{}/System32/SearchIndexer.exe",
            windows_root.replace('\\', "/").to_ascii_uppercase()
        );
        let cases = vec![
            (
                "aerotune.exe".to_string(),
                "F:/AeroTune/aerotune.exe".to_string(),
            ),
            (
                "svchost.exe".to_string(),
                format!("{windows_root}/System32/svchost.exe"),
            ),
            ("SearchIndexer.exe".to_string(), mixed_case_system_path),
            (
                "SystemWorker.exe".to_string(),
                format!("{windows_root}/System32/Other.exe"),
            ),
            (
                "NormalApp.exe".to_string(),
                "C:/Program Files/Vendor/NormalApp.exe".to_string(),
            ),
            (
                "StoreApp.exe".to_string(),
                "Z:/WindowsApps/Vendor/StoreApp.exe".to_string(),
            ),
            (
                "UserApp.exe".to_string(),
                "C:/Users/Example/UserApp.exe".to_string(),
            ),
            (
                "Portable.exe".to_string(),
                "D:/Portable/Portable.exe".to_string(),
            ),
            (
                "autostartapp.exe".to_string(),
                "C:/Tools/autostartapp.exe".to_string(),
            ),
            (
                "TeamUpdater.exe".to_string(),
                "C:/Tools/TeamUpdater.exe".to_string(),
            ),
            ("TeamUpdater.exe".to_string(), String::new()),
            ("Missing.exe".to_string(), String::new()),
            (
                "Boundary.exe".to_string(),
                format!("{windows_root}Old\\Boundary.exe"),
            ),
            (
                "Odd.exe".to_string(),
                r"\\?\Volume{1234}\Unusual\Odd.exe".to_string(),
            ),
        ];
        let autostart_names = autostart();
        for (name, path) in cases {
            assert_eq!(
                classify_process(&name, &path, &autostart_names),
                classify_process_before_path_reuse(&name, &path, &autostart_names),
                "name={name}, path={path}"
            );
        }
    }

    #[test]
    #[ignore = "manual Release benchmark for normalized process-path reuse"]
    fn benchmark_classify_process_path_normalization_reuse() {
        const ROWS: usize = 500;
        const BATCHES: usize = 101;
        let windows_root =
            crate::processes::trusted_windows_directory().expect("Windows directory API succeeds");
        let fixtures = [
            ("aerotune.exe", "F:/AeroTune/aerotune.exe".to_string()),
            (
                "svchost.exe",
                format!("{windows_root}/System32/svchost.exe"),
            ),
            (
                "SearchIndexer.exe",
                format!("{windows_root}/System32/SearchIndexer.exe"),
            ),
            (
                "SystemWorker.exe",
                format!("{windows_root}/System32/Other.exe"),
            ),
            (
                "NormalApp.exe",
                "C:/Program Files/Vendor/NormalApp.exe".to_string(),
            ),
            (
                "StoreApp.exe",
                "Z:/WindowsApps/Vendor/StoreApp.exe".to_string(),
            ),
            ("UserApp.exe", "C:/Users/Example/UserApp.exe".to_string()),
            ("autostartapp.exe", "C:/Tools/autostartapp.exe".to_string()),
            ("TeamUpdater.exe", "C:/Tools/TeamUpdater.exe".to_string()),
            ("Boundary.exe", format!("{windows_root}Old\\Boundary.exe")),
            ("Odd.exe", r"\\?\Volume{1234}\Unusual\Odd.exe".to_string()),
        ];
        let rows: Vec<(String, String)> = (0..ROWS)
            .map(|index| {
                let (name, path) = &fixtures[index % fixtures.len()];
                ((*name).to_string(), path.clone())
            })
            .collect();
        let autostart_names = autostart();
        let reference: Vec<_> = rows
            .iter()
            .map(|(name, path)| classify_process_before_path_reuse(name, path, &autostart_names))
            .collect();
        let optimized: Vec<_> = rows
            .iter()
            .map(|(name, path)| classify_process(name, path, &autostart_names))
            .collect();
        assert_eq!(reference, optimized);

        fn measure(
            rows: &[(String, String)],
            autostart_names: &HashSet<String>,
            reference: bool,
        ) -> u128 {
            let started = std::time::Instant::now();
            let roles: Vec<_> = rows
                .iter()
                .map(|(name, path)| {
                    if reference {
                        classify_process_before_path_reuse(name, path, autostart_names)
                    } else {
                        classify_process(name, path, autostart_names)
                    }
                })
                .collect();
            std::hint::black_box(roles);
            started.elapsed().as_nanos()
        }

        let mut reference_nanos = Vec::with_capacity(BATCHES);
        let mut optimized_nanos = Vec::with_capacity(BATCHES);
        for warmup in 0..7 {
            if warmup % 2 == 0 {
                measure(&rows, &autostart_names, true);
                measure(&rows, &autostart_names, false);
            } else {
                measure(&rows, &autostart_names, false);
                measure(&rows, &autostart_names, true);
            }
        }
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                reference_nanos.push(measure(&rows, &autostart_names, true));
                optimized_nanos.push(measure(&rows, &autostart_names, false));
            } else {
                optimized_nanos.push(measure(&rows, &autostart_names, false));
                reference_nanos.push(measure(&rows, &autostart_names, true));
            }
        }
        reference_nanos.sort_unstable();
        optimized_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "process path classification ({} rows): before reuse median/p95={:.1}/{:.1} us; one normalized path={:.1}/{:.1} us",
            ROWS,
            reference_nanos[BATCHES / 2] as f64 / 1_000.0,
            reference_nanos[p95_index] as f64 / 1_000.0,
            optimized_nanos[BATCHES / 2] as f64 / 1_000.0,
            optimized_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn classify_known_roles_from_static_evidence() {
        assert_eq!(
            classify_process(
                "svchost.exe",
                "C:\\Windows\\System32\\svchost.exe",
                &autostart()
            )
            .id,
            ROLE_SYSTEM_CORE
        );
        assert_eq!(
            classify_process(
                "searchindexer.exe",
                "C:\\Windows\\system32\\searchindexer.exe",
                &autostart()
            )
            .id,
            ROLE_WINDOWS_BACKGROUND
        );
        assert_eq!(
            classify_process(
                "autostartapp.exe",
                "C:\\Tools\\autostartapp.exe",
                &autostart()
            )
            .id,
            ROLE_AUTOSTART
        );
        assert_eq!(
            classify_process(
                "TeamUpdater.exe",
                "C:\\Tools\\TeamUpdater.exe",
                &HashSet::new()
            )
            .id,
            ROLE_UPDATER_SUSPECT
        );
        assert_eq!(
            classify_process(
                "NormalApp.exe",
                "C:\\Program Files\\Vendor\\NormalApp.exe",
                &HashSet::new()
            )
            .id,
            ROLE_USER_APP
        );
        assert_eq!(
            classify_process("Weird.exe", "Z:\\drop\\Weird.exe", &HashSet::new()).id,
            ROLE_UNKNOWN
        );
        assert_eq!(classify_process("", "", &HashSet::new()).id, ROLE_UNKNOWN);
        assert_eq!(
            classify_process("aerotune.exe", "F:\\x\\aerotune.exe", &HashSet::new()).id,
            ROLE_SELF
        );
    }

    #[test]
    fn per_row_role_lookup_matches_one_cached_snapshot() {
        let autostart_names = autostart_snapshot();
        let mut fixtures = vec![
            (
                "aerotune.exe".to_string(),
                "C:\\Program Files\\AeroTune\\aerotune.exe".to_string(),
            ),
            (
                "svchost.exe".to_string(),
                "C:\\Windows\\System32\\svchost.exe".to_string(),
            ),
            (
                "searchindexer.exe".to_string(),
                "C:\\Windows\\System32\\searchindexer.exe".to_string(),
            ),
            (
                "NormalApp.exe".to_string(),
                "C:\\Program Files\\Vendor\\NormalApp.exe".to_string(),
            ),
            (
                "TeamUpdater.exe".to_string(),
                "C:\\Tools\\TeamUpdater.exe".to_string(),
            ),
            ("Weird.exe".to_string(), String::new()),
        ];
        if let Some(startup_name) = autostart_names.iter().next() {
            fixtures.push((startup_name.clone(), format!("C:\\Tools\\{startup_name}")));
        }

        for (name, path) in fixtures {
            assert_eq!(
                role_for(&name, &path),
                classify_process(&name, &path, &autostart_names),
                "name={name}, path={path}"
            );
        }
    }

    #[test]
    #[ignore = "manual Release benchmark for process-role snapshot reuse"]
    fn benchmark_process_role_snapshot_reuse() {
        const ROWS: usize = 500;
        const BATCHES: usize = 101;
        let autostart_names = autostart_snapshot();
        let templates = [
            ("aerotune.exe", "C:\\Program Files\\AeroTune\\aerotune.exe"),
            ("svchost.exe", "C:\\Windows\\System32\\svchost.exe"),
            (
                "searchindexer.exe",
                "C:\\Windows\\System32\\searchindexer.exe",
            ),
            ("NormalApp.exe", "C:\\Program Files\\Vendor\\NormalApp.exe"),
            ("TeamUpdater.exe", "C:\\Tools\\TeamUpdater.exe"),
            ("Weird.exe", ""),
        ];
        let rows = (0..ROWS)
            .map(|index| {
                let (name, path) = templates[index % templates.len()];
                (name.to_string(), path.to_string())
            })
            .collect::<Vec<_>>();
        let per_row_reference = rows
            .iter()
            .map(|(name, path)| role_for(name, path))
            .collect::<Vec<_>>();
        let reused_snapshot = rows
            .iter()
            .map(|(name, path)| classify_process(name, path, &autostart_names))
            .collect::<Vec<_>>();
        assert_eq!(per_row_reference, reused_snapshot);

        let measure_per_row = || {
            let started = std::time::Instant::now();
            let roles = rows
                .iter()
                .map(|(name, path)| role_for(name, path))
                .collect::<Vec<_>>();
            std::hint::black_box(roles);
            started.elapsed().as_nanos()
        };
        let measure_reused = || {
            let started = std::time::Instant::now();
            let names = autostart_snapshot();
            let roles = rows
                .iter()
                .map(|(name, path)| classify_process(name, path, &names))
                .collect::<Vec<_>>();
            std::hint::black_box(roles);
            started.elapsed().as_nanos()
        };
        for _ in 0..7 {
            std::hint::black_box(measure_per_row());
            std::hint::black_box(measure_reused());
        }
        let mut per_row_nanos = Vec::with_capacity(BATCHES);
        let mut reused_nanos = Vec::with_capacity(BATCHES);
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                per_row_nanos.push(measure_per_row());
                reused_nanos.push(measure_reused());
            } else {
                reused_nanos.push(measure_reused());
                per_row_nanos.push(measure_per_row());
            }
        }
        per_row_nanos.sort_unstable();
        reused_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "process role snapshot ({} rows): per-row cache clone median/p95={:.1}/{:.1} us; one snapshot={:.1}/{:.1}",
            ROWS,
            per_row_nanos[BATCHES / 2] as f64 / 1_000.0,
            per_row_nanos[p95_index] as f64 / 1_000.0,
            reused_nanos[BATCHES / 2] as f64 / 1_000.0,
            reused_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn updater_marker_does_not_shadow_protected_names() {
        let role = classify_process(
            "setup.exe",
            "C:\\Windows\\System32\\setup.exe",
            &HashSet::new(),
        );
        assert_eq!(role.id, ROLE_SYSTEM_CORE);
    }

    #[test]
    fn startup_command_exe_name_extracts_leafs() {
        assert_eq!(
            startup_command_exe_name("\"C:\\Program Files\\App\\App.exe\" /bg"),
            Some("app.exe".to_string())
        );
        assert_eq!(
            startup_command_exe_name("C:\\Tools\\tool.exe"),
            Some("tool.exe".to_string())
        );
        assert_eq!(startup_command_exe_name("  "), None);
    }

    #[test]
    fn signature_budget_defers_without_caching() {
        let mut budget = 0;
        let status = verify_signature_cached("C:\\nonexistent-budget-test.exe", &mut budget);
        assert_eq!(status, SignatureStatus::Unknown);
        assert!(!signature_cache()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key("c:\\nonexistent-budget-test.exe"));
    }

    #[test]
    fn signer_subject_display_is_bounded_and_control_free() {
        assert_eq!(
            sanitize_signer_subject("  Acme\nLtd\0  ".into()),
            Some("AcmeLtd".into())
        );
        assert_eq!(sanitize_signer_subject("\n\0".into()), None);
        let long = "A".repeat(200);
        assert_eq!(sanitize_signer_subject(long).unwrap().chars().count(), 160);
    }

    #[test]
    #[ignore = "live WinVerifyTrust signer extraction smoke test on installed Windows binaries"]
    fn live_signature_check_exposes_verified_signer_subject() {
        let candidates = [
            "C:\\Program Files\\Docker\\Docker\\frontend\\Docker Desktop.exe",
            "C:\\Program Files\\NVIDIA Corporation\\NVIDIA app\\CEF\\NVIDIA app.exe",
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
        ];
        for path in candidates {
            if !std::path::Path::new(path).exists() {
                continue;
            }
            let mut budget = 1;
            let evidence = verify_signature_evidence_cached(path, &mut budget);
            if evidence.status == SignatureStatus::Signed {
                assert!(
                    evidence
                        .signer_subject
                        .as_deref()
                        .is_some_and(|name| !name.trim().is_empty()),
                    "verified signature did not expose a primary signer subject for {path}"
                );
                return;
            }
        }
        panic!("no installed embedded-signed third-party binary was available for the signer smoke test");
    }

    #[test]
    #[ignore = "live WinVerifyTrust read-only check against the OS"]
    fn live_signature_check_distinguishes_signed_and_missing_files() {
        // Third-party apps carry embedded Authenticode signatures; catalog-signed
        // OS binaries intentionally read as unsigned here (they never reach the UI).
        let candidates = [
            "C:\\Program Files\\Docker\\Docker\\frontend\\Docker Desktop.exe",
            "C:\\Program Files\\NVIDIA Corporation\\NVIDIA app\\CEF\\NVIDIA app.exe",
        ];
        let found_signed = candidates.iter().any(|path| {
            std::path::Path::new(path).exists() && {
                let mut budget = 1;
                verify_signature_cached(path, &mut budget) == SignatureStatus::Signed
            }
        });
        assert!(
            found_signed,
            "expected at least one embedded-signed third-party binary to verify"
        );
        let missing = "C:\\Windows\\System32\\definitely-missing-aerotune-probe.exe";
        let mut budget = 1;
        assert_ne!(
            verify_signature_cached(missing, &mut budget),
            SignatureStatus::Signed
        );
    }
}
