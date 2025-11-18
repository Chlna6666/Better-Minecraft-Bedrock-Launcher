use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashSet;
use std::fs as stdfs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs as tokio_fs;

use walkdir::WalkDir; // 确保在 Cargo.toml 添加 walkdir = "2"
use num_cpus; // 确保在 Cargo.toml 添加 num_cpus = "1"

/// 表示发现的一个 “base” 源（例如 UWP LocalState 或 Roaming Users/<user>）
/// path: 指向 minecraftWorlds 目录本身
/// source: "UWP" 或 "GDK"
/// edition: "正式版" 或 "预览版"
/// source_root: 更详细的来源根（例如 LocalState 路径或 Users/<user> 路径）
#[derive(Debug, Clone)]
struct WorldBase {
    path: PathBuf,
    source: String,
    edition: String,
    source_root: String,
}

/// 返回可能存在的 minecraftWorlds 源集合（UWP LocalState + Roaming Bedrock Users/*）
/// 顺序：UWP release -> UWP preview -> Roaming Bedrock (release) -> Roaming Bedrock Preview
fn default_minecraft_worlds_sources() -> Vec<WorldBase> {
    let mut res: Vec<WorldBase> = Vec::new();

    // 1) UWP LocalState（正式版）
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let uwp_root = PathBuf::from(&local_appdata).join("Packages").join("Microsoft.MinecraftUWP_8wekyb3d8bbwe").join("LocalState");
        let uwp_worlds = uwp_root.join("games").join("com.mojang").join("minecraftWorlds");
        if uwp_worlds.exists() && uwp_worlds.is_dir() {
            res.push(WorldBase {
                path: uwp_worlds.clone(),
                source: "UWP".to_string(),
                edition: "正式版".to_string(),
                source_root: uwp_root.to_string_lossy().into_owned(),
            });
        }

        // 2) UWP Preview/Beta
        let uwp_preview_root = PathBuf::from(&local_appdata)
            .join("Packages")
            .join("Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe")
            .join("LocalState");
        let uwp_preview_worlds = uwp_preview_root.join("games").join("com.mojang").join("minecraftWorlds");
        if uwp_preview_worlds.exists() && uwp_preview_worlds.is_dir() {
            res.push(WorldBase {
                path: uwp_preview_worlds.clone(),
                source: "UWP".to_string(),
                edition: "预览版".to_string(),
                source_root: uwp_preview_root.to_string_lossy().into_owned(),
            });
        }
    }

    // 3) Roaming (GDK) 下的 Minecraft Bedrock / Minecraft Bedrock Preview -> Users\<user>\games\com.mojang\minecraftWorlds
    if let Ok(roaming) = std::env::var("APPDATA") {
        for (candidate, edition_label) in &[("Minecraft Bedrock", "正式版"), ("Minecraft Bedrock Preview", "预览版")] {
            let users_dir = PathBuf::from(&roaming).join(candidate).join("Users");
            if users_dir.exists() && users_dir.is_dir() {
                if let Ok(entries) = stdfs::read_dir(&users_dir) {
                    for e in entries.filter_map(|e| e.ok()) {
                        // 忽略非目录、隐藏文件等
                        let user_folder = e.path();
                        if !user_folder.exists() || !user_folder.is_dir() {
                            continue;
                        }
                        let p = user_folder.join("games").join("com.mojang").join("minecraftWorlds");
                        if p.exists() && p.is_dir() {
                            res.push(WorldBase {
                                path: p.clone(),
                                source: "GDK".to_string(), // 你说 GDK 就是 Roaming
                                edition: edition_label.to_string(),
                                source_root: user_folder.to_string_lossy().into_owned(),
                            });
                        }
                    }
                }
            }
        }
    }

    // 去重：按 canonical display path 去重（避免同一路径重复入列）
    let mut seen = HashSet::new();
    res.retain(|wb| {
        let k = wb.path.to_string_lossy().to_lowercase();
        if seen.contains(&k) {
            false
        } else {
            seen.insert(k);
            true
        }
    });

    res
}

#[derive(Debug, Serialize)]
pub struct McMapInfo {
    pub folder_name: String,
    pub folder_path: String,
    pub level_name: Option<String>,
    pub icon_path: Option<String>,
    pub modified: Option<String>,
    pub size_bytes: Option<u64>,
    pub size_readable: Option<String>,
    pub behavior_packs: Option<Value>,
    pub resource_packs: Option<Value>,
    pub behavior_packs_count: Option<usize>,
    pub resource_packs_count: Option<usize>,

    // 新增字段：来源与版本、以及根来源信息（便于 UI/日志展示）
    pub source: Option<String>,      // "UWP" or "GDK"
    pub edition: Option<String>,     // "正式版" or "预览版"
    pub source_root: Option<String>, // 具体 root（LocalState 或 Users/<user>）

    // 新增字段：若来自 GDK（Roaming），解析出 Users\<X> 中的 X（用户文件夹名）
    pub gdk_user: Option<String>,
}

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
    if i == 0 {
        format!("{} {}", bytes, UNITS[i])
    } else {
        format!("{:.2} {}", b, UNITS[i])
    }
}

/// 使用 walkdir 在阻塞线程中高效遍历并汇总文件大小
async fn get_dir_size(path: &Path) -> anyhow::Result<u64> {
    let path = path.to_path_buf();
    let size = tokio::task::spawn_blocking(move || -> anyhow::Result<u64> {
        let mut total: u64 = 0;
        // 使用 WalkDir，忽略可能的错误项
        for entry in WalkDir::new(&path).follow_links(false).into_iter().filter_map(|e| e.ok()) {
            // 只对文件取大小；metadata 在同步线程中执行（更快且不会阻塞 tokio）
            if let Ok(md) = entry.metadata() {
                if md.is_file() {
                    total = total.saturating_add(md.len());
                }
            }
        }
        Ok(total)
    })
        .await
        .context("spawn_blocking failed")??;

    Ok(size)
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

/// 将 serde_json::from_str 移到阻塞线程（JSON 解析可能较重）
async fn parse_json_value_blocking(s: String) -> Option<Value> {
    tokio::task::spawn_blocking(move || serde_json::from_str::<Value>(&s).ok())
        .await
        .ok()
        .flatten()
}

/// 列出所有世界（支持多来源：UWP + GDK/Users 多用户）
/// concurrency: 0 表示使用默认并发 (num_cpus * 8)
pub async fn list_minecraft_worlds(concurrency: usize) -> Result<Vec<McMapInfo>> {
    // 收集所有 base sources
    let bases = default_minecraft_worlds_sources();
    if bases.is_empty() {
        anyhow::bail!("无法确定 minecraftWorlds 路径（未找到 UWP LocalState 或 Roaming Minecraft Bedrock Users）");
    }

    // 为每个 base 收集其下的世界目录（并记录来源信息）
    let mut world_items: Vec<(PathBuf, String, String, String)> = Vec::new();
    for b in bases.into_iter() {
        if !b.path.exists() || !b.path.is_dir() {
            continue;
        }
        match tokio_fs::read_dir(&b.path).await {
            Ok(mut dir) => {
                while let Ok(Some(entry)) = dir.next_entry().await {
                    let p = entry.path();
                    // 只加入目录条目（具体验证在 worker 中完成）
                    world_items.push((p, b.source.clone(), b.edition.clone(), b.source_root.clone()));
                }
            }
            Err(e) => {
                eprintln!("warning: read_dir failed for {}: {:?}", b.path.display(), e);
                continue;
            }
        }
    }

    if world_items.is_empty() {
        anyhow::bail!("未在任何已知目录下找到世界数据：请检查路径或权限");
    }

    // 去重 world_items（按路径）
    let mut seen = HashSet::new();
    world_items.retain(|(p, _, _, _)| {
        let k = p.to_string_lossy().to_lowercase();
        if seen.contains(&k) {
            false
        } else {
            seen.insert(k);
            true
        }
    });

    // 并发策略：默认使用 num_cpus * 8（I/O 密集型场景可以用更高倍数）
    let concurrency = if concurrency == 0 {
        std::cmp::max(1, num_cpus::get() * 8)
    } else {
        concurrency
    };

    // 构建 stream 并发处理每个 world item
    let tasks_stream = stream::iter(world_items.into_iter().map(move |(path, source, edition, source_root)| {
        async move {
            // 跳过非目录或隐藏目录（以 '.' 开头）
            let md = tokio_fs::metadata(&path).await.ok()?;
            if !md.is_dir() {
                return None;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    return None;
                }
            }

            let folder_name = path.file_name()?.to_string_lossy().into_owned();
            let folder_path_str = path.to_string_lossy().into_owned();

            // 解析 gdk_user（如果来源是 GDK，则尝试从 source_root 提取 Users\<X> 的 X）
            let gdk_user = if source == "GDK" {
                // source_root 在构建 WorldBase 时为 Users\<X> 的路径（user_folder）
                Path::new(&source_root)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_string())
            } else {
                None
            };

            let mut info = McMapInfo {
                folder_name: folder_name.clone(),
                folder_path: folder_path_str.clone(),
                level_name: None,
                icon_path: None,
                modified: None,
                size_bytes: None,
                size_readable: None,
                behavior_packs: None,
                resource_packs: None,
                behavior_packs_count: None,
                resource_packs_count: None,
                source: Some(source.clone()),
                edition: Some(edition.clone()),
                source_root: Some(source_root.clone()),
                gdk_user,
            };

            // level name（小文件，异步读取）
            if let Ok(content) = tokio_fs::read_to_string(path.join("levelname.txt")).await {
                let s = content.trim();
                if !s.is_empty() {
                    info.level_name = Some(s.to_string());
                }
            }

            // icon（文件存在检查）
            let icon_file = path.join("world_icon.jpeg");
            if tokio_fs::metadata(&icon_file).await.is_ok() {
                info.icon_path = Some(icon_file.to_string_lossy().into_owned());
            }

            // modified 时间（使用先前已取得的 md）
            if let Ok(md_time) = md.modified() {
                info.modified = Some(systemtime_to_iso(md_time));
            }

            // 计算目录大小（放到阻塞线程，使用 walkdir）
            if let Ok(size) = get_dir_size(&path).await {
                info.size_bytes = Some(size);
                info.size_readable = Some(bytes_to_human(size));
            }

            // behavior & resource packs（读取 JSON 并在阻塞线程解析）
            for (field_value_slot, count_slot, name) in [
                (&mut info.behavior_packs, &mut info.behavior_packs_count, "world_behavior_packs.json"),
                (&mut info.resource_packs, &mut info.resource_packs_count, "world_resource_packs.json"),
            ] {
                let json_file = path.join(name);
                if tokio_fs::metadata(&json_file).await.is_ok() {
                    if let Ok(s) = tokio_fs::read_to_string(&json_file).await {
                        if let Some(v) = parse_json_value_blocking(s).await {
                            *field_value_slot = Some(v.clone());
                            *count_slot = Some(count_packs(&v));
                        }
                    }
                }
            }

            Some(info)
        }
    }))
        .buffer_unordered(concurrency);

    // 收集结果并返回
    Ok(tasks_stream.filter_map(|m| async move { m }).collect().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn test_list_worlds_async_print_timing() {
        let start = Instant::now();

        match list_minecraft_worlds(0).await {
            Ok(worlds) => {
                let elapsed = start.elapsed();
                let ms = elapsed.as_secs_f64() * 1000.0;
                println!(
                    "async: 找到 {} 个世界，耗时 {:.3} ms ({}.{:03}s)",
                    worlds.len(),
                    ms,
                    elapsed.as_secs(),
                    elapsed.subsec_millis()
                );
                println!("{}", serde_json::to_string_pretty(&worlds).unwrap());
            }
            Err(e) => {
                let elapsed = start.elapsed();
                eprintln!(
                    "async: 出错: {:?}，已耗时 {:.3} ms",
                    e,
                    elapsed.as_secs_f64() * 1000.0
                );
            }
        }
    }
}
