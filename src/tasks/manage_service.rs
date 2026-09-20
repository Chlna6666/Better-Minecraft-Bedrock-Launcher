use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::core::minecraft::map::McMapInfo;
use crate::core::minecraft::paths::{GamePathOptions, GameTargetDir, get_game_root};
use crate::core::minecraft::resource_packs::McPackInfo;
use crate::core::minecraft::screenshots::McScreenshotInfo;
use crate::core::minecraft::servers::ExternalServerEntry;
use crate::core::minecraft::skin_packs::McSkinPackInfo;
use crate::core::version::settings::{VersionConfig, get_version_config_blocking};

use super::runtime::{BlockingTaskOptions, run_blocking};

#[derive(Clone, Copy)]
pub enum PackKind {
    Resource,
    Behavior,
}

#[derive(Debug)]
pub struct ManagedModInfo {
    pub folder_name: String,
    pub name: String,
    pub file_path: PathBuf,
    pub folder_path: PathBuf,
    pub enabled: bool,
    pub mod_type: String,
    pub version: Option<String>,
    pub inject_delay_ms: u64,
}

#[derive(Deserialize, Serialize)]
struct ModManifest {
    name: String,
    entry: String,
    #[serde(rename = "type")]
    mod_type: String,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    inject_delay_ms: Option<u64>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug)]
pub struct GdkUserDirectory {
    pub folder_name: String,
    pub has_worlds: bool,
    pub has_screenshots: bool,
    pub has_servers: bool,
}

pub async fn load_version_config(folder_name: String) -> Result<VersionConfig, String> {
    run_blocking(
        BlockingTaskOptions::hidden("读取版本配置"),
        move || get_version_config_blocking(&folder_name),
    )
    .await
}

pub async fn load_gdk_users(options: GamePathOptions) -> Result<Vec<GdkUserDirectory>, String> {
    run_blocking(
        BlockingTaskOptions::hidden("读取 GDK 用户"),
        move || {
            let root =
                get_game_root(&options).ok_or_else(|| "无法解析 Minecraft 根目录".to_string())?;
            let users_dir = root.join("Users");
            if !users_dir.exists() {
                return Ok(Vec::new());
            }

            let entries = fs::read_dir(&users_dir)
                .map_err(|error| format!("读取 GDK 用户目录失败: {error}"))?;
            let mut users = entries
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    entry
                        .file_type()
                        .ok()
                        .filter(std::fs::FileType::is_dir)
                        .map(|_| {
                            let com_mojang = entry.path().join("games").join("com.mojang");
                            GdkUserDirectory {
                                folder_name: entry.file_name().to_string_lossy().into_owned(),
                                has_worlds: directory_contains_file(
                                    &com_mojang.join(GameTargetDir::MinecraftWorlds.name()),
                                ),
                                has_screenshots: directory_contains_file(
                                    &com_mojang.join(GameTargetDir::Screenshots.name()),
                                ),
                                has_servers: fs::metadata(
                                    com_mojang
                                        .join(GameTargetDir::MinecraftPe.name())
                                        .join("external_servers.txt"),
                                )
                                .is_ok_and(|metadata| metadata.len() > 0),
                            }
                        })
                })
                .filter(|user| !user.folder_name.eq_ignore_ascii_case("public"))
                .collect::<Vec<_>>();
            sort_gdk_user_directories(&mut users);
            Ok(users)
        },
    )
    .await
}

fn sort_gdk_user_directories(users: &mut [GdkUserDirectory]) {
    users.sort_by(|left, right| {
        left.folder_name
            .eq_ignore_ascii_case("shared")
            .cmp(&right.folder_name.eq_ignore_ascii_case("shared"))
            .then_with(|| left.folder_name.cmp(&right.folder_name))
    });
}

fn directory_contains_file(root: &Path) -> bool {
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_file() {
                return true;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            }
        }
    }
    false
}

pub async fn load_mods(version_folder: String) -> Result<Vec<ManagedModInfo>, String> {
    run_blocking(BlockingTaskOptions::hidden("读取 Mod"), move || {
        load_mods_blocking(&version_folder)
    })
    .await
}

pub async fn load_packs(
    kind: PackKind,
    locale_code: String,
    options: GamePathOptions,
) -> Result<Vec<McPackInfo>, String> {
    run_blocking(BlockingTaskOptions::hidden("读取资源包"), move || {
        let kind = match kind {
            PackKind::Resource => "resource_packs",
            PackKind::Behavior => "behavior_packs",
        };
        crate::core::minecraft::resource_packs::read_packs_standard(kind, &locale_code, &options)
            .map_err(|error| format!("读取资源包失败: {error:?}"))
    })
    .await
}

pub async fn load_skin_packs(
    locale_code: String,
    options: GamePathOptions,
) -> Result<Vec<McSkinPackInfo>, String> {
    run_blocking(BlockingTaskOptions::hidden("读取皮肤包"), move || {
        crate::core::minecraft::skin_packs::read_skin_packs_standard(&locale_code, &options)
            .map_err(|error| format!("读取皮肤包失败: {error:?}"))
    })
    .await
}

pub async fn load_maps(options: GamePathOptions) -> Result<Vec<McMapInfo>, String> {
    run_blocking(BlockingTaskOptions::hidden("读取地图"), move || {
        crate::core::minecraft::map::list_worlds_standard(&options)
            .map_err(|error| format!("读取地图失败: {error:?}"))
    })
    .await
}

pub async fn load_screenshots(options: GamePathOptions) -> Result<Vec<McScreenshotInfo>, String> {
    run_blocking(BlockingTaskOptions::hidden("读取截图"), move || {
        crate::core::minecraft::screenshots::list_screenshots_standard(&options)
            .map_err(|error| format!("读取截图失败: {error:?}"))
    })
    .await
}

pub async fn load_external_servers(
    options: GamePathOptions,
) -> Result<Vec<ExternalServerEntry>, String> {
    run_blocking(BlockingTaskOptions::hidden("读取服务器"), move || {
        crate::core::minecraft::servers::read_external_servers(&options)
            .map_err(|error| format!("读取服务器失败: {error:?}"))
    })
    .await
}

fn load_mods_blocking(version_folder: &str) -> Result<Vec<ManagedModInfo>, String> {
    let mods_dir = crate::utils::file_ops::bmcbl_subdir("versions")
        .join(version_folder)
        .join("mods");
    if !mods_dir.exists() {
        return Ok(Vec::new());
    }

    let entries =
        fs::read_dir(&mods_dir).map_err(|error| format!("读取 mods 目录失败: {error}"))?;
    let mut mods = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let folder_path = entry.path();
        if !folder_path.is_dir() {
            continue;
        }
        let enabled_path = folder_path.join("manifest.json");
        let disabled_path = folder_path.join(".manifest.json");
        let (enabled, manifest_path) = if enabled_path.exists() {
            (true, enabled_path)
        } else if disabled_path.exists() {
            (false, disabled_path)
        } else {
            continue;
        };
        let manifest = match fs::read_to_string(&manifest_path)
            .map_err(|error| error.to_string())
            .and_then(|content| {
                serde_json::from_str::<ModManifest>(&content).map_err(|error| error.to_string())
            }) {
            Ok(manifest) => manifest,
            Err(error) => {
                warn!(path = %manifest_path.display(), %error, "mod manifest skipped");
                continue;
            }
        };
        mods.push(ManagedModInfo {
            folder_name: entry.file_name().to_string_lossy().into_owned(),
            file_path: folder_path.join(&manifest.entry),
            folder_path,
            enabled,
            name: manifest.name,
            mod_type: manifest.mod_type,
            version: manifest
                .version
                .filter(|version| !version.trim().is_empty()),
            inject_delay_ms: manifest.inject_delay_ms.unwrap_or(0),
        });
    }
    Ok(mods)
}


fn mod_directory(version_folder: &str, mod_id: &str) -> Result<PathBuf, String> {
    if version_folder.trim().is_empty()
        || mod_id.trim().is_empty()
        || mod_id.contains("..")
        || mod_id.contains('/')
        || mod_id.contains('\\')
    {
        return Err("无效的 Mod 标识".to_string());
    }

    let directory = crate::utils::file_ops::bmcbl_subdir("versions")
        .join(version_folder)
        .join("mods")
        .join(mod_id);
    if !directory.is_dir() {
        return Err(format!("Mod 目录不存在: {mod_id}"));
    }
    Ok(directory)
}

fn editable_manifest_path_blocking(version_folder: &str, mod_id: &str) -> Result<PathBuf, String> {
    let mod_dir = mod_directory(version_folder, mod_id)?;
    let enabled = mod_dir.join("manifest.json");
    if enabled.is_file() {
        return Ok(enabled);
    }
    let disabled = mod_dir.join(".manifest.json");
    if disabled.is_file() {
        return Ok(disabled);
    }
    Err("未找到 manifest.json 或 .manifest.json".to_string())
}

fn task_path_token(task_id: &str) -> String {
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

fn sibling_transaction_path(path: &Path, task_id: &str, role: &str) -> Result<PathBuf, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Manifest 没有父目录: {}", path.display()))?;
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("manifest.json");
    Ok(parent.join(format!(
        ".{name}.bmcb-{role}-{}",
        task_path_token(task_id)
    )))
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("清理临时文件失败 {}: {error}", path.display())),
    }
}

fn ensure_mod_task_active(task_id: &str) -> Result<(), String> {
    if crate::tasks::task_manager::is_cancelled(task_id) {
        Err("Mod 操作已取消".to_string())
    } else {
        Ok(())
    }
}

fn write_manifest_transactionally(
    manifest_path: &Path,
    manifest: &ModManifest,
    task_id: &str,
) -> Result<(), String> {
    let staging = sibling_transaction_path(manifest_path, task_id, "staging")?;
    let backup = sibling_transaction_path(manifest_path, task_id, "backup")?;
    remove_file_if_exists(&staging)?;
    remove_file_if_exists(&backup)?;

    let formatted = serde_json::to_vec_pretty(manifest)
        .map_err(|error| format!("Manifest 序列化失败: {error}"))?;
    let mut staging_file = fs::File::create(&staging)
        .map_err(|error| format!("创建 Manifest staging 失败: {error}"))?;
    if let Err(error) = staging_file.write_all(&formatted) {
        let _ = remove_file_if_exists(&staging);
        return Err(format!("写入 Manifest staging 失败: {error}"));
    }
    if let Err(error) = staging_file.sync_all() {
        let _ = remove_file_if_exists(&staging);
        return Err(format!("刷新 Manifest staging 失败: {error}"));
    }
    drop(staging_file);

    if let Err(error) = ensure_mod_task_active(task_id) {
        let _ = remove_file_if_exists(&staging);
        return Err(error);
    }

    fs::rename(manifest_path, &backup).map_err(|error| {
        let _ = remove_file_if_exists(&staging);
        format!("备份旧 Manifest 失败: {error}")
    })?;

    if let Err(error) = ensure_mod_task_active(task_id) {
        let restore_result = fs::rename(&backup, manifest_path);
        let _ = remove_file_if_exists(&staging);
        return match restore_result {
            Ok(()) => Err(error),
            Err(restore_error) => Err(format!(
                "{error}；恢复旧 Manifest 失败: {restore_error}"
            )),
        };
    }

    if let Err(error) = fs::rename(&staging, manifest_path) {
        let restore_result = fs::rename(&backup, manifest_path);
        let _ = remove_file_if_exists(&staging);
        return match restore_result {
            Ok(()) => Err(format!("提交 Manifest 失败: {error}")),
            Err(restore_error) => Err(format!(
                "提交 Manifest 失败: {error}；恢复旧 Manifest 也失败: {restore_error}"
            )),
        };
    }

    // Atomic rename above is the commit boundary. A late cancellation must not roll back the
    // already complete manifest; only best-effort cleanup remains.
    if let Err(error) = remove_file_if_exists(&backup) {
        warn!(
            path = %backup.display(),
            %error,
            "manifest committed but backup cleanup failed"
        );
    }
    Ok(())
}

fn start_mod_mutation_task<F>(
    title: &'static str,
    detail: String,
    stage: &'static str,
    operation: F,
) -> Result<String, String>
where
    F: FnOnce(&str) -> Result<String, String> + Send + 'static,
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
                Some("Mod 操作已取消".to_string()),
            );
            return;
        }

        let result = crate::tasks::runtime::run_io_blocking(move || {
            operation(blocking_task_id.as_str())
        })
        .await;

        match result {
            Ok(Ok(message)) => crate::tasks::task_manager::finish_task(
                &worker_task_id,
                "completed",
                Some(message),
            ),
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
                crate::tasks::task_manager::finish_task(&worker_task_id, "error", Some(error));
            }
            Err(error) => {
                crate::tasks::task_manager::finish_task(&worker_task_id, "error", Some(error));
            }
        }
    })
    .map_err(|error| {
        crate::tasks::task_manager::finish_task(&task_id, "error", Some(error.clone()));
        error
    })?;

    let monitor_task_id = task_id.clone();
    let _ = crate::tasks::runtime::spawn_io(async move {
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
                        Some("Mod 任务未正确收尾".to_string()),
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
                    Some(format!("Mod 任务异常结束: {error}")),
                );
            }
        }
    });

    Ok(task_id)
}

pub fn start_set_mod_enabled(
    version_folder: String,
    mod_id: String,
    enabled: bool,
) -> Result<String, String> {
    let detail = format!("{version_folder} · {mod_id}");
    start_mod_mutation_task(
        if enabled { "启用 Mod" } else { "禁用 Mod" },
        detail,
        "updating_mod_state",
        move |task_id| {
            let mod_dir = mod_directory(&version_folder, &mod_id)?;
            let enabled_path = mod_dir.join("manifest.json");
            let disabled_path = mod_dir.join(".manifest.json");

            if enabled {
                if enabled_path.exists() {
                    return Ok("Mod 已启用".to_string());
                }
                if !disabled_path.exists() {
                    return Err("未找到 .manifest.json，无法启用".to_string());
                }
                ensure_mod_task_active(task_id)?;
                fs::rename(&disabled_path, &enabled_path)
                    .map_err(|error| format!("启用 Mod 失败: {error}"))?;
                return Ok("Mod 已启用".to_string());
            }

            if disabled_path.exists() {
                if enabled_path.exists() {
                    ensure_mod_task_active(task_id)?;
                    let trash = sibling_transaction_path(&enabled_path, task_id, "trash")?;
                    remove_file_if_exists(&trash)?;
                    fs::rename(&enabled_path, &trash)
                        .map_err(|error| format!("提交禁用 Mod 状态失败: {error}"))?;
                    if let Err(error) = remove_file_if_exists(&trash) {
                        warn!(path = %trash.display(), %error, "disabled Mod cleanup failed");
                    }
                }
                return Ok("Mod 已禁用".to_string());
            }

            if !enabled_path.exists() {
                return Err("未找到 manifest.json，无法禁用".to_string());
            }
            ensure_mod_task_active(task_id)?;
            fs::rename(&enabled_path, &disabled_path)
                .map_err(|error| format!("禁用 Mod 失败: {error}"))?;
            Ok("Mod 已禁用".to_string())
        },
    )
}

pub fn start_update_mod_settings(
    version_folder: String,
    mod_id: String,
    mod_type: String,
    inject_delay_ms: Option<u64>,
) -> Result<String, String> {
    let detail = format!("{version_folder} · {mod_id}");
    start_mod_mutation_task("更新 Mod 配置", detail, "updating_mod_manifest", move |task_id| {
        let manifest_path = editable_manifest_path_blocking(&version_folder, &mod_id)?;
        let content = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("读取 Manifest 失败: {error}"))?;
        let mut manifest: ModManifest = serde_json::from_str(&content)
            .map_err(|error| format!("Manifest 解析失败: {error}"))?;
        manifest.mod_type = mod_type.trim().to_string();
        if let Some(inject_delay_ms) = inject_delay_ms {
            manifest.inject_delay_ms = Some(inject_delay_ms);
        }
        write_manifest_transactionally(&manifest_path, &manifest, task_id)?;
        Ok("Mod 配置已更新".to_string())
    })
}

pub fn start_set_mod_inject_delay(
    version_folder: String,
    mod_id: String,
    inject_delay_ms: u64,
) -> Result<String, String> {
    let detail = format!("{version_folder} · {mod_id}");
    start_mod_mutation_task("更新 Mod 注入延迟", detail, "updating_mod_manifest", move |task_id| {
        let manifest_path = editable_manifest_path_blocking(&version_folder, &mod_id)?;
        let content = fs::read_to_string(&manifest_path)
            .map_err(|error| format!("读取 Manifest 失败: {error}"))?;
        let mut manifest: ModManifest = serde_json::from_str(&content)
            .map_err(|error| format!("Manifest 解析失败: {error}"))?;
        manifest.inject_delay_ms = Some(inject_delay_ms);
        write_manifest_transactionally(&manifest_path, &manifest, task_id)?;
        Ok("Mod 注入延迟已更新".to_string())
    })
}


fn rollback_staged_mod_directories(staged: &[(PathBuf, PathBuf)]) -> Result<(), String> {
    let mut errors = Vec::new();
    for (original, tombstone) in staged.iter().rev() {
        if !tombstone.exists() {
            continue;
        }
        if original.exists() {
            errors.push(format!(
                "无法回滚 {}：原路径已重新出现",
                original.display()
            ));
            continue;
        }
        if let Err(error) = fs::rename(tombstone, original) {
            errors.push(format!(
                "恢复 {} 失败: {error}",
                original.display()
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("；"))
    }
}

pub fn start_delete_mods_task(
    version_folder: String,
    mod_ids: Vec<String>,
) -> Result<String, String> {
    if mod_ids.is_empty() {
        return Err("没有选择要删除的 Mod".to_string());
    }
    let detail = format!("{version_folder} · {} 个 Mod", mod_ids.len());

    start_mod_mutation_task("删除 Mod", detail, "deleting_mods", move |task_id| {
        let mut targets = Vec::with_capacity(mod_ids.len());
        for mod_id in &mod_ids {
            let path = mod_directory(&version_folder, mod_id)?;
            if !targets.contains(&path) {
                targets.push(path);
            }
        }

        let mut planned = Vec::with_capacity(targets.len());
        for (index, target) in targets.iter().enumerate() {
            let parent = target
                .parent()
                .ok_or_else(|| format!("Mod 目录没有父目录: {}", target.display()))?;
            let name = target
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("mod");
            let tombstone = parent.join(format!(
                ".{name}.bmcb-delete-{}-{index}",
                task_path_token(task_id)
            ));
            if tombstone.exists() {
                return Err(format!("Mod 删除暂存目录已存在: {}", tombstone.display()));
            }
            planned.push((target.clone(), tombstone));
        }

        let mut staged: Vec<(PathBuf, PathBuf)> = Vec::with_capacity(planned.len());
        for (target, tombstone) in planned {
            if let Err(error) = ensure_mod_task_active(task_id) {
                if let Err(rollback_error) = rollback_staged_mod_directories(&staged) {
                    return Err(format!("{error}；{rollback_error}"));
                }
                return Err(error);
            }

            if let Err(error) = fs::rename(&target, &tombstone) {
                let rollback_error = rollback_staged_mod_directories(&staged).err();
                return Err(match rollback_error {
                    Some(rollback_error) => format!(
                        "暂存 Mod 删除失败 {}: {error}；{rollback_error}",
                        target.display()
                    ),
                    None => format!("暂存 Mod 删除失败 {}: {error}", target.display()),
                });
            }
            staged.push((target, tombstone));
        }

        // All canonical Mod directories have been atomically removed from view. This is the
        // commit boundary; ignore late cancellation and finish tombstone cleanup.
        for (_, tombstone) in &staged {
            fs::remove_dir_all(tombstone).map_err(|error| {
                format!("清理 Mod 删除暂存目录失败 {}: {error}", tombstone.display())
            })?;
        }

        Ok(format!("已删除 {} 个 Mod", staged.len()))
    })
}
