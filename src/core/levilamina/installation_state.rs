use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::install::{
    CRASH_LOGGER_PACKAGE, LEVILAMINA_PACKAGE, LOCATION_PACKAGE, LeviLaminaInstallation,
    PRELOADER_PACKAGE, RUNTIME_DATA_PACKAGE,
};
use super::lip::PackageId;
use super::planner::ResolvedPackage;

const LOCK_FILE_NAME: &str = ".bmcbl-levilamina-lock.json";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct InstallLock {
    format_version: u32,
    #[serde(default)]
    packages: Vec<LockedPackage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LockedPackage {
    package_id: String,
    variant: String,
    version: String,
    explicit: bool,
    files: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct InstalledPackage {
    pub(super) id: PackageId,
    pub(super) version: String,
}

pub(super) async fn inspect_installation(
    game_directory: PathBuf,
) -> Result<LeviLaminaInstallation, String> {
    crate::tasks::runtime::run_io_blocking(move || inspect_installation_sync(&game_directory))
        .await?
}

pub(super) async fn read_installed_packages(
    game_directory: &Path,
) -> Result<Vec<InstalledPackage>, String> {
    let game_directory = game_directory.to_path_buf();
    crate::tasks::runtime::run_io_blocking(move || read_installed_packages_sync(&game_directory))
        .await?
}

pub(super) async fn write_preloader_manifest(
    game_directory: &Path,
    version: &str,
) -> Result<PathBuf, String> {
    let game_directory = game_directory.to_path_buf();
    let version = version.to_string();
    crate::tasks::runtime::run_archive_blocking(move || {
        let path = game_directory.join("mods/PreLoader/manifest.json");
        let parent = path
            .parent()
            .ok_or_else(|| "PreLoader 目录无效".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 PreLoader 目录失败: {error}"))?;
        let manifest = serde_json::json!({
            "name": "LeviLamina PreLoader",
            "entry": "PreLoader.dll",
            "type": "preload",
            "inject_delay_ms": null,
            "version": version,
        });
        let bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| format!("序列化 PreLoader 清单失败: {error}"))?;
        std::fs::write(&path, bytes)
            .map_err(|error| format!("写入 PreLoader 清单失败: {error}"))?;
        Ok(path)
    })
    .await?
}

pub(super) async fn update_lock(
    game_directory: &Path,
    package: &ResolvedPackage,
    files: Vec<PathBuf>,
) -> Result<(), String> {
    let game_directory = game_directory.to_path_buf();
    let package_id = package.id.clone();
    let version = package.manifest.version.clone();
    let explicit = package.explicit;
    crate::tasks::runtime::run_archive_blocking(move || {
        let mut lock = read_lock(&game_directory)?;
        remove_locked_package(&mut lock, &package_id);
        let relative_files = files
            .into_iter()
            .map(|path| {
                path.strip_prefix(&game_directory)
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"))
                    .map_err(|_| format!("Lip 安装文件越出游戏目录: {}", path.display()))
            })
            .collect::<Result<Vec<_>, String>>()?;
        lock.format_version = 1;
        lock.packages.push(LockedPackage {
            package_id: package_id.path,
            variant: package_id.variant,
            version,
            explicit,
            files: relative_files,
        });
        write_lock(&game_directory, &lock)
    })
    .await?
}

pub(super) async fn prepare_package_install(
    game_directory: &Path,
    package_id: &PackageId,
) -> Result<(), String> {
    let game_directory = game_directory.to_path_buf();
    let package_id = package_id.clone();
    crate::tasks::runtime::run_archive_blocking(move || {
        let mut lock = read_lock(&game_directory)?;
        let removed_files = remove_locked_package(&mut lock, &package_id);
        remove_relative_files(&game_directory, &removed_files)?;
        write_lock(&game_directory, &lock)
    })
    .await?
}

pub(super) async fn uninstall_loader(game_directory: &Path) -> Result<(), String> {
    let game_directory = game_directory.to_path_buf();
    crate::tasks::runtime::run_archive_blocking(move || {
        let mut lock = read_lock(&game_directory)?;
        let mut retained = Vec::with_capacity(lock.packages.len());
        for package in lock.packages {
            if is_loader_component(&package.package_id) {
                remove_relative_files(&game_directory, &package.files)?;
            } else {
                retained.push(package);
            }
        }
        lock.packages = retained;
        remove_known_loader_paths(&game_directory)?;
        if lock.packages.is_empty() {
            remove_file_if_exists(&game_directory.join(LOCK_FILE_NAME))?;
        } else {
            write_lock(&game_directory, &lock)?;
        }
        Ok(())
    })
    .await?
}

fn inspect_installation_sync(game_directory: &Path) -> Result<LeviLaminaInstallation, String> {
    let loader_version = read_mod_manifest_version(&game_directory.join("mods/LeviLamina"))?;
    let preloader_version = read_mod_manifest_version(&game_directory.join("mods/PreLoader"))?;
    Ok(LeviLaminaInstallation {
        loader_version,
        preloader_version,
        has_runtime_data: game_directory.join("bedrock_runtime_data").is_file(),
    })
}

fn read_mod_manifest_version(directory: &Path) -> Result<Option<String>, String> {
    let path = directory.join("manifest.json");
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| format!("读取 Mod 清单失败 {}: {error}", path.display()))?;
    let json: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("解析 Mod 清单失败 {}: {error}", path.display()))?;
    Ok(json
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(ToOwned::to_owned))
}

fn is_loader_component(package_path: &str) -> bool {
    [
        LEVILAMINA_PACKAGE,
        PRELOADER_PACKAGE,
        RUNTIME_DATA_PACKAGE,
        CRASH_LOGGER_PACKAGE,
        LOCATION_PACKAGE,
    ]
    .iter()
    .any(|component| package_path.eq_ignore_ascii_case(component))
}

fn read_lock(game_directory: &Path) -> Result<InstallLock, String> {
    let path = game_directory.join(LOCK_FILE_NAME);
    if !path.is_file() {
        return Ok(InstallLock::default());
    }
    let bytes =
        std::fs::read(&path).map_err(|error| format!("读取 LeviLamina 锁文件失败: {error}"))?;
    serde_json::from_slice(&bytes).map_err(|error| format!("解析 LeviLamina 锁文件失败: {error}"))
}

fn read_installed_packages_sync(game_directory: &Path) -> Result<Vec<InstalledPackage>, String> {
    let lock = read_lock(game_directory)?;
    let mut installed = Vec::with_capacity(lock.packages.len());
    for package in lock.packages {
        if package.files.is_empty() {
            continue;
        }
        let mut files_present = true;
        for relative_path in &package.files {
            if !safe_locked_path(game_directory, relative_path)?.exists() {
                files_present = false;
                break;
            }
        }
        if files_present {
            installed.push(InstalledPackage {
                id: PackageId {
                    path: package.package_id,
                    variant: package.variant,
                },
                version: package.version,
            });
        }
    }
    Ok(installed)
}

fn write_lock(game_directory: &Path, lock: &InstallLock) -> Result<(), String> {
    let path = game_directory.join(LOCK_FILE_NAME);
    let temporary = game_directory.join(format!("{LOCK_FILE_NAME}.tmp"));
    let bytes = serde_json::to_vec_pretty(lock)
        .map_err(|error| format!("序列化 LeviLamina 锁文件失败: {error}"))?;
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("写入 LeviLamina 临时锁文件失败: {error}"))?;
    remove_file_if_exists(&path)?;
    std::fs::rename(&temporary, &path)
        .map_err(|error| format!("提交 LeviLamina 锁文件失败: {error}"))
}

fn remove_locked_package(lock: &mut InstallLock, package_id: &PackageId) -> Vec<String> {
    let mut removed_files = Vec::new();
    lock.packages.retain(|package| {
        let matches = package.package_id.eq_ignore_ascii_case(&package_id.path)
            && package.variant == package_id.variant;
        if matches {
            removed_files.extend(package.files.clone());
        }
        !matches
    });
    removed_files
}

fn remove_relative_files(game_directory: &Path, files: &[String]) -> Result<(), String> {
    for relative in files {
        let path = safe_locked_path(game_directory, relative)?;
        remove_file_if_exists(&path)?;
    }
    Ok(())
}

pub(super) fn safe_locked_path(game_directory: &Path, relative: &str) -> Result<PathBuf, String> {
    let relative_path = Path::new(relative);
    if relative_path.is_absolute()
        || relative_path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(format!("锁文件包含不安全路径: {relative}"));
    }
    Ok(game_directory.join(relative_path))
}

fn remove_known_loader_paths(game_directory: &Path) -> Result<(), String> {
    for directory in ["mods/LeviLamina", "mods/PreLoader"] {
        let path = game_directory.join(directory);
        if path.exists() {
            std::fs::remove_dir_all(&path)
                .map_err(|error| format!("删除 LeviLamina 目录失败 {}: {error}", path.display()))?;
        }
    }
    remove_file_if_exists(&game_directory.join("bedrock_runtime_data"))
}

fn remove_file_if_exists(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    std::fs::remove_file(path)
        .map_err(|error| format!("删除 LeviLamina 文件失败 {}: {error}", path.display()))
}
