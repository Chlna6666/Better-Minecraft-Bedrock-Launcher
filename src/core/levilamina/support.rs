use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::OnceCell;

use crate::http::proxy::get_client_for_proxy;

const VERSION_DATABASE_URL: &str = "https://raw.githubusercontent.com/LiteLDev/levilamina-client-version-db/refs/heads/main/v2/version-db.json";
static SUPPORT_DATABASE_CACHE: OnceCell<LeviLaminaSupportDatabase> = OnceCell::const_new();

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct LeviLaminaSupportDatabase {
    pub format_version: u32,
    #[serde(default)]
    pub versions: HashMap<String, Vec<String>>,
}

impl LeviLaminaSupportDatabase {
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

pub async fn fetch_support_database() -> Result<LeviLaminaSupportDatabase, String> {
    let client = get_client_for_proxy().map_err(|error| error.to_string())?;
    let response = client
        .get(VERSION_DATABASE_URL)
        .send()
        .await
        .map_err(|error| format!("获取 LeviLamina 版本数据库失败: {error}"))?
        .error_for_status()
        .map_err(|error| format!("LeviLamina 版本数据库返回错误: {error}"))?;
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

pub async fn cached_support_database() -> Result<LeviLaminaSupportDatabase, String> {
    let database = SUPPORT_DATABASE_CACHE
        .get_or_try_init(|| async { fetch_support_database().await })
        .await?;
    Ok(database.clone())
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
    supported_form == target || supported_form.starts_with(target)
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
