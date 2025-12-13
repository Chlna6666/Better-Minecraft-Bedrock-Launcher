use anyhow::{Context, Result};
use num_cpus;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::fs as tokio_fs;
use tracing::debug;
use walkdir::WalkDir; // for any future use (kept)

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
#[derive(Debug, Serialize)]
pub struct McPackInfo {
    pub folder_name: String,
    pub folder_path: String,
    pub manifest: Value,      // 解析后的 Value（原始 JSON，可能被语言替换）
    pub manifest_raw: String, // 原始 json 文本
    pub manifest_parsed: Option<Manifest>, // 尝试解析成结构化 Manifest
    pub icon_path: Option<String>, // 如果存在 pack_icon.png
    pub icon_rel: Option<String>, // 相对 base 的路径
    pub short_description: Option<String>,

    // 来源信息（新增）
    pub source: Option<String>,      // "UWP" or "GDK"
    pub edition: Option<String>,     // "正式版" or "预览版"
    pub source_root: Option<String>, // 具体 root（LocalState 或 Users/<user>）
    pub gdk_user: Option<String>,    // 若来自 GDK，则为 Users\<X> 的 X
}

/// 获取可能的 resource/behavior packs 源集合（UWP LocalState + Roaming Users/*）
/// 返回 (base_path, source, edition, source_root)
fn default_pack_sources_for(kind: &str) -> Vec<(PathBuf, String, String, String)> {
    // kind: "resource_packs" 或 "behavior_packs"
    let mut res: Vec<(PathBuf, String, String, String)> = Vec::new();

    // 1) UWP LocalState（正式版）
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let uwp_root = PathBuf::from(&local_appdata)
            .join("Packages")
            .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
            .join("LocalState");
        let uwp_base = uwp_root.join("games").join("com.mojang").join(kind);
        if uwp_base.exists() && uwp_base.is_dir() {
            res.push((
                uwp_base.clone(),
                "UWP".to_string(),
                "正式版".to_string(),
                uwp_root.to_string_lossy().into_owned(),
            ));
        }

        // UWP Preview
        let uwp_preview_root = PathBuf::from(&local_appdata)
            .join("Packages")
            .join("Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe")
            .join("LocalState");
        let uwp_preview_base = uwp_preview_root.join("games").join("com.mojang").join(kind);
        if uwp_preview_base.exists() && uwp_preview_base.is_dir() {
            res.push((
                uwp_preview_base.clone(),
                "UWP".to_string(),
                "预览版".to_string(),
                uwp_preview_root.to_string_lossy().into_owned(),
            ));
        }
    }

    // 2) Roaming (GDK) 下的 Minecraft Bedrock / Minecraft Bedrock Preview -> Users\<user>\games\com.mojang\<kind>
    if let Ok(roaming) = std::env::var("APPDATA") {
        for (candidate, edition_label) in &[
            ("Minecraft Bedrock", "正式版"),
            ("Minecraft Bedrock Preview", "预览版"),
        ] {
            let users_dir = PathBuf::from(&roaming).join(candidate).join("Users");
            if users_dir.exists() && users_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&users_dir) {
                    for e in entries.filter_map(|e| e.ok()) {
                        let user_folder = e.path();
                        if !user_folder.exists() || !user_folder.is_dir() {
                            continue;
                        }
                        let p = user_folder.join("games").join("com.mojang").join(kind);
                        if p.exists() && p.is_dir() {
                            res.push((
                                p.clone(),
                                "GDK".to_string(),
                                edition_label.to_string(),
                                user_folder.to_string_lossy().into_owned(),
                            ));
                        }
                    }
                }
            }
        }
    }

    // 去重
    let mut seen = HashSet::new();
    res.retain(|(p, _, _, _)| {
        let k = p.to_string_lossy().to_lowercase();
        if seen.contains(&k) {
            false
        } else {
            seen.insert(k);
            true
        }
    });

    res
}

/// 解析 lang 文本（同你已有实现）
/// 返回 None 或 HashMap<key, value>
fn load_lang_map_for_pack(folder: &PathBuf, lang: &str) -> Option<HashMap<String, String>> {
    let candidates_dirs = ["texts", "text", "lang"];

    // helper: 解析单个 .lang 文件为 map
    fn parse_lang_file(path: &PathBuf) -> Option<HashMap<String, String>> {
        if let Ok(content) = fs::read_to_string(path) {
            let mut map = HashMap::new();
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.starts_with('#') || line.starts_with("//") {
                    continue;
                }
                if let Some((k, val)) = line.split_once('=') {
                    map.insert(k.trim().to_string(), val.trim().to_string());
                } else if let Some((k, val)) = line.split_once(':') {
                    map.insert(k.trim().to_string(), val.trim().to_string());
                }
            }
            if map.is_empty() {
                None
            } else {
                Some(map)
            }
        } else {
            None
        }
    }

    // 收集所有 .lang 文件 (path, stem)
    let mut lang_files: Vec<(PathBuf, String)> = Vec::new();
    for dir in &candidates_dirs {
        let base = folder.join(dir);
        if !base.exists() || !base.is_dir() {
            continue;
        }
        if let Ok(entries) = fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_file() {
                    if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                        if ext.eq_ignore_ascii_case("lang") {
                            let stem = p
                                .file_stem()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            lang_files.push((p, stem));
                        }
                    }
                }
            }
        }
    }

    if lang_files.is_empty() {
        return None;
    }

    if lang_files.len() == 1 {
        return parse_lang_file(&lang_files[0].0);
    }

    // 构造候选变体
    let mut lang_variants = Vec::new();
    lang_variants.push(lang.to_string());
    lang_variants.push(lang.to_lowercase());
    lang_variants.push(lang.to_uppercase());
    if lang.contains('-') {
        lang_variants.push(lang.replace('-', "_"));
        lang_variants.push(lang.replace('-', "_").to_lowercase());
    }
    if lang.contains('_') {
        lang_variants.push(lang.replace('_', "-"));
    }
    lang_variants.sort();
    lang_variants.dedup();

    let try_load_by_name = |target: &str| -> Option<HashMap<String, String>> {
        for (path, stem) in &lang_files {
            if stem.eq_ignore_ascii_case(target) {
                if let Some(m) = parse_lang_file(path) {
                    return Some(m);
                }
            }
        }
        None
    };

    for v in &lang_variants {
        if let Some(m) = try_load_by_name(v) {
            return Some(m);
        }
    }

    let fallback_candidates = ["en_US", "en-US", "en", "en_us"];
    for fc in &fallback_candidates {
        if let Some(m) = try_load_by_name(fc) {
            return Some(m);
        }
    }

    let requested_norm = lang.to_lowercase().replace('-', "_");
    for (path, stem) in &lang_files {
        let stem_norm = stem.to_lowercase().replace('-', "_");
        if stem_norm == requested_norm {
            if let Some(m) = parse_lang_file(path) {
                return Some(m);
            }
        }
    }

    None
}

/// replace header name/description with lang_map if they are placeholder keys
fn replace_header_with_lang(manifest_value: &mut Value, lang_map: &HashMap<String, String>) {
    if let Value::Object(ref mut root) = manifest_value {
        if let Some(Value::Object(ref mut header)) = root.get_mut("header") {
            // name
            if let Some(Value::String(ref name_s)) = header.get("name") {
                if let Some(repl) = lang_map.get(name_s) {
                    header.insert("name".to_string(), Value::String(repl.clone()));
                }
            }
            // description
            if let Some(Value::String(ref desc_s)) = header.get("description") {
                if let Some(repl) = lang_map.get(desc_s) {
                    header.insert("description".to_string(), Value::String(repl.clone()));
                }
            }
        }
    }
}

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

/// ---- helper: collect folders for a given kind from all sources ----
/// 返回 Vec<(folder_path, base_path, source, edition, source_root)>
fn collect_pack_folders(kind: &str) -> Result<Vec<(PathBuf, PathBuf, String, String, String)>> {
    let sources = default_pack_sources_for(kind);
    if sources.is_empty() {
        return Ok(Vec::new());
    }

    let mut items: Vec<(PathBuf, PathBuf, String, String, String)> = Vec::new();
    for (base, source, edition, source_root) in sources {
        // read sync (cheap) to gather folder names
        if let Ok(entries) = fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    items.push((
                        p.clone(),
                        base.clone(),
                        source.clone(),
                        edition.clone(),
                        source_root.clone(),
                    ));
                }
            }
        }
    }

    // 去重按 folder path
    let mut seen = HashSet::new();
    items.retain(|(p, base, _, _, _)| {
        let k = p.to_string_lossy().to_lowercase();
        if seen.contains(&k) {
            false
        } else {
            seen.insert(k);
            true
        }
    });

    Ok(items)
}

/// 高性能并行读取 resource_packs
pub async fn read_all_resource_packs(lang: &str) -> Result<Vec<McPackInfo>> {
    let start = Instant::now();
    let items = collect_pack_folders("resource_packs")?;
    if items.is_empty() {
        debug!("no resource_packs found");
        return Ok(Vec::new());
    }

    // 并行处理，rayon
    let results: Vec<McPackInfo> = items
        .into_par_iter()
        .filter_map(|(folder_path, base_path, source, edition, source_root)| {
            // folder scanning in rayon thread
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

            let mut manifest_value: Value = match serde_json::from_str(&manifest_raw) {
                Ok(v) => v,
                Err(_) => return None,
            };

            // 尝试加载语言替换
            if let Some(lang_map) = load_lang_map_for_pack(&folder_path, lang) {
                replace_header_with_lang(&mut manifest_value, &lang_map);
            }

            let manifest_parsed: Option<Manifest> =
                match serde_json::from_value(manifest_value.clone()) {
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

            // icon absolute
            let icon_abs = {
                let p = folder_path.join("pack_icon.png");
                if p.exists() {
                    Some(p.to_string_lossy().to_string())
                } else {
                    None
                }
            };

            // icon relative to base_path if possible
            let icon_rel =
                icon_abs
                    .as_ref()
                    .and_then(|_| match folder_path.strip_prefix(&base_path) {
                        Ok(rel) => {
                            let mut rp = rel.to_path_buf();
                            rp.push("pack_icon.png");
                            Some(rp.to_string_lossy().to_string())
                        }
                        Err(_) => None,
                    });

            // derive gdk_user if source == "GDK"
            let gdk_user = if source == "GDK" {
                PathBuf::from(&source_root)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            };

            Some(McPackInfo {
                folder_name,
                folder_path: folder_path.to_string_lossy().to_string(),
                manifest: manifest_value,
                manifest_raw,
                manifest_parsed,
                icon_path: icon_abs,
                icon_rel,
                short_description,
                source: Some(source),
                edition: Some(edition),
                source_root: Some(source_root),
                gdk_user,
            })
        })
        .collect();

    debug!("Finished reading resource packs in {:?}", start.elapsed());
    Ok(results)
}

/// 高性能并行读取 behavior_packs
pub async fn read_all_behavior_packs(lang: &str) -> Result<Vec<McPackInfo>> {
    let start = Instant::now();
    let items = collect_pack_folders("behavior_packs")?;
    if items.is_empty() {
        debug!("no behavior_packs found");
        return Ok(Vec::new());
    }

    let results: Vec<McPackInfo> = items
        .into_par_iter()
        .filter_map(|(folder_path, base_path, source, edition, source_root)| {
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

            let mut manifest_value: Value = match serde_json::from_str(&manifest_raw) {
                Ok(v) => v,
                Err(_) => return None,
            };

            if let Some(lang_map) = load_lang_map_for_pack(&folder_path, lang) {
                replace_header_with_lang(&mut manifest_value, &lang_map);
            }

            let manifest_parsed: Option<Manifest> =
                match serde_json::from_value(manifest_value.clone()) {
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

            let icon_abs = {
                let p = folder_path.join("pack_icon.png");
                if p.exists() {
                    Some(p.to_string_lossy().to_string())
                } else {
                    None
                }
            };

            let icon_rel =
                icon_abs
                    .as_ref()
                    .and_then(|_| match folder_path.strip_prefix(&base_path) {
                        Ok(rel) => {
                            let mut rp = rel.to_path_buf();
                            rp.push("pack_icon.png");
                            Some(rp.to_string_lossy().to_string())
                        }
                        Err(_) => None,
                    });

            let gdk_user = if source == "GDK" {
                PathBuf::from(&source_root)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            };

            Some(McPackInfo {
                folder_name,
                folder_path: folder_path.to_string_lossy().to_string(),
                manifest: manifest_value,
                manifest_raw,
                manifest_parsed,
                icon_path: icon_abs,
                icon_rel,
                short_description,
                source: Some(source),
                edition: Some(edition),
                source_root: Some(source_root),
                gdk_user,
            })
        })
        .collect();

    debug!("Finished reading behavior packs in {:?}", start.elapsed());
    Ok(results)
}
