use crate::config::config::read_config;
use crate::http::proxy::get_client_for_proxy;
use crate::http::request::{GLOBAL_CLIENT, RequestOptions, send_request_with_options};
use crate::utils::file_ops;
use anyhow::{Context as _, Result};
use futures_util::StreamExt as _;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt as _;
use tokio::time::sleep;
use tracing::debug;

const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 12);
const CACHE_FILE_NAME: &str = "appx_api_cache.json";
const CACHE_BACKUP_COUNT: usize = 3;
const CACHE_SCHEMA_VERSION: u32 = 2;
const REMOTE_VERSIONS_MAX_ATTEMPTS: usize = 3;
const REMOTE_VERSIONS_RETRY_DELAY_MS: u64 = 250;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMinecraftVersion {
    pub version: String,
    pub package_id: String,
    pub version_type: i32,
    pub version_type_str: String,
    pub build_type: String,
    pub archival_status: Option<i32>,
    pub meta_present: bool,
    pub md5: Option<String>,
    pub is_gdk: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CacheFile {
    #[serde(default)]
    schema_version: u32,
    ts_unix_ms: u64,
    creation_time: Option<String>,
    versions: Vec<RemoteMinecraftVersion>,
}

fn cache_path() -> PathBuf {
    file_ops::cache_subdir("api").join(CACHE_FILE_NAME)
}

fn legacy_cache_path() -> PathBuf {
    file_ops::bmcbl_subdir("cache").join(CACHE_FILE_NAME)
}

fn cache_backup_path(index: usize) -> PathBuf {
    file_ops::cache_subdir("api").join(format!("appx_api_cache.{}.json", index))
}

fn legacy_cache_backup_path(index: usize) -> PathBuf {
    file_ops::bmcbl_subdir("cache").join(format!("appx_api_cache.{}.json", index))
}

fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn read_cache() -> Option<CacheFile> {
    fn read_one(path: &PathBuf) -> Option<CacheFile> {
        let raw = fs::read_to_string(path).ok()?;
        let cache: CacheFile = serde_json::from_str(&raw).ok()?;
        (cache.schema_version == CACHE_SCHEMA_VERSION).then_some(cache)
    }

    let path = cache_path();
    if let Some(v) = read_one(&path) {
        return Some(v);
    }

    let legacy = legacy_cache_path();
    if let Some(v) = read_one(&legacy) {
        return Some(v);
    }

    for i in 1..=CACHE_BACKUP_COUNT {
        let backup = cache_backup_path(i);
        if let Some(v) = read_one(&backup) {
            return Some(v);
        }
    }

    for i in 1..=CACHE_BACKUP_COUNT {
        let backup = legacy_cache_backup_path(i);
        if let Some(v) = read_one(&backup) {
            return Some(v);
        }
    }

    None
}

fn rotate_cache_files() {
    if CACHE_BACKUP_COUNT == 0 {
        return;
    }

    let path = cache_path();

    let last = cache_backup_path(CACHE_BACKUP_COUNT);
    let _ = fs::remove_file(&last);

    for i in (1..CACHE_BACKUP_COUNT).rev() {
        let src = cache_backup_path(i);
        let dst = cache_backup_path(i + 1);
        if src.exists() {
            let _ = fs::rename(src, dst);
        }
    }

    if path.exists() {
        let _ = fs::rename(&path, cache_backup_path(1));
    }
}

fn write_cache(cache: &CacheFile) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let Ok(raw) = serde_json::to_string(cache) else {
        return;
    };

    rotate_cache_files();

    // Best-effort atomic replace on Windows: write tmp, then rename into place.
    let tmp = path.with_extension("json.tmp");
    if fs::write(&tmp, raw).is_ok() {
        let _ = fs::remove_file(&path);
        let _ = fs::rename(tmp, path);
    }
}

fn parse_version_to_vec_simple(v: &str) -> Vec<u64> {
    v.split(|c| c == '.' || c == '-' || c == '+')
        .map(|seg| {
            let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

fn compare_versions_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let va = parse_version_to_vec_simple(a);
    let vb = parse_version_to_vec_simple(b);
    let n = std::cmp::max(va.len(), vb.len());
    for i in 0..n {
        let ai = *va.get(i).unwrap_or(&0);
        let bi = *vb.get(i).unwrap_or(&0);
        match bi.cmp(&ai) {
            std::cmp::Ordering::Equal => continue,
            non_eq => return non_eq,
        }
    }
    std::cmp::Ordering::Equal
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteItem {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    build_type: String,
    #[serde(default)]
    archival_status: Option<i32>,
    #[serde(default, rename = "ID", alias = "Id", alias = "id")]
    id: String,
    #[serde(default)]
    variations: Vec<RemoteVariation>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RemoteVariation {
    #[serde(default)]
    arch: String,
    #[serde(default)]
    archival_status: Option<i32>,
    #[serde(default, rename = "MD5", alias = "Md5", alias = "md5")]
    md5: Option<String>,
    #[serde(default)]
    meta_data: Option<RemoteMetaData>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum RemoteMetaData {
    String(String),
    Array(Vec<String>),
    Other(Value),
}

fn pick_x64_variation<'a>(vars: &'a [RemoteVariation]) -> Option<&'a RemoteVariation> {
    vars.iter()
        .find(|v| v.arch.eq_ignore_ascii_case("x64"))
        .or_else(|| vars.first())
}

fn meta_to_first_string(meta: &Option<RemoteMetaData>) -> Option<&str> {
    match meta.as_ref()? {
        RemoteMetaData::String(s) => Some(s.as_str()),
        RemoteMetaData::Array(a) => a.first().map(|s| s.as_str()),
        RemoteMetaData::Other(v) => {
            if let Some(arr) = v.as_array() {
                return arr.first().and_then(|v| v.as_str());
            }
            v.as_str()
        }
    }
}

fn normalize_md5(md5: &Option<String>) -> Option<String> {
    let value = md5.as_deref()?.trim();
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Some(value.to_ascii_lowercase())
    } else {
        None
    }
}

fn push_remote_version(
    version_key: String,
    item: RemoteItem,
    versions: &mut Vec<RemoteMinecraftVersion>,
) {
    if item.variations.is_empty() && item.id.trim().is_empty() {
        return;
    }

    let type_str = item.r#type.clone();
    let version_type = match type_str.as_str() {
        "Release" => 0,
        "Beta" => 1,
        _ => 2,
    };

    let build_type = item.build_type.clone();
    let is_gdk = build_type.eq_ignore_ascii_case("gdk");

    let Some(variation) = pick_x64_variation(&item.variations) else {
        return;
    };

    let archival_status = variation.archival_status.or(item.archival_status);
    let md5 = normalize_md5(&variation.md5);

    let meta_first = meta_to_first_string(&variation.meta_data)
        .unwrap_or("")
        .to_string();
    let meta_present = !meta_first.trim().is_empty();

    let mut package_id = meta_first;
    if package_id.trim().is_empty() {
        package_id = md5.clone().unwrap_or_default();
    }
    if package_id.trim().is_empty() {
        package_id = item.id.clone();
    }
    if package_id.trim().is_empty() {
        return;
    }

    versions.push(RemoteMinecraftVersion {
        version: version_key,
        package_id,
        version_type,
        version_type_str: type_str,
        build_type,
        archival_status,
        meta_present,
        md5,
        is_gdk,
    });
}

struct Parsed {
    creation_time: Option<String>,
    versions: Vec<RemoteMinecraftVersion>,
}

impl<'de> Deserialize<'de> for Parsed {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{IgnoredAny, MapAccess, Visitor};
        use std::fmt;

        struct V;

        impl<'de> Visitor<'de> for V {
            type Value = Parsed;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a map")
            }

            fn visit_map<A>(self, mut map: A) -> std::result::Result<Parsed, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut creation_time: Option<String> = None;
                let mut versions: Vec<RemoteMinecraftVersion> = Vec::new();

                while let Some(key) = map.next_key::<String>()? {
                    if key == "CreationTime" {
                        creation_time = map.next_value::<Option<String>>()?;
                        continue;
                    }

                    // Newer API schema wraps versions inside an extra object (e.g. "From_mcappx.com").
                    // If the key doesn't look like a version number, treat it as a container map and
                    // try to parse its entries as version->RemoteItem.
                    let key_looks_like_version =
                        key.chars().next().is_some_and(|c| c.is_ascii_digit());

                    if !key_looks_like_version {
                        let value: Value = map.next_value()?;
                        if let Some(obj) = value.as_object() {
                            for (inner_key, inner_value) in obj {
                                let Ok(item) =
                                    serde_json::from_value::<RemoteItem>(inner_value.clone())
                                else {
                                    continue;
                                };
                                push_remote_version(inner_key.to_string(), item, &mut versions);
                            }
                        }
                        continue;
                    }

                    // Parse each entry directly and discard it immediately to keep peak memory
                    // usage low (avoids building a huge serde_json::Value tree).
                    let item = match map.next_value::<RemoteItem>() {
                        Ok(v) => v,
                        Err(_) => {
                            let _: IgnoredAny = map.next_value()?;
                            continue;
                        }
                    };

                    push_remote_version(key, item, &mut versions);
                }

                versions.sort_by(|a, b| compare_versions_desc(&a.version, &b.version));
                Ok(Parsed {
                    creation_time,
                    versions,
                })
            }
        }

        deserializer.deserialize_map(V)
    }
}

fn parse_api_body_streaming(body: &str) -> Result<(Option<String>, Vec<RemoteMinecraftVersion>)> {
    let mut de = serde_json::Deserializer::from_str(body);
    let parsed = Parsed::deserialize(&mut de).context("invalid json response")?;
    Ok((parsed.creation_time, parsed.versions))
}

fn parse_api_reader_streaming<R: std::io::Read>(
    reader: R,
) -> Result<(Option<String>, Vec<RemoteMinecraftVersion>)> {
    let mut de = serde_json::Deserializer::from_reader(reader);
    let parsed = Parsed::deserialize(&mut de).context("invalid json response")?;
    Ok((parsed.creation_time, parsed.versions))
}

async fn load_or_fetch_versions_once(force_refresh: bool) -> Result<Vec<RemoteMinecraftVersion>> {
    if !force_refresh {
        if let Some(cache) = read_cache() {
            let age = Duration::from_millis(unix_now_ms().saturating_sub(cache.ts_unix_ms));
            if age <= CACHE_TTL && !cache.versions.is_empty() {
                return Ok(cache.versions);
            }
        }
    }

    let cfg = read_config().unwrap_or_else(|_| crate::config::config::get_default_config());
    let api = if cfg.launcher.custom_appx_api.trim().is_empty() {
        crate::config::config::get_default_config()
            .launcher
            .custom_appx_api
    } else {
        cfg.launcher.custom_appx_api
    };
    let url = Url::parse(&api).with_context(|| format!("invalid custom_appx_api url: {api}"))?;

    let client = get_client_for_proxy().unwrap_or_else(|e| {
        debug!("proxy client build failed, using global client: {e:?}");
        GLOBAL_CLIENT.clone()
    });

    let mut headers = HashMap::new();
    headers.insert("Accept".to_string(), "application/json, text/*".to_string());

    let opts = RequestOptions {
        method: "GET",
        headers: Some(&headers),
        timeout_ms: Some(20_000),
        allow_redirects: Some(true),
    };

    let resp = send_request_with_options(&client, &url, &opts)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;
    let resp = resp
        .error_for_status()
        .context("remote versions api returned error status")?;

    let tmp_path = file_ops::cache_subdir("data").join("appx_versions.json.tmp");
    if let Some(parent) = tmp_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("create cache data dir failed")?;
    }

    {
        let mut file = tokio::fs::File::create(&tmp_path)
            .await
            .context("create temp api body file failed")?;
        let mut stream = resp.bytes_stream();
        while let Some(next) = stream.next().await {
            let chunk = next.context("read api body chunk failed")?;
            file.write_all(&chunk)
                .await
                .context("write api body chunk failed")?;
        }
        file.flush().await.ok();
    }

    let tmp_path_for_parse = tmp_path.clone();
    let (creation_time, versions) = tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&tmp_path_for_parse).with_context(|| {
            format!(
                "open temp api body file failed: {}",
                tmp_path_for_parse.display()
            )
        })?;
        let reader = std::io::BufReader::new(file);
        parse_api_reader_streaming(reader)
    })
    .await
    .context("parse remote versions join failed")??;

    let _ = tokio::fs::remove_file(&tmp_path).await;

    if versions.is_empty() {
        return Err(anyhow::anyhow!(
            "remote versions api parsed 0 entries (schema may have changed)"
        ));
    }

    write_cache(&CacheFile {
        schema_version: CACHE_SCHEMA_VERSION,
        ts_unix_ms: unix_now_ms(),
        creation_time,
        versions: versions.clone(),
    });

    Ok(versions)
}

pub async fn load_or_fetch_versions(force_refresh: bool) -> Result<Vec<RemoteMinecraftVersion>> {
    let mut last_error = None;

    for attempt in 0..REMOTE_VERSIONS_MAX_ATTEMPTS {
        match load_or_fetch_versions_once(force_refresh).await {
            Ok(versions) => return Ok(versions),
            Err(error) => {
                last_error = Some(error);
                if attempt + 1 < REMOTE_VERSIONS_MAX_ATTEMPTS {
                    sleep(Duration::from_millis(REMOTE_VERSIONS_RETRY_DELAY_MS)).await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("remote versions load failed")))
}

#[cfg(test)]
mod tests {
    use super::parse_api_body_streaming;

    #[test]
    fn parses_uppercase_md5_and_id_from_mcappx_schema() {
        let body = r#"{
            "CreationTime": "2026-06-02T18:49:12.948379+00:00",
            "From_mcappx.com": {
                "1.21.93": {
                    "BuildType": "UWP",
                    "ID": "1.21.9301",
                    "Type": "Release",
                    "Variations": [
                        {
                            "Arch": "x64",
                            "ArchivalStatus": 3,
                            "MD5": "F0D6CB8024BA725D60A0520F13CCAC49",
                            "MetaData": ["9a1e10b3-e8e1-4d01-a2c0-c3ecac48fd13"],
                            "OSbuild": "19041"
                        }
                    ]
                }
            }
        }"#;

        let (_creation_time, versions) =
            parse_api_body_streaming(body).expect("mcappx schema should parse");
        let version = versions
            .iter()
            .find(|version| version.version == "1.21.93")
            .expect("version should be present");

        assert_eq!(version.package_id, "9a1e10b3-e8e1-4d01-a2c0-c3ecac48fd13");
        assert_eq!(
            version.md5.as_deref(),
            Some("f0d6cb8024ba725d60a0520f13ccac49")
        );
    }
}
