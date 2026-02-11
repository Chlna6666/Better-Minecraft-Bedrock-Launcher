// src-tauri/src/commands/map.rs
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime};
use tracing::{debug, error};
use walkdir::WalkDir;
use crate::core::minecraft::paths::{scan_game_dirs, GamePathOptions};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McMapInfo {
    pub folder_name: String,
    pub folder_path: String,
    pub level_name: Option<String>,      // 从 levelname.txt 读取
    pub icon_path: Option<String>,       // 绝对路径
    pub modified: Option<String>,        // ISO 时间字符串
    pub size_bytes: Option<u64>,
    pub size_readable: Option<String>,

    // 资源包/行为包引用信息 (简单解析 world_behavior_packs.json)
    pub behavior_packs: Option<Value>,
    pub resource_packs: Option<Value>,
    pub behavior_packs_count: Option<usize>,
    pub resource_packs_count: Option<usize>,

    // 来源元数据
    pub source: Option<String>,      // "UWP", "GDK", "GDK-Isolated"
    pub edition: Option<String>,     // "正式版", "预览版", "隔离版"
    pub source_root: Option<String>,
    pub gdk_user: Option<String>,    // 所属用户目录名
}

// ==================================================================================
// 3. 核心逻辑
// ==================================================================================

pub(crate) fn list_worlds_standard(options: &GamePathOptions) -> Result<Vec<McMapInfo>> {
    let start = Instant::now();

    // 1. 使用通用路径模块获取所有可能的地图根目录
    let world_dirs = scan_game_dirs(options, "minecraftWorlds");

    if world_dirs.is_empty() {
        debug!("No world roots found for options: {:?}", options);
        return Ok(Vec::new());
    }

    // 2. 收集具体的地图文件夹
    let mut map_folders = Vec::new();
    for root in world_dirs {
        if let Ok(entries) = fs::read_dir(&root) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    // 保存 (地图路径, 它的父级根目录)
                    map_folders.push((entry.path(), root.clone()));
                }
            }
        }
    }

    // 3. 并行处理详情
    let mut results: Vec<McMapInfo> = map_folders
        .into_par_iter()
        .filter_map(|(folder_path, source_root)| {
            let folder_name = folder_path.file_name()?.to_string_lossy().to_string();
            if folder_name.starts_with('.') { return None; }

            // 尝试反推 gdk_user (如果路径包含 Users/xxx)
            let gdk_user = if folder_path.to_string_lossy().contains("Users") {
                folder_path.parent() // minecraftWorlds
                    .and_then(|p| p.parent()) // com.mojang
                    .and_then(|p| p.parent()) // games
                    .and_then(|p| p.parent()) // <User_ID>
                    .and_then(|p| p.file_name())
                    .map(|s| s.to_string_lossy().to_string())
            } else {
                None
            };

            let mut info = McMapInfo {
                folder_name,
                folder_path: folder_path.to_string_lossy().to_string(),
                level_name: None,
                icon_path: None,
                modified: None,
                size_bytes: None,
                size_readable: None,
                behavior_packs: None,
                resource_packs: None,
                behavior_packs_count: None,
                resource_packs_count: None,
                source: Some(format!("{:?}", options.build_type)),
                edition: Some(format!("{:?}", options.edition)),
                source_root: Some(source_root.to_string_lossy().to_string()),
                gdk_user,
            };

            // 读取 levelname.txt
            if let Ok(c) = fs::read_to_string(folder_path.join("levelname.txt")) {
                let trimmed = c.trim();
                if !trimmed.is_empty() { info.level_name = Some(trimmed.to_string()); }
            }

            // 图标
            let icon = folder_path.join("world_icon.jpeg");
            if icon.exists() { info.icon_path = Some(icon.to_string_lossy().to_string()); }

            // 修改时间
            if let Ok(md) = fs::metadata(&folder_path) {
                if let Ok(m) = md.modified() { info.modified = Some(systemtime_to_iso(m)); }
            }

            // 大小
            if let Ok(size) = get_dir_size_sync(&folder_path) {
                info.size_bytes = Some(size);
                info.size_readable = Some(bytes_to_human(size));
            }

            // 解析包依赖
            for (json_name, val_slot, count_slot) in [
                ("world_behavior_packs.json", &mut info.behavior_packs, &mut info.behavior_packs_count),
                ("world_resource_packs.json", &mut info.resource_packs, &mut info.resource_packs_count),
            ] {
                let p = folder_path.join(json_name);
                if p.exists() {
                    if let Ok(c) = fs::read_to_string(&p) {
                        if let Ok(v) = serde_json::from_str::<Value>(&c) {
                            *count_slot = Some(count_packs(&v));
                            *val_slot = Some(v);
                        }
                    }
                }
            }

            Some(info)
        })
        .collect();

    // 4. 排序
    results.sort_by(|a, b| {
        b.modified.as_deref().unwrap_or("").cmp(a.modified.as_deref().unwrap_or(""))
    });

    debug!("Scanned {} worlds in {:?}", results.len(), start.elapsed());
    Ok(results)
}

/// 路径收集器：返回 (minecraftWorlds_Path, Source_Label, Edition, Source_Root_Path, GDK_User_Label)
fn collect_world_roots_filtered(
    source: &str,
    isolation_id: Option<&str>,
    gdk_user_filter: Option<&str>,
) -> Vec<(PathBuf, String, String, String, Option<String>)> {
    let mut roots = Vec::new();
    let is_gdk = source.eq_ignore_ascii_case("gdk");
    let is_uwp = source.eq_ignore_ascii_case("uwp");

    // =========================================================
    // 1. 隔离模式 (Isolation Mode)
    // =========================================================
    if let Some(iso_id) = isolation_id {
        let versions_root = Path::new("./BMCBL/versions");
        // 基础路径: ./BMCBL/versions/<id>/Minecraft Bedrock
        let version_base = versions_root.join(iso_id).join("Minecraft Bedrock");

        if is_gdk {
            // GDK 隔离: Minecraft Bedrock/Users/<ID>/games/com.mojang/minecraftWorlds
            let users_dir = version_base.join("Users");
            if users_dir.exists() {
                if let Ok(entries) = fs::read_dir(&users_dir) {
                    for entry in entries.flatten() {
                        let user_path = entry.path();
                        if !user_path.is_dir() { continue; }

                        let dir_name = user_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();

                        // GDK 特性：Shared 文件夹通常不放存档，且不是用户ID
                        if dir_name.eq_ignore_ascii_case("Shared") { continue; }

                        let worlds_dir = user_path.join("games").join("com.mojang").join("minecraftWorlds");
                        if worlds_dir.exists() {
                            roots.push((
                                worlds_dir,
                                "GDK-Isolated".into(),
                                iso_id.to_string(), // Edition 显示为版本名
                                user_path.to_string_lossy().to_string(), // Root 指向用户层级
                                Some(dir_name) // 用户 ID
                            ));
                        }
                    }
                }
            }
        } else if is_uwp {
            // UWP 隔离: Minecraft Bedrock/games/com.mojang/minecraftWorlds (无 Users 层级)
            let worlds_dir = version_base.join("games").join("com.mojang").join("minecraftWorlds");
            if worlds_dir.exists() {
                roots.push((
                    worlds_dir,
                    "UWP-Isolated".into(),
                    iso_id.to_string(),
                    version_base.to_string_lossy().to_string(),
                    None
                ));
            }
        }

        // 如果开启了隔离模式，我们只扫描隔离目录，不混合系统目录
        return roots;
    }

    // =========================================================
    // 2. 系统默认模式 (System Mode)
    // =========================================================

    // GDK 系统逻辑
    if is_gdk {
        let mut scan_users_dir = |users_dir: PathBuf, edition_lbl: &str| {
            if !users_dir.exists() { return; }

            // 如果指定了 gdk_user，只扫描该用户
            if let Some(target_user) = gdk_user_filter {
                let user_folder = users_dir.join(target_user);
                let worlds_dir = user_folder.join("games").join("com.mojang").join("minecraftWorlds");
                if worlds_dir.exists() {
                    roots.push((
                        worlds_dir,
                        "GDK".into(),
                        edition_lbl.to_string(),
                        user_folder.to_string_lossy().to_string(),
                        Some(target_user.to_string())
                    ));
                }
            } else if let Ok(entries) = fs::read_dir(&users_dir) {
                // 未指定用户，扫描所有
                for e in entries.flatten() {
                    let user_folder = e.path();
                    if !user_folder.is_dir() { continue; }
                    let worlds_dir = user_folder.join("games").join("com.mojang").join("minecraftWorlds");
                    let user_name = user_folder.file_name().map(|s| s.to_string_lossy().to_string());
                    if worlds_dir.exists() {
                        roots.push((
                            worlds_dir,
                            "GDK".into(),
                            edition_lbl.to_string(),
                            user_folder.to_string_lossy().to_string(),
                            user_name
                        ));
                    }
                }
            }
        };

        if let Ok(roaming) = std::env::var("APPDATA") {
            // 正式版
            scan_users_dir(PathBuf::from(&roaming).join("Minecraft Bedrock").join("Users"), "正式版");
            // 预览版
            scan_users_dir(PathBuf::from(&roaming).join("Minecraft Bedrock Preview").join("Users"), "预览版");
        }
    }
    // UWP 系统逻辑
    else if is_uwp {
        if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
            // 正式版
            let uwp_root = PathBuf::from(&local_appdata)
                .join("Packages")
                .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
                .join("LocalState");
            let uwp_worlds = uwp_root.join("games").join("com.mojang").join("minecraftWorlds");

            if uwp_worlds.exists() {
                roots.push((
                    uwp_worlds,
                    "UWP".into(),
                    "正式版".into(),
                    uwp_root.to_string_lossy().into(),
                    None
                ));
            }

            // 预览版
            let uwp_preview_root = PathBuf::from(&local_appdata)
                .join("Packages")
                .join("Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe")
                .join("LocalState");
            let uwp_preview_worlds = uwp_preview_root.join("games").join("com.mojang").join("minecraftWorlds");

            if uwp_preview_worlds.exists() {
                roots.push((
                    uwp_preview_worlds,
                    "UWP".into(),
                    "预览版".into(),
                    uwp_preview_root.to_string_lossy().into(),
                    None
                ));
            }
        }
    }

    // 去重 (防止重复扫描)
    let mut seen = HashSet::new();
    roots.retain(|(p, _, _, _, _)| {
        let k = p.to_string_lossy().to_lowercase();
        if seen.contains(&k) { false } else { seen.insert(k); true }
    });

    roots
}

// ==================================================================================
// 4. 辅助函数
// ==================================================================================

fn systemtime_to_iso(t: SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339()
}

fn bytes_to_human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut b = bytes as f64;
    let mut i = 0usize;
    while b >= 1024.0 && i < UNITS.len() - 1 {
        b /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", b, UNITS[i])
}

// 同步版本的目录大小计算 (配合 Rayon 使用)
fn get_dir_size_sync(path: &Path) -> Result<u64> {
    let mut total: u64 = 0;
    for entry in WalkDir::new(path).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if let Ok(md) = entry.metadata() {
            if md.is_file() {
                total = total.saturating_add(md.len());
            }
        }
    }
    Ok(total)
}

fn count_packs(value: &Value) -> usize {
    match value {
        Value::Array(a) => a.len(),
        Value::Object(o) => {
            if let Some(Value::Array(a)) = o.get("entries") {
                a.len()
            } else if let Some(Value::Array(a)) = o.get("packs") {
                a.len()
            } else {
                o.len()
            }
        }
        _ => 0,
    }
}