use crate::config::config::read_config;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tauri::State;

// [基岩版 Game ID]
const GAME_ID: &str = "78022";
const CURSEFORGE_OFFICIAL_BASE: &str = "https://api.curseforge.com";
const CURSEFORGE_MIRROR_BASE: &str = "https://mod.mcimirror.top/curseforge";

// [注意] 请替换为你的有效 API Key
const API_KEY: &str = "$2a$10$3Dr/WLO28GST4n7h7vD0zeWNPjIbwqb1cyVsL66BXAfliCpBC5Ejm";

// --- 客户端封装 ---

pub struct CurseForgeClient {
    pub client: Client,
}

impl CurseForgeClient {
    pub fn new() -> Self {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-api-key", reqwest::header::HeaderValue::from_static(API_KEY));
        let client = Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap();
        Self { client }
    }

    pub async fn get<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T, String> {
        let res = self.client.get(url).send().await.map_err(|e| e.to_string())?;

        if !res.status().is_success() {
            return Err(format!("API Error: Status {}", res.status()));
        }

        let text = res.text().await.map_err(|e| e.to_string())?;

        match serde_json::from_str::<T>(&text) {
            Ok(data) => Ok(data),
            Err(e) => {
                println!("CurseForge Decode Error: {}", e);
                // 打印出出错的 JSON 片段以便调试 (截取前 500 字符)
                println!("Response Body (Snippet): {:.500}", text);
                Err(format!("JSON Decode Error: {} | URL: {}", e, url))
            }
        }
    }
}

fn build_base_urls() -> (String, String) {
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
    (
        format!("{}/v1", base),
        format!("{}/v2", base),
    )
}

// --- 数据模型 ---

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

    #[serde(default)]
    pub latest_files: Vec<CurseForgeFile>,
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
    pub thumbnail_url: Option<String>
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Author { pub name: String }

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

// --- 响应结构 ---

#[derive(Debug, Serialize, Deserialize, Clone)]
struct SearchResponse {
    pub data: Vec<CurseForgeMod>,
    pub pagination: Option<Pagination>,
}
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Pagination {
    pub result_count: u32,
    pub total_count: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetCategoriesResponse { pub data: Vec<Category> }

// [调试修改] 使用 Value 接收 versions，防止类型错误
#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetVersionsV2Response {
    pub data: Vec<GameVersionTypeV2>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct GameVersionTypeV2 {
    #[serde(rename = "type")]
    pub type_id: i32,
    #[serde(default)]
    pub versions: Vec<serde_json::Value>, // 改为 Value 以兼容 String 或 Object
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetModResponse { pub data: CurseForgeMod }
#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetStringResponse { pub data: String }
#[derive(Debug, Serialize, Deserialize, Clone)]
struct GetModFilesResponse { pub data: Vec<CurseForgeFile>, pub pagination: Option<Pagination> }

// --- 辅助函数 ---

fn version_compare(v1: &str, v2: &str) -> std::cmp::Ordering {
    let parts1: Vec<&str> = v1.split('.').collect();
    let parts2: Vec<&str> = v2.split('.').collect();
    let len = std::cmp::max(parts1.len(), parts2.len());

    for i in 0..len {
        let p1 = parts1.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let p2 = parts2.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        if p1 != p2 {
            return p1.cmp(&p2);
        }
    }
    std::cmp::Ordering::Equal
}

// --- Tauri 命令 ---

#[tauri::command]
pub async fn get_curseforge_categories(state: State<'_, CurseForgeClient>) -> Result<Vec<Category>, String> {
    let (base_v1, _) = build_base_urls();
    let url = format!("{}/categories?gameId={}", base_v1, GAME_ID);
    let res = state.get::<GetCategoriesResponse>(&url).await?;
    Ok(res.data)
}

// [修正] 获取版本列表
#[tauri::command]
pub async fn get_minecraft_versions(state: State<'_, CurseForgeClient>) -> Result<Vec<String>, String> {
    let (_, base_v2) = build_base_urls();
    let url = format!("{}/games/{}/versions", base_v2, GAME_ID);
    let res = state.get::<GetVersionsV2Response>(&url).await?;

    // 手动处理 versions 字段
    let mut versions: Vec<String> = res.data.into_iter()
        .flat_map(|v| v.versions)
        .filter_map(|val| {
            // 尝试直接作为字符串
            if let Some(s) = val.as_str() {
                return Some(s.to_string());
            }
            // 如果是对象，尝试提取 "name" 或 "versionString"
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

#[tauri::command]
pub async fn search_curseforge_mods(
    state: State<'_, CurseForgeClient>,
    class_id: Option<i32>,
    category_id: Option<i32>,
    game_version: Option<String>,
    search_filter: Option<String>,
    sort_field: Option<i32>,
    sort_order: Option<String>,
    page_size: Option<u32>,
    index: Option<u32>
) -> Result<Vec<CurseForgeMod>, String> {
    let (base_v1, _) = build_base_urls();
    let mut params = Vec::new();
    params.push(("gameId", GAME_ID.to_string()));

    if let Some(cid) = class_id { params.push(("classId", cid.to_string())); }
    if let Some(cat_id) = category_id { params.push(("categoryId", cat_id.to_string())); }

    if let Some(ver) = game_version {
        if !ver.is_empty() && ver != "all" {
            params.push(("gameVersion", ver));
        }
    }

    if let Some(search) = search_filter { if !search.is_empty() { params.push(("searchFilter", search)); } }
    if let Some(sort) = sort_field { params.push(("sortField", sort.to_string())); }
    if let Some(order) = sort_order { params.push(("sortOrder", order)); }
    params.push(("pageSize", page_size.unwrap_or(20).to_string()));
    params.push(("index", index.unwrap_or(0).to_string()));

    let url = reqwest::Url::parse_with_params(&format!("{}/mods/search", base_v1), &params).map_err(|e| e.to_string())?;
    let res = state.get::<SearchResponse>(url.as_str()).await?;
    Ok(res.data)
}

#[tauri::command]
pub async fn get_curseforge_mod(state: State<'_, CurseForgeClient>, mod_id: i32) -> Result<CurseForgeMod, String> {
    let (base_v1, _) = build_base_urls();
    let url = format!("{}/mods/{}", base_v1, mod_id);
    let res = state.get::<GetModResponse>(&url).await?;
    Ok(res.data)
}

#[tauri::command]
pub async fn get_curseforge_mod_description(state: State<'_, CurseForgeClient>, mod_id: i32) -> Result<String, String> {
    let (base_v1, _) = build_base_urls();
    let url = format!("{}/mods/{}/description", base_v1, mod_id);
    let res = state.get::<GetStringResponse>(&url).await?;
    Ok(res.data)
}
#[tauri::command]
pub async fn get_curseforge_mod_files(
    state: State<'_, CurseForgeClient>,
    mod_id: i32,
    game_version: Option<String>
) -> Result<Vec<CurseForgeFile>, String> {
    let (base_v1, _) = build_base_urls();
    let mut params = Vec::new();
    if let Some(ver) = game_version { if !ver.is_empty() && ver != "all" { params.push(("gameVersion", ver)); } }
    params.push(("pageSize", "50".to_string()));
    let url = reqwest::Url::parse_with_params(&format!("{}/mods/{}/files", base_v1, mod_id), &params).map_err(|e| e.to_string())?;
    let res = state.get::<GetModFilesResponse>(url.as_str()).await?;
    Ok(res.data)
}
