use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use tracing::{debug, error, info};

use crate::commands::cancel_install::CANCEL_INSTALL;
use crate::config::config::read_config;
use crate::core::minecraft::appx::extract_zip::extract_zip;
use crate::core::minecraft::appx::utils::get_manifest_identity;
use crate::result::CoreResult;

// 我们的补丁库（请根据实际模块路径确认）
use crate::core::minecraft::key_patcher::{patch_path, PatchError};

#[tauri::command]
pub async fn extract_zip_appx(
    file_name: String,
    destination: String,
    force_replace: bool,
    _delete_signature: bool,
) -> Result<(), String> {
    debug!(
        "收到解压请求：file_name='{}', destination='{}', force_replace={}, delete_signature={}",
        file_name, destination, force_replace, _delete_signature
    );

    CANCEL_INSTALL.store(false, Ordering::SeqCst);

    let versions_root = Path::new("./BMCBL/versions");
    fs::create_dir_all(versions_root).map_err(|e| e.to_string())?;
    let stem = Path::new(&file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "无法解析文件名的主干部分".to_string())?;

    debug!("解析到的文件主干（stem）：'{}'", stem);

    let extract_to: PathBuf = versions_root.join(stem);

    debug!("最终解压目标目录：{}", extract_to.display());

    fs::create_dir_all(&extract_to).map_err(|e| e.to_string())?;

    let file = File::open(&destination).map_err(|e| {
        debug!("打开目标文件失败：{}，路径：{}", e, destination);
        e.to_string()
    })?;

    debug!("已打开待解压文件：{}", destination);

    let archive = zip::ZipArchive::new(file).map_err(|e| {
        debug!("创建 ZipArchive 失败：{}，路径：{}", e, destination);
        e.to_string()
    })?;

    debug!("ZipArchive 条目数：{}", archive.len());

    match extract_zip(archive, extract_to.to_str().unwrap(), force_replace).await {
        Ok(CoreResult::Success(())) => {
            info!("解压完成：{}", extract_to.display());
        }
        Ok(CoreResult::Cancelled) => {
            info!("用户取消了解压");
            debug!("解压被用户取消：file_name='{}', destination='{}'", file_name, destination);

            // 删除原文件
            if let Err(e) = fs::remove_file(&destination) {
                error!("取消解压时删除原文件失败：{}，路径：{}", e, destination);
            } else {
                info!("取消解压时已删除原文件：{}", destination);
            }

            // 删除解压目标目录
            if let Err(e) = fs::remove_dir_all(&extract_to) {
                error!("取消解压时删除解压目录失败：{}，目录：{}", e, extract_to.display());
            } else {
                info!("取消解压时已删除解压目录：{}", extract_to.display());
            }

            return Ok(());
        }

        Ok(CoreResult::Error(e)) => {
            error!("解压内部错误：{}", e);
            debug!("解压内部错误详细：{}，file_name='{}', destination='{}'", e, file_name, destination);
            return Err(format!("解压失败：{}", e));
        }
        Err(core_err) => {
            error!("解压失败（外层错误）：{}", core_err);
            debug!("解压外层错误详细：{}，file_name='{}', destination='{}'", core_err, file_name, destination);
            return Err(format!("解压失败：{}", core_err));
        }
    }

    // 读取配置，判断是否保留 APPX 源文件
    let config = read_config().map_err(|e| e.to_string())?;
    let game_cfg = &config.game;
    if !game_cfg.keep_appx_after_install {
        if let Err(e) = fs::remove_file(&destination) {
            error!("解压完成后删除源文件失败：{}，路径：{}", e, destination);
        } else {
            info!("解压完成后已删除源文件：{}", destination);
        }
    } else {
        info!("配置开启了保留 APPX，源文件未删除：{}", destination);
    }

    // 如果需要也删除解压目录内的签名
    let sig = extract_to.join("AppxSignature.p7x");
    if sig.exists() {
        info!("删除解压目录中的签名文件：{}", sig.display());
        match fs::remove_file(&sig) {
            Ok(()) => debug!("签名删除成功：{}", sig.display()),
            Err(e) => error!("签名删除失败：{}，路径：{}", e, sig.display()),
        }
    } else {
        debug!("未在解压目录找到签名文件（无需删除）：{}", sig.display());
    }

    let mods_dir = extract_to.join("mods");
    std::fs::create_dir_all(&mods_dir)
        .map_err(|e| {
            debug!("创建 mods 目录失败：{}，目录：{}", e, mods_dir.display());
            format!("创建 mods 目录失败：{}", e)
        })?;
    info!("已创建 mods 目录：{}", mods_dir.display());
    debug!("mods 目录路径：{}", mods_dir.display());

    // 读取 manifest identity 并在满足条件时尝试 patch
    if let Some(extract_path_str) = extract_to.to_str() {
        match get_manifest_identity(extract_path_str).await {
            Ok((name, version)) => {
                debug!("解析到的 Manifest Identity Name: {}, Version: {}", name, version);

                if matches!(name.as_str(), "Microsoft.MinecraftUWP" | "Microsoft.MinecraftWindowsBeta")
                {
                    // 只有在识别为 Minecraft UWP/WindowsBeta 且配置允许时才修改 manifest
                    if game_cfg.modify_appx_manifest {
                        match crate::core::minecraft::appx::utils::patch_manifest(&extract_to) {
                            Ok(true) => info!("Manifest 修改成功"),
                            Ok(false) => info!("未找到 Manifest，跳过修改"),
                            Err(e) => {
                                error!("修改 Manifest 失败：{}", e);
                                debug!("修改 Manifest 失败详细：{}，目录：{}", e, extract_to.display());
                                return Err(format!("修改 Manifest 失败：{}", e));
                            }
                        }
                    } else {
                        info!("配置禁用了 Manifest 修改，跳过 patch_manifest");
                    }

                    // ---------- 版本判断（触发条件：版本 < 1.21） ----------
                    // 更稳健地提取 major/minor，缺项视为 0，忽略非数字后缀
                    let mut parts_iter = version
                        .split('.')
                        .filter_map(|s| s.trim().split(|c: char| !c.is_digit(10)).next())
                        .filter_map(|s| s.parse::<u32>().ok());

                    let major = parts_iter.next().unwrap_or(0);
                    let minor = parts_iter.next().unwrap_or(0);

                    let needs_patch = (major < 1) || (major == 1 && minor < 21);

                    if needs_patch {
                        info!("检测到旧版本 (<1.21.x)，准备执行补丁 (patch_path) 于：{}", extract_to.display());

                        // 使用 spawn_blocking 在后台线程执行同步 patch_path
                        let extract_clone = extract_to.clone();
                        let join_handle = tokio::task::spawn_blocking(move || {
                            // 调用库函数：patch_path(&Path)
                            patch_path(&extract_clone)
                        });

                        match join_handle.await {
                            Ok(Ok(bak_path)) => {
                                info!("补丁应用成功，备份创建于：{}", bak_path.display());
                            }
                            Ok(Err(patch_err)) => {
                                match patch_err {
                                    PatchError::Io(ioe) => {
                                        error!("补丁失败（IO 错误）：{}", ioe);
                                    }
                                    PatchError::InvalidExeName => {
                                        error!("补丁失败：目标文件名无效（不是 Minecraft.Windows.exe）");
                                    }
                                    PatchError::KeyNotFound => {
                                        info!("补丁未执行：未在目标文件中发现旧公钥（KeyNotFound）。可能已被补丁或使用不同密钥。");
                                    }
                                    PatchError::BackupFailed(ioe) => {
                                        error!("补丁失败（备份创建失败）：{}", ioe);
                                    }
                                }
                            }
                            Err(join_err) => {
                                error!("执行补丁任务失败（join error）：{}", join_err);
                            }
                        }
                    } else {
                        info!("版本 >= 1.21，跳过补丁逻辑。解析到版本: {}", version);
                    }
                } else {
                    info!("非 Minecraft UWP/WindowsBeta 包（{}），将跳过 Manifest 修改与补丁逻辑", name);
                }
            }
            Err(e) => {
                debug!("获取 Manifest Identity 失败，跳过 Manifest 修改与补丁：{}", e);
            }
        }
    } else {
        debug!("extract_to 路径无法转换为字符串，跳过 Manifest 修改");
    }

    Ok(())
}
