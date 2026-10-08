use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use futures_util::stream::{self, StreamExt as _, TryStreamExt as _};

use crate::core::minecraft::paths::{
    BuildType, Edition, GamePathOptions, GameTargetDir, get_game_root,
};
use crate::tasks::runtime::{BlockingTaskOptions, run_blocking};

const USER_SCAN_CONCURRENCY: usize = 4;

mod cache;

/// Read-only metadata snapshot of a GDK `Users/<user>` directory.
#[derive(Clone, Debug)]
pub struct GdkUserDirectory {
    /// Directory name used to resolve this user's game data.
    pub folder_name: String,
    /// Whether `games/com.mojang/minecraftWorlds` contains a regular file.
    pub has_worlds: bool,
    /// Whether `games/com.mojang/Screenshots` contains a regular file.
    pub has_screenshots: bool,
    /// Whether `minecraftpe/external_servers.txt` is nonempty.
    pub has_servers: bool,
    /// Latest regular-file modification time anywhere beneath the user directory.
    /// Empty directories or files without readable timestamps yield `None`.
    pub last_modified: Option<SystemTime>,
}

/// Reads GDK users using bounded concurrent metadata scans on AppRuntime.
///
/// This blocking-I/O scheduling adapter reads no file contents and does not modify
/// game data. `Public` is excluded before scanning; symbolic links are not followed.
/// Unreadable entries are skipped, so timestamps describe the readable snapshot,
/// not an atomic view of a concurrently running Minecraft process. Results retain
/// name order with `Shared` last, regardless of scan completion order.
/// Results are cached in memory by resolved `Users` path. File change notifications
/// invalidate reuse and pause monitoring after the first relevant update. The next
/// actual read rearms monitoring before scanning. If monitoring is unavailable,
/// each request scans again. Events never trigger scans or GPUI updates by themselves.
/// Concurrent requests for one path share a scan. No file-content hash is computed.
///
/// # Errors
/// Returns an error if the game root cannot be resolved, `Users` cannot be listed
/// (except when absent), or an AppRuntime blocking task fails or times out.
pub async fn load_gdk_users(options: GamePathOptions) -> Result<Vec<GdkUserDirectory>, String> {
    let root = run_blocking(
        BlockingTaskOptions::hidden("解析 GDK 用户目录"),
        move || {
            let root =
                get_game_root(&options).ok_or_else(|| "无法解析 Minecraft 根目录".to_string())?;
            Ok(root.join("Users"))
        },
    )
    .await?;
    cache::load(root).await
}

/// Invalidates in-memory GDK user snapshots before an explicit Manage refresh.
///
/// Marks existing entries dirty without filesystem I/O or changing game data.
/// A scan already in progress cannot publish a reusable snapshot for this generation.
///
/// # Errors
/// Returns an error if the cache registry lock was poisoned.
pub fn invalidate_gdk_users() -> Result<(), String> {
    cache::invalidate()
}

/// Schedules a delayed scan of the startup-selected GDK version's user directory.
///
/// AppRuntime owns this read-only warmup independently of a page. After the startup
/// settle delay it reads the version's current redirection config and populates the
/// same path cache used by Manage. Cache requests coalesce with any foreground scan.
/// Failures are logged and remain retryable by a normal Manage request.
///
/// # Errors
/// Returns an error if AppRuntime cannot schedule the warmup.
pub fn prewarm_gdk_users(folder_name: String, edition: Edition) -> Result<(), String> {
    crate::tasks::runtime::spawn_io(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let result = async {
            let config = super::load_version_config(folder_name.clone()).await?;
            load_gdk_users(GamePathOptions {
                build_type: BuildType::Gdk,
                edition,
                version_name: folder_name,
                enable_isolation: config.enable_redirection,
                user_id: None,
                allow_shared_fallback: true,
            })
            .await
        }
        .await;
        if let Err(error) = result {
            tracing::debug!(%error, "startup GDK user cache warmup failed");
        }
    })
    .map(drop)
}

async fn scan_user_directories(
    directories: Vec<(String, PathBuf)>,
) -> Result<Vec<GdkUserDirectory>, String> {
    let mut users = stream::iter(directories)
        .map(|(folder_name, path)| async move {
            run_blocking(
                BlockingTaskOptions::hidden("读取 GDK 用户"),
                move || Ok(scan_user_directory(folder_name, &path)),
            )
            .await
        })
        .buffer_unordered(USER_SCAN_CONCURRENCY)
        .try_collect::<Vec<_>>()
        .await?;
    sort_gdk_user_directories(&mut users);
    Ok(users)
}

fn user_directories(root: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("读取 GDK 用户目录失败: {error}")),
    };
    Ok(entries
        .filter_map(Result::ok)
        .filter(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("public")
        })
        .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect())
}

fn scan_user_directory(folder_name: String, root: &Path) -> GdkUserDirectory {
    let com_mojang = root.join("games").join("com.mojang");
    let worlds = com_mojang.join(GameTargetDir::MinecraftWorlds.name());
    let screenshots = com_mojang.join(GameTargetDir::Screenshots.name());
    let servers = com_mojang
        .join(GameTargetDir::MinecraftPe.name())
        .join("external_servers.txt");
    let mut user = GdkUserDirectory {
        folder_name,
        has_worlds: false,
        has_screenshots: false,
        has_servers: false,
        last_modified: None,
    };
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file() {
                user.has_worlds |= path.starts_with(&worlds);
                user.has_screenshots |= path.starts_with(&screenshots);
                if let Ok(metadata) = entry.metadata() {
                    user.has_servers |= path == servers && metadata.len() > 0;
                    user.last_modified = user.last_modified.max(metadata.modified().ok());
                }
            }
        }
    }
    user
}

fn sort_gdk_user_directories(users: &mut [GdkUserDirectory]) {
    users.sort_by(|left, right| {
        left.folder_name
            .eq_ignore_ascii_case("shared")
            .cmp(&right.folder_name.eq_ignore_ascii_case("shared"))
            .then_with(|| left.folder_name.cmp(&right.folder_name))
    });
}

#[cfg(test)]
mod tests;
