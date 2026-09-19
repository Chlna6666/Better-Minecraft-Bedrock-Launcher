use std::collections::BTreeMap;
use std::sync::Mutex;

use once_cell::sync::Lazy;
use reqwest::Client;
use serde::{Deserialize, Serialize};

mod install;

pub use install::{NativeModInstallRequest, start_install};

const INDEX_URL: &str = "https://pkg.roundstudio.top/index.json";

static INDEX_CACHE: Lazy<Mutex<Option<Vec<NativeModEntry>>>> = Lazy::new(|| Mutex::new(None));

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeModEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    pub repository: String,
    pub author: String,
    pub tags: Vec<String>,
    pub files: BTreeMap<String, NativeModFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeModFile {
    pub url: String,
    #[serde(rename = "type")]
    pub file_type: String,
    #[serde(default)]
    pub actions: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct NativeModIndex {
    #[serde(default)]
    packages: Vec<NativeModPackage>,
}

#[derive(Debug, Deserialize)]
struct NativeModPackage {
    id: String,
    #[serde(default)]
    header: NativeModHeader,
    #[serde(default)]
    files: BTreeMap<String, NativeModFile>,
}

#[derive(Debug, Default, Deserialize)]
struct NativeModHeader {
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default, alias = "repo", alias = "repository")]
    reop: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct GithubLatestRelease {
    tag_name: String,
}

/// Clears the in-memory native-mod catalog so the next UI load fetches the current index.
pub fn clear_cache() {
    let mut cache = INDEX_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *cache = None;
}

/// Returns the native-mod catalog cached for the lifetime of this process.
///
/// The catalog is independent from the LeviLamina registry. Failed requests are not cached.
pub async fn package_index() -> Result<Vec<NativeModEntry>, String> {
    if let Some(entries) = INDEX_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
    {
        return Ok(entries);
    }

    let entries = fetch_index().await?;
    *INDEX_CACHE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(entries.clone());
    Ok(entries)
}

async fn fetch_index() -> Result<Vec<NativeModEntry>, String> {
    let client = crate::http::proxy::get_client_for_proxy().map_err(|error| error.to_string())?;
    let response = client
        .get(INDEX_URL)
        .send()
        .await
        .map_err(|error| format!("请求原生 Mod 索引失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!("原生 Mod 索引返回错误状态：{}", response.status()));
    }

    let index = response
        .json::<NativeModIndex>()
        .await
        .map_err(|error| format!("解析原生 Mod 索引失败：{error}"))?;
    let mut entries = index
        .packages
        .into_iter()
        .filter(|package| !package.files.is_empty())
        .map(|package| NativeModEntry {
            name: if package.header.name.trim().is_empty() {
                package.id.clone()
            } else {
                package.header.name
            },
            id: package.id,
            description: package.header.description,
            repository: package.header.reop,
            author: package.header.author,
            tags: package.header.tags,
            files: package.files,
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(entries)
}

pub(crate) async fn resolve_file_url(
    client: &Client,
    repository: &str,
    template: &str,
) -> Result<String, String> {
    if !template.contains("{{tag}}") {
        return Ok(template.to_string());
    }
    let repository = github_repository(repository)
        .ok_or_else(|| format!("原生 Mod 仓库不是 GitHub 地址：{repository}"))?;
    let release_url = format!("https://api.github.com/repos/{repository}/releases/latest");
    let response = client
        .get(release_url)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .map_err(|error| format!("获取 GitHub Release 失败：{error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "GitHub Release 返回错误状态：{}",
            response.status()
        ));
    }
    let release = response
        .json::<GithubLatestRelease>()
        .await
        .map_err(|error| format!("解析 GitHub Release 失败：{error}"))?;
    if release.tag_name.trim().is_empty() {
        return Err("GitHub Release 没有 tag_name".to_string());
    }
    Ok(template.replace("{{tag}}", &release.tag_name))
}

fn github_repository(repository: &str) -> Option<String> {
    let url = reqwest::Url::parse(repository).ok()?;
    if url.host_str()? != "github.com" {
        return None;
    }
    let path = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    (path.len() >= 2).then(|| format!("{}/{}", path[0], path[1]))
}

#[cfg(test)]
mod tests {
    use super::{NativeModIndex, github_repository, resolve_file_url};

    #[test]
    fn parses_native_mod_index_shape() {
        let index: NativeModIndex = serde_json::from_str(
            r#"{
                "packages": [{
                    "id": "example.Mod",
                    "header": {"name": "Example", "reop": "https://github.com/example/mod"},
                    "files": {"Example.dll": {"url": "https://github.com/example/mod/releases/download/{{tag}}/Example.dll", "type": "native.dll"}}
                }]
            }"#,
        )
        .expect("valid native mod index");
        assert_eq!(index.packages.len(), 1);
        assert_eq!(index.packages[0].files.len(), 1);
    }

    #[test]
    fn extracts_github_repository() {
        assert_eq!(
            github_repository("https://github.com/example/mod"),
            Some("example/mod".into())
        );
        assert_eq!(github_repository("https://example.com/example/mod"), None);
    }

    #[tokio::test]
    async fn leaves_direct_file_urls_unchanged() {
        let client = reqwest::Client::new();
        let url = resolve_file_url(
            &client,
            "https://github.com/example/mod",
            "https://example.com/mod.dll",
        )
        .await
        .expect("direct URL should not need a release request");
        assert_eq!(url, "https://example.com/mod.dll");
    }
}
