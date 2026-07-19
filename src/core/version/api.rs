use anyhow::{Context as _, Result};
use std::fs;
use std::time::Instant;
use tracing::{debug, error};

use crate::core::version::launch_versions::LaunchVersionEntry;
use crate::core::version::version_manager::get_appx_version_list;
use crate::utils::file_ops;

pub async fn get_version_list() -> Result<Vec<LaunchVersionEntry>> {
    let path = file_ops::bmcbl_subdir("versions");
    anyhow::ensure!(path.as_os_str().len() > 0, "invalid versions folder path");
    get_appx_version_list(&path).await
}

pub async fn delete_version(folder_name: &str) -> Result<()> {
    let version_dir = file_ops::bmcbl_subdir("versions").join(folder_name);
    let version_dir_for_log = version_dir.clone();
    let start = Instant::now();

    debug!(
        "开始删除版本目录: folder={}, path={}",
        folder_name,
        version_dir.display()
    );

    let result = tokio::task::spawn_blocking(move || {
        anyhow::ensure!(
            version_dir.exists(),
            "version dir does not exist: {}",
            version_dir.display()
        );

        fs::remove_dir_all(&version_dir)
            .with_context(|| format!("remove version dir failed: {}", version_dir.display()))?;

        Ok::<(), anyhow::Error>(())
    })
    .await
    .context("wait version delete task failed")?;

    match result {
        Ok(()) => {
            debug!(
                "删除版本目录完成: folder={}, path={}, elapsed={:?}",
                folder_name,
                version_dir_for_log.display(),
                start.elapsed()
            );
        }
        Err(error) => {
            error!(
                "删除版本目录失败: folder={}, path={}, elapsed={:?}, error={:?}",
                folder_name,
                version_dir_for_log.display(),
                start.elapsed(),
                error
            );
            return Err(error);
        }
    }

    Ok(())
}
