use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::HashSet;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use winreg::{RegKey, RegValue};

const MAX_JOURNAL_BYTES: usize = 16 * 1024 * 1024;
const MAX_SAFE_JS_ID: u64 = 9_007_199_254_740_991;

/// 可还原的旧值
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "lowercase", tag = "k")]
pub enum OldVal {
    Dword(u32),
    Sz(String),
    Raw { bytes: Vec<u8>, vtype: u32 },
    Missing,
}

/// DNS settings are stored per address family; None means automatic/DHCP.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct DnsSettings {
    pub ipv4: Option<Vec<String>>,
    pub ipv6: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "lowercase", tag = "t")]
pub enum Revert {
    Registry {
        key: String,
        value: String,
        old: OldVal,
        #[serde(default)]
        applied: Option<OldVal>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nic_guid: Option<String>,
    },
    #[serde(rename = "power_plan", alias = "powerplan")]
    PowerPlan {
        guid: String,
        #[serde(default)]
        applied_guid: Option<String>,
    },
    Dns {
        iface: String,
        old: DnsSettings,
        applied: DnsSettings,
    },
    Tcp {
        level: String,
        #[serde(default)]
        applied_level: Option<String>,
    },
    Epp {
        old: Option<u32>,
        #[serde(default)]
        applied: Option<u32>,
    },
    None,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum EppWriteOutcome {
    Applied,
    AppliedWithWarning(String),
    Unchanged(String),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Change {
    pub id: u64,
    pub ts: u64,
    pub desc: String,
    #[serde(default)]
    pub group_id: Option<u64>,
    #[serde(default)]
    pub group_label: Option<String>,
    pub revert: Revert,
}

fn validate_change_identities(changes: &[Change]) -> Result<(), String> {
    let mut ids = HashSet::with_capacity(changes.len());
    for change in changes {
        if change.id == 0 || change.id > MAX_SAFE_JS_ID {
            return Err(format!("变更 ID {} 超出支持范围", change.id));
        }
        if !ids.insert(change.id) {
            return Err(format!(
                "发现重复变更 ID {}，无法确认唯一恢复目标",
                change.id
            ));
        }
        if change
            .group_id
            .is_some_and(|id| id == 0 || id > MAX_SAFE_JS_ID)
        {
            return Err("恢复组 ID 超出支持范围".into());
        }
        if change.ts > 8_640_000_000_000 {
            return Err("变更时间超出前端日期范围".into());
        }
    }
    Ok(())
}

fn find_unique_change_index(changes: &[Change], id: u64) -> Result<usize, String> {
    if id == 0 || id > MAX_SAFE_JS_ID {
        return Err("变更 ID 超出前端安全范围".into());
    }
    let mut matches = changes
        .iter()
        .enumerate()
        .filter(|(_, change)| change.id == id);
    let index = matches.next().map(|(index, _)| index).ok_or("变更不存在")?;
    if matches.next().is_some() {
        return Err("变更 ID 不唯一，无法安全确认恢复目标".into());
    }
    Ok(index)
}

fn find_group_change_index(changes: &[Change], group_id: u64, id: u64) -> Result<usize, String> {
    let mut matches = changes.iter().enumerate().filter(|(_, change)| {
        change.id == id
            && change.group_id == Some(group_id)
            && !matches!(change.revert, Revert::None)
    });
    let index = matches
        .next()
        .map(|(index, _)| index)
        .ok_or("恢复组成员不存在")?;
    if matches.next().is_some() {
        return Err("恢复组成员 ID 不唯一，停止组恢复".into());
    }
    Ok(index)
}

pub struct Journal {
    changes: Vec<Change>,
    path: PathBuf,
    next_id: u64,
    next_group_id: u64,
    storage_error: Option<String>,
}

#[derive(Clone)]
struct GroupContext {
    id: u64,
    label: String,
}

thread_local! {
    static ACTIVE_GROUP: RefCell<Option<GroupContext>> = const { RefCell::new(None) };
}

pub struct GroupGuard {
    id: u64,
    previous: Option<GroupContext>,
}

impl GroupGuard {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        let previous = self.previous.take();
        ACTIVE_GROUP.with(|active| *active.borrow_mut() = previous);
    }
}

/// Assign journal entries recorded synchronously on this worker thread to one durable group.
pub fn begin_change_group(label: &str) -> Result<GroupGuard, String> {
    let id = {
        let mut journal = JOURNAL.lock().unwrap();
        journal.refresh_if_repaired()?;
        journal.ensure_storage_healthy()?;
        let id = journal.next_group_id;
        if id == 0 || id > MAX_SAFE_JS_ID {
            return Err("恢复组标识超出安全范围；拒绝开始可撤销操作".into());
        }
        journal.next_group_id = id.saturating_add(1);
        id
    };
    let previous = ACTIVE_GROUP.with(|active| {
        active.borrow_mut().replace(GroupContext {
            id,
            label: label.chars().take(96).collect(),
        })
    });
    Ok(GroupGuard { id, previous })
}

impl Journal {
    fn load() -> Self {
        Self::load_from(crate::util::app_dir().join("changes.json"))
    }

    fn load_from(path: PathBuf) -> Self {
        let stored = (|| -> Result<Option<Vec<u8>>, String> {
            let file = match std::fs::File::open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    let parent = path
                        .parent()
                        .ok_or_else(|| "变更日志路径没有可验证的父目录".to_string())?;
                    return match std::fs::metadata(parent) {
                        Ok(metadata) if metadata.is_dir() => Ok(None),
                        Ok(_) => Err("变更日志父路径不是目录；拒绝将历史视为空".into()),
                        Err(parent_error) => Err(format!(
                            "无法验证变更日志目录；拒绝将历史视为空：{parent_error}"
                        )),
                    };
                }
                Err(error) => return Err(format!("无法读取现有变更日志：{error}")),
            };
            let mut bytes = Vec::new();
            file.take(MAX_JOURNAL_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("无法读取现有变更日志：{error}"))?;
            if bytes.len() > MAX_JOURNAL_BYTES {
                return Err(format!(
                    "现有变更日志超过 {} MiB 上限；拒绝将其视为空日志",
                    MAX_JOURNAL_BYTES / (1024 * 1024)
                ));
            }
            Ok(Some(bytes))
        })();
        let (changes, storage_error) = match stored {
            Ok(None) => (Vec::new(), None),
            Ok(Some(bytes)) => match serde_json::from_slice::<Vec<Change>>(&bytes) {
                Ok(changes) => match validate_change_identities(&changes) {
                    Ok(()) => (changes, None),
                    Err(error) => (
                        Vec::new(),
                        Some(format!("现有变更日志标识无效；原文件已保留：{error}")),
                    ),
                },
                Err(error) => (
                    Vec::new(),
                    Some(format!("现有变更日志格式损坏；原文件已保留：{error}")),
                ),
            },
            Err(error) => (Vec::new(), Some(error)),
        };
        if let Some(error) = &storage_error {
            tracing::error!(%error, "变更日志不可用；拒绝覆盖原文件或应用可撤销修改");
        }
        let next_id = changes
            .iter()
            .map(|change| change.id.saturating_add(1))
            .max()
            .unwrap_or(1);
        let next_group_id = changes
            .iter()
            .filter_map(|change| change.group_id)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Self {
            changes,
            path,
            next_id,
            next_group_id,
            storage_error,
        }
    }

    fn ensure_storage_healthy(&self) -> Result<(), String> {
        match &self.storage_error {
            Some(error) => Err(format!("变更日志不可用，拒绝覆盖现有历史：{error}")),
            None => Ok(()),
        }
    }

    fn refresh_if_repaired(&mut self) -> Result<(), String> {
        if self.storage_error.is_none() {
            return Ok(());
        }
        if !std::fs::metadata(&self.path).is_ok_and(|metadata| metadata.is_file()) {
            return self.ensure_storage_healthy();
        }
        let refreshed = Self::load_from(self.path.clone());
        if refreshed.storage_error.is_some() {
            self.storage_error = refreshed.storage_error;
            return self.ensure_storage_healthy();
        }
        let next_group_id = self.next_group_id;
        *self = refreshed;
        self.next_group_id = self.next_group_id.max(next_group_id);
        Ok(())
    }

    fn save(&self) -> Result<(), String> {
        self.ensure_storage_healthy()?;
        let b = serde_json::to_vec_pretty(&self.changes).map_err(|e| e.to_string())?;
        if b.len() > MAX_JOURNAL_BYTES {
            return Err(format!(
                "变更日志超过 {} MiB 安全上限；拒绝保存以避免下次启动丢失历史",
                MAX_JOURNAL_BYTES / (1024 * 1024)
            ));
        }
        crate::util::atomic_write(&self.path, &b).map_err(|e| format!("日志持久化失败: {e}"))
    }

    fn push(&mut self, desc: &str, revert: Revert) -> Result<u64, String> {
        let id = self.next_id;
        if id == 0 || id > MAX_SAFE_JS_ID {
            return Err("变更日志 ID 已超出前端安全范围；拒绝执行可撤销修改".into());
        }
        if self.changes.iter().any(|change| change.id == id) {
            return Err("变更日志 ID 冲突；拒绝执行可撤销修改".into());
        }
        let group = ACTIVE_GROUP.with(|active| active.borrow().clone());
        if group
            .as_ref()
            .is_some_and(|group| group.id == 0 || group.id > MAX_SAFE_JS_ID)
        {
            return Err("恢复组 ID 已超出前端安全范围；拒绝执行可撤销修改".into());
        }
        let next_id = id + 1;
        self.changes.push(Change {
            id,
            ts: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            desc: desc.to_string(),
            group_id: group.as_ref().map(|context| context.id),
            group_label: group.map(|context| context.label),
            revert,
        });
        self.next_id = next_id;
        if let Err(error) = self.save() {
            self.changes.pop();
            self.next_id = id;
            tracing::error!(%error, "变更日志保存失败；拒绝报告变更已记录");
            return Err(error);
        }
        tracing::info!(
            id,
            desc,
            reversible = !matches!(
                self.changes.last().map(|change| &change.revert),
                Some(Revert::None)
            ),
            "变更日志已持久化"
        );
        Ok(id)
    }

    fn remove(&mut self, id: u64) -> Result<(), String> {
        let index = self
            .changes
            .iter()
            .position(|change| change.id == id)
            .ok_or_else(|| "待移除的变更记录不存在".to_string())?;
        let removed = self.changes.remove(index);
        if let Err(error) = self.save() {
            self.changes.insert(index, removed);
            return Err(error);
        }
        Ok(())
    }
}

pub static JOURNAL: LazyLock<Mutex<Journal>> = LazyLock::new(|| Mutex::new(Journal::load()));

fn record_save_failure(
    revert: Revert,
    save_error: String,
    rollback: impl FnOnce(Revert) -> Result<(), String>,
) -> Result<(), String> {
    if matches!(&revert, Revert::None) {
        return Err(format!(
            "操作可能已完成，但变更日志无法持久化且此操作不可通过日志撤销：{save_error}"
        ));
    }
    match rollback(revert) {
        Ok(()) => Err(format!(
            "变更日志无法持久化：{save_error}；已自动还原此次可逆修改"
        )),
        Err(rollback_error) => Err(format!(
            "变更日志无法持久化：{save_error}；自动还原失败：{rollback_error}。请立即检查系统状态"
        )),
    }
}

/// Persist a change after an external mutation; revert it if durable logging fails.
pub fn record(desc: &str, revert: Revert) -> Result<(), String> {
    match JOURNAL.lock().unwrap().push(desc, revert.clone()) {
        Ok(_) => Ok(()),
        Err(error) => record_save_failure(revert, error, do_revert),
    }
}

fn split_hive(path: &str) -> Result<(winreg::HKEY, &str), String> {
    const PREFIXES: [(&str, winreg::HKEY); 4] = [
        ("HKEY_CURRENT_USER\\", HKEY_CURRENT_USER),
        ("HKCU\\", HKEY_CURRENT_USER),
        ("HKEY_LOCAL_MACHINE\\", HKEY_LOCAL_MACHINE),
        ("HKLM\\", HKEY_LOCAL_MACHINE),
    ];
    for (prefix, hive) in PREFIXES {
        if path
            .get(..prefix.len())
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
        {
            let rest = &path[prefix.len()..];
            if rest.is_empty() {
                return Err(format!("注册表路径缺少子键: {path}"));
            }
            return Ok((hive, rest));
        }
    }
    Err(format!("不支持的注册表路径: {path}"))
}

fn snapshot_dword_state(snapshot: &OldVal) -> Option<Option<u32>> {
    match snapshot {
        OldVal::Missing => Some(None),
        OldVal::Dword(value) => Some(Some(*value)),
        OldVal::Raw { bytes, vtype: 4 } if bytes.len() == 4 => {
            Some(Some(u32::from_le_bytes(bytes.as_slice().try_into().ok()?)))
        }
        _ => None,
    }
}

fn state_matches(state: Option<u32>, allowed: &[u32], allow_missing: bool) -> bool {
    match state {
        Some(value) => allowed.contains(&value),
        None => allow_missing,
    }
}

fn is_braced_guid(value: &str) -> bool {
    let Some(compact) = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
    else {
        return false;
    };
    let parts = compact.split('-').collect::<Vec<_>>();
    parts.len() == 5
        && [8, 4, 4, 4, 12].iter().zip(parts).all(|(expected, part)| {
            part.len() == *expected && part.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

/// User-writable history may carry forged snapshots. Machine hives are therefore restorable only
/// for AeroTune-owned targets with the expected value names, DWORD representation and value range.
fn allowed_hklm_registry_revert(
    key_path: &str,
    value_name: &str,
    old: &OldVal,
    applied: Option<&OldVal>,
) -> bool {
    let Ok((hive, subkey)) = split_hive(key_path) else {
        return false;
    };
    if hive != HKEY_LOCAL_MACHINE {
        return true;
    }
    let (Some(old), Some(applied)) = (
        snapshot_dword_state(old),
        applied.and_then(snapshot_dword_state),
    ) else {
        return false;
    };
    if subkey.starts_with('\\') || subkey.ends_with('\\') || subkey.contains("\\\\") {
        return false;
    }
    let key = subkey.to_ascii_lowercase();
    let value = value_name.to_ascii_lowercase();

    match (key.as_str(), value.as_str()) {
        ("system\\currentcontrolset\\control\\graphicsdrivers", "hwschmode") => {
            state_matches(old, &[1, 2], true) && state_matches(applied, &[1, 2], false)
        }
        ("system\\currentcontrolset\\control\\prioritycontrol", "win32priorityseparation") => {
            let allowed = [2, 0x18, 0x26, 0x28, 0x2a];
            state_matches(old, &allowed, true) && state_matches(applied, &allowed, false)
        }
        ("software\\policies\\microsoft\\windows\\windowsupdate\\au", "noautoupdate") => {
            state_matches(old, &[0, 1], true) && state_matches(applied, &[1], true)
        }
        ("software\\policies\\microsoft\\windows\\datacollection", "allowtelemetry") => {
            state_matches(old, &[0, 1, 2, 3], true) && state_matches(applied, &[0], false)
        }
        ("system\\currentcontrolset\\services\\diagtrack", "start") => {
            state_matches(old, &[2, 3, 4], true) && state_matches(applied, &[4], false)
        }
        _ => {
            let service_prefix = "system\\currentcontrolset\\services\\";
            if let Some(service_name) = key.strip_prefix(service_prefix) {
                if !service_name.contains('\\') {
                    if !crate::services::service_restore_target_allowed(service_name) {
                        return false;
                    }
                    return match value.as_str() {
                        "start" => {
                            state_matches(old, &[2, 3, 4], true)
                                && state_matches(applied, &[2, 3, 4], false)
                        }
                        "delayedautostart" => {
                            state_matches(old, &[0, 1], true) && state_matches(applied, &[1], true)
                        }
                        _ => false,
                    };
                }
            }

            let nic_prefix = "system\\currentcontrolset\\control\\class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\";
            if let Some(instance) = key.strip_prefix(nic_prefix) {
                return value == "pnpcapabilities"
                    && instance.len() == 4
                    && instance.chars().all(|ch| ch.is_ascii_digit())
                    && state_matches(old, &[0, 24], true)
                    && state_matches(applied, &[24], true);
            }

            let tcp_prefix = "system\\currentcontrolset\\services\\tcpip\\parameters\\interfaces\\";
            if let Some(guid) = key.strip_prefix(tcp_prefix) {
                return is_braced_guid(guid)
                    && matches!(value.as_str(), "tcpackfrequency" | "tcpnodelay")
                    && state_matches(old, &[0, 1, 2], true)
                    && state_matches(applied, &[1], true);
            }
            false
        }
    }
}

fn open_key(path: &str, write: bool) -> Result<RegKey, String> {
    let (root, sub) = split_hive(path)?;
    let access = if write {
        winreg::enums::KEY_READ | winreg::enums::KEY_WRITE
    } else {
        winreg::enums::KEY_READ
    };
    match RegKey::predef(root).open_subkey_with_flags(sub, access) {
        Ok(k) => Ok(k),
        Err(e) if write => RegKey::predef(root)
            .create_subkey(sub)
            .map(|(k, _)| k)
            .map_err(|e2| format!("打开/创建注册表项失败 {path}: {e} / {e2}")),
        Err(e) => Err(format!("打开注册表项失败 {path}: {e}")),
    }
}

fn open_key_existing_write(path: &str) -> Result<RegKey, String> {
    let (root, sub) = split_hive(path)?;
    RegKey::predef(root)
        .open_subkey_with_flags(sub, winreg::enums::KEY_READ | winreg::enums::KEY_WRITE)
        .map_err(|e| format!("打开既有注册表项失败 {path}: {e}"))
}

fn machine_service_registry_key_name(path: &str) -> Option<String> {
    let (hive, subkey) = split_hive(path).ok()?;
    if hive != HKEY_LOCAL_MACHINE {
        return None;
    }
    let normalized = subkey.to_ascii_lowercase();
    let service_name = normalized.strip_prefix("system\\currentcontrolset\\services\\")?;
    (!service_name.is_empty() && !service_name.contains('\\')).then(|| service_name.to_string())
}

fn open_registry_key_for_write(path: &str) -> Result<RegKey, String> {
    if let Some(service_name) = machine_service_registry_key_name(path) {
        // DiagTrack is separately constrained by its explicit compiled registry policy; all
        // dynamic service targets get a final SCM/account/image/ACL recheck after journal I/O.
        if !service_name.eq_ignore_ascii_case("DiagTrack")
            && !crate::services::service_restore_target_allowed(&service_name)
        {
            return Err("服务账户、镜像路径或 ACL 在注册表写入前未通过复核；拒绝修改或恢复".into());
        }
        // A concurrent service uninstall must not turn a restore into an orphan service key.
        open_key_existing_write(path)
    } else {
        open_key(path, true)
    }
}

fn read_old(path: &str, value: &str) -> Result<OldVal, String> {
    let (root, sub) = split_hive(path)?;
    let key = match RegKey::predef(root).open_subkey(sub) {
        Ok(k) => k,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(OldVal::Missing),
        Err(e) => return Err(format!("读取旧值失败: {e}")),
    };
    match key.get_raw_value(value) {
        // 保留原始 UTF-16LE 字节和类型，包括 REG_EXPAND_SZ；不做有损解码。
        Ok(RegValue { bytes, vtype }) => Ok(OldVal::Raw {
            bytes,
            vtype: vtype as u32,
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OldVal::Missing),
        Err(e) => Err(format!("读取旧值失败: {e}")),
    }
}

fn applied_dword(value: u32) -> OldVal {
    OldVal::Raw {
        bytes: value.to_le_bytes().to_vec(),
        vtype: 4,
    }
}

fn applied_sz(value: &str) -> OldVal {
    let mut bytes = value
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect::<Vec<_>>();
    bytes.extend_from_slice(&[0, 0]);
    OldVal::Raw { bytes, vtype: 1 }
}

fn write_ahead_registry_change_with_journal(
    journal: &mut Journal,
    desc: &str,
    revert: Revert,
    old: OldVal,
    applied: OldVal,
    apply: impl FnOnce() -> Result<(), String>,
    mut read_current: impl FnMut() -> Result<OldVal, String>,
) -> Result<(), String> {
    let id = journal.push(desc, revert)?;
    match read_current() {
        Ok(current) if current == old => {}
        Ok(_) => {
            return match journal.remove(id) {
                Ok(()) => Err("注册表值在写前日志准备期间已变化；未执行修改".into()),
                Err(error) => Err(format!(
                    "注册表值已变化；未执行修改，但写前恢复记录未能清除：{error}"
                )),
            };
        }
        Err(error) => {
            return match journal.remove(id) {
                Ok(()) => Err(format!("无法复核注册表旧值；未执行修改：{error}")),
                Err(cleanup_error) => Err(format!(
                    "无法复核注册表旧值；未执行修改，恢复记录未能清除：{error}；{cleanup_error}"
                )),
            };
        }
    }
    match apply() {
        Ok(()) => Ok(()),
        Err(write_error) => match read_current() {
            Ok(current) if current == old => match journal.remove(id) {
                Ok(()) => Err(format!("注册表写入失败，已确认值保持不变：{write_error}")),
                Err(cleanup_error) => Err(format!(
                    "注册表写入失败，值保持不变但恢复记录未能清除：{write_error}；{cleanup_error}"
                )),
            },
            Ok(current) if current == applied => Err(format!(
                "注册表写入报告失败但目标值已出现；恢复记录已保留，请检查状态：{write_error}"
            )),
            Ok(_) => Err(format!(
                "注册表写入失败且当前值与旧值/目标值均不匹配；恢复记录已保留：{write_error}"
            )),
            Err(read_error) => Err(format!(
                "注册表写入失败且无法确认当前值；恢复记录已保留：{write_error}；读取失败：{read_error}"
            )),
        },
    }
}

fn remove_intent_or_report(journal: &mut Journal, id: u64, reason: String) -> String {
    match journal.remove(id) {
        Ok(()) => reason,
        Err(error) => format!("{reason}；日志清理失败且记录仍保留：{error}"),
    }
}

fn write_ahead_epp_change_with_journal(
    journal: &mut Journal,
    desc: &str,
    old: u32,
    applied: u32,
    apply: impl FnOnce() -> Result<(), String>,
    mut read_current: impl FnMut() -> Result<Option<u32>, String>,
) -> Result<EppWriteOutcome, String> {
    if !crate::power::epp_value_valid(old) || !crate::power::epp_value_valid(applied) {
        return Err("EPP 快照超出 0-100 范围；未执行设置".into());
    }
    if old == applied {
        return match read_current() {
            Ok(Some(current)) if current == old => {
                Ok(EppWriteOutcome::Unchanged("EPP 已处于目标值".into()))
            }
            Ok(Some(current)) => Err(format!(
                "EPP 在设置前已变化（当前 {current}，预期 {old}）；未执行设置"
            )),
            Ok(None) => Err("无法读取 EPP 当前值；未执行设置".into()),
            Err(error) => Err(format!("无法复核 EPP 当前值；未执行设置：{error}")),
        };
    }

    let id = journal.push(
        desc,
        Revert::Epp {
            old: Some(old),
            applied: Some(applied),
        },
    )?;
    match read_current() {
        Ok(Some(current)) if current == old => {}
        Ok(Some(current)) => {
            return Err(remove_intent_or_report(
                journal,
                id,
                format!("EPP 在写前日志准备期间已变化（当前 {current}，预期 {old}）；未执行设置"),
            ));
        }
        Ok(None) => {
            return Err(remove_intent_or_report(
                journal,
                id,
                "无法读取 EPP 当前值；未执行设置".into(),
            ));
        }
        Err(error) => {
            return Err(remove_intent_or_report(
                journal,
                id,
                format!("无法复核 EPP 当前值；未执行设置：{error}"),
            ));
        }
    }

    match apply() {
        Ok(()) => match read_current() {
            Ok(Some(current)) if current == applied => Ok(EppWriteOutcome::Applied),
            Ok(Some(current)) if current == old => match journal.remove(id) {
                Ok(()) => Ok(EppWriteOutcome::Unchanged(
                    "EPP 命令已返回，但目标值未生效；已确认原值保持不变".into(),
                )),
                Err(error) => Err(format!(
                    "EPP 目标值未生效且原值保持不变，但恢复记录未能清除：{error}"
                )),
            },
            Ok(Some(current)) => Err(format!(
                "EPP 设置后状态既非原值也非目标值（当前 {current}）；恢复记录已保留"
            )),
            Ok(None) => Err("EPP 设置后无法读取当前值；恢复记录已保留".into()),
            Err(error) => Err(format!(
                "EPP 设置后无法复核当前值；恢复记录已保留：{error}"
            )),
        },
        Err(apply_error) => match read_current() {
            Ok(Some(current)) if current == applied => Ok(EppWriteOutcome::AppliedWithWarning(
                format!("EPP 目标值已读回，但设置命令返回错误：{apply_error}"),
            )),
            Ok(Some(current)) if current == old => match journal.remove(id) {
                Ok(()) => Ok(EppWriteOutcome::Unchanged(format!(
                    "EPP 设置失败且已确认原值保持不变：{apply_error}"
                ))),
                Err(error) => Err(format!(
                    "EPP 设置失败且原值保持不变，但恢复记录未能清除：{apply_error}；{error}"
                )),
            },
            Ok(Some(current)) => Err(format!(
                "EPP 设置失败且当前值与原值/目标值均不匹配（当前 {current}）；恢复记录已保留：{apply_error}"
            )),
            Ok(None) => Err(format!(
                "EPP 设置失败且无法读取当前值；恢复记录已保留：{apply_error}"
            )),
            Err(read_error) => Err(format!(
                "EPP 设置失败且无法复核当前值；恢复记录已保留：{apply_error}；读取失败：{read_error}"
            )),
        },
    }
}

pub(crate) fn set_epp_with_journal(
    desc: &str,
    old: u32,
    applied: u32,
    apply: impl FnOnce() -> Result<(), String>,
    read_current: impl FnMut() -> Result<Option<u32>, String>,
) -> Result<EppWriteOutcome, String> {
    let mut journal = JOURNAL.lock().unwrap();
    journal.refresh_if_repaired()?;
    journal.ensure_storage_healthy()?;
    write_ahead_epp_change_with_journal(&mut journal, desc, old, applied, apply, read_current)
}

fn write_ahead_power_plan_change_with_journal(
    journal: &mut Journal,
    desc: &str,
    old_guid: &str,
    applied_guid: &str,
    apply: impl FnOnce() -> Result<(), String>,
    mut read_current: impl FnMut() -> Result<String, String>,
) -> Result<(), String> {
    let id = journal.push(
        desc,
        Revert::PowerPlan {
            guid: old_guid.to_string(),
            applied_guid: Some(applied_guid.to_string()),
        },
    )?;
    match read_current() {
        Ok(current) if current.eq_ignore_ascii_case(old_guid) => {}
        Ok(_) => {
            return Err(remove_intent_or_report(
                journal,
                id,
                "活动电源计划在写前日志准备期间已变化；未执行切换".into(),
            ));
        }
        Err(error) => {
            return Err(remove_intent_or_report(
                journal,
                id,
                format!("无法复核当前电源计划；未执行切换：{error}"),
            ));
        }
    }

    match apply() {
        Ok(()) => match read_current() {
            Ok(current) if current.eq_ignore_ascii_case(applied_guid) => Ok(()),
            Ok(current) if current.eq_ignore_ascii_case(old_guid) => Err(remove_intent_or_report(
                journal,
                id,
                "电源计划命令报告成功但活动计划未变化；已保留原状态".into(),
            )),
            Ok(_) => Err("电源计划命令报告成功但活动计划变为其他值；撤销记录已保留".into()),
            Err(error) => Err(format!(
                "电源计划切换后无法确认活动计划；撤销记录已保留：{error}"
            )),
        },
        Err(apply_error) => match read_current() {
            Ok(current) if current.eq_ignore_ascii_case(old_guid) => Err(remove_intent_or_report(
                journal,
                id,
                format!("电源计划切换失败，已确认状态未改变：{apply_error}"),
            )),
            Ok(current) if current.eq_ignore_ascii_case(applied_guid) => Err(format!(
                "电源计划命令报告失败但目标计划已生效；撤销记录已保留：{apply_error}"
            )),
            Ok(_) => Err(format!(
                "电源计划切换失败且活动计划变为其他值；撤销记录已保留：{apply_error}"
            )),
            Err(read_error) => Err(format!(
                "电源计划切换失败且无法确认当前状态；撤销记录已保留：{apply_error}；{read_error}"
            )),
        },
    }
}

/// Persist a guarded active-plan revert before invoking powercfg.
pub fn change_power_plan_with_journal(
    old_guid: String,
    applied_guid: String,
    desc: &str,
    apply: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !crate::power::is_valid_power_plan_guid(&old_guid)
        || !crate::power::is_valid_power_plan_guid(&applied_guid)
    {
        return Err("电源计划 GUID 不合法；拒绝执行未记录的切换".into());
    }
    let mut journal = JOURNAL.lock().unwrap();
    if old_guid.eq_ignore_ascii_case(&applied_guid) {
        let current = crate::power::active_scheme_guid()
            .ok_or_else(|| "无法复核当前活动电源计划；拒绝确认无变更".to_string())?;
        if current.eq_ignore_ascii_case(&old_guid) {
            return Ok(());
        }
        return Err("活动电源计划已变化；未执行无变更请求".into());
    }
    write_ahead_power_plan_change_with_journal(
        &mut journal,
        desc,
        &old_guid,
        &applied_guid,
        apply,
        || {
            crate::power::active_scheme_guid()
                .ok_or_else(|| "无法读取当前活动电源计划 GUID".to_string())
        },
    )
}

fn write_ahead_registry_change(
    path: &str,
    value: &str,
    desc: &str,
    applied: OldVal,
    nic_guid: Option<String>,
    apply: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let mut journal = JOURNAL.lock().unwrap();
    let old = read_old(path, value)?;
    if old == applied {
        return Ok(());
    }
    if !allowed_hklm_registry_revert(path, value, &old, Some(&applied)) {
        return Err("机器级注册表目标或取值超出 AeroTune 编译策略；未执行修改".into());
    }
    validate_nic_registry_target(path, value, nic_guid.as_deref())?;
    let revert = Revert::Registry {
        key: path.to_string(),
        value: value.to_string(),
        old: old.clone(),
        applied: Some(applied.clone()),
        nic_guid,
    };
    write_ahead_registry_change_with_journal(
        &mut journal,
        desc,
        revert,
        old,
        applied,
        apply,
        || read_old(path, value),
    )
}

/// 写前先持久化撤销信息；日志无法写入时不会触碰注册表。
pub fn set_reg_dword(path: &str, value: &str, new: u32, desc: &str) -> Result<(), String> {
    let applied = applied_dword(new);
    write_ahead_registry_change(path, value, desc, applied, None, || {
        let key = open_registry_key_for_write(path)?;
        key.set_value(value, &new)
            .map_err(|e| format!("写入注册表失败 {path}\\{value}: {e}"))
    })
}

pub fn set_nic_reg_dword(
    path: &str,
    value: &str,
    new: u32,
    desc: &str,
    nic_guid: &str,
) -> Result<(), String> {
    let applied = applied_dword(new);
    write_ahead_registry_change(
        path,
        value,
        desc,
        applied,
        Some(nic_guid.to_string()),
        || {
            let key = open_key_existing_write(path)?;
            validate_open_nic_registry_key(&key, nic_guid)?;
            key.set_value(value, &new)
                .map_err(|e| format!("写入注册表失败 {path}\\{value}: {e}"))
        },
    )
}

/// 写前先持久化撤销信息；日志无法写入时不会触碰注册表。
pub fn set_reg_sz(path: &str, value: &str, new: &str, desc: &str) -> Result<(), String> {
    let applied = applied_sz(new);
    write_ahead_registry_change(path, value, desc, applied, None, || {
        let key = open_registry_key_for_write(path)?;
        key.set_value(value, &new)
            .map_err(|e| format!("写入注册表失败 {path}\\{value}: {e}"))
    })
}

pub fn read_reg_dword(path: &str, value: &str) -> Option<u32> {
    let key = open_key(path, false).ok()?;
    key.get_value(value).ok()
}

/// 删除前先持久化原值；日志无法写入时不执行删除。
pub fn del_reg_value(path: &str, value: &str, desc: &str) -> Result<(), String> {
    write_ahead_registry_change(path, value, desc, OldVal::Missing, None, || {
        let key = open_registry_key_for_write(path)?;
        key.delete_value(value)
            .map_err(|e| format!("删除注册表值失败 {path}\\{value}: {e}"))
    })
}

pub fn del_nic_reg_value(
    path: &str,
    value: &str,
    desc: &str,
    nic_guid: &str,
) -> Result<(), String> {
    write_ahead_registry_change(
        path,
        value,
        desc,
        OldVal::Missing,
        Some(nic_guid.to_string()),
        || {
            let key = open_key_existing_write(path)?;
            validate_open_nic_registry_key(&key, nic_guid)?;
            key.delete_value(value)
                .map_err(|e| format!("删除注册表值失败 {path}\\{value}: {e}"))
        },
    )
}

fn registry_revert_decision(
    current: &OldVal,
    old: &OldVal,
    applied: Option<&OldVal>,
) -> Result<bool, ()> {
    match applied {
        Some(applied) if current == applied => Ok(true),
        Some(_) if current == old => Ok(false),
        Some(_) => Err(()),
        None => Ok(true),
    }
}

fn nic_registry_class_instance_index(key_path: &str, value: &str) -> Option<String> {
    if !value.eq_ignore_ascii_case("PnPCapabilities") {
        return None;
    }
    let (hive, subkey) = split_hive(key_path).ok()?;
    if hive != HKEY_LOCAL_MACHINE {
        return None;
    }
    let prefix =
        "system\\currentcontrolset\\control\\class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\";
    let instance = subkey.to_ascii_lowercase();
    let instance = instance.strip_prefix(prefix)?;
    (instance.len() == 4 && instance.chars().all(|ch| ch.is_ascii_digit()))
        .then(|| instance.to_string())
}

fn validate_nic_registry_target_with(
    key_path: &str,
    value: &str,
    expected_guid: Option<&str>,
    key_exists: impl FnOnce(&str) -> bool,
    instance_matches_guid: impl FnOnce(&str, &str) -> bool,
) -> Result<(), String> {
    if let Some(instance) = nic_registry_class_instance_index(key_path, value) {
        let Some(expected_guid) = expected_guid else {
            return Err("网卡恢复记录缺少写入时的适配器 GUID；保留记录".into());
        };
        if !key_exists(key_path) {
            return Err("网卡注册目标已不存在；拒绝创建孤立配置并保留恢复记录".into());
        }
        if !instance_matches_guid(&instance, expected_guid) {
            return Err("网卡类实例已重绑定或不匹配记录中的适配器 GUID；保留恢复记录".into());
        }
    } else if expected_guid.is_some() {
        return Err("非网卡注册目标带有网卡身份信息；拒绝恢复".into());
    }
    Ok(())
}

fn validate_nic_registry_identity_with(
    stored_guid: &str,
    expected_guid: &str,
    guid_is_uniquely_enumerated: impl FnOnce(&str) -> bool,
) -> Result<(), String> {
    let expected_guid = crate::network::normalize_adapter_guid(expected_guid)
        .ok_or_else(|| "网卡恢复记录中的 GUID 无效；保留恢复记录".to_string())?;
    let stored_guid = crate::network::normalize_adapter_guid(stored_guid)
        .ok_or_else(|| "网卡注册实例的 NetCfgInstanceId 无效；保留恢复记录".to_string())?;
    if !stored_guid.eq_ignore_ascii_case(&expected_guid) {
        return Err("已打开的网卡注册实例与恢复记录中的适配器 GUID 不匹配；保留恢复记录".into());
    }
    if !guid_is_uniquely_enumerated(&expected_guid) {
        return Err("记录中的适配器 GUID 当前未唯一映射到已枚举网卡；保留恢复记录".into());
    }
    Ok(())
}

fn validate_open_nic_registry_key(key: &RegKey, expected_guid: &str) -> Result<(), String> {
    let stored_guid: String = key
        .get_value("NetCfgInstanceId")
        .map_err(|_| "网卡注册实例缺少 NetCfgInstanceId；保留恢复记录".to_string())?;
    validate_nic_registry_identity_with(
        &stored_guid,
        expected_guid,
        crate::network::nic_guid_is_uniquely_enumerated,
    )
}

fn validate_nic_registry_target(
    key_path: &str,
    value: &str,
    expected_guid: Option<&str>,
) -> Result<(), String> {
    validate_nic_registry_target_with(
        key_path,
        value,
        expected_guid,
        |path| open_key(path, false).is_ok(),
        |_instance, expected_guid| {
            open_key(key_path, false)
                .and_then(|key| validate_open_nic_registry_key(&key, expected_guid).map(|_| key))
                .is_ok()
        },
    )
}

fn revert_registry(
    key_path: &str,
    value: &str,
    old: &OldVal,
    applied: Option<&OldVal>,
    nic_guid: Option<&str>,
) -> Result<(), String> {
    if !allowed_hklm_registry_revert(key_path, value, old, applied) {
        return Err("机器级注册表恢复目标或快照不符合 AeroTune 编译授权策略；已保留记录".into());
    }
    if !registry_revert_decision(&read_old(key_path, value)?, old, applied)
        .map_err(|_| format!("{key_path}\\{value} 已被其他操作改动；保留恢复记录"))?
    {
        return Ok(());
    }
    let is_nic_target = nic_registry_class_instance_index(key_path, value).is_some();
    validate_nic_registry_target(key_path, value, nic_guid)?;
    let key = if is_nic_target {
        open_key_existing_write(key_path)?
    } else {
        open_registry_key_for_write(key_path)?
    };
    if is_nic_target {
        let Some(expected_guid) = nic_guid else {
            return Err("网卡恢复记录缺少写入时的适配器 GUID；保留记录".into());
        };
        validate_open_nic_registry_key(&key, expected_guid)?;
    }
    match old {
        OldVal::Dword(v) => key.set_value(value, v).map_err(|e| e.to_string()),
        OldVal::Sz(v) => key.set_value(value, v).map_err(|e| e.to_string()),
        OldVal::Raw { bytes, vtype } => {
            use winreg::enums::RegType;
            let t = match *vtype {
                0 => RegType::REG_NONE,
                1 => RegType::REG_SZ,
                2 => RegType::REG_EXPAND_SZ,
                3 => RegType::REG_BINARY,
                4 => RegType::REG_DWORD,
                5 => RegType::REG_DWORD_BIG_ENDIAN,
                6 => RegType::REG_LINK,
                7 => RegType::REG_MULTI_SZ,
                8 => RegType::REG_RESOURCE_LIST,
                9 => RegType::REG_FULL_RESOURCE_DESCRIPTOR,
                10 => RegType::REG_RESOURCE_REQUIREMENTS_LIST,
                11 => RegType::REG_QWORD,
                _ => return Err("不支持的注册表类型".into()),
            };
            key.set_raw_value(
                value,
                &RegValue {
                    bytes: bytes.clone(),
                    vtype: t,
                },
            )
            .map_err(|e| e.to_string())
        }
        OldVal::Missing => match key.delete_value(value) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        },
    }
}

#[tauri::command]
pub fn list_changes() -> Result<Vec<Change>, String> {
    let mut journal = JOURNAL.lock().unwrap();
    journal.refresh_if_repaired()?;
    Ok(journal.changes.clone())
}

#[tauri::command]
pub fn revert_change(id: u64) -> Result<(), String> {
    let mut j = JOURNAL.lock().unwrap();
    j.ensure_storage_healthy()?;
    let idx = find_unique_change_index(&j.changes, id)?;
    let removed = restore_at(&mut j.changes, idx, do_revert)?;
    if let Err(e) = j.save() {
        j.changes.insert(idx, removed);
        return Err(format!("系统已还原，但记录保存失败（记录保留）: {e}"));
    }
    Ok(())
}

#[tauri::command]
pub fn revert_change_group(group_id: u64) -> Result<usize, String> {
    if group_id == 0 || group_id > MAX_SAFE_JS_ID {
        return Err("恢复组 ID 超出前端安全范围".into());
    }
    let mut journal = JOURNAL.lock().unwrap();
    journal.ensure_storage_healthy()?;
    let ids = journal
        .changes
        .iter()
        .filter(|change| {
            change.group_id == Some(group_id) && !matches!(change.revert, Revert::None)
        })
        .map(|change| change.id)
        .collect::<Vec<_>>();
    if ids.is_empty() {
        return Err("该恢复组没有可还原的变更".into());
    }
    let mut restored = 0usize;
    for id in ids.into_iter().rev() {
        let index = find_group_change_index(&journal.changes, group_id, id)
            .map_err(|error| format!("已还原 {restored} 条；组内其余记录保留：{error}"))?;
        let removed = restore_at(&mut journal.changes, index, do_revert)
            .map_err(|error| format!("已还原 {restored} 条；组内其余记录保留: {error}"))?;
        if let Err(error) = journal.save() {
            journal.changes.insert(index, removed);
            return Err(format!(
                "已还原 {restored} 条；记录保存失败，已保留当前记录: {error}"
            ));
        }
        restored += 1;
    }
    Ok(restored)
}

#[tauri::command]
pub fn revert_all() -> Result<usize, String> {
    let mut j = JOURNAL.lock().unwrap();
    j.ensure_storage_healthy()?;
    let mut ok = 0;
    // 逆序还原；失败即停止，避免同一资源的更早快照覆盖失败的新快照。
    for idx in (0..j.changes.len()).rev() {
        if matches!(j.changes[idx].revert, Revert::None) {
            continue;
        }
        let removed = restore_at(&mut j.changes, idx, do_revert)
            .map_err(|e| format!("已还原 {ok} 条；失败及更早记录已保留: {e}"))?;
        if let Err(e) = j.save() {
            j.changes.insert(idx, removed);
            return Err(format!("系统已还原，但记录保存失败，停止后续恢复: {e}"));
        }
        ok += 1;
    }
    Ok(ok)
}

#[tauri::command]
pub fn clear_journal() -> Result<(), String> {
    let mut j = JOURNAL.lock().unwrap();
    j.ensure_storage_healthy()?;
    let previous = std::mem::take(&mut j.changes);
    if let Err(e) = j.save() {
        j.changes = previous;
        return Err(e);
    }
    Ok(())
}

fn restore_at(
    changes: &mut Vec<Change>,
    idx: usize,
    restore: impl FnOnce(Revert) -> Result<(), String>,
) -> Result<Change, String> {
    restore(changes[idx].revert.clone())?;
    Ok(changes.remove(idx))
}

/// Snapshots may come from persisted JSON. Only pass actual TCP levels to PowerShell,
/// and use a checked command so a failed restore retains its journal entry.
fn restore_tcp(
    level: &str,
    run: impl FnOnce(&str, &[&str]) -> Result<(), String>,
) -> Result<(), String> {
    let level = crate::network::normalize_tcp_autotuning_level(level)
        .ok_or_else(|| format!("TCP 旧级别不合法，保留恢复记录: {level}"))?;
    let script = format!(
        "$ErrorActionPreference='Stop'; Set-NetTCPSetting -SettingName Internet -AutoTuningLevelLocal {level} -ErrorAction Stop"
    );
    run(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", &script],
    )
    .map_err(|e| format!("TCP 还原失败（记录已保留）: {e}"))
}

fn restore_dns_exact(
    iface: &str,
    old: &DnsSettings,
    applied: &DnsSettings,
    read_current: impl FnMut(&str) -> Result<DnsSettings, String>,
    mut write: impl FnMut(&str, &DnsSettings) -> Result<(), String>,
) -> Result<(), String> {
    let mut read_current = read_current;
    let current = read_current(iface)
        .map_err(|error| format!("读取当前 DNS 状态失败，保留恢复记录: {error}"))?;
    if &current != applied {
        return Err("DNS 设置已被其他操作修改；保留恢复记录".into());
    }
    match write(iface, old) {
        Ok(()) => Ok(()),
        Err(restore_error) => {
            let after_failure = read_current(iface).map_err(|read_error| {
                format!("DNS 还原失败且无法安全读取失败后的状态；保留日志，请检查网络：{restore_error}；{read_error}")
            })?;
            if &after_failure == old {
                return Ok(());
            }
            if &after_failure == applied {
                return Err(format!(
                    "DNS 还原失败且当前设置未变化；保留恢复记录：{restore_error}"
                ));
            }

            // The writer applies Reset, then IPv4, then IPv6. This is state-based
            // compensation: only known intermediate states are eligible; all other
            // observed states are left untouched.
            let reset_state = DnsSettings::default();
            let after_ipv4 = DnsSettings {
                ipv4: old.ipv4.clone(),
                ipv6: None,
            };
            if after_failure != reset_state && after_failure != after_ipv4 {
                return Err(format!(
                    "DNS 还原失败后检测到非预期状态变化；为避免覆盖外部操作未执行回滚，保留日志：{restore_error}"
                ));
            }

            match write(iface, applied) {
                Ok(()) => match read_current(iface) {
                    Ok(restored) if &restored == applied => Err(format!(
                        "DNS 还原失败；已重新应用并验证恢复前设置，保留日志：{restore_error}"
                    )),
                    Ok(_) => Err(format!(
                        "DNS 还原失败；回滚报告成功但当前状态不匹配，保留日志：{restore_error}"
                    )),
                    Err(read_error) => Err(format!(
                        "DNS 还原失败；回滚后无法验证当前状态，保留日志：{restore_error}；{read_error}"
                    )),
                },
                Err(rollback_error) => Err(format!(
                    "DNS 还原失败且自动恢复当前设置也失败；网络状态可能已部分变化，请检查：{restore_error}；{rollback_error}"
                )),
            }
        }
    }
}

fn power_plan_restore_needed(
    current: &str,
    old_guid: &str,
    applied_guid: &str,
) -> Result<bool, String> {
    if current.eq_ignore_ascii_case(old_guid) {
        return Ok(false);
    }
    if current.eq_ignore_ascii_case(applied_guid) {
        return Ok(true);
    }
    Err("电源计划已被其他操作改动；保留恢复记录".into())
}

fn legacy_power_plan_undo_is_safe(current: &str, old_guid: &str) -> Result<(), String> {
    if current.eq_ignore_ascii_case(old_guid) {
        return Ok(());
    }
    Err(
        "旧版电源计划记录缺少应用 GUID，无法确认当前计划归属；为避免覆盖外部更改，保留恢复记录"
            .into(),
    )
}

fn do_revert(r: Revert) -> Result<(), String> {
    match r {
        Revert::Registry {
            key,
            value,
            old,
            applied,
            nic_guid,
        } => revert_registry(&key, &value, &old, applied.as_ref(), nic_guid.as_deref()),
        Revert::PowerPlan { guid, applied_guid } => {
            if let Some(applied) = applied_guid {
                let current = crate::power::active_scheme_guid()
                    .ok_or_else(|| "无法读取当前活动电源计划；保留恢复记录".to_string())?;
                if !power_plan_restore_needed(&current, &guid, &applied)? {
                    return Ok(());
                }
                crate::power::set_plan_raw_if_current(&guid, &applied)
            } else {
                let current = crate::power::active_scheme_guid()
                    .ok_or_else(|| "无法读取当前活动电源计划；保留恢复记录".to_string())?;
                legacy_power_plan_undo_is_safe(&current, &guid)
            }
        }
        Revert::Dns {
            iface,
            old,
            applied,
        } => restore_dns_exact(
            &iface,
            &old,
            &applied,
            crate::network::read_dns_settings,
            crate::network::apply_dns_settings,
        ),
        Revert::Tcp {
            level,
            applied_level,
        } => {
            if let Some(applied) = applied_level {
                if !crate::network::get_tcp_autotuning().eq_ignore_ascii_case(&applied) {
                    return Err("TCP 设置已被其他操作改动；保留恢复记录".into());
                }
            }
            restore_tcp(&level, crate::util::run_checked)
        }
        Revert::Epp { old: None, .. } => Err("旧版 EPP 记录缺少原始值，不能猜测恢复".into()),
        Revert::Epp {
            old: Some(old),
            applied,
        } => {
            if !crate::power::epp_value_valid(old)
                || applied.is_some_and(|value| !crate::power::epp_value_valid(value))
            {
                return Err("EPP 快照超出 0-100 范围；保留恢复记录".into());
            }
            if let Some(applied) = applied {
                if crate::power::get_epp_value() != Some(applied) {
                    return Err("EPP 已被其他操作改动；保留恢复记录".into());
                }
            }
            crate::power::set_epp_raw(Some(old)).map_err(|e| format!("EPP 还原失败: {e}"))?;
            if crate::power::get_epp_value() != Some(old) {
                return Err("EPP 还原命令已完成但原值未读回；保留恢复记录".into());
            }
            Ok(())
        }
        Revert::None => Err("此操作不支持日志撤销，请使用专用恢复入口（如有）".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_journal_path(label: &str) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir()
            .join(format!(
                "aerotune-journal-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
            .join("changes.json")
    }

    fn change(id: u64) -> Change {
        Change {
            id,
            ts: 0,
            desc: String::new(),
            group_id: None,
            group_label: None,
            revert: Revert::Tcp {
                level: id.to_string(),
                applied_level: None,
            },
        }
    }

    #[test]
    fn duplicate_ids_make_existing_journal_unhealthy_and_preserve_original_bytes() {
        let path = test_journal_path("duplicate-ids");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let original = serde_json::to_vec(&vec![change(42), change(42)]).unwrap();
        std::fs::write(&path, &original).unwrap();

        let mut journal = Journal::load_from(path.clone());
        assert!(journal
            .storage_error
            .as_deref()
            .is_some_and(|error| error.contains("重复变更 ID")));
        assert!(journal.changes.is_empty());
        assert!(journal.push("must not overwrite", Revert::None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn change_identities_reject_js_unsafe_record_and_group_ids() {
        let unsafe_id = MAX_SAFE_JS_ID + 1;
        assert!(validate_change_identities(&[change(unsafe_id)]).is_err());

        let mut unsafe_group = change(1);
        unsafe_group.group_id = Some(unsafe_id);
        assert!(validate_change_identities(&[unsafe_group]).is_err());

        let duplicate = vec![change(12), change(12)];
        assert!(validate_change_identities(&duplicate).is_err());
        assert!(find_unique_change_index(&duplicate, 12).is_err());
        assert!(find_unique_change_index(&[change(12)], unsafe_id).is_err());
    }

    #[test]
    fn group_lookup_cannot_escape_selected_group_on_duplicate_ids() {
        let mut other_group = change(77);
        other_group.group_id = Some(1);
        let mut selected_group = change(77);
        selected_group.group_id = Some(2);
        let changes = vec![other_group, selected_group];
        assert_eq!(find_group_change_index(&changes, 2, 77), Ok(1));
    }

    #[test]
    fn corrupt_journal_is_not_treated_as_empty_or_overwritten() {
        let path = test_journal_path("corrupt-file");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let original = b"{ invalid journal json";
        std::fs::write(&path, original).unwrap();

        let mut journal = Journal::load_from(path.clone());
        assert!(journal.storage_error.is_some());
        assert!(journal.ensure_storage_healthy().is_err());
        assert!(journal.push("must not overwrite", Revert::None).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(journal.changes.is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn retry_reloads_only_a_valid_restored_journal_file() {
        let path = test_journal_path("repaired-file");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(&path, b"{ invalid journal json").unwrap();
        let mut journal = Journal::load_from(path.clone());
        assert!(journal.storage_error.is_some());

        let restored = vec![change(42)];
        std::fs::write(&path, serde_json::to_vec(&restored).unwrap()).unwrap();
        journal.refresh_if_repaired().unwrap();
        assert!(journal.storage_error.is_none());
        assert_eq!(journal.changes.len(), 1);
        assert_eq!(journal.changes[0].id, 42);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn retry_does_not_treat_a_missing_corrupt_file_as_repaired() {
        let path = test_journal_path("removed-corrupt-file");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        std::fs::write(&path, b"{ invalid journal json").unwrap();
        let mut journal = Journal::load_from(path.clone());
        std::fs::remove_file(&path).unwrap();

        assert!(journal.refresh_if_repaired().is_err());
        assert!(journal.storage_error.is_some());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn journal_path_with_unreadable_parent_fails_closed() {
        let path = test_journal_path("file-as-parent");
        let parent = path.parent().unwrap();
        let original = b"preserve parent object";
        std::fs::write(parent, original).unwrap();

        let mut journal = Journal::load_from(path.clone());
        assert!(journal.storage_error.is_some());
        assert!(journal.push("must not overwrite", Revert::None).is_err());
        assert_eq!(std::fs::read(parent).unwrap(), original);
        let _ = std::fs::remove_file(parent);
    }

    #[test]
    fn oversized_journal_is_bounded_and_preserved() {
        let path = test_journal_path("oversized-file");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let original = vec![b' '; MAX_JOURNAL_BYTES + 1];
        std::fs::write(&path, &original).unwrap();

        let mut journal = Journal::load_from(path.clone());
        assert!(journal
            .storage_error
            .as_deref()
            .is_some_and(|error| error.contains("超过")));
        assert!(journal.push("must not overwrite", Revert::None).is_err());
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            original.len() as u64
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn journal_does_not_save_an_entry_that_would_exceed_the_load_limit() {
        let path = test_journal_path("save-size-limit");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal {
            changes: Vec::new(),
            path: path.clone(),
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };

        assert!(journal
            .push(&"x".repeat(MAX_JOURNAL_BYTES), Revert::None)
            .is_err());
        assert!(journal.changes.is_empty());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_journal_save_does_not_publish_an_in_memory_record() {
        let path = test_journal_path("missing-parent");
        let directory = path.parent().unwrap();
        let _ = std::fs::remove_dir_all(directory);
        let mut journal = Journal {
            changes: Vec::new(),
            path,
            next_id: 7,
            next_group_id: 1,
            storage_error: None,
        };
        assert!(journal.push("must not apply", Revert::None).is_err());
        assert!(journal.changes.is_empty());
        assert_eq!(journal.next_id, 7);
    }

    #[test]
    fn power_plan_undo_treats_old_state_as_noop_and_rejects_third_party_state() {
        let old = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let applied = "a1841308-3541-4fab-bc81-f71556f20b4a";
        let external = "b1a8a2b7-1e6d-4c52-8de4-7c8fdf6c7b44";
        assert_eq!(power_plan_restore_needed(old, old, applied), Ok(false));
        assert_eq!(power_plan_restore_needed(applied, old, applied), Ok(true));
        assert!(power_plan_restore_needed(external, old, applied)
            .unwrap_err()
            .contains("其他操作"));
    }

    #[test]
    fn legacy_power_plan_undo_without_applied_guid_only_accepts_already_restored_state() {
        let old = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let current = "a1841308-3541-4fab-bc81-f71556f20b4a";
        assert!(legacy_power_plan_undo_is_safe(old, old).is_ok());
        assert!(legacy_power_plan_undo_is_safe(current, old)
            .unwrap_err()
            .contains("缺少应用 GUID"));
    }

    #[test]
    fn power_plan_write_ahead_intent_is_durable_before_switch_and_verifies_target() {
        let path = test_journal_path("power-plan-write-ahead");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal {
            changes: Vec::new(),
            path: path.clone(),
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };
        let old = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let applied = "a1841308-3541-4fab-bc81-f71556f20b4a";
        let current = std::cell::RefCell::new(old.to_string());
        let mut saw_durable_intent = false;
        write_ahead_power_plan_change_with_journal(
            &mut journal,
            "switch plan",
            old,
            applied,
            || {
                let persisted: Vec<Change> =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                assert!(matches!(
                    &persisted[0].revert,
                    Revert::PowerPlan {
                        guid,
                        applied_guid: Some(applied_guid),
                    } if guid == old && applied_guid == applied
                ));
                saw_durable_intent = true;
                *current.borrow_mut() = applied.to_string();
                Ok(())
            },
            || Ok(current.borrow().clone()),
        )
        .unwrap();
        assert!(saw_durable_intent);
        assert_eq!(journal.changes.len(), 1);
        assert_eq!(
            current.borrow().to_ascii_lowercase(),
            applied.to_ascii_lowercase()
        );
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn failed_power_plan_switch_cleans_unchanged_intent_but_keeps_ambiguous_target() {
        let path = test_journal_path("power-plan-failed-unchanged");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal {
            changes: Vec::new(),
            path: path.clone(),
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };
        let old = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let applied = "a1841308-3541-4fab-bc81-f71556f20b4a";
        let error = write_ahead_power_plan_change_with_journal(
            &mut journal,
            "switch plan",
            old,
            applied,
            || Err("injected powercfg failure".into()),
            || Ok(old.to_string()),
        )
        .unwrap_err();
        assert!(error.contains("已确认状态未改变"));
        assert!(journal.changes.is_empty());
        let _ = std::fs::remove_dir_all(directory);

        let path = test_journal_path("power-plan-failed-applied");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal {
            changes: Vec::new(),
            path: path.clone(),
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };
        let current = std::cell::RefCell::new(old.to_string());
        let error = write_ahead_power_plan_change_with_journal(
            &mut journal,
            "switch plan",
            old,
            applied,
            || {
                *current.borrow_mut() = applied.to_string();
                Err("injected ambiguous powercfg failure".into())
            },
            || Ok(current.borrow().clone()),
        )
        .unwrap_err();
        assert!(error.contains("目标计划已生效"));
        assert_eq!(journal.changes.len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn power_plan_write_ahead_refuses_switch_after_concurrent_plan_change() {
        let path = test_journal_path("power-plan-conflict");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal {
            changes: Vec::new(),
            path: path.clone(),
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };
        let old = "381b4222-f694-41f0-9685-ff5bb260df2e";
        let applied = "a1841308-3541-4fab-bc81-f71556f20b4a";
        let current = "b1a8a2b7-1e6d-4c52-8de4-7c8fdf6c7b44";
        let mut mutated = false;
        let error = write_ahead_power_plan_change_with_journal(
            &mut journal,
            "switch plan",
            old,
            applied,
            || {
                mutated = true;
                Ok(())
            },
            || Ok(current.to_string()),
        )
        .unwrap_err();
        assert!(error.contains("已变化"));
        assert!(!mutated);
        assert!(journal.changes.is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn registry_write_ahead_record_is_durable_before_mutation_and_removed_if_unapplied() {
        let path = test_journal_path("write-ahead");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal {
            changes: Vec::new(),
            path: path.clone(),
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };
        let old = applied_dword(1);
        let applied = applied_dword(2);
        let mut saw_durable_intent = false;
        let current_old = old.clone();
        let error = write_ahead_registry_change_with_journal(
            &mut journal,
            "test value",
            Revert::Registry {
                key: "HKCU\\Software\\AeroTuneTest".into(),
                value: "Value".into(),
                old: old.clone(),
                applied: Some(applied.clone()),
                nic_guid: None,
            },
            old,
            applied,
            || {
                let persisted: Vec<Change> =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                assert_eq!(persisted.len(), 1);
                saw_durable_intent = true;
                Err("injected registry write failure".into())
            },
            || Ok(current_old.clone()),
        )
        .unwrap_err();
        assert!(saw_durable_intent, "{error}");
        assert!(error.contains("已确认值保持不变"));
        assert!(journal.changes.is_empty());
        let persisted: Vec<Change> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(persisted.is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn registry_write_failure_with_applied_value_keeps_recovery_record() {
        let path = test_journal_path("ambiguous-write");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal {
            changes: Vec::new(),
            path: path.clone(),
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };
        let old = OldVal::Missing;
        let applied = applied_dword(2);
        let current_applied = applied.clone();
        let mut read_count = 0;
        let error = write_ahead_registry_change_with_journal(
            &mut journal,
            "ambiguous value",
            Revert::Registry {
                key: "HKCU\\Software\\AeroTuneTest".into(),
                value: "Value".into(),
                old,
                applied: Some(applied.clone()),
                nic_guid: None,
            },
            OldVal::Missing,
            applied,
            || Err("injected ambiguous write result".into()),
            || {
                read_count += 1;
                if read_count == 1 {
                    Ok(OldVal::Missing)
                } else {
                    Ok(current_applied.clone())
                }
            },
        )
        .unwrap_err();
        assert!(error.contains("恢复记录已保留"));
        assert_eq!(journal.changes.len(), 1);
        let persisted: Vec<Change> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted.len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn registry_write_ahead_refuses_mutation_if_intent_cannot_be_saved() {
        let path = test_journal_path("no-write-before-save");
        let directory = path.parent().unwrap();
        let _ = std::fs::remove_dir_all(directory);
        let mut journal = Journal {
            changes: Vec::new(),
            path,
            next_id: 1,
            next_group_id: 1,
            storage_error: None,
        };
        let mut mutated = false;
        let error = write_ahead_registry_change_with_journal(
            &mut journal,
            "must not run",
            Revert::Registry {
                key: "HKCU\\Software\\AeroTuneTest".into(),
                value: "Value".into(),
                old: OldVal::Missing,
                applied: Some(applied_dword(1)),
                nic_guid: None,
            },
            OldVal::Missing,
            applied_dword(1),
            || {
                mutated = true;
                Ok(())
            },
            || Ok(OldVal::Missing),
        )
        .unwrap_err();
        assert!(!mutated);
        assert!(journal.changes.is_empty());
        assert!(!error.is_empty());
    }

    #[test]
    fn registry_revert_distinguishes_applied_old_and_external_values() {
        let old = applied_dword(1);
        let applied = applied_dword(2);
        assert_eq!(
            registry_revert_decision(&applied, &old, Some(&applied)),
            Ok(true)
        );
        assert_eq!(
            registry_revert_decision(&old, &old, Some(&applied)),
            Ok(false)
        );
        assert!(registry_revert_decision(&OldVal::Dword(3), &old, Some(&applied)).is_err());
    }

    #[test]
    fn registry_hive_parser_handles_unicode_case_expansion_without_panicking() {
        let path = format!("HKLM\\{}", "İ".repeat(6));
        assert!(split_hive(&path).is_ok());
        assert!(!allowed_hklm_registry_revert(
            &path,
            "Start",
            &OldVal::Missing,
            Some(&applied_dword(2)),
        ));
    }

    #[test]
    fn machine_registry_undo_rejects_forged_paths_and_invalid_snapshot_types() {
        let old = OldVal::Missing;
        let applied = applied_dword(1);
        assert!(!allowed_hklm_registry_revert(
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run",
            "Start",
            &old,
            Some(&applied),
        ));
        assert!(!allowed_hklm_registry_revert(
            r"HKLM\Software\Policies\Microsoft\Windows\WindowsUpdate\AU",
            "NoAutoUpdate",
            &OldVal::Sz("untrusted".into()),
            Some(&applied),
        ));
        assert!(!allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "HwSchMode",
            &old,
            None,
        ));
        assert!(!allowed_hklm_registry_revert(
            r"HKLM\\SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "HwSchMode",
            &old,
            Some(&applied),
        ));
    }

    #[test]
    fn service_registry_write_targets_are_classified_for_existing_only_access() {
        assert_eq!(
            machine_service_registry_key_name(r"HKLM\SYSTEM\CurrentControlSet\Services\VendorSvc")
                .as_deref(),
            Some("vendorsvc")
        );
        assert_eq!(
            machine_service_registry_key_name(
                r"HKEY_LOCAL_MACHINE\SYSTEM\CurrentControlSet\SERVICES\VendorSvc"
            )
            .as_deref(),
            Some("vendorsvc")
        );
        assert!(machine_service_registry_key_name(
            r"HKLM\SYSTEM\CurrentControlSet\Services\VendorSvc\Parameters"
        )
        .is_none());
        assert!(machine_service_registry_key_name(
            r"HKCU\SYSTEM\CurrentControlSet\Services\VendorSvc"
        )
        .is_none());
        assert!(
            machine_service_registry_key_name(r"HKLM\SYSTEM\CurrentControlSet\Services").is_none()
        );
    }

    #[test]
    fn machine_registry_undo_accepts_only_aero_tune_targets_and_ranges() {
        assert!(allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "HwSchMode",
            &applied_dword(2),
            Some(&applied_dword(1)),
        ));
        assert!(allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\{01234567-89ab-cdef-0123-456789abcdef}",
            "TcpAckFrequency",
            &OldVal::Missing,
            Some(&applied_dword(1)),
        ));
        assert!(allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\0000",
            "PnPCapabilities",
            &applied_dword(0),
            Some(&applied_dword(24)),
        ));
        assert!(allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\0000",
            "PnPCapabilities",
            &applied_dword(24),
            Some(&OldVal::Missing),
        ));
        assert!(!allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\9999",
            "PnPCapabilities",
            &applied_dword(u32::MAX),
            Some(&OldVal::Missing),
        ));
        assert!(!allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "HwSchMode",
            &OldVal::Missing,
            Some(&applied_dword(99)),
        ));
        assert!(!allowed_hklm_registry_revert(
            r"HKLM\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\not-a-guid",
            "TcpAckFrequency",
            &OldVal::Missing,
            Some(&applied_dword(1)),
        ));
        assert!(allowed_hklm_registry_revert(
            r"HKCU\Software\UnrestrictedUserHive",
            "Value",
            &OldVal::Sz("user-owned".into()),
            None,
        ));
    }

    #[test]
    fn nic_undo_requires_an_existing_key_and_live_adapter_binding() {
        let target = r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\0000";
        assert_eq!(
            nic_registry_class_instance_index(target, "PnPCapabilities").as_deref(),
            Some("0000"),
        );
        assert!(nic_registry_class_instance_index(
            r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\99999",
            "PnPCapabilities",
        )
        .is_none());
        assert!(
            nic_registry_class_instance_index(r"HKCU\Software\AeroTune", "PnPCapabilities",)
                .is_none()
        );
        let expected_guid = "{01234567-89AB-CDEF-0123-456789ABCDEF}";
        assert!(validate_nic_registry_target_with(
            target,
            "PnPCapabilities",
            Some(expected_guid),
            |_| true,
            |index, guid| index == "0000" && guid == expected_guid,
        )
        .is_ok());
        assert!(validate_nic_registry_target_with(
            target,
            "PnPCapabilities",
            Some(expected_guid),
            |_| false,
            |_, _| panic!("adapter mapping must not be queried when its registry key is absent"),
        )
        .unwrap_err()
        .contains("拒绝创建孤立配置"));
        assert!(validate_nic_registry_target_with(
            target,
            "PnPCapabilities",
            Some(expected_guid),
            |_| true,
            |_, _| false,
        )
        .unwrap_err()
        .contains("不匹配记录中的适配器 GUID"));
        assert!(validate_nic_registry_target_with(
            target,
            "PnPCapabilities",
            None,
            |_| true,
            |_, _| true,
        )
        .unwrap_err()
        .contains("缺少写入时的适配器 GUID"));
        assert!(validate_nic_registry_target_with(
            r"HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "HwSchMode",
            Some(expected_guid),
            |_| true,
            |_, _| true,
        )
        .unwrap_err()
        .contains("非网卡注册目标带有网卡身份信息"));
        let mut enumerated_guid = None;
        assert!(validate_nic_registry_identity_with(
            "{01234567-89ab-cdef-0123-456789abcdef}",
            expected_guid,
            |guid| {
                enumerated_guid = Some(guid.to_string());
                guid == expected_guid
            },
        )
        .is_ok());
        assert_eq!(enumerated_guid.as_deref(), Some(expected_guid));
        assert!(validate_nic_registry_identity_with(
            "{aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa}",
            expected_guid,
            |_| panic!("adapter enumeration must not run for a handle GUID mismatch"),
        )
        .unwrap_err()
        .contains("不匹配"));
        assert!(
            validate_nic_registry_identity_with(expected_guid, expected_guid, |_| false,)
                .unwrap_err()
                .contains("未唯一映射")
        );
    }

    #[test]
    fn failed_record_persistence_compensates_reversible_mutations_and_reports_irreversible_ones() {
        let revert = Revert::Epp {
            old: Some(1),
            applied: Some(2),
        };
        let mut rolled_back = false;
        let error = record_save_failure(revert, "injected disk error".into(), |_| {
            rolled_back = true;
            Ok(())
        })
        .unwrap_err();
        assert!(rolled_back);
        assert!(error.contains("已自动还原"));

        let mut rollback_called = false;
        let error = record_save_failure(Revert::None, "injected disk error".into(), |_| {
            rollback_called = true;
            Ok(())
        })
        .unwrap_err();
        assert!(!rollback_called);
        assert!(error.contains("操作可能已完成"));
    }

    #[test]
    fn legacy_journal_entries_deserialize_without_group_metadata() {
        let raw = r#"{"id":1,"ts":10,"desc":"old record","revert":{"t":"tcp","level":"normal"}}"#;
        let change: Change = serde_json::from_str(raw).unwrap();
        assert_eq!(change.group_id, None);
        assert_eq!(change.group_label, None);
        assert!(matches!(
            change.revert,
            Revert::Tcp {
                applied_level: None,
                ..
            }
        ));
        let legacy_plan: Change = serde_json::from_str(
            r#"{"id":2,"ts":10,"desc":"legacy plan","revert":{"t":"powerplan","guid":"old-guid"}}"#,
        )
        .unwrap();
        assert!(matches!(
            legacy_plan.revert,
            Revert::PowerPlan {
                applied_guid: None,
                ..
            }
        ));
        let legacy_nic: Change = serde_json::from_str(
            r#"{"id":3,"ts":10,"desc":"legacy NIC","revert":{"t":"registry","key":"HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e972-e325-11ce-bfc1-08002be10318}\\0000","value":"PnPCapabilities","old":{"k":"missing"}}}"#,
        )
        .unwrap();
        assert!(matches!(
            legacy_nic.revert,
            Revert::Registry { nic_guid: None, .. }
        ));
        let nic_guid = "{01234567-89AB-CDEF-0123-456789ABCDEF}".to_string();
        let current_nic = Revert::Registry {
            key: r"HKLM\SYSTEM\CurrentControlSet\Control\Class\{4d36e972-e325-11ce-bfc1-08002be10318}\0000".into(),
            value: "PnPCapabilities".into(),
            old: OldVal::Missing,
            applied: None,
            nic_guid: Some(nic_guid.clone()),
        };
        let current_nic: Revert =
            serde_json::from_value(serde_json::to_value(current_nic).unwrap()).unwrap();
        assert!(
            matches!(current_nic, Revert::Registry { nic_guid: Some(value), .. } if value == nic_guid)
        );

        let current = serde_json::to_value(Revert::PowerPlan {
            guid: "old".into(),
            applied_guid: Some("new".into()),
        })
        .unwrap();
        assert_eq!(current["t"], "power_plan");
    }

    #[test]
    fn raw_applied_registry_values_match_windows_storage_encoding() {
        assert_eq!(
            applied_dword(0x12345678),
            OldVal::Raw {
                bytes: 0x12345678u32.to_le_bytes().to_vec(),
                vtype: 4
            }
        );
        assert_eq!(
            applied_sz("A"),
            OldVal::Raw {
                bytes: vec![b'A', 0, 0, 0],
                vtype: 1
            }
        );
    }

    #[test]
    fn failed_restore_keeps_record() {
        let mut c = vec![change(1)];
        assert!(restore_at(&mut c, 0, |_| Err("mock failure".into())).is_err());
        assert_eq!(c[0].id, 1);
        assert!(do_revert(Revert::None).is_err());
    }
    #[test]
    fn reverse_restore_stops_on_failure() {
        let mut c = vec![change(1), change(2), change(3)];
        let mut seen = vec![];
        for idx in (0..c.len()).rev() {
            if restore_at(&mut c, idx, |r| {
                let Revert::Tcp { level, .. } = r else {
                    unreachable!()
                };
                seen.push(level.clone());
                if level == "2" {
                    Err("mock failure".into())
                } else {
                    Ok(())
                }
            })
            .is_err()
            {
                break;
            }
        }
        assert_eq!(seen, vec!["3", "2"]);
        assert_eq!(c.iter().map(|x| x.id).collect::<Vec<_>>(), vec![1, 2]);
    }
    #[test]
    fn tcp_restore_preserves_record_on_command_failure() {
        let mut changes = vec![Change {
            id: 1,
            ts: 0,
            desc: String::new(),
            group_id: None,
            group_label: None,
            revert: Revert::Tcp {
                level: "Normal".into(),
                applied_level: None,
            },
        }];
        let err = restore_at(&mut changes, 0, |r| {
            let Revert::Tcp { level, .. } = r else {
                unreachable!()
            };
            restore_tcp(&level, |program, args| {
                assert_eq!(program, "powershell");
                assert_eq!(args[0..3], ["-NoProfile", "-NonInteractive", "-Command"]);
                assert!(args[3].contains("-AutoTuningLevelLocal normal"));
                assert!(args[3].ends_with("-ErrorAction Stop"));
                Err("access denied".into())
            })
        })
        .unwrap_err();
        assert!(err.contains("access denied"));
        assert_eq!(changes.len(), 1);
    }
    #[test]
    fn tcp_restore_rejects_unknown_or_injected_snapshot() {
        for level in ["unknown", "normal; Write-Output injected"] {
            assert!(restore_tcp(level, |_, _| panic!("must not invoke PowerShell")).is_err());
        }
    }
    #[test]
    fn dns_restore_supports_automatic_and_static_prior_states() {
        let old_auto = DnsSettings::default();
        let applied_static = DnsSettings {
            ipv4: Some(vec!["1.1.1.1".into(), "1.0.0.1".into()]),
            ipv6: None,
        };
        let mut wrote_auto = false;
        restore_dns_exact(
            "Ethernet",
            &old_auto,
            &applied_static,
            |_| Ok(applied_static.clone()),
            |iface, old| {
                assert_eq!(iface, "Ethernet");
                assert_eq!(old, &old_auto);
                wrote_auto = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(wrote_auto);

        let old_static = DnsSettings {
            ipv4: Some(vec!["9.9.9.9".into()]),
            ipv6: Some(vec!["2606:4700:4700::1111".into()]),
        };
        let applied_auto = DnsSettings::default();
        let mut restored_static = false;
        restore_dns_exact(
            "Wi-Fi",
            &old_static,
            &applied_auto,
            |_| Ok(applied_auto.clone()),
            |_, old| {
                assert_eq!(old, &old_static);
                restored_static = true;
                Ok(())
            },
        )
        .unwrap();
        assert!(restored_static);
    }

    #[test]
    fn dns_restore_preserves_record_when_external_state_differs() {
        let old = DnsSettings::default();
        let applied = DnsSettings {
            ipv4: Some(vec!["1.1.1.1".into()]),
            ipv6: None,
        };
        let external = DnsSettings {
            ipv4: Some(vec!["8.8.8.8".into()]),
            ipv6: None,
        };
        let mut wrote = false;
        let error = restore_dns_exact(
            "Ethernet",
            &old,
            &applied,
            |_| Ok(external.clone()),
            |_, _| {
                wrote = true;
                Ok(())
            },
        )
        .unwrap_err();
        assert!(error.contains("保留恢复记录"));
        assert!(!wrote);
    }

    #[test]
    fn dns_restore_compensates_known_partial_state_and_preserves_the_record() {
        let old = DnsSettings {
            ipv4: Some(vec!["9.9.9.9".into()]),
            ipv6: Some(vec!["2606:4700:4700::1000".into()]),
        };
        let applied = DnsSettings {
            ipv4: Some(vec!["1.1.1.1".into()]),
            ipv6: Some(vec!["2606:4700:4700::1111".into()]),
        };
        let partial = DnsSettings {
            ipv4: old.ipv4.clone(),
            ipv6: None,
        };
        let mut reads = 0;
        let mut writes = Vec::new();
        let error = restore_dns_exact(
            "Ethernet",
            &old,
            &applied,
            |_| {
                reads += 1;
                Ok(match reads {
                    1 => applied.clone(),
                    2 => partial.clone(),
                    _ => applied.clone(),
                })
            },
            |_, requested| {
                writes.push(requested.clone());
                if writes.len() == 1 {
                    Err("injected IPv6 write failure after partial mutation".into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(error.contains("已重新应用并验证恢复前设置"));
        assert_eq!(writes, vec![old, applied]);
        assert_eq!(reads, 3);
    }

    #[test]
    fn dns_restore_does_not_overwrite_unexpected_concurrent_changes() {
        let old = DnsSettings::default();
        let applied = DnsSettings {
            ipv4: Some(vec!["1.1.1.1".into()]),
            ipv6: Some(vec!["2606:4700:4700::1111".into()]),
        };
        let external = DnsSettings {
            ipv4: Some(vec!["8.8.8.8".into()]),
            ipv6: Some(vec!["2001:4860:4860::8888".into()]),
        };
        let mut reads = 0;
        let mut writes = 0;
        let error = restore_dns_exact(
            "Ethernet",
            &old,
            &applied,
            |_| {
                reads += 1;
                Ok(if reads == 1 {
                    applied.clone()
                } else {
                    external.clone()
                })
            },
            |_, _| {
                writes += 1;
                Err("primary write failed".into())
            },
        )
        .unwrap_err();
        assert!(error.contains("避免覆盖外部操作"));
        assert_eq!(writes, 1);
    }

    #[test]
    fn dns_restore_reports_failed_compensation_without_dropping_the_record() {
        let old = DnsSettings {
            ipv4: Some(vec!["9.9.9.9".into()]),
            ipv6: Some(vec!["2606:4700:4700::1000".into()]),
        };
        let applied = DnsSettings {
            ipv4: Some(vec!["1.1.1.1".into()]),
            ipv6: Some(vec!["2606:4700:4700::1111".into()]),
        };
        let partial = DnsSettings {
            ipv4: old.ipv4.clone(),
            ipv6: None,
        };
        let mut reads = 0;
        let mut attempts = 0;
        let error = restore_dns_exact(
            "Ethernet",
            &old,
            &applied,
            |_| {
                reads += 1;
                Ok(if reads == 1 {
                    applied.clone()
                } else {
                    partial.clone()
                })
            },
            |_, _| {
                attempts += 1;
                Err(if attempts == 1 {
                    "primary failure"
                } else {
                    "rollback failure"
                }
                .into())
            },
        )
        .unwrap_err();
        assert_eq!(attempts, 2);
        assert!(error.contains("网络状态可能已部分变化"));
        assert!(error.contains("primary failure"));
        assert!(error.contains("rollback failure"));
    }

    #[test]
    fn dns_revert_record_roundtrips_family_specific_settings() {
        let record = Revert::Dns {
            iface: "Ethernet".into(),
            old: DnsSettings {
                ipv4: None,
                ipv6: Some(vec!["2606:4700:4700::1111".into()]),
            },
            applied: DnsSettings {
                ipv4: Some(vec!["1.1.1.1".into()]),
                ipv6: Some(vec!["2606:4700:4700::1111".into()]),
            },
        };
        let value = serde_json::to_value(&record).unwrap();
        assert_eq!(value["t"], "dns");
        let restored: Revert = serde_json::from_value(value).unwrap();
        let Revert::Dns { old, applied, .. } = restored else {
            panic!("expected DNS revert record")
        };
        assert_eq!(old.ipv4, None);
        assert_eq!(applied.ipv4, Some(vec!["1.1.1.1".into()]));
    }

    #[test]
    fn utf16_raw_snapshot_roundtrips_without_type_loss() {
        let bytes: Vec<u8> = "中文 %APPDATA%\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let old = OldVal::Raw {
            bytes: bytes.clone(),
            vtype: 2,
        };
        let restored: OldVal = serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(matches!(restored, OldVal::Raw { bytes: b, vtype: 2 } if b == bytes));
    }

    #[test]
    fn epp_write_ahead_retains_intent_when_command_errors_after_target_was_written() {
        let path = test_journal_path("epp-partial-applied");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal::load_from(path.clone());
        let state = std::cell::Cell::new(Some(40u32));
        let outcome = write_ahead_epp_change_with_journal(
            &mut journal,
            "test EPP mode apply",
            40,
            25,
            || {
                state.set(Some(25));
                Err("injected /setactive failure after index write".into())
            },
            || Ok(state.get()),
        )
        .unwrap();
        assert!(
            matches!(outcome, EppWriteOutcome::AppliedWithWarning(warning) if warning.contains("/setactive failure"))
        );
        assert_eq!(journal.changes.len(), 1);
        assert!(matches!(
            &journal.changes[0].revert,
            Revert::Epp {
                old: Some(40),
                applied: Some(25)
            }
        ));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn epp_write_ahead_removes_intent_when_failure_is_verified_unchanged() {
        let path = test_journal_path("epp-unchanged");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal::load_from(path.clone());
        let state = std::cell::Cell::new(Some(40u32));
        let outcome = write_ahead_epp_change_with_journal(
            &mut journal,
            "test EPP mode apply",
            40,
            25,
            || Err("injected denied write".into()),
            || Ok(state.get()),
        )
        .unwrap();
        assert!(
            matches!(outcome, EppWriteOutcome::Unchanged(reason) if reason.contains("confirmed") || reason.contains("已确认"))
        );
        assert!(journal.changes.is_empty());
        assert_eq!(state.get(), Some(40));
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn epp_write_ahead_preserves_record_when_post_write_state_is_unreadable() {
        let path = test_journal_path("epp-readback-unknown");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal::load_from(path.clone());
        let reads = std::cell::Cell::new(0usize);
        let error = write_ahead_epp_change_with_journal(
            &mut journal,
            "test EPP mode apply",
            40,
            25,
            || Ok(()),
            || {
                let read = reads.get() + 1;
                reads.set(read);
                Ok(if read == 1 { Some(40) } else { None })
            },
        )
        .unwrap_err();
        assert!(error.contains("恢复记录已保留"));
        assert_eq!(journal.changes.len(), 1);
        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn epp_write_ahead_refuses_changed_preflight_value_without_mutating() {
        let path = test_journal_path("epp-preflight-conflict");
        let directory = path.parent().unwrap();
        std::fs::create_dir_all(directory).unwrap();
        let mut journal = Journal::load_from(path.clone());
        let write_called = std::cell::Cell::new(false);
        let error = write_ahead_epp_change_with_journal(
            &mut journal,
            "test EPP mode apply",
            40,
            25,
            || {
                write_called.set(true);
                Ok(())
            },
            || Ok(Some(55)),
        )
        .unwrap_err();
        assert!(error.contains("已变化"));
        assert!(!write_called.get());
        assert!(journal.changes.is_empty());
        let _ = std::fs::remove_dir_all(directory);
    }
}
