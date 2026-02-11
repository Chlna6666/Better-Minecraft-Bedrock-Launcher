// src-tauri/src/commands/resource_packs.rs

use anyhow::Result;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use tracing::debug;
use crate::core::minecraft::paths::{scan_game_dirs, GamePathOptions};

/// 返回给前端的资源包信息结构
#[derive(Debug, Serialize)]
pub struct McPackInfo {
    pub folder_name: String,
    pub folder_path: String,
    pub manifest: Value,
    pub manifest_raw: String,
    pub manifest_parsed: Option<Manifest>,
    pub icon_path: Option<String>,
    pub icon_rel: Option<String>,
    pub short_description: Option<String>,
    pub source: Option<String>,
    pub edition: Option<String>,
    pub source_root: Option<String>,
    pub gdk_user: Option<String>,
}

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
    pub capabilities: Option<Value>,
    pub metadata: Option<Metadata>,
}


/// 通用读取逻辑 (运行在 Blocking Thread 中)
pub(crate) fn read_packs_standard(
    kind: &str, // "resource_packs" or "behavior_packs"
    lang: &str,
    options: &GamePathOptions,
) -> Result<Vec<McPackInfo>> {
    let start = Instant::now();

    // 1. 扫描目录
    let pack_roots = scan_game_dirs(options, kind);

    if pack_roots.is_empty() {
        return Ok(Vec::new());
    }

    // 收集所有包文件夹
    let mut pack_folders = Vec::new();
    for root in pack_roots {
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    pack_folders.push((entry.path(), root.clone()));
                }
            }
        }
    }

    // 2. 并行解析
    let results: Vec<McPackInfo> = pack_folders
        .into_par_iter()
        .filter_map(|(folder_path, base_path)| {
            let folder_name = folder_path.file_name()?.to_string_lossy().to_string();
            let manifest_path = folder_path.join("manifest.json");

            if !manifest_path.exists() { return None; }

            let manifest_raw = fs::read_to_string(&manifest_path).ok()?;
            let clean_json = strip_json_comments(&manifest_raw);
            let mut manifest_value: Value = serde_json::from_str(&clean_json).ok()?;

            // 本地化
            if let Some(lang_map) = load_lang_map_for_pack(&folder_path, lang) {
                replace_header_with_lang(&mut manifest_value, &lang_map);
            }

            let manifest_parsed: Option<Manifest> = serde_json::from_value(manifest_value.clone()).ok();

            // 描述
            let short_description = manifest_parsed.as_ref()
                .and_then(|m| m.header.as_ref().and_then(|h| h.description.clone()))
                .or_else(|| manifest_value.get("header")?.get("description")?.as_str().map(|s| s.to_string()))
                .map(|s| s.chars().take(50).collect::<String>()); // 简单截断

            // 图标
            let icon_abs = folder_path.join("pack_icon.png");
            let icon_path = if icon_abs.exists() { Some(icon_abs.to_string_lossy().to_string()) } else { None };

            let icon_rel = icon_path.as_ref().and_then(|_| {
                folder_path.strip_prefix(&base_path).ok()
                    .map(|p| p.join("pack_icon.png").to_string_lossy().to_string())
            });

            // GDK User
            let gdk_user = if folder_path.to_string_lossy().contains("Users") {
                base_path.parent() // com.mojang
                    .and_then(|p| p.parent()) // games
                    .and_then(|p| p.parent()) // <User>
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().to_string())
            } else { None };

            Some(McPackInfo {
                folder_name,
                folder_path: folder_path.to_string_lossy().to_string(),
                manifest: manifest_value,
                manifest_raw,
                manifest_parsed,
                icon_path,
                icon_rel,
                short_description,
                source: Some(format!("{:?}", options.build_type)),
                edition: Some(format!("{:?}", options.edition)),
                source_root: Some(base_path.to_string_lossy().to_string()),
                gdk_user,
            })
        })
        .collect();

    debug!("Finished reading {} ({}) in {:?}", kind, results.len(), start.elapsed());
    Ok(results)
}

/// 路径收集器
fn collect_pack_folders_filtered(
    kind: &str, // "resource_packs" or "behavior_packs"
    source: &str,
    isolation_id: Option<&str>,
) -> Vec<(PathBuf, PathBuf, String, String, String)> {
    let mut roots: Vec<(PathBuf, String, String, String)> = Vec::new();
    let is_gdk = source.eq_ignore_ascii_case("gdk");
    let is_uwp = source.eq_ignore_ascii_case("uwp");

    // =========================================================
    // 1. 隔离模式 (Isolation Mode)
    // =========================================================
    if let Some(iso_id) = isolation_id {
        let versions_root = Path::new("./BMCBL/versions");
        let version_base = versions_root.join(iso_id).join("Minecraft Bedrock");

        if is_gdk {
            // GDK 隔离: Users/Shared/games/com.mojang/<kind>
            let pack_root = version_base
                .join("Users")
                .join("Shared")
                .join("games")
                .join("com.mojang")
                .join(kind);

            if pack_root.exists() && pack_root.is_dir() {
                roots.push((
                    pack_root,
                    "GDK-Isolated".into(),
                    iso_id.to_string(),
                    version_base.join("Users/Shared").to_string_lossy().into(),
                ));
            }
        } else if is_uwp {
            // UWP 隔离: games/com.mojang/<kind>
            let pack_root = version_base
                .join("games")
                .join("com.mojang")
                .join(kind);

            if pack_root.exists() && pack_root.is_dir() {
                roots.push((
                    pack_root,
                    "UWP-Isolated".into(),
                    iso_id.to_string(),
                    version_base.to_string_lossy().into(),
                ));
            }
        }

        // 隔离模式下直接返回，不混入系统资源
        return expand_roots_to_folders(roots);
    }

    // =========================================================
    // 2. 系统模式 (System Mode)
    // =========================================================
    if is_uwp {
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            // 正式版
            let uwp_root = PathBuf::from(&local_appdata)
                .join("Packages")
                .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
                .join("LocalState");
            let uwp_base = uwp_root.join("games").join("com.mojang").join(kind);
            if uwp_base.exists() && uwp_base.is_dir() {
                roots.push((
                    uwp_base,
                    "UWP".into(),
                    "正式版".into(),
                    uwp_root.to_string_lossy().into(),
                ));
            }

            // 预览版
            let uwp_preview_root = PathBuf::from(&local_appdata)
                .join("Packages")
                .join("Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe")
                .join("LocalState");
            let uwp_preview_base = uwp_preview_root.join("games").join("com.mojang").join(kind);
            if uwp_preview_base.exists() && uwp_preview_base.is_dir() {
                roots.push((
                    uwp_preview_base,
                    "UWP".into(),
                    "预览版".into(),
                    uwp_preview_root.to_string_lossy().into(),
                ));
            }
        }
    } else if is_gdk {
        if let Ok(roaming) = std::env::var("APPDATA") {
            for (candidate, edition_label) in &[
                ("Minecraft Bedrock", "正式版"),
                ("Minecraft Bedrock Preview", "预览版"),
            ] {
                let users_dir = PathBuf::from(&roaming).join(candidate).join("Users");
                if users_dir.exists() && users_dir.is_dir() {
                    // GDK 下，资源包可能存在于具体用户目录下，也可能在 Shared 下
                    // 为了保险，扫描 Users 下所有子目录
                    if let Ok(entries) = std::fs::read_dir(&users_dir) {
                        for e in entries.flatten() {
                            let user_folder = e.path();
                            if !user_folder.is_dir() { continue; }

                            let p = user_folder.join("games").join("com.mojang").join(kind);
                            if p.exists() && p.is_dir() {
                                roots.push((
                                    p,
                                    "GDK".into(),
                                    edition_label.to_string(),
                                    user_folder.to_string_lossy().into(),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    expand_roots_to_folders(roots)
}

fn expand_roots_to_folders(
    roots: Vec<(PathBuf, String, String, String)>
) -> Vec<(PathBuf, PathBuf, String, String, String)> {
    let mut items = Vec::new();

    for (base, src_label, edition, src_root) in roots {
        if let Ok(entries) = fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    items.push((
                        p,
                        base.clone(),
                        src_label.clone(),
                        edition.clone(),
                        src_root.clone(),
                    ));
                }
            }
        }
    }

    // 去重
    let mut seen = HashSet::new();
    items.retain(|(p, _, _, _, _)| {
        let k = p.to_string_lossy().to_lowercase();
        if seen.contains(&k) {
            false
        } else {
            seen.insert(k);
            true
        }
    });

    items
}

// ==================================================================================
// 4. 辅助函数
// ==================================================================================

/// 去除 JSONC (JSON with Comments) 中的注释
fn strip_json_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    let mut in_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    if let Some(&first_char) = chars.peek() {
        if first_char == '\u{feff}' {
            chars.next();
        }
    }

    while let Some(c) = chars.next() {
        if in_line_comment {
            if c == '\n' {
                in_line_comment = false;
                output.push(c);
            }
            continue;
        }

        if in_block_comment {
            if c == '*' {
                if let Some('/') = chars.peek() {
                    chars.next();
                    in_block_comment = false;
                }
            }
            continue;
        }

        if in_string {
            output.push(c);
            if c == '\\' {
                if let Some(next) = chars.next() {
                    output.push(next);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }

        if c == '"' {
            in_string = true;
            output.push(c);
            continue;
        }

        if c == '/' {
            if let Some(&next) = chars.peek() {
                if next == '/' {
                    chars.next();
                    in_line_comment = true;
                    continue;
                } else if next == '*' {
                    chars.next();
                    in_block_comment = true;
                    continue;
                }
            }
        }

        output.push(c);
    }

    output
}

pub fn load_lang_map_for_pack(folder: &PathBuf, lang: &str) -> Option<HashMap<String, String>> {
    let candidates_dirs = ["texts", "text", "lang"];

    fn parse_lang_file(path: &PathBuf) -> Option<HashMap<String, String>> {
        if let Ok(content) = fs::read_to_string(path) {
            let mut map = HashMap::new();
            // 处理 BOM
            let content = content.strip_prefix('\u{feff}').unwrap_or(&content);

            for line in content.lines() {
                // [关键修改] 先按 # 分割，只取前部分，用于屏蔽行尾注释 (如 "key=val #comment")
                let raw_line = if let Some((valid, _)) = line.split_once('#') {
                    valid
                } else {
                    line
                };

                let line_str = raw_line.trim();

                // 过滤无效行
                if line_str.is_empty()
                    || line_str.starts_with("//")
                    || line_str.starts_with('[')
                {
                    continue;
                }

                // 解析 Key-Value
                if let Some((k, val)) = line_str.split_once('=') {
                    map.insert(k.trim().to_string(), val.trim().to_string());
                } else if let Some((k, val)) = line_str.split_once(':') {
                    map.insert(k.trim().to_string(), val.trim().to_string());
                } else {
                    // 尝试 Tab 分割兼容
                    let parts: Vec<&str> = line_str.splitn(2, '\t').collect();
                    if parts.len() == 2 {
                        map.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
                    }
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
    let fallback_candidates = ["en-US", "en", "en-us"];
    for fc in &fallback_candidates {
        if let Some(m) = try_load_by_name(fc) {
            return Some(m);
        }
        if fc.contains('-') {
            let alt = fc.replace('-', "_");
            if let Some(m) = try_load_by_name(&alt) {
                return Some(m);
            }
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

pub fn replace_header_with_lang(manifest_value: &mut Value, lang_map: &HashMap<String, String>) {
    if let Value::Object(ref mut root) = manifest_value {
        if let Some(Value::Object(ref mut header)) = root.get_mut("header") {
            if let Some(Value::String(ref name_s)) = header.get("name") {
                if let Some(repl) = lang_map.get(name_s) {
                    header.insert("name".to_string(), Value::String(repl.clone()));
                }
            }
            if let Some(Value::String(ref desc_s)) = header.get("description") {
                if let Some(repl) = lang_map.get(desc_s) {
                    header.insert("description".to_string(), Value::String(repl.clone()));
                }
            }
        }
        if let Some(Value::Array(ref mut modules)) = root.get_mut("modules") {
            for m in modules {
                if let Value::Object(ref mut mod_obj) = m {
                    if let Some(Value::String(ref desc_s)) = mod_obj.get("description") {
                        if let Some(repl) = lang_map.get(desc_s) {
                            mod_obj.insert("description".to_string(), Value::String(repl.clone()));
                        }
                    }
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
