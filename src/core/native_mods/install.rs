use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{NativeModEntry, resolve_file_url};

#[derive(Clone, Debug)]
pub struct NativeModInstallRequest {
    pub game_directory: PathBuf,
    pub mod_entry: NativeModEntry,
    pub file_name: String,
}

#[derive(Clone, Debug)]
pub struct NativeModImportRequest {
    pub version_folder: String,
    pub paths: Vec<PathBuf>,
}

pub fn start_install(request: NativeModInstallRequest) -> Result<String, String> {
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        "安装原生 Mod",
        Some(format!(
            "{} · {}",
            request.mod_entry.name, request.file_name
        )),
        "installing_native_mod",
        None,
        false,
    );
    crate::tasks::task_manager::register_task_cooperative_cancel(task_id.clone());
    let active_child_task = Arc::new(Mutex::new(None::<String>));
    let cancel_child = Arc::clone(&active_child_task);
    crate::tasks::task_manager::register_task_cancel_hook(task_id.clone(), move || {
        if let Some(child) = cancel_child.lock().ok().and_then(|guard| guard.clone()) {
            crate::tasks::task_manager::cancel_task(&child);
        }
    });

    let task_id_for_workflow = task_id.clone();
    let child_for_workflow = Arc::clone(&active_child_task);
    let workflow = crate::tasks::runtime::spawn_io(async move {
        let result = install(request, &task_id_for_workflow, &child_for_workflow).await;
        match result {
            Ok(path) => crate::tasks::task_manager::finish_task(
                &task_id_for_workflow,
                "completed",
                Some(path.to_string_lossy().into_owned()),
            ),
            Err(error) if crate::tasks::task_manager::is_cancelled(&task_id_for_workflow) => {
                crate::tasks::task_manager::finish_task(
                    &task_id_for_workflow,
                    "cancelled",
                    Some(error),
                )
            }
            Err(error) => {
                crate::tasks::task_manager::finish_task(&task_id_for_workflow, "error", Some(error))
            }
        }
    })
    .map_err(|error| {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        error
    })?;

    let monitor_task_id = task_id.clone();
    let _ = crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
            && !crate::tasks::task_manager::is_cancelled(&monitor_task_id)
        {
            crate::tasks::task_manager::finish_task(
                &monitor_task_id,
                "error",
                Some(format!("原生 Mod 安装任务异常结束: {error}")),
            );
        }
    });
    Ok(task_id)
}

pub fn start_import(request: NativeModImportRequest) -> Result<String, String> {
    if request.paths.is_empty() {
        return Err("没有可导入的 Mod 文件".to_string());
    }

    let count = request.paths.len();
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        "导入 Minecraft Mod",
        Some(format!("{} · {} 个文件", request.version_folder, count)),
        "installing_mod",
        Some(count as u64),
        false,
    );
    crate::tasks::task_manager::register_task_cooperative_cancel(task_id.clone());
    let worker_task_id = task_id.clone();
    let workflow = crate::tasks::runtime::spawn_io(async move {
        let result = import_local_mods(request, &worker_task_id).await;
        match result {
            Ok(()) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "completed",
                Some(format!("已导入 {count} 个 Mod")),
            ),
            Err(error) if crate::tasks::task_manager::is_cancelled(&worker_task_id) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "cancelled",
                    Some(error),
                )
            }
            Err(error) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "error",
                Some(error),
            ),
        }
    })
    .map_err(|error| {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        error
    })?;

    let monitor_task_id = task_id.clone();
    let _ = crate::tasks::runtime::spawn_io(async move {
        if let Err(error) = workflow.await
            && !error.is_cancelled()
            && !crate::tasks::task_manager::is_cancelled(&monitor_task_id)
        {
            crate::tasks::task_manager::finish_task(
                &monitor_task_id,
                "error",
                Some(format!("Mod 导入任务异常结束: {error}")),
            );
        }
    });
    Ok(task_id)
}

async fn install(
    request: NativeModInstallRequest,
    task_id: &str,
    active_child_task: &Arc<Mutex<Option<String>>>,
) -> Result<PathBuf, String> {
    let file = request
        .mod_entry
        .files
        .get(&request.file_name)
        .ok_or_else(|| format!("索引中不存在文件：{}", request.file_name))?;
    let client = crate::http::proxy::get_download_client_for_proxy()
        .map_err(|error| format!("构建下载客户端失败：{error}"))?;
    let url = resolve_file_url(&client, &request.mod_entry.repository, &file.url).await?;
    let cache_name = format!(
        "{}-{}",
        sanitize_component(&request.mod_entry.id),
        sanitize_component(&request.file_name)
    );
    let download_task =
        crate::downloads::api::download_resource(url, cache_name, None, Some(false), None).await?;
    if let Ok(mut child) = active_child_task.lock() {
        *child = Some(download_task.clone());
    }
    let snapshot = crate::tasks::task_manager::wait_for_task_terminal(&download_task).await?;
    if let Ok(mut child) = active_child_task.lock() {
        *child = None;
    }
    if crate::tasks::task_manager::is_cancelled(task_id) {
        return Err("原生 Mod 安装已取消".to_string());
    }
    if snapshot.status.as_ref() != "completed" {
        return Err(format!(
            "原生 Mod 下载失败：{}",
            snapshot.message.as_deref().unwrap_or("没有错误详情")
        ));
    }
    let cached_path = snapshot
        .message
        .as_deref()
        .ok_or_else(|| "原生 Mod 下载完成但没有返回文件路径".to_string())?;
    crate::tasks::task_manager::set_task_message(
        task_id,
        Some(format!("正在安装 {}", request.mod_entry.name)),
    );

    let file_name = Path::new(&request.file_name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("无效的原生 Mod 文件名：{}", request.file_name))?;
    let mod_folder = sanitize_component(&request.mod_entry.id);
    let mods_dir = request.game_directory.join("mods");
    tokio::fs::create_dir_all(&mods_dir)
        .await
        .map_err(|error| format!("创建原生 Mod 根目录失败：{error}"))?;
    let target_dir = mods_dir.join(&mod_folder);
    let task_token = sanitize_component(task_id);
    let staging = mods_dir.join(format!(".{mod_folder}.bmcb-install-{task_token}"));
    let backup = mods_dir.join(format!(".{mod_folder}.bmcb-backup-{task_token}"));
    remove_dir_if_exists(&staging).await?;
    remove_dir_if_exists(&backup).await?;
    tokio::fs::create_dir(&staging)
        .await
        .map_err(|error| format!("创建原生 Mod staging 失败：{error}"))?;

    let target_file = staging.join(file_name);
    if let Err(error) = tokio::fs::copy(cached_path, &target_file).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("复制原生 Mod 文件失败：{error}"));
    }
    let manifest_type = match file.file_type.as_str() {
        "native.dll" => "preload-native",
        "delay.dll" => "hot-inject",
        other => {
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(format!("暂不支持原生 Mod 类型：{other}"));
        }
    };
    let manifest = json!({
        "name": request.mod_entry.name,
        "entry": file_name,
        "type": manifest_type,
        "version": request.mod_entry.id,
        "inject_delay_ms": 0,
    });
    let manifest_text = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("序列化原生 Mod manifest 失败：{error}"))?;
    if let Err(error) = tokio::fs::write(staging.join("manifest.json"), manifest_text).await {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("写入原生 Mod manifest 失败：{error}"));
    }

    if crate::tasks::task_manager::is_cancelled(task_id) {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err("原生 Mod 安装已取消".to_string());
    }

    let had_previous = tokio::fs::try_exists(&target_dir)
        .await
        .map_err(|error| format!("检查原生 Mod 目标失败：{error}"))?;
    if had_previous {
        tokio::fs::rename(&target_dir, &backup)
            .await
            .map_err(|error| format!("备份旧原生 Mod 失败：{error}"))?;
    }

    if crate::tasks::task_manager::is_cancelled(task_id) {
        if had_previous {
            let _ = tokio::fs::rename(&backup, &target_dir).await;
        }
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err("原生 Mod 安装已取消".to_string());
    }

    if let Err(error) = tokio::fs::rename(&staging, &target_dir).await {
        if had_previous {
            if let Err(restore_error) = tokio::fs::rename(&backup, &target_dir).await {
                return Err(format!(
                    "提交原生 Mod 失败：{error}；恢复旧版本也失败：{restore_error}"
                ));
            }
        }
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(format!("提交原生 Mod 安装失败：{error}"));
    }
    if had_previous
        && let Err(error) = tokio::fs::remove_dir_all(&backup).await
    {
        tracing::warn!(path = %backup.display(), %error, "failed to clean old native Mod backup");
    }

    Ok(target_dir.join(file_name))
}

async fn import_local_mods(
    request: NativeModImportRequest,
    task_id: &str,
) -> Result<(), String> {
    let mods_dir = crate::utils::file_ops::bmcbl_subdir("versions")
        .join(&request.version_folder)
        .join("mods");
    let mut names = HashSet::with_capacity(request.paths.len());
    let mut plans = Vec::with_capacity(request.paths.len());

    for source_path in &request.paths {
        ensure_task_active(task_id)?;
        let metadata = tokio::fs::metadata(source_path)
            .await
            .map_err(|error| format!("无法读取 Mod 文件“{}”: {error}", source_path.display()))?;
        if !metadata.is_file() {
            return Err(format!("所选路径不是文件: {}", source_path.display()));
        }
        let file_name = source_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("无效文件名: {}", source_path.display()))?
            .to_string();
        let folder_name = source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("无法识别 Mod 名称: {}", source_path.display()))?
            .to_string();
        if !names.insert(folder_name.to_lowercase()) {
            return Err(format!("选择的文件中包含重复 Mod: {folder_name}"));
        }
        let target_dir = mods_dir.join(&folder_name);
        if tokio::fs::try_exists(&target_dir)
            .await
            .map_err(|error| format!("检查 Mod 目标目录失败: {error}"))?
        {
            return Err(format!(
                "Mod“{folder_name}”已存在，请先在 Mod 列表中删除后再导入，避免覆盖现有配置"
            ));
        }
        plans.push((source_path.clone(), file_name, folder_name, target_dir));
    }

    tokio::fs::create_dir_all(&mods_dir)
        .await
        .map_err(|error| format!("创建 mods 目录失败: {error}"))?;

    let task_token = sanitize_component(task_id);
    let mut staged = Vec::with_capacity(plans.len());
    for (source_path, file_name, folder_name, target_dir) in plans {
        ensure_task_active(task_id)?;
        let staging = mods_dir.join(format!(".{folder_name}.bmcb-import-{task_token}"));
        if tokio::fs::try_exists(&staging).await.unwrap_or(false) {
            let _ = tokio::fs::remove_dir_all(&staging).await;
        }
        tokio::fs::create_dir(&staging)
            .await
            .map_err(|error| format!("创建 Mod staging 失败: {error}"))?;

        let manifest = json!({
            "name": folder_name,
            "entry": file_name.clone(),
            "type": "preload-native",
            "inject_delay_ms": 0
        });
        let manifest_text = serde_json::to_string_pretty(&manifest)
            .map_err(|error| format!("Manifest 序列化失败: {error}"))?;
        let target_file = staging.join(&file_name);

        if let Err(error) = tokio::fs::copy(&source_path, &target_file).await {
            cleanup_staging_dirs(&staged).await;
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(format!("复制 Mod 文件失败: {error}"));
        }
        if let Err(error) = tokio::fs::write(staging.join("manifest.json"), manifest_text).await {
            cleanup_staging_dirs(&staged).await;
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(format!("写入 Manifest 失败: {error}"));
        }
        if let Err(error) = ensure_task_active(task_id) {
            cleanup_staging_dirs(&staged).await;
            let _ = tokio::fs::remove_dir_all(&staging).await;
            return Err(error);
        }
        staged.push((staging, target_dir));
    }

    if let Err(error) = ensure_task_active(task_id) {
        cleanup_staging_dirs(&staged).await;
        return Err(error);
    }

    let mut committed = Vec::with_capacity(staged.len());
    for (index, (staging, target_dir)) in staged.iter().enumerate() {
        if crate::tasks::task_manager::is_cancelled(task_id) {
            rollback_committed_dirs(&committed).await;
            cleanup_staging_dirs(&staged[index..]).await;
            return Err("Mod 导入已取消".to_string());
        }
        if let Err(error) = tokio::fs::rename(staging, target_dir).await {
            rollback_committed_dirs(&committed).await;
            cleanup_staging_dirs(&staged[index..]).await;
            return Err(format!(
                "提交 Mod 导入失败: {} -> {} ({error})",
                staging.display(),
                target_dir.display()
            ));
        }
        committed.push(target_dir.clone());
        crate::tasks::task_manager::update_progress(
            task_id,
            1,
            Some(request.paths.len() as u64),
            Some("installing_mod"),
        );
    }
    Ok(())
}

fn ensure_task_active(task_id: &str) -> Result<(), String> {
    if crate::tasks::task_manager::is_cancelled(task_id) {
        Err("Mod 导入已取消".to_string())
    } else {
        Ok(())
    }
}

async fn remove_dir_if_exists(path: &Path) -> Result<(), String> {
    match tokio::fs::remove_dir_all(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理目录失败: {} ({error})", path.display())),
    }
}

async fn cleanup_staging_dirs(entries: &[(PathBuf, PathBuf)]) {
    for (staging, _) in entries {
        if let Err(error) = tokio::fs::remove_dir_all(staging).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %staging.display(), %error, "failed to clean native Mod staging");
        }
    }
}

async fn rollback_committed_dirs(entries: &[PathBuf]) {
    for path in entries.iter().rev() {
        if let Err(error) = tokio::fs::remove_dir_all(path).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(path = %path.display(), %error, "failed to roll back imported native Mod");
        }
    }
}

fn sanitize_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "native-mod".to_string()
    } else {
        sanitized
    }
}
