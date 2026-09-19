use std::collections::HashMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

use crate::http::proxy::get_client_for_proxy;

const VERSION_DATABASE_URL: &str =
    "https://fastly.jsdelivr.net/gh/LiteLDev/levilamina-client-version-db@main/v2/version-db.json";
const API_CACHE_TTL: Duration = Duration::from_secs(60 * 60);
static SUPPORT_DATABASE_CACHE: Lazy<Mutex<Option<LeviLaminaSupportDatabase>>> =
    Lazy::new(|| Mutex::new(None));

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LeviLaminaSupportDatabase {
    pub format_version: u32,
    #[serde(default)]
    pub versions: HashMap<String, Vec<String>>,
}

impl LeviLaminaSupportDatabase {
    /// Returns every loader version published by the compatibility database, sorted newest first.
    #[must_use]
    pub fn all_loader_versions(&self) -> Vec<String> {
        let mut versions = self
            .versions
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        versions.sort_by(|left, right| super::compare_version_desc(left, right));
        versions.dedup();
        versions
    }

    #[must_use]
    pub fn loader_versions(&self, game_version: &str) -> Vec<String> {
        loader_versions_for_game(&self.versions, game_version)
    }

    #[must_use]
    pub fn supports_loader(&self, game_version: &str, loader_version: &str) -> bool {
        self.loader_versions(game_version)
            .iter()
            .any(|version| version == loader_version)
    }

    #[must_use]
    pub fn supports_game(&self, game_version: &str) -> bool {
        !self.loader_versions(game_version).is_empty()
    }
}

async fn fetch_support_database() -> Result<LeviLaminaSupportDatabase, String> {
    let client = get_client_for_proxy().map_err(|error| error.to_string())?;
    let response = crate::github::get(&client, VERSION_DATABASE_URL)
        .await
        .map_err(|error| format!("获取 LeviLamina 版本数据库失败: {error}"))?;
    let mut database = response
        .json::<LeviLaminaSupportDatabase>()
        .await
        .map_err(|error| format!("解析 LeviLamina 版本数据库失败: {error}"))?;
    for versions in database.versions.values_mut() {
        versions.sort_by(|left, right| super::compare_version_desc(left, right));
        versions.dedup();
    }
    Ok(database)
}

fn api_cache_path() -> PathBuf {
    crate::utils::file_ops::levilamina_api_cache_dir().join("version-db.json")
}

async fn read_api_cache() -> Option<LeviLaminaSupportDatabase> {
    let path = api_cache_path();
    match crate::tasks::runtime::run_io_blocking(move || {
        crate::core::api_cache::read_fresh(&path, API_CACHE_TTL)
    })
    .await
    {
        Ok(cache) => cache,
        Err(error) => {
            tracing::warn!(%error, "LeviLamina support API cache read worker failed");
            None
        }
    }
}

async fn write_api_cache(database: LeviLaminaSupportDatabase) {
    let path = api_cache_path();
    if let Err(error) = crate::tasks::runtime::run_io_blocking(move || {
        crate::core::api_cache::write(&path, &database)
    })
    .await
    {
        tracing::warn!(%error, "LeviLamina support API cache write worker failed");
    }
}

/// Clears the compatibility database and its current disk cache so the next UI load fetches data.
pub fn clear_cache() {
    let mut cache = SUPPORT_DATABASE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache = None;
    if let Err(error) = crate::core::api_cache::remove(&api_cache_path()) {
        tracing::warn!(%error, "LeviLamina support API cache removal failed");
    }
}

/// Returns the LeviLamina compatibility database cached for this process.
///
/// A failed request is not stored, allowing a later request to retry.
pub async fn support_database() -> Result<LeviLaminaSupportDatabase, String> {
    if let Some(database) = SUPPORT_DATABASE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Ok(database);
    }

    if let Some(database) = read_api_cache().await {
        *SUPPORT_DATABASE_CACHE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(database.clone());
        return Ok(database);
    }

    let database = fetch_support_database().await?;
    write_api_cache(database.clone()).await;
    *SUPPORT_DATABASE_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(database.clone());
    Ok(database)
}

#[must_use]
pub fn loader_versions_for_game(
    versions: &HashMap<String, Vec<String>>,
    game_version: &str,
) -> Vec<String> {
    let Some(normalized_target) = numeric_version(game_version) else {
        return Vec::new();
    };
    let mut loader_versions = versions
        .iter()
        .filter_map(|(supported_game, loader_versions)| {
            let supported = numeric_version(supported_game)?;
            game_versions_match(&normalized_target, &supported)
                .then_some(loader_versions.as_slice())
        })
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    loader_versions.sort_by(|left, right| super::compare_version_desc(left, right));
    loader_versions.dedup();
    loader_versions
}

fn game_versions_match(target: &[u64], supported: &[u64]) -> bool {
    version_form_matches(target, supported)
        || supported
            .strip_prefix(&[1])
            .is_some_and(|without_major| version_form_matches(target, without_major))
}

fn version_form_matches(target: &[u64], supported_form: &[u64]) -> bool {
    supported_form == target
        || supported_form.starts_with(target)
        || (target.len() >= 4
            && supported_form.len() >= 4
            && target[..target.len() - 1] == supported_form[..supported_form.len() - 1])
}

fn numeric_version(version: &str) -> Option<Vec<u64>> {
    version
        .trim()
        .trim_start_matches('v')
        .split('.')
        .map(|part| part.parse::<u64>().ok())
        .collect()
}

#[cfg(test)]
#[path = "support_tests.rs"]
mod tests;
