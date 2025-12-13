use crate::archive::zip::extract_zip;
use crate::config::config::read_config;
use crate::core::minecraft::appx::utils::{get_manifest_identity, patch_manifest};
use crate::core::minecraft::key_patcher::{patch_path, PatchResult};
use crate::result::CoreResult;
use crate::tasks::task_manager::{create_task, finish_task, is_cancelled, update_progress};
use std::fs;
use std::fs::File;
use std::path::Path;
use std::path::PathBuf;
use tracing::{debug, error, info};

#[tauri::command]
pub async fn import_appx(
    source_path: String, // 前端传入的本地文件路径（绝对或相对）
    file_name: Option<String>,
) -> Result<String, String> {
    debug!(
        "收到导入请求：source_path='{}', file_name='{:?}'",
        source_path, file_name,
    );

    // 1. 创建任务并立刻返回 id 给前端
    let task_id = create_task(None, "extracting", None);

    // 2. spawn 后台任务执行导入流程（打开文件 -> 解压 -> 后处理）
    //    重要：为后台任务 clone 一份 task_id，避免 move 后无法在当前函数返回
    let task_id_for_task = task_id.clone();
    let source_clone = source_path.clone();
    let file_name_clone = file_name.clone();

    tokio::spawn(async move {
        // small initial update
        update_progress(&task_id_for_task, 0, None, Some("starting"));

        // 检查源文件存在
        let src = Path::new(&source_clone);
        if !src.exists() || !src.is_file() {
            let msg = format!("源文件不存在或不是文件：{}", source_clone);
            error!("{}", msg);
            finish_task(&task_id_for_task, "error", Some(msg));
            return;
        }

        // 确定解压目录名
        let dest_file_name = if let Some(f) = file_name_clone {
            f
        } else {
            src.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| "import_unknown.appx".to_string())
        };

        let dest_file_name = if !dest_file_name.to_lowercase().ends_with(".appx") {
            format!("{}.appx", dest_file_name)
        } else {
            dest_file_name
        };

        // 准备版本目录
        let versions_root = Path::new("./BMCBL/versions");
        if let Err(e) = fs::create_dir_all(versions_root) {
            let msg = format!(
                "创建 versions 目录失败：{}，目录：{}",
                e,
                versions_root.display()
            );
            error!("{}", msg);
            finish_task(&task_id_for_task, "error", Some(msg));
            return;
        }

        let stem = Path::new(&dest_file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "import_unknown".to_string());

        let extract_to = versions_root.join(stem);

        if let Err(e) = fs::create_dir_all(&extract_to) {
            let msg = format!(
                "创建解压目标目录失败：{}，目录：{}",
                e,
                extract_to.display()
            );
            error!("{}", msg);
            finish_task(&task_id_for_task, "error", Some(msg));
            return;
        }

        // 打开并创建 ZipArchive（阻塞 IO）
        let file = match File::open(&src) {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("打开源文件失败：{}，路径：{}", e, src.display());
                error!("{}", msg);
                finish_task(&task_id_for_task, "error", Some(msg));
                return;
            }
        };

        let archive = match zip::ZipArchive::new(file) {
            Ok(a) => a,
            Err(e) => {
                let msg = format!("创建 ZipArchive 失败：{}，路径：{}", e, src.display());
                error!("{}", msg);
                finish_task(&task_id_for_task, "error", Some(msg));
                return;
            }
        };

        // 调用你的 extract_zip（它会使用 task_id 来上报进度/取消）
        match extract_zip(
            archive,
            extract_to.to_str().unwrap(),
            true,
            task_id_for_task.clone(),
        )
        .await
        {
            Ok(CoreResult::Success(())) => {
                info!("导入解压完成：{}", extract_to.display());
                // 后处理：删除签名文件（如果存在）
                let sig = extract_to.join("AppxSignature.p7x");
                if sig.exists() {
                    if let Err(e) = fs::remove_file(&sig) {
                        error!("签名删除失败：{}，路径：{}", e, sig.display());
                    } else {
                        debug!("签名删除成功：{}", sig.display());
                    }
                }

                // 创建 mods 目录（保持与原行为）
                let mods_dir = extract_to.join("mods");
                if let Err(e) = std::fs::create_dir_all(&mods_dir) {
                    let msg = format!("创建 mods 目录失败：{}，目录：{}", e, mods_dir.display());
                    error!("{}", msg);
                    // 报错但不一定要把任务设为 error；这里我们设为 error 以便前端能看到
                    finish_task(&task_id_for_task, "error", Some(msg));
                    return;
                }

                // 读取 manifest identity 并根据配置决定是否 patch
                match get_manifest_identity(extract_to.to_str().unwrap()).await {
                    Ok((name, _version)) => {
                        debug!("解析到的 Manifest Identity Name: {}", name);
                        if matches!(
                            name.as_str(),
                            "Microsoft.MinecraftUWP" | "Microsoft.MinecraftWindowsBeta"
                        ) {
                            if let Ok(cfg) = read_config() {
                                if cfg.game.modify_appx_manifest {
                                    match patch_manifest(&extract_to) {
                                        Ok(true) => info!("Manifest 修改成功"),
                                        Ok(false) => info!("未找到 Manifest，跳过修改"),
                                        Err(e) => {
                                            let msg = format!("修改 Manifest 失败：{}", e);
                                            error!("{}", msg);
                                            finish_task(&task_id_for_task, "error", Some(msg));
                                            return;
                                        }
                                    }
                                } else {
                                    info!("配置禁用了 Manifest 修改，跳过");
                                }
                            } else {
                                debug!("读取配置失败，跳过 Manifest 修改");
                            }
                        } else {
                            info!(
                                "非 Minecraft UWP/WindowsBeta 包（{}），跳过 Manifest 修改",
                                name
                            );
                        }
                    }
                    Err(e) => {
                        debug!("获取 Manifest Identity 失败，跳过 Manifest 修改：{}", e);
                    }
                }

                // 一切成功
                finish_task(&task_id_for_task, "completed", None);
            }

            Ok(CoreResult::Cancelled) => {
                info!("用户取消了解压（导入流程）");
                // 取消时删除解压目标目录（不删除源文件）
                if let Err(e) = fs::remove_dir_all(&extract_to) {
                    error!(
                        "取消导入时删除解压目录失败：{}，目录：{}",
                        e,
                        extract_to.display()
                    );
                } else {
                    info!("取消导入时已删除解压目录：{}", extract_to.display());
                }
                // finish_task 已在 extract_zip 中设置为 cancelled，若没有可补设：
                if !is_cancelled(&task_id_for_task) {
                    finish_task(
                        &task_id_for_task,
                        "cancelled",
                        Some("user cancelled".into()),
                    );
                }
            }

            Ok(CoreResult::Error(e)) => {
                error!("导入解压内部错误：{}", e);
                let _ = fs::remove_dir_all(&extract_to);
                finish_task(
                    &task_id_for_task,
                    "error",
                    Some(format!("extract error: {}", e)),
                );
            }

            Err(core_err) => {
                error!("导入解压失败（外层错误）：{}", core_err);
                let _ = fs::remove_dir_all(&extract_to);
                finish_task(
                    &task_id_for_task,
                    "error",
                    Some(format!("extract failed: {}", core_err)),
                );
            }
        }
    });

    // 立即返回 task_id 给前端（前端会通过 id 查询进度或取消）
    Ok(task_id)
}

#[tauri::command]
pub async fn extract_zip_appx(
    file_name: String,
    destination: String,
    force_replace: bool,
    delete_signature: bool,
) -> Result<String, String> {
    debug!(
        "收到解压请求：file_name='{}', destination='{}', force_replace={}, delete_signature={}",
        file_name, destination, force_replace, delete_signature
    );

    // 创建任务并立即返回 id
    let task_id = create_task(None, "extracting", None);

    // spawn 后台任务
    let file_name_clone = file_name.clone();
    let destination_clone = destination.clone();
    let task_id_clone = task_id.clone();

    tokio::spawn(async move {
        update_progress(&task_id_clone, 0, None, Some("starting"));

        // 准备目标路径
        let versions_root = Path::new("./BMCBL/versions");
        if let Err(e) = fs::create_dir_all(versions_root) {
            let msg = format!(
                "创建 versions 目录失败：{}，目录：{}",
                e,
                versions_root.display()
            );
            error!("{}", msg);
            finish_task(&task_id_clone, "error", Some(msg));
            return;
        }

        let stem = Path::new(&destination_clone)
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| file_name_clone.clone());

        let extract_to: PathBuf = versions_root.join(stem);

        if let Err(e) = fs::create_dir_all(&extract_to) {
            let msg = format!(
                "创建解压目标目录失败：{}，目录：{}",
                e,
                extract_to.display()
            );
            error!("{}", msg);
            finish_task(&task_id_clone, "error", Some(msg));
            return;
        }

        // 打开并创建 ZipArchive
        let file = match File::open(&destination_clone) {
            Ok(f) => f,
            Err(e) => {
                let msg = format!("打开目标文件失败：{}，路径：{}", e, destination_clone);
                error!("{}", msg);
                finish_task(&task_id_clone, "error", Some(msg));
                return;
            }
        };

        let archive = match zip::ZipArchive::new(file) {
            Ok(a) => a,
            Err(e) => {
                let msg = format!("创建 ZipArchive 失败：{}，路径：{}", e, destination_clone);
                error!("{}", msg);
                finish_task(&task_id_clone, "error", Some(msg));
                return;
            }
        };

        // 调用 extract_zip（它会使用 task_id 来上报进度/取消）
        match extract_zip(
            archive,
            extract_to.to_str().unwrap(),
            force_replace,
            task_id_clone.clone(),
        )
        .await
        {
            Ok(CoreResult::Success(())) => {
                info!("解压完成：{}", extract_to.display());
                // 删除源文件（根据配置或参数）
                if let Ok(cfg) = read_config() {
                    if !cfg.game.keep_appx_after_install {
                        if let Err(e) = fs::remove_file(&destination_clone) {
                            error!(
                                "解压完成后删除源文件失败：{}，路径：{}",
                                e, destination_clone
                            );
                        } else {
                            info!("解压完成后已删除源文件：{}", destination_clone);
                        }
                    } else {
                        info!("配置要求保留 APPX，未删除：{}", destination_clone);
                    }
                }
                // 删除签名（如果需要）
                let sig = extract_to.join("AppxSignature.p7x");
                if sig.exists() {
                    if let Err(e) = fs::remove_file(&sig) {
                        error!("签名删除失败：{}，路径：{}", e, sig.display());
                    } else {
                        debug!("签名删除成功：{}", sig.display());
                    }
                }

                // 创建 mods 目录
                if let Err(e) = std::fs::create_dir_all(extract_to.join("mods")) {
                    error!("创建 mods 目录失败：{}，目录：{}", e, extract_to.display());
                    finish_task(&task_id_clone, "error", Some("create mods failed".into()));
                    return;
                }

                // 尝试 patch manifest & maybe patch_path
                if let Some(extract_path_str) = extract_to.to_str() {
                    match get_manifest_identity(extract_path_str).await {
                        Ok((name, version)) => {
                            debug!(
                                "解析到的 Manifest Identity Name: {}, Version: {}",
                                name, version
                            );
                            if matches!(
                                name.as_str(),
                                "Microsoft.MinecraftUWP" | "Microsoft.MinecraftWindowsBeta"
                            ) {
                                if let Ok(cfg) = read_config() {
                                    if cfg.game.modify_appx_manifest {
                                        match crate::core::minecraft::appx::utils::patch_manifest(
                                            &extract_to,
                                        ) {
                                            Ok(true) => info!("Manifest 修改成功"),
                                            Ok(false) => info!("未找到 Manifest，跳过修改"),
                                            Err(e) => {
                                                error!("修改 Manifest 失败：{}", e);
                                                finish_task(
                                                    &task_id_clone,
                                                    "error",
                                                    Some(format!("patch manifest failed: {}", e)),
                                                );
                                                return;
                                            }
                                        }
                                    }
                                }
                                // 版本检查并可能 patch_path（同你原逻辑）
                                let mut parts_iter = version
                                    .split('.')
                                    .filter_map(|s| {
                                        s.trim().split(|c: char| !c.is_digit(10)).next()
                                    })
                                    .filter_map(|s| s.parse::<u32>().ok());

                                let major = parts_iter.next().unwrap_or(0);
                                let minor = parts_iter.next().unwrap_or(0);
                                let needs_patch = (major < 1) || (major == 1 && minor < 21);

                                if needs_patch {
                                    info!(
                                        "检测到旧版本 (<1.21.x)，准备执行补丁 (patch_path) 于：{}",
                                        extract_to.display()
                                    );
                                    // spawn_blocking 执行同步补丁
                                    let extract_clone = extract_to.clone();
                                    match tokio::task::spawn_blocking(move || {
                                        patch_path(&extract_clone)
                                    })
                                    .await
                                    {
                                        Ok(Ok(patch_result)) => {
                                            match patch_result {
                                                PatchResult::Patched(bak_path) => {
                                                    info!(
                                                        "补丁应用成功，备份创建于：{}",
                                                        bak_path.display()
                                                    );
                                                }
                                                PatchResult::NotApplicable => {
                                                    info!("补丁不适用，未找到目标 exe。");
                                                }
                                            }
                                        }
                                        Ok(Err(patch_err)) => {
                                            error!("补丁失败：{:?}", patch_err);
                                            // 将具体错误映射为用户可读信息
                                            finish_task(
                                                &task_id_clone,
                                                "error",
                                                Some(format!("patch error: {:?}", patch_err)),
                                            );
                                            return;
                                        }
                                        Err(join_err) => {
                                            error!("执行补丁任务失败（join error）：{}", join_err);
                                            finish_task(
                                                &task_id_clone,
                                                "error",
                                                Some(format!("patch join error: {}", join_err)),
                                            );
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            debug!(
                                "获取 Manifest Identity 失败，跳过 Manifest 修改与补丁：{}",
                                e
                            );
                        }
                    }
                }

                // 一切成功
                finish_task(&task_id_clone, "completed", None);
            }

            Ok(CoreResult::Cancelled) => {
                info!("用户取消了解压");
                // 清理解压目录、源文件等
                if let Err(e) = fs::remove_dir_all(&extract_to) {
                    error!(
                        "取消时删除解压目录失败：{}，目录：{}",
                        e,
                        extract_to.display()
                    );
                }
                if let Err(e) = fs::remove_file(&destination_clone) {
                    error!("取消时删除源文件失败：{}，路径：{}", e, destination_clone);
                }
                if !is_cancelled(&task_id_clone) {
                    finish_task(&task_id_clone, "cancelled", Some("user cancelled".into()));
                }
            }

            Ok(CoreResult::Error(e)) => {
                error!("解压内部错误：{}", e);
                let _ = fs::remove_dir_all(&extract_to);
                finish_task(
                    &task_id_clone,
                    "error",
                    Some(format!("extract error: {}", e)),
                );
            }

            Err(core_err) => {
                error!("解压失败（外层错误）：{}", core_err);
                let _ = fs::remove_dir_all(&extract_to);
                finish_task(
                    &task_id_clone,
                    "error",
                    Some(format!("extract failed: {}", core_err)),
                );
            }
        }
    });

    // 立即返回 task_id 给前端
    Ok(task_id)
}
