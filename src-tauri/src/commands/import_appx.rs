use std::fs;
use std::fs::File;
use std::path::{Path};
use std::sync::atomic::Ordering;
use tauri::AppHandle;
use tracing::{debug, error, info};
use crate::commands::cancel_install::CANCEL_INSTALL;
use crate::config::config::read_config;
use crate::core::minecraft::appx::extract_zip::extract_zip;
use crate::core::minecraft::appx::utils::patch_manifest;
use crate::core::result::CoreResult;

#[tauri::command]
pub async fn import_appx(
    source_path: String,      // 前端传入的本地文件路径（绝对或相对）
    file_name: Option<String>,
    app: AppHandle,
) -> Result<(), String> {
    debug!(
        "收到导入请求：source_path='{}', file_name='{:?}'",
        source_path, file_name,
    );

    CANCEL_INSTALL.store(false, Ordering::SeqCst);

    // 检查源文件是否存在并且是文件
    let src = Path::new(&source_path);
    if !src.exists() {
        let msg = format!("源文件不存在：{}", source_path);
        error!("{}", msg);
        return Err(msg);
    }
    if !src.is_file() {
        let msg = format!("源路径不是文件：{}", source_path);
        error!("{}", msg);
        return Err(msg);
    }

    // 决定用于解压目录名的文件名（优先使用 file_name，否则使用源文件名）
    let dest_file_name = if let Some(f) = file_name {
        f
    } else {
        src.file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "import_unknown.appx".to_string())
    };

    // 确保文件名以 .appx 结尾（和之前行为保持一致）
    let dest_file_name = if !dest_file_name.to_lowercase().ends_with(".appx") {
        format!("{}.appx", dest_file_name)
    } else {
        dest_file_name
    };

    // 创建 versions 目录
    let versions_root = Path::new("./BMCBL/versions");
    if let Err(e) = fs::create_dir_all(versions_root) {
        error!("创建 versions 目录失败：{}，目录：{}", e, versions_root.display());
        return Err(format!("创建版本目录失败：{}", e));
    }

    // 从 dest_file_name 提取 stem 作为解压目录名
    let stem = Path::new(&dest_file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "无法解析文件名的主干部分".to_string())?;

    debug!("解析到的文件主干（stem）：'{}'", stem);

    let extract_to = versions_root.join(stem);
    debug!("最终解压目标目录：{}", extract_to.display());

    if let Err(e) = fs::create_dir_all(&extract_to) {
        error!("创建解压目标目录失败：{}，目录：{}", e, extract_to.display());
        return Err(format!("创建解压目标目录失败：{}", e));
    }

    // 直接打开前端传入的源文件（不再复制）
    let file = match File::open(src) {
        Ok(f) => f,
        Err(e) => {
            error!("打开源文件失败：{}，路径：{}", e, src.display());
            return Err(e.to_string());
        }
    };

    debug!("已打开待解压源文件：{}", src.display());

    let archive = match zip::ZipArchive::new(file) {
        Ok(a) => a,
        Err(e) => {
            error!("创建 ZipArchive 失败：{}，路径：{}", e, src.display());
            return Err(e.to_string());
        }
    };

    debug!("ZipArchive 条目数：{}", archive.len());

    match extract_zip(
        archive,
        extract_to.to_str().unwrap(),
        true, // 导入默认 force_replace = true（如需可调整）
        app.clone(),
    ).await {
        Ok(CoreResult::Success(())) => {
            info!("导入解压完成：{}", extract_to.display());
        }
        Ok(CoreResult::Cancelled) => {
            info!("用户取消了解压（导入流程）");
            debug!("导入解压被用户取消：source='{}'", source_path);

            // 取消时删除解压目标目录（不删除源文件）
            if let Err(e) = fs::remove_dir_all(&extract_to) {
                error!("取消导入时删除解压目录失败：{}，目录：{}", e, extract_to.display());
            } else {
                info!("取消导入时已删除解压目录：{}", extract_to.display());
            }

            return Ok(());
        }

        Ok(CoreResult::Error(e)) => {
            error!("导入解压内部错误：{}", e);
            debug!("导入解压内部错误详细：{}，source='{}'", e, source_path);
            // 出错时尝试清理解压目录
            let _ = fs::remove_dir_all(&extract_to);
            return Err(format!("解压失败：{}", e));
        }
        Err(core_err) => {
            error!("导入解压失败（外层错误）：{}", core_err);
            debug!("导入解压外层错误详细：{}，source='{}'", core_err, source_path);
            let _ = fs::remove_dir_all(&extract_to);
            return Err(format!("解压失败：{}", core_err));
        }
    }


    // 尝试删除解压目录中的签名文件（同原逻辑）
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
    if let Err(e) = std::fs::create_dir_all(&mods_dir) {
        debug!("创建 mods 目录失败：{}，目录：{}", e, mods_dir.display());
        return Err(format!("创建 mods 目录失败：{}", e));
    }
    info!("已创建 mods 目录：{}", mods_dir.display());
    debug!("mods 目录路径：{}", mods_dir.display());

    let config = read_config().map_err(|e| e.to_string())?;
    let game_cfg = &config.game;
    // ✅ 判断配置是否启用 Manifest 修改
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
