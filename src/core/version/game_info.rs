use chrono::{DateTime, Days, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

const CONFIG_DIRECTORY: &str = "config/BMCBL";
const GAME_INFO_FILE: &str = "game_info.json";

static ACTIVE_SESSIONS: LazyLock<Mutex<HashSet<(PathBuf, u32)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));
static GAME_INFO_WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameInfo {
    #[serde(default)]
    pub total_play_time: u64,
    #[serde(default)]
    pub last_play_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub total_sessions: u64,
    #[serde(default)]
    pub first_play_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub daily: BTreeMap<NaiveDate, DailyGameInfo>,
    #[serde(default, flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyGameInfo {
    #[serde(default)]
    pub play_time: u64,
    #[serde(default)]
    pub sessions: u64,
}

#[derive(Clone, Debug)]
pub struct GameSession {
    instance_path: PathBuf,
    process_id: u32,
    started_at: DateTime<Utc>,
    saved_through: DateTime<Utc>,
}

impl GameSession {
    pub async fn start(instance_path: PathBuf, process_id: u32) -> Result<Option<Self>, String> {
        let key = (instance_path.clone(), process_id);
        let inserted = {
            let mut active = ACTIVE_SESSIONS
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            active.insert(key)
        };
        if !inserted {
            return Ok(None);
        }

        let started_at = Utc::now();
        let path_for_write = instance_path.clone();
        let write_result = crate::tasks::runtime::run_io_blocking(move || {
            record_session_start(&path_for_write, started_at)
        })
        .await;
        let write_result = write_result
            .map_err(|error| format!("记录游戏会话任务失败：{error}"))
            .and_then(|result| result);
        if let Err(error) = write_result {
            ACTIVE_SESSIONS
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .remove(&(instance_path.clone(), process_id));
            return Err(error);
        }
        crate::core::version::catalog_events::notify_local_versions_changed();
        Ok(Some(Self {
            instance_path,
            process_id,
            started_at,
            saved_through: started_at,
        }))
    }

    pub async fn checkpoint(&mut self) -> Result<(), String> {
        let checkpoint_at = Utc::now();
        let elapsed = checkpoint_at
            .signed_duration_since(self.saved_through)
            .to_std()
            .unwrap_or(Duration::ZERO);
        if elapsed.as_secs() == 0 {
            return Ok(());
        }
        let instance_path = self.instance_path.clone();
        let saved_through = self.saved_through;
        let result = crate::tasks::runtime::run_io_blocking(move || {
            record_session_duration(&instance_path, saved_through, checkpoint_at, elapsed)
        })
        .await
        .map_err(|error| format!("更新游戏统计任务失败：{error}"))
        .and_then(|result| result);
        if result.is_ok() {
            self.saved_through = checkpoint_at;
            crate::core::version::catalog_events::notify_local_versions_changed();
        }
        result
    }

    pub async fn finish(mut self) -> Result<(), String> {
        let result = self.checkpoint().await;
        ACTIVE_SESSIONS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&(self.instance_path, self.process_id));
        result
    }
}

#[must_use]
pub fn game_info_path(instance_path: &Path) -> PathBuf {
    instance_path.join(CONFIG_DIRECTORY).join(GAME_INFO_FILE)
}

pub fn load_game_info(instance_path: &Path) -> Result<GameInfo, String> {
    load_game_info_path(&game_info_path(instance_path))
}

fn load_game_info_path(path: &Path) -> Result<GameInfo, String> {
    if !path.exists() {
        return Ok(GameInfo::default());
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("无法读取游戏统计 {}：{error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|error| format!("无法解析游戏统计 {}：{error}", path.display()))?;
    let mut object = value
        .as_object()
        .cloned()
        .ok_or_else(|| format!("游戏统计根节点不是对象：{}", path.display()))?;
    if let Some(player_data) = object.remove("playerData") {
        let player_data = player_data
            .as_object()
            .ok_or_else(|| format!("游戏统计 playerData 不是对象：{}", path.display()))?;
        for (key, value) in player_data {
            object.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(serde_json::Value::Object(object))
        .map_err(|error| format!("无法解析游戏统计 {}：{error}", path.display()))
}

fn record_session_start(instance_path: &Path, started_at: DateTime<Utc>) -> Result<(), String> {
    update_game_info(instance_path, |info| {
        info.total_sessions = info.total_sessions.saturating_add(1);
        info.first_play_time.get_or_insert(started_at);
        info.last_play_time = Some(started_at);
        let daily = info.daily.entry(started_at.date_naive()).or_default();
        daily.sessions = daily.sessions.saturating_add(1);
    })
}

fn record_session_duration(
    instance_path: &Path,
    started_at: DateTime<Utc>,
    finished_at: DateTime<Utc>,
    elapsed: Duration,
) -> Result<(), String> {
    update_game_info(instance_path, |info| {
        info.total_play_time = info.total_play_time.saturating_add(elapsed.as_secs());
        add_daily_play_time(info, started_at, finished_at);
    })
}

fn add_daily_play_time(info: &mut GameInfo, mut cursor: DateTime<Utc>, finished_at: DateTime<Utc>) {
    while cursor < finished_at {
        let date = cursor.date_naive();
        let Some(next_date) = date.checked_add_days(Days::new(1)) else {
            break;
        };
        let Some(next_midnight) = next_date.and_hms_opt(0, 0, 0).map(|time| time.and_utc()) else {
            break;
        };
        let segment_end = finished_at.min(next_midnight);
        let seconds = segment_end
            .signed_duration_since(cursor)
            .num_seconds()
            .max(0) as u64;
        let daily = info.daily.entry(date).or_default();
        daily.play_time = daily.play_time.saturating_add(seconds);
        cursor = segment_end;
    }
}

fn update_game_info(
    instance_path: &Path,
    update: impl FnOnce(&mut GameInfo),
) -> Result<(), String> {
    let _write_guard = GAME_INFO_WRITE_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut info = load_game_info(instance_path)?;
    update(&mut info);
    write_json_atomically(&game_info_path(instance_path), &info)
}

pub(crate) fn write_json_atomically(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("配置路径没有父目录：{}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("无法创建配置目录 {}：{error}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("无法序列化配置 {}：{error}", path.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("无法创建临时配置 {}：{error}", path.display()))?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("无法写入临时配置 {}：{error}", path.display()))?;
    temporary
        .persist(path)
        .map_err(|error| format!("无法替换配置 {}：{}", path.display(), error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapped_player_data_is_accepted() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = directory.path().join("game_info.json");
        std::fs::write(
            &path,
            r#"{"playerData":{"totalPlayTime":42,"totalSessions":3}}"#,
        )
        .map_err(|error| error.to_string())?;

        let info = load_game_info_path(&path)?;
        assert_eq!(info.total_play_time, 42);
        assert_eq!(info.total_sessions, 3);
        Ok(())
    }

    #[test]
    fn wrapped_unknown_fields_are_preserved_when_normalized() -> Result<(), String> {
        let directory = tempfile::tempdir().map_err(|error| error.to_string())?;
        let instance_path = directory.path();
        let path = game_info_path(instance_path);
        std::fs::create_dir_all(path.parent().ok_or("missing parent")?)
            .map_err(|error| error.to_string())?;
        std::fs::write(
            &path,
            r#"{"foreignTop":true,"playerData":{"totalSessions":2,"foreignInner":"keep"}}"#,
        )
        .map_err(|error| error.to_string())?;

        let started_at = Utc::now();
        record_session_duration(
            instance_path,
            started_at,
            started_at + chrono::Duration::seconds(5),
            Duration::from_secs(5),
        )?;
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        assert!(value.get("playerData").is_none());
        assert_eq!(value.get("totalPlayTime"), Some(&serde_json::json!(5)));
        assert_eq!(value.get("foreignTop"), Some(&serde_json::json!(true)));
        assert_eq!(value.get("foreignInner"), Some(&serde_json::json!("keep")));
        Ok(())
    }

    #[test]
    fn session_updates_are_saturating() {
        let mut info = GameInfo {
            total_play_time: u64::MAX,
            total_sessions: u64::MAX,
            ..GameInfo::default()
        };
        info.total_sessions = info.total_sessions.saturating_add(1);
        info.total_play_time = info.total_play_time.saturating_add(1);
        assert_eq!(info.total_sessions, u64::MAX);
        assert_eq!(info.total_play_time, u64::MAX);
    }

    #[test]
    fn daily_play_time_is_split_across_utc_midnight() -> Result<(), String> {
        let started_at = DateTime::parse_from_rfc3339("2026-08-11T23:50:00Z")
            .map_err(|error| error.to_string())?
            .with_timezone(&Utc);
        let finished_at = DateTime::parse_from_rfc3339("2026-08-12T00:20:00Z")
            .map_err(|error| error.to_string())?
            .with_timezone(&Utc);
        let mut info = GameInfo::default();

        add_daily_play_time(&mut info, started_at, finished_at);

        let first_day = NaiveDate::parse_from_str("2026-08-11", "%Y-%m-%d")
            .map_err(|error| error.to_string())?;
        let second_day = NaiveDate::parse_from_str("2026-08-12", "%Y-%m-%d")
            .map_err(|error| error.to_string())?;
        assert_eq!(
            info.daily.get(&first_day).map(|day| day.play_time),
            Some(600)
        );
        assert_eq!(
            info.daily.get(&second_day).map(|day| day.play_time),
            Some(1_200)
        );
        Ok(())
    }
}
