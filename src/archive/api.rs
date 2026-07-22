use crate::archive::runtime::spawn_archive_task;
use crate::archive::zip::extract_zip;
use crate::config::config::read_config;
#[cfg(target_os = "windows")]
use crate::core::minecraft::appx::utils::{get_manifest_identity, patch_manifest};
#[cfg(target_os = "linux")]
use crate::core::minecraft::appx_utils::{get_manifest_identity, patch_manifest};
use crate::core::minecraft::key_patcher::{PatchResult, patch_path};
use crate::result::CoreResult;
use crate::tasks::task_manager::{
    create_task_with_details, finish_task, is_cancelled, update_progress,
};
use crate::utils::file_ops;
use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use tracing::{debug, error, info};

fn task_target_name(name: &str, fallback: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn finish_error(task_id: &str, message: impl Into<String>) {
    let message = message.into();
    error!(task_id, message = %message, "archive task failed");
    finish_task(task_id, "error", Some(message));
}

fn remove_dir_all_if_exists(path: &Path, context: &str) {
    if !path.exists() {
        return;
    }

    if let Err(error) = fs::remove_dir_all(path) {
        error!("{context}: {} ({error})", path.display());
    }
}

fn remove_file_if_exists(path: &Path, context: &str) -> bool {
    if !path.exists() {
        return true;
    }

    match fs::remove_file(path) {
        Ok(()) => true,
        Err(error) => {
            error!("{context}: {} ({error})", path.display());
            false
        }
    }
}

fn task_was_cancelled(task_id: &str) -> bool {
    is_cancelled(task_id)
}

pub async fn import_appx(source_path: String, file_name: Option<String>) -> Result<String, String> {
    crate::archive::register_archive_task_stage_labels();
    debug!(
        "import_appx: source_path='{}', file_name='{:?}'",
        source_path, file_name
    );

    let task_label = file_name
        .as_deref()
        .map(|name| task_target_name(name, "导入安装包"))
        .unwrap_or_else(|| task_target_name(&source_path, "导入安装包"));
    let task_id =
        create_task_with_details(None, "导入安装包", Some(task_label), "queued", None, false);

    if let Err(error) = spawn_archive_task(task_id.clone(), {
        let task_id = task_id.clone();
        async move {
            run_import_appx_task(task_id, source_path, file_name).await;
        }
    }) {
        finish_error(&task_id, error);
    }

    Ok(task_id)
}

async fn run_import_appx_task(task_id: String, source_path: String, file_name: Option<String>) {
    update_progress(&task_id, 0, None, Some("starting"));

    let source = Path::new(&source_path);
    if !source.exists() || !source.is_file() {
        finish_error(&task_id, format!("源文件不存在或不是文件：{source_path}"));
        return;
    }

    let destination_file_name = file_name.unwrap_or_else(|| {
        source
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "import_unknown.appx".to_string())
    });
    let destination_file_name = if destination_file_name.to_lowercase().ends_with(".appx") {
        destination_file_name
    } else {
        format!("{destination_file_name}.appx")
    };

    let versions_root = file_ops::bmcbl_subdir("versions");
    if let Err(error) = fs::create_dir_all(&versions_root) {
        finish_error(
            &task_id,
            format!(
                "创建 versions 目录失败：{} ({})",
                error,
                versions_root.display()
            ),
        );
        return;
    }

    let stem = Path::new(&destination_file_name)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "import_unknown".to_string());
    let extract_to = versions_root.join(stem);
    if let Err(error) = fs::create_dir_all(&extract_to) {
        finish_error(
            &task_id,
            format!("创建解压目标目录失败：{} ({})", error, extract_to.display()),
        );
        return;
    }

    let archive = match open_zip_archive(source, &task_id) {
        Some(archive) => archive,
        None => return,
    };

    let extract_to_str = match extract_to.to_str() {
        Some(value) => value.to_string(),
        None => {
            finish_error(
                &task_id,
                format!("invalid extract path (non-utf8): {}", extract_to.display()),
            );
            return;
        }
    };

    match extract_zip(archive, &extract_to_str, true, task_id.clone()).await {
        Ok(CoreResult::Success(())) => {
            update_progress(&task_id, 0, None, Some("preparing_files"));
            let signature_path = extract_to.join("AppxSignature.p7x");
            remove_file_if_exists(&signature_path, "删除 AppxSignature.p7x 失败");
            if let Err(error) = fs::create_dir_all(extract_to.join("mods")) {
                finish_error(&task_id, format!("创建 mods 目录失败：{error}"));
                return;
            }
            finish_task(
                &task_id,
                "completed",
                Some(format!("已导入到 {}", extract_to.display())),
            );
        }
        Ok(CoreResult::Cancelled) => {
            remove_dir_all_if_exists(&extract_to, "取消导入时删除解压目录失败");
            if !task_was_cancelled(&task_id) {
                finish_task(&task_id, "cancelled", Some("user cancelled".into()));
            }
        }
        Ok(CoreResult::Error(error)) => {
            remove_dir_all_if_exists(&extract_to, "导入失败后删除解压目录失败");
            finish_error(&task_id, format!("extract error: {error}"));
        }
        Err(error) => {
            remove_dir_all_if_exists(&extract_to, "导入失败后删除解压目录失败");
            finish_error(&task_id, format!("extract failed: {error}"));
        }
    }
}

pub async fn extract_zip_appx(
    file_name: String,
    destination: String,
    force_replace: bool,
    delete_signature: bool,
) -> Result<String, String> {
    crate::archive::register_archive_task_stage_labels();
    debug!(
        "extract_zip_appx: file_name='{}', destination='{}', force_replace={}, delete_signature={}",
        file_name, destination, force_replace, delete_signature
    );

    let task_target = task_target_name(&file_name, "游戏版本");
    let task_id =
        create_task_with_details(None, "安装游戏", Some(task_target), "queued", None, false);

    if let Err(error) = spawn_archive_task(task_id.clone(), {
        let task_id = task_id.clone();
        async move {
            run_extract_zip_appx_task(
                task_id,
                file_name,
                destination,
                force_replace,
                delete_signature,
            )
            .await;
        }
    }) {
        finish_error(&task_id, error);
    }

    Ok(task_id)
}

async fn run_extract_zip_appx_task(
    task_id: String,
    file_name: String,
    destination: String,
    force_replace: bool,
    delete_signature: bool,
) {
    update_progress(&task_id, 0, None, Some("starting"));

    let versions_root = file_ops::bmcbl_subdir("versions");
    if let Err(error) = fs::create_dir_all(&versions_root) {
        finish_error(
            &task_id,
            format!(
                "创建 versions 目录失败：{} ({})",
                error,
                versions_root.display()
            ),
        );
        return;
    }

    let preferred_stem = Path::new(&file_name)
        .file_stem()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_default();

    let stem = if preferred_stem.trim().is_empty() {
        Path::new(&destination)
            .file_stem()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| "unknown".to_string())
    } else {
        preferred_stem
    };

    let extract_to: PathBuf = versions_root.join(stem);
    if extract_to.exists() && force_replace {
        update_progress(&task_id, 0, None, Some("preparing_files"));
        remove_dir_all_if_exists(&extract_to, "替换安装目录失败");
        if extract_to.exists() {
            finish_error(
                &task_id,
                format!("无法替换安装目录：{}", extract_to.display()),
            );
            return;
        }
    }

    if let Err(error) = fs::create_dir_all(&extract_to) {
        finish_error(
            &task_id,
            format!("创建解压目标目录失败：{} ({})", error, extract_to.display()),
        );
        return;
    }

    let archive = match open_zip_archive(Path::new(&destination), &task_id) {
        Some(archive) => archive,
        None => return,
    };

    let extract_to_str = match extract_to.to_str() {
        Some(value) => value.to_string(),
        None => {
            finish_error(
                &task_id,
                format!("invalid extract path (non-utf8): {}", extract_to.display()),
            );
            return;
        }
    };

    match extract_zip(archive, &extract_to_str, force_replace, task_id.clone()).await {
        Ok(CoreResult::Success(())) => {
            if task_was_cancelled(&task_id) {
                remove_dir_all_if_exists(&extract_to, "取消安装时删除解压目录失败");
                return;
            }

            if !finish_appx_install(&task_id, &extract_to, delete_signature).await {
                return;
            }

            finish_task(
                &task_id,
                "completed",
                Some(format!("已安装到 {}", extract_to.display())),
            );
        }
        Ok(CoreResult::Cancelled) => {
            remove_dir_all_if_exists(&extract_to, "取消安装时删除解压目录失败");
            if !task_was_cancelled(&task_id) {
                finish_task(&task_id, "cancelled", Some("user cancelled".into()));
            }
        }
        Ok(CoreResult::Error(error)) => {
            remove_dir_all_if_exists(&extract_to, "安装失败后删除解压目录失败");
            finish_error(&task_id, format!("extract error: {error}"));
        }
        Err(error) => {
            remove_dir_all_if_exists(&extract_to, "安装失败后删除解压目录失败");
            finish_error(&task_id, format!("extract failed: {error}"));
        }
    }
}

fn open_zip_archive(path: &Path, task_id: &str) -> Option<zip::ZipArchive<File>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) => {
            finish_error(
                task_id,
                format!("打开安装包失败：{} ({})", error, path.display()),
            );
            return None;
        }
    };

    match zip::ZipArchive::new(file) {
        Ok(archive) => Some(archive),
        Err(error) => {
            finish_error(
                task_id,
                format!("创建 ZipArchive 失败：{} ({})", error, path.display()),
            );
            None
        }
    }
}

async fn finish_appx_install(task_id: &str, extract_to: &Path, delete_signature: bool) -> bool {
    update_progress(task_id, 0, None, Some("preparing_files"));

    if delete_signature {
        let signature_path = extract_to.join("AppxSignature.p7x");
        if !remove_file_if_exists(&signature_path, "删除 AppxSignature.p7x 失败") {
            finish_error(task_id, "删除 AppxSignature.p7x 失败");
            return false;
        }
    }

    if let Err(error) = fs::create_dir_all(extract_to.join("mods")) {
        finish_error(task_id, format!("创建 mods 目录失败：{error}"));
        return false;
    }

    if task_was_cancelled(task_id) {
        return false;
    }

    if let Ok(config) = read_config() {
        if config.game.modify_appx_manifest {
            update_progress(task_id, 0, None, Some("patching"));
            match patch_manifest(extract_to) {
                Ok(true) => debug!("Manifest 修改成功: {}", extract_to.display()),
                Ok(false) => debug!("未找到 Manifest，跳过修改: {}", extract_to.display()),
                Err(error) => {
                    finish_error(task_id, format!("patch manifest failed: {error}"));
                    return false;
                }
            }
        }
    }

    let Ok((_identity_name, version)) =
        get_manifest_identity(extract_to.to_string_lossy().as_ref()).await
    else {
        debug!(
            "获取 Manifest Identity 失败，跳过旧版本补丁: {}",
            extract_to.display()
        );
        return true;
    };

    let mut version_parts = version
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0));
    let major = version_parts.next().unwrap_or(0);
    let minor = version_parts.next().unwrap_or(0);
    let needs_patch = (major < 1) || (major == 1 && minor < 21);
    if !needs_patch || task_was_cancelled(task_id) {
        return true;
    }

    update_progress(task_id, 0, None, Some("patching"));
    let extract_clone = extract_to.to_path_buf();
    match tokio::task::spawn_blocking(move || patch_path(&extract_clone)).await {
        Ok(Ok(PatchResult::Patched(backup_path))) => {
            info!("旧版补丁应用成功，备份文件：{}", backup_path.display());
            true
        }
        Ok(Ok(PatchResult::NotApplicable)) => {
            debug!("旧版补丁不适用: {}", extract_to.display());
            true
        }
        Ok(Err(error)) => {
            finish_error(task_id, format!("patch error: {error:?}"));
            false
        }
        Err(error) => {
            finish_error(task_id, format!("patch join error: {error}"));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::extract_zip_appx;
    use crate::tasks::task_manager::{TaskSnapshot, get_snapshot_arc, remove_task};
    use crate::utils::file_ops;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tokio::time::sleep;

    const CURL_DOWNLOADED_APPX_PATH: &str = "C:\\Users\\Administrator\\Desktop\\BMCBL\\target\\debug\\BMCBL\\downloads\\curl-1.21.100.appx";
    const CURL_DOWNLOADED_APPX_NAME: &str = "curl-1.21.100.appx";
    async fn wait_task_finished_for_test(task_id: &str, timeout: Duration) -> Arc<TaskSnapshot> {
        let started_at = Instant::now();
        let mut last_log_at = Instant::now() - Duration::from_secs(60);

        loop {
            assert!(
                started_at.elapsed() < timeout,
                "archive task timed out after {:?}: {task_id}",
                timeout
            );

            if let Some(snapshot) = get_snapshot_arc(task_id) {
                if last_log_at.elapsed() >= Duration::from_secs(10) {
                    println!(
                        "task={} status={} stage={} done={} total={:?} percent={:?} speed={:.2}B/s message={}",
                        snapshot.id,
                        snapshot.status,
                        snapshot.stage,
                        snapshot.done,
                        snapshot.total,
                        snapshot.percent,
                        snapshot.speed_bytes_per_sec,
                        snapshot.message.as_deref().unwrap_or("")
                    );
                    last_log_at = Instant::now();
                }

                if !matches!(
                    snapshot.status.as_ref(),
                    "running" | "paused" | "cancelling"
                ) {
                    return snapshot;
                }
            }

            sleep(Duration::from_millis(250)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "extracts a locally downloaded Minecraft UWP appx using the archive pipeline"]
    async fn extract_locally_downloaded_valid_uwp_appx() {
        crate::utils::file_ops::create_initial_directories();

        assert!(
            Path::new(CURL_DOWNLOADED_APPX_PATH).is_file(),
            "missing local appx for extraction test: {}",
            CURL_DOWNLOADED_APPX_PATH
        );

        let extracted_dir = file_ops::bmcbl_subdir("versions").join("curl-1.21.100");
        if extracted_dir.exists() {
            std::fs::remove_dir_all(&extracted_dir)
                .expect("failed to remove previous extracted version dir");
        }

        let task_id = extract_zip_appx(
            CURL_DOWNLOADED_APPX_NAME.to_string(),
            CURL_DOWNLOADED_APPX_PATH.to_string(),
            true,
            true,
        )
        .await
        .expect("archive task should be created");

        println!("created archive task: {task_id}");
        let snapshot = wait_task_finished_for_test(&task_id, Duration::from_secs(30 * 60)).await;
        let status = snapshot.status.as_ref();
        let message = snapshot.message.as_deref().unwrap_or("");
        assert_eq!(status, "completed", "archive task failed: {message}");
        assert!(
            extracted_dir.is_dir(),
            "archive task completed but extracted dir is missing: {}",
            extracted_dir.display()
        );

        remove_task(&task_id);
    }
}
