use std::fs;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{Ordering};
use tauri::AppHandle;
use tracing::{debug, error, info};
use crate::commands::cancel_install::CANCEL_INSTALL;
use crate::config::config::read_config;
use crate::core::minecraft::appx::extract_zip::extract_zip;
use crate::core::minecraft::appx::utils::patch_manifest;
use crate::core::result::CoreResult;

#[tauri::command]
pub async fn extract_zip_appx(
    file_name: String,
    destination: String,
    force_replace: bool,
    delete_signature: bool,
    app: AppHandle,
) -> Result<(), String> {
    debug!(
        "收到解压请求：file_name='{}', destination='{}', force_replace={}, delete_signature={}",
        file_name, destination, force_replace, delete_signature
    );

    CANCEL_INSTALL.store(false, Ordering::SeqCst);

    let versions_root = Path::new("./BMCBL/versions");
    fs::create_dir_all(versions_root).map_err(|e| e.to_string())?;
    let stem = Path::new(&file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "无法解析文件名的主干部分".to_string())?;

    debug!("解析到的文件主干（stem）：'{}'", stem);

    let extract_to = versions_root.join(stem);

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

    match extract_zip(
        archive,
        extract_to.to_str().unwrap(),
        force_replace,
        app.clone(),
    ).await {
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
    
    if game_cfg.modify_appx_manifest {
        match patch_manifest(&extract_to) {
            Ok(true)  => info!("Manifest 修改成功"),
            Ok(false) => info!("未找到 Manifest，跳过修改"),
            Err(e)    => {
                error!("修改 Manifest 失败：{}", e);
                debug!("修改 Manifest 失败详细：{}，目录：{}", e, extract_to.display());
                return Err(format!("修改 Manifest 失败：{}", e));
            }
        }
    } else {
        info!("配置禁用了 Manifest 修改，跳过 patch_manifest");
    }

    Ok(())
}
