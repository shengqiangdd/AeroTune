// 场景识别影子观测：记录"确认 D3D 全屏但不在游戏名单"的前台进程会话，
// 跨天累积后给出加入名单的建议。纯观察——不加名单任何行为不变，
// 采纳与否始终由用户决定；数据只存本机 scene-shadow.json，不联网。
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SceneHit {
    pub total: u32,
    /// 本地日期（yyyy-mm-dd）→ 当日会话数。
    pub days: HashMap<String, u32>,
    pub last_unix: u64,
}

pub type SceneShadowData = HashMap<String, SceneHit>;

/// 建议门槛：累计会话数与跨天数同时达标才出现在建议列表。
pub const SUGGESTION_MIN_TOTAL: u32 = 3;
pub const SUGGESTION_MIN_DAYS: usize = 2;
/// 建议列表上限。
pub const SUGGESTION_MAX_ITEMS: usize = 5;
/// 观测条目上限；超出时按 last_unix 淘汰最旧。
const STORE_MAX_ENTRIES: usize = 64;
/// 30 天无新会话即淘汰。
const STALE_ENTRY_SECS: u64 = 30 * 24 * 3600;

#[derive(Serialize, Clone, Debug)]
pub struct SceneSuggestion {
    pub name: String,
    pub total_hits: u32,
    pub distinct_days: u32,
    pub last_seen_unix: u64,
}

pub struct SceneShadow {
    data: SceneShadowData,
    path: PathBuf,
}

impl SceneShadow {
    pub fn load() -> Self {
        Self::load_from(crate::util::app_dir().join("scene-shadow.json"))
    }

    pub fn load_from(path: PathBuf) -> Self {
        let data = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SceneShadowData>(&bytes).ok())
            .unwrap_or_default();
        let mut store = Self { data, path };
        store.enforce_capacity();
        store
    }

    /// 记录一次"确认游戏场景会话"；返回是否产生了变化（供调用方决定是否落盘）。
    pub fn record(&mut self, name: &str, day: &str, now_unix: u64) -> bool {
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty() || name == "aerotune" {
            return false;
        }
        let entry = self.data.entry(name).or_default();
        entry.total = entry.total.saturating_add(1);
        *entry.days.entry(day.to_string()).or_insert(0) += 1;
        entry.last_unix = now_unix;
        self.enforce_capacity();
        true
    }

    /// 用户明确忽略：移除观测条目（可由后续会话重新累积）。
    pub fn dismiss(&mut self, name: &str) -> bool {
        self.data
            .remove(&name.trim().to_ascii_lowercase())
            .is_some()
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("场景观测目录创建失败: {e}"))?;
        }
        let bytes = serde_json::to_vec_pretty(&self.data)
            .map_err(|e| format!("场景观测序列化失败: {e}"))?;
        crate::util::atomic_write(&self.path, &bytes)
    }

    fn enforce_capacity(&mut self) {
        while self.data.len() > STORE_MAX_ENTRIES {
            let oldest = self
                .data
                .iter()
                .min_by_key(|(_, hit)| hit.last_unix)
                .map(|(name, _)| name.clone());
            match oldest {
                Some(name) => {
                    self.data.remove(&name);
                }
                None => break,
            }
        }
    }
}

/// 纯函数：门槛规则——累计 ≥3 次会话且跨 ≥2 天、未在名单内才建议；按热度排序。
pub fn eligible_suggestions(
    data: &SceneShadowData,
    game_list: &[String],
    now_unix: u64,
) -> Vec<SceneSuggestion> {
    let mut suggestions: Vec<SceneSuggestion> = data
        .iter()
        .filter(|(name, hit)| {
            hit.total >= SUGGESTION_MIN_TOTAL
                && hit.days.len() >= SUGGESTION_MIN_DAYS
                && !game_list
                    .iter()
                    .any(|listed| listed.eq_ignore_ascii_case(name))
                && now_unix.saturating_sub(hit.last_unix) <= STALE_ENTRY_SECS
        })
        .map(|(name, hit)| SceneSuggestion {
            name: name.clone(),
            total_hits: hit.total,
            distinct_days: hit.days.len() as u32,
            last_seen_unix: hit.last_unix,
        })
        .collect();
    suggestions.sort_by(|a, b| {
        b.total_hits
            .cmp(&a.total_hits)
            .then_with(|| a.name.cmp(&b.name))
    });
    suggestions.truncate(SUGGESTION_MAX_ITEMS);
    suggestions
}

/// 淘汰 30 天无新会话的条目；返回淘汰数。
pub fn prune_stale(data: &mut SceneShadowData, now_unix: u64) -> usize {
    let stale: Vec<String> = data
        .iter()
        .filter(|(_, hit)| now_unix.saturating_sub(hit.last_unix) > STALE_ENTRY_SECS)
        .map(|(name, _)| name.clone())
        .collect();
    let removed = stale.len();
    for name in stale {
        data.remove(&name);
    }
    removed
}

static SCENE_SHADOW: OnceLock<Mutex<SceneShadow>> = OnceLock::new();

fn store() -> &'static Mutex<SceneShadow> {
    SCENE_SHADOW.get_or_init(|| Mutex::new(SceneShadow::load()))
}

/// 供自动引擎 tick 调用的记录入口：一次确认的游戏场景会话。
/// 落盘失败只记日志，不影响采样主流程。
pub fn observe_game_scene_session(name: &str) {
    let now_unix = chrono::Utc::now().timestamp().max(0) as u64;
    let day = chrono::Local::now().format("%Y-%m-%d").to_string();
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let recorded = guard.record(name, &day, now_unix);
    let pruned = prune_stale(&mut guard.data, now_unix);
    if recorded || pruned > 0 {
        if let Err(error) = guard.save() {
            tracing::warn!(error = %error, "场景影子观测持久化失败（内存数据仍有效）");
        }
    }
}

/// 只读命令：当前可建议加入游戏名单的候选。
#[tauri::command]
pub fn get_scene_suggestions(
    auto: tauri::State<'_, crate::auto::AutoStateLock>,
) -> Vec<SceneSuggestion> {
    let game_list = {
        let state = auto.lock().unwrap_or_else(|e| e.into_inner());
        state.settings.game_list.clone()
    };
    snapshot_scene(&game_list)
}

/// 供智能建议管线复用的快照（与命令同一门槛，无副作用）。
pub(crate) fn snapshot_scene(game_list: &[String]) -> Vec<SceneSuggestion> {
    let now_unix = chrono::Utc::now().timestamp().max(0) as u64;
    let guard = store().lock().unwrap_or_else(|e| e.into_inner());
    eligible_suggestions(&guard.data, game_list, now_unix)
}

/// 忽略命令：移除一条观测（用户不想要的建议）。
#[tauri::command]
pub fn dismiss_scene_suggestion(name: String) -> Result<bool, String> {
    let mut guard = store().lock().unwrap_or_else(|e| e.into_inner());
    let removed = guard.dismiss(&name);
    if removed {
        guard.save()?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(total: u32, days: &[&str], last: u64) -> SceneHit {
        SceneHit {
            total,
            days: days.iter().map(|d| (d.to_string(), 1)).collect(),
            last_unix: last,
        }
    }

    #[test]
    fn suggestions_require_total_days_and_absence_from_list() {
        let mut data = SceneShadowData::new();
        data.insert(
            "strong.exe".into(),
            hit(5, &["2026-10-01", "2026-10-03"], 2000),
        );
        data.insert("one-day.exe".into(), hit(9, &["2026-10-01"], 2000));
        data.insert(
            "two-hits.exe".into(),
            hit(2, &["2026-10-01", "2026-10-02"], 2000),
        );
        data.insert(
            "listed.exe".into(),
            hit(7, &["2026-10-01", "2026-10-02"], 2000),
        );
        data.insert(
            "stale.exe".into(),
            hit(7, &["2026-09-01", "2026-09-02"], 100),
        );
        let list = vec!["Listed.exe".to_string()];
        let now = 1000 + STALE_ENTRY_SECS;
        let suggestions = eligible_suggestions(&data, &list, now);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].name, "strong.exe");
        assert_eq!(suggestions[0].distinct_days, 2);
    }

    #[test]
    fn suggestions_sort_by_hits_and_cap() {
        let mut data = SceneShadowData::new();
        for index in 0..8 {
            data.insert(
                format!("game{index:02}.exe"),
                hit(10 - index as u32, &["2026-10-01", "2026-10-02"], 2000),
            );
        }
        let suggestions = eligible_suggestions(&data, &[], 2000);
        assert_eq!(suggestions.len(), SUGGESTION_MAX_ITEMS);
        assert!(suggestions[0].total_hits >= suggestions[1].total_hits);
    }

    #[test]
    fn record_accumulates_days_and_persists_roundtrip() {
        let path = std::env::temp_dir()
            .join(format!(
                "aerotune-scene-shadow-{}-{}.json",
                std::process::id(),
                1
            ))
            .join("scene-shadow.json");
        let mut store = SceneShadow::load_from(path.clone());
        assert!(store.record("newgame.exe", "2026-10-01", 1000));
        assert!(!store.record("", "2026-10-01", 1000));
        assert!(!store.record("aerotune", "2026-10-01", 1000));
        assert!(store.record("newgame.exe", "2026-10-02", 2000));
        store.save().unwrap();
        let reloaded = SceneShadow::load_from(path.clone());
        assert_eq!(reloaded.data["newgame.exe"].total, 2);
        assert_eq!(reloaded.data["newgame.exe"].days.len(), 2);
        let mut reloaded = reloaded;
        assert!(reloaded.dismiss("NewGame.exe"));
        assert!(!reloaded.dismiss("missing.exe"));
        reloaded.save().unwrap();
        assert!(SceneShadow::load_from(path.clone()).data.is_empty());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn capacity_evicts_oldest_first() {
        let path = std::env::temp_dir()
            .join(format!(
                "aerotune-scene-shadow-{}-{}.json",
                std::process::id(),
                2
            ))
            .join("scene-shadow.json");
        let mut store = SceneShadow::load_from(path.clone());
        for index in 0..STORE_MAX_ENTRIES {
            store.record(
                &format!("game{index:03}.exe"),
                "2026-10-01",
                1000 + index as u64,
            );
        }
        assert_eq!(store.data.len(), STORE_MAX_ENTRIES);
        store.record("newest.exe", "2026-10-02", 9999);
        assert_eq!(store.data.len(), STORE_MAX_ENTRIES);
        assert!(!store.data.contains_key("game000.exe"), "oldest evicted");
        assert!(store.data.contains_key("newest.exe"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn prune_removes_only_stale_entries() {
        let mut data = SceneShadowData::new();
        data.insert(
            "fresh.exe".into(),
            hit(3, &["2026-10-01", "2026-10-02"], 5000),
        );
        data.insert(
            "old.exe".into(),
            hit(3, &["2026-08-01", "2026-08-02"], 1000),
        );
        let now = 5000 + STALE_ENTRY_SECS;
        assert_eq!(prune_stale(&mut data, now), 1);
        assert!(data.contains_key("fresh.exe"));
        assert!(!data.contains_key("old.exe"));
    }
}
