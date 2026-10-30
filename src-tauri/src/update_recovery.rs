//! Transaction-journal contract and read-only ProgramData startup gate.
//!
//! With recovery required, the Windows gate refuses startup if the machine-wide journal store has
//! an unexpected owner/DACL, a reparse point, or any state entry. With updates hard-off, a missing
//! store is tolerated; an untrusted same-name directory is ignored read-only to avoid user-created
//! ProgramData startup denial, while a protected store is still inspected and unknown state blocks.
//! A private atomic writer exists but is not integrated; provisioning and recovery remain absent.
//! Updates therefore remain disabled.

use serde::{Deserialize, Serialize};

/// Fail closed when a machine-wide recovery record or an unexpected state file is present.
pub fn ensure_no_pending_journal(store_required: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        windows_store::ensure_no_pending_journal(store_required)
    }
    #[cfg(not(windows))]
    {
        if store_required {
            Err("Windows 更新恢复存储在此平台不可用。".into())
        } else {
            Ok(())
        }
    }
}

fn is_missing_optional_store(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound
}

#[cfg(windows)]
#[allow(dead_code)]
fn create_prepared_journal(
    _gate: &crate::updater::UpdateGateGuard,
    journal: &UpdateTransactionJournal,
) -> Result<(), String> {
    if !crate::updater::RECOVERY_STORE_REQUIRED {
        return Err("更新恢复存储写入器在当前版本 readiness 未启用。".into());
    }
    windows_store::create_prepared(journal)
}

#[cfg(windows)]
#[allow(dead_code)]
fn advance_journal(
    _gate: &crate::updater::UpdateGateGuard,
    expected: &UpdateTransactionJournal,
    next: &UpdateTransactionJournal,
) -> Result<(), String> {
    if !crate::updater::RECOVERY_STORE_REQUIRED {
        return Err("更新恢复存储写入器在当前版本 readiness 未启用。".into());
    }
    windows_store::advance(expected, next)
}

#[cfg(test)]
fn reject_any_directory_entry<T>(
    mut entries: impl Iterator<Item = std::io::Result<T>>,
) -> Result<(), String> {
    if entries.next().is_some() {
        Err("检测到待恢复或未知的更新状态文件；已拒绝启动和新更新。".into())
    } else {
        Ok(())
    }
}

trait JournalCommitIo {
    type FileId: Eq;

    fn preflight(&mut self) -> Result<(), String>;
    fn create_temp(&mut self) -> Result<(), String>;
    fn inspect_temp(&mut self) -> Result<Self::FileId, String>;
    fn validate_temp_security(&mut self) -> Result<(), String>;
    fn write_temp(&mut self, bytes: &[u8]) -> Result<(), String>;
    fn flush_temp(&mut self) -> Result<(), String>;
    fn rename_temp(&mut self, replace_existing: bool) -> Result<(), String>;
    fn verify_final(
        &mut self,
        expected: &UpdateTransactionJournal,
        expected_bytes: &[u8],
        staged_file_id: &Self::FileId,
    ) -> Result<(), String>;
    fn flush_final(&mut self) -> Result<(), String>;
    fn verify_final_directory(&mut self) -> Result<(), String>;
}

fn commit_journal<I: JournalCommitIo>(
    io: &mut I,
    journal: &UpdateTransactionJournal,
    replace_existing: bool,
) -> Result<(), String> {
    let bytes = journal.encode()?;
    io.preflight()?;
    io.create_temp()?;
    let staged_file_id = io.inspect_temp()?;
    io.validate_temp_security()?;
    io.write_temp(&bytes)?;
    io.flush_temp()?;
    io.rename_temp(replace_existing)?;
    io.verify_final(journal, &bytes, &staged_file_id)?;
    io.flush_final()?;
    io.verify_final_directory()
}

#[cfg(windows)]
mod windows_store {
    use super::{
        commit_journal, validate_journal_advance, validate_prepared_journal, JournalCommitIo,
        UpdateTransactionJournal,
    };
    use std::ffi::c_void;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Path, PathBuf, Prefix};
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND,
        GENERIC_ALL, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
        GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetLengthSid,
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl, IsValidSid, ACCESS_ALLOWED_ACE,
        ACE_HEADER, ACL, ACL_SIZE_INFORMATION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
        INHERIT_ONLY_ACE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
        PSID, SECURITY_ATTRIBUTES, SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, GetDriveTypeW, GetFileInformationByHandle, MoveFileExW,
        ReadFile, WriteFile, BY_HANDLE_FILE_INFORMATION, CREATE_NEW, FILE_ADD_FILE,
        FILE_ADD_SUBDIRECTORY, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_FLAG_WRITE_THROUGH, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_READ_ATTRIBUTES,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_EA,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, OPEN_EXISTING, READ_CONTROL, WRITE_DAC,
    };
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE,
    };
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;
    use windows_sys::Win32::UI::Shell::{FOLDERID_ProgramData, SHGetKnownFolderPath};

    const PROGRAM_DATA_SUBDIR: &str = "AeroTuneUpdater";
    const JOURNAL_SUBDIR: &str = "State";
    const JOURNAL_FILE: &str = "transaction.json";
    const JOURNAL_TEMP_FILE: &str = "transaction.json.next";
    const JOURNAL_FILE_DACL: &str = "D:P(D;;WD;;;OW)(A;;FA;;;SY)(A;;FA;;;BA)(A;;FR;;;AU)";

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct LocalMemory(*mut c_void);

    impl Drop for LocalMemory {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    LocalFree(self.0 as HLOCAL);
                }
            }
        }
    }

    struct VerifiedStore {
        _volume_root: Handle,
        _program_data: Handle,
        _state_root: Handle,
        state_dir: Handle,
        journal_dir: PathBuf,
    }

    fn open_program_data_directories(program_data: &Path) -> Result<(Handle, Handle), String> {
        let volume_root = program_data
            .parent()
            .ok_or_else(|| "ProgramData 的卷根目录无效。".to_string())?;
        let volume_handle = open_directory(volume_root)?
            .ok_or_else(|| "无法打开 ProgramData 所在卷根目录。".to_string())?;
        let program_data_handle = open_directory(program_data)?
            .ok_or_else(|| "无法打开系统 ProgramData 目录。".to_string())?;
        Ok((volume_handle, program_data_handle))
    }

    fn open_verified_program_data(program_data: &Path) -> Result<(Handle, Handle), String> {
        let (volume_handle, program_data_handle) = open_program_data_directories(program_data)?;
        if !has_expected_program_data_security(&program_data_handle)? {
            return Err("系统 ProgramData 目录的所有者或 DACL 不符合机器级策略。".into());
        }
        Ok((volume_handle, program_data_handle))
    }

    fn open_verified_store() -> Result<VerifiedStore, String> {
        let program_data = program_data_path()?;
        open_verified_store_at(&program_data)
    }

    fn open_verified_store_at(program_data: &Path) -> Result<VerifiedStore, String> {
        let (volume_handle, program_data_handle) = open_verified_program_data(program_data)?;
        let state_root = program_data.join(PROGRAM_DATA_SUBDIR);
        let root_handle = open_directory(&state_root)?
            .ok_or_else(|| "ProgramData 更新恢复存储尚未由受信任安装器预置。".to_string())?;
        if !has_expected_directory_security(&root_handle)? {
            return Err("ProgramData 更新状态根目录的所有者或 DACL 不符合固定策略。".into());
        }

        let journal_dir = state_root.join(JOURNAL_SUBDIR);
        let state_dir = open_directory(&journal_dir)?
            .ok_or_else(|| "ProgramData 更新日志目录尚未由受信任安装器预置。".to_string())?;
        if !has_expected_directory_security(&state_dir)? {
            return Err("ProgramData 更新日志目录的所有者或 DACL 不符合固定策略。".into());
        }

        Ok(VerifiedStore {
            _volume_root: volume_handle,
            _program_data: program_data_handle,
            _state_root: root_handle,
            state_dir,
            journal_dir,
        })
    }

    fn ensure_optional_store_at(program_data: &Path) -> Result<(), String> {
        // This compatibility path is used only while all updater readiness flags are hard-off.
        // A standard user can create arbitrary children under ProgramData, so do not let an
        // unprovisioned, untrusted AeroTuneUpdater directory become a global startup DoS. Never
        // mutate it; a protected store, when present, is still checked strictly below.
        let (_volume_handle, _program_data_handle) = open_program_data_directories(program_data)?;
        let state_root = program_data.join(PROGRAM_DATA_SUBDIR);
        match std::fs::symlink_metadata(&state_root) {
            Err(error) if super::is_missing_optional_store(&error) => return Ok(()),
            // The optional subtree is not a trusted recovery store. In hard-off mode, failures
            // to read it must not let a user-controlled ACL create a global app-startup DoS.
            // Required mode below always opens and validates the full protected store.
            Err(_) => return Ok(()),
            Ok(_) => {}
        }

        let root_handle = match open_directory(&state_root) {
            Ok(Some(handle)) => handle,
            Ok(None) | Err(_) => return Ok(()),
        };
        if !matches!(has_expected_directory_security(&root_handle), Ok(true)) {
            return Ok(());
        }

        let store = open_verified_store_at(program_data)?;
        if list_state_entries(&store)?.is_empty() {
            Ok(())
        } else {
            Err("检测到待恢复或未知的更新状态文件；已拒绝启动和新更新。".into())
        }
    }

    pub fn ensure_no_pending_journal(store_required: bool) -> Result<(), String> {
        if !store_required {
            return ensure_optional_store_at(&program_data_path()?);
        }
        let store = open_verified_store()?;
        if list_state_entries(&store)?.is_empty() {
            Ok(())
        } else {
            Err("检测到待恢复或未知的更新状态文件；已拒绝启动和新更新。".into())
        }
    }

    #[allow(dead_code)]
    pub fn create_prepared(journal: &UpdateTransactionJournal) -> Result<(), String> {
        validate_prepared_journal(journal)?;
        let store = open_verified_store()?;
        if !list_state_entries(&store)?.is_empty() {
            return Err("更新日志目录非空；拒绝覆盖或清理现有证据。".into());
        }
        write_journal_atomically(&store, journal, false)
    }

    #[allow(dead_code)]
    pub fn advance(
        expected: &UpdateTransactionJournal,
        next: &UpdateTransactionJournal,
    ) -> Result<(), String> {
        validate_journal_advance(expected, next, expected)?;
        let store = open_verified_store()?;
        let entries = list_state_entries(&store)?;
        if !contains_exact_names(&entries, &[JOURNAL_FILE]) {
            return Err("更新日志目录包含缺失、临时或未知项；拒绝推进事务。".into());
        }
        let current = read_journal_file(&store)?;
        validate_journal_advance(expected, next, &current)?;
        write_journal_atomically(&store, next, true)
    }

    fn list_state_entries(store: &VerifiedStore) -> Result<Vec<std::ffi::OsString>, String> {
        let expected_identity = directory_identity(&store.state_dir)?;
        let entries = std::fs::read_dir(&store.journal_dir)
            .map_err(|_| "无法检查 ProgramData 更新日志目录；已拒绝继续。".to_string())?;
        let mut names = Vec::with_capacity(2);
        for entry in entries {
            let entry = entry.map_err(|_| "枚举更新日志目录失败；已拒绝继续。".to_string())?;
            if names.len() == 2 {
                return Err("更新日志目录含过多条目；已拒绝继续。".into());
            }
            names.push(entry.file_name());
        }
        let reopened = open_directory(&store.journal_dir)?
            .ok_or_else(|| "更新日志目录在检查期间消失。".to_string())?;
        if directory_identity(&reopened)? != expected_identity {
            return Err("更新日志目录在检查期间发生替换。".into());
        }
        Ok(names)
    }

    fn directory_identity(handle: &Handle) -> Result<(u32, u64), String> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err("无法确认更新状态目录的固定身份。".into());
        }
        Ok(file_id(&info))
    }

    fn regular_file_identity(handle: &Handle) -> Result<(u32, u64), String> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0
            || info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT)
                != 0
        {
            return Err("更新日志路径不是普通文件或包含重解析点。".into());
        }
        Ok(file_id(&info))
    }

    fn file_id(info: &BY_HANDLE_FILE_INFORMATION) -> (u32, u64) {
        let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
        (info.dwVolumeSerialNumber, index)
    }

    fn contains_exact_names(names: &[std::ffi::OsString], expected: &[&str]) -> bool {
        names.len() == expected.len()
            && expected.iter().all(|expected_name| {
                names
                    .iter()
                    .any(|name| name.as_os_str() == std::ffi::OsStr::new(expected_name))
            })
    }

    fn journal_path(store: &VerifiedStore, leaf: &str) -> PathBuf {
        store.journal_dir.join(leaf)
    }

    fn open_journal_file(
        store: &VerifiedStore,
        leaf: &str,
        desired_access: u32,
    ) -> Result<Handle, String> {
        let path = journal_path(store, leaf);
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                desired_access,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            let error = unsafe { GetLastError() };
            return Err(format!("无法安全打开更新日志文件（Win32 {error}）。"));
        }
        let handle = Handle(handle);
        regular_file_identity(&handle)?;
        if !has_expected_journal_file_security(&handle)? {
            return Err("更新日志文件 DACL 不符合 OWNER RIGHTS 保护策略。".into());
        }
        Ok(handle)
    }

    fn read_journal_file(store: &VerifiedStore) -> Result<UpdateTransactionJournal, String> {
        let handle = open_journal_file(
            store,
            JOURNAL_FILE,
            GENERIC_READ | READ_CONTROL | FILE_READ_ATTRIBUTES,
        )?;
        let bytes = read_bounded_file(&handle)?;
        UpdateTransactionJournal::decode(&bytes)
    }

    fn read_bounded_file(handle: &Handle) -> Result<Vec<u8>, String> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0 {
            return Err("无法读取更新日志文件大小。".into());
        }
        let size = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
        if size == 0 || size > super::MAX_JOURNAL_BYTES as u64 {
            return Err("更新日志文件大小超出允许范围。".into());
        }
        let mut bytes = vec![0; size as usize];
        let mut offset = 0usize;
        while offset < bytes.len() {
            let mut read = 0u32;
            let requested = (bytes.len() - offset) as u32;
            if unsafe {
                ReadFile(
                    handle.0,
                    bytes[offset..].as_mut_ptr(),
                    requested,
                    &mut read,
                    ptr::null_mut(),
                )
            } == 0
            {
                let error = unsafe { GetLastError() };
                return Err(format!("读取更新日志失败（Win32 {error}）。"));
            }
            if read == 0 {
                return Err("更新日志在读取期间意外结束。".into());
            }
            offset += read as usize;
        }
        Ok(bytes)
    }

    fn journal_file_security_attributes() -> Result<(LocalMemory, SECURITY_ATTRIBUTES), String> {
        let sddl: Vec<u16> = JOURNAL_FILE_DACL
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let mut descriptor_size = 0u32;
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                &mut descriptor_size,
            )
        } == 0
            || descriptor.is_null()
        {
            let error = unsafe { GetLastError() };
            return Err(format!("无法构建更新日志文件安全描述符（Win32 {error}）。"));
        }
        let memory = LocalMemory(descriptor.cast());
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        Ok((memory, attributes))
    }

    fn current_process_can_write_journal_file() -> Result<bool, String> {
        use windows_sys::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN};
        use windows_sys::Win32::Security::{
            AccessCheck, DuplicateToken, SecurityImpersonation, GENERIC_MAPPING, PRIVILEGE_SET,
            SECURITY_IMPERSONATION_LEVEL, TOKEN_DUPLICATE, TOKEN_QUERY,
        };
        use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES};
        use windows_sys::Win32::System::Threading::{
            GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
        };

        let sddl = format!("O:SYG:SY{JOURNAL_FILE_DACL}");
        let wide_sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
        let mut descriptor_raw: PSECURITY_DESCRIPTOR = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide_sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor_raw,
                ptr::null_mut(),
            )
        } == 0
            || descriptor_raw.is_null()
        {
            let error = unsafe { GetLastError() };
            return Err(format!("无法构建更新日志权限预检描述符（Win32 {error}）。"));
        }
        let _descriptor = LocalMemory(descriptor_raw.cast());

        let mut effective_token_raw = ptr::null_mut();
        let effective_token = if unsafe {
            OpenThreadToken(
                GetCurrentThread(),
                TOKEN_QUERY | TOKEN_DUPLICATE,
                1,
                &mut effective_token_raw,
            )
        } != 0
        {
            if effective_token_raw.is_null() {
                return Err("线程令牌检查返回了空句柄。".into());
            }
            Handle(effective_token_raw)
        } else {
            let error = unsafe { GetLastError() };
            if error != ERROR_NO_TOKEN {
                return Err(format!("无法检查更新线程令牌（Win32 {error}）。"));
            }
            let mut process_token_raw = ptr::null_mut();
            if unsafe {
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_QUERY | TOKEN_DUPLICATE,
                    &mut process_token_raw,
                )
            } == 0
                || process_token_raw.is_null()
            {
                let error = unsafe { GetLastError() };
                return Err(format!("无法读取更新进程令牌（Win32 {error}）。"));
            }
            Handle(process_token_raw)
        };
        let mut impersonation_raw = ptr::null_mut();
        let level: SECURITY_IMPERSONATION_LEVEL = SecurityImpersonation;
        if unsafe { DuplicateToken(effective_token.0, level, &mut impersonation_raw) } == 0
            || impersonation_raw.is_null()
        {
            let error = unsafe { GetLastError() };
            return Err(format!("无法构建更新权限预检令牌（Win32 {error}）。"));
        }
        let impersonation_token = Handle(impersonation_raw);
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };

        let mut privileges_bytes = 512usize;
        loop {
            let mut privileges = vec![0u64; privileges_bytes.div_ceil(8)];
            let mut privileges_size = (privileges.len() * 8) as u32;
            let mut granted_access = 0u32;
            let mut access_status = 0;
            let succeeded = unsafe {
                AccessCheck(
                    descriptor_raw,
                    impersonation_token.0,
                    FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES,
                    &mapping,
                    privileges.as_mut_ptr().cast::<PRIVILEGE_SET>(),
                    &mut privileges_size,
                    &mut granted_access,
                    &mut access_status,
                )
            };
            if succeeded != 0 {
                return Ok(access_status != 0);
            }
            let error = unsafe { GetLastError() };
            if error != ERROR_INSUFFICIENT_BUFFER {
                return Err(format!("更新写入权限预检失败（Win32 {error}）。"));
            }
            privileges_bytes = (privileges_size as usize).max(privileges_bytes.saturating_mul(2));
            if privileges_bytes > 64 * 1024 {
                return Err("更新权限预检要求了异常大的特权缓冲区。".into());
            }
        }
    }

    struct WindowsJournalCommit<'a> {
        store: &'a VerifiedStore,
        temp_handle: Option<Handle>,
        final_handle: Option<Handle>,
    }

    impl JournalCommitIo for WindowsJournalCommit<'_> {
        type FileId = (u32, u64);

        fn preflight(&mut self) -> Result<(), String> {
            if current_process_can_write_journal_file()? {
                Ok(())
            } else {
                Err("当前进程没有满足更新日志 DACL 的写入权限；必须由固定提权 runner 执行。".into())
            }
        }

        fn create_temp(&mut self) -> Result<(), String> {
            if self.temp_handle.is_some() {
                return Err("更新日志暂存句柄状态无效。".into());
            }
            let temp_path = journal_path(self.store, JOURNAL_TEMP_FILE);
            let temp_wide: Vec<u16> = temp_path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let (_security_memory, security_attributes) = journal_file_security_attributes()?;
            let handle = unsafe {
                CreateFileW(
                    temp_wide.as_ptr(),
                    GENERIC_WRITE | READ_CONTROL | FILE_READ_ATTRIBUTES,
                    0,
                    &security_attributes,
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
                    ptr::null_mut(),
                )
            };
            if handle == INVALID_HANDLE_VALUE || handle.is_null() {
                let error = unsafe { GetLastError() };
                return Err(format!(
                    "无法以 CREATE_NEW 创建更新日志暂存文件（Win32 {error}）。"
                ));
            }
            self.temp_handle = Some(Handle(handle));
            Ok(())
        }

        fn inspect_temp(&mut self) -> Result<Self::FileId, String> {
            let handle = self
                .temp_handle
                .as_ref()
                .ok_or_else(|| "更新日志暂存句柄缺失。".to_string())?;
            regular_file_identity(handle)
        }

        fn validate_temp_security(&mut self) -> Result<(), String> {
            let handle = self
                .temp_handle
                .as_ref()
                .ok_or_else(|| "更新日志暂存句柄缺失。".to_string())?;
            if !has_expected_journal_file_security(handle)? {
                return Err("更新日志暂存文件的 DACL 不符合 OWNER RIGHTS 保护策略。".into());
            }
            Ok(())
        }

        fn write_temp(&mut self, bytes: &[u8]) -> Result<(), String> {
            let handle = self
                .temp_handle
                .as_ref()
                .ok_or_else(|| "更新日志暂存句柄缺失。".to_string())?;
            write_all(handle, bytes)
        }

        fn flush_temp(&mut self) -> Result<(), String> {
            let handle = self
                .temp_handle
                .as_ref()
                .ok_or_else(|| "更新日志暂存句柄缺失。".to_string())?;
            if unsafe { FlushFileBuffers(handle.0) } == 0 {
                let error = unsafe { GetLastError() };
                return Err(format!("刷新更新日志暂存文件失败（Win32 {error}）。"));
            }
            Ok(())
        }

        fn rename_temp(&mut self, replace_existing: bool) -> Result<(), String> {
            drop(self.temp_handle.take());
            let temp_path = journal_path(self.store, JOURNAL_TEMP_FILE);
            let final_path = journal_path(self.store, JOURNAL_FILE);
            let temp_wide: Vec<u16> = temp_path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let final_wide: Vec<u16> = final_path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let mut move_flags = MOVEFILE_WRITE_THROUGH;
            if replace_existing {
                move_flags |= MOVEFILE_REPLACE_EXISTING;
            }
            if unsafe { MoveFileExW(temp_wide.as_ptr(), final_wide.as_ptr(), move_flags) } == 0 {
                let error = unsafe { GetLastError() };
                return Err(format!("原子提交更新日志失败（Win32 {error}）。"));
            }
            Ok(())
        }

        fn verify_final(
            &mut self,
            expected: &UpdateTransactionJournal,
            expected_bytes: &[u8],
            staged_file_id: &Self::FileId,
        ) -> Result<(), String> {
            let handle = open_journal_file(
                self.store,
                JOURNAL_FILE,
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL | FILE_READ_ATTRIBUTES,
            )?;
            if regular_file_identity(&handle)? != *staged_file_id {
                return Err("提交后的更新日志文件身份与暂存文件不一致。".into());
            }
            let persisted_bytes = read_bounded_file(&handle)?;
            let persisted = UpdateTransactionJournal::decode(&persisted_bytes)?;
            if persisted != *expected || persisted_bytes != expected_bytes {
                return Err("提交后的更新日志内容与预期不一致。".into());
            }
            self.final_handle = Some(handle);
            Ok(())
        }

        fn flush_final(&mut self) -> Result<(), String> {
            let handle = self
                .final_handle
                .as_ref()
                .ok_or_else(|| "已提交更新日志句柄缺失。".to_string())?;
            if unsafe { FlushFileBuffers(handle.0) } == 0 {
                let error = unsafe { GetLastError() };
                return Err(format!("刷新已提交更新日志失败（Win32 {error}）。"));
            }
            drop(self.final_handle.take());
            Ok(())
        }

        fn verify_final_directory(&mut self) -> Result<(), String> {
            if !contains_exact_names(&list_state_entries(self.store)?, &[JOURNAL_FILE]) {
                return Err("提交后更新日志目录出现未知或临时条目。".into());
            }
            Ok(())
        }
    }

    fn write_journal_atomically(
        store: &VerifiedStore,
        journal: &UpdateTransactionJournal,
        replace_existing: bool,
    ) -> Result<(), String> {
        let mut io = WindowsJournalCommit {
            store,
            temp_handle: None,
            final_handle: None,
        };
        commit_journal(&mut io, journal, replace_existing)
    }

    fn write_all(handle: &Handle, bytes: &[u8]) -> Result<(), String> {
        let mut offset = 0usize;
        while offset < bytes.len() {
            let mut written = 0u32;
            let requested = (bytes.len() - offset) as u32;
            if unsafe {
                WriteFile(
                    handle.0,
                    bytes[offset..].as_ptr(),
                    requested,
                    &mut written,
                    ptr::null_mut(),
                )
            } == 0
            {
                let error = unsafe { GetLastError() };
                return Err(format!("写入更新日志失败（Win32 {error}）。"));
            }
            if written == 0 {
                return Err("更新日志写入出现零长度进展。".into());
            }
            offset += written as usize;
        }
        Ok(())
    }

    fn program_data_path() -> Result<PathBuf, String> {
        let mut raw = ptr::null_mut();
        let result =
            unsafe { SHGetKnownFolderPath(&FOLDERID_ProgramData, 0, ptr::null_mut(), &mut raw) };
        if result < 0 || raw.is_null() {
            return Err("无法解析系统 ProgramData 已知文件夹。".into());
        }
        let mut length = 0usize;
        while length < 32_768 && unsafe { *raw.add(length) } != 0 {
            length += 1;
        }
        if length == 32_768 {
            unsafe { CoTaskMemFree(raw.cast()) };
            return Err("ProgramData 已知文件夹路径无效。".into());
        }
        let wide = unsafe { std::slice::from_raw_parts(raw, length) };
        let path = PathBuf::from(std::ffi::OsString::from_wide(wide));
        unsafe { CoTaskMemFree(raw.cast()) };
        if !path.is_absolute() {
            return Err("ProgramData 已知文件夹路径不是绝对路径。".into());
        }
        let components: Vec<_> = path.components().collect();
        let drive_prefix = matches!(
            components.first(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
        );
        let root_component = matches!(components.get(1), Some(Component::RootDir));
        let program_data_leaf = matches!(
            components.get(2),
            Some(Component::Normal(name)) if name.to_string_lossy().eq_ignore_ascii_case("ProgramData")
        );
        if components.len() != 3 || !drive_prefix || !root_component || !program_data_leaf {
            return Err("ProgramData 必须是本机卷根目录下的直接 ProgramData 子目录。".into());
        }
        let volume_root = path
            .parent()
            .ok_or_else(|| "ProgramData 的卷根目录无效。".to_string())?;
        let root_wide: Vec<u16> = volume_root
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        if unsafe { GetDriveTypeW(root_wide.as_ptr()) } != DRIVE_FIXED {
            return Err("ProgramData 必须位于本机固定磁盘卷。".into());
        }
        Ok(path)
    }

    fn open_directory(path: &Path) -> Result<Option<Handle>, String> {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                READ_CONTROL | FILE_READ_ATTRIBUTES,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            let error = unsafe { GetLastError() };
            if error == ERROR_FILE_NOT_FOUND || error == ERROR_PATH_NOT_FOUND {
                return Ok(None);
            }
            return Err(format!("无法安全打开状态目录（Win32 {error}）。"));
        }

        let handle = Handle(handle);
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0 {
            return Err("无法检查状态目录的文件身份。".into());
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err("状态路径不是普通目录或包含重解析点。".into());
        }
        Ok(Some(handle))
    }

    fn has_expected_directory_security(handle: &Handle) -> Result<bool, String> {
        let oici = (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8;
        let allow = ACCESS_ALLOWED_ACE_TYPE as u8;
        has_expected_security(
            handle,
            &["S-1-5-18", "S-1-5-32-544", "S-1-5-11"],
            &[0, 1],
            &[
                (0, FILE_ALL_ACCESS, oici, allow),
                (1, FILE_ALL_ACCESS, oici, allow),
                (2, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE, oici, allow),
            ],
            false,
            "更新状态目录",
        )
    }

    fn has_expected_program_data_security(handle: &Handle) -> Result<bool, String> {
        let oici = (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8;
        let creator_owner_flags = oici | INHERIT_ONLY_ACE as u8;
        let users_create_rights =
            FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES;
        let allow = ACCESS_ALLOWED_ACE_TYPE as u8;
        has_expected_security(
            handle,
            &["S-1-5-18", "S-1-5-32-544", "S-1-3-0", "S-1-5-32-545"],
            &[0, 1],
            &[
                (2, GENERIC_ALL, creator_owner_flags, allow),
                (0, FILE_ALL_ACCESS, oici, allow),
                (1, FILE_ALL_ACCESS, oici, allow),
                (3, FILE_GENERIC_READ | FILE_GENERIC_EXECUTE, oici, allow),
                (3, users_create_rights, CONTAINER_INHERIT_ACE as u8, allow),
            ],
            false,
            "系统 ProgramData 目录",
        )
    }

    fn has_expected_journal_file_security(handle: &Handle) -> Result<bool, String> {
        has_expected_security(
            handle,
            &["S-1-3-4", "S-1-5-18", "S-1-5-32-544", "S-1-5-11"],
            &[],
            &[
                (0, WRITE_DAC, 0, ACCESS_DENIED_ACE_TYPE as u8),
                (1, FILE_ALL_ACCESS, 0, ACCESS_ALLOWED_ACE_TYPE as u8),
                (2, FILE_ALL_ACCESS, 0, ACCESS_ALLOWED_ACE_TYPE as u8),
                (3, FILE_GENERIC_READ, 0, ACCESS_ALLOWED_ACE_TYPE as u8),
            ],
            true,
            "更新事务日志文件",
        )
    }

    fn has_expected_security(
        handle: &Handle,
        sid_strings: &[&str],
        trusted_owner_sids: &[usize],
        expected_aces: &[(usize, u32, u8, u8)],
        require_ordered: bool,
        label: &str,
    ) -> Result<bool, String> {
        let mut owner: PSID = ptr::null_mut();
        let mut dacl: *mut ACL = ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                handle.0,
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
                unsafe { LocalFree(descriptor as HLOCAL) };
            }
            return Err(format!("无法读取{label}安全描述符（Win32 {status}）。"));
        }
        let _descriptor_memory = LocalMemory(descriptor.cast());
        if owner.is_null() || dacl.is_null() || expected_aces.is_empty() {
            return Ok(false);
        }

        let expected_sids: Vec<LocalMemory> = sid_strings
            .iter()
            .map(|sid| sid_from_string(sid))
            .collect::<Result<_, _>>()?;
        if trusted_owner_sids
            .iter()
            .any(|index| *index >= expected_sids.len())
            || expected_aces
                .iter()
                .any(|(index, _, _, _)| *index >= expected_sids.len())
        {
            return Ok(false);
        }
        let owner_is_trusted = if trusted_owner_sids.is_empty() {
            unsafe { IsValidSid(owner) != 0 }
        } else {
            trusted_owner_sids
                .iter()
                .any(|index| unsafe { EqualSid(owner, expected_sids[*index].0 as PSID) != 0 })
        };
        if !owner_is_trusted {
            return Ok(false);
        }

        let mut control = 0u16;
        let mut revision = 0u32;
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
            || control & SE_DACL_PROTECTED == 0
        {
            return Ok(false);
        }
        let mut dacl_present = 0;
        let mut dacl_defaulted = 0;
        let mut actual_dacl: *mut ACL = ptr::null_mut();
        if unsafe {
            GetSecurityDescriptorDacl(
                descriptor,
                &mut dacl_present,
                &mut actual_dacl,
                &mut dacl_defaulted,
            )
        } == 0
            || dacl_present == 0
            || dacl_defaulted != 0
            || actual_dacl.is_null()
        {
            return Ok(false);
        }
        let mut acl_info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe {
            GetAclInformation(
                actual_dacl,
                (&mut acl_info as *mut ACL_SIZE_INFORMATION).cast(),
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
        } == 0
            || acl_info.AceCount as usize != expected_aces.len()
        {
            return Ok(false);
        }

        let mut matched = vec![false; expected_aces.len()];
        for index in 0..expected_aces.len() as u32 {
            let mut ace_pointer: *mut c_void = ptr::null_mut();
            if unsafe { GetAce(actual_dacl, index, &mut ace_pointer) } == 0 || ace_pointer.is_null()
            {
                return Ok(false);
            }
            let header = unsafe { &*ace_pointer.cast::<ACE_HEADER>() };
            let ace_size = usize::from(header.AceSize);
            let sid_offset = std::mem::size_of::<ACE_HEADER>() + std::mem::size_of::<u32>();
            if ace_size < sid_offset + 8 {
                return Ok(false);
            }
            let ace = unsafe { &*ace_pointer.cast::<ACCESS_ALLOWED_ACE>() };
            let ace_sid = std::ptr::addr_of!(ace.SidStart) as PSID;
            let sub_authority_count = unsafe { *ace_sid.cast::<u8>().add(1) } as usize;
            let sid_length = 8 + 4 * sub_authority_count;
            if sid_length > ace_size - sid_offset
                || unsafe { IsValidSid(ace_sid) } == 0
                || unsafe { GetLengthSid(ace_sid) } as usize != sid_length
            {
                return Ok(false);
            }

            let matches_expected = |expected_index: usize| {
                let (sid_index, mask, flags, ace_type) = expected_aces[expected_index];
                header.AceType == ace_type
                    && header.AceFlags == flags
                    && ace.Mask == mask
                    && unsafe { EqualSid(ace_sid, expected_sids[sid_index].0 as PSID) } != 0
            };
            if require_ordered {
                if !matches_expected(index as usize) {
                    return Ok(false);
                }
                matched[index as usize] = true;
            } else {
                let Some(expected_index) = (0..expected_aces.len()).find(|expected_index| {
                    !matched[*expected_index] && matches_expected(*expected_index)
                }) else {
                    return Ok(false);
                };
                matched[expected_index] = true;
            }
        }
        Ok(matched.into_iter().all(|value| value))
    }

    fn sid_from_string(value: &str) -> Result<LocalMemory, String> {
        let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let mut sid: PSID = ptr::null_mut();
        if unsafe { ConvertStringSidToSidW(wide.as_ptr(), &mut sid) } == 0 || sid.is_null() {
            return Err("无法解析预期的 Windows 安全 SID。".into());
        }
        Ok(LocalMemory(sid.cast()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct DisposableDirectory(PathBuf);

        impl Drop for DisposableDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        #[test]
        fn journal_file_sddl_has_owner_rights_deny_ace() {
            let (_descriptor_memory, attributes) = journal_file_security_attributes().unwrap();
            let descriptor = attributes.lpSecurityDescriptor as PSECURITY_DESCRIPTOR;
            let mut control = 0u16;
            let mut revision = 0u32;
            assert_ne!(
                unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
                0
            );
            assert_ne!(control & SE_DACL_PROTECTED, 0);

            let mut present = 0;
            let mut dacl: *mut ACL = ptr::null_mut();
            let mut defaulted = 0;
            assert_ne!(
                unsafe {
                    GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted)
                },
                0
            );
            assert_ne!(present, 0);
            assert_eq!(defaulted, 0);
            assert!(!dacl.is_null());

            let mut info: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
            assert_ne!(
                unsafe {
                    GetAclInformation(
                        dacl,
                        (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                        std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                        AclSizeInformation,
                    )
                },
                0
            );
            assert_eq!(info.AceCount, 4);

            let mut ace_pointer: *mut c_void = ptr::null_mut();
            assert_ne!(unsafe { GetAce(dacl, 0, &mut ace_pointer) }, 0);
            assert!(!ace_pointer.is_null());
            let header = unsafe { &*ace_pointer.cast::<ACE_HEADER>() };
            assert_eq!(header.AceType, ACCESS_DENIED_ACE_TYPE as u8);
            assert_eq!(header.AceFlags, 0);
            let ace = unsafe { &*ace_pointer.cast::<ACCESS_ALLOWED_ACE>() };
            assert_eq!(ace.Mask, WRITE_DAC);
            let owner_rights = sid_from_string("S-1-3-4").unwrap();
            let ace_sid = std::ptr::addr_of!(ace.SidStart) as PSID;
            assert_ne!(unsafe { EqualSid(ace_sid, owner_rights.0 as PSID) }, 0);
        }

        #[test]
        fn restricted_current_owner_lacks_write_dac_but_retains_read() {
            use windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER;
            use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
            use windows_sys::Win32::Security::{
                AccessCheck, CheckTokenMembership, CreateRestrictedToken, DuplicateToken,
                GetTokenInformation, SecurityImpersonation, TokenUser, DISABLE_MAX_PRIVILEGE,
                GENERIC_MAPPING, PRIVILEGE_SET, SECURITY_IMPERSONATION_LEVEL, SID_AND_ATTRIBUTES,
                TOKEN_DUPLICATE, TOKEN_QUERY, TOKEN_USER,
            };
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES,
            };
            use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

            let mut process_token_raw = ptr::null_mut();
            assert_ne!(
                unsafe {
                    OpenProcessToken(
                        GetCurrentProcess(),
                        TOKEN_QUERY | TOKEN_DUPLICATE,
                        &mut process_token_raw,
                    )
                },
                0
            );
            let process_token = Handle(process_token_raw);

            let mut token_info_size = 0u32;
            unsafe {
                GetTokenInformation(
                    process_token.0,
                    TokenUser,
                    ptr::null_mut(),
                    0,
                    &mut token_info_size,
                );
            }
            assert!(token_info_size >= std::mem::size_of::<TOKEN_USER>() as u32);
            let mut token_info = vec![0u64; token_info_size.div_ceil(8) as usize];
            assert_ne!(
                unsafe {
                    GetTokenInformation(
                        process_token.0,
                        TokenUser,
                        token_info.as_mut_ptr().cast(),
                        token_info_size,
                        &mut token_info_size,
                    )
                },
                0
            );
            let token_user = unsafe { &*token_info.as_ptr().cast::<TOKEN_USER>() };
            let local_system_sid = sid_from_string("S-1-5-18").unwrap();
            if unsafe { EqualSid(token_user.User.Sid, local_system_sid.0 as PSID) } != 0 {
                eprintln!("Skipping OWNER RIGHTS access check under LocalSystem token.");
                return;
            }
            let mut owner_sid_raw = ptr::null_mut();
            assert_ne!(
                unsafe { ConvertSidToStringSidW(token_user.User.Sid, &mut owner_sid_raw) },
                0
            );
            let owner_sid_memory = LocalMemory(owner_sid_raw.cast());
            let mut owner_sid_len = 0usize;
            unsafe {
                while *owner_sid_raw.add(owner_sid_len) != 0 {
                    owner_sid_len += 1;
                }
            }
            let owner_sid = unsafe {
                String::from_utf16_lossy(std::slice::from_raw_parts(owner_sid_raw, owner_sid_len))
            };
            let sddl = format!("O:{owner_sid}G:{owner_sid}{JOURNAL_FILE_DACL}");
            let wide_sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
            let mut descriptor_raw: PSECURITY_DESCRIPTOR = ptr::null_mut();
            assert_ne!(
                unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        wide_sddl.as_ptr(),
                        SDDL_REVISION_1,
                        &mut descriptor_raw,
                        ptr::null_mut(),
                    )
                },
                0
            );
            drop(owner_sid_memory);
            let descriptor_memory = LocalMemory(descriptor_raw.cast());
            let descriptor = descriptor_memory.0.cast::<c_void>();

            let administrators_sid = sid_from_string("S-1-5-32-544").unwrap();
            let disabled_administrators = SID_AND_ATTRIBUTES {
                Sid: administrators_sid.0 as PSID,
                Attributes: 0,
            };
            let mut restricted_raw = ptr::null_mut();
            assert_ne!(
                unsafe {
                    CreateRestrictedToken(
                        process_token.0,
                        DISABLE_MAX_PRIVILEGE,
                        1,
                        &disabled_administrators,
                        0,
                        ptr::null(),
                        0,
                        ptr::null(),
                        &mut restricted_raw,
                    )
                },
                0
            );
            let restricted_token = Handle(restricted_raw);
            let mut impersonation_raw = ptr::null_mut();
            let impersonation_level: SECURITY_IMPERSONATION_LEVEL = SecurityImpersonation;
            assert_ne!(
                unsafe {
                    DuplicateToken(
                        restricted_token.0,
                        impersonation_level,
                        &mut impersonation_raw,
                    )
                },
                0
            );
            let impersonation_token = Handle(impersonation_raw);
            let authenticated_users_sid = sid_from_string("S-1-5-11").unwrap();
            let mut is_authenticated_user = 0;
            assert_ne!(
                unsafe {
                    CheckTokenMembership(
                        impersonation_token.0,
                        authenticated_users_sid.0 as PSID,
                        &mut is_authenticated_user,
                    )
                },
                0,
                "CheckTokenMembership failed with Win32 {}",
                unsafe { GetLastError() }
            );
            if is_authenticated_user == 0 {
                eprintln!(
                    "Skipping OWNER RIGHTS access check for a token without Authenticated Users."
                );
                return;
            }

            let mapping = GENERIC_MAPPING {
                GenericRead: FILE_GENERIC_READ,
                GenericWrite: FILE_GENERIC_WRITE,
                GenericExecute: FILE_GENERIC_EXECUTE,
                GenericAll: FILE_ALL_ACCESS,
            };
            let check_access = |requested: u32| {
                let mut privileges_bytes = 512usize;
                loop {
                    let mut privileges = vec![0u64; privileges_bytes.div_ceil(8)];
                    let mut privileges_size = (privileges.len() * 8) as u32;
                    let mut granted_access = 0u32;
                    let mut access_status = 0;
                    let succeeded = unsafe {
                        AccessCheck(
                            descriptor,
                            impersonation_token.0,
                            requested,
                            &mapping,
                            privileges.as_mut_ptr().cast::<PRIVILEGE_SET>(),
                            &mut privileges_size,
                            &mut granted_access,
                            &mut access_status,
                        )
                    };
                    if succeeded != 0 {
                        return (access_status != 0, granted_access);
                    }
                    let error = unsafe { GetLastError() };
                    if error != ERROR_INSUFFICIENT_BUFFER {
                        panic!("AccessCheck failed with Win32 {error}");
                    }
                    privileges_bytes =
                        (privileges_size as usize).max(privileges_bytes.saturating_mul(2));
                    assert!(
                        privileges_bytes <= 64 * 1024,
                        "AccessCheck requested an unreasonable privilege buffer size"
                    );
                }
            };

            let (can_change_dacl, granted_dacl) = check_access(WRITE_DAC);
            assert!(
                !can_change_dacl,
                "restricted owner unexpectedly got WRITE_DAC: {granted_dacl:#x}"
            );
            let (can_write_journal, granted_write) =
                check_access(FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES);
            assert!(
                !can_write_journal,
                "restricted owner unexpectedly got journal write rights: {granted_write:#x}"
            );
            let (can_read, granted_read) = check_access(FILE_GENERIC_READ);
            assert!(
                can_read,
                "restricted owner should retain read access: {granted_read:#x}"
            );
        }

        #[test]
        fn untrusted_optional_store_does_not_block_startup_or_delete_evidence() {
            use std::time::{SystemTime, UNIX_EPOCH};

            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let program_data = std::env::temp_dir().join(format!(
                "aerotune-untrusted-store-fixture-{}-{nonce:x}",
                std::process::id()
            ));
            let state_root = program_data.join(PROGRAM_DATA_SUBDIR);
            let state_dir = state_root.join(JOURNAL_SUBDIR);
            std::fs::create_dir_all(&state_dir).unwrap();
            let evidence = state_dir.join("untrusted-test-entry");
            std::fs::write(&evidence, b"keep").unwrap();
            let directory = DisposableDirectory(program_data.clone());

            assert!(
                ensure_optional_store_at(&program_data).is_ok(),
                "unprovisioned user-owned state must not block ordinary startup while the gate is off"
            );
            assert_eq!(std::fs::read(&evidence).unwrap(), b"keep");
            drop(directory);
        }

        #[test]
        fn atomic_writer_exercises_disposable_temp_directory_with_current_token() {
            use std::time::{SystemTime, UNIX_EPOCH};

            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "aerotune-journal-fixture-{}-{nonce:x}",
                std::process::id()
            ));
            std::fs::create_dir(&path).unwrap();
            let directory = DisposableDirectory(path);
            let open_fixture_directory = || {
                open_directory(&directory.0)
                    .unwrap()
                    .expect("disposable fixture directory must exist")
            };
            let store = VerifiedStore {
                _volume_root: open_fixture_directory(),
                _program_data: open_fixture_directory(),
                _state_root: open_fixture_directory(),
                state_dir: open_fixture_directory(),
                journal_dir: directory.0.clone(),
            };
            let prepared = UpdateTransactionJournal {
                schema_version: super::super::JOURNAL_SCHEMA_VERSION,
                transaction_id: "0123456789abcdef0123456789abcdef".into(),
                from_version: "1.5.9".into(),
                target_version: "1.5.10".into(),
                update_artifact_sha256: "a".repeat(64),
                rollback_artifact_sha256: "b".repeat(64),
                phase: super::super::TransactionPhase::Prepared,
            };
            let next = prepared
                .advance(super::super::TransactionPhase::InstallIntent)
                .unwrap();

            assert!(list_state_entries(&store).unwrap().is_empty());
            if let Err(error) = write_journal_atomically(&store, &prepared, false) {
                let entries = list_state_entries(&store).unwrap();
                if entries.is_empty() {
                    assert!(
                        error.contains("当前进程没有满足更新日志 DACL 的写入权限"),
                        "unexpected no-write preflight error: {error}"
                    );
                    eprintln!(
                        "Writer preflight refused the current token before creating any file."
                    );
                    return;
                }
                assert!(
                    error.contains("Win32 5"),
                    "unexpected writer error: {error}"
                );
                let evidence_state = if contains_exact_names(&entries, &[JOURNAL_FILE]) {
                    assert_eq!(read_journal_file(&store).unwrap(), prepared);
                    "canonical journal is readable and matches the requested state"
                } else if contains_exact_names(&entries, &[JOURNAL_TEMP_FILE]) {
                    let temp = open_journal_file(
                        &store,
                        JOURNAL_TEMP_FILE,
                        GENERIC_READ | READ_CONTROL | FILE_READ_ATTRIBUTES,
                    )
                    .unwrap();
                    assert!(regular_file_identity(&temp).is_ok());
                    "protected temporary evidence remains"
                } else {
                    assert!(
                        entries.is_empty(),
                        "unexpected leftover entries: {entries:?}"
                    );
                    "no file was committed"
                };
                eprintln!("Writer failed closed for the current token: {error}; {evidence_state}.");
                return;
            }
            assert_eq!(read_journal_file(&store).unwrap(), prepared);
            assert!(contains_exact_names(
                &list_state_entries(&store).unwrap(),
                &[JOURNAL_FILE]
            ));
            assert!(!directory.0.join(JOURNAL_TEMP_FILE).exists());

            write_journal_atomically(&store, &next, true).unwrap();
            assert_eq!(read_journal_file(&store).unwrap(), next);
            assert!(contains_exact_names(
                &list_state_entries(&store).unwrap(),
                &[JOURNAL_FILE]
            ));
            assert!(!directory.0.join(JOURNAL_TEMP_FILE).exists());
        }
    }
}

pub const JOURNAL_SCHEMA_VERSION: u32 = 1;
pub const MAX_JOURNAL_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionPhase {
    Prepared,
    InstallIntent,
    InstallerObserved,
    Verifying,
    RollbackRequired,
    RollbackIntent,
    RollbackObserved,
    Committed,
    RolledBack,
}

impl TransactionPhase {
    fn allows(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Prepared, Self::InstallIntent)
                | (Self::InstallIntent, Self::InstallerObserved)
                | (Self::InstallIntent, Self::RollbackRequired)
                | (Self::InstallerObserved, Self::Verifying)
                | (Self::InstallerObserved, Self::RollbackRequired)
                | (Self::Verifying, Self::Committed)
                | (Self::Verifying, Self::RollbackRequired)
                | (Self::RollbackRequired, Self::RollbackIntent)
                | (Self::RollbackIntent, Self::RollbackObserved)
                | (Self::RollbackObserved, Self::RolledBack)
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Committed | Self::RolledBack)
    }
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateTransactionJournal {
    schema_version: u32,
    /// Opaque lowercase hexadecimal transaction identifier; never interpreted as a path or PID.
    transaction_id: String,
    from_version: String,
    target_version: String,
    update_artifact_sha256: String,
    rollback_artifact_sha256: String,
    phase: TransactionPhase,
}

impl UpdateTransactionJournal {
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn test_fixture() -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            transaction_id: "0123456789abcdef0123456789abcdef".into(),
            from_version: "1.5.9".into(),
            target_version: "1.5.10".into(),
            update_artifact_sha256: "a".repeat(64),
            rollback_artifact_sha256: "b".repeat(64),
            phase: TransactionPhase::Prepared,
        }
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn phase(&self) -> TransactionPhase {
        self.phase
    }

    #[allow(dead_code)]
    pub(crate) fn from_verified_artifacts(
        transaction_id: String,
        from_version: String,
        update_artifact: &crate::updater::VerifiedUpdateArtifact,
        rollback_artifact: &crate::updater::VerifiedUpdateArtifact,
    ) -> Result<Self, String> {
        if rollback_artifact.target_version().to_string() != from_version {
            return Err("已验证回滚工件版本必须精确匹配当前已安装版本。".into());
        }
        let journal = Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            transaction_id,
            target_version: update_artifact.target_version().to_string(),
            update_artifact_sha256: update_artifact.sha256_hex(),
            rollback_artifact_sha256: rollback_artifact.sha256_hex(),
            from_version,
            phase: TransactionPhase::Prepared,
        };
        validate_prepared_journal(&journal)?;
        Ok(journal)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != JOURNAL_SCHEMA_VERSION {
            return Err("更新事务日志版本不受支持。".into());
        }
        if !is_hex(&self.transaction_id, 32) {
            return Err("更新事务 ID 格式无效。".into());
        }
        let from = semver::Version::parse(&self.from_version)
            .map_err(|_| "更新事务的当前版本无效。".to_string())?;
        let target = semver::Version::parse(&self.target_version)
            .map_err(|_| "更新事务的目标版本无效。".to_string())?;
        if target.cmp_precedence(&from) != std::cmp::Ordering::Greater {
            return Err("更新事务目标版本必须严格高于当前版本。".into());
        }
        if !is_hex(&self.update_artifact_sha256, 64) || !is_hex(&self.rollback_artifact_sha256, 64)
        {
            return Err("更新或回滚工件 SHA-256 格式无效。".into());
        }
        Ok(())
    }

    pub fn advance(&self, next: TransactionPhase) -> Result<Self, String> {
        if !self.phase.allows(next) {
            return Err("更新事务阶段转换不允许。".into());
        }
        let mut updated = self.clone();
        updated.phase = next;
        updated.validate()?;
        Ok(updated)
    }

    pub fn validate_successor(&self, next: &Self) -> Result<(), String> {
        self.validate()?;
        next.validate()?;
        let expected = self.advance(next.phase)?;
        if expected != *next {
            return Err("更新事务身份、版本或工件摘要不得在阶段转换时改变。".into());
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|_| "更新事务日志编码失败。".to_string())?;
        if bytes.len() > MAX_JOURNAL_BYTES {
            return Err("更新事务日志超过大小上限。".into());
        }
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_JOURNAL_BYTES {
            return Err("更新事务日志大小无效。".into());
        }
        let journal: Self = serde_json::from_slice(bytes)
            .map_err(|_| "更新事务日志格式无效或包含未知字段。".to_string())?;
        journal.validate()?;
        Ok(journal)
    }
}

fn validate_prepared_journal(journal: &UpdateTransactionJournal) -> Result<(), String> {
    journal.validate()?;
    if journal.phase != TransactionPhase::Prepared {
        return Err("新事务日志必须从 prepared 阶段开始。".into());
    }
    Ok(())
}

fn validate_journal_advance(
    expected: &UpdateTransactionJournal,
    next: &UpdateTransactionJournal,
    current: &UpdateTransactionJournal,
) -> Result<(), String> {
    if expected.phase.is_terminal() {
        return Err("终态更新事务不得继续推进。".into());
    }
    if current != expected {
        return Err("当前磁盘日志与预期事务状态不匹配。".into());
    }
    expected.validate_successor(next)
}

fn is_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> UpdateTransactionJournal {
        UpdateTransactionJournal {
            schema_version: JOURNAL_SCHEMA_VERSION,
            transaction_id: "0123456789abcdef0123456789abcdef".into(),
            from_version: "1.5.9".into(),
            target_version: "1.5.10".into(),
            update_artifact_sha256: "a".repeat(64),
            rollback_artifact_sha256: "b".repeat(64),
            phase: TransactionPhase::Prepared,
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum CommitStep {
        Preflight,
        CreateTemp,
        InspectTemp,
        ValidateTempSecurity,
        WriteTemp,
        FlushTemp,
        Rename,
        VerifyFinal,
        FlushFinal,
        VerifyDirectory,
    }

    struct FakeCommitIo {
        fail_at: Option<CommitStep>,
        steps: Vec<CommitStep>,
        temp_exists: bool,
        final_exists: bool,
        partial_write: bool,
        replace_existing: Option<bool>,
    }

    impl FakeCommitIo {
        fn new(fail_at: Option<CommitStep>) -> Self {
            Self {
                fail_at,
                steps: Vec::new(),
                temp_exists: false,
                final_exists: false,
                partial_write: false,
                replace_existing: None,
            }
        }

        fn record(&mut self, step: CommitStep) -> Result<(), String> {
            self.steps.push(step);
            if self.fail_at == Some(step) {
                Err(format!("injected {step:?} failure"))
            } else {
                Ok(())
            }
        }
    }

    impl JournalCommitIo for FakeCommitIo {
        type FileId = u8;

        fn preflight(&mut self) -> Result<(), String> {
            self.record(CommitStep::Preflight)
        }

        fn create_temp(&mut self) -> Result<(), String> {
            self.record(CommitStep::CreateTemp)?;
            self.temp_exists = true;
            Ok(())
        }

        fn inspect_temp(&mut self) -> Result<Self::FileId, String> {
            self.record(CommitStep::InspectTemp)?;
            Ok(7)
        }

        fn validate_temp_security(&mut self) -> Result<(), String> {
            self.record(CommitStep::ValidateTempSecurity)
        }

        fn write_temp(&mut self, _bytes: &[u8]) -> Result<(), String> {
            if self.fail_at == Some(CommitStep::WriteTemp) {
                self.steps.push(CommitStep::WriteTemp);
                self.partial_write = true;
                return Err("injected partial WriteFile failure".into());
            }
            self.record(CommitStep::WriteTemp)
        }

        fn flush_temp(&mut self) -> Result<(), String> {
            self.record(CommitStep::FlushTemp)
        }

        fn rename_temp(&mut self, replace_existing: bool) -> Result<(), String> {
            self.record(CommitStep::Rename)?;
            self.replace_existing = Some(replace_existing);
            self.temp_exists = false;
            self.final_exists = true;
            Ok(())
        }

        fn verify_final(
            &mut self,
            expected: &UpdateTransactionJournal,
            expected_bytes: &[u8],
            staged_file_id: &Self::FileId,
        ) -> Result<(), String> {
            self.record(CommitStep::VerifyFinal)?;
            assert_eq!(*staged_file_id, 7);
            assert_eq!(
                UpdateTransactionJournal::decode(expected_bytes).unwrap(),
                *expected
            );
            Ok(())
        }

        fn flush_final(&mut self) -> Result<(), String> {
            self.record(CommitStep::FlushFinal)
        }

        fn verify_final_directory(&mut self) -> Result<(), String> {
            self.record(CommitStep::VerifyDirectory)?;
            if self.temp_exists || !self.final_exists {
                return Err("unexpected final directory contents".into());
            }
            Ok(())
        }
    }

    #[test]
    fn atomic_writer_stops_at_each_injected_failure_and_preserves_evidence() {
        let ordered_steps = [
            CommitStep::Preflight,
            CommitStep::CreateTemp,
            CommitStep::InspectTemp,
            CommitStep::ValidateTempSecurity,
            CommitStep::WriteTemp,
            CommitStep::FlushTemp,
            CommitStep::Rename,
            CommitStep::VerifyFinal,
            CommitStep::FlushFinal,
            CommitStep::VerifyDirectory,
        ];
        for (failure_index, failure) in ordered_steps.iter().copied().enumerate() {
            let mut io = FakeCommitIo::new(Some(failure));
            assert!(commit_journal(&mut io, &fixture(), true).is_err());
            assert_eq!(io.steps, ordered_steps[..=failure_index].to_vec());
            assert_eq!(io.replace_existing, (failure_index >= 7).then_some(true));
            if failure_index <= 1 {
                assert!(!io.temp_exists && !io.final_exists);
            } else if failure_index <= 6 {
                assert!(io.temp_exists && !io.final_exists);
            } else {
                assert!(!io.temp_exists && io.final_exists);
            }
            assert_eq!(io.partial_write, failure == CommitStep::WriteTemp);
        }
    }

    #[test]
    fn atomic_writer_runs_all_steps_and_passes_replace_policy() {
        let mut io = FakeCommitIo::new(None);
        assert!(commit_journal(&mut io, &fixture(), true).is_ok());
        assert_eq!(
            io.steps,
            [
                CommitStep::Preflight,
                CommitStep::CreateTemp,
                CommitStep::InspectTemp,
                CommitStep::ValidateTempSecurity,
                CommitStep::WriteTemp,
                CommitStep::FlushTemp,
                CommitStep::Rename,
                CommitStep::VerifyFinal,
                CommitStep::FlushFinal,
                CommitStep::VerifyDirectory,
            ]
        );
        assert_eq!(io.replace_existing, Some(true));
        assert!(!io.temp_exists && io.final_exists);
    }

    #[test]
    fn initial_journal_commit_never_replaces_existing_record() {
        let mut io = FakeCommitIo::new(None);
        assert!(commit_journal(&mut io, &fixture(), false).is_ok());
        assert_eq!(io.replace_existing, Some(false));
    }

    #[test]
    fn invalid_journal_is_rejected_before_any_writer_io() {
        let mut journal = fixture();
        journal.target_version = "not-semver".into();
        let mut io = FakeCommitIo::new(None);
        assert!(commit_journal(&mut io, &journal, false).is_err());
        assert!(io.steps.is_empty());
        assert!(!io.temp_exists && !io.final_exists);
    }

    #[test]
    fn optional_store_is_skipped_only_when_missing() {
        let missing = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let denied = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert!(is_missing_optional_store(&missing));
        assert!(!is_missing_optional_store(&denied));
    }

    #[test]
    fn disabled_update_gate_tolerates_an_absent_store() {
        assert!(ensure_no_pending_journal(false).is_ok());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "read-only check for this host's unprovisioned ProgramData recovery directory"]
    fn enabled_update_gate_rejects_missing_programdata_store() {
        let error = ensure_no_pending_journal(true).unwrap_err();
        assert!(
            error.contains("更新恢复存储尚未由受信任安装器预置"),
            "{error}"
        );
    }

    #[test]
    fn startup_gate_rejects_any_state_entry_or_directory_read_error() {
        let empty: Vec<std::io::Result<()>> = Vec::new();
        assert!(reject_any_directory_entry(empty.into_iter()).is_ok());

        let populated = vec![Ok(())];
        assert!(reject_any_directory_entry(populated.into_iter()).is_err());

        let unreadable: Vec<std::io::Result<()>> = vec![Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "test",
        ))];
        assert!(reject_any_directory_entry(unreadable.into_iter()).is_err());
    }

    #[test]
    fn journal_round_trips_and_write_ahead_phases_are_monotonic() {
        let journal = fixture();
        let encoded = journal.encode().unwrap();
        assert_eq!(UpdateTransactionJournal::decode(&encoded).unwrap(), journal);

        let journal = journal.advance(TransactionPhase::InstallIntent).unwrap();
        let journal = journal
            .advance(TransactionPhase::InstallerObserved)
            .unwrap();
        let journal = journal.advance(TransactionPhase::Verifying).unwrap();
        let journal = journal.advance(TransactionPhase::Committed).unwrap();
        assert!(journal.phase.is_terminal());
        assert!(journal.advance(TransactionPhase::RollbackIntent).is_err());

        let rollback = fixture()
            .advance(TransactionPhase::InstallIntent)
            .unwrap()
            .advance(TransactionPhase::RollbackRequired)
            .unwrap()
            .advance(TransactionPhase::RollbackIntent)
            .unwrap()
            .advance(TransactionPhase::RollbackObserved)
            .unwrap()
            .advance(TransactionPhase::RolledBack)
            .unwrap();
        assert!(rollback.phase.is_terminal());
    }

    #[test]
    fn rollback_is_reachable_from_each_post_intent_phase_and_preserves_identity() {
        assert!(fixture()
            .advance(TransactionPhase::RollbackRequired)
            .is_err());
        let paths = [
            vec![TransactionPhase::InstallIntent],
            vec![
                TransactionPhase::InstallIntent,
                TransactionPhase::InstallerObserved,
            ],
            vec![
                TransactionPhase::InstallIntent,
                TransactionPhase::InstallerObserved,
                TransactionPhase::Verifying,
            ],
        ];

        for path in paths {
            let mut journal = fixture();
            for phase in path {
                journal = journal.advance(phase).unwrap();
            }
            assert!(journal.advance(TransactionPhase::RollbackRequired).is_ok());
            let rollback = journal
                .advance(TransactionPhase::RollbackRequired)
                .unwrap()
                .advance(TransactionPhase::RollbackIntent)
                .unwrap()
                .advance(TransactionPhase::RollbackObserved)
                .unwrap()
                .advance(TransactionPhase::RolledBack)
                .unwrap();

            assert_eq!(rollback.transaction_id, fixture().transaction_id);
            assert_eq!(rollback.from_version, fixture().from_version);
            assert_eq!(rollback.target_version, fixture().target_version);
            assert_eq!(
                rollback.update_artifact_sha256,
                fixture().update_artifact_sha256
            );
            assert_eq!(
                rollback.rollback_artifact_sha256,
                fixture().rollback_artifact_sha256
            );
            assert!(rollback.phase.is_terminal());
            assert!(rollback.advance(TransactionPhase::InstallIntent).is_err());
        }
    }

    #[test]
    fn writer_plan_requires_prepared_then_exact_nonterminal_successors() {
        let prepared = fixture();
        assert!(validate_prepared_journal(&prepared).is_ok());

        let install_intent = prepared.advance(TransactionPhase::InstallIntent).unwrap();
        assert!(validate_journal_advance(&prepared, &install_intent, &prepared).is_ok());

        let different_current = prepared.advance(TransactionPhase::InstallIntent).unwrap();
        assert!(validate_journal_advance(&prepared, &install_intent, &different_current).is_err());

        let mut not_prepared = prepared.clone();
        not_prepared.phase = TransactionPhase::InstallIntent;
        assert!(validate_prepared_journal(&not_prepared).is_err());

        let committed = prepared
            .advance(TransactionPhase::InstallIntent)
            .unwrap()
            .advance(TransactionPhase::InstallerObserved)
            .unwrap()
            .advance(TransactionPhase::Verifying)
            .unwrap()
            .advance(TransactionPhase::Committed)
            .unwrap();
        assert!(validate_journal_advance(&committed, &committed, &committed).is_err());
    }

    #[test]
    fn journal_successor_must_advance_one_phase_without_changing_identity() {
        let prepared = fixture();
        let install_intent = prepared.advance(TransactionPhase::InstallIntent).unwrap();
        assert!(prepared.validate_successor(&install_intent).is_ok());

        let mut tampered = install_intent.clone();
        tampered.rollback_artifact_sha256 = "c".repeat(64);
        assert!(prepared.validate_successor(&tampered).is_err());

        let mut skipped = prepared.clone();
        skipped.phase = TransactionPhase::InstallerObserved;
        assert!(prepared.validate_successor(&skipped).is_err());
    }

    #[test]
    fn journal_rejects_skipped_phases_and_invalid_versions_or_hashes() {
        let journal = fixture();
        assert!(journal.advance(TransactionPhase::Committed).is_err());

        let mut invalid_version = fixture();
        invalid_version.target_version = "1.5.8".into();
        assert!(invalid_version.validate().is_err());

        let mut invalid_hash = fixture();
        invalid_hash.rollback_artifact_sha256 = "../artifact.zip".into();
        assert!(invalid_hash.validate().is_err());
    }

    #[test]
    fn journal_rejects_build_metadata_only_version_progression() {
        let mut journal = fixture();
        journal.from_version = "1.5.9+previous".into();
        journal.target_version = "1.5.9+candidate".into();
        assert!(journal.validate().is_err());

        journal.target_version = "1.5.10+candidate".into();
        assert!(journal.validate().is_ok());
    }

    #[test]
    fn journal_decode_rejects_unknown_fields_and_unbounded_payloads() {
        let mut value = serde_json::to_value(fixture()).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("untrusted_path".into(), serde_json::json!("C:\\temp"));
        let encoded = serde_json::to_vec(&value).unwrap();
        assert!(UpdateTransactionJournal::decode(&encoded).is_err());
        assert!(UpdateTransactionJournal::decode(&vec![b'x'; MAX_JOURNAL_BYTES + 1]).is_err());
    }
}
