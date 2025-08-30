use serde::Serialize;
use std::{collections::HashMap, time::Instant, path::PathBuf};
use futures::StreamExt;
use tokio::fs::read_dir;
use tokio_stream::wrappers::ReadDirStream;
use tracing::debug;
use crate::core::minecraft::appx::utils::get_manifest_identity;

#[derive(Serialize, Clone)]
struct AppxVersion {
    name: String,
    version: String,
}

pub async fn get_appx_version_list(folder: &str) -> serde_json::Value {
    let start = Instant::now();
    debug!("开始读取目录: {}", folder);

    let rd = match read_dir(folder).await {
        Ok(d) => d,
        Err(e) => {
            debug!("读取目录失败: {}", e);
            return serde_json::json!({ "error": "读取目录失败" });
        }
    };

    let dir_stream = ReadDirStream::new(rd);

    // 并发
    let concurrency = (num_cpus::get() * 2).max(16);

    let versions: Vec<(String, AppxVersion)> = dir_stream
        .filter_map(|res| async {
            match res {
                Ok(entry) => {
                    // 注意：使用异步 API 判断是否目录，避免阻塞
                    match entry.file_type().await {
                        Ok(ft) if ft.is_dir() => Some(entry),
                        Ok(_) => {
                            debug!("跳过非目录: {:?}", entry.path());
                            None
                        }
                        Err(e) => {
                            debug!("获取文件类型失败: {}", e);
                            None
                        }
                    }
                }
                Err(e) => {
                    debug!("遍历目录项出错: {}", e);
                    None
                }
            }
        })
        .map(|entry| {
            // 这里是纯 async，调用已经不阻塞的 get_manifest_identity
            async move {
                let path_buf: PathBuf = entry.path();
                let folder_name = match path_buf.file_name().and_then(|f| f.to_str()).map(str::to_string) {
                    Some(n) => n,
                    None => return None,
                };
                match get_manifest_identity(&path_buf.to_string_lossy()).await {
                    Ok((name, version)) => {
                        debug!("解析成功 {} -> {} {}", folder_name, name, version);
                        Some((folder_name, AppxVersion { name, version }))
                    }
                    Err(e) => {
                        debug!("解析失败 {}: {}", path_buf.display(), e);
                        None
                    }
                }
            }
        })
        .buffer_unordered(concurrency)
        .filter_map(|opt| async move { opt })
        .collect()
        .await;

    let result: HashMap<_, _> = versions.into_iter().collect();

    debug!("get_appx_version_list 完成，用时: {:?}, 并发度: {}", start.elapsed(), concurrency);
    serde_json::to_value(&result).unwrap_or_else(|_| serde_json::json!({ "error": "序列化失败" }))
}
