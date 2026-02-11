use crate::core::minecraft::appx::utils::get_manifest_identity;
use futures::StreamExt;
use serde::Serialize;
use serde_json::Value;
use std::{collections::HashMap, path::PathBuf, time::Instant};
use tokio::fs;
use tokio::fs::read_dir;
use tokio_stream::wrappers::ReadDirStream;
use tracing::debug;

#[derive(Serialize, Clone)]
struct AppxVersion {
    name: String,
    version: String,
    path: String, // 完整绝对路径
    kind: String, // "UWP" 或 "GDK"
    config: Value,
}

/// 简单解析：把版本段尽量保留为整数（不要把长段再拆）
/// 例如 "1.21.12201.0" -> vec![1,21,12201,0]
fn parse_version_to_vec_simple(v: &str) -> Vec<u64> {
    v.split(|c| c == '.' || c == '-' || c == '+')
        .map(|seg| {
            let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().unwrap_or(0)
        })
        .collect()
}

fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let va = parse_version_to_vec_simple(a);
    let vb = parse_version_to_vec_simple(b);
    let n = std::cmp::max(va.len(), vb.len());
    for i in 0..n {
        let ai = *va.get(i).unwrap_or(&0);
        let bi = *vb.get(i).unwrap_or(&0);
        match ai.cmp(&bi) {
            std::cmp::Ordering::Equal => continue,
            non_eq => return non_eq,
        }
    }
    std::cmp::Ordering::Equal
}

/// 新的判定函数：优先处理你观测到的特殊情况（1.21.12201.* 为 GDK），其余回退到阈值比较
fn is_win32_version(version: &str) -> bool {
    // 先把版本拆为数字向量
    let v = parse_version_to_vec_simple(version);

    // 特殊规则：对于 1.21 系列，如果第三段 >= 12201，则视为 GDK
    if v.len() >= 3 {
        if v[0] == 1 && v[1] == 21 && v[2] >= 12201 {
            return true;
        }
    }

    const THRESHOLD: &str = "1.21.12000.20";
    compare_versions(version, THRESHOLD) != std::cmp::Ordering::Less
}

/// 根据版本字符串返回 "GDK" 或 "UWP"
fn determine_kind_from_version(version: &str) -> String {
    if is_win32_version(version) {
        "GDK".to_string()
    } else {
        "UWP".to_string()
    }
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
    let concurrency = (num_cpus::get() * 8).max(16);

    let versions: Vec<(String, AppxVersion)> = dir_stream
        .filter_map(|res| async {
            match res {
                Ok(entry) => match entry.file_type().await {
                    Ok(ft) if ft.is_dir() => Some(entry),
                    Ok(_) => {
                        debug!("跳过非目录: {:?}", entry.path());
                        None
                    }
                    Err(e) => {
                        debug!("获取文件类型失败: {}", e);
                        None
                    }
                },
                Err(e) => {
                    debug!("遍历目录项出错: {}", e);
                    None
                }
            }
        })
        .map(|entry| {
            async move {
                let path_buf: PathBuf = match fs::canonicalize(entry.path()).await {
                    Ok(p) => p,
                    Err(e) => {
                        debug!("获取绝对路径失败 {}: {}", entry.path().display(), e);
                        return None;
                    }
                };

                let folder_name = path_buf
                    .file_name()
                    .and_then(|f| f.to_str())
                    .map(str::to_string);

                let folder_name = match folder_name {
                    Some(n) => n,
                    None => return None,
                };

                let config = match fs::read_to_string(path_buf.join("config.json")).await {
                    Ok(raw) => serde_json::from_str::<Value>(&raw).unwrap_or_else(|_| serde_json::json!({})),
                    Err(_) => serde_json::json!({}),
                };

                // 使用你已有的 get_manifest_identity 获取 name/version
                match get_manifest_identity(&path_buf.to_string_lossy()).await {
                    Ok((name, version)) => {
                        // 基于 version 判定类型
                        let kind = determine_kind_from_version(&version);
                        Some((
                            folder_name.clone(),
                            AppxVersion {
                                name,
                                version,
                                path: path_buf.to_string_lossy().to_string(),
                                kind,
                                config,
                            },
                        ))
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

    debug!(
        "get_appx_version_list 完成，用时: {:?}, 并发度: {}",
        start.elapsed(),
        concurrency
    );
    serde_json::to_value(&result).unwrap_or_else(|_| serde_json::json!({ "error": "序列化失败" }))
}
