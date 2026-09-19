use serde::{Serialize, de::DeserializeOwned};
use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CACHE_SCHEMA_VERSION: u32 = 1;

#[derive(serde::Deserialize, serde::Serialize)]
struct CacheEnvelope<T> {
    schema_version: u32,
    fetched_at_unix_ms: u64,
    value: T,
}

pub(crate) fn read_fresh<T: DeserializeOwned>(path: &Path, max_age: Duration) -> Option<T> {
    let raw = fs::read_to_string(path).ok()?;
    let cache: CacheEnvelope<T> = serde_json::from_str(&raw).ok()?;
    if cache.schema_version != CACHE_SCHEMA_VERSION {
        return None;
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let age = Duration::from_millis(now.saturating_sub(cache.fetched_at_unix_ms));
    (age <= max_age).then_some(cache.value)
}

pub(crate) fn write<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let envelope = CacheEnvelope {
        schema_version: CACHE_SCHEMA_VERSION,
        fetched_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        value,
    };
    let raw = serde_json::to_vec(&envelope).map_err(std::io::Error::other)?;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "cache path has no parent")
    })?;
    fs::create_dir_all(parent)?;

    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, raw)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(tmp_path, path)
}

pub(crate) fn remove(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}
