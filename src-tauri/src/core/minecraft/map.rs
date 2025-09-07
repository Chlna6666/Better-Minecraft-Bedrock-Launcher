use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tokio::fs as tokio_fs;

use walkdir::WalkDir; // 新增

fn default_minecraft_worlds_path() -> Option<PathBuf> {
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        Some(PathBuf::from(local_appdata)
            .join("Packages")
            .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
            .join("LocalState")
            .join("games")
            .join("com.mojang")
            .join("minecraftWorlds"))
    } else {
        None
    }
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

pub async fn list_minecraft_worlds(concurrency: usize) -> Result<Vec<McMapInfo>> {
    let base = default_minecraft_worlds_path().context("无法确定 minecraftWorlds 路径")?;
    if !base.exists() || !base.is_dir() {
        anyhow::bail!("路径不存在: {}", base.display());
    }

    let mut dir = tokio_fs::read_dir(&base).await?;
    let mut paths = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await {
        paths.push(entry.path());
    }

    // 合理默认并发：至少 1，且不超过 CPU * 4（可以根据 IO 特性调优）
    let concurrency = if concurrency == 0 {
        std::cmp::max(1, num_cpus::get().saturating_mul(4))
    } else {
        concurrency
    };
    let base_clone = base.clone();

    let tasks_stream = stream::iter(paths.into_iter().map(move |path| {
        let base_for_task = base_clone.clone();
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
