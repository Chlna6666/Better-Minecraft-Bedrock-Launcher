use std::fs;
use std::fs::File;
use std::path::Path;
use std::sync::atomic::{Ordering};
use tauri::AppHandle;
use tracing::{debug, error, info};
use zip::ZipArchive;
use crate::commands::cancel_install::CANCEL_INSTALL;
use crate::core::minecraft::appx::extract_zip::{extract_zip};
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
    // 打印传入参数（中文）
    debug!(
        "收到解压请求：file_name='{}', destination='{}', force_replace={}, delete_signature={}",
        file_name, destination, force_replace, delete_signature
    );

    // 这里假定你在其它地方定义了 CANCEL_INSTALL
    CANCEL_INSTALL.store(false, Ordering::SeqCst);

    let versions_root = Path::new("./BMCBL/versions");
    fs::create_dir_all(versions_root).map_err(|e| e.to_string())?;
    let stem = Path::new(&file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "无法解析文件名的主干部分".to_string())?;

    // 打印解析出的 stem（中文）
    debug!("解析到的文件主干（stem）：'{}'", stem);

    let extract_to = versions_root.join(stem);

    // 打印最终解压目标路径（中文）
    debug!("最终解压目标目录：{}", extract_to.display());

    fs::create_dir_all(&extract_to).map_err(|e| e.to_string())?;

    let file = File::open(&destination).map_err(|e| {
        debug!("打开目标文件失败：{}，路径：{}", e, destination);
        e.to_string()
    })?;

    // 打印打开文件成功（中文）
    debug!("已打开待解压文件：{}", destination);

    let archive = zip::ZipArchive::new(file).map_err(|e| {
        debug!("创建 ZipArchive 失败：{}，路径：{}", e, destination);
        e.to_string()
    })?;

    // 打印 zip 条目数量（中文）
    debug!("ZipArchive 条目数：{}", archive.len());

    // 调用改造后的 extract_zip（返回 Result<CoreResult<()>, CoreError>）
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
            debug!("解压被用户取消：file_name='{}', destination='{}'",file_name, destination);
            return Ok(());
        }

        Ok(CoreResult::Error(e)) => {
            error!("解压内部错误：{}", e);
            debug!("解压内部错误详细：{}，file_name='{}', destination='{}'", e, file_name, destination);
            return Err(format!("解压失败：{}", e));
        }
        Err(core_err) => {
            // 这里是 CoreError（例如 Zip 或 Io 等从 ? 上抛出的错误）
            error!("解压失败（外层错误）：{}", core_err);
            debug!("解压外层错误详细：{}，file_name='{}', destination='{}'", core_err, file_name, destination);
            return Err(format!("解压失败：{}", core_err));
        }
    }

    //删除签名（可选）
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



    // 创建 mods 目录
    let mods_dir = extract_to.join("mods");
    std::fs::create_dir_all(&mods_dir)
        .map_err(|e| {
            debug!("创建 mods 目录失败：{}，目录：{}", e, mods_dir.display());
            format!("创建 mods 目录失败：{}", e)
        })?;
    info!("已创建 mods 目录：{}", mods_dir.display());
    debug!("mods 目录路径：{}", mods_dir.display());

    // 再修改清单
    match patch_manifest(&extract_to) {
        Ok(true)  => info!("Manifest 修改成功"),
        Ok(false) => info!("未找到 Manifest，跳过修改"),
        Err(e)    => {
            error!("修改 Manifest 失败：{}", e);
            debug!("修改 Manifest 失败详细：{}，目录：{}", e, extract_to.display());
            return Err(format!("修改 Manifest 失败：{}", e));
        }
    }

    Ok(())
}
