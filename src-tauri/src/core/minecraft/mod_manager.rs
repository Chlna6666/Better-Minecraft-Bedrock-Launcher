use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ModManifest {
    name: String,
    entry: String,
    #[serde(rename = "type")]
    mod_type: String,
    /// Only meaningful for `type = "hot-inject"`; handled by `BLoader.dll`.
    #[serde(default)]
    inject_delay_ms: Option<u64>,
}

/// 扫描 mods 目录，返回需要加载的 DLL **绝对路径**列表
/// 逻辑：扫描子文件夹 -> 检查 manifest.json (已启用) -> 解析 entry -> 返回路径
///
/// 注意：`inject_delay_ms` 的延迟注入由 `BLoader.dll` 处理，这里只负责读取并提供 DLL 路径。
/// 返回: Vec<(AbsolutePath, DelayMs)> (Delay 固定为 0)
pub async fn load_mods_config(mods_dir: &Path) -> Result<Vec<(PathBuf, u64)>> {
    let mut result = Vec::new();
    let mut has_preloader = false;
    let mut preloader_path: Option<PathBuf> = None;

    // 确保目录存在
    if !mods_dir.exists() {
        fs::create_dir_all(mods_dir).await
            .context("Failed to create mods directory")?;
        return Ok(result);
    }

    // 预扫描：检测第三方加载器 PreLoader.dll（启用状态下）
    // 如果存在，PreLoader 可以代理加载除 hot-inject 外的其它类型，且可完全代理 preload-native。
    {
        let mut entries = fs::read_dir(mods_dir).await?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest_path = path.join("manifest.json");
            if !manifest_path.exists() {
                continue;
            }
            if let Ok(content) = fs::read_to_string(&manifest_path).await {
                if let Ok(manifest) = serde_json::from_str::<ModManifest>(&content) {
                    if manifest.entry.eq_ignore_ascii_case("PreLoader.dll") {
                        let dll_path = path.join(&manifest.entry);
                        if dll_path.exists() {
                            if let Ok(abs_path) = fs::canonicalize(&dll_path).await {
                                let mut clean_path = abs_path.clone();
                                if let Some(str_path) = abs_path.to_str() {
                                    if str_path.starts_with(r"\\?\") {
                                        clean_path = PathBuf::from(&str_path[4..]);
                                    }
                                }
                                has_preloader = true;
                                preloader_path = Some(clean_path);
                                break;
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(p) = preloader_path {
        debug!("检测到第三方加载器 PreLoader.dll，将仅由 Launcher 直接加载 hot-inject（以及 PreLoader 本体）：{}", p.display());
        result.push((p, 0));
    }

    let mut entries = fs::read_dir(mods_dir).await?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();

        // 只处理目录
        if path.is_dir() {
            // 检查 manifest.json (如果只有 .manifest.json 则视为禁用，不加载)
            let manifest_path = path.join("manifest.json");

            if manifest_path.exists() {
                // 读取并解析
                match fs::read_to_string(&manifest_path).await {
                    Ok(content) => {
                        match serde_json::from_str::<ModManifest>(&content) {
                            Ok(manifest) => {
                                // 检查类型
                                // 如果存在 PreLoader.dll，则第三方加载器将代理除 hot-inject 外的类型，且可完全代理 preload-native。
                                if has_preloader {
                                    if manifest.mod_type != "hot-inject" {
                                        continue;
                                    }
                                    // 避免重复加入 PreLoader.dll 自身
                                    if manifest.entry.eq_ignore_ascii_case("PreLoader.dll") {
                                        continue;
                                    }
                                } else if manifest.mod_type != "preload-native" && manifest.mod_type != "hot-inject" {
                                    continue;
                                }

                                    let dll_path = path.join(&manifest.entry);
                                    if dll_path.exists() {
                                        // 获取规范路径
                                        if let Ok(abs_path) = fs::canonicalize(&dll_path).await {
                                            let mut clean_path = abs_path.clone();
                                            // 去除 Windows UNC 前缀
                                            if let Some(str_path) = abs_path.to_str() {
                                                if str_path.starts_with(r"\\?\") {
                                                    clean_path = PathBuf::from(&str_path[4..]);
                                                }
                                            }
                                            debug!("加载 Mod: {} ({})", manifest.name, clean_path.display());
                                            result.push((clean_path, 0)); // Delay 由 BLoader 处理
                                        }
                                    } else {
                                        warn!("Manifest 指定的 DLL 不存在: {}", dll_path.display());
                                    }
                            },
                            Err(e) => {
                                warn!("Manifest 解析错误 {}: {}", manifest_path.display(), e);
                            }
                        }
                    },
                    Err(e) => {
                        warn!("无法读取 Manifest {}: {}", manifest_path.display(), e);
                    }
                }
            }
        }
        // 如果是根目录下的 .dll 文件，Preloader 会在启动时自动打包
        // 但 mod_manager 此时不处理它们，等待下次启动变为文件夹后再加载
        // 这样保持了逻辑分离：Preloader 负责整理，Launcher 负责读取已整理好的
    }

    Ok(result)
}
