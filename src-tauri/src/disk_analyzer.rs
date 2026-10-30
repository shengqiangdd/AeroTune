// 磁盘空间分析器：全盘/目录扫描 → 深度二级目录聚合 + 大文件排行
// 支持进度事件推送与取消；删除走回收站（可从回收站还原）。
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, Metadata};
use std::io::{BufReader, Read};
#[cfg(test)]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::AppHandle;
use uuid::Uuid;
use walkdir::WalkDir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanKind {
    Disk,
    Duplicate,
}

#[derive(Default)]
struct ScanControl {
    active: Option<ScanKind>,
    cancel: bool,
}
static SCAN_CONTROL: OnceLock<Mutex<ScanControl>> = OnceLock::new();
fn scan_control() -> &'static Mutex<ScanControl> {
    SCAN_CONTROL.get_or_init(|| Mutex::new(ScanControl::default()))
}

fn try_start_scan(control: &Mutex<ScanControl>, kind: ScanKind) -> bool {
    let Ok(mut state) = control.lock() else {
        return false;
    };
    if state.active.is_some() {
        return false;
    }
    state.active = Some(kind);
    state.cancel = false;
    true
}

fn request_scan_cancel(control: &Mutex<ScanControl>, kind: ScanKind) -> bool {
    let Ok(mut state) = control.lock() else {
        return false;
    };
    if state.active != Some(kind) {
        return false;
    }
    state.cancel = true;
    true
}

fn scan_cancelled(control: &Mutex<ScanControl>, kind: ScanKind) -> bool {
    control
        .lock()
        .map(|state| state.active != Some(kind) || state.cancel)
        .unwrap_or(true)
}

struct ScanActiveGuard {
    kind: ScanKind,
}
impl Drop for ScanActiveGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = scan_control().lock() {
            if state.active == Some(self.kind) {
                state.active = None;
                state.cancel = false;
            }
        }
    }
}

fn begin_scan(kind: ScanKind) -> Result<ScanActiveGuard, String> {
    if try_start_scan(scan_control(), kind) {
        Ok(ScanActiveGuard { kind })
    } else {
        Err("已有磁盘分析任务正在运行".into())
    }
}

const MAX_DELETE_GRANTS: usize = 4;
const DELETE_GRANT_TTL: Duration = Duration::from_secs(30 * 60);
const MAX_RECYCLE_DELETE_BATCH: usize = 100;
const MAX_DUPLICATE_ROOT_SELECTIONS: usize = 4;
const DUPLICATE_ROOT_SELECTION_TTL: Duration = Duration::from_secs(30 * 60);
static RECYCLE_DELETE_LOCK: Mutex<()> = Mutex::new(());
static DELETE_GRANTS: OnceLock<Mutex<HashMap<String, DeleteGrant>>> = OnceLock::new();
static DUPLICATE_ROOT_SELECTIONS: OnceLock<Mutex<HashMap<String, DuplicateRootSelection>>> =
    OnceLock::new();

#[derive(Clone)]
struct DeleteFileGrant {
    bytes: u64,
    modified_unix_s: i64,
    duplicate_group: Option<String>,
}

struct DeleteGrant {
    created: Instant,
    files: HashMap<std::path::PathBuf, DeleteFileGrant>,
    claimed: HashSet<std::path::PathBuf>,
    duplicate_remaining: HashMap<String, usize>,
}

#[derive(Clone)]
struct DuplicateRootSelection {
    created: Instant,
    canonical_root: std::path::PathBuf,
    display_root: String,
}

fn duplicate_root_selections() -> &'static Mutex<HashMap<String, DuplicateRootSelection>> {
    DUPLICATE_ROOT_SELECTIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn new_opaque_token() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| "无法生成安全扫描标识".to_string())?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(Uuid::from_bytes(bytes).to_string())
}

fn register_duplicate_root_selection(path: &std::path::Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err("请选择绝对路径的扫描文件夹".into());
    }
    let metadata = fs::metadata(path).map_err(|_| "所选文件夹不存在或不可访问")?;
    if !metadata.is_dir() {
        return Err("请选择文件夹而非文件".into());
    }
    let canonical_root = fs::canonicalize(path).map_err(|_| "无法验证所选文件夹")?;
    if !fs::metadata(&canonical_root)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
    {
        return Err("所选文件夹不可用".into());
    }
    let selection = DuplicateRootSelection {
        created: Instant::now(),
        canonical_root,
        display_root: path.to_string_lossy().into_owned(),
    };
    let token = new_opaque_token()?;
    let mut selections = duplicate_root_selections()
        .lock()
        .map_err(|_| "扫描目录选择状态不可用".to_string())?;
    selections.retain(|_, existing| existing.created.elapsed() < DUPLICATE_ROOT_SELECTION_TTL);
    if selections.len() >= MAX_DUPLICATE_ROOT_SELECTIONS {
        if let Some(oldest) = selections
            .iter()
            .min_by_key(|(_, existing)| existing.created)
            .map(|(token, _)| token.clone())
        {
            selections.remove(&oldest);
        }
    }
    selections.insert(token.clone(), selection);
    Ok(token)
}

fn resolve_duplicate_root_selection(token: &str) -> Result<(std::path::PathBuf, String), String> {
    let selection = {
        let mut selections = duplicate_root_selections()
            .lock()
            .map_err(|_| "扫描目录选择状态不可用".to_string())?;
        selections.retain(|_, existing| existing.created.elapsed() < DUPLICATE_ROOT_SELECTION_TTL);
        selections
            .get(token)
            .cloned()
            .ok_or_else(|| "扫描目录选择已失效，请重新选择".to_string())?
    };
    let current = fs::canonicalize(&selection.canonical_root)
        .map_err(|_| "所选文件夹已不可用，请重新选择")?;
    if current != selection.canonical_root
        || !fs::metadata(&current)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
    {
        return Err("所选文件夹已变化，请重新选择".into());
    }
    Ok((selection.canonical_root, selection.display_root))
}

#[cfg(test)]
mod root_selection_tests {
    use super::*;

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aerotune-root-selection-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn duplicate_root_selection_is_canonical_opaque_and_reusable_only_by_token() {
        let dir = TestDir::new();
        let token = register_duplicate_root_selection(&dir.0).unwrap();
        assert!(Uuid::parse_str(&token).is_ok());

        let (root, display) = resolve_duplicate_root_selection(&token).unwrap();
        assert_eq!(root, fs::canonicalize(&dir.0).unwrap());
        assert_eq!(display, dir.0.to_string_lossy());
        assert_eq!(resolve_duplicate_root_selection(&token).unwrap().0, root);
        assert!(resolve_duplicate_root_selection(&display).is_err());

        let file = dir.0.join("not-a-folder.json");
        fs::write(&file, b"data").unwrap();
        assert!(register_duplicate_root_selection(&file).is_err());
    }

    #[test]
    fn duplicate_root_selection_expiry_and_disk_presets_fail_closed() {
        let dir = TestDir::new();
        let token = new_opaque_token().unwrap();
        let selection = DuplicateRootSelection {
            created: Instant::now() - DUPLICATE_ROOT_SELECTION_TTL - Duration::from_secs(1),
            canonical_root: fs::canonicalize(&dir.0).unwrap(),
            display_root: dir.0.to_string_lossy().into_owned(),
        };
        duplicate_root_selections()
            .lock()
            .unwrap()
            .insert(token.clone(), selection);
        assert!(resolve_duplicate_root_selection(&token).is_err());

        assert_eq!(
            serde_json::from_str::<DiskScanPreset>("\"user_profile\"").unwrap(),
            DiskScanPreset::UserProfile
        );
        assert_eq!(
            serde_json::from_str::<DiskScanPreset>("\"system_drive\"").unwrap(),
            DiskScanPreset::SystemDrive
        );
        assert!(serde_json::from_str::<DiskScanPreset>("\"C:\\\\Users\\\\Alice\"").is_err());
    }
}

fn delete_grants() -> &'static Mutex<HashMap<String, DeleteGrant>> {
    DELETE_GRANTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn canonical_current_file(path: &str) -> Result<(std::path::PathBuf, u64, i64), String> {
    let supplied = std::path::Path::new(path);
    if !supplied.is_absolute() || path.contains('\0') {
        return Err("删除目标必须是绝对文件路径".into());
    }
    let link_metadata = fs::symlink_metadata(supplied).map_err(|_| "文件不存在或不可访问")?;
    if !link_metadata.is_file() || link_metadata.file_type().is_symlink() {
        return Err("回收站操作只接受普通文件，不接受目录或链接".into());
    }
    let canonical = fs::canonicalize(supplied).map_err(|_| "无法验证删除目标路径")?;
    let metadata = fs::metadata(&canonical).map_err(|_| "删除目标已变化，请重新扫描")?;
    if !metadata.is_file() {
        return Err("回收站操作只接受普通文件".into());
    }
    let modified = modified_unix_s(metadata.modified().ok());
    Ok((canonical, metadata.len(), modified))
}

fn canonical_scanned_file(
    path: &str,
    bytes: u64,
    modified_unix_s: i64,
) -> Option<std::path::PathBuf> {
    let (canonical, current_bytes, current_modified) = canonical_current_file(path).ok()?;
    (current_bytes == bytes && current_modified == modified_unix_s).then_some(canonical)
}

fn register_delete_grant(mut grant: DeleteGrant) -> Result<String, String> {
    grant.created = Instant::now();
    let token = new_opaque_token()?;
    let mut grants = delete_grants()
        .lock()
        .map_err(|_| "扫描删除授权状态不可用".to_string())?;
    grants.retain(|_, existing| existing.created.elapsed() < DELETE_GRANT_TTL);
    if grants.len() >= MAX_DELETE_GRANTS {
        if let Some(oldest) = grants
            .iter()
            .min_by_key(|(_, existing)| existing.created)
            .map(|(token, _)| token.clone())
        {
            grants.remove(&oldest);
        }
    }
    grants.insert(token.clone(), grant);
    Ok(token)
}

fn disk_delete_grant(files: &[FileSize]) -> DeleteGrant {
    let mut grant = DeleteGrant {
        created: Instant::now(),
        files: HashMap::new(),
        claimed: HashSet::new(),
        duplicate_remaining: HashMap::new(),
    };
    for file in files {
        if let Some(path) = canonical_scanned_file(&file.path, file.bytes, file.modified) {
            grant.files.entry(path).or_insert(DeleteFileGrant {
                bytes: file.bytes,
                modified_unix_s: file.modified,
                duplicate_group: None,
            });
        }
    }
    grant
}

fn register_disk_delete_grant(files: &[FileSize]) -> Result<String, String> {
    register_delete_grant(disk_delete_grant(files))
}

fn duplicate_delete_grant(groups: &[DuplicateGroup]) -> DeleteGrant {
    let mut grant = DeleteGrant {
        created: Instant::now(),
        files: HashMap::new(),
        claimed: HashSet::new(),
        duplicate_remaining: HashMap::new(),
    };
    for group in groups {
        let mut visible_files = 0usize;
        for file in &group.files {
            if let Some(path) = canonical_scanned_file(&file.path, file.bytes, file.modified_unix_s)
            {
                if let std::collections::hash_map::Entry::Vacant(entry) = grant.files.entry(path) {
                    entry.insert(DeleteFileGrant {
                        bytes: file.bytes,
                        modified_unix_s: file.modified_unix_s,
                        duplicate_group: Some(group.id.clone()),
                    });
                    visible_files += 1;
                }
            }
        }
        if visible_files > 0 {
            // Preserve one currently visible file even for results truncated to 200 paths.
            grant
                .duplicate_remaining
                .insert(group.id.clone(), visible_files);
        }
    }
    grant
}

fn register_duplicate_delete_grant(groups: &[DuplicateGroup]) -> Result<String, String> {
    register_delete_grant(duplicate_delete_grant(groups))
}

fn claim_delete_request(
    grants: &mut HashMap<String, DeleteGrant>,
    token: &str,
    paths: &[(std::path::PathBuf, u64, i64)],
) -> Result<(), String> {
    grants.retain(|_, grant| grant.created.elapsed() < DELETE_GRANT_TTL);
    let grant = grants
        .get_mut(token)
        .ok_or_else(|| "扫描授权不存在或已过期，请重新扫描".to_string())?;
    reserve_recycle_paths(grant, paths)
}

fn reserve_recycle_paths(
    grant: &mut DeleteGrant,
    paths: &[(std::path::PathBuf, u64, i64)],
) -> Result<(), String> {
    if paths.is_empty() || paths.len() > MAX_RECYCLE_DELETE_BATCH {
        return Err("所选文件数量超出安全范围".into());
    }
    let mut unique = HashSet::with_capacity(paths.len());
    let mut group_counts: HashMap<String, usize> = HashMap::new();
    for (path, bytes, modified_unix_s) in paths {
        if !unique.insert(path) {
            return Err("删除请求包含重复路径".into());
        }
        let scoped = grant
            .files
            .get(path)
            .ok_or_else(|| "删除目标不属于此扫描结果，请重新扫描".to_string())?;
        if scoped.bytes != *bytes || scoped.modified_unix_s != *modified_unix_s {
            return Err("文件已在扫描后变化，请重新扫描".into());
        }
        if grant.claimed.contains(path) {
            return Err("此扫描结果中的文件已处理或请求过，请重新扫描".into());
        }
        if let Some(group) = &scoped.duplicate_group {
            *group_counts.entry(group.clone()).or_default() += 1;
        }
    }
    for (group, selected_count) in &group_counts {
        let remaining = grant
            .duplicate_remaining
            .get(group)
            .ok_or_else(|| "重复文件授权状态不可用，请重新扫描".to_string())?;
        if remaining <= selected_count {
            return Err("每组至少保留一个已核实副本".into());
        }
    }

    // Reserve the whole batch before invoking PowerShell. Reservations intentionally
    // remain consumed if the external recycle operation fails or partially succeeds.
    for (path, _, _) in paths {
        grant.claimed.insert(path.clone());
    }
    for (group, selected_count) in group_counts {
        if let Some(remaining) = grant.duplicate_remaining.get_mut(&group) {
            *remaining -= selected_count;
        }
    }
    Ok(())
}

#[tauri::command]
pub fn cancel_duplicate_scan() {
    let _ = request_scan_cancel(scan_control(), ScanKind::Duplicate);
}
fn scan_root(root: &str) -> Result<String, String> {
    let p = std::path::Path::new(root);
    if !p.is_absolute() || root.contains('\0') {
        return Err("扫描要求绝对路径（盘根须包含反斜杠）".into());
    }
    Ok(p.to_string_lossy().into_owned())
}

fn retain_largest(files: &mut Vec<FileSize>, candidate: FileSize, limit: usize) -> bool {
    if files.len() < limit {
        files.push(candidate);
        return false;
    }
    let Some((smallest, _)) = files.iter().enumerate().min_by_key(|(_, f)| f.bytes) else {
        return true;
    };
    if candidate.bytes > files[smallest].bytes {
        files[smallest] = candidate;
    }
    true
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn drive_root_remains_absolute() {
        assert_eq!(scan_root(r"C:\").unwrap(), r"C:\");
        assert!(scan_root("C:").is_err());
        assert_eq!(
            scan_root(r"C:\Users\fixture\").unwrap(),
            r"C:\Users\fixture\"
        );
    }

    #[test]
    fn concurrent_scans_are_rejected_until_active_guard_drops() {
        let control = Mutex::new(ScanControl::default());
        assert!(try_start_scan(&control, ScanKind::Disk));
        assert!(!try_start_scan(&control, ScanKind::Disk));
        assert!(!try_start_scan(&control, ScanKind::Duplicate));
        assert!(!request_scan_cancel(&control, ScanKind::Duplicate));
        assert!(request_scan_cancel(&control, ScanKind::Disk));
        assert!(scan_cancelled(&control, ScanKind::Disk));
        *control.lock().unwrap() = ScanControl::default();
        assert!(try_start_scan(&control, ScanKind::Duplicate));
    }

    #[test]
    fn shared_scan_guard_releases_after_drop() {
        let disk = begin_scan(ScanKind::Disk).unwrap();
        assert!(begin_scan(ScanKind::Duplicate).is_err());
        drop(disk);
        let duplicate = begin_scan(ScanKind::Duplicate).unwrap();
        drop(duplicate);
    }

    #[test]
    fn cancelled_scan_result_is_marked_explicitly() {
        let result = DiskScanResult {
            cancelled: true,
            ..Default::default()
        };
        assert!(result.cancelled);
        let serialized = serde_json::to_value(result).unwrap();
        assert_eq!(serialized["cancelled"], true);
    }

    #[test]
    fn large_file_retention_is_bounded_and_keeps_top_n() {
        let mut files = Vec::new();
        let mut truncated = false;
        for bytes in 1..=1000 {
            truncated |= retain_largest(
                &mut files,
                FileSize {
                    path: bytes.to_string(),
                    bytes,
                    modified: 0,
                },
                60,
            );
            assert!(files.len() <= 60);
        }
        assert!(truncated);
        assert_eq!(files.iter().map(|f| f.bytes).min(), Some(941));
        assert_eq!(files.iter().map(|f| f.bytes).max(), Some(1000));
        assert_eq!(files.len(), 60);
    }
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DirSize {
    pub path: String,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct FileSize {
    pub path: String,
    pub bytes: u64,
    pub modified: i64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DiskScanResult {
    pub root: String,
    pub total_files: u64,
    pub total_bytes: u64,
    pub cancelled: bool,
    pub truncated: bool,
    pub dirs: Vec<DirSize>,
    pub files: Vec<FileSize>,
    pub duration_ms: u64,
    pub delete_token: String,
}

#[derive(Serialize, Clone)]
pub struct ScanProgress {
    pub files: u64,
    pub bytes: u64,
    pub current: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiskScanPreset {
    UserProfile,
    SystemDrive,
}

fn resolve_disk_scan_preset(preset: DiskScanPreset) -> Result<String, String> {
    let root = match preset {
        DiskScanPreset::UserProfile => {
            std::env::var("USERPROFILE").map_err(|_| "无法定位用户目录".to_string())?
        }
        DiskScanPreset::SystemDrive => {
            let drive =
                std::env::var("SystemDrive").map_err(|_| "无法定位 Windows 系统盘".to_string())?;
            let drive = drive.trim().trim_end_matches(['\\', '/']);
            let bytes = drive.as_bytes();
            if bytes.len() != 2 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
                return Err("Windows 系统盘标识无效".into());
            }
            format!("{drive}\\")
        }
    };
    let root = scan_root(&root)?;
    if !std::path::Path::new(&root).is_dir() {
        return Err("扫描范围不存在或不可访问".into());
    }
    Ok(root)
}

#[tauri::command]
pub fn cancel_disk_scan() {
    let _ = request_scan_cancel(scan_control(), ScanKind::Disk);
}

#[tauri::command]
pub async fn scan_disk(preset: DiskScanPreset, app: AppHandle) -> Result<DiskScanResult, String> {
    let root = resolve_disk_scan_preset(preset)?;
    let active_guard = begin_scan(ScanKind::Disk)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _active_guard = active_guard;
        use tauri::Emitter;
        let start = Instant::now();
        // PathBuf 保留 C:\ 根，绝不转换成相对当前驱动目录的 C:。
        let root_clean = std::path::PathBuf::from(&root)
            .to_string_lossy()
            .into_owned();
        let mut total_files = 0u64;
        let mut total_bytes = 0u64;
        let mut dirs: HashMap<String, (u64, u64)> = HashMap::new();
        const LARGE_FILE_LIMIT: usize = 60;
        let mut files: Vec<FileSize> = Vec::with_capacity(LARGE_FILE_LIMIT + 1);
        let mut large_files_truncated = false;
        let mut last_emit = Instant::now();
        let mut cancelled = false;

        for (index, entry) in WalkDir::new(&root_clean)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .enumerate()
        {
            // Bound cancellation latency while avoiding a mutex operation per file.
            if index % 64 == 0 && scan_cancelled(scan_control(), ScanKind::Disk) {
                cancelled = true;
                break;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(md) = entry.metadata() else { continue };
            let size = md.len();
            total_files += 1;
            total_bytes += size;

            // 深度二级目录聚合：root\c1\c2（更深的文件计入其二级父目录）
            if let Ok(rel) = entry.path().strip_prefix(&root_clean) {
                let root_path = std::path::Path::new(&root_clean);
                let mut comps = rel.components();
                let c1 = comps.next();
                if let Some(c1) = c1 {
                    let key = match comps.next() {
                        Some(c2) if comps.next().is_some() => {
                            root_path.join(c1).join(c2).to_string_lossy().into_owned()
                        }
                        _ => root_path.join(c1).to_string_lossy().into_owned(),
                    };
                    let e = dirs.entry(key).or_insert((0, 0));
                    e.0 += size;
                    e.1 += 1;
                }
            }

            if size > 32 * 1024 * 1024 {
                let modified = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                let candidate = FileSize {
                    path: entry.path().to_string_lossy().into_owned(),
                    bytes: size,
                    modified,
                };
                // Retain only the largest 60 throughout the walk; cap memory use
                // even if a drive contains hundreds of thousands of large files.
                large_files_truncated |= retain_largest(&mut files, candidate, LARGE_FILE_LIMIT);
            }

            if last_emit.elapsed().as_millis() >= 400 {
                last_emit = Instant::now();
                let _ = app.emit(
                    "disk_scan_progress",
                    &ScanProgress {
                        files: total_files,
                        bytes: total_bytes,
                        current: entry.path().to_string_lossy().into_owned(),
                    },
                );
            }
        }

        let mut dirs: Vec<DirSize> = dirs
            .into_iter()
            .map(|(path, (bytes, n))| DirSize {
                path,
                bytes,
                files: n,
            })
            .collect();
        dirs.sort_by_key(|dir| std::cmp::Reverse(dir.bytes));
        dirs.truncate(80);
        files.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.path.cmp(&b.path)));
        let truncated = large_files_truncated;

        let mut result = DiskScanResult {
            root: root_clean,
            total_files,
            total_bytes,
            cancelled,
            truncated,
            dirs,
            files,
            duration_ms: start.elapsed().as_millis() as u64,
            delete_token: String::new(),
        };
        result.delete_token = register_disk_delete_grant(&result.files)?;
        let _ = app.emit(
            "disk_scan_progress",
            &ScanProgress {
                files: total_files,
                bytes: total_bytes,
                current: if cancelled {
                    "（已取消）".into()
                } else {
                    "完成".into()
                },
            },
        );
        tracing::info!(
            root = %result.root,
            files = result.total_files,
            mb = result.total_bytes / 1048576,
            ms = result.duration_ms,
            cancelled,
            "磁盘扫描完成"
        );
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

const MAX_DUPLICATE_SCAN_FILES: u64 = 100_000;
const MAX_DUPLICATE_SCAN_ENTRIES: usize = 500_000;
const MAX_DUPLICATE_GROUPS: usize = 500;
const MAX_DUPLICATE_FILES_PER_GROUP: usize = 200;
const HASH_BUFFER_SIZE: usize = 64 * 1024;
const HASH_PROGRESS_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Clone, Debug, Default)]
pub struct DuplicateFile {
    pub path: String,
    pub bytes: u64,
    pub modified_unix_s: i64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DuplicateGroup {
    pub id: String,
    pub bytes: u64,
    pub files_total: u64,
    pub files_truncated: bool,
    pub logical_duplicate_bytes: u64,
    pub files: Vec<DuplicateFile>,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DuplicateScanResult {
    pub root: String,
    pub files_scanned: u64,
    pub candidate_files: u64,
    pub bytes_hashed: u64,
    pub logical_duplicate_bytes: u64,
    pub skipped_unreadable: u64,
    pub changed_during_scan: u64,
    pub cancelled: bool,
    pub truncated: bool,
    pub duration_ms: u64,
    pub groups: Vec<DuplicateGroup>,
    pub delete_token: String,
    pub root_token: String,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct DuplicateScanProgress {
    pub phase: String,
    pub files_scanned: u64,
    pub candidate_files: u64,
    pub bytes_hashed: u64,
    pub current: String,
}

#[derive(Clone)]
struct DuplicateCandidate {
    file: DuplicateFile,
    modified: Option<SystemTime>,
}

fn modified_unix_s(value: Option<SystemTime>) -> i64 {
    value
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn metadata_matches_candidate(metadata: &Metadata, candidate: &DuplicateCandidate) -> bool {
    metadata.len() == candidate.file.bytes && metadata.modified().ok() == candidate.modified
}

fn emit_duplicate_progress<F>(
    result: &DuplicateScanResult,
    phase: &str,
    current: &str,
    last_emit: &mut Instant,
    emit: &mut F,
    force: bool,
) where
    F: FnMut(DuplicateScanProgress),
{
    if force || last_emit.elapsed() >= Duration::from_millis(400) {
        *last_emit = Instant::now();
        emit(DuplicateScanProgress {
            phase: phase.into(),
            files_scanned: result.files_scanned,
            candidate_files: result.candidate_files,
            bytes_hashed: result.bytes_hashed,
            current: current.into(),
        });
    }
}

enum HashOutcome {
    Hashed([u8; 32]),
    Unreadable,
    Changed,
    Cancelled,
}

fn hash_duplicate_candidate<F>(
    candidate: &DuplicateCandidate,
    control: &Mutex<ScanControl>,
    result: &mut DuplicateScanResult,
    last_emit: &mut Instant,
    emit: &mut F,
) -> HashOutcome
where
    F: FnMut(DuplicateScanProgress),
{
    let path = std::path::Path::new(&candidate.file.path);
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return HashOutcome::Unreadable,
    };
    let initial = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return HashOutcome::Unreadable,
    };
    if !metadata_matches_candidate(&initial, candidate) {
        return HashOutcome::Changed;
    }

    let mut reader = BufReader::with_capacity(HASH_BUFFER_SIZE, file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; HASH_BUFFER_SIZE];
    let mut chunks = 0usize;
    let mut last_progress_bucket = result.bytes_hashed / HASH_PROGRESS_BYTES;
    loop {
        if chunks.is_multiple_of(32) && scan_cancelled(control, ScanKind::Duplicate) {
            return HashOutcome::Cancelled;
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                result.bytes_hashed = result.bytes_hashed.saturating_add(read as u64);
                chunks += 1;
                let progress_bucket = result.bytes_hashed / HASH_PROGRESS_BYTES;
                let force_progress = progress_bucket > last_progress_bucket;
                if force_progress {
                    last_progress_bucket = progress_bucket;
                }
                emit_duplicate_progress(
                    result,
                    "hashing",
                    &candidate.file.path,
                    last_emit,
                    emit,
                    force_progress,
                );
            }
            Err(_) => return HashOutcome::Unreadable,
        }
    }

    let after_handle = match reader.get_ref().metadata() {
        Ok(metadata) => metadata,
        Err(_) => return HashOutcome::Unreadable,
    };
    let after_path = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return HashOutcome::Changed,
    };
    if !metadata_matches_candidate(&after_handle, candidate)
        || !metadata_matches_candidate(&after_path, candidate)
    {
        return HashOutcome::Changed;
    }
    HashOutcome::Hashed(hasher.finalize().into())
}

fn digest_hex(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn scan_duplicate_root<F>(
    root: &str,
    control: &Mutex<ScanControl>,
    emit: &mut F,
) -> DuplicateScanResult
where
    F: FnMut(DuplicateScanProgress),
{
    let start = Instant::now();
    let root_path = std::path::PathBuf::from(root);
    let root_clean = root_path.to_string_lossy().into_owned();
    let mut result = DuplicateScanResult {
        root: root_clean.clone(),
        ..Default::default()
    };
    let mut by_size: BTreeMap<u64, Vec<DuplicateCandidate>> = BTreeMap::new();
    let mut last_emit = Instant::now();

    for (index, entry_result) in WalkDir::new(&root_path)
        .follow_links(false)
        .into_iter()
        .enumerate()
    {
        if index >= MAX_DUPLICATE_SCAN_ENTRIES {
            result.truncated = true;
            break;
        }
        if index % 64 == 0 && scan_cancelled(control, ScanKind::Duplicate) {
            result.cancelled = true;
            break;
        }
        let entry = match entry_result {
            Ok(entry) => entry,
            Err(_) => {
                result.skipped_unreadable = result.skipped_unreadable.saturating_add(1);
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                result.skipped_unreadable = result.skipped_unreadable.saturating_add(1);
                continue;
            }
        };
        if result.files_scanned >= MAX_DUPLICATE_SCAN_FILES {
            result.truncated = true;
            break;
        }
        result.files_scanned = result.files_scanned.saturating_add(1);
        let bytes = metadata.len();
        // Empty files consume no payload space; omit them from duplicate reclamation results.
        if bytes > 0 {
            let modified = metadata.modified().ok();
            by_size.entry(bytes).or_default().push(DuplicateCandidate {
                file: DuplicateFile {
                    path: entry.path().to_string_lossy().into_owned(),
                    bytes,
                    modified_unix_s: modified_unix_s(modified),
                },
                modified,
            });
        }
        emit_duplicate_progress(
            &result,
            "enumerating",
            &entry.path().to_string_lossy(),
            &mut last_emit,
            emit,
            false,
        );
    }

    result.candidate_files = by_size
        .values()
        .filter(|entries| entries.len() > 1)
        .map(|entries| entries.len() as u64)
        .sum();
    let mut grouped: HashMap<(u64, [u8; 32]), Vec<DuplicateFile>> = HashMap::new();
    for (bytes, candidates) in by_size.iter().filter(|(_, entries)| entries.len() > 1) {
        for candidate in candidates {
            if scan_cancelled(control, ScanKind::Duplicate) {
                result.cancelled = true;
                break;
            }
            match hash_duplicate_candidate(candidate, control, &mut result, &mut last_emit, emit) {
                HashOutcome::Hashed(digest) => grouped
                    .entry((*bytes, digest))
                    .or_default()
                    .push(candidate.file.clone()),
                HashOutcome::Unreadable => {
                    result.skipped_unreadable = result.skipped_unreadable.saturating_add(1)
                }
                HashOutcome::Changed => {
                    result.changed_during_scan = result.changed_during_scan.saturating_add(1)
                }
                HashOutcome::Cancelled => {
                    result.cancelled = true;
                    break;
                }
            }
        }
        if result.cancelled {
            break;
        }
    }

    let mut groups: Vec<DuplicateGroup> = grouped
        .into_iter()
        .filter_map(|((bytes, digest), mut files)| {
            if files.len() < 2 {
                return None;
            }
            files.sort_by(|a, b| a.path.cmp(&b.path));
            let files_total = files.len() as u64;
            let logical_duplicate_bytes = bytes.saturating_mul(files_total.saturating_sub(1));
            result.logical_duplicate_bytes = result
                .logical_duplicate_bytes
                .saturating_add(logical_duplicate_bytes);
            let files_truncated = files.len() > MAX_DUPLICATE_FILES_PER_GROUP;
            files.truncate(MAX_DUPLICATE_FILES_PER_GROUP);
            Some(DuplicateGroup {
                id: format!("{bytes}:{}", digest_hex(&digest)),
                bytes,
                files_total,
                files_truncated,
                logical_duplicate_bytes,
                files,
            })
        })
        .collect();
    groups.sort_by(|a, b| {
        b.logical_duplicate_bytes
            .cmp(&a.logical_duplicate_bytes)
            .then_with(|| b.bytes.cmp(&a.bytes))
            .then_with(|| a.id.cmp(&b.id))
    });
    if groups.len() > MAX_DUPLICATE_GROUPS {
        groups.truncate(MAX_DUPLICATE_GROUPS);
        result.truncated = true;
    }
    result.groups = groups;
    result.duration_ms = start.elapsed().as_millis() as u64;
    emit_duplicate_progress(
        &result,
        if result.cancelled {
            "cancelled"
        } else {
            "complete"
        },
        if result.cancelled {
            "（已取消，保留已发现结果）"
        } else {
            "完成"
        },
        &mut last_emit,
        emit,
        true,
    );
    result
}

#[tauri::command]
pub async fn pick_duplicate_scan_root(app: AppHandle) -> Result<Option<String>, String> {
    let selected = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;

        app.dialog()
            .file()
            .set_title("选择重复文件扫描目录")
            .blocking_pick_folder()
            .map(|file| {
                file.into_path()
                    .map_err(|_| "所选文件夹路径不可用".to_string())
            })
            .transpose()
    })
    .await
    .map_err(|error| format!("文件夹选择器失败: {error}"))??;
    let Some(path) = selected else {
        return Ok(None);
    };
    register_duplicate_root_selection(&path).map(Some)
}

#[tauri::command]
pub async fn scan_duplicate_files(
    root_token: String,
    app: AppHandle,
) -> Result<DuplicateScanResult, String> {
    let (root, display_root) = resolve_duplicate_root_selection(&root_token)?;
    let root = root.to_string_lossy().into_owned();
    let active_guard = begin_scan(ScanKind::Duplicate)?;
    tauri::async_runtime::spawn_blocking(move || {
        let _active_guard = active_guard;
        use tauri::Emitter;
        let mut result = scan_duplicate_root(&root, scan_control(), &mut |progress| {
            let _ = app.emit("duplicate_scan_progress", progress);
        });
        result.root = display_root;
        result.root_token = root_token;
        result.delete_token = register_duplicate_delete_grant(&result.groups)?;
        Ok(result)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod duplicate_scan_tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aerotune-duplicate-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn active_duplicate_control(cancel: bool) -> Mutex<ScanControl> {
        Mutex::new(ScanControl {
            active: Some(ScanKind::Duplicate),
            cancel,
        })
    }

    #[test]
    fn exact_full_content_groups_duplicates_not_just_equal_sizes_or_edges() {
        let dir = TestDir::new();
        let size = 128 * 1024;
        let original = vec![b'x'; size];
        fs::write(dir.0.join("copy-a.bin"), &original).unwrap();
        fs::write(dir.0.join("copy-b.bin"), &original).unwrap();
        let mut middle_differs = original.clone();
        middle_differs[size / 2] = b'y';
        fs::write(dir.0.join("middle-differs.bin"), middle_differs).unwrap();
        fs::write(dir.0.join("different-size.bin"), vec![b'x'; size + 1]).unwrap();
        fs::write(dir.0.join("empty-a.bin"), []).unwrap();
        fs::write(dir.0.join("empty-b.bin"), []).unwrap();

        let control = active_duplicate_control(false);
        let result = scan_duplicate_root(&dir.0.to_string_lossy(), &control, &mut |_| {});
        assert_eq!(result.files_scanned, 6);
        assert_eq!(result.candidate_files, 3);
        assert_eq!(result.bytes_hashed, (size * 3) as u64);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files_total, 2);
        assert_eq!(result.groups[0].bytes, size as u64);
        assert_eq!(result.logical_duplicate_bytes, size as u64);
        assert!(result.groups[0].files[0].path.ends_with("copy-a.bin"));
        assert!(result.groups[0].files[1].path.ends_with("copy-b.bin"));
    }

    #[test]
    fn cancellation_during_large_file_hash_keeps_completed_duplicate_groups() {
        let dir = TestDir::new();
        fs::write(dir.0.join("small-a.bin"), b"small-copy").unwrap();
        fs::write(dir.0.join("small-b.bin"), b"small-copy").unwrap();
        let large = vec![0x5au8; (HASH_PROGRESS_BYTES * 2) as usize];
        fs::write(dir.0.join("large-a.bin"), &large).unwrap();
        fs::write(dir.0.join("large-b.bin"), &large).unwrap();

        let control = active_duplicate_control(false);
        let result = scan_duplicate_root(&dir.0.to_string_lossy(), &control, &mut |progress| {
            if progress.phase == "hashing" && progress.bytes_hashed >= HASH_PROGRESS_BYTES {
                let _ = request_scan_cancel(&control, ScanKind::Duplicate);
            }
        });
        assert!(result.cancelled);
        assert_eq!(result.groups.len(), 1);
        assert_eq!(result.groups[0].files_total, 2);
        assert!(result.bytes_hashed >= HASH_PROGRESS_BYTES);
        assert!(result.bytes_hashed < (HASH_PROGRESS_BYTES * 3));
    }

    #[test]
    fn cancellation_is_reported_and_keeps_the_scan_result_shape() {
        let dir = TestDir::new();
        fs::write(dir.0.join("candidate-a.bin"), b"content").unwrap();
        fs::write(dir.0.join("candidate-b.bin"), b"content").unwrap();
        let control = active_duplicate_control(true);
        let result = scan_duplicate_root(&dir.0.to_string_lossy(), &control, &mut |_| {});
        assert!(result.cancelled);
        assert!(result.groups.is_empty());
        assert_eq!(result.files_scanned, 0);
    }
}

/// 将当前扫描结果中的普通文件送入回收站；拒绝任意路径与目录。
#[tauri::command]
pub fn recycle_delete(delete_token: String, paths: Vec<String>) -> Result<(), String> {
    if paths.is_empty() || paths.len() > MAX_RECYCLE_DELETE_BATCH {
        return Err("所选文件数量超出安全范围".into());
    }
    let _delete_guard = RECYCLE_DELETE_LOCK
        .lock()
        .map_err(|_| "回收站操作状态不可用".to_string())?;
    let resolved = paths
        .iter()
        .map(|path| canonical_current_file(path))
        .collect::<Result<Vec<_>, _>>()?;
    {
        let mut grants = delete_grants()
            .lock()
            .map_err(|_| "扫描删除授权状态不可用".to_string())?;
        claim_delete_request(&mut grants, &delete_token, &resolved)?;
    }

    let items: Vec<String> = resolved
        .iter()
        .map(|(path, _, _)| path.to_string_lossy().replace('\'', "''"))
        .collect();
    let script = items
        .iter()
        .map(|path| {
            format!(
                "try {{ if (Test-Path -LiteralPath '{path}' -PathType Leaf) {{ [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{path}','OnlyErrorDialogs','SendToRecycleBin') }} else {{ throw 'File does not exist' }} }} catch {{ Write-Error $_ -ErrorAction Continue; $failed = $true }}"
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let script = format!("$ErrorActionPreference='Stop'; Add-Type -AssemblyName Microsoft.VisualBasic; $failed=$false; {script}; if ($failed) {{ exit 1 }}");
    // Claims remain consumed if PowerShell fails or partially moves the batch; callers must rescan.
    crate::util::run_checked(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )?;
    tracing::info!(count = items.len(), "已送入回收站");
    Ok(())
}

#[cfg(test)]
mod recycle_delete_auth_tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;

    struct TestFiles(std::path::PathBuf);

    impl TestFiles {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "aerotune-delete-auth-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }

        fn file(&self, name: &str, data: &[u8]) -> std::path::PathBuf {
            let path = self.0.join(name);
            fs::write(&path, data).unwrap();
            path
        }
    }

    impl Drop for TestFiles {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn file_size(path: &std::path::Path) -> FileSize {
        let (canonical, bytes, modified) = canonical_current_file(&path.to_string_lossy()).unwrap();
        FileSize {
            path: canonical.to_string_lossy().into_owned(),
            bytes,
            modified,
        }
    }

    fn duplicate_file(path: &std::path::Path) -> DuplicateFile {
        let (canonical, bytes, modified_unix_s) =
            canonical_current_file(&path.to_string_lossy()).unwrap();
        DuplicateFile {
            path: canonical.to_string_lossy().into_owned(),
            bytes,
            modified_unix_s,
        }
    }

    fn snapshot(path: &std::path::Path) -> (std::path::PathBuf, u64, i64) {
        canonical_current_file(&path.to_string_lossy()).unwrap()
    }

    #[test]
    fn disk_grant_contains_only_returned_files_and_rejects_mixed_batch_atomically() {
        let files = TestFiles::new();
        let listed = files.file("listed.bin", b"large file");
        let unlisted = files.file("unlisted.bin", b"other file");
        let mut grants = HashMap::new();
        grants.insert(
            "disk-token".to_string(),
            disk_delete_grant(&[file_size(&listed)]),
        );

        assert!(claim_delete_request(
            &mut grants,
            "disk-token",
            &[snapshot(&listed), snapshot(&unlisted)]
        )
        .is_err());
        assert!(grants["disk-token"].claimed.is_empty());
        assert!(claim_delete_request(&mut grants, "disk-token", &[snapshot(&listed)]).is_ok());
        assert!(claim_delete_request(&mut grants, "disk-token", &[snapshot(&listed)]).is_err());
    }

    #[test]
    fn duplicate_grant_preserves_one_visible_copy_across_batches() {
        let files = TestFiles::new();
        let first = files.file("copy-a.bin", b"identical");
        let second = files.file("copy-b.bin", b"identical");
        let unreturned = files.file("not-in-result.bin", b"identical");
        let group = DuplicateGroup {
            id: "group-a".into(),
            bytes: 9,
            files_total: 3,
            files_truncated: true,
            files: vec![duplicate_file(&first), duplicate_file(&second)],
            ..Default::default()
        };
        let mut grants = HashMap::new();
        grants.insert(
            "duplicate-token".to_string(),
            duplicate_delete_grant(&[group]),
        );

        assert!(
            claim_delete_request(&mut grants, "duplicate-token", &[snapshot(&unreturned)]).is_err()
        );
        assert!(grants["duplicate-token"].claimed.is_empty());
        assert!(claim_delete_request(&mut grants, "duplicate-token", &[snapshot(&first)]).is_ok());
        assert!(
            claim_delete_request(&mut grants, "duplicate-token", &[snapshot(&second)]).is_err()
        );
        let grant = &grants["duplicate-token"];
        assert_eq!(grant.duplicate_remaining.get("group-a"), Some(&1));
        assert!(!grant.claimed.contains(&snapshot(&second).0));
    }

    #[test]
    fn missing_expired_stale_and_duplicate_requests_fail_closed() {
        let files = TestFiles::new();
        let path = files.file("stale.bin", b"before");
        let scanned = file_size(&path);
        let mut grants = HashMap::new();
        assert!(claim_delete_request(&mut grants, "missing", &[]).is_err());

        let mut expired = disk_delete_grant(std::slice::from_ref(&scanned));
        expired.created = Instant::now() - DELETE_GRANT_TTL - Duration::from_secs(1);
        grants.insert("expired".to_string(), expired);
        assert!(claim_delete_request(&mut grants, "expired", &[snapshot(&path)]).is_err());
        assert!(!grants.contains_key("expired"));

        grants.insert("stale".to_string(), disk_delete_grant(&[scanned]));
        fs::write(&path, b"changed size after scan").unwrap();
        assert!(claim_delete_request(&mut grants, "stale", &[snapshot(&path)]).is_err());
        assert!(grants["stale"].claimed.is_empty());

        let current = file_size(&path);
        grants.insert("duplicate".to_string(), disk_delete_grant(&[current]));
        let path_snapshot = snapshot(&path);
        assert!(claim_delete_request(
            &mut grants,
            "duplicate",
            &[path_snapshot.clone(), path_snapshot]
        )
        .is_err());
        assert!(grants["duplicate"].claimed.is_empty());
    }

    #[test]
    fn failed_external_recycle_does_not_restore_a_claim() {
        let files = TestFiles::new();
        let path = files.file("claimed.bin", b"content");
        let mut grants =
            HashMap::from([("token".to_string(), disk_delete_grant(&[file_size(&path)]))]);
        let request = snapshot(&path);
        assert!(claim_delete_request(&mut grants, "token", std::slice::from_ref(&request)).is_ok());

        let simulated_executor: Result<(), &str> = Err("PowerShell may have partially succeeded");
        assert!(simulated_executor.is_err());
        assert!(claim_delete_request(&mut grants, "token", &[request]).is_err());
    }

    #[test]
    fn concurrent_requests_cannot_replay_the_same_scanned_path() {
        let files = TestFiles::new();
        let path = files.file("one.bin", b"one");
        let grant = disk_delete_grant(&[file_size(&path)]);
        let grants = Arc::new(Mutex::new(HashMap::from([("token".to_string(), grant)])));
        let barrier = Arc::new(Barrier::new(3));
        let request = snapshot(&path);
        let mut workers = Vec::new();
        for _ in 0..2 {
            let grants = Arc::clone(&grants);
            let barrier = Arc::clone(&barrier);
            let request = request.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                claim_delete_request(&mut grants.lock().unwrap(), "token", &[request]).is_ok()
            }));
        }
        barrier.wait();
        let successes = workers
            .into_iter()
            .map(|worker| usize::from(worker.join().unwrap()))
            .sum::<usize>();
        assert_eq!(successes, 1);
    }
}

// ---------- SMART 磁盘健康 ----------

#[derive(Serialize, Clone, Debug, Default)]
pub struct DiskHealth {
    pub name: String,
    pub media: String,
    pub health: String,
    pub size_gb: f64,
    pub temp: Option<f64>,
    /// 磨损百分比（SSD），HDD 无意义
    pub wear: Option<f64>,
    /// 通电小时数
    pub power_on_hours: Option<f64>,
    /// 未纠正的读取错误计数；驱动不支持时为 None
    pub read_errors: Option<f64>,
    /// 未纠正的写入错误计数；驱动不支持时为 None
    pub write_errors: Option<f64>,
}

fn optional_nonnegative_number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|number| number.is_finite() && *number >= 0.0)
}

fn parse_disk_health_json(raw: &str) -> Result<Vec<DiskHealth>, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let rows: Vec<serde_json::Value> = match value {
        serde_json::Value::Array(rows) => rows,
        object @ serde_json::Value::Object(_) => vec![object],
        _ => Vec::new(),
    };
    Ok(rows
        .into_iter()
        .map(|row| {
            let wear = optional_nonnegative_number(&row["wear"]).filter(|value| *value <= 100.0);
            DiskHealth {
                name: row["name"].as_str().unwrap_or("").into(),
                media: row["media"].as_str().unwrap_or("").into(),
                health: row["health"].as_str().unwrap_or("").into(),
                size_gb: optional_nonnegative_number(&row["size"]).unwrap_or(0.0),
                temp: optional_nonnegative_number(&row["temp"]),
                wear,
                power_on_hours: optional_nonnegative_number(&row["hours"]),
                read_errors: optional_nonnegative_number(&row["rerr"]),
                write_errors: optional_nonnegative_number(&row["werr"]),
            }
        })
        .collect())
}

const DISK_HEALTH_SCRIPT: &str = r#"
$disks = Get-PhysicalDisk -ErrorAction SilentlyContinue
$out = foreach ($d in $disks) {
  $rc = $d | Get-StorageReliabilityCounter -ErrorAction SilentlyContinue | Select-Object -First 1
  [pscustomobject]@{
    name   = [string]$d.FriendlyName
    media  = [string]$d.MediaType
    health = [string]$d.HealthStatus
    size   = [math]::Round($d.Size / 1GB, 0)
    temp   = $rc.Temperature
    wear   = $rc.Wear
    hours  = $rc.PowerOnHours
    rerr   = $rc.ReadErrorsUncorrected
    werr   = $rc.WriteErrorsUncorrected
  }
}
if ($out) { $out | ConvertTo-Json -Compress } else { '[]' }
"#;

/// 物理磁盘健康（SMART 汇总），温度与可靠性计数取决于磁盘驱动支持情况。
#[tauri::command]
pub async fn disk_health() -> Vec<DiskHealth> {
    tauri::async_runtime::spawn_blocking(|| {
        let Some(raw) = crate::util::run_ps(DISK_HEALTH_SCRIPT) else {
            return Vec::new();
        };
        parse_disk_health_json(&raw).unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
mod smart_health_tests {
    use super::*;

    #[test]
    fn smart_parser_preserves_zero_and_missing_fields() {
        let rows = parse_disk_health_json(
            r#"[
                {"name":"SSD","media":"SSD","health":"Healthy","size":512,"temp":39,"wear":12,"hours":0,"rerr":0,"werr":2},
                {"name":"USB","media":"Unspecified","health":"Healthy","size":64,"temp":null,"wear":null,"hours":null,"rerr":null,"werr":null}
            ]"#,
        )
        .unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].wear, Some(12.0));
        assert_eq!(rows[0].power_on_hours, Some(0.0));
        assert_eq!(rows[0].read_errors, Some(0.0));
        assert_eq!(rows[0].write_errors, Some(2.0));
        assert_eq!(rows[1].temp, None);
        assert_eq!(rows[1].wear, None);
        assert_eq!(rows[1].power_on_hours, None);
        assert_eq!(rows[1].read_errors, None);
        assert_eq!(rows[1].write_errors, None);
    }

    #[test]
    fn smart_parser_accepts_single_object_and_filters_invalid_counters() {
        let row = parse_disk_health_json(
            r#"{"name":"Fixture","wear":101,"hours":-1,"rerr":-3,"werr":4}"#,
        )
        .unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].wear, None);
        assert_eq!(row[0].power_on_hours, None);
        assert_eq!(row[0].read_errors, None);
        assert_eq!(row[0].write_errors, Some(4.0));
    }

    #[test]
    fn smart_query_projects_reliability_properties() {
        for property in [
            "Get-StorageReliabilityCounter",
            "$rc.Wear",
            "$rc.PowerOnHours",
            "$rc.ReadErrorsUncorrected",
            "$rc.WriteErrorsUncorrected",
        ] {
            assert!(DISK_HEALTH_SCRIPT.contains(property), "missing {property}");
        }
    }

    #[test]
    fn smart_parser_handles_empty_and_malformed_responses() {
        assert!(parse_disk_health_json("[]").unwrap().is_empty());
        assert!(parse_disk_health_json("not-json").is_err());
    }
}
