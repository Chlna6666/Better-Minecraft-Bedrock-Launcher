use std::io::Read;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use futures::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::fs as tokio_fs;

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
    pub icon_rel: Option<String>,
    pub level_dat_size: Option<u64>,
    pub modified: Option<String>,
    pub behavior_packs: Option<Value>,
    pub resource_packs: Option<Value>,
}

fn systemtime_to_iso(t: SystemTime) -> String {
    let dt: DateTime<Utc> = t.into();
    dt.to_rfc3339()
}


pub async fn list_minecraft_worlds(concurrency: usize) -> Result<Vec<McMapInfo>> {
    let base = default_minecraft_worlds_path().context("无法确定 minecraftWorlds 路径")?;
    if !base.exists() || !base.is_dir() { anyhow::bail!("路径不存在: {}", base.display()); }

    let mut dir = tokio_fs::read_dir(&base).await?;
    let mut paths = Vec::new();
    while let Ok(Some(entry)) = dir.next_entry().await { paths.push(entry.path()); }

    let concurrency = if concurrency == 0 { std::cmp::max(1, num_cpus::get() * 2) } else { concurrency };
    let base_clone = base.clone();

    let tasks_stream = stream::iter(paths.into_iter().map(move |path| {
        let base_for_task = base_clone.clone();
        async move {
            let md = tokio_fs::metadata(&path).await.ok()?;
            if !md.is_dir() { return None; }

            let folder_name = path.file_name()?.to_string_lossy().into_owned();
            let folder_path_str = path.to_string_lossy().into_owned();
            let mut info = McMapInfo {
                folder_name: folder_name.clone(), folder_path: folder_path_str.clone(),
                level_name: None, icon_path: None, icon_rel: None,
                level_dat_size: None, modified: None, behavior_packs: None, resource_packs: None,
            };

            if let Ok(content) = tokio_fs::read_to_string(path.join("levelname.txt")).await {
                let s = content.trim(); if !s.is_empty() { info.level_name = Some(s.to_string()); }
            }

            let icon_file = path.join("world_icon.jpeg");
            if tokio_fs::metadata(&icon_file).await.is_ok() {
                info.icon_path = Some(icon_file.to_string_lossy().into_owned());
                info.icon_rel = Some(format!("{}/world_icon.jpeg", folder_name));
            }


            for (field, name) in [(&mut info.behavior_packs, "world_behavior_packs.json"), (&mut info.resource_packs, "world_resource_packs.json")] {
                let json_file = path.join(name);
                if tokio_fs::metadata(&json_file).await.is_ok() {
                    if let Ok(s) = tokio_fs::read_to_string(&json_file).await {
                        if let Ok(v) = serde_json::from_str::<Value>(&s) { *field = Some(v); }
                    }
                }
            }

            Some(info)
        }
    })).buffer_unordered(concurrency);

    Ok(tasks_stream.filter_map(|m| async move { m }).collect().await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tracing_subscriber;

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
