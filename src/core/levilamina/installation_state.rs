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
        let path = game_directory.join("mods/PreLoader/bloader.json");
        let parent = path
            .parent()
            .ok_or_else(|| "PreLoader 目录无效".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 PreLoader 目录失败: {error}"))?;
        if !parent.join("PreLoader.dll").is_file() {
            return Err(format!(
                "PreLoader.dll 尚未安装: {}，请检查 Lip 资产下载及解压步骤",
                parent.join("PreLoader.dll").display()
            ));
        }
        let manifest = serde_json::json!({
            "name": "PreLoader",
            "entry": "PreLoader.dll",
            "type": "preloader-bootstrap",
            "version": version,
        });
        let bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| format!("序列化 PreLoader 清单失败: {error}"))?;
        std::fs::write(&path, bytes)
            .map_err(|error| format!("写入 PreLoader 引导清单失败: {error}"))?;
        repair_preloader_manifest_type(&game_directory)?;
        tracing::info!(
            path = %path.display(),
            "PreLoader 引导元数据写入 bloader.json；DLL 位置未改变"
        );
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
    // Status inspection must be read-only: no DLL moves while the user
    // selects a game version or opens the manage page.
    let loader_version = read_mod_manifest_version(&game_directory.join("mods/LeviLamina"))?;
    let preloader_version = validated_preloader_version(game_directory)?;
    Ok(LeviLaminaInstallation {
        loader_version,
        preloader_version,
        has_runtime_data: game_directory.join("bedrock_runtime_data").is_file(),
    })
}

/// Read-only installation validation. A lock record is insufficient: older
/// BMCBL builds could record a bootstrap DLL at the wrong path and then
/// report it as installed even though the runtime could not discover it.
pub(super) fn validated_preloader_version(game_directory: &Path) -> Result<Option<String>, String> {
    let dir = game_directory.join("mods").join("PreLoader");
    if !dir.join("PreLoader.dll").is_file() {
        return Ok(None);
    }
    let path = dir.join("bloader.json");
    if !path.is_file() {
        return Ok(None);
    }
    let raw = std::fs::read(&path)
        .map_err(|error| format!("读取 PreLoader 元数据失败 {}: {error}", path.display()))?;
    let value: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "PreLoader 元数据损坏，安装器将重新下载而非误判依赖已安装"
            );
            return Ok(None);
        }
    };
    let valid = value.get("type").and_then(serde_json::Value::as_str)
        == Some("preloader-bootstrap")
        && value.get("entry").and_then(serde_json::Value::as_str)
            == Some("PreLoader.dll");
    if !valid {
        return Ok(None);
    }
    Ok(value.get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|version| !version.trim().is_empty())
        .map(str::to_owned))
}

/// LeviLamina scans manifest.json under mods/, including PreLoader itself.
/// BLoader metadata must therefore use bloader.json. Only BMCBL-owned
/// metadata is renamed; PreLoader.dll and directories are never touched.
pub(super) fn repair_preloader_manifest_type(game_directory: &Path) -> Result<(), String> {
    let dir = game_directory.join("mods").join("PreLoader");
    if !dir.join("PreLoader.dll").is_file() {
        return Ok(());
    }
    let legacy = dir.join("manifest.json");
    if !legacy.is_file() {
        return Ok(());
    }
    let raw = std::fs::read(&legacy)
        .map_err(|e| format!("读取旧 PreLoader 元数据失败 {}: {e}", legacy.display()))?;
    let mut value: serde_json::Value = match serde_json::from_slice(&raw) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(path = %legacy.display(), %error, "跳过损坏的 PreLoader 元数据");
            return Ok(());
        }
    };
    let owned = matches!(
        value.get("name").and_then(serde_json::Value::as_str),
        Some("PreLoader" | "LeviLamina PreLoader")
    ) && value.get("entry").and_then(serde_json::Value::as_str) == Some("PreLoader.dll")
        && matches!(
            value.get("type").and_then(serde_json::Value::as_str),
            Some("preload" | "preload-native" | "preloader-bootstrap")
        );
    if !owned {
        return Ok(());
    }
    let object = value.as_object_mut()
        .ok_or_else(|| "PreLoader Manifest 必须为 JSON 对象".to_string())?;
    object.insert("type".to_string(), serde_json::json!("preloader-bootstrap"));
    object.insert("name".to_string(), serde_json::json!("PreLoader"));
    let sidecar = dir.join("bloader.json");
    if !sidecar.exists() {
        let data = serde_json::to_vec_pretty(&value)
            .map_err(|e| format!("序列化 PreLoader 元数据失败: {e}"))?;
        std::fs::write(&sidecar, data)
            .map_err(|e| format!("写入 PreLoader 引导元数据失败 {}: {e}", sidecar.display()))?;
    }
    // Remove only the known BLoader-owned legacy JSON after the new
    // descriptor is durably present; never change DLL or package files.
    std::fs::remove_file(&legacy)
        .map_err(|e| format!("清理重复 PreLoader Manifest 失败 {}: {e}", legacy.display()))?;
    tracing::info!(
        path = %sidecar.display(),
        "PreLoader 使用独立 bloader.json，不再被 LeviLamina 识别为普通 Mod"
    );
    Ok(())
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
    // This path runs only inside the explicit installer, never when launching
    // Minecraft or reading version details.
    repair_preloader_manifest_type(game_directory)?;
    let lock = read_lock(game_directory)?;
    let mut installed = Vec::with_capacity(lock.packages.len());
    for package in lock.packages {
        if package.files.is_empty() {
            continue;
        }
        if package.package_id.eq_ignore_ascii_case(PRELOADER_PACKAGE) {
            let valid_version = validated_preloader_version(game_directory)?;
            if valid_version.as_deref() != Some(package.version.as_str()) {
                tracing::info!(
                    package = %package.package_id,
                    expected = %package.version,
                    installed = ?valid_version,
                    "PreLoader 缺失或元数据不匹配，将重新安装而非复用旧锁记录"
                );
                continue;
            }
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
