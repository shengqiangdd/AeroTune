use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use walkdir::WalkDir;

#[derive(Serialize, Clone)]
pub struct JunkItem {
    pub id: String,
    pub label: String,
    pub path: String,
    pub size_mb: f64,
    pub files: u64,
    pub admin_required: bool,
    /// 置信度：基于文件年龄中位数的高/中/低（越旧越适合清理）
    pub confidence: String,
    pub median_age_days: f64,
}

fn dir_size(path: &Path) -> (u64, u64) {
    let mut size = 0u64;
    let mut files = 0u64;
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file() {
            if let Ok(md) = entry.metadata() {
                size += md.len();
                files += 1;
            }
        }
    }
    (size, files)
}

/// 采样最多 600 个文件的修改时间，返回年龄中位数（天）。
fn median_age_days(path: &Path) -> f64 {
    let mut ages: Vec<f64> = Vec::new();
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if ages.len() >= 600 {
            break;
        }
        if entry.file_type().is_file() {
            if let Ok(md) = entry.metadata() {
                if let Ok(mtime) = md.modified() {
                    let age = std::time::SystemTime::now()
                        .duration_since(mtime)
                        .map(|d| d.as_secs() as f64 / 86400.0)
                        .unwrap_or(0.0);
                    ages.push(age);
                }
            }
        }
    }
    if ages.is_empty() {
        return 0.0;
    }
    ages.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ages[ages.len() / 2]
}

fn user_temp() -> Option<PathBuf> {
    std::env::var("TEMP").ok().map(PathBuf::from)
}

fn local_appdata(sub: &str) -> Option<PathBuf> {
    std::env::var("LOCALAPPDATA")
        .ok()
        .map(|l| PathBuf::from(l).join(sub))
}

/// 浏览器全部 Profile 的缓存聚合（含多开配置文件）
fn browser_cache_aggregate(user_data: PathBuf) -> Option<(u64, u64)> {
    let rd = std::fs::read_dir(&user_data).ok()?;
    let mut size = 0u64;
    let mut files = 0u64;
    let mut found = false;
    for e in rd.flatten() {
        let cache = e.path().join("Cache");
        if cache.exists() {
            found = true;
            let (s, f) = dir_size(&cache);
            size += s;
            files += f;
        }
    }
    found.then_some((size, files))
}

fn cleanup_targets(id: &str, root: &Path) -> Vec<PathBuf> {
    match id {
        "user_temp" | "win_temp" | "update_cache" => vec![root.to_path_buf()],
        "edge_cache" | "chrome_cache" => std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path().join("Cache"))
            .filter(|p| p.is_dir())
            .collect(),
        _ => Vec::new(),
    }
}

const CLEANUP_PLAN_TTL: Duration = Duration::from_secs(5 * 60);
const CLEANUP_PLAN_LIMIT: usize = 8;
const CLEANUP_CANDIDATE_LIMIT: usize = 100_000;
const CLEANUP_WALK_LIMIT: usize = 500_000;
const CLEANUP_SAMPLE_LIMIT: usize = 12;

#[derive(Serialize, Clone)]
pub struct JunkPreviewCategory {
    pub id: String,
    pub label: String,
    pub candidate_files: u64,
    pub candidate_bytes: u64,
    pub sample_paths: Vec<String>,
    pub age_filtered_files: u64,
    pub skipped_files: u64,
    pub permanent_action: bool,
    pub exact: bool,
}

#[derive(Serialize, Clone)]
pub struct JunkCleanupPreview {
    pub plan_id: Option<String>,
    pub older_than_days: u32,
    pub categories: Vec<JunkPreviewCategory>,
    pub can_execute: bool,
    pub over_limit: bool,
}

#[derive(Serialize, Clone, Default)]
pub struct JunkCleanupResult {
    pub cancelled: bool,
    pub deleted_files: u64,
    pub deleted_bytes: u64,
    pub skipped_changed: u64,
    pub skipped_unsafe: u64,
    pub failed: u64,
    pub errors: Vec<String>,
}

struct PlannedFile {
    root: PathBuf,
    path: PathBuf,
    size: u64,
    modified: SystemTime,
    older_than_days: u32,
}

struct CleanupPlan {
    created_at: Instant,
    files: Vec<PlannedFile>,
    file_labels: Vec<String>,
    recycle_selected: bool,
}

static NEXT_CLEANUP_PLAN: AtomicU64 = AtomicU64::new(1);
static CLEANUP_PLANS: OnceLock<Mutex<HashMap<String, CleanupPlan>>> = OnceLock::new();

fn cleanup_plan_store() -> &'static Mutex<HashMap<String, CleanupPlan>> {
    CLEANUP_PLANS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn metadata_is_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn is_reparse_path(path: &Path) -> bool {
    std::fs::symlink_metadata(path).map_or(true, |metadata| metadata_is_reparse(&metadata))
}

#[cfg(windows)]
fn residual_file_link_count(path: &Path) -> Result<u32, String> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
    };

    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .map_err(|_| format!("无法安全读取文件链接信息：{}", path.display()))?;
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
        return Err(format!("无法安全读取文件链接信息：{}", path.display()));
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err("残留树在检查期间出现重解析点；拒绝继续".into());
    }
    if information.nNumberOfLinks == 0 {
        return Err("无法确认残留文件的硬链接数；拒绝继续".into());
    }
    Ok(information.nNumberOfLinks)
}

#[cfg(not(windows))]
fn residual_file_link_count(_path: &Path) -> Result<u32, String> {
    Ok(1)
}

fn cleanup_root(id: &str) -> Option<PathBuf> {
    match id {
        "user_temp" => user_temp(),
        "win_temp" => Some(win_dir("Temp")),
        "update_cache" => Some(win_dir(r"SoftwareDistribution\Download")),
        "edge_cache" => local_appdata(r"Microsoft\Edge\User Data"),
        "chrome_cache" => local_appdata(r"Google\Chrome\User Data"),
        _ => None,
    }
}

fn cleanup_label(id: &str) -> &'static str {
    match id {
        "user_temp" => "用户临时文件",
        "win_temp" => "系统临时文件（Windows\\Temp）",
        "update_cache" => "Windows 更新缓存",
        "edge_cache" => "Edge 浏览器缓存",
        "chrome_cache" => "Chrome 浏览器缓存",
        "recycle" => "回收站",
        _ => "未知清理项",
    }
}

fn is_old_enough(modified: SystemTime, now: SystemTime, days: u32) -> bool {
    if days == 0 {
        return true;
    }
    now.duration_since(modified)
        .map(|age| age.as_secs() >= days as u64 * 86_400)
        .unwrap_or(false)
}

fn canonical_child(root: &Path, candidate: &Path) -> Option<(PathBuf, PathBuf)> {
    if is_reparse_path(root) || is_reparse_path(candidate) {
        return None;
    }
    let root_canonical = root.canonicalize().ok()?;
    let candidate_canonical = candidate.canonicalize().ok()?;
    if !candidate_canonical.starts_with(&root_canonical) {
        return None;
    }
    Some((root_canonical, candidate_canonical))
}

fn add_preview_plan(
    files: Vec<PlannedFile>,
    recycle_selected: bool,
    file_labels: Vec<String>,
) -> Result<String, String> {
    let sequence = NEXT_CLEANUP_PLAN.fetch_add(1, Ordering::Relaxed);
    let plan_id = format!(
        "{:x}-{:x}",
        SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        sequence
    );
    let mut plans = cleanup_plan_store()
        .lock()
        .map_err(|_| "清理预览状态不可用".to_string())?;
    plans.retain(|_, plan| plan.created_at.elapsed() <= CLEANUP_PLAN_TTL);
    while plans.len() >= CLEANUP_PLAN_LIMIT {
        let oldest = plans
            .iter()
            .min_by_key(|(_, plan)| plan.created_at)
            .map(|(id, _)| id.clone());
        if let Some(oldest) = oldest {
            plans.remove(&oldest);
        } else {
            break;
        }
    }
    plans.insert(
        plan_id.clone(),
        CleanupPlan {
            created_at: Instant::now(),
            files,
            file_labels,
            recycle_selected,
        },
    );
    Ok(plan_id)
}

fn take_cleanup_plan(plan_id: &str) -> Result<CleanupPlan, String> {
    let plan = cleanup_plan_store()
        .lock()
        .map_err(|_| "清理预览状态不可用".to_string())?
        .remove(plan_id);
    match plan {
        Some(plan) if plan.created_at.elapsed() <= CLEANUP_PLAN_TTL => Ok(plan),
        _ => Err("清理预览已过期或不存在，请重新预览后再执行".into()),
    }
}

fn reserve_unique_cleanup_path(seen_paths: &mut HashSet<PathBuf>, path: &Path) -> bool {
    seen_paths.insert(path.to_path_buf())
}

fn collect_junk_preview(
    ids: Vec<String>,
    older_than_days: u32,
) -> Result<JunkCleanupPreview, String> {
    if ids.is_empty() {
        return Err("未选择任何清理项".into());
    }
    if older_than_days > 365 {
        return Err("旧文件筛选天数超出范围".into());
    }
    let mut unique = HashSet::new();
    let mut seen_paths = HashSet::new();
    let mut files = Vec::new();
    let mut categories = Vec::new();
    let mut walked = 0usize;
    let mut over_limit = false;
    let mut recycle_selected = false;

    for id in ids {
        if !unique.insert(id.clone()) {
            continue;
        }
        if id == "recycle" {
            recycle_selected = true;
            let bytes = recycle_size().unwrap_or(0.0).max(0.0) * 1024.0 * 1024.0;
            categories.push(JunkPreviewCategory {
                id,
                label: cleanup_label("recycle").into(),
                candidate_files: 0,
                candidate_bytes: bytes.min(u64::MAX as f64) as u64,
                sample_paths: Vec::new(),
                age_filtered_files: 0,
                skipped_files: 0,
                permanent_action: true,
                exact: false,
            });
            continue;
        }
        let root = cleanup_root(&id).ok_or_else(|| format!("未知清理类别：{id}"))?;
        if !root.exists() {
            return Err(format!("清理目录已不存在：{}", root.display()));
        }
        let root_canonical = root
            .canonicalize()
            .map_err(|e| format!("无法验证清理目录：{e}"))?;
        if is_reparse_path(&root) {
            return Err("清理目录是链接或重解析点，已停止预览".into());
        }
        let targets = cleanup_targets(&id, &root);
        let mut category = JunkPreviewCategory {
            id: id.clone(),
            label: cleanup_label(&id).into(),
            candidate_files: 0,
            candidate_bytes: 0,
            sample_paths: Vec::new(),
            age_filtered_files: 0,
            skipped_files: 0,
            permanent_action: false,
            exact: true,
        };
        let now = SystemTime::now();
        'targets: for target in targets {
            let Some((target_root, safe_target)) = canonical_child(&root_canonical, &target) else {
                category.skipped_files += 1;
                category.exact = false;
                continue;
            };
            for entry in WalkDir::new(&safe_target).follow_links(false).min_depth(1) {
                walked += 1;
                if walked > CLEANUP_WALK_LIMIT || files.len() >= CLEANUP_CANDIDATE_LIMIT {
                    over_limit = true;
                    category.exact = false;
                    break 'targets;
                }
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        category.skipped_files += 1;
                        category.exact = false;
                        continue;
                    }
                };
                if entry.file_type().is_symlink() || is_reparse_path(entry.path()) {
                    category.skipped_files += 1;
                    category.exact = false;
                    continue;
                }
                if !entry.file_type().is_file() {
                    continue;
                }
                let metadata = match entry.metadata() {
                    Ok(metadata) => metadata,
                    Err(_) => {
                        category.skipped_files += 1;
                        category.exact = false;
                        continue;
                    }
                };
                let modified = match metadata.modified() {
                    Ok(modified) => modified,
                    Err(_) => {
                        category.skipped_files += 1;
                        category.exact = false;
                        continue;
                    }
                };
                if !is_old_enough(modified, now, older_than_days) {
                    category.age_filtered_files += 1;
                    continue;
                }
                let Some((_, canonical_file)) = canonical_child(&target_root, entry.path()) else {
                    category.skipped_files += 1;
                    category.exact = false;
                    continue;
                };
                if !reserve_unique_cleanup_path(&mut seen_paths, &canonical_file) {
                    continue;
                }
                let size = metadata.len();
                category.candidate_files += 1;
                category.candidate_bytes = category.candidate_bytes.saturating_add(size);
                if category.sample_paths.len() < CLEANUP_SAMPLE_LIMIT {
                    category
                        .sample_paths
                        .push(canonical_file.to_string_lossy().into_owned());
                }
                files.push(PlannedFile {
                    root: target_root.clone(),
                    path: canonical_file,
                    size,
                    modified,
                    older_than_days,
                });
            }
        }
        categories.push(category);
        if over_limit {
            break;
        }
    }

    let file_labels = categories
        .iter()
        .filter(|category| !category.permanent_action && category.candidate_files > 0)
        .map(|category| category.label.clone())
        .collect::<Vec<_>>();
    let plan_id = if !over_limit && (!files.is_empty() || recycle_selected) {
        Some(add_preview_plan(files, recycle_selected, file_labels)?)
    } else {
        None
    };
    let can_execute = plan_id.is_some();
    Ok(JunkCleanupPreview {
        plan_id,
        older_than_days,
        categories,
        can_execute,
        over_limit,
    })
}

#[tauri::command]
pub async fn preview_junk(
    ids: Vec<String>,
    older_than_days: u32,
) -> Result<JunkCleanupPreview, String> {
    tauri::async_runtime::spawn_blocking(move || collect_junk_preview(ids, older_than_days))
        .await
        .map_err(|e| e.to_string())?
}

fn run_after_native_confirmation<T>(
    confirm: impl FnOnce() -> Result<bool, String>,
    action: impl FnOnce() -> Result<T, String>,
) -> Result<Option<T>, String> {
    if confirm()? {
        action().map(Some)
    } else {
        Ok(None)
    }
}

#[cfg(windows)]
fn show_native_confirmation(title: &str, message: &str) -> Result<bool, String> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, IDOK, MB_DEFBUTTON2, MB_ICONWARNING, MB_OKCANCEL, MB_SETFOREGROUND,
        MB_TASKMODAL, MB_TOPMOST,
    };

    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let message: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let response = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OKCANCEL
                | MB_ICONWARNING
                | MB_DEFBUTTON2
                | MB_SETFOREGROUND
                | MB_TASKMODAL
                | MB_TOPMOST,
        )
    };
    if response == 0 {
        Err("无法显示 Windows 系统确认框；未执行清理操作".into())
    } else {
        Ok(response == IDOK)
    }
}

#[cfg(not(windows))]
fn show_native_confirmation(_title: &str, _message: &str) -> Result<bool, String> {
    Err("此操作需要 Windows 系统确认框；未执行清理操作".into())
}

fn confirm_cleanup_plan(
    file_count: usize,
    total_bytes: u64,
    file_labels: &[String],
) -> Result<bool, String> {
    let scope = if file_labels.is_empty() {
        "AeroTune 固定清理目录".to_string()
    } else {
        file_labels.join("、")
    };
    show_native_confirmation(
        "AeroTune — 确认永久清理",
        &format!(
            "即将从「{scope}」永久删除预览中的 {file_count} 个文件（{total_bytes} 字节），不会进入回收站。\n\n执行时会再次核验路径、文件类型、大小和修改时间，期间变化的文件将跳过。\n\n确定继续？"
        ),
    )
}

fn confirm_empty_recycle_bin() -> Result<bool, String> {
    show_native_confirmation(
        "AeroTune — 确认清空回收站",
        "即将永久清空当前 Windows 用户回收站中的全部项目。回收站未提供可逐项预览的清单；此操作范围不受清理预览的估算限制，且不可撤销。\n\n确定继续？",
    )
}

fn clean_junk_files(plan: CleanupPlan) -> Result<JunkCleanupResult, String> {
    let mut result = JunkCleanupResult::default();
    let now = SystemTime::now();
    for candidate in plan.files {
        let metadata = match std::fs::symlink_metadata(&candidate.path) {
            Ok(metadata) => metadata,
            Err(_) => {
                result.skipped_changed += 1;
                continue;
            }
        };
        if !metadata.file_type().is_file() || is_reparse_path(&candidate.path) {
            result.skipped_unsafe += 1;
            continue;
        }
        let Some((canonical_root, canonical_file)) =
            canonical_child(&candidate.root, &candidate.path)
        else {
            result.skipped_unsafe += 1;
            continue;
        };
        let current = match std::fs::metadata(&canonical_file) {
            Ok(metadata) => metadata,
            Err(_) => {
                result.skipped_changed += 1;
                continue;
            }
        };
        let modified = current.modified().ok();
        if current.len() != candidate.size
            || modified != Some(candidate.modified)
            || !is_old_enough(candidate.modified, now, candidate.older_than_days)
        {
            result.skipped_changed += 1;
            continue;
        }
        if !canonical_file.starts_with(&canonical_root) {
            result.skipped_unsafe += 1;
            continue;
        }
        match std::fs::remove_file(&canonical_file) {
            Ok(()) => {
                result.deleted_files += 1;
                result.deleted_bytes = result.deleted_bytes.saturating_add(candidate.size);
            }
            Err(error) => {
                result.failed += 1;
                if result.errors.len() < 3 {
                    result.errors.push(error.to_string());
                }
            }
        }
    }
    Ok(result)
}

#[tauri::command]
pub async fn clean_junk(plan_id: String) -> Result<JunkCleanupResult, String> {
    let plan = take_cleanup_plan(&plan_id)?;
    let file_count = plan.files.len();
    let total_bytes = plan
        .files
        .iter()
        .fold(0u64, |total, file| total.saturating_add(file.size));
    let file_labels = plan.file_labels.clone();
    if file_count == 0 {
        return Ok(JunkCleanupResult::default());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = run_after_native_confirmation(
            || confirm_cleanup_plan(file_count, total_bytes, &file_labels),
            || clean_junk_files(plan),
        )?;
        Ok(result.unwrap_or_else(|| JunkCleanupResult {
            cancelled: true,
            ..JunkCleanupResult::default()
        }))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn empty_recycle_bin(plan_id: String) -> Result<bool, String> {
    let plan = take_cleanup_plan(&plan_id)?;
    if !plan.recycle_selected {
        return Err("当前预览没有包含回收站，请重新预览后操作".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let result = run_after_native_confirmation(confirm_empty_recycle_bin, || {
            crate::util::run_checked_with_timeout(
                "powershell",
                &[
                    "-NoProfile",
                    "-NonInteractive",
                    "-Command",
                    "$ErrorActionPreference='Stop'; Clear-RecycleBin -Force -ErrorAction Stop",
                ],
                Duration::from_secs(10 * 60),
            )
            .map(|_| ())
        })?;
        Ok(result.is_some())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod cleanup_tests {
    use super::*;

    #[test]
    fn cancelled_native_confirmation_never_executes_cleanup_action() {
        let action_called = std::cell::Cell::new(false);
        let result = run_after_native_confirmation(
            || Ok(false),
            || {
                action_called.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(result.is_none());
        assert!(!action_called.get());
    }

    #[test]
    fn confirmed_native_confirmation_executes_action_and_returns_its_result() {
        let action_called = std::cell::Cell::new(false);
        let result = run_after_native_confirmation(
            || Ok(true),
            || {
                action_called.set(true);
                Ok(17)
            },
        )
        .unwrap();
        assert_eq!(result, Some(17));
        assert!(action_called.get());
    }

    #[test]
    fn native_confirmation_failure_does_not_execute_cleanup_action() {
        let action_called = std::cell::Cell::new(false);
        let error = run_after_native_confirmation::<()>(
            || Err("confirmation unavailable".into()),
            || {
                action_called.set(true);
                Ok(())
            },
        )
        .unwrap_err();
        assert_eq!(error, "confirmation unavailable");
        assert!(!action_called.get());
    }

    #[test]
    fn overlapping_cleanup_roots_keep_one_planned_path() {
        let mut seen_paths = HashSet::new();
        let file = PathBuf::from(r"C:\Windows\Temp\sample.tmp");
        assert!(reserve_unique_cleanup_path(&mut seen_paths, &file));
        assert!(!reserve_unique_cleanup_path(&mut seen_paths, &file));
        assert_eq!(seen_paths.len(), 1);
    }

    #[test]
    fn deletion_counts_only_mock_success_and_reports_partial_failure() {
        let mut t = DeleteTally::default();
        t.account(100, Ok(()));
        t.account(900, Err("locked".into()));
        assert_eq!(t.bytes, 100);
        assert_eq!(t.files, 1);
        let err = t.finish().unwrap_err();
        assert!(err.contains("100 字节"));
        assert!(err.contains("locked"));
        let mut t = DeleteTally::default();
        t.account(42, Ok(()));
        assert_eq!(t.finish().unwrap(), (42, 1));
    }

    #[test]
    fn ordinary_directories_are_not_browser_profiles() {
        let root = Path::new(r"Z:\nonexistent\temporary");
        for id in ["user_temp", "win_temp", "update_cache"] {
            assert_eq!(cleanup_targets(id, root), vec![root.to_path_buf()]);
        }
        assert!(cleanup_targets("unknown", root).is_empty());
        assert!(cleanup_targets("edge_cache", root).is_empty());
    }

    #[test]
    fn age_filter_requires_a_known_older_timestamp() {
        let now = std::time::UNIX_EPOCH + Duration::from_secs(2_000_000);
        assert!(is_old_enough(now - Duration::from_secs(7 * 86_400), now, 7));
        assert!(!is_old_enough(
            now - Duration::from_secs(7 * 86_400 - 1),
            now,
            7
        ));
        assert!(!is_old_enough(now + Duration::from_secs(1), now, 7));
        assert!(is_old_enough(now + Duration::from_secs(1), now, 0));
    }

    #[test]
    fn canonical_containment_rejects_paths_outside_the_cleanup_root() {
        let unique = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("aerotune-preview-{unique}"));
        let root = base.join("allowed");
        let outside = base.join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let inside_file = root.join("sample.tmp");
        let outside_file = outside.join("sample.tmp");
        std::fs::write(&inside_file, b"fixture").unwrap();
        std::fs::write(&outside_file, b"fixture").unwrap();
        assert!(canonical_child(&root, &inside_file).is_some());
        assert!(canonical_child(&root, &outside_file).is_none());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn preview_plans_are_short_lived_single_use_and_keep_native_prompt_scope() {
        let file_labels = vec!["用户临时文件".to_string()];
        let id = add_preview_plan(Vec::new(), false, file_labels.clone()).unwrap();
        let plan = take_cleanup_plan(&id).unwrap();
        assert!(!plan.recycle_selected);
        assert_eq!(plan.file_labels, file_labels);
        assert!(take_cleanup_plan(&id).is_err());

        let stale_id = format!(
            "test-stale-{}",
            NEXT_CLEANUP_PLAN.fetch_add(1, Ordering::Relaxed)
        );
        cleanup_plan_store().lock().unwrap().insert(
            stale_id.clone(),
            CleanupPlan {
                created_at: Instant::now() - CLEANUP_PLAN_TTL - Duration::from_secs(1),
                files: Vec::new(),
                file_labels: Vec::new(),
                recycle_selected: false,
            },
        );
        assert!(take_cleanup_plan(&stale_id).is_err());
    }
}

fn win_dir(sub: &str) -> PathBuf {
    PathBuf::from(r"C:\Windows").join(sub)
}

/// 回收站大小（Shell COM），失败返回 None
fn recycle_size() -> Option<f64> {
    let out = crate::util::run_ps(
        "$s=0; (New-Object -ComObject Shell.Application).NameSpace(0xA).Items() | ForEach-Object { $s += $_.Size }; [math]::Round($s/1MB,1)",
    )?;
    out.trim().parse().ok()
}

#[tauri::command]
pub async fn scan_junk() -> Vec<JunkItem> {
    tauri::async_runtime::spawn_blocking(|| {
        let admin = crate::util::is_admin();
        let mut items: Vec<JunkItem> = Vec::new();

        let mk = |id: &str, label: &str, p: PathBuf, admin_required: bool| -> Option<JunkItem> {
            if !p.exists() {
                return None;
            }
            let (size, files) = dir_size(&p);
            if files == 0 {
                return None;
            }
            let age = median_age_days(&p);
            let confidence = if age >= 7.0 {
                "high"
            } else if age >= 2.0 {
                "mid"
            } else {
                "low"
            };
            Some(JunkItem {
                id: id.into(),
                label: label.into(),
                path: p.to_string_lossy().into_owned(),
                size_mb: size as f64 / 1024.0 / 1024.0,
                files,
                admin_required,
                confidence: confidence.into(),
                median_age_days: (age * 10.0).round() / 10.0,
            })
        };

        if let Some(p) = user_temp() {
            items.extend(mk("user_temp", "用户临时文件", p, false));
        }
        items.extend(mk(
            "win_temp",
            "系统临时文件（Windows\\Temp）",
            win_dir("Temp"),
            true,
        ));
        items.extend(mk(
            "update_cache",
            "Windows 更新缓存（SoftwareDistribution\\Download）",
            win_dir(r"SoftwareDistribution\Download"),
            true,
        ));
        for (id, label, vendor_root) in [
            (
                "edge_cache",
                "Edge 浏览器缓存（全部 Profile）",
                local_appdata(r"Microsoft\Edge\User Data"),
            ),
            (
                "chrome_cache",
                "Chrome 浏览器缓存（全部 Profile）",
                local_appdata(r"Google\Chrome\User Data"),
            ),
        ] {
            if let Some(root) = vendor_root {
                if let Some((size, files)) = browser_cache_aggregate(root.clone()) {
                    if files > 0 {
                        items.push(JunkItem {
                            id: id.into(),
                            label: label.into(),
                            path: root.to_string_lossy().into_owned(),
                            size_mb: size as f64 / 1024.0 / 1024.0,
                            files,
                            admin_required: false,
                            confidence: "mid".into(),
                            median_age_days: 0.0,
                        });
                    }
                }
            }
        }
        if let Some(size_mb) = recycle_size() {
            if size_mb > 0.0 {
                items.push(JunkItem {
                    id: "recycle".into(),
                    label: "回收站".into(),
                    path: r"$Recycle.Bin".into(),
                    size_mb,
                    files: 0,
                    admin_required: false,
                    confidence: "high".into(),
                    median_age_days: 0.0,
                });
            }
        }
        items.sort_by(|a, b| {
            b.size_mb
                .partial_cmp(&a.size_mb)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let _ = admin;
        items
    })
    .await
    .unwrap_or_default()
}

#[cfg(test)]
#[derive(Default)]
struct DeleteTally {
    bytes: u64,
    files: u64,
    errors: Vec<String>,
}
#[cfg(test)]
impl DeleteTally {
    fn account(&mut self, bytes: u64, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.bytes += bytes;
                self.files += 1;
            }
            Err(e) => self.errors.push(e),
        }
    }
    fn finish(self) -> Result<(u64, u64), String> {
        if self.errors.is_empty() {
            Ok((self.bytes, self.files))
        } else {
            Err(format!(
                "部分清理失败；成功删除 {} 字节 / {} 文件；{} 项失败：{}",
                self.bytes,
                self.files,
                self.errors.len(),
                self.errors
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("；")
            ))
        }
    }
}

// ---------- UWP 预装应用管理 ----------

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct AppxApp {
    /// Readable display name, resolved from Start-menu/manifest metadata first.
    pub name: String,
    /// Package identity name, retained for searches and exact identification.
    pub package_name: String,
    pub full_name: String,
    pub publisher: String,
    pub install_location: String,
    pub version: String,
    /// System-signed or protected Windows inbox package.
    pub protected: bool,
}

/// 拆除会导致系统功能损坏的核心包
const PROTECTED_APPX: &[&str] = &[
    "shellexperiencehost",
    "startmenuexperiencehost",
    "ui.xaml",
    "vclibs",
    "net.native",
    "sechealthui",
    "windowsstore",
    "desktopappinstaller",
    "storepurchaseapp",
    "services.store",
    "accountscontrol",
    "windows.apprep",
    "windows.appinstall",
];

fn appx_protected(full_name: &str) -> bool {
    let prefix = full_name.split('_').next().unwrap_or("").to_lowercase();
    PROTECTED_APPX.iter().any(|p| prefix.contains(p))
}

fn non_resource_name(value: &str) -> Option<&str> {
    let value = value.trim();
    if value.is_empty() || value.starts_with("ms-resource:") || value.starts_with('@') {
        None
    } else {
        Some(value)
    }
}

fn appx_display_name(
    identity_name: &str,
    manifest_name: &str,
    start_menu_name: &str,
    install_location: &str,
) -> String {
    if let Some(name) =
        non_resource_name(start_menu_name).or_else(|| non_resource_name(manifest_name))
    {
        return name.to_owned();
    }
    let identity = identity_name.trim_matches(['{', '}']).to_ascii_lowercase();
    let folder_name = install_location.rsplit(['\\', '/']).next().unwrap_or("");
    let folder = folder_name.to_ascii_lowercase();
    let known = [
        ("1527c705-839a-4832-9118-54d4bd6a0c89", "Windows 文件选择器"),
        ("c5e2524a-ea46-4f67-841f-6a9465d9d515", "文件资源管理器"),
        ("e2a4f912-2574-4a75-9bb0-0d023378592b", "打开方式"),
        ("f46d4000-fd22-4db4-ac8e-4e1ddde828fe", "将文件夹添加到库"),
    ];
    if let Some((_, name)) = known.iter().find(|(id, _)| *id == identity) {
        return (*name).to_owned();
    }
    for (marker, name) in [
        ("microsoft.windows.filepicker_", "Windows 文件选择器"),
        ("microsoft.windows.fileexplorer_", "文件资源管理器"),
        ("microsoft.windows.appresolverux_", "打开方式"),
        (
            "microsoft.windows.addsuggestedfolderstolibrarydialog_",
            "将文件夹添加到库",
        ),
    ] {
        if folder.contains(marker) {
            return name.to_owned();
        }
    }
    let friendly = folder_name
        .strip_suffix("_cw5n1h2txyewy")
        .or_else(|| folder_name.strip_suffix("_8wekyb3d8bbwe"))
        .unwrap_or(folder_name)
        .replace(['_', '-'], " ");
    if friendly.trim().is_empty() {
        identity_name.to_owned()
    } else {
        friendly
    }
}

fn appx_from_value(r: serde_json::Value) -> Option<AppxApp> {
    let package_name = r["Name"].as_str()?.trim().to_owned();
    let full_name = r["PackageFullName"].as_str()?.trim().to_owned();
    if package_name.is_empty() || full_name.is_empty() {
        return None;
    }
    let install_location = r["InstallLocation"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_owned();
    let name = appx_display_name(
        &package_name,
        r["ManifestDisplayName"].as_str().unwrap_or(""),
        r["StartMenuName"].as_str().unwrap_or(""),
        &install_location,
    );
    let protected = appx_protected(&full_name)
        || r["IsDevelopmentMode"].as_bool().unwrap_or(false)
        || r["SignatureKind"]
            .as_str()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("System"));
    Some(AppxApp {
        name,
        package_name,
        full_name,
        publisher: r["Publisher"]
            .as_str()
            .and_then(non_resource_name)
            .unwrap_or("")
            .to_owned(),
        install_location,
        version: r["Version"].as_str().unwrap_or("").to_owned(),
        protected,
    })
}

#[cfg(test)]
mod appx_tests {
    use super::*;

    #[test]
    fn display_name_prefers_registered_labels_then_manifest_then_friendly_folder() {
        assert_eq!(
            appx_display_name(
                "UUID",
                "ms-resource:Display",
                "File Picker",
                r"C:\Windows\SystemApps\Microsoft.Windows.FilePicker_cw5n1h2txyewy"
            ),
            "File Picker"
        );
        assert_eq!(
            appx_display_name(
                "Example.Package",
                "Example App",
                "",
                r"C:\Program Files\WindowsApps\Example.App_1.0.0.0_x64__publisher"
            ),
            "Example App"
        );
        assert_eq!(
            appx_display_name(
                "c5e2524a-ea46-4f67-841f-6a9465d9d515",
                "@{Display}",
                "",
                r"C:\Windows\SystemApps\Microsoft.Windows.FileExplorer_cw5n1h2txyewy"
            ),
            "文件资源管理器"
        );
        assert_eq!(
            appx_display_name(
                "Unknown.Package",
                "ms-resource:Display",
                "",
                r"C:\Apps\Vendor_App_cw5n1h2txyewy"
            ),
            "Vendor App"
        );
        assert_eq!(
            appx_display_name(
                "{C5E2524A-EA46-4F67-841F-6A9465D9D515}",
                "ms-resource:Display",
                "",
                ""
            ),
            "文件资源管理器"
        );
    }

    #[test]
    fn appx_value_mapping_preserves_exact_identity_location_and_version() {
        let value = serde_json::json!({
            "Name":"1527c705-839a-4832-9118-54d4Bd6a0c89",
            "PackageFullName":"1527c705-839a-4832-9118-54d4Bd6a0c89_1.2.3.0_neutral__publisher",
            "Publisher":"Microsoft Corporation","IsDevelopmentMode":false,"SignatureKind":"Store",
            "InstallLocation":r"C:\Windows\SystemApps\Microsoft.Windows.FilePicker_cw5n1h2txyewy",
            "Version":"1.2.3.0","ManifestDisplayName":"ms-resource:AppxManifest_DisplayName","StartMenuName":""
        });
        let app = appx_from_value(value).unwrap();
        assert_eq!(app.name, "Windows 文件选择器");
        assert_eq!(app.package_name, "1527c705-839a-4832-9118-54d4Bd6a0c89");
        assert_eq!(
            app.install_location,
            r"C:\Windows\SystemApps\Microsoft.Windows.FilePicker_cw5n1h2txyewy"
        );
        assert_eq!(app.version, "1.2.3.0");
        assert_eq!(app.publisher, "Microsoft Corporation");
        assert!(!app.protected);
        let system = appx_from_value(serde_json::json!({
            "Name":"Example.SystemApp","PackageFullName":"Example.SystemApp_1_neutral__publisher",
            "InstallLocation":r"C:\Windows\SystemApps\Example","Version":"1.0",
            "IsDevelopmentMode":false,"SignatureKind":"System"
        }))
        .unwrap();
        assert!(system.protected);
        let appx_user = appx_from_value(serde_json::json!({
            "Name":"Example.App","PackageFullName":"Example.App_1_neutral__publisher",
            "InstallLocation":r"C:\Program Files\WindowsApps\Example.App","Version":"1.0",
            "SignatureKind":"Store","IsDevelopmentMode":false
        }))
        .unwrap();
        assert!(!appx_user.protected);
        assert!(appx_from_value(serde_json::json!({"Name":"Microsoft.WindowsStore","PackageFullName":"Microsoft.WindowsStore_1_neutral__publisher"})).unwrap().protected);
        assert!(appx_from_value(
            serde_json::json!({"Name":"Example.Package","PackageFullName":""})
        )
        .is_none());
    }
}

/// AppX 列表短 TTL 缓存：预装应用页挂载时避免每次重跑 Get-AppxPackage（可达 1-2 分钟）。
/// 卸载成功后主动失效，保证列表即时刷新。
static APPX_CACHE: std::sync::LazyLock<
    std::sync::Mutex<Option<(std::time::Instant, Vec<AppxApp>)>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));
const APPX_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(120);

pub(crate) fn invalidate_appx_cache() {
    if let Ok(mut guard) = APPX_CACHE.lock() {
        *guard = None;
    }
}

#[tauri::command]
pub async fn list_appx() -> Result<Vec<AppxApp>, String> {
    if let Ok(guard) = APPX_CACHE.lock() {
        if let Some((at, apps)) = guard.as_ref() {
            if at.elapsed() < APPX_CACHE_TTL {
                return Ok(apps.clone());
            }
        }
    }
    let apps = tauri::async_runtime::spawn_blocking(|| {
        let script = r#"
$startApps = @{};
try { Get-StartApps | ForEach-Object { $appId=[string]$_.AppID; $display=[string]$_.Name; if($appId -and $display -and -not $startApps.ContainsKey($appId)){$startApps[$appId]=$display} } } catch { }
$packages = @(Get-AppxPackage | Where-Object { -not $_.IsFramework } | ForEach-Object {
  $manifestDisplay=''; $family=[string]$_.PackageFamilyName; $identity=[string]$_.Name;
  $startName=''; if($family){foreach($key in $startApps.Keys){if($key.StartsWith($family+'!',[System.StringComparison]::OrdinalIgnoreCase)){$startName=$startApps[$key];break}}}
  if($identity -match '^[{]?[0-9a-fA-F]{8}(-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}[}]?$') {
    try { $manifest=Get-AppxPackageManifest -Package $_.PackageFullName; $manifestDisplay=[string]$manifest.Package.Properties.DisplayName } catch { }
  }
  [pscustomobject]@{Name=[string]$_.Name;PackageFullName=[string]$_.PackageFullName;Publisher=[string]$_.Publisher;InstallLocation=[string]$_.InstallLocation;Version=[string]$_.Version;ManifestDisplayName=$manifestDisplay;StartMenuName=$startName;IsDevelopmentMode=[bool]$_.IsDevelopmentMode;SignatureKind=[string]$_.SignatureKind}
});
ConvertTo-Json -InputObject $packages -Compress -Depth 5
"#;
        let raw = crate::util::run_ps_with_timeout(script, std::time::Duration::from_secs(120))
            .ok_or_else(|| "读取 Windows 预装应用列表失败，请刷新重试。".to_owned())?;
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            if value.is_array() || value.is_object() {
                let count = if let Some(array) = value.as_array() { array.len() } else { 1 };
                tracing::info!(count, "AppX 清单读取完成");
            }
        }
        let parsed: serde_json::Value = serde_json::from_str(&raw).map_err(|e| format!("解析 Windows 应用清单失败：{e}"))?;
        let values = match parsed {
            serde_json::Value::Array(values) => values,
            serde_json::Value::Object(value) => vec![serde_json::Value::Object(value)],
            _ => return Err("Windows 应用清单格式不受支持。".to_owned()),
        };
        let mut apps: Vec<AppxApp> = values.into_iter().filter_map(appx_from_value).collect();
        apps.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.package_name.to_lowercase().cmp(&b.package_name.to_lowercase()))
        });
        Ok(apps)
    })
    .await
    .map_err(|e| format!("预装应用扫描任务失败：{e}"))??;
    if let Ok(mut guard) = APPX_CACHE.lock() {
        *guard = Some((std::time::Instant::now(), apps.clone()));
    }
    Ok(apps)
}

/// 卸载预装应用（当前用户；商店可重装）
#[tauri::command]
pub fn remove_appx(full_name: String) -> Result<(), String> {
    if full_name.contains('\'') || full_name.len() > 200 {
        return Err("包名不合法".into());
    }
    if appx_protected(&full_name) {
        return Err("该应用为系统核心组件，AeroTune 拒绝卸载".into());
    }
    crate::util::run_checked_with_timeout(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!("Remove-AppxPackage -Package '{full_name}'"),
        ],
        std::time::Duration::from_secs(10 * 60),
    )?;
    invalidate_appx_cache();
    crate::journal::record(
        &format!("卸载预装应用 {full_name}（当前用户）"),
        crate::journal::Revert::None,
    )?;
    tracing::info!(package = %full_name, "预装应用已卸载");
    Ok(())
}

// ---------- 系统级开关：CompactOS / 内存压缩 ----------

/// CompactOS 系统文件压缩状态（true=已压缩）
#[tauri::command]
pub fn compactos_status() -> Result<bool, String> {
    let out = crate::util::run("compact", &["/compactos:query"]).ok_or("compact 命令不可用")?;
    let lower = out.to_lowercase();
    Ok(lower.contains("处于 compact")
        || lower.contains("in the compact")
        || lower.contains("已处于"))
}

#[tauri::command]
pub fn set_compactos(on: bool) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("CompactOS 切换需要管理员权限".into());
    }
    let argument = if on {
        "/compactos:always"
    } else {
        "/compactos:never"
    };
    crate::util::run_checked_with_timeout(
        "compact",
        &[argument],
        std::time::Duration::from_secs(30 * 60),
    )?;
    crate::journal::record(
        &format!(
            "CompactOS 系统文件压缩 → {}",
            if on {
                "开启（省磁盘空间，轻微 CPU 开销）"
            } else {
                "关闭"
            }
        ),
        crate::journal::Revert::None,
    )?;
    tracing::info!(on, "CompactOS 已切换");
    Ok(())
}

/// 内存压缩状态（Get-MMAgent）
#[tauri::command]
pub fn mem_compression_status() -> Result<bool, String> {
    let out = crate::util::run_ps("(Get-MMAgent).MemoryCompression").ok_or("MMAgent 不可用")?;
    Ok(out.trim().eq_ignore_ascii_case("true"))
}

#[tauri::command]
pub fn set_mem_compression(on: bool) -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("内存压缩切换需要管理员权限".into());
    }
    crate::util::run_checked(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "Set-MMAgent -MemoryCompression ${}",
                if on { "true" } else { "false" }
            ),
        ],
    )?;
    crate::journal::record(
        &format!(
            "内存压缩 → {}",
            if on {
                "开启（重启后生效）"
            } else {
                "关闭（重启后生效）"
            }
        ),
        crate::journal::Revert::None,
    )?;
    tracing::info!(on, "内存压缩已切换（重启后生效）");
    Ok(())
}

// ---------- 系统升级残留治理（C 盘大头盲区） ----------

/// 固定白名单：只允许对以下已知安全的升级残留路径执行删除
const RESIDUAL_PATHS: &[(&str, &str, &str)] = &[
    (
        r"C:\Windows.old",
        "Windows.old（旧系统备份）",
        "升级/重装后保留的完整旧系统，通常 10-30GB",
    ),
    (r"C:\$WinREAgent", "恢复环境更新残留", "系统升级临时文件"),
    (r"C:\ESD", "ESD 升级映像", "Windows 升级下载与解压映像"),
    (
        r"C:\Config.Msi",
        "安装回滚事务文件",
        "MSI 安装/升级回滚数据",
    ),
    (
        r"C:\Windows\LiveKernelReports",
        "内核实时转储",
        "系统故障时生成的转储文件",
    ),
    (r"C:\Windows\Minidump", "小型内存转储", "蓝屏诊断转储"),
];

fn residual_entry(path: &str) -> Option<(&'static str, &'static str, &'static str)> {
    let normalized = path.trim_end_matches('\\');
    RESIDUAL_PATHS
        .iter()
        .copied()
        .find(|(allowed, _, _)| allowed.eq_ignore_ascii_case(normalized))
}

const RESIDUAL_TREE_ENTRY_LIMIT: usize = 1_000_000;
const RESIDUAL_TREE_TIMEOUT: Duration = Duration::from_secs(120);
const RESIDUAL_SCAN_ENTRY_LIMIT: usize = 250_000;
const RESIDUAL_SCAN_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
const RESIDUAL_SCAN_PER_ROOT_TIMEOUT: Duration = Duration::from_secs(10);
static RESIDUAL_SCAN_ACTIVE: AtomicBool = AtomicBool::new(false);

struct ResidualScanGuard;

impl ResidualScanGuard {
    fn acquire() -> Result<Self, String> {
        RESIDUAL_SCAN_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| "系统残留扫描已在进行中".into())
    }
}

impl Drop for ResidualScanGuard {
    fn drop(&mut self) {
        RESIDUAL_SCAN_ACTIVE.store(false, Ordering::Release);
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct ResidualTreeStats {
    entries: usize,
    files: u64,
    bytes: u64,
}

fn validate_residual_root(path: &Path) -> Result<(), String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "系统残留目录不存在或不可访问".to_string())?;
    if !metadata.is_dir() || metadata_is_reparse(&metadata) {
        return Err("拒绝对重解析点或非文件夹执行递归删除".into());
    }
    Ok(())
}

fn validate_residual_tree_with_limits(
    root: &Path,
    max_entries: usize,
    timeout: Duration,
) -> Result<ResidualTreeStats, String> {
    let started = Instant::now();
    let root_metadata = std::fs::symlink_metadata(root)
        .map_err(|_| "无法完整检查系统残留目录；未执行删除".to_string())?;
    if !root_metadata.is_dir() || metadata_is_reparse(&root_metadata) {
        return Err("检测到无效或重解析的根目录；未执行删除".into());
    }

    let root_entries =
        std::fs::read_dir(root).map_err(|_| "无法完整检查系统残留目录；未执行删除".to_string())?;
    let mut stack = vec![(root.to_path_buf(), root_entries)];
    let mut stats = ResidualTreeStats {
        entries: 1,
        ..ResidualTreeStats::default()
    };
    if stats.entries > max_entries {
        return Err("残留树超过安全检查上限；未执行删除".into());
    }
    while let Some((directory, mut entries)) = stack.pop() {
        if started.elapsed() >= timeout {
            return Err("残留树安全检查超时；未执行删除".into());
        }
        let entry = match entries.next() {
            Some(Ok(entry)) => entry,
            Some(Err(_)) => return Err("无法完整检查残留树；未执行删除".into()),
            None => continue,
        };
        stack.push((directory, entries));
        stats.entries = stats.entries.saturating_add(1);
        if stats.entries > max_entries {
            return Err("残留树超过安全检查上限；未执行删除".into());
        }
        let child = entry.path();
        let metadata = std::fs::symlink_metadata(&child)
            .map_err(|_| "无法完整检查残留树；未执行删除".to_string())?;
        if metadata_is_reparse(&metadata) {
            return Err("残留树包含重解析点；拒绝递归删除".into());
        }
        if metadata.is_file() {
            if residual_file_link_count(&child)? > 1 {
                return Err("残留树包含硬链接文件；拒绝递归更改其所有权或权限".into());
            }
            stats.files = stats.files.saturating_add(1);
            stats.bytes = stats.bytes.saturating_add(metadata.len());
        } else if metadata.is_dir() {
            let child_entries = std::fs::read_dir(&child)
                .map_err(|_| "无法完整检查残留树；未执行删除".to_string())?;
            stack.push((child, child_entries));
        }
    }
    Ok(stats)
}

fn validate_residual_tree(root: &Path) -> Result<ResidualTreeStats, String> {
    validate_residual_tree_with_limits(root, RESIDUAL_TREE_ENTRY_LIMIT, RESIDUAL_TREE_TIMEOUT)
}

#[derive(Serialize, Clone, Debug)]
pub struct ResidualItem {
    pub path: String,
    pub label: String,
    pub desc: String,
    pub size_mb: Option<f64>,
    pub exists: bool,
    pub safe_to_delete: bool,
    pub scan_error: Option<String>,
}

fn residual_scan_item(
    path: &str,
    label: &str,
    desc: &str,
    result: Result<ResidualTreeStats, String>,
) -> Option<ResidualItem> {
    match result {
        Ok(stats) if stats.bytes > 0 => Some(ResidualItem {
            path: path.to_string(),
            label: label.to_string(),
            desc: desc.to_string(),
            size_mb: Some(stats.bytes as f64 / 1024.0 / 1024.0),
            exists: true,
            safe_to_delete: true,
            scan_error: None,
        }),
        Ok(_) => None,
        Err(error) => Some(ResidualItem {
            path: path.to_string(),
            label: label.to_string(),
            desc: desc.to_string(),
            size_mb: None,
            exists: true,
            safe_to_delete: false,
            scan_error: Some(error),
        }),
    }
}

fn scan_residuals_blocking() -> Vec<ResidualItem> {
    let scan_started = Instant::now();
    let mut items = Vec::new();
    for &(path, label, desc) in RESIDUAL_PATHS {
        let root = Path::new(path);
        match std::fs::symlink_metadata(root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                if let Some(item) = residual_scan_item(
                    path,
                    label,
                    desc,
                    Err("无法确认系统残留路径状态；为安全起见已禁用删除".into()),
                ) {
                    items.push(item);
                }
                continue;
            }
            Ok(_) => {}
        }

        let remaining = RESIDUAL_SCAN_TOTAL_TIMEOUT.saturating_sub(scan_started.elapsed());
        let timeout = remaining.min(RESIDUAL_SCAN_PER_ROOT_TIMEOUT);
        let result = if timeout.is_zero() {
            Err("残留扫描总耗时达到上限；本项无法安全估算".into())
        } else {
            validate_residual_tree_with_limits(root, RESIDUAL_SCAN_ENTRY_LIMIT, timeout)
        };
        if let Some(item) = residual_scan_item(path, label, desc, result) {
            items.push(item);
        }
    }
    items
}

/// 枚举升级残留；检查不完整的项目可见但不提供删除操作。
#[tauri::command]
pub async fn scan_residuals() -> Result<Vec<ResidualItem>, String> {
    let guard = ResidualScanGuard::acquire()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        scan_residuals_blocking()
    })
    .await
    .map_err(|error| format!("系统残留扫描工作线程失败：{error}"))
}

fn remove_residual_authorized_blocking(
    path: &'static str,
    label: &'static str,
) -> Result<(), String> {
    let target = Path::new(path);
    let inspected = validate_residual_tree(target)?;
    tracing::info!(
        path,
        label,
        entries = inspected.entries,
        files = inspected.files,
        "残留树安全检查通过"
    );
    tracing::warn!(path, label, "开始删除系统升级残留（takeown 链）");
    let cleanup_timeout = Duration::from_secs(30 * 60);
    crate::util::run_checked_with_timeout(
        "takeown",
        &["/f", path, "/r", "/d", "y"],
        cleanup_timeout,
    )?;
    crate::util::run_checked_with_timeout(
        "icacls",
        &[path, "/grant", "Administrators:F", "/t", "/q"],
        cleanup_timeout,
    )?;
    validate_residual_root(target)?;
    crate::util::run_checked_with_timeout(
        "cmd",
        &["/c", &format!("rd /s /q {path}")],
        cleanup_timeout,
    )?;
    if target.exists() {
        return Err("删除未完成（文件可能被占用，重启后再试）".into());
    }
    crate::journal::record(
        &format!("删除系统升级残留 {path}（不可恢复，已原生确认）"),
        crate::journal::Revert::None,
    )?;
    tracing::info!(path, label, "系统升级残留已删除");
    Ok(())
}

/// 删除升级残留：先对后代树无跟随预检，再执行 takeown → icacls → rd。
/// 仅接受固定白名单路径；预检/权限命令失败时不运行递归删除。
#[tauri::command]
pub async fn remove_residual(app: tauri::AppHandle, path: String) -> Result<bool, String> {
    if !crate::util::is_admin() {
        return Err("删除系统残留需要管理员权限".into());
    }
    let (p, label, description) = residual_entry(&path)
        .ok_or_else(|| "该路径不在安全白名单内，AeroTune 拒绝删除".to_string())?;
    validate_residual_root(Path::new(p))?;

    let message = format!(
        "将永久删除以下 Windows 系统残留（不移入回收站，无法撤销）：\n\n{label}\n{p}\n{description}\n\n仅在确认这是预期清理项时继续。"
    );
    let confirmed = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

        app.dialog()
            .message(message)
            .title("确认永久删除系统残留")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancel)
            .blocking_show()
    })
    .await
    .map_err(|error| format!("原生确认对话框失败: {error}"))?;
    if !confirmed {
        return Ok(false);
    }

    tauri::async_runtime::spawn_blocking(move || remove_residual_authorized_blocking(p, label))
        .await
        .map_err(|error| format!("系统残留清理任务失败: {error}"))??;
    Ok(true)
}

#[cfg(test)]
mod residual_tests {
    use super::*;

    #[test]
    fn residual_authorization_returns_only_canonical_allowlist_paths() {
        for &(path, _, _) in RESIDUAL_PATHS {
            let (safe_path, _, _) = residual_entry(path).unwrap();
            assert_eq!(safe_path, path);
            assert_eq!(residual_entry(&path.to_ascii_lowercase()).unwrap().0, path);
            assert!(!path.chars().any(|ch| {
                ch.is_whitespace() || matches!(ch, '"' | '&' | '|' | '<' | '>' | '^' | '%' | '!')
            }));
        }
        for path in [
            r"C:\Windows.old\..\Windows",
            r"C:\Windows.old\child",
            r"C:\Windows.old:stream",
            r"\\server\share\Windows.old",
            r"C:\$WinREAgent\..\Windows.old",
        ] {
            assert!(residual_entry(path).is_none(), "accepted {path:?}");
        }
    }

    #[test]
    fn residual_root_validation_accepts_plain_directories_only() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "aerotune-residual-root-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        assert!(validate_residual_root(&root).is_ok());
        let file = root.join("not-a-directory");
        std::fs::write(&file, b"test").unwrap();
        assert!(validate_residual_root(&file).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn residual_tree_preflight_fails_closed_on_limits_and_reparse_points() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "aerotune-residual-tree-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("file.bin"), b"data").unwrap();
        std::fs::write(nested.join("child.bin"), b"data").unwrap();

        let inspected =
            validate_residual_tree_with_limits(&root, 10, Duration::from_secs(5)).unwrap();
        assert!(inspected.entries >= 4);
        assert_eq!(inspected.files, 2);
        assert_eq!(inspected.bytes, 8);
        assert!(
            validate_residual_tree_with_limits(&root, 1, Duration::from_secs(5))
                .unwrap_err()
                .contains("上限")
        );
        assert!(
            validate_residual_tree_with_limits(&root, 10, Duration::ZERO)
                .unwrap_err()
                .contains("超时")
        );

        #[cfg(windows)]
        {
            let outside = root.join("outside");
            std::fs::create_dir_all(&outside).unwrap();
            let symlink = nested.join("reparse-symlink");
            let junction = nested.join("reparse-junction");
            let symlink_created = std::os::windows::fs::symlink_dir(&outside, &symlink).is_ok();
            let junction_created = if symlink_created {
                false
            } else {
                let link_arg = junction.to_string_lossy().into_owned();
                let target_arg = outside.to_string_lossy().into_owned();
                match crate::util::run_checked(
                    "cmd",
                    &["/d", "/c", "mklink", "/J", &link_arg, &target_arg],
                ) {
                    Ok(()) => true,
                    Err(error) => {
                        eprintln!("skip reparse fixture (junction creation failed): {error}");
                        false
                    }
                }
            };
            if symlink_created || junction_created {
                assert!(
                    validate_residual_tree_with_limits(&root, 20, Duration::from_secs(5))
                        .unwrap_err()
                        .contains("重解析点")
                );
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn residual_preflight_rejects_hard_links_before_recursive_acl_changes() {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "aerotune-residual-hardlink-tree-{}-{suffix}",
            std::process::id()
        ));
        let outside = std::env::temp_dir().join(format!(
            "aerotune-residual-hardlink-target-{}-{suffix}.bin",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"linked target").unwrap();
        std::fs::hard_link(&outside, root.join("linked-file.bin")).unwrap();

        assert!(
            validate_residual_tree_with_limits(&root, 10, Duration::from_secs(5))
                .unwrap_err()
                .contains("硬链接")
        );
        assert_eq!(std::fs::read(&outside).unwrap(), b"linked target");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn residual_scan_marks_incomplete_roots_unsafe_and_hides_empty_items() {
        let safe = residual_scan_item(
            r"C:\Windows.old",
            "Windows.old",
            "test",
            Ok(ResidualTreeStats {
                entries: 4,
                files: 2,
                bytes: 8,
            }),
        )
        .unwrap();
        assert!(safe.safe_to_delete);
        assert_eq!(safe.size_mb, Some(8.0 / 1024.0 / 1024.0));
        assert_eq!(safe.scan_error, None);

        let unsafe_item = residual_scan_item(
            r"C:\ESD",
            "ESD",
            "test",
            Err("残留树包含重解析点；拒绝递归删除".into()),
        )
        .unwrap();
        assert!(!unsafe_item.safe_to_delete);
        assert_eq!(unsafe_item.size_mb, None);
        assert!(unsafe_item.scan_error.is_some());
        assert!(
            residual_scan_item("path", "label", "desc", Ok(ResidualTreeStats::default())).is_none()
        );
    }

    #[test]
    fn residual_scan_guard_is_single_flight_and_releases_on_drop() {
        let first = ResidualScanGuard::acquire().unwrap();
        assert!(ResidualScanGuard::acquire().is_err());
        drop(first);
        assert!(ResidualScanGuard::acquire().is_ok());
    }
}

// ---------- WinSxS 组件存储清理（DISM 官方手段） ----------

/// 标准清理：移除被取代的组件版本（耗时数分钟，期间请勿关机）
#[tauri::command]
pub async fn dism_component_cleanup() -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("组件存储清理需要管理员权限".into());
    }
    tracing::info!("WinSxS 组件清理开始（StartComponentCleanup）");
    tauri::async_runtime::spawn_blocking(|| {
        crate::util::run_checked_with_timeout(
            "Dism.exe",
            &["/Online", "/Cleanup-Image", "/StartComponentCleanup"],
            std::time::Duration::from_secs(2 * 60 * 60),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    crate::journal::record(
        "WinSxS 组件存储清理（DISM StartComponentCleanup）",
        crate::journal::Revert::None,
    )?;
    tracing::info!("WinSxS 组件清理完成");
    Ok(())
}

/// 深度清理：额外移除所有更新的备份版本（/ResetBase，更新将无法卸载）
#[tauri::command]
pub async fn dism_component_cleanup_reset() -> Result<(), String> {
    if !crate::util::is_admin() {
        return Err("组件存储清理需要管理员权限".into());
    }
    tracing::warn!("WinSxS 深度清理开始（/ResetBase，已装更新将不可卸载）");
    tauri::async_runtime::spawn_blocking(|| {
        crate::util::run_checked_with_timeout(
            "Dism.exe",
            &[
                "/Online",
                "/Cleanup-Image",
                "/StartComponentCleanup",
                "/ResetBase",
            ],
            std::time::Duration::from_secs(2 * 60 * 60),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    crate::journal::record(
        "WinSxS 深度清理（DISM /ResetBase —— 已装更新不可卸载）",
        crate::journal::Revert::None,
    )?;
    tracing::info!("WinSxS 深度清理完成");
    Ok(())
}
