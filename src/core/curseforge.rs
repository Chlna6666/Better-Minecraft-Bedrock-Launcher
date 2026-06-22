pub mod queries;

use crate::config::config::read_config;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;
use std::time::Duration;
use url::Url;

// Bedrock Game ID (CurseForge).
const GAME_ID: &str = "78022";
const CURSEFORGE_OFFICIAL_BASE: &str = "https://api.curseforge.com";
const CURSEFORGE_MIRROR_BASE: &str = "https://mod.mcimirror.top/curseforge";

// NOTE: This matches the upstream web/tauri implementation. Prefer configuring a proxy/base URL
// over changing this constant.
const API_KEY: &str = "$2a$10$3Dr/WLO28GST4n7h7vD0zeWNPjIbwqb1cyVsL66BXAfliCpBC5Ejm";
const CURSEFORGE_API_MAX_ATTEMPTS: usize = 3;
const CURSEFORGE_API_RETRY_DELAY_MS: u64 = 200;

#[derive(Clone)]
pub struct CurseForgeClient {
    client: Client,
    base_v1: String,
    base_v2: String,
}

impl CurseForgeClient {
    pub fn new() -> Result<Self, String> {
        let client = build_http_client()?.clone();
        let (base_v1, base_v2) = build_base_urls();
        Ok(Self {
            client,
            base_v1,
            base_v2,
        })
    }

    async fn get<T: for<'de> Deserialize<'de>>(&self, url: Url) -> Result<T, String> {
        let mut last_error = None;

        for attempt in 0..CURSEFORGE_API_MAX_ATTEMPTS {
            match self.client.get(url.clone()).send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        return response
                            .json::<T>()
                            .await
                            .map_err(|error| error.to_string());
                    }

                    let status = response.status();
                    if attempt + 1 < CURSEFORGE_API_MAX_ATTEMPTS && should_retry_status(status) {
                        last_error = Some(format!("API Error: Status {}", status));
                        tokio::time::sleep(Duration::from_millis(CURSEFORGE_API_RETRY_DELAY_MS))
                            .await;
                        continue;
                    }

                    return Err(format!("API Error: Status {}", status));
                }
                Err(error) => {
                    let error_message = error.to_string();
                    if attempt + 1 < CURSEFORGE_API_MAX_ATTEMPTS {
                        last_error = Some(error_message);
                        tokio::time::sleep(Duration::from_millis(CURSEFORGE_API_RETRY_DELAY_MS))
                            .await;
                        continue;
                    }

                    return Err(error_message);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| "API request failed".to_string()))
    }

    pub async fn get_categories(&self) -> Result<Vec<Category>, String> {
        let mut url =
            Url::parse(&format!("{}/categories", self.base_v1)).map_err(|e| e.to_string())?;
        url.query_pairs_mut().append_pair("gameId", GAME_ID);
        let response = self.get::<GetCategoriesResponse>(url).await?;
        Ok(response.data)
    }

    pub async fn get_minecraft_versions(&self) -> Result<Vec<String>, String> {
        let url_primary = Url::parse(&format!("{}/games/{}/versions", self.base_v2, GAME_ID))
            .map_err(|e| e.to_string())?;
        let response = match self.get::<GetVersionsV2Response>(url_primary).await {
            Ok(response) => response,
            Err(err) => {
                // Some mirrors do not implement the v2 versions endpoint.
                // Fall back to the official API for this list only.
                if err.contains("Status 404") {
                    let url_official = Url::parse(&format!(
                        "{}/v2/games/{}/versions",
                        CURSEFORGE_OFFICIAL_BASE, GAME_ID
                    ))
                    .map_err(|e| e.to_string())?;
                    match self.get::<GetVersionsV2Response>(url_official).await {
                        Ok(response) => response,
                        Err(_) => return Ok(Vec::new()),
                    }
                } else {
                    return Err(err);
                }
            }
        };

        let mut versions: Vec<String> = response
            .data
            .into_iter()
            .flat_map(|v| v.versions)
            .filter_map(|val| {
                if let Some(s) = val.as_str() {
                    return Some(s.to_string());
                }
                if let Some(obj) = val.as_object() {
                    if let Some(s) = obj.get("name").and_then(|v| v.as_str()) {
                        return Some(s.to_string());
                    }
                    if let Some(s) = obj.get("versionString").and_then(|v| v.as_str()) {
                        return Some(s.to_string());
                    }
                }
                None
            })
            .collect();

        versions.sort();
        versions.dedup();
        versions.sort_by(|a, b| version_compare(b, a));
        Ok(versions)
    }

    pub async fn search_mods(&self, query: SearchModsQuery) -> Result<SearchResponse, String> {
        let mut params: Vec<(String, String)> = Vec::new();
        params.push(("gameId".to_string(), GAME_ID.to_string()));
        if let Some(class_id) = query.class_id {
            params.push(("classId".to_string(), class_id.to_string()));
        }
        if let Some(category_id) = query.category_id {
            params.push(("categoryId".to_string(), category_id.to_string()));
        }
        if let Some(game_version) = query.game_version {
            if !game_version.is_empty() && game_version != "all" {
                params.push(("gameVersion".to_string(), game_version));
            }
        }
        if let Some(search_filter) = query.search_filter {
            if !search_filter.trim().is_empty() {
                params.push(("searchFilter".to_string(), search_filter));
            }
        }
        if let Some(sort_field) = query.sort_field {
            params.push(("sortField".to_string(), sort_field.to_string()));
        }
        if let Some(sort_order) = query.sort_order {
            params.push(("sortOrder".to_string(), sort_order));
        }
        params.push((
            "pageSize".to_string(),
            query.page_size.unwrap_or(20).to_string(),
        ));
        params.push(("index".to_string(), query.index.unwrap_or(0).to_string()));

        let url = Url::parse_with_params(
            &format!("{}/mods/search", self.base_v1),
            params.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .map_err(|e| e.to_string())?;
        self.get::<SearchResponse>(url).await
    }

    pub async fn get_mod_files(
        &self,
        mod_id: i32,
        game_version: Option<String>,
    ) -> Result<GetModFilesResponse, String> {
        let mut params: Vec<(String, String)> = Vec::new();
        if let Some(ver) = game_version {
            if !ver.is_empty() && ver != "all" {
                params.push(("gameVersion".to_string(), ver));
            }
        }
        params.push(("pageSize".to_string(), "50".to_string()));

        let url = Url::parse_with_params(
            &format!("{}/mods/{}/files", self.base_v1, mod_id),
            params.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        )
        .map_err(|e| e.to_string())?;
        self.get::<GetModFilesResponse>(url).await
    }

    pub async fn get_mod(&self, mod_id: i32) -> Result<CurseForgeMod, String> {
        let url =
            Url::parse(&format!("{}/mods/{}", self.base_v1, mod_id)).map_err(|e| e.to_string())?;
        let response = self.get::<GetModResponse>(url).await?;
        Ok(response.data)
    }

    pub async fn get_mod_description(&self, mod_id: i32) -> Result<String, String> {
        let url = Url::parse(&format!("{}/mods/{}/description", self.base_v1, mod_id))
            .map_err(|e| e.to_string())?;
        let response = self.get::<GetStringResponse>(url).await?;
        Ok(response.data)
    }
}

#[derive(Default, Clone, Debug)]
pub struct SearchModsQuery {
    pub class_id: Option<i32>,
    pub category_id: Option<i32>,
    pub game_version: Option<String>,
    pub search_filter: Option<String>,
    pub sort_field: Option<i32>,
    pub sort_order: Option<String>,
    pub page_size: Option<u32>,
    pub index: Option<u32>,
}

fn build_http_client() -> Result<&'static Client, String> {
    static CLIENT: OnceLock<Result<Client, String>> = OnceLock::new();

    CLIENT
        .get_or_init(|| {
            let mut headers = reqwest::header::HeaderMap::new();
            headers.insert(
                "x-api-key",
                reqwest::header::HeaderValue::from_static(API_KEY),
            );

            Client::builder()
                .default_headers(headers)
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(Clone::clone)
}

fn should_retry_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn build_base_urls() -> (String, String) {
    static BASE_URLS: OnceLock<(String, String)> = OnceLock::new();
    BASE_URLS.get_or_init(build_base_urls_uncached).clone()
}

fn build_base_urls_uncached() -> (String, String) {
    let (source, custom_base) = read_config()
        .ok()
        .map(|cfg| {
            (
                cfg.launcher.download.curseforge_api_source,
                cfg.launcher.download.curseforge_api_base,
            )
        })
        .unwrap_or_default();

    let source = source.trim().to_lowercase();
    let base = match source.as_str() {
        "official" => CURSEFORGE_OFFICIAL_BASE,
        "mirror" => CURSEFORGE_MIRROR_BASE,
        "custom" => {
            let trimmed = custom_base.trim().trim_end_matches('/');
            if trimmed.is_empty() {
                CURSEFORGE_MIRROR_BASE
            } else {
                trimmed
            }
        }
        _ => CURSEFORGE_MIRROR_BASE,
    };
    (format!("{}/v1", base), format!("{}/v2", base))
}

fn version_compare(v1: &str, v2: &str) -> std::cmp::Ordering {
    let parts1: Vec<&str> = v1.split('.').collect();
    let parts2: Vec<&str> = v2.split('.').collect();
    let len = std::cmp::max(parts1.len(), parts2.len());

    for i in 0..len {
        let p1 = parts1
            .get(i)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let p2 = parts2
            .get(i)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if p1 != p2 {
            return p1.cmp(&p2);
        }
    }
    std::cmp::Ordering::Equal
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeMod {
    pub id: i32,
    pub game_id: i32,
    pub name: String,
    pub slug: String,
    pub summary: Option<String>,
    pub download_count: f64,
    pub logo: Option<Logo>,

    #[serde(default)]
    pub authors: Vec<Author>,

    #[serde(default)]
    pub categories: Vec<Category>,

    pub class_id: Option<i32>,
    pub date_modified: String,
    pub date_created: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CurseForgeFile {
    pub id: i32,
    pub display_name: String,
    pub file_name: String,
    pub file_length: u64,
    pub download_url: Option<String>,

    #[serde(default)]
    pub game_versions: Vec<String>,

    pub file_date: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Logo {
    pub url: String,
    pub thumbnail_url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Author {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Category {
    pub id: i32,
    pub name: String,
    pub slug: String,
    pub icon_url: Option<String>,
    pub is_class: Option<bool>,
    pub class_id: Option<i32>,
    pub parent_category_id: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    pub result_count: u32,
    pub total_count: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SearchResponse {
    pub data: Vec<CurseForgeMod>,
    pub pagination: Option<Pagination>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GetModFilesResponse {
    pub data: Vec<CurseForgeFile>,
    pub pagination: Option<Pagination>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetModResponse {
    pub data: CurseForgeMod,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetStringResponse {
    pub data: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetCategoriesResponse {
    pub data: Vec<Category>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetVersionsV2Response {
    pub data: Vec<GameVersionTypeV2>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameVersionTypeV2 {
    #[serde(rename = "type")]
    pub type_id: i32,
    #[serde(default)]
    pub versions: Vec<serde_json::Value>,
}
