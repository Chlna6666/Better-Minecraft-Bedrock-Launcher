use anyhow::Context as _;
use gpui::SharedString;
use serde::{Deserialize, Serialize};
use std::fs::{self as std_fs, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::warn;
use zip::write::SimpleFileOptions;

use crate::core::minecraft::assets::{
    CheckImportRequest, DeleteAssetPayload, ImportAssetsRequest, ImportAssetsResult,
    check_import_conflict, delete_game_asset, import_assets, inspect_import_file,
};
use crate::core::minecraft::import::{ImportCheckResult, PackagePreview};
use crate::core::minecraft::map::McMapInfo;
pub use crate::core::minecraft::nbt::LevelDatDocument;
use crate::core::minecraft::nbt::{
    read_level_dat_document as read_level_dat_file_document,
    write_level_dat_document as write_level_dat_file_document,
};
use crate::core::minecraft::paths::{GamePathOptions, get_game_root};
use crate::core::minecraft::resource_packs::{Header, McPackInfo};
use crate::core::minecraft::skin_packs::McSkinPackInfo;
use crate::core::version::settings::{
    VANILLA_SKIN_PACK_REDIRECTION_SOURCE, VersionConfig, get_version_config, save_version_config,
};
use crate::ui::views::manage::state::{
    ManageAssetEntry, ManageAssetKind, ManageGdkUser, ManagePackSubtype, ManageScreenshotEntry,
    ManageServerEntry, ManageServerMotd, ManageServerMotdStatus, ManageServerMotdTarget,
    ManageSkinPreviewEntry, ManageTab, ManageVersionConfig, ManagedVersionEntry,
};
use futures_util::stream::{self, StreamExt as _};
use std::time::{Duration, Instant};

const SERVER_MOTD_QUERY_TIMEOUT: Duration = Duration::from_millis(1500);
const SERVER_MOTD_RETRY_COUNT: usize = 2;
const SERVER_MOTD_RETRY_DELAY: Duration = Duration::from_millis(120);

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ModManifest {
    name: String,
    entry: String,
    #[serde(rename = "type")]
    mod_type: String,
    #[serde(default)]
    inject_delay_ms: Option<u64>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

pub async fn load_version_config(
    version: &ManagedVersionEntry,
) -> Result<ManageVersionConfig, String> {
    let config = get_version_config(version.folder.to_string()).await?;
    Ok(manage_version_config_from_core(config))
}

pub async fn save_manage_version_config(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
) -> Result<(), String> {
    let mut core_config = get_version_config(version.folder.to_string())
        .await
        .unwrap_or_default();
    core_config.enable_debug_console = config.enable_debug_console;
    core_config.enable_redirection = config.enable_redirection;
    core_config.editor_mode = config.editor_mode;
    core_config.disable_mod_loading = config.disable_mod_loading;
    core_config.lock_mouse_on_launch = config.lock_mouse_on_launch;
    core_config.unlock_mouse_hotkey = config.unlock_mouse_hotkey.to_string();
    core_config.reduce_pixels = config.reduce_pixels;
    core_config.set_vanilla_skin_pack_redirect(
        config
            .vanilla_skin_pack_redirect
            .as_ref()
            .map(|path| path.to_string()),
    );

    save_version_config(version.folder.to_string(), core_config).await
}

pub fn load_gdk_users(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
) -> Result<Vec<ManageGdkUser>, String> {
    let options = GamePathOptions {
        build_type: version.build_type(),
        edition: version.edition(),
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: None,
        allow_shared_fallback: true,
    };
    let root = get_game_root(&options).ok_or_else(|| "无法解析 Minecraft 根目录".to_string())?;
    let users_dir = root.join("Users");
    if !users_dir.exists() {
        return Ok(Vec::new());
    }

    let mut users = Vec::new();
    let entries =
        std_fs::read_dir(&users_dir).map_err(|error| format!("读取 GDK 用户目录失败: {error}"))?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let folder_name = entry.file_name().to_string_lossy().to_string();
        if folder_name.eq_ignore_ascii_case("public") {
            continue;
        }
        users.push(ManageGdkUser {
            folder_name: SharedString::from(folder_name),
        });
    }
    users.sort_by(|left, right| {
        let left_shared = left.folder_name.as_ref().eq_ignore_ascii_case("shared");
        let right_shared = right.folder_name.as_ref().eq_ignore_ascii_case("shared");
        left_shared
            .cmp(&right_shared)
            .then_with(|| left.folder_name.as_ref().cmp(right.folder_name.as_ref()))
    });
    Ok(users)
}

pub async fn load_assets(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    tab: ManageTab,
    pack_subtype: ManagePackSubtype,
    selected_gdk_user: Option<&str>,
    locale_code: &str,
) -> Result<Vec<ManageAssetEntry>, String> {
    match tab {
        ManageTab::Mod => load_mod_assets(version).await,
        ManageTab::ResourcePack => {
            load_pack_assets(
                version,
                config,
                pack_subtype,
                selected_gdk_user,
                locale_code,
            )
            .await
        }
        ManageTab::SkinPack => load_skin_pack_assets(version, config, locale_code).await,
        ManageTab::Map => load_map_assets(version, config, selected_gdk_user).await,
        ManageTab::Screenshot | ManageTab::Server => Ok(Vec::new()),
    }
}

pub async fn delete_assets(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    tab: ManageTab,
    pack_subtype: ManagePackSubtype,
    selected_gdk_user: Option<&str>,
    folder_names: &[String],
) -> Result<(), String> {
    match tab {
        ManageTab::Mod => delete_mods(version.folder.as_ref(), folder_names).await,
        ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map => {
            let build_type = version.build_type();
            let edition = version.edition();
            let delete_type = match tab {
                ManageTab::Map => "maps",
                ManageTab::SkinPack => "skins",
                ManageTab::ResourcePack => match pack_subtype {
                    ManagePackSubtype::Resource => "resourcePacks",
                    ManagePackSubtype::Behavior => "behaviorPacks",
                },
                ManageTab::Mod => unreachable!(),
                ManageTab::Screenshot | ManageTab::Server => unreachable!(),
            };

            for folder_name in folder_names {
                let payload = DeleteAssetPayload {
                    build_type: build_type.clone(),
                    edition: edition.clone(),
                    version_name: version.folder.to_string(),
                    enable_isolation: config.enable_redirection,
                    user_id: selected_gdk_user.map(ToString::to_string),
                    delete_type: delete_type.to_string(),
                    name: folder_name.clone(),
                };
                delete_game_asset(payload)?;
            }
            Ok(())
        }
        ManageTab::Screenshot | ManageTab::Server => Ok(()),
    }
}

pub async fn load_screenshots(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    selected_gdk_user: Option<&str>,
) -> Result<Vec<ManageScreenshotEntry>, String> {
    let options = GamePathOptions {
        build_type: version.build_type(),
        edition: version.edition(),
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: selected_gdk_user.map(ToString::to_string),
        allow_shared_fallback: false,
    };
    let entries = tokio::task::spawn_blocking(move || {
        crate::core::minecraft::screenshots::list_screenshots_standard(&options)
    })
    .await
    .map_err(|error| format!("读取截图任务失败: {error:?}"))?
    .map_err(|error| format!("读取截图失败: {error:?}"))?;

    Ok(entries
        .into_iter()
        .map(manage_screenshot_from_core)
        .collect())
}

pub async fn delete_screenshot(entry: &ManageScreenshotEntry) -> Result<(), String> {
    let image_path = entry.image_path.to_string();
    let folder_path = entry.folder_path.to_string();
    let file_name = entry.file_name.to_string();
    tokio::task::spawn_blocking(move || {
        let path = PathBuf::from(&image_path);
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let json_path = PathBuf::from(&folder_path).join(format!("{stem}.json"));
        let mc_path = PathBuf::from(&folder_path).join(format!("{stem}.mc"));
        crate::core::minecraft::screenshots::delete_screenshot(
            &crate::core::minecraft::screenshots::McScreenshotInfo {
                key: format!("screenshot:{image_path}"),
                image_path,
                folder_path,
                file_name,
                capture_time: None,
                modified: None,
                size_bytes: None,
                json_path: json_path
                    .exists()
                    .then(|| json_path.to_string_lossy().to_string()),
                mc_path: mc_path
                    .exists()
                    .then(|| mc_path.to_string_lossy().to_string()),
                source_root: None,
                gdk_user: None,
            },
        )
    })
    .await
    .map_err(|error| format!("删除截图任务失败: {error:?}"))?
    .map_err(|error| format!("删除截图失败: {error:?}"))
}

pub async fn load_external_servers(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    selected_gdk_user: Option<&str>,
) -> Result<Vec<ManageServerEntry>, String> {
    let options = external_server_options(version, config, selected_gdk_user);
    let entries = tokio::task::spawn_blocking(move || {
        crate::core::minecraft::servers::read_external_servers(&options)
    })
    .await
    .map_err(|error| format!("读取服务器任务失败: {error:?}"))?
    .map_err(|error| format!("读取服务器失败: {error:?}"))?;

    Ok(entries.into_iter().map(manage_server_from_core).collect())
}

pub async fn add_external_server(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    selected_gdk_user: Option<&str>,
    name: &str,
    address: &str,
    port: u16,
) -> Result<ManageServerEntry, String> {
    let options = external_server_options(version, config, selected_gdk_user);
    let name = name.to_string();
    let address = address.to_string();
    tokio::task::spawn_blocking(move || {
        crate::core::minecraft::servers::add_external_server(&options, &name, &address, port)
    })
    .await
    .map_err(|error| format!("添加服务器任务失败: {error:?}"))?
    .map(manage_server_from_core)
    .map_err(|error| format!("添加服务器失败: {error:?}"))
}

pub async fn update_external_server(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    selected_gdk_user: Option<&str>,
    key: &str,
    name: &str,
    address: &str,
    port: u16,
) -> Result<ManageServerEntry, String> {
    let options = external_server_options(version, config, selected_gdk_user);
    let key = key.to_string();
    let name = name.to_string();
    let address = address.to_string();
    tokio::task::spawn_blocking(move || {
        crate::core::minecraft::servers::update_external_server(
            &options, &key, &name, &address, port,
        )
    })
    .await
    .map_err(|error| format!("编辑服务器任务失败: {error:?}"))?
    .map(manage_server_from_core)
    .map_err(|error| format!("编辑服务器失败: {error:?}"))
}

pub async fn delete_external_server(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    selected_gdk_user: Option<&str>,
    key: &str,
) -> Result<(), String> {
    let options = external_server_options(version, config, selected_gdk_user);
    let key = key.to_string();
    tokio::task::spawn_blocking(move || {
        crate::core::minecraft::servers::delete_external_server(&options, &key)
    })
    .await
    .map_err(|error| format!("删除服务器任务失败: {error:?}"))?
    .map_err(|error| format!("删除服务器失败: {error:?}"))
}

pub async fn query_server_motd_batch(
    servers: Vec<ManageServerMotdTarget>,
) -> Vec<(SharedString, ManageServerMotdStatus)> {
    stream::iter(servers)
        .map(|server| async move {
            let key = server.key.clone();
            let status = query_server_motd_with_retries(&server).await;
            (key, status)
        })
        .buffer_unordered(8)
        .collect()
        .await
}

async fn query_server_motd_with_retries(server: &ManageServerMotdTarget) -> ManageServerMotdStatus {
    let mut last_error = String::new();

    for attempt in 0..=SERVER_MOTD_RETRY_COUNT {
        let started_at = Instant::now();
        match mc_motd::query_bedrock(
            server.address.as_ref(),
            server.port,
            SERVER_MOTD_QUERY_TIMEOUT,
        )
        .await
        {
            Ok(motd) => {
                return ManageServerMotdStatus::Online(ManageServerMotd {
                    line_1: SharedString::from(motd.motd_line_1),
                    line_2: (!motd.motd_line_2.trim().is_empty())
                        .then(|| SharedString::from(motd.motd_line_2)),
                    version: (!motd.version.trim().is_empty())
                        .then(|| SharedString::from(motd.version)),
                    players_online: motd.players_online,
                    players_max: motd.players_max,
                    latency_ms: Some(started_at.elapsed().as_millis()),
                });
            }
            Err(error) => {
                last_error = error.to_string();
            }
        }

        if attempt < SERVER_MOTD_RETRY_COUNT {
            tokio::time::sleep(SERVER_MOTD_RETRY_DELAY).await;
        }
    }

    if last_error.is_empty() {
        last_error = "服务器未响应".to_string();
    }
    ManageServerMotdStatus::Offline(SharedString::from(last_error))
}

pub async fn set_mod_enabled(
    version_folder: &str,
    mod_id: &str,
    enabled: bool,
) -> Result<(), String> {
    let mods_dir = version_mods_dir(version_folder);
    let mod_dir = mods_dir.join(mod_id);
    if !mod_dir.exists() {
        return Err(format!("Mod 目录不存在: {mod_id}"));
    }

    let enabled_path = mod_dir.join("manifest.json");
    let disabled_path = mod_dir.join(".manifest.json");

    if enabled {
        if enabled_path.exists() {
            return Ok(());
        }
        if disabled_path.exists() {
            fs::rename(disabled_path, enabled_path)
                .await
                .map_err(|error| format!("启用 Mod 失败: {error}"))?;
            return Ok(());
        }
        return Err("未找到 .manifest.json，无法启用".to_string());
    }

    if disabled_path.exists() {
        if enabled_path.exists() {
            fs::remove_file(enabled_path)
                .await
                .map_err(|error| format!("清理冲突 manifest 失败: {error}"))?;
        }
        return Ok(());
    }

    if enabled_path.exists() {
        fs::rename(enabled_path, disabled_path)
            .await
            .map_err(|error| format!("禁用 Mod 失败: {error}"))?;
        return Ok(());
    }

    Err("未找到 manifest.json，无法禁用".to_string())
}

pub async fn set_mod_type(
    version_folder: &str,
    mod_id: &str,
    mod_type: &str,
) -> Result<(), String> {
    let manifest_path = editable_manifest_path(version_folder, mod_id).await?;
    let content = fs::read_to_string(&manifest_path)
        .await
        .map_err(|error| format!("读取 Manifest 失败: {error}"))?;
    let mut manifest: ModManifest =
        serde_json::from_str(&content).map_err(|error| format!("Manifest 解析失败: {error}"))?;
    manifest.mod_type = mod_type.trim().to_string();
    let formatted = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("Manifest 序列化失败: {error}"))?;
    let mut file = fs::File::create(manifest_path)
        .await
        .map_err(|error| format!("写入 Manifest 失败: {error}"))?;
    file.write_all(formatted.as_bytes())
        .await
        .map_err(|error| format!("写入 Manifest 失败: {error}"))?;
    Ok(())
}

pub async fn set_mod_inject_delay(
    version_folder: &str,
    mod_id: &str,
    inject_delay_ms: u64,
) -> Result<(), String> {
    let manifest_path = editable_manifest_path(version_folder, mod_id).await?;
    let content = fs::read_to_string(&manifest_path)
        .await
        .map_err(|error| format!("读取 Manifest 失败: {error}"))?;
    let mut manifest: ModManifest =
        serde_json::from_str(&content).map_err(|error| format!("Manifest 解析失败: {error}"))?;
    manifest.inject_delay_ms = Some(inject_delay_ms);
    let formatted = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("Manifest 序列化失败: {error}"))?;
    let mut file = fs::File::create(manifest_path)
        .await
        .map_err(|error| format!("写入 Manifest 失败: {error}"))?;
    file.write_all(formatted.as_bytes())
        .await
        .map_err(|error| format!("写入 Manifest 失败: {error}"))?;
    Ok(())
}

pub async fn set_vanilla_skin_pack_redirect(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    asset: Option<&ManageAssetEntry>,
) -> Result<ManageVersionConfig, String> {
    let mut next_config = config.clone();

    if let Some(asset) = asset {
        validate_vanilla_skin_pack_source(version)?;
        validate_skin_pack_redirect_target(asset)?;
        next_config.vanilla_skin_pack_redirect = Some(asset.open_path.clone());
    } else {
        next_config.vanilla_skin_pack_redirect = None;
    }

    save_manage_version_config(version, &next_config).await?;
    Ok(next_config)
}

fn validate_vanilla_skin_pack_source(version: &ManagedVersionEntry) -> Result<(), String> {
    let source_path =
        PathBuf::from(version.path.as_ref()).join(VANILLA_SKIN_PACK_REDIRECTION_SOURCE);
    if source_path.is_dir() {
        Ok(())
    } else {
        Err(format!(
            "当前游戏缺少默认皮肤目录: {}",
            source_path.display()
        ))
    }
}

fn validate_skin_pack_redirect_target(asset: &ManageAssetEntry) -> Result<(), String> {
    if asset.kind != ManageAssetKind::SkinPack {
        return Err("请选择一个皮肤包".to_string());
    }

    let target_path = PathBuf::from(asset.open_path.as_ref());
    if !target_path.is_dir() {
        return Err(format!("皮肤包目录不存在: {}", target_path.display()));
    }
    if !target_path.join("skins.json").is_file() && !target_path.join("manifest.json").is_file() {
        return Err("所选目录不是有效皮肤包".to_string());
    }

    Ok(())
}

pub async fn import_mod_files(version_folder: &str, paths: &[String]) -> Result<(), String> {
    let mods_dir = version_mods_dir(version_folder);
    fs::create_dir_all(&mods_dir)
        .await
        .map_err(|error| format!("创建 mods 目录失败: {error}"))?;

    for path in paths {
        let source_path = PathBuf::from(path);
        if !source_path.exists() {
            return Err(format!("文件不存在: {path}"));
        }

        let file_name = source_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| format!("无效文件名: {path}"))?;
        let folder_name = source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("UnknownMod");
        let target_dir = mods_dir.join(folder_name);
        fs::create_dir_all(&target_dir)
            .await
            .map_err(|error| format!("创建目标目录失败: {error}"))?;

        let target_file = target_dir.join(file_name);
        fs::copy(&source_path, &target_file)
            .await
            .map_err(|error| format!("复制 Mod 文件失败: {error}"))?;

        if target_dir.join(".manifest.json").exists() {
            continue;
        }

        let manifest = ModManifest {
            name: folder_name.to_string(),
            entry: file_name.to_string(),
            mod_type: "preload-native".to_string(),
            inject_delay_ms: Some(0),
            extra: serde_json::Map::new(),
        };
        let manifest_text = serde_json::to_string_pretty(&manifest)
            .map_err(|error| format!("Manifest 序列化失败: {error}"))?;
        fs::write(target_dir.join("manifest.json"), manifest_text)
            .await
            .map_err(|error| format!("写入 Manifest 失败: {error}"))?;
    }

    Ok(())
}

pub async fn import_non_mod_files(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    tab: ManageTab,
    pack_subtype: ManagePackSubtype,
    selected_gdk_user: Option<&str>,
    file_paths: Vec<String>,
    allow_shared_fallback: bool,
    overwrite: bool,
) -> Result<ImportAssetsResult, String> {
    let _ = pack_subtype;
    let build_type = version.build_type();
    let edition = version.edition();

    import_assets(ImportAssetsRequest {
        build_type,
        edition,
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: if matches!(tab, ManageTab::Map) {
            selected_gdk_user.map(ToString::to_string)
        } else {
            None
        },
        file_paths,
        overwrite,
        allow_shared_fallback,
    })
    .await
}

pub async fn inspect_import_path(
    file_path: String,
    locale_code: Option<String>,
) -> Result<PackagePreview, String> {
    inspect_import_file(file_path, locale_code).await
}

pub async fn check_asset_import_conflict(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    tab: ManageTab,
    selected_gdk_user: Option<&str>,
    file_path: String,
    allow_shared_fallback: bool,
) -> Result<ImportCheckResult, String> {
    check_import_conflict(CheckImportRequest {
        build_type: version.build_type(),
        edition: version.edition(),
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: if matches!(tab, ManageTab::Map) {
            selected_gdk_user.map(ToString::to_string)
        } else {
            None
        },
        file_path,
        allow_shared_fallback,
    })
    .await
}

pub fn read_level_dat_document(folder_path: &str) -> Result<LevelDatDocument, String> {
    let level_dat_path = PathBuf::from(folder_path).join("level.dat");
    read_level_dat_file_document(&level_dat_path)
        .map_err(|error| format!("读取 level.dat 失败: {error}"))
}

pub fn write_level_dat_document(
    folder_path: &str,
    document: &LevelDatDocument,
) -> Result<(), String> {
    let level_dat_path = PathBuf::from(folder_path).join("level.dat");
    write_level_dat_file_document(&level_dat_path, document)
        .map_err(|error| format!("写入 level.dat 失败: {error}"))
}

pub fn export_map(folder_path: &str, target_path: &str) -> Result<(), String> {
    let source = PathBuf::from(folder_path);
    let target = PathBuf::from(target_path);
    zip_directory(&source, &target).map_err(|error| error.to_string())
}

pub fn backup_map(folder_path: &str, map_name: &str) -> Result<String, String> {
    let source = PathBuf::from(folder_path);
    let backup_dir = crate::utils::file_ops::bmcbl_subdir("backup");
    std::fs::create_dir_all(&backup_dir).map_err(|error| format!("创建备份目录失败: {error}"))?;

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let safe_name: String = map_name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect();
    let target_path = backup_dir.join(format!("{safe_name}_{timestamp}.mcworld"));
    zip_directory(&source, &target_path).map_err(|error| error.to_string())?;
    Ok(target_path.to_string_lossy().to_string())
}

fn manage_version_config_from_core(config: VersionConfig) -> ManageVersionConfig {
    ManageVersionConfig {
        enable_debug_console: config.enable_debug_console,
        enable_redirection: config.enable_redirection,
        editor_mode: config.editor_mode,
        disable_mod_loading: config.disable_mod_loading,
        lock_mouse_on_launch: config.lock_mouse_on_launch,
        unlock_mouse_hotkey: config.unlock_mouse_hotkey.into(),
        reduce_pixels: config.reduce_pixels,
        vanilla_skin_pack_redirect: config.vanilla_skin_pack_redirect.map(SharedString::from),
    }
}

async fn load_mod_assets(version: &ManagedVersionEntry) -> Result<Vec<ManageAssetEntry>, String> {
    let version_folder = version.folder.to_string();
    tokio::task::spawn_blocking(move || load_mod_assets_blocking(&version_folder))
        .await
        .map_err(|error| format!("读取 Mod 任务失败: {error:?}"))?
}

fn load_mod_assets_blocking(version_folder: &str) -> Result<Vec<ManageAssetEntry>, String> {
    let mods_dir = version_mods_dir(version_folder);
    if !mods_dir.exists() {
        std_fs::create_dir_all(&mods_dir)
            .map_err(|error| format!("创建 mods 目录失败: {error}"))?;
        return Ok(Vec::new());
    }

    let entries =
        std_fs::read_dir(&mods_dir).map_err(|error| format!("读取 mods 目录失败: {error}"))?;
    let mut assets = Vec::new();

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!(
                    "read mod directory entry failed {}: {error}",
                    mods_dir.display()
                );
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let folder_name = entry.file_name().to_string_lossy().to_string();
        let enabled_manifest_path = path.join("manifest.json");
        let disabled_manifest_path = path.join(".manifest.json");
        let (enabled, manifest_path) = if enabled_manifest_path.exists() {
            (true, enabled_manifest_path)
        } else if disabled_manifest_path.exists() {
            (false, disabled_manifest_path)
        } else {
            continue;
        };

        let content = match std_fs::read_to_string(&manifest_path) {
            Ok(content) => content,
            Err(error) => {
                warn!(
                    "read mod manifest failed {}: {error}",
                    manifest_path.display()
                );
                continue;
            }
        };
        let manifest: ModManifest = match serde_json::from_str(&content) {
            Ok(manifest) => manifest,
            Err(error) => {
                warn!(
                    "parse mod manifest failed {}: {error}",
                    manifest_path.display()
                );
                continue;
            }
        };

        let file_path = path.join(&manifest.entry);
        let detail = if manifest.mod_type == "hot-inject" {
            Some(SharedString::from(format!(
                "{} · {} ms",
                manifest.mod_type,
                manifest.inject_delay_ms.unwrap_or(0)
            )))
        } else {
            Some(SharedString::from(manifest.mod_type.clone()))
        };

        assets.push(ManageAssetEntry {
            key: SharedString::from(format!("mod:{folder_name}")),
            folder_name: SharedString::from(folder_name.clone()),
            display_name: SharedString::from(manifest.name),
            detail,
            description: None,
            file_path: SharedString::from(file_path.to_string_lossy().to_string()),
            open_path: SharedString::from(path.to_string_lossy().to_string()),
            icon_path: None,
            modified_iso: None,
            modified_label: None,
            size_bytes: None,
            size_label: None,
            source: None,
            edition: None,
            gdk_user: None,
            enabled: Some(enabled),
            mod_type: Some(SharedString::from(manifest.mod_type)),
            inject_delay_ms: Some(manifest.inject_delay_ms.unwrap_or(0)),
            resource_pack_count: None,
            behavior_pack_count: None,
            skin_count: None,
            first_skin_full_texture_path: None,
            first_skin_model_label: None,
            skin_previews: None,
            kind: ManageAssetKind::Mod,
        });
    }

    Ok(assets)
}

async fn load_pack_assets(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    pack_subtype: ManagePackSubtype,
    _selected_gdk_user: Option<&str>,
    locale_code: &str,
) -> Result<Vec<ManageAssetEntry>, String> {
    let options = GamePathOptions {
        build_type: version.build_type(),
        edition: version.edition(),
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: None,
        allow_shared_fallback: false,
    };
    let locale = locale_code.to_string();
    let entries = tokio::task::spawn_blocking(move || match pack_subtype {
        ManagePackSubtype::Resource => crate::core::minecraft::resource_packs::read_packs_standard(
            "resource_packs",
            &locale,
            &options,
        ),
        ManagePackSubtype::Behavior => crate::core::minecraft::resource_packs::read_packs_standard(
            "behavior_packs",
            &locale,
            &options,
        ),
    })
    .await
    .map_err(|error| format!("读取资源包任务失败: {error:?}"))?
    .map_err(|error| format!("读取资源包失败: {error:?}"))?;

    Ok(entries
        .into_iter()
        .map(|pack| manage_asset_from_pack(pack, pack_subtype))
        .collect())
}

async fn load_skin_pack_assets(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    locale_code: &str,
) -> Result<Vec<ManageAssetEntry>, String> {
    let options = GamePathOptions {
        build_type: version.build_type(),
        edition: version.edition(),
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: None,
        allow_shared_fallback: false,
    };
    let locale = locale_code.to_string();
    let entries = tokio::task::spawn_blocking(move || {
        crate::core::minecraft::skin_packs::read_skin_packs_standard(&locale, &options)
    })
    .await
    .map_err(|error| format!("读取皮肤包任务失败: {error:?}"))?
    .map_err(|error| format!("读取皮肤包失败: {error:?}"))?;

    Ok(entries
        .into_iter()
        .map(manage_asset_from_skin_pack)
        .collect())
}

async fn load_map_assets(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    selected_gdk_user: Option<&str>,
) -> Result<Vec<ManageAssetEntry>, String> {
    let options = GamePathOptions {
        build_type: version.build_type(),
        edition: version.edition(),
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: selected_gdk_user.map(ToString::to_string),
        allow_shared_fallback: false,
    };
    let entries = tokio::task::spawn_blocking(move || {
        crate::core::minecraft::map::list_worlds_standard(&options)
    })
    .await
    .map_err(|error| format!("读取地图任务失败: {error:?}"))?
    .map_err(|error| format!("读取地图失败: {error:?}"))?;

    Ok(entries.into_iter().map(manage_asset_from_map).collect())
}

fn manage_asset_from_pack(pack: McPackInfo, pack_subtype: ManagePackSubtype) -> ManageAssetEntry {
    let manifest_header = pack
        .manifest_parsed
        .as_ref()
        .and_then(|manifest| manifest.header.as_ref());
    let display_name = manifest_header
        .and_then(|header| header.name.clone())
        .or_else(|| {
            pack.manifest.get("header").and_then(|header| {
                header
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(ToString::to_string)
            })
        })
        .unwrap_or_else(|| pack.folder_name.clone());

    let detail = manifest_version_label(manifest_header);
    let description = pack
        .short_description
        .filter(|value| !value.trim().is_empty())
        .map(SharedString::from);

    ManageAssetEntry {
        key: SharedString::from(format!("pack:{}", pack.folder_name)),
        folder_name: SharedString::from(pack.folder_name.clone()),
        display_name: SharedString::from(display_name),
        detail,
        description,
        file_path: SharedString::from(pack.folder_path.clone()),
        open_path: SharedString::from(pack.folder_path.clone()),
        icon_path: pack.icon_path.map(SharedString::from),
        modified_iso: None,
        modified_label: None,
        size_bytes: None,
        size_label: None,
        source: pack.source.map(SharedString::from),
        edition: pack.edition.map(SharedString::from),
        gdk_user: pack.gdk_user.map(SharedString::from),
        enabled: None,
        mod_type: None,
        inject_delay_ms: None,
        resource_pack_count: None,
        behavior_pack_count: None,
        skin_count: None,
        first_skin_full_texture_path: None,
        first_skin_model_label: None,
        skin_previews: None,
        kind: match pack_subtype {
            ManagePackSubtype::Resource | ManagePackSubtype::Behavior => {
                ManageAssetKind::ResourcePack
            }
        },
    }
}

fn manage_asset_from_skin_pack(pack: McSkinPackInfo) -> ManageAssetEntry {
    let mut detail_parts = Vec::new();
    detail_parts.push(format!("{} 个皮肤", pack.skin_count));
    if pack.slim_skin_count > 0 {
        detail_parts.push(format!("Alex {}", pack.slim_skin_count));
    }
    if let Some(version) = pack.version.clone() {
        detail_parts.push(version);
    }
    let detail = (!detail_parts.is_empty()).then(|| SharedString::from(detail_parts.join(" · ")));
    let first_skin_full_texture_path = pack
        .first_full_skin_texture_path()
        .map(|path| SharedString::from(path.to_string()));
    let skin_previews = skin_previews_from_pack(&pack);
    let description = pack
        .description
        .filter(|value| !value.trim().is_empty())
        .map(SharedString::from);
    let first_skin_model_label = pack
        .skins
        .iter()
        .find(|skin| skin.full_texture_path().is_some())
        .map(|skin| SharedString::from(skin.model_label.clone()));
    ManageAssetEntry {
        key: SharedString::from(format!("skin:{}", pack.folder_name)),
        folder_name: SharedString::from(pack.folder_name.clone()),
        display_name: SharedString::from(pack.display_name),
        detail,
        description,
        file_path: SharedString::from(pack.folder_path.clone()),
        open_path: SharedString::from(pack.folder_path.clone()),
        icon_path: pack.preview_path.or(pack.icon_path).map(SharedString::from),
        modified_iso: None,
        modified_label: None,
        size_bytes: None,
        size_label: None,
        source: pack.source.map(SharedString::from),
        edition: pack.edition.map(SharedString::from),
        gdk_user: pack.gdk_user.map(SharedString::from),
        enabled: None,
        mod_type: None,
        inject_delay_ms: None,
        resource_pack_count: None,
        behavior_pack_count: None,
        skin_count: Some(pack.skin_count),
        first_skin_full_texture_path,
        first_skin_model_label,
        skin_previews,
        kind: ManageAssetKind::SkinPack,
    }
}

fn skin_previews_from_pack(pack: &McSkinPackInfo) -> Option<Arc<[ManageSkinPreviewEntry]>> {
    let previews = pack
        .skins
        .iter()
        .filter_map(|skin| {
            let full_texture_path = skin.full_texture_path()?;
            Some(ManageSkinPreviewEntry {
                display_name: SharedString::from(skin.display_name.clone()),
                full_texture_path: SharedString::from(full_texture_path.to_string()),
                preview_path: skin.preview_path.clone().map(SharedString::from),
                model_label: SharedString::from(skin.model_label.clone()),
                geometry_path: skin.geometry_path.clone().map(SharedString::from),
                geometry_identifier: skin.geometry_identifier.clone().map(SharedString::from),
            })
        })
        .collect::<Vec<_>>();

    (!previews.is_empty()).then(|| Arc::from(previews.into_boxed_slice()))
}

fn manage_asset_from_map(map: McMapInfo) -> ManageAssetEntry {
    let display_name = map
        .level_name
        .clone()
        .unwrap_or_else(|| map.folder_name.clone());

    let mut detail_parts = Vec::new();
    if let Some(size) = map.size_readable.clone() {
        detail_parts.push(size);
    }
    if let Some(modified) = map.modified.clone() {
        detail_parts.push(format_date_label(&modified));
    }
    let detail = (!detail_parts.is_empty()).then(|| SharedString::from(detail_parts.join(" · ")));

    ManageAssetEntry {
        key: SharedString::from(format!("map:{}", map.folder_name)),
        folder_name: SharedString::from(map.folder_name.clone()),
        display_name: SharedString::from(display_name),
        detail,
        description: None,
        file_path: SharedString::from(map.folder_path.clone()),
        open_path: SharedString::from(map.folder_path.clone()),
        icon_path: map.icon_path.map(SharedString::from),
        modified_iso: map.modified.clone().map(SharedString::from),
        modified_label: map
            .modified
            .as_deref()
            .map(format_date_label)
            .map(SharedString::from),
        size_bytes: map.size_bytes,
        size_label: map.size_readable.map(SharedString::from),
        source: map.source.map(SharedString::from),
        edition: map.edition.map(SharedString::from),
        gdk_user: map.gdk_user.map(SharedString::from),
        enabled: None,
        mod_type: None,
        inject_delay_ms: None,
        resource_pack_count: map.resource_packs_count,
        behavior_pack_count: map.behavior_packs_count,
        skin_count: None,
        first_skin_full_texture_path: None,
        first_skin_model_label: None,
        skin_previews: None,
        kind: ManageAssetKind::Map,
    }
}

fn manage_screenshot_from_core(
    screenshot: crate::core::minecraft::screenshots::McScreenshotInfo,
) -> ManageScreenshotEntry {
    ManageScreenshotEntry {
        key: SharedString::from(screenshot.key),
        image_path: SharedString::from(screenshot.image_path),
        folder_path: SharedString::from(screenshot.folder_path),
        file_name: SharedString::from(screenshot.file_name),
        capture_time_iso: screenshot.capture_time.clone().map(SharedString::from),
        capture_time_label: screenshot
            .capture_time
            .as_deref()
            .map(format_date_label)
            .map(SharedString::from),
        modified_iso: screenshot.modified.clone().map(SharedString::from),
        modified_label: screenshot
            .modified
            .as_deref()
            .map(format_date_label)
            .map(SharedString::from),
        size_bytes: screenshot.size_bytes,
        size_label: screenshot
            .size_bytes
            .map(bytes_to_human)
            .map(SharedString::from),
        gdk_user: screenshot.gdk_user.map(SharedString::from),
    }
}

fn manage_server_from_core(
    server: crate::core::minecraft::servers::ExternalServerEntry,
) -> ManageServerEntry {
    ManageServerEntry {
        key: SharedString::from(server.key),
        index: server.index,
        name: SharedString::from(server.name),
        address: SharedString::from(server.address),
        port: server.port,
        file_path: SharedString::from(server.file_path),
        line_number: server.line_number,
    }
}

fn external_server_options(
    version: &ManagedVersionEntry,
    config: &ManageVersionConfig,
    selected_gdk_user: Option<&str>,
) -> GamePathOptions {
    GamePathOptions {
        build_type: version.build_type(),
        edition: version.edition(),
        version_name: version.folder.to_string(),
        enable_isolation: config.enable_redirection,
        user_id: selected_gdk_user.map(ToString::to_string),
        allow_shared_fallback: false,
    }
}

async fn delete_mods(version_folder: &str, mod_ids: &[String]) -> Result<(), String> {
    let mods_dir = version_mods_dir(version_folder);
    for mod_id in mod_ids {
        let target_dir = mods_dir.join(mod_id);
        if !target_dir.exists() {
            return Err(format!("未找到 Mod 目录: {mod_id}"));
        }
        fs::remove_dir_all(&target_dir)
            .await
            .map_err(|error| format!("删除 Mod 失败: {error}"))?;
    }
    Ok(())
}

async fn editable_manifest_path(version_folder: &str, mod_id: &str) -> Result<PathBuf, String> {
    let mod_dir = version_mods_dir(version_folder).join(mod_id);
    if !mod_dir.exists() {
        return Err(format!("Mod 目录不存在: {mod_id}"));
    }

    let enabled_path = mod_dir.join("manifest.json");
    if enabled_path.exists() {
        return Ok(enabled_path);
    }
    let disabled_path = mod_dir.join(".manifest.json");
    if disabled_path.exists() {
        return Ok(disabled_path);
    }
    Err("未找到 manifest.json 或 .manifest.json".to_string())
}

fn version_mods_dir(version_folder: &str) -> PathBuf {
    crate::utils::file_ops::bmcbl_subdir("versions")
        .join(version_folder)
        .join("mods")
}

fn manifest_version_label(header: Option<&Header>) -> Option<SharedString> {
    let version = header.and_then(|header| header.version.as_ref())?;
    if version.is_empty() {
        return None;
    }
    Some(SharedString::from(
        version
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join("."),
    ))
}

fn format_date_label(iso_text: &str) -> String {
    let trimmed = iso_text.trim();
    if trimmed.len() >= 10 {
        trimmed[..10].replace('-', "/")
    } else {
        trimmed.to_string()
    }
}

fn bytes_to_human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit_index = 0;
    while value >= 1024.0 && unit_index + 1 < UNITS.len() {
        value /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.2} {}", UNITS[unit_index])
    }
}

fn zip_directory(source_dir: &Path, target_file: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(source_dir.exists(), "源目录不存在");

    let file = File::create(target_file)
        .with_context(|| format!("创建目标文件失败: {}", target_file.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    let walker = walkdir::WalkDir::new(source_dir);
    for entry in walker.into_iter().filter_map(Result::ok) {
        let path = entry.path();
        let relative_name = path
            .strip_prefix(source_dir)?
            .to_string_lossy()
            .replace('\\', "/");

        if path.is_file() {
            zip.start_file(relative_name, options)?;
            let mut source_file = File::open(path)?;
            let mut buffer = Vec::new();
            source_file.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
        } else if !relative_name.is_empty() {
            zip.add_directory(relative_name, options)?;
        }
    }

    zip.finish()?;
    Ok(())
}
