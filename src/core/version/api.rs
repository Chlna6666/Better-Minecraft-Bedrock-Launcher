use anyhow::{Context as _, Result};
use std::fs;
use std::path::Path;
use std::time::Instant;
use tracing::{debug, error};

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

pub async fn delete_version(folder_name: &str) -> Result<()> {
    let version_dir = file_ops::bmcbl_subdir("versions").join(folder_name);
    let version_dir_for_log = version_dir.clone();
    let start = Instant::now();

    debug!(
        "开始删除版本目录: folder={}, path={}",
        folder_name,
        version_dir.display()
    );

    let result = crate::tasks::runtime::run_io_blocking(move || {
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
    .map_err(anyhow::Error::msg)?;

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
