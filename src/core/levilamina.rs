use crate::http::proxy::get_client_for_proxy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const LEVILAUNCHER_INDEX_URL: &str = "https://lipr.levimc.org/levilauncher.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LipIndex {
    pub format_version: Option<u32>,
    pub format_uuid: Option<String>,
    #[serde(default)]
    pub packages: HashMap<String, LipPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LipPackage {
    pub stargazer_count: Option<u32>,
    pub updated_at: Option<String>,
    #[serde(default)]
    pub info: LipPackageInfo,
    #[serde(default)]
    pub variants: HashMap<String, LipVariant>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LipPackageInfo {
    pub name: Option<String>,
    pub description: Option<String>,
    pub tags: Option<Vec<String>>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LipVariant {
    #[serde(default)]
    pub versions: HashMap<String, LipVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LipVersion {
    #[serde(default)]
    pub dependencies: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeviLaminaModEntry {
    pub package_id: String,
    pub name: String,
    pub description: String,
    pub avatar_url: String,
    pub stargazer_count: u32,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub client_versions: Vec<String>,
    pub all_versions: Vec<String>,
    pub version_dependencies: HashMap<String, HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeviLaminaIndexResult {
    pub loader_versions: Vec<String>,
    pub client_mods: Vec<LeviLaminaModEntry>,
}

/// Simple semver-aware comparator for version sorting (descending).
pub fn compare_version_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let parse_v = |v: &str| {
        let clean = v.trim_start_matches('v').split('-').next().unwrap_or(v);
        let parts: Vec<u64> = clean
            .split('.')
            .map(|s| s.parse::<u64>().unwrap_or(0))
            .collect();
        parts
    };
    let va = parse_v(a);
    let vb = parse_v(b);
    vb.cmp(&va).then_with(|| b.cmp(a))
}

pub async fn fetch_levilamina_index() -> Result<LeviLaminaIndexResult, String> {
    let client = get_client_for_proxy().map_err(|e| e.to_string())?;
    let response = client
        .get(LEVILAUNCHER_INDEX_URL)
        .send()
        .await
        .map_err(|e| format!("Network request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Server returned error status: {}",
            response.status()
        ));
    }

    let index: LipIndex = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse LeviLamina JSON: {e}"))?;

    // Extract loader versions from github.com/LiteLDev/LeviLamina
    let mut loader_versions = Vec::new();
    if let Some(levilamina_pkg) = index.packages.get("github.com/LiteLDev/LeviLamina") {
        for (variant_name, variant) in &levilamina_pkg.variants {
            if variant_name == "client" || variant_name.is_empty() {
                for ver in variant.versions.keys() {
                    if !loader_versions.contains(ver) {
                        loader_versions.push(ver.clone());
                    }
                }
            }
        }
    }
    loader_versions.sort_by(|a, b| compare_version_desc(a, b));

    let mut client_mods = Vec::new();

    for (package_id, pkg) in &index.packages {
        // Skip the loader package itself from standard mod cards
        if package_id == "github.com/LiteLDev/LeviLamina" {
            continue;
        }

        // Determine if it's a client mod:
        // 1. Has "client" or variant starting with "client" in variants map
        // 2. OR info.tags contains "type:client" or "client"
        let has_client_variant = pkg
            .variants
            .keys()
            .any(|k| k == "client" || k.starts_with("client_") || k.contains("client"));
        let has_client_tag = pkg
            .info
            .tags
            .as_ref()
            .map(|tags| {
                tags.iter().any(|t| {
                    let lowercase = t.to_lowercase();
                    lowercase == "client"
                        || lowercase == "type:client"
                        || lowercase.contains("client")
                })
            })
            .unwrap_or(false);

        if !has_client_variant && !has_client_tag {
            continue;
        }

        let name = pkg
            .info
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| {
                package_id
                    .split('/')
                    .last()
                    .unwrap_or(package_id)
                    .to_string()
            });

        let description = pkg.info.description.clone().unwrap_or_default();
        let avatar_url = pkg.info.avatar_url.clone().unwrap_or_default();
        let stargazer_count = pkg.stargazer_count.unwrap_or(0);
        let updated_at = pkg
            .updated_at
            .clone()
            .unwrap_or_default()
            .split('T')
            .next()
            .unwrap_or("")
            .to_string();

        let tags = pkg.info.tags.clone().unwrap_or_default();

        let mut client_versions_set = Vec::new();
        let mut all_versions_set = Vec::new();
        let mut version_dependencies = HashMap::new();

        for (variant_name, variant) in &pkg.variants {
            let is_client_var = variant_name == "client"
                || variant_name.starts_with("client_")
                || variant_name.contains("client");
            for (ver, ver_info) in &variant.versions {
                if !all_versions_set.contains(ver) {
                    all_versions_set.push(ver.clone());
                }
                if is_client_var && !client_versions_set.contains(ver) {
                    client_versions_set.push(ver.clone());
                }
                version_dependencies
                    .entry(ver.clone())
                    .or_insert_with(HashMap::new)
                    .extend(ver_info.dependencies.clone());
            }
        }

        client_versions_set.sort_by(|a, b| compare_version_desc(a, b));
        all_versions_set.sort_by(|a, b| compare_version_desc(a, b));

        let available_versions = if !client_versions_set.is_empty() {
            client_versions_set
        } else {
            all_versions_set.clone()
        };

        client_mods.push(LeviLaminaModEntry {
            package_id: package_id.clone(),
            name,
            description,
            avatar_url,
            stargazer_count,
            updated_at,
            tags,
            client_versions: available_versions,
            all_versions: all_versions_set,
            version_dependencies,
        });
    }

    // Sort client mods by stargazer_count descending, then by name
    client_mods.sort_by(|a, b| {
        b.stargazer_count
            .cmp(&a.stargazer_count)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(LeviLaminaIndexResult {
        loader_versions,
        client_mods,
    })
}

pub fn mod_matches_loader_version(
    mod_entry: &LeviLaminaModEntry,
    target_loader: &str,
    target_loader_version: &str,
) -> bool {
    let loader_match = if target_loader.is_empty()
        || target_loader == "全部"
        || target_loader == "全部加载器"
        || target_loader == "All"
    {
        true
    } else if target_loader == "LeviLamina" {
        mod_entry
            .tags
            .iter()
            .any(|t| t.to_lowercase().contains("levilamina"))
            || mod_entry
                .version_dependencies
                .values()
                .any(|deps| deps.keys().any(|k| k.to_lowercase().contains("levilamina")))
    } else {
        let loader_lower = target_loader.to_lowercase();
        mod_entry
            .tags
            .iter()
            .any(|t| t.to_lowercase().contains(&loader_lower))
            || mod_entry.version_dependencies.values().any(|deps| {
                deps.keys()
                    .any(|k| k.to_lowercase().contains(&loader_lower))
            })
    };

    if !loader_match {
        return false;
    }

    if target_loader_version.is_empty()
        || target_loader_version == "全部"
        || target_loader_version == "全部版本"
        || target_loader_version == "All"
    {
        return true;
    }

    let target_prefix = target_loader_version
        .split('.')
        .take(2)
        .collect::<Vec<&str>>()
        .join(".");

    for (ver, deps) in &mod_entry.version_dependencies {
        for (dep_key, dep_req) in deps {
            let key_match = if target_loader == "全部"
                || target_loader == "全部加载器"
                || target_loader.is_empty()
            {
                dep_key.contains("LeviLamina")
            } else {
                dep_key
                    .to_lowercase()
                    .contains(&target_loader.to_lowercase())
                    || dep_key.contains("LeviLamina")
            };

            if key_match {
                if dep_req.contains(&target_prefix)
                    || dep_req.contains(target_loader_version)
                    || dep_req == "*"
                {
                    return true;
                }
                if target_loader_version.starts_with("26.10") && dep_req.contains("26.10") {
                    return true;
                }
                if target_loader_version.starts_with("26.20") && dep_req.contains("26.20") {
                    return true;
                }
                if target_loader_version.starts_with("1.9") && dep_req.contains("1.9") {
                    return true;
                }
                if target_loader_version.starts_with("1.8") && dep_req.contains("1.8") {
                    return true;
                }
            }
        }
        if ver.starts_with(&target_prefix) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compare_version_desc() {
        assert_eq!(
            compare_version_desc("26.20.4", "26.10.9"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_version_desc("1.9.7", "26.10.0"),
            std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn test_mod_matches_loader_version() {
        let mut deps = HashMap::new();
        let mut ver_deps = HashMap::new();
        deps.insert(
            "github.com/LiteLDev/LeviLamina#client".to_string(),
            ">=26.10.0 <26.20.0".to_string(),
        );
        ver_deps.insert("1.0.0".to_string(), deps);

        let mod_entry = LeviLaminaModEntry {
            package_id: "github.com/test/mod".to_string(),
            name: "Test Mod".to_string(),
            description: "A test client mod".to_string(),
            avatar_url: "".to_string(),
            stargazer_count: 5,
            updated_at: "2026-07-23".to_string(),
            tags: vec!["platform:levilamina".to_string(), "type:client".to_string()],
            client_versions: vec!["1.0.0".to_string()],
            all_versions: vec!["1.0.0".to_string()],
            version_dependencies: ver_deps,
        };

        assert!(mod_matches_loader_version(&mod_entry, "全部", "全部版本"));
        assert!(mod_matches_loader_version(
            &mod_entry,
            "LeviLamina",
            "26.10.9"
        ));
        assert!(!mod_matches_loader_version(
            &mod_entry,
            "LeviLamina",
            "1.8.0"
        ));
    }
}
