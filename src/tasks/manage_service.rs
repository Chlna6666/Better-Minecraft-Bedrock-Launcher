use std::fs;
use std::path::PathBuf;

use serde::Deserialize;
use tracing::warn;

use crate::core::minecraft::map::McMapInfo;
use crate::core::minecraft::paths::{GamePathOptions, get_game_root};
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
    pub inject_delay_ms: u64,
}

#[derive(Deserialize)]
struct ModManifest {
    name: String,
    entry: String,
    #[serde(rename = "type")]
    mod_type: String,
    #[serde(default)]
    inject_delay_ms: Option<u64>,
}

pub async fn load_version_config(folder_name: String) -> Result<VersionConfig, String> {
    run_blocking(
        BlockingTaskOptions::hidden("读取版本配置"),
        move || get_version_config_blocking(&folder_name),
    )
    .await
}

pub async fn load_gdk_users(options: GamePathOptions) -> Result<Vec<String>, String> {
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
                        .map(|_| entry.file_name().to_string_lossy().into_owned())
                })
                .filter(|folder_name| !folder_name.eq_ignore_ascii_case("public"))
                .collect::<Vec<_>>();
            users.sort_by(|left, right| {
                left.eq_ignore_ascii_case("shared")
                    .cmp(&right.eq_ignore_ascii_case("shared"))
                    .then_with(|| left.cmp(right))
            });
            Ok(users)
        },
    )
    .await
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
        fs::create_dir_all(&mods_dir).map_err(|error| format!("创建 mods 目录失败: {error}"))?;
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
            inject_delay_ms: manifest.inject_delay_ms.unwrap_or(0),
        });
    }
    Ok(mods)
}
