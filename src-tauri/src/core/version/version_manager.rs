use serde::Serialize;
use std::{collections::HashMap, time::Instant};
use futures::StreamExt;
use tokio::{ task};
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

    // 打开目录
    let rd = match read_dir(folder).await {
        Ok(d) => d,
        Err(e) => {
            debug!("读取目录失败: {}", e);
            return serde_json::json!({ "error": "读取目录失败" });
        }
    };

    // 转成 Stream
    let dir_stream = ReadDirStream::new(rd)
        .filter_map(|res| async move {
            match res {
                Ok(entry) if entry.path().is_dir() => Some(entry),
                Ok(entry) => {
                    debug!("跳过非目录: {:?}", entry.path());
                    None
                }
                Err(e) => {
                    debug!("遍历目录项出错: {}", e);
                    None
                }
            }
        });

    // 并发度
    let concurrency = num_cpus::get().max(2);

    // 并发处理
    let versions: Vec<(String, AppxVersion)> = dir_stream
        .map(|entry| {
            let path_buf = entry.path();
            async move {
                // 子目录名
                let folder_name = match path_buf.file_name().and_then(|f| f.to_str()).map(str::to_string) {
                    Some(n) => n,
                    None => return None,
                };
                // 调用异步函数，内部已做 blocking 文件读取
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
