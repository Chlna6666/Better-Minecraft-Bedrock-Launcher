use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use futures::{StreamExt as _, TryStreamExt as _};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::downloads::verified::{Request, valid_sha256};

const ORIGIN: &str = "https://bmcbl-plugin.chlna6666.com";
const MAX_PAGE_BYTES: usize = 128 * 1024;
const MAX_INDEX_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct Catalog {
    revision: String,
    pages: BTreeMap<String, Arc<Vec<Plugin>>>,
    pub(crate) plugins: Arc<[Plugin]>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Plugin {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) publisher: String,
    pub(crate) status: String,
    pub(crate) version: String,
    pub(crate) icon: Option<Icon>,
}

#[derive(Clone, Deserialize)]
pub(crate) struct Icon {
    path: String,
    sha256: String,
    size: u64,
}

impl Icon {
    pub(crate) fn url(&self, plugin_id: &str) -> Option<String> {
        (valid_id(plugin_id)
            && valid_sha256(&self.sha256)
            && (1..=64 * 1024).contains(&self.size)
            && self.path == format!("/assets/plugins/{plugin_id}/icons/{}.png", self.sha256))
        .then(|| format!("{ORIGIN}{}", self.path))
    }
}

#[derive(Deserialize)]
struct Index {
    schema_version: u32,
    revision: String,
    page_count: usize,
    total: usize,
    pages: Vec<PageRef>,
}

#[derive(Deserialize)]
struct PageRef {
    page: usize,
    count: usize,
    path: String,
    sha256: String,
    bytes: usize,
}

#[derive(Deserialize)]
struct Page {
    schema_version: u32,
    page: usize,
    total: usize,
    plugins: Vec<Plugin>,
}

#[derive(Deserialize)]
struct Detail {
    schema_version: u32,
    api_version: String,
    id: String,
    version: String,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    filename: String,
    sha256: String,
    size: u64,
    sources: Vec<Source>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Source {
    Edgeone {
        path: String,
    },
    Github {
        url: String,
    },
    #[serde(other)]
    Other,
}

pub(crate) async fn catalog(previous: Catalog) -> Result<Catalog, String> {
    let client = crate::http::proxy::clients::client().map_err(|error| error.to_string())?;
    let index: Index = decode(&fetch(&client, "/v1/index.json", MAX_INDEX_BYTES).await?)?;
    if index.schema_version != 1
        || !valid_sha256(&index.revision)
        || index.pages.len() != index.page_count
        || index.pages.len() > 1024
    {
        return Err("插件目录索引格式无效".into());
    }
    if index.revision == previous.revision {
        return Ok(previous);
    }
    let pages: Vec<_> = futures::stream::iter(index.pages.into_iter().enumerate())
        .map(|(number, descriptor)| {
            let client = &client;
            let cached = previous.pages.get(&descriptor.sha256).cloned();
            async move {
                let PageRef {
                    page,
                    count,
                    path,
                    sha256,
                    bytes,
                } = descriptor;
                let filename = path.strip_prefix("/v1/pages/").unwrap_or_default();
                if page != number
                    || count > 100
                    || bytes > MAX_PAGE_BYTES
                    || !valid_sha256(&sha256)
                    || filename != format!("{page:04}.{}.json", &sha256[..20])
                {
                    return Err("插件目录分页格式无效".to_string());
                }
                let entries = if let Some(entries) = cached {
                    entries
                } else {
                    let body = fetch(client, &path, bytes).await?;
                    if body.len() != bytes || hex::encode(Sha256::digest(&body)) != sha256 {
                        return Err("插件目录分页 SHA-256 校验失败".into());
                    }
                    let data: Page = decode(&body)?;
                    if data.schema_version != 1 || data.page != page || data.total < count {
                        return Err("插件目录分页与索引不符".into());
                    }
                    Arc::new(data.plugins)
                };
                if entries.len() != count {
                    return Err("插件目录分页数量不符".into());
                }
                Ok((sha256, entries))
            }
        })
        .buffered(4)
        .try_collect()
        .await?;
    let plugins: Vec<_> = pages
        .iter()
        .flat_map(|(_, entries)| entries.iter().cloned())
        .collect();
    let mut ids = BTreeSet::new();
    if plugins.len() != index.total
        || plugins
            .iter()
            .any(|plugin| !valid_id(&plugin.id) || !ids.insert(&plugin.id))
    {
        return Err("插件目录包含无效或重复的插件 ID".into());
    }
    Ok(Catalog {
        revision: index.revision,
        pages: pages.into_iter().collect(),
        plugins: plugins.into(),
    })
}

pub(crate) async fn readme(
    id: String,
    version: String,
    locale: String,
) -> Result<Option<String>, String> {
    let package = crate::downloads::verified::fetch(package(&id, &version).await?).await?;
    crate::tasks::runtime::blocking::run_io(move || {
        super::manifest::read_package_readme(&package, &id, &version, &locale)
            .map_err(|error| error.to_string())
    })
    .await?
}

pub(crate) async fn package(id: &str, version: &str) -> Result<Request, String> {
    super::manifest::validate_plugin_id(id).map_err(|error| error.to_string())?;
    let client = crate::http::proxy::clients::client().map_err(|error| error.to_string())?;
    let detail: Detail =
        decode(&fetch(&client, &format!("/v1/plugins/{id}.json"), MAX_INDEX_BYTES).await?)?;
    if detail.schema_version != 1
        || detail.api_version != crate::plugins::manifest::CURRENT_API_VERSION
        || detail.id != id
        || detail.version != version
    {
        return Err("目录已更新或插件 API 不兼容，请刷新后重试".into());
    }
    let asset = detail
        .assets
        .into_iter()
        .find(|asset| asset.filename.ends_with(".bmcblx"))
        .ok_or("该版本未发布插件包")?;
    let mut urls = Vec::new();
    for source in asset.sources {
        match source {
            Source::Edgeone { path }
                if path == format!("/downloads/official/{id}/{}.bmcblx", asset.sha256) =>
            {
                urls.push(format!("{ORIGIN}{path}"));
            }
            Source::Github { url } if url.starts_with("https://github.com/") => urls.push(url),
            _ => {}
        }
    }
    Ok(Request {
        urls,
        filename: asset.filename,
        sha256: asset.sha256,
        size: asset.size,
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..")
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

fn decode<T: serde::de::DeserializeOwned>(data: &[u8]) -> Result<T, String> {
    serde_json::from_slice(data).map_err(|error| format!("插件目录 JSON 无效：{error}"))
}

async fn fetch(client: &reqwest::Client, path: &str, limit: usize) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(format!("{ORIGIN}{path}"))
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if bytes.len() + chunk.len() > limit {
            return Err("插件目录响应超过大小限制".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
