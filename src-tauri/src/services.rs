// 服务管理：仅显示非关键且镜像位于已知 Program Files 根目录的服务可调整
// 启动类型本质是注册表 Services\{name}\Start（+DelayedAutostart），直接走变更日志可逆体系。
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
pub struct SvcInfo {
    pub name: String,
    pub display: String,
    pub start_mode: String,
    pub state: String,
    pub path: String,
    pub account: String,
    pub modifiable: bool,
    /// 注册表当前值：Start（2 自动 3 手动 4 禁用）
    pub start_reg: Option<u32>,
    pub delayed: Option<u32>,
}

/// 核心、更新和安全服务（无论路径）一律拒绝修改
const CRITICAL_SERVICES: &[&str] = &[
    "windefend",
    "wdnissvc",
    "wdfilter",
    "wdnisdrv",
    "wdboot",
    "securityhealthservice",
    "wscsvc",
    "sense",
    "wuauserv",
    "bits",
    "cryptsvc",
    "trustedinstaller",
    "appidsvc",
    "msiserver",
    "rpcss",
    "rpceptmapper",
    "dcomlaunch",
    "winmgmt",
    "eventlog",
    "schedule",
    "profsvc",
    "usermanager",
    "systemeventsbroker",
    "audiosrv",
    "audioendpointbuilder",
    "themes",
    "nsi",
    "dhcp",
    "dnscache",
    "lanmanserver",
    "lanmanworkstation",
    "mpssvc",
    "bfe",
    "power",
    "plugplay",
    "brokerinfrastructure",
    "statestorage",
    "wcmsvc",
    "coremessagingregistrar",
];

fn services_key() -> Option<winreg::RegKey> {
    winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(
            r"SYSTEM\CurrentControlSet\Services",
            winreg::enums::KEY_READ,
        )
        .ok()
}

fn read_service_reg(
    services: &winreg::RegKey,
    name: &str,
) -> (Option<u32>, Option<u32>, Option<String>, Option<String>) {
    let Ok(k) = services.open_subkey(name) else {
        return (None, None, None, None);
    };
    (
        k.get_value::<u32, _>("Start").ok(),
        k.get_value::<u32, _>("DelayedAutostart").ok(),
        k.get_value::<String, _>("ImagePath").ok(),
        k.get_value::<String, _>("ObjectName").ok(),
    )
}

fn canonical_service_account(account: &str) -> Option<String> {
    let account = account.trim();
    if account.is_empty() || account.len() > 256 || account.chars().any(char::is_control) {
        return None;
    }
    let normalized = account.to_ascii_lowercase();
    let canonical = match normalized.as_str() {
        "localsystem" | "system" | "nt authority\\system" | "nt authority\\localsystem" => {
            "localsystem".to_string()
        }
        "localservice" | "nt authority\\localservice" => "localservice".to_string(),
        "networkservice" | "nt authority\\networkservice" => "networkservice".to_string(),
        _ => normalized,
    };
    if let Some(service_name) = canonical.strip_prefix("nt service\\") {
        if !valid_service_name(service_name) {
            return None;
        }
    }
    Some(canonical)
}

fn service_account_is_supported(service_name: &str, account: &str) -> bool {
    let Some(account) = canonical_service_account(account) else {
        return false;
    };
    matches!(
        account.as_str(),
        "localsystem" | "localservice" | "networkservice"
    ) || account
        .strip_prefix("nt service\\")
        .is_some_and(|name| name.eq_ignore_ascii_case(service_name))
}

fn service_accounts_match(registry_account: &str, reported_account: &str) -> bool {
    canonical_service_account(registry_account)
        .zip(canonical_service_account(reported_account))
        .is_some_and(|(registry, reported)| registry == reported)
}

fn service_account_from_scm_config(service_type: u32, start_name: Option<&str>) -> Option<String> {
    use windows_sys::Win32::System::Services::{
        SERVICE_WIN32_OWN_PROCESS, SERVICE_WIN32_SHARE_PROCESS,
    };

    if service_type != SERVICE_WIN32_OWN_PROCESS && service_type != SERVICE_WIN32_SHARE_PROCESS {
        return None;
    }
    let account = start_name.unwrap_or("LocalSystem");
    (!account.trim().is_empty()).then(|| account.to_string())
}

fn service_account_configurations_match(
    service_name: &str,
    registry_account: &str,
    scm_account: &str,
) -> bool {
    service_account_is_supported(service_name, scm_account)
        && service_accounts_match(registry_account, scm_account)
}

struct ServiceScmHandle(windows_sys::Win32::System::Services::SC_HANDLE);

impl Drop for ServiceScmHandle {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::System::Services::CloseServiceHandle(self.0) };
    }
}

fn query_scm_service_account(service_name: &str) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::System::Services::{
        OpenSCManagerW, OpenServiceW, QueryServiceConfigW, QUERY_SERVICE_CONFIGW,
        SC_MANAGER_CONNECT, SERVICE_QUERY_CONFIG,
    };

    let manager = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if manager.is_null() {
        return None;
    }
    let _manager_handle = ServiceScmHandle(manager);
    let wide_name: Vec<u16> = std::ffi::OsStr::new(service_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let service = unsafe { OpenServiceW(manager, wide_name.as_ptr(), SERVICE_QUERY_CONFIG) };
    if service.is_null() {
        return None;
    }
    let _service_handle = ServiceScmHandle(service);

    let mut bytes_needed = 0u32;
    unsafe { QueryServiceConfigW(service, ptr::null_mut(), 0, &mut bytes_needed) };
    if bytes_needed == 0 || bytes_needed > 8 * 1024 {
        return None;
    }
    let word_count = (bytes_needed as usize).div_ceil(std::mem::size_of::<u64>());
    let mut storage = vec![0u64; word_count];
    let storage_bytes = word_count.checked_mul(std::mem::size_of::<u64>())?;
    let config = storage.as_mut_ptr().cast::<QUERY_SERVICE_CONFIGW>();
    if unsafe {
        QueryServiceConfigW(
            service,
            config,
            u32::try_from(storage_bytes).ok()?,
            &mut bytes_needed,
        )
    } == 0
    {
        return None;
    }
    let service_type = unsafe { (*config).dwServiceType };
    let start_name = unsafe { (*config).lpServiceStartName };
    let account = if start_name.is_null() {
        None
    } else {
        let storage_start = storage.as_ptr() as usize;
        let storage_end = storage_start.checked_add(storage_bytes)?;
        let name_start = start_name as usize;
        if name_start < storage_start
            || name_start >= storage_end
            || !name_start.is_multiple_of(std::mem::align_of::<u16>())
        {
            return None;
        }
        let name_capacity = (storage_end - name_start) / std::mem::size_of::<u16>();
        let name_buffer = unsafe { std::slice::from_raw_parts(start_name, name_capacity) };
        let name_length = name_buffer.iter().position(|unit| *unit == 0)?;
        Some(String::from_utf16(&name_buffer[..name_length]).ok()?)
    };
    service_account_from_scm_config(service_type, account.as_deref())
}

fn service_scm_account_matches(service_name: &str, registry_account: &str) -> bool {
    query_scm_service_account(service_name).is_some_and(|scm_account| {
        service_account_configurations_match(service_name, registry_account, &scm_account)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceAclAce {
    sid: String,
    mask: u32,
    allow: bool,
    inherit_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceAclSnapshot {
    owner_sid: String,
    is_directory: bool,
    is_volume_root: bool,
    aces: Vec<ServiceAclAce>,
}

const TRUSTED_SERVICE_WRITER_SIDS: &[&str] = &[
    "S-1-5-18",                                                       // Local System
    "S-1-5-32-544",                                                   // Built-in Administrators
    "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464", // TrustedInstaller
];
const SERVICE_COMMON_WRITE_MASK: u32 = 0x0000_0010 // FILE_WRITE_EA
    | 0x0000_0100 // FILE_WRITE_ATTRIBUTES
    | 0x0001_0000 // DELETE
    | 0x0004_0000 // WRITE_DAC
    | 0x0008_0000 // WRITE_OWNER
    | 0x0200_0000 // MAXIMUM_ALLOWED (fail closed if present in an allow ACE)
    | 0x1000_0000 // GENERIC_ALL
    | 0x4000_0000; // GENERIC_WRITE
const SERVICE_FILE_WRITE_MASK: u32 = SERVICE_COMMON_WRITE_MASK
    | 0x0000_0002 // FILE_WRITE_DATA
    | 0x0000_0004; // FILE_APPEND_DATA
const SERVICE_DIRECTORY_WRITE_MASK: u32 = SERVICE_COMMON_WRITE_MASK
    | 0x0000_0002 // FILE_ADD_FILE
    | 0x0000_0004 // FILE_ADD_SUBDIRECTORY
    | 0x0000_0040; // FILE_DELETE_CHILD

fn service_acl_snapshot_is_protected(snapshot: &ServiceAclSnapshot) -> bool {
    let mut write_mask = if snapshot.is_directory {
        SERVICE_DIRECTORY_WRITE_MASK
    } else {
        SERVICE_FILE_WRITE_MASK
    };
    // Standard Windows volume roots may let Authenticated Users create sibling directories;
    // that does not grant write/delete access to the existing Program Files tree.
    if snapshot.is_volume_root {
        write_mask &= !0x0000_0004;
    }
    TRUSTED_SERVICE_WRITER_SIDS
        .iter()
        .any(|sid| snapshot.owner_sid.eq_ignore_ascii_case(sid))
        && snapshot.aces.iter().all(|ace| {
            !ace.allow
                || ace.inherit_only
                || ace.mask & write_mask == 0
                || TRUSTED_SERVICE_WRITER_SIDS
                    .iter()
                    .any(|sid| ace.sid.eq_ignore_ascii_case(sid))
        })
}

fn service_acl_chain_is_protected(snapshots: impl IntoIterator<Item = ServiceAclSnapshot>) -> bool {
    let mut checked = false;
    for snapshot in snapshots {
        checked = true;
        if !service_acl_snapshot_is_protected(&snapshot) {
            return false;
        }
    }
    checked
}

struct ServiceAclHandle(windows_sys::Win32::Foundation::HANDLE);

impl Drop for ServiceAclHandle {
    fn drop(&mut self) {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

struct ServiceAclLocalMemory(*mut std::ffi::c_void);

impl Drop for ServiceAclLocalMemory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { windows_sys::Win32::Foundation::LocalFree(self.0 as _) };
        }
    }
}

fn service_sid_string(sid: windows_sys::Win32::Security::PSID) -> Option<String> {
    use std::ptr;
    use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;

    if sid.is_null() || unsafe { windows_sys::Win32::Security::IsValidSid(sid) } == 0 {
        return None;
    }
    let mut raw: *mut u16 = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid, &mut raw) } == 0 || raw.is_null() {
        return None;
    }
    let _raw_memory = ServiceAclLocalMemory(raw.cast());
    let mut length = 0usize;
    while length < 184 && unsafe { *raw.add(length) } != 0 {
        length += 1;
    }
    if length == 184 {
        return None;
    }
    Some(String::from_utf16_lossy(unsafe {
        std::slice::from_raw_parts(raw, length)
    }))
}

fn read_service_acl_snapshot(path: &Path) -> Option<ServiceAclSnapshot> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        AclSizeInformation, GetAce, GetAclInformation, GetSecurityDescriptorDacl,
        ACCESS_ALLOWED_ACE, ACE_HEADER, ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION,
        OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING, READ_CONTROL,
    };

    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let raw_handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            READ_CONTROL | FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
            ptr::null_mut(),
        )
    };
    if raw_handle.is_null() || raw_handle == INVALID_HANDLE_VALUE {
        return None;
    }
    let _handle = ServiceAclHandle(raw_handle);
    let mut file_info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(raw_handle, &mut file_info) } == 0
        || file_info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
    {
        return None;
    }

    let mut owner: windows_sys::Win32::Security::PSID = ptr::null_mut();
    let mut dacl: *mut windows_sys::Win32::Security::ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            raw_handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 || descriptor.is_null() {
        if !descriptor.is_null() {
            unsafe { windows_sys::Win32::Foundation::LocalFree(descriptor as _) };
        }
        return None;
    }
    let _descriptor_memory = ServiceAclLocalMemory(descriptor.cast());
    let owner_sid = service_sid_string(owner)?;

    let mut present = 0;
    let mut defaulted = 0;
    let mut actual_dacl: *mut windows_sys::Win32::Security::ACL = ptr::null_mut();
    if unsafe {
        GetSecurityDescriptorDacl(descriptor, &mut present, &mut actual_dacl, &mut defaulted)
    } == 0
        || present == 0
        || actual_dacl.is_null()
    {
        return None;
    }
    let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        GetAclInformation(
            actual_dacl,
            (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
        || info.AceCount > 4096
    {
        return None;
    }

    let mut aces = Vec::with_capacity(info.AceCount as usize);
    for index in 0..info.AceCount {
        let mut ace_pointer: *mut std::ffi::c_void = ptr::null_mut();
        if unsafe { GetAce(actual_dacl, index, &mut ace_pointer) } == 0 || ace_pointer.is_null() {
            return None;
        }
        let header = unsafe { &*ace_pointer.cast::<ACE_HEADER>() };
        let ace_size = usize::from(header.AceSize);
        if ace_size < std::mem::size_of::<ACE_HEADER>() {
            return None;
        }
        match header.AceType {
            0 => {
                let sid_offset = std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>();
                if ace_size < sid_offset + 8 {
                    return None;
                }
                let ace = unsafe { &*ace_pointer.cast::<ACCESS_ALLOWED_ACE>() };
                let sid = std::ptr::addr_of!(ace.SidStart) as windows_sys::Win32::Security::PSID;
                if unsafe { windows_sys::Win32::Security::IsValidSid(sid) } == 0 {
                    return None;
                }
                let sid_length =
                    unsafe { windows_sys::Win32::Security::GetLengthSid(sid) } as usize;
                if sid_length < 8 || sid_length > ace_size - sid_offset {
                    return None;
                }
                aces.push(ServiceAclAce {
                    sid: service_sid_string(sid)?,
                    mask: ace.Mask,
                    allow: true,
                    inherit_only: header.AceFlags
                        & windows_sys::Win32::Security::INHERIT_ONLY_ACE as u8
                        != 0,
                });
            }
            // Deny, audit, mandatory-label and other known non-granting ACEs cannot add write access.
            1..=3 | 6..=8 | 10 | 12..=21 => {}
            // Compound/object/callback allow ACEs and unknown future ACE types fail closed.
            _ => return None,
        }
    }
    Some(ServiceAclSnapshot {
        owner_sid,
        is_directory: file_info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_volume_root: path.parent().is_none(),
        aces,
    })
}

fn service_image_acl_is_protected(executable: &Path) -> bool {
    let mut snapshots = Vec::new();
    for ancestor in executable.ancestors() {
        let Some(snapshot) = read_service_acl_snapshot(ancestor) else {
            return false;
        };
        snapshots.push(snapshot);
        if ancestor.parent().is_none() {
            break;
        }
    }
    service_acl_chain_is_protected(snapshots)
}

fn service_path_has_no_reparse_components(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;

    let mut checked = false;
    for ancestor in path.ancestors() {
        let Ok(metadata) = std::fs::symlink_metadata(ancestor) else {
            return false;
        };
        checked = true;
        if metadata.file_type().is_symlink() || metadata.file_attributes() & 0x0400 != 0 {
            return false;
        }
    }
    checked
}

fn service_image_executable(image_path: &str, windows_dir: &Path) -> Option<PathBuf> {
    let raw = image_path.trim();
    let executable = if let Some(quoted) = raw.strip_prefix('"') {
        let end = quoted.find('"')?;
        &quoted[..end]
    } else {
        let lower = raw.to_ascii_lowercase();
        let end = lower.find(".exe")?.checked_add(4)?;
        if raw.get(end..)?.trim_start().starts_with(':') {
            return None;
        }
        let candidate = raw.get(..end)?;
        if candidate.chars().any(char::is_whitespace) {
            return None;
        }
        candidate
    };
    if executable.is_empty()
        || executable.starts_with("\\\\")
        || executable.starts_with("\\??\\")
        || executable.starts_with("\\\\?\\")
    {
        return None;
    }

    let windows = windows_dir.to_string_lossy();
    let lower = executable.to_ascii_lowercase();
    let expanded = if lower.starts_with("%systemroot%") || lower.starts_with("%windir%") {
        let token_len = if lower.starts_with("%systemroot%") {
            "%systemroot%".len()
        } else {
            "%windir%".len()
        };
        format!(
            "{}{}",
            windows.trim_end_matches('\\'),
            &executable[token_len..]
        )
    } else if lower.starts_with("\\systemroot\\") {
        format!(
            "{}{}",
            windows.trim_end_matches('\\'),
            &executable[r"\systemroot".len()..]
        )
    } else if lower.starts_with("%systemdrive%") {
        let drive = windows.get(..2)?;
        format!("{drive}{}", &executable["%systemdrive%".len()..])
    } else {
        executable.to_string()
    };
    if expanded.contains('%') {
        return None;
    }
    let path = PathBuf::from(expanded);
    let path_text = path.to_string_lossy();
    let has_alternate_stream = path_text.chars().skip(2).any(|character| character == ':');
    let has_parent_traversal = path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir));
    (path.is_absolute()
        && !has_alternate_stream
        && !has_parent_traversal
        && service_path_has_no_reparse_components(&path))
    .then_some(path)
}

fn path_is_within(root: &Path, candidate: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase()
            .trim_end_matches('\\')
            .to_string()
    };
    let root = normalize(root);
    let candidate = normalize(candidate);
    candidate == root
        || candidate
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

fn service_image_is_modifiable(
    image_path: &str,
    windows_dir: &Path,
    program_files_dirs: &[PathBuf],
) -> bool {
    service_image_is_modifiable_with_acl(
        image_path,
        windows_dir,
        program_files_dirs,
        service_image_acl_is_protected,
    )
}

fn service_image_is_modifiable_with_acl(
    image_path: &str,
    windows_dir: &Path,
    program_files_dirs: &[PathBuf],
    mut acl_is_protected: impl FnMut(&Path) -> bool,
) -> bool {
    let Some(original_executable) = service_image_executable(image_path, windows_dir) else {
        return false;
    };
    let Ok(executable) = std::fs::canonicalize(&original_executable) else {
        return false;
    };
    let Ok(windows_dir) = std::fs::canonicalize(windows_dir) else {
        return false;
    };
    let protected_root_matches = program_files_dirs.iter().any(|root| {
        std::fs::canonicalize(root).is_ok_and(|root| path_is_within(&root, &executable))
    });
    if !std::fs::metadata(&executable).is_ok_and(|metadata| metadata.is_file())
        || path_is_within(&windows_dir, &executable)
        || !protected_root_matches
        || !acl_is_protected(&original_executable)
    {
        return false;
    }
    let original_key = original_executable
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    let canonical_key = executable
        .to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase();
    original_key == canonical_key || acl_is_protected(&executable)
}

fn apply_service_startup_pair(
    first: impl FnOnce() -> Result<(), String>,
    second: impl FnOnce() -> Result<(), String>,
    rollback: impl FnOnce() -> Result<usize, String>,
) -> Result<(), String> {
    first()?;
    if let Err(second_error) = second() {
        return match rollback() {
            Ok(restored) if restored > 0 => Err(format!(
                "第二项服务启动设置失败：{second_error}；已自动回滚 {restored} 项首次写入"
            )),
            Ok(_) => Err(format!(
                "第二项服务启动设置失败：{second_error}；回滚组没有可确认的记录，请检查变更日志"
            )),
            Err(rollback_error) => Err(format!(
                "第二项服务启动设置失败：{second_error}；自动回滚也失败：{rollback_error}。请检查变更日志中的恢复记录"
            )),
        };
    }
    Ok(())
}

/// 服务列表短 TTL 缓存：页面挂载时列表命令不再每次重跑 Get-CimInstance（秒级）。
/// 任何服务启动类型变更（set_service_startup）都会主动失效，保证本应用操作后即时刷新。
static SERVICES_CACHE: std::sync::LazyLock<
    std::sync::Mutex<Option<(std::time::Instant, Vec<SvcInfo>)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
const SERVICES_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(30);

pub(crate) fn invalidate_services_cache() {
    if let Ok(mut guard) = SERVICES_CACHE.lock() {
        *guard = None;
    }
}

fn cached_services() -> Option<Vec<SvcInfo>> {
    let guard = SERVICES_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some((at, services)) if at.elapsed() < SERVICES_CACHE_TTL => Some(services.clone()),
        _ => None,
    }
}

fn store_services_cache(services: Vec<SvcInfo>) {
    if let Ok(mut guard) = SERVICES_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), services));
    }
}

fn sort_service_rows(rows: &mut [SvcInfo]) {
    rows.sort_by_cached_key(|service| (!service.modifiable, service.display.to_lowercase()));
}

#[tauri::command]
pub async fn list_services() -> Vec<SvcInfo> {
    if let Some(cached) = cached_services() {
        return cached;
    }
    let services = tauri::async_runtime::spawn_blocking(|| {
        let raw = crate::util::run_ps(
            "Get-CimInstance Win32_Service | Select-Object Name,DisplayName,StartMode,State,PathName,StartName | ConvertTo-Json -Compress -Depth 2",
        );
        let v: serde_json::Value = match raw.and_then(|r| serde_json::from_str(&r).ok()) {
            Some(v) => v,
            None => return Vec::new(),
        };
        let arr: Vec<serde_json::Value> = match v {
            serde_json::Value::Array(a) => a,
            obj @ serde_json::Value::Object(_) => vec![obj],
            _ => vec![],
        };
        let services = services_key();
        let windows_dir = crate::util::windows_directory().ok();
        let program_files_dirs = crate::util::program_files_directories().ok();
        let mut acl_cache: std::collections::HashMap<String, bool> =
            std::collections::HashMap::new();
        let mut out: Vec<SvcInfo> = arr
            .into_iter()
            .map(|s| {
                let name = s["Name"].as_str().unwrap_or("").to_string();
                let path = s["PathName"].as_str().unwrap_or("").to_string();
                let account = s["StartName"].as_str().unwrap_or("").to_string();
                let (start_reg, delayed, image_path, registry_account) = services
                    .as_ref()
                    .map(|k| read_service_reg(k, &name))
                    .unwrap_or((None, None, None, None));
                let normalized_name = name.to_ascii_lowercase();
                let account_valid = service_account_is_supported(&name, &account)
                    && registry_account
                        .as_deref()
                        .is_some_and(|registry| service_accounts_match(registry, &account));
                let modifiable = !CRITICAL_SERVICES.contains(&normalized_name.as_str())
                    && account_valid
                    && match (
                        image_path.as_deref(),
                        windows_dir.as_deref(),
                        program_files_dirs.as_deref(),
                    ) {
                        (Some(image_path), Some(windows_dir), Some(program_files_dirs)) => {
                            service_image_is_modifiable_with_acl(
                                image_path,
                                windows_dir,
                                program_files_dirs,
                                |executable| {
                                    let key = executable.to_string_lossy().to_ascii_lowercase();
                                    *acl_cache
                                        .entry(key)
                                        .or_insert_with(|| service_image_acl_is_protected(executable))
                                },
                            ) && registry_account.as_deref().is_some_and(|registry| {
                                service_scm_account_matches(&name, registry)
                            })
                        }
                        _ => false,
                    };
                SvcInfo {
                    name: name.clone(),
                    display: s["DisplayName"].as_str().unwrap_or(&name).to_string(),
                    start_mode: s["StartMode"].as_str().unwrap_or("").to_string(),
                    state: s["State"].as_str().unwrap_or("").to_string(),
                    path,
                    account,
                    modifiable,
                    start_reg,
                    delayed,
                }
            })
            .collect();
        sort_service_rows(&mut out);
        out
    })
    .await
    .unwrap_or_default();
    // 空结果不缓存（PowerShell 失败时 spawn_blocking 返回 Vec::new，缓存会固化故障）
    if !services.is_empty() {
        store_services_cache(services.clone());
    }
    services
}

fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '$')
}

fn service_name_allowed_for_tweaks(name: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    valid_service_name(name) && !CRITICAL_SERVICES.contains(&normalized.as_str())
}

/// Recheck the critical-service and protected Program Files image policy before applying persisted undo.
pub(crate) fn service_restore_target_allowed(name: &str) -> bool {
    if !service_name_allowed_for_tweaks(name) {
        return false;
    }
    let Some(services) = services_key() else {
        return false;
    };
    let (_, _, Some(image_path), Some(account)) = read_service_reg(&services, name) else {
        return false;
    };
    if !service_account_is_supported(name, &account) || !service_scm_account_matches(name, &account)
    {
        return false;
    }
    let Ok(windows_dir) = crate::util::windows_directory() else {
        return false;
    };
    let Ok(program_files_dirs) = crate::util::program_files_directories() else {
        return false;
    };
    service_image_is_modifiable(&image_path, &windows_dir, &program_files_dirs)
}

/// 启动类型切换：mode = auto | delayed | manual | disabled
#[tauri::command]
pub fn set_service_startup(name: String, mode: String) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("服务管理需要管理员权限，请在「设置」页提权后重试".into());
    }
    if !valid_service_name(&name) {
        return Err("服务名不合法".into());
    }
    let n = name.to_lowercase();
    if CRITICAL_SERVICES.contains(&n.as_str()) {
        return Err(format!("「{name}」为系统关键服务，AeroTune 拒绝修改"));
    }
    let start: u32 = match mode.as_str() {
        "auto" | "delayed" => 2,
        "manual" => 3,
        "disabled" => 4,
        _ => return Err(format!("未知启动类型: {mode}")),
    };

    let windows_dir = crate::util::windows_directory()?;
    let program_files_dirs = crate::util::program_files_directories()?;
    let services =
        services_key().ok_or_else(|| "无法安全读取服务注册信息；拒绝修改".to_string())?;
    let (_, _, image_path, account) = read_service_reg(&services, &name);
    let account = account.ok_or_else(|| "无法确认服务运行账户；拒绝修改".to_string())?;
    if !service_account_is_supported(&name, &account) {
        return Err("服务使用未审查的自定义或域账户；拒绝修改启动类型".into());
    }
    if !service_scm_account_matches(&name, &account) {
        return Err("服务管理器账户与注册表配置缺失或不一致；拒绝修改启动类型".into());
    }
    let image_path = image_path.ok_or_else(|| "无法确认服务可执行路径；拒绝修改".to_string())?;
    if !service_image_is_modifiable(&image_path, &windows_dir, &program_files_dirs) {
        return Err(
            "服务镜像路径、文件/父目录 ACL 或受保护的 Program Files 根未通过验证；拒绝修改启动类型"
                .into(),
        );
    }

    crate::admin::auto_restore_point(&format!("修改服务 {} 启动类型", name));
    let path = format!(r"HKLM\SYSTEM\CurrentControlSet\Services\{name}");
    let label = match mode.as_str() {
        "auto" => "自动",
        "delayed" => "自动（延迟启动）",
        "manual" => "手动",
        _ => "禁用",
    };
    let group = crate::journal::begin_change_group(&format!("服务「{name}」启动类型 → {label}"))?;
    let group_id = group.id();
    let result = apply_service_startup_pair(
        || {
            crate::journal::set_reg_dword(
                &path,
                "Start",
                start,
                &format!("服务「{name}」启动类型 → {label}"),
            )
        },
        || match mode.as_str() {
            "delayed" => crate::journal::set_reg_dword(
                &path,
                "DelayedAutostart",
                1,
                &format!("服务「{name}」启用延迟启动"),
            ),
            _ => crate::journal::del_reg_value(
                &path,
                "DelayedAutostart",
                &format!("服务「{name}」取消延迟启动"),
            ),
        },
        || crate::journal::revert_change_group(group_id),
    );
    drop(group);
    result?;
    invalidate_services_cache();
    tracing::info!(service = %name, mode = %mode, "服务启动类型已修改");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    fn synthetic_service_row(name: &str, display: &str, modifiable: bool) -> SvcInfo {
        SvcInfo {
            name: name.to_string(),
            display: display.to_string(),
            start_mode: "auto".to_string(),
            state: "running".to_string(),
            path: format!("C:\\Program Files\\Vendor\\{name}.exe"),
            account: "LocalSystem".to_string(),
            modifiable,
            start_reg: Some(if modifiable { 2 } else { 3 }),
            delayed: Some(0),
        }
    }

    fn reference_sort_service_rows(rows: &mut [SvcInfo]) {
        rows.sort_by(|a, b| {
            b.modifiable
                .cmp(&a.modifiable)
                .then(a.display.to_lowercase().cmp(&b.display.to_lowercase()))
        });
    }

    fn service_rows_snapshot(
        rows: &[SvcInfo],
    ) -> Vec<(
        String,
        String,
        String,
        String,
        String,
        String,
        bool,
        Option<u32>,
        Option<u32>,
    )> {
        rows.iter()
            .map(|row| {
                (
                    row.name.clone(),
                    row.display.clone(),
                    row.start_mode.clone(),
                    row.state.clone(),
                    row.path.clone(),
                    row.account.clone(),
                    row.modifiable,
                    row.start_reg,
                    row.delayed,
                )
            })
            .collect()
    }

    #[test]
    fn cached_service_sort_matches_reference_order_and_preserves_rows() {
        let input = vec![
            synthetic_service_row("alpha-disabled", "alpha", false),
            synthetic_service_row("zeta", "Zeta", true),
            synthetic_service_row("alpha-first", "Alpha", true),
            synthetic_service_row("alpha-second", "ALPHA", true),
            synthetic_service_row("beta-disabled", "Beta", false),
            synthetic_service_row("unicode-upper", "Äpp Service", true),
            synthetic_service_row("unicode-lower", "äpp service", false),
        ];
        let mut expected = input.clone();
        let mut actual = input;
        reference_sort_service_rows(&mut expected);
        sort_service_rows(&mut actual);
        assert_eq!(
            service_rows_snapshot(&actual),
            service_rows_snapshot(&expected)
        );
        assert_eq!(actual[0].name, "alpha-first");
        assert_eq!(actual[1].name, "alpha-second");
        assert_eq!(actual[2].name, "zeta");
        assert_eq!(actual[3].name, "unicode-upper");
    }

    #[test]
    #[ignore = "manual Release benchmark for cached service sort keys"]
    fn benchmark_service_sort_cached_unicode_keys() {
        const ROWS: usize = 500;
        const BATCHES: usize = 101;
        let display_prefixes = [
            "Alpha",
            "alpha",
            "ALPHA",
            "Beta",
            "beta",
            "Équipement",
            "équipement",
            "Zeta",
        ];
        let rows: Vec<_> = (0..ROWS)
            .map(|index| {
                synthetic_service_row(
                    &format!("service-{index:04}"),
                    &format!(
                        "{} Service {:02}",
                        display_prefixes[index % display_prefixes.len()],
                        index % 64
                    ),
                    !index.is_multiple_of(5),
                )
            })
            .collect();
        let mut reference = rows.clone();
        let mut cached = rows.clone();
        reference_sort_service_rows(&mut reference);
        sort_service_rows(&mut cached);
        assert_eq!(
            service_rows_snapshot(&cached),
            service_rows_snapshot(&reference)
        );

        fn measure(rows: &[SvcInfo], cached_keys: bool) -> u128 {
            let mut sorted = rows.to_vec();
            let started = std::time::Instant::now();
            if cached_keys {
                sort_service_rows(&mut sorted);
            } else {
                reference_sort_service_rows(&mut sorted);
            }
            std::hint::black_box(&sorted);
            started.elapsed().as_nanos()
        }

        let mut reference_nanos = Vec::with_capacity(BATCHES);
        let mut cached_nanos = Vec::with_capacity(BATCHES);
        for warmup in 0..7 {
            if warmup % 2 == 0 {
                measure(&rows, false);
                measure(&rows, true);
            } else {
                measure(&rows, true);
                measure(&rows, false);
            }
        }
        for batch in 0..BATCHES {
            if batch % 2 == 0 {
                reference_nanos.push(measure(&rows, false));
                cached_nanos.push(measure(&rows, true));
            } else {
                cached_nanos.push(measure(&rows, true));
                reference_nanos.push(measure(&rows, false));
            }
        }
        reference_nanos.sort_unstable();
        cached_nanos.sort_unstable();
        let p95_index = (BATCHES * 95).div_ceil(100) - 1;
        eprintln!(
            "service sort ({} rows): comparator lowercase median/p95={:.1}/{:.1} us; cached lowercase keys={:.1}/{:.1} us",
            ROWS,
            reference_nanos[BATCHES / 2] as f64 / 1_000.0,
            reference_nanos[p95_index] as f64 / 1_000.0,
            cached_nanos[BATCHES / 2] as f64 / 1_000.0,
            cached_nanos[p95_index] as f64 / 1_000.0,
        );
    }

    #[test]
    fn service_name_validation() {
        assert!(valid_service_name("NvContainerLocalSystem"));
        assert!(valid_service_name("wuauserv"));
        assert!(!valid_service_name(""));
        assert!(!valid_service_name("foo bar"));
        assert!(!valid_service_name("../x"));
        assert!(!valid_service_name(&"a".repeat(65)));
    }

    #[test]
    fn service_image_scope_fails_closed_and_requires_a_protected_install_root() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "aerotune-service-scope-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let windows = root.join("Windows");
        let system32 = windows.join("System32");
        let program_files = root.join("Program Files");
        let vendor_dir = program_files.join("Vendor");
        let profile_dir = root.join("Users").join("Alice").join("AppData");
        let windows_old = root.join("Windows.old");
        let program_files_roots = vec![program_files.clone()];
        std::fs::create_dir_all(&system32).unwrap();
        std::fs::create_dir_all(&vendor_dir).unwrap();
        std::fs::create_dir_all(&profile_dir).unwrap();
        std::fs::create_dir_all(&windows_old).unwrap();
        std::fs::write(system32.join("svchost.exe"), b"system").unwrap();
        std::fs::write(vendor_dir.join("service.exe"), b"vendor").unwrap();
        std::fs::write(profile_dir.join("service.exe"), b"user-writable").unwrap();
        std::fs::write(windows_old.join("service.exe"), b"vendor").unwrap();

        let scope_is_modifiable = |image_path: &str| {
            service_image_is_modifiable_with_acl(image_path, &windows, &program_files_roots, |_| {
                true
            })
        };
        assert!(!scope_is_modifiable(
            r"%SystemRoot%\System32\svchost.exe -k netsvcs"
        ));
        assert!(!scope_is_modifiable(
            r"\SystemRoot\System32\svchost.exe -k netsvcs"
        ));
        let vendor_executable = vendor_dir.join("service.exe");
        let vendor = format!("\"{}\" --service", vendor_executable.display());
        assert!(scope_is_modifiable(&vendor));
        assert!(!scope_is_modifiable(&format!(
            "{}:stream",
            vendor_executable.display()
        )));
        assert!(!scope_is_modifiable(&format!(
            "\"{}:stream\"",
            vendor_executable.display()
        )));
        let unquoted_vendor = format!("{} --service", vendor_executable.display());
        assert!(!scope_is_modifiable(&unquoted_vendor));
        let user_profile = format!("\"{}\"", profile_dir.join("service.exe").display());
        assert!(!scope_is_modifiable(&user_profile));
        assert!(!scope_is_modifiable(r"%UNKNOWN_ROOT%\Vendor\service.exe"));
        assert!(!path_is_within(&windows, &windows_old.join("service.exe")));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn service_account_policy_allows_only_builtin_or_matching_virtual_accounts() {
        assert!(service_account_is_supported("VendorSvc", "LocalSystem"));
        assert!(service_account_is_supported(
            "VendorSvc",
            "NT AUTHORITY\\LocalService"
        ));
        assert!(service_account_is_supported(
            "VendorSvc",
            "NT AUTHORITY\\NetworkService"
        ));
        assert!(service_account_is_supported(
            "VendorSvc",
            "NT SERVICE\\VendorSvc"
        ));
        assert!(!service_account_is_supported(
            "VendorSvc",
            "NT SERVICE\\OtherSvc"
        ));
        assert!(!service_account_is_supported(
            "VendorSvc",
            "CONTOSO\\svc-admin"
        ));
        assert!(!service_account_is_supported("VendorSvc", ""));
        assert!(service_accounts_match(
            "LocalSystem",
            "NT AUTHORITY\\SYSTEM"
        ));
        assert!(!service_accounts_match("LocalSystem", "CONTOSO\\svc-admin"));
        assert!(service_account_configurations_match(
            "VendorSvc",
            "LocalSystem",
            "NT AUTHORITY\\SYSTEM"
        ));
        assert!(service_account_configurations_match(
            "VendorSvc",
            "NT SERVICE\\VendorSvc",
            "NT SERVICE\\vendorsvc"
        ));
        assert!(!service_account_configurations_match(
            "VendorSvc",
            "LocalSystem",
            "CONTOSO\\svc-admin"
        ));
        assert!(!service_account_configurations_match(
            "VendorSvc",
            "LocalSystem",
            "LocalService"
        ));
        assert!(!service_account_configurations_match(
            "VendorSvc",
            "LocalSystem",
            ""
        ));
    }

    #[test]
    fn service_scm_account_null_and_type_policy_fails_closed() {
        use windows_sys::Win32::System::Services::{
            SERVICE_KERNEL_DRIVER, SERVICE_WIN32_OWN_PROCESS, SERVICE_WIN32_SHARE_PROCESS,
        };

        assert_eq!(
            service_account_from_scm_config(SERVICE_WIN32_OWN_PROCESS, None),
            Some("LocalSystem".to_string())
        );
        assert_eq!(
            service_account_from_scm_config(SERVICE_WIN32_SHARE_PROCESS, None),
            Some("LocalSystem".to_string())
        );
        assert_eq!(
            service_account_from_scm_config(
                SERVICE_WIN32_OWN_PROCESS,
                Some("NT AUTHORITY\\LocalService")
            ),
            Some("NT AUTHORITY\\LocalService".to_string())
        );
        assert_eq!(
            service_account_from_scm_config(SERVICE_KERNEL_DRIVER, None),
            None
        );
        assert_eq!(
            service_account_from_scm_config(SERVICE_KERNEL_DRIVER, Some("LocalSystem")),
            None
        );
        assert_eq!(
            service_account_from_scm_config(
                SERVICE_WIN32_OWN_PROCESS | 0x0000_0100, // SERVICE_INTERACTIVE_PROCESS
                Some("LocalSystem")
            ),
            None
        );
    }

    #[test]
    #[ignore = "live read-only SCM query; run explicitly on a Windows host"]
    fn live_scm_query_matches_registry_accounts() {
        let services = services_key().expect("could not open the Windows service registry");
        let mut checked = false;
        for name in ["RpcSs", "edgeupdate"] {
            let (_, _, _, Some(registry_account)) = read_service_reg(&services, name) else {
                continue;
            };
            let Some(scm_account) = query_scm_service_account(name) else {
                if name == "RpcSs" {
                    panic!("SCM query for the built-in RpcSs service failed");
                }
                continue;
            };
            eprintln!("SCM account: {name}={scm_account}");
            assert!(service_account_configurations_match(
                name,
                &registry_account,
                &scm_account
            ));
            checked = true;
        }
        assert!(checked, "no live service account was verified");
    }

    #[test]
    fn service_acl_query_reads_protected_install_roots() {
        let roots = crate::util::program_files_directories().unwrap();
        assert!(!roots.is_empty());
        for root in roots {
            let snapshot = read_service_acl_snapshot(&root).unwrap_or_else(|| {
                panic!("could not read Program Files ACL at {}", root.display())
            });
            eprintln!(
                "Program Files root ACL chain: path={}, trusted_policy_pass={}",
                root.display(),
                service_image_acl_is_protected(&root)
            );
            assert!(service_acl_snapshot_is_protected(&snapshot));
            assert!(service_image_acl_is_protected(&root));
            let canonical_root = std::fs::canonicalize(&root).unwrap();
            assert!(service_image_acl_is_protected(&canonical_root));
        }
    }

    #[test]
    fn service_acl_policy_rejects_untrusted_write_and_ownership_paths() {
        let trusted_owner = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
        let admin = "S-1-5-32-544";
        let user = "S-1-5-32-545";
        let safe = ServiceAclSnapshot {
            owner_sid: trusted_owner.to_string(),
            is_directory: false,
            is_volume_root: false,
            aces: vec![
                ServiceAclAce {
                    sid: admin.to_string(),
                    mask: SERVICE_FILE_WRITE_MASK,
                    allow: true,
                    inherit_only: false,
                },
                ServiceAclAce {
                    sid: user.to_string(),
                    mask: 0x0012_0089, // read, read-attributes, read-control, synchronize
                    allow: true,
                    inherit_only: false,
                },
            ],
        };
        assert!(service_acl_snapshot_is_protected(&safe));
        assert!(service_acl_chain_is_protected([safe.clone()]));
        assert!(!service_acl_chain_is_protected(
            Vec::<ServiceAclSnapshot>::new()
        ));

        let unsafe_masks = [
            0x0000_0002, // write data / add file
            0x0000_0004, // append / add subdirectory
            0x0000_0010, // write extended attributes
            0x0000_0100, // write attributes
            0x0001_0000, // delete
            0x0004_0000, // write DACL
            0x0008_0000, // take ownership
            0x0200_0000, // maximum allowed
            0x1000_0000, // generic all
            0x4000_0000, // generic write
        ];
        for mask in unsafe_masks {
            let snapshot = ServiceAclSnapshot {
                owner_sid: trusted_owner.to_string(),
                is_directory: false,
                is_volume_root: false,
                aces: vec![ServiceAclAce {
                    sid: user.to_string(),
                    mask,
                    allow: true,
                    inherit_only: false,
                }],
            };
            assert!(
                !service_acl_snapshot_is_protected(&snapshot),
                "mask {mask:#x}"
            );
        }
        let user_owned = ServiceAclSnapshot {
            owner_sid: user.to_string(),
            is_directory: false,
            is_volume_root: false,
            aces: Vec::new(),
        };
        assert!(!service_acl_snapshot_is_protected(&user_owned));
        let volume_root_add_subdirectory = ServiceAclSnapshot {
            owner_sid: trusted_owner.to_string(),
            is_directory: true,
            is_volume_root: true,
            aces: vec![ServiceAclAce {
                sid: user.to_string(),
                mask: 0x0000_0004,
                allow: true,
                inherit_only: false,
            }],
        };
        assert!(service_acl_snapshot_is_protected(
            &volume_root_add_subdirectory
        ));
        for mask in [0x0000_0002, 0x0000_0004, 0x0000_0040] {
            let directory_writer = ServiceAclSnapshot {
                owner_sid: trusted_owner.to_string(),
                is_directory: true,
                is_volume_root: false,
                aces: vec![ServiceAclAce {
                    sid: user.to_string(),
                    mask,
                    allow: true,
                    inherit_only: false,
                }],
            };
            assert!(
                !service_acl_snapshot_is_protected(&directory_writer),
                "directory mask {mask:#x}"
            );
        }
        let custom_sid_writer = ServiceAclSnapshot {
            owner_sid: trusted_owner.to_string(),
            is_directory: true,
            is_volume_root: false,
            aces: vec![ServiceAclAce {
                sid: "S-1-5-21-111-222-333-1001".to_string(),
                mask: 0x0000_0040,
                allow: true,
                inherit_only: false,
            }],
        };
        assert!(!service_acl_snapshot_is_protected(&custom_sid_writer));
    }

    #[test]
    fn critical_blocked() {
        assert!(CRITICAL_SERVICES.contains(&"rpcss"));
        assert!(CRITICAL_SERVICES.contains(&"winmgmt"));
        assert!(CRITICAL_SERVICES.contains(&"windefend"));
        assert!(CRITICAL_SERVICES.contains(&"wuauserv"));
        assert!(CRITICAL_SERVICES.contains(&"trustedinstaller"));
        assert!(!CRITICAL_SERVICES.contains(&"nvcontainer"));
        assert!(!service_name_allowed_for_tweaks("RPCSS"));
        assert!(!service_name_allowed_for_tweaks("Windows\\\\Update"));
        assert!(service_name_allowed_for_tweaks("NvContainer"));
    }

    #[test]
    fn service_startup_pair_rolls_back_first_write_when_second_fails() {
        let first_applied = std::sync::Arc::new(AtomicBool::new(false));
        let apply_state = std::sync::Arc::clone(&first_applied);
        let rollback_state = std::sync::Arc::clone(&first_applied);
        let error = apply_service_startup_pair(
            move || {
                apply_state.store(true, Ordering::SeqCst);
                Ok(())
            },
            || Err("injected delayed-value write failure".into()),
            move || {
                rollback_state.store(false, Ordering::SeqCst);
                Ok(1)
            },
        )
        .unwrap_err();

        assert!(!first_applied.load(Ordering::SeqCst));
        assert!(error.contains("已自动回滚 1 项"));
        assert!(error.contains("injected delayed-value write failure"));
    }

    #[test]
    fn service_startup_pair_reports_rollback_failure_and_stops_after_first_error() {
        let error = apply_service_startup_pair(
            || Ok(()),
            || Err("injected second-write failure".into()),
            || Err("injected journal conflict".into()),
        )
        .unwrap_err();
        assert!(error.contains("自动回滚也失败"));
        assert!(error.contains("injected journal conflict"));

        let first_error = apply_service_startup_pair(
            || Err("injected first-write failure".into()),
            || panic!("second write must not run"),
            || panic!("rollback must not run"),
        )
        .unwrap_err();
        assert_eq!(first_error, "injected first-write failure");
    }
}
