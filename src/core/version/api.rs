use anyhow::{Context as _, Result};
use std::fs;
use std::path::Path;
use std::time::Instant;
use tracing::debug;

use crate::core::version::launch_versions::{LaunchVersionEntry, sort_launch_versions};
use crate::core::version::version_manager::get_appx_version_list_blocking;
use crate::utils::file_ops;

pub async fn get_version_list() -> Result<Vec<LaunchVersionEntry>> {
    let path = file_ops::bmcbl_subdir("versions");
    anyhow::ensure!(path.as_os_str().len() > 0, "invalid versions folder path");
    let versions = crate::tasks::runtime::run_cpu(move || get_appx_version_list_blocking(&path))
        .await
        .map_err(anyhow::Error::msg)??;
    crate::tasks::runtime::run_io_blocking(move || {
        let mut versions = versions;
        for version in &mut versions {
            match crate::core::version::game_info::load_game_info(Path::new(version.path.as_ref())) {
                Ok(game_info) => version.game_info = game_info,
                Err(error) => {
                    tracing::warn!(folder = %version.folder, %error, "failed to load game statistics");
                }
            }
        }
        sort_launch_versions(&mut versions);
        Ok::<_, String>(versions)
    })
    .await
    .map_err(anyhow::Error::msg)?
    .map_err(anyhow::Error::msg)
}

enum VersionMutationOutcome {
    Completed(String),
}

fn validate_version_folder_component(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(|ch| matches!(ch, ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err("无效的版本目录名称".to_string());
    }
    Ok(value.to_string())
}

fn version_task_token(task_id: &str) -> String {
    let mut token = String::with_capacity(task_id.len().min(64));
    for ch in task_id.chars().take(64) {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
            token.push(ch);
        } else {
            token.push('_');
        }
    }
    if token.is_empty() {
        "task".to_string()
    } else {
        token
    }
}

fn ensure_version_task_active(task_id: &str) -> Result<(), String> {
    if crate::tasks::task_manager::is_cancelled(task_id) {
        Err("版本操作已取消".to_string())
    } else {
        Ok(())
    }
}

fn start_version_mutation_task<F>(
    title: &'static str,
    detail: String,
    stage: &'static str,
    operation: F,
) -> Result<String, String>
where
    F: FnOnce(&str) -> Result<VersionMutationOutcome, String> + Send + 'static,
{
    let task_id = crate::tasks::task_manager::create_task_with_details(
        None,
        title,
        Some(detail),
        stage,
        None,
        false,
    );
    crate::tasks::task_manager::register_task_cooperative_cancel(task_id.clone());

    let worker_task_id = task_id.clone();
    let blocking_task_id = task_id.clone();
    let workflow = crate::tasks::runtime::spawn_io(async move {
        crate::tasks::task_manager::reset_progress(&worker_task_id, None, Some(stage));
        if crate::tasks::task_manager::is_cancelled(&worker_task_id) {
            crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "cancelled",
                Some("版本操作已取消".to_string()),
            );
            return;
        }

        let result = crate::tasks::runtime::run_io_blocking(move || {
            operation(blocking_task_id.as_str())
        })
        .await;

        match result {
            Ok(Ok(VersionMutationOutcome::Completed(message))) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "completed",
                    Some(message),
                );
            }
            Ok(Err(error))
                if crate::tasks::task_manager::is_cancelled(&worker_task_id)
                    && error.contains("已取消") =>
            {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "cancelled",
                    Some(error),
                );
            }
            Ok(Err(error)) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "error",
                    Some(error),
                );
            }
            Err(error) => {
                crate::tasks::task_manager::finish_task(
                    &worker_task_id,
                    "error",
                    Some(error),
                );
            }
        }
    })
    .map_err(|error| {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        error
    })?;

    let monitor_task_id = task_id.clone();
    if let Err(error) = crate::tasks::runtime::spawn_io(async move {
        match workflow.await {
            Ok(()) => {
                let unfinished = crate::tasks::task_manager::get_snapshot_arc(&monitor_task_id)
                    .is_some_and(|snapshot| !snapshot.is_terminal());
                if unfinished {
                    let status = if crate::tasks::task_manager::is_cancelled(&monitor_task_id) {
                        "cancelled"
                    } else {
                        "error"
                    };
                    crate::tasks::task_manager::finish_task(
                        &monitor_task_id,
                        status,
                        Some("版本任务未正确收尾".to_string()),
                    );
                }
            }
            Err(error) => {
                let status = if crate::tasks::task_manager::is_cancelled(&monitor_task_id) {
                    "cancelled"
                } else {
                    "error"
                };
                crate::tasks::task_manager::finish_task(
                    &monitor_task_id,
                    status,
                    Some(format!("版本任务异常结束: {error}")),
                );
            }
        }
    }) {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }

    Ok(task_id)
}

pub fn start_delete_version_task(folder_name: String) -> Result<String, String> {
    let folder_name = validate_version_folder_component(&folder_name)?;
    let detail = folder_name.clone();

    start_version_mutation_task("删除游戏版本", detail, "deleting_version", move |task_id| {
        let versions_root = file_ops::bmcbl_subdir("versions");
        let version_dir = versions_root.join(&folder_name);
        if !version_dir.is_dir() {
            return Err(format!("版本目录不存在: {}", version_dir.display()));
        }

        ensure_version_task_active(task_id)?;
        let tombstone = versions_root.join(format!(
            ".{}.bmcb-delete-{}",
            folder_name,
            version_task_token(task_id)
        ));
        if tombstone.exists() {
            return Err(format!("删除暂存目录已存在: {}", tombstone.display()));
        }

        let started_at = Instant::now();
        fs::rename(&version_dir, &tombstone).map_err(|error| {
            format!(
                "提交版本删除失败: {} -> {} ({error})",
                version_dir.display(),
                tombstone.display()
            )
        })?;

        // Rename is the delete commit boundary. From this point cancellation cannot leave the
        // canonical version half-present; finish tombstone cleanup before publishing terminal.
        fs::remove_dir_all(&tombstone).with_context(|| {
            format!("清理已删除版本暂存目录失败: {}", tombstone.display())
        }).map_err(|error| error.to_string())?;

        debug!(
            folder = %folder_name,
            elapsed = ?started_at.elapsed(),
            "版本删除事务完成"
        );
        Ok(VersionMutationOutcome::Completed("版本已删除".to_string()))
    })
}

pub fn start_rename_version_task(
    old_name: String,
    new_name: String,
) -> Result<String, String> {
    let old_name = validate_version_folder_component(&old_name)?;
    let new_name = validate_version_folder_component(&new_name)?;
    if old_name == new_name {
        return Err("新旧版本目录名称相同".to_string());
    }
    let detail = format!("{old_name} → {new_name}");

    start_version_mutation_task("重命名游戏版本", detail, "renaming_version", move |task_id| {
        let versions_root = file_ops::bmcbl_subdir("versions");
        let old_dir = versions_root.join(&old_name);
        let new_dir = versions_root.join(&new_name);
        if !old_dir.is_dir() {
            return Err(format!("原版本目录不存在: {}", old_dir.display()));
        }
        if new_dir.exists() {
            return Err(format!("已存在同名的游戏实例: {new_name}"));
        }

        ensure_version_task_active(task_id)?;
        fs::rename(&old_dir, &new_dir).map_err(|error| {
            format!(
                "重命名版本目录失败: {} -> {} ({error})",
                old_dir.display(),
                new_dir.display()
            )
        })?;

        // Directory rename is the atomic commit point. Late cancellation reports the committed
        // result instead of attempting a second rename rollback.
        Ok(VersionMutationOutcome::Completed(format!(
            "版本已重命名为 {new_name}"
        )))
    })
}
