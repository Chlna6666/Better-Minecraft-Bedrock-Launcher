use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs as tokio_fs;
use std::time::Instant;
use tracing::debug;

/// ---------- 结构化 manifest 类型（按你的示例与常见字段建模） ----------
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Header {
    pub description: Option<String>,
    pub name: Option<String>,
    pub uuid: Option<String>,
    pub version: Option<Vec<u32>>,
    pub min_engine_version: Option<Vec<u32>>,
    pub base_game_version: Option<Vec<u32>>,
    pub lock_template_options: Option<bool>,
    pub pack_scope: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Module {
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub module_type: Option<String>,
    pub uuid: Option<String>,
    pub version: Option<Vec<u32>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Subpack {
    pub folder_name: Option<String>,
    pub name: Option<String>,
    pub memory_tier: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Dependency {
    pub uuid: Option<String>,
    pub version: Option<Vec<u32>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Metadata {
    pub authors: Option<Vec<String>>,
    pub license: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Manifest {
    pub format_version: Option<u32>,
    pub header: Option<Header>,
    pub modules: Option<Vec<Module>>,
    pub subpacks: Option<Vec<Subpack>>,
    pub dependencies: Option<Vec<Dependency>>,
    pub capabilities: Option<Value>, // 保留为 Value，灵活处理
    pub metadata: Option<Metadata>,
    // 你可以按需继续添加字段
}

/// ---------- 返回给前端的资源包信息结构 ----------
/// 通用的 McPackInfo 结构
#[derive(Debug, Serialize)]
pub struct McPackInfo {
    pub folder_name: String,
    pub folder_path: String,
    pub manifest: Value,                   // 解析后的 Value（原始 JSON）
    pub manifest_raw: String,              // 原始 json 文本
    pub manifest_parsed: Option<Manifest>, // 尝试解析成结构化 Manifest
    pub icon_path: Option<String>,         // 如果存在 pack_icon.png
    pub icon_rel: Option<String>,          // 相对 resource_packs 的路径 (比如 "ThreeD-.../pack_icon.png")
    pub short_description: Option<String>,
}

/// 获取 Windows 上 Minecraft UWP resource_packs 路径（若非 Windows 可自行调整）
fn default_minecraft_resource_packs_path() -> Option<PathBuf> {
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(local_appdata)
            .join("Packages")
            .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
            .join("LocalState")
            .join("games")
            .join("com.mojang")
            .join("resource_packs");
        return Some(p);
    }
    None
}

/// 获取 Windows 上 Minecraft UWP behavior_packs 路径（若非 Windows 可自行调整）
fn default_minecraft_behavior_packs_path() -> Option<PathBuf> {
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(local_appdata)
            .join("Packages")
            .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
            .join("LocalState")
            .join("games")
            .join("com.mojang")
            .join("behavior_packs");
        return Some(p);
    }
    None
}

/// 清理并截断描述（与之前功能一致）
fn short_desc_from_opt(raw: Option<String>, max_chars: usize) -> Option<String> {
    raw.map(|desc| {
        let cleaned: String = desc.chars().filter(|c| !c.is_control()).collect();
        if cleaned.chars().count() > max_chars {
            let s: String = cleaned.chars().take(max_chars).collect();
            format!("{}...", s)
        } else {
            cleaned
        }
    })
}

/// 核心异步函数：列出目录（async），然后并行解析每个子目录（rayon）
pub async fn read_all_resource_packs() -> Result<Vec<McPackInfo>> {
    let start = Instant::now();
    let resource_packs_dir = default_minecraft_resource_packs_path()
        .context("无法确定 resource_packs 路径 (LOCALAPPDATA 未设置)")?;

    if !resource_packs_dir.exists() {
        return Ok(Vec::new());
    }

    // 异步列出子目录（仅收集路径）
    let mut rd = tokio_fs::read_dir(&resource_packs_dir)
        .await
        .with_context(|| format!("无法打开目录 {}", resource_packs_dir.display()))?;

    let mut folders: Vec<PathBuf> = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let p = entry.path();
        if p.is_dir() {
            folders.push(p);
        }
    }

    // 使用 rayon 并行处理每个文件夹（在 rayon 线程里使用 std::fs）
    let results: Vec<McPackInfo> = folders
        .into_par_iter()
        .filter_map(|folder_path: PathBuf| {
            let folder_name = folder_path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| folder_path.to_string_lossy().to_string());

            let manifest_path = folder_path.join("manifest.json");
            if !manifest_path.exists() {
                return None;
            }

            // 读取 manifest.json（阻塞，但在 rayon 线程池中是 ok）
            let manifest_raw = match fs::read_to_string(&manifest_path) {
                Ok(s) => s,
                Err(_) => return None,
            };

            // 解析 json（若解析失败，仍保留原始内容并把 parsed 设为 None）
            let manifest_value: Value = match serde_json::from_str(&manifest_raw) {
                Ok(v) => v,
                Err(_) => return None,
            };

            let manifest_parsed: Option<Manifest> = match serde_json::from_str(&manifest_raw) {
                Ok(m) => Some(m),
                Err(_) => None,
            };

            // short description: 优先从已解析的结构体 header.description，fallback 为 None
            let short_description = manifest_parsed
                .as_ref()
                .and_then(|m| m.header.as_ref().and_then(|h| h.description.clone()))
                .or_else(|| {
                    manifest_value
                        .get("header")
                        .and_then(|hdr| hdr.get("description"))
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                })
                .and_then(|s| short_desc_from_opt(Some(s), 50));

            let base = Arc::new(resource_packs_dir.clone());
            // pack_icon.png 是否存在
            let icon_abs = {
                let p = folder_path.join("pack_icon.png");
                if p.exists() {
                    Some(p.to_string_lossy().to_string()) // 绝对路径，不带 file://
                } else {
                    None
                }
            };

            // 计算相对于 resource_packs 根的相对路径（用于 asset server）
            let icon_rel = icon_abs.as_ref().and_then(|_| {
                match folder_path.strip_prefix(&*base) {
                    Ok(rel) => {
                        let mut rp = rel.to_path_buf();
                        rp.push("pack_icon.png");
                        Some(rp.to_string_lossy().to_string())
                    }
                    Err(_) => None,
                }
            });

            Some(McPackInfo {
                folder_name,
                folder_path: folder_path.to_string_lossy().to_string(),
                manifest: manifest_value,
                manifest_raw,
                manifest_parsed,
                icon_path: icon_abs,
                icon_rel,
                short_description,
            })
        })
        .collect();

   debug!("Finished reading resource packs in {:?}", start.elapsed());
    Ok(results)
}

/// 核心异步函数：列出 behavior_packs 目录并处理
pub async fn read_all_behavior_packs() -> Result<Vec<McPackInfo>> {
    let start = Instant::now();
    let behavior_packs_dir = default_minecraft_behavior_packs_path()
        .context("无法确定 behavior_packs 路径 (LOCALAPPDATA 未设置)")?;

    if !behavior_packs_dir.exists() {
        return Ok(Vec::new());
    }

    let mut rd = tokio_fs::read_dir(&behavior_packs_dir)
        .await
        .with_context(|| format!("无法打开目录 {}", behavior_packs_dir.display()))?;

    let mut folders: Vec<PathBuf> = Vec::new();
    while let Some(entry) = rd.next_entry().await? {
        let p = entry.path();
        if p.is_dir() {
            folders.push(p);
        }
    }

    let results: Vec<McPackInfo> = folders
        .into_par_iter()
        .filter_map(|folder_path: PathBuf| {
            let folder_name = folder_path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| folder_path.to_string_lossy().to_string());

            let manifest_path = folder_path.join("manifest.json");
            if !manifest_path.exists() {
                return None;
            }

            let manifest_raw = match fs::read_to_string(&manifest_path) {
                Ok(s) => s,
                Err(_) => return None,
            };

            let manifest_value: Value = match serde_json::from_str(&manifest_raw) {
                Ok(v) => v,
                Err(_) => return None,
            };

            let manifest_parsed: Option<Manifest> = match serde_json::from_str(&manifest_raw) {
                Ok(m) => Some(m),
                Err(_) => None,
            };

            let short_description = manifest_parsed
                .as_ref()
                .and_then(|m| m.header.as_ref().and_then(|h| h.description.clone()))
                .or_else(|| {
                    manifest_value
                        .get("header")
                        .and_then(|hdr| hdr.get("description"))
                        .and_then(|v| v.as_str().map(|s| s.to_string()))
                })
                .and_then(|s| short_desc_from_opt(Some(s), 50));

            let base = Arc::new(behavior_packs_dir.clone());
            let icon_abs = {
                let p = folder_path.join("pack_icon.png");
                if p.exists() {
                    Some(p.to_string_lossy().to_string())
                } else {
                    None
                }
            };

            let icon_rel = icon_abs.as_ref().and_then(|_| {
                match folder_path.strip_prefix(&*base) {
                    Ok(rel) => {
                        let mut rp = rel.to_path_buf();
                        rp.push("pack_icon.png");
                        Some(rp.to_string_lossy().to_string())
                    }
                    Err(_) => None,
                }
            });

            Some(McPackInfo {
                folder_name,
                folder_path: folder_path.to_string_lossy().to_string(),
                manifest: manifest_value,
                manifest_raw,
                manifest_parsed,
                icon_path: icon_abs,
                icon_rel,
                short_description,
            })
        })
        .collect();

    debug!("Finished reading behavior packs in {:?}", start.elapsed());
    Ok(results)
}
