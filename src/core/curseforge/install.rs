use crate::core::minecraft::assets::{
    CheckImportRequest, ImportAssetsRequest, check_import_conflict, import_assets,
    inspect_import_file,
};
use crate::core::minecraft::paths::{BuildType, Edition};
use crate::tasks::task_manager::{
    create_task_with_details, finish_task, is_cancelled, register_task_abort_handle,
    reset_progress,
};

#[derive(Clone, Debug)]
pub struct CurseForgeInstallTarget {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,
    pub enable_isolation: bool,
    pub user_id: Option<String>,
    pub allow_shared_fallback: bool,
}

#[derive(Clone, Debug)]
pub struct CurseForgeInstallRequest {
    pub project_name: String,
    pub file_name: String,
    pub download_url: String,
    pub target: CurseForgeInstallTarget,
}

#[derive(Clone, Debug)]
pub struct CurseForgeInstallResult {
    pub imported_count: usize,
    pub target_version: String,
}

pub fn start_install(request: CurseForgeInstallRequest) -> Result<String, String> {
    let task_id = create_task_with_details(
        None,
        format!("安装 CurseForge · {}", request.project_name),
        Some(format!("{} → {}", request.file_name, request.target.version_name)),
        "ready",
        None,
        true,
    );
    let worker_task_id = task_id.clone();

    let abort_handle = match crate::tasks::runtime::spawn_download_task(
        task_id.clone(),
        async move {
            let result = run_install(&worker_task_id, request).await;
            if is_cancelled(&worker_task_id) {
                return;
            }
            match result {
                Ok(result) => finish_task(
                    &worker_task_id,
                    "completed",
                    Some(format!(
                        "已安装 {} 个内容包到 {}",
                        result.imported_count, result.target_version
                    )),
                ),
                Err(error) => finish_task(&worker_task_id, "error", Some(error)),
            }
        },
    ) {
        Ok(abort_handle) => abort_handle,
        Err(error) => {
            finish_task(&task_id, "error", Some(error.clone()));
            return Err(error);
        }
    };
    register_task_abort_handle(task_id.clone(), abort_handle);
    Ok(task_id)
}

async fn run_install(
    task_id: &str,
    request: CurseForgeInstallRequest,
) -> Result<CurseForgeInstallResult, String> {
    let downloaded = crate::downloads::api::download_resource_to_cache_in_task(
        task_id,
        request.download_url,
        request.file_name,
        None,
        None,
    )
    .await?;

    ensure_not_cancelled(task_id)?;
    reset_progress(task_id, None, Some("inspecting"));
    let downloaded_path = downloaded.to_string_lossy().to_string();
    let preview = inspect_import_file(downloaded_path.clone(), None).await?;
    if !preview.valid {
        return Err(preview
            .invalid_reason
            .unwrap_or_else(|| "下载内容不是有效的 Minecraft Bedrock 包".to_string()));
    }

    ensure_not_cancelled(task_id)?;
    reset_progress(task_id, None, Some("checking_conflict"));
    let target = request.target;
    let conflict = check_import_conflict(CheckImportRequest {
        build_type: target.build_type.clone(),
        edition: target.edition.clone(),
        version_name: target.version_name.clone(),
        enable_isolation: target.enable_isolation,
        user_id: target.user_id.clone(),
        file_path: downloaded_path.clone(),
        allow_shared_fallback: target.allow_shared_fallback,
    })
    .await?;

    // Reinstall/update of the same manifest UUID is the normal CurseForge path. It should not
    // require a second modal confirmation. Other conflicts (for example a GDK user/shared
    // fallback decision) are not silently redirected.
    let overwrite = conflict.has_conflict
        && conflict.conflict_type.as_deref() == Some("uuid_match");
    if conflict.has_conflict && !overwrite {
        return Err(if conflict.message.trim().is_empty() {
            "目标位置存在需要人工处理的冲突".to_string()
        } else {
            conflict.message
        });
    }

    ensure_not_cancelled(task_id)?;
    reset_progress(task_id, None, Some("installing"));
    let result = import_assets(ImportAssetsRequest {
        build_type: target.build_type,
        edition: target.edition,
        version_name: target.version_name.clone(),
        enable_isolation: target.enable_isolation,
        user_id: target.user_id,
        file_paths: vec![downloaded_path],
        overwrite,
        allow_shared_fallback: target.allow_shared_fallback,
    })
    .await?;

    if result.imported_count == 0 || result.failed_count != 0 {
        return Err(format!(
            "安装未完整完成：成功 {}，失败 {}",
            result.imported_count, result.failed_count
        ));
    }

    Ok(CurseForgeInstallResult {
        imported_count: result.imported_count,
        target_version: target.version_name,
    })
}

fn ensure_not_cancelled(task_id: &str) -> Result<(), String> {
    if is_cancelled(task_id) {
        Err("安装已取消".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_target_keeps_real_version_metadata() {
        let target = CurseForgeInstallTarget {
            build_type: BuildType::Gdk,
            edition: Edition::Preview,
            version_name: "26.40.5".to_string(),
            enable_isolation: false,
            user_id: None,
            allow_shared_fallback: false,
        };

        assert_eq!(target.build_type, BuildType::Gdk);
        assert_eq!(target.edition, Edition::Preview);
        assert!(!target.enable_isolation);
        assert_eq!(target.version_name, "26.40.5");
    }
}
