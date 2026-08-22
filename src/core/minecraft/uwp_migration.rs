#![cfg(target_os = "windows")]

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Management::Deployment::PackageManager;
use windows::core::HSTRING;

const RELEASE_FAMILY: &str = "Microsoft.MinecraftUWP_8wekyb3d8bbwe";
const PREVIEW_FAMILY: &str = "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe";
const EDUCATION_FAMILY: &str = "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe";
const EDUCATION_PREVIEW_FAMILY: &str =
    "Microsoft.MinecraftEducationEditionBeta_8wekyb3d8bbwe";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MinecraftDataSummary {
    pub family_name: String,
    pub registered: bool,
    pub registered_version: Option<String>,
    pub registered_path: Option<PathBuf>,
    pub development_mode: bool,
    pub bmcbl_managed_registration: bool,
    pub local_state: PathBuf,
    pub data_present: bool,
    pub file_count: u64,
    pub total_size: u64,
    pub worlds: u64,
    pub resource_packs: u64,
    pub behavior_packs: u64,
    pub skin_packs: u64,
    pub screenshots: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct OnboardingEnvironmentSummary {
    pub release: MinecraftDataSummary,
    pub preview: MinecraftDataSummary,
    pub bmcbl_versions: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MigrationManifest {
    schema: u32,
    family_name: String,
    created_at_epoch: u64,
    source_local_state: PathBuf,
    backup_local_state: PathBuf,
    file_count: u64,
    total_size: u64,
    restored: bool,
}

#[derive(Debug, Default)]
struct RegistrationSummary {
    registered: bool,
    version: Option<String>,
    path: Option<PathBuf>,
    development_mode: bool,
}

fn local_state_for_family(family_name: &str) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(local)
            .join("Packages")
            .join(family_name)
            .join("LocalState"),
    )
}

fn read_registration(family_name: &str) -> RegistrationSummary {
    let Ok(manager) = PackageManager::new() else {
        return RegistrationSummary::default();
    };
    let Ok(packages) = manager.FindPackagesByUserSecurityIdPackageFamilyName(
        &HSTRING::new(),
        &HSTRING::from(family_name),
    ) else {
        return RegistrationSummary::default();
    };

    // 同一个 PackageFamily 可能包含资源包。引导只展示主包注册状态，避免把语言/资源包
    // 的版本或安装路径误当成 Minecraft 主包。
    for package in packages {
        let Ok(id) = package.Id() else { continue };
        if id.ResourceId().is_ok_and(|resource_id| !resource_id.is_empty()) {
            continue;
        }

        let version = id.Version().ok().map(|version| {
            format!(
                "{}.{}.{}.{}",
                version.Major, version.Minor, version.Build, version.Revision
            )
        });
        let path = package
            .InstalledLocation()
            .ok()
            .and_then(|location| location.Path().ok())
            .map(|path| PathBuf::from(path.to_string_lossy().to_string()));
        let development_mode = package.IsDevelopmentMode().unwrap_or(false);
        return RegistrationSummary {
            registered: true,
            version,
            path,
            development_mode,
        };
    }

    RegistrationSummary::default()
}

fn count_directories(path: &Path) -> u64 {
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .count() as u64
        })
        .unwrap_or(0)
}

fn walk_stats(path: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(metadata.len());
            }
        }
    }
    (files, bytes)
}

pub fn summarize_family(family_name: &str) -> MinecraftDataSummary {
    let registration = read_registration(family_name);
    let bmcbl_managed_registration = registration.development_mode
        && registration
            .path
            .as_deref()
            .is_some_and(is_bmcbl_managed_registration);
    let local_state = local_state_for_family(family_name).unwrap_or_default();
    let com_mojang = local_state.join("games").join("com.mojang");
    let data_present = com_mojang.is_dir();
    let (file_count, total_size) = if data_present {
        walk_stats(&com_mojang)
    } else {
        (0, 0)
    };
    MinecraftDataSummary {
        family_name: family_name.to_string(),
        registered: registration.registered,
        registered_version: registration.version,
        registered_path: registration.path,
        development_mode: registration.development_mode,
        bmcbl_managed_registration,
        local_state,
        data_present,
        file_count,
        total_size,
        worlds: count_directories(&com_mojang.join("minecraftWorlds")),
        resource_packs: count_directories(&com_mojang.join("resource_packs")),
        behavior_packs: count_directories(&com_mojang.join("behavior_packs")),
        skin_packs: count_directories(&com_mojang.join("skin_packs")),
        screenshots: fs::read_dir(com_mojang.join("Screenshots"))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_file())
                    .count() as u64
            })
            .unwrap_or(0),
    }
}

pub fn scan_onboarding_environment() -> OnboardingEnvironmentSummary {
    let versions = crate::utils::file_ops::bmcbl_subdir("versions");
    let bmcbl_versions = fs::read_dir(versions)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .count() as u64
        })
        .unwrap_or(0);
    OnboardingEnvironmentSummary {
        release: summarize_family(RELEASE_FAMILY),
        preview: summarize_family(PREVIEW_FAMILY),
        bmcbl_versions,
    }
}

pub fn is_bmcbl_managed_registration(path: &Path) -> bool {
    let versions = crate::utils::file_ops::bmcbl_subdir("versions");
    let versions = fs::canonicalize(&versions).unwrap_or(versions);
    let path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    path.starts_with(versions)
}

fn package_family_for_identity(identity_name: &str) -> Option<&'static str> {
    match identity_name {
        "Microsoft.MinecraftUWP" => Some(RELEASE_FAMILY),
        "Microsoft.MinecraftWindowsBeta" => Some(PREVIEW_FAMILY),
        "Microsoft.MinecraftEducationEdition" => Some(EDUCATION_FAMILY),
        "Microsoft.MinecraftEducationPreview" | "Microsoft.MinecraftEducationEditionBeta" => {
            Some(EDUCATION_PREVIEW_FAMILY)
        }
        _ => None,
    }
}

fn pending_root() -> PathBuf {
    crate::utils::file_ops::bmcbl_subdir("backups")
        .join("migrations")
        .join("uwp")
}

fn safe_family_name(family_name: &str) -> String {
    family_name.replace(
        |c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_',
        "_",
    )
}

fn pending_marker_for_family(family_name: &str) -> PathBuf {
    pending_root().join(format!("pending-{}.txt", safe_family_name(family_name)))
}

fn legacy_pending_marker() -> PathBuf {
    pending_root().join("pending.txt")
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(u64, u64), String> {
    fs::create_dir_all(destination).map_err(|e| format!("创建迁移目录失败: {e}"))?;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((src_dir, dst_dir)) = stack.pop() {
        fs::create_dir_all(&dst_dir)
            .map_err(|e| format!("创建目录失败 {}: {e}", dst_dir.display()))?;
        for entry in fs::read_dir(&src_dir)
            .map_err(|e| format!("读取目录失败 {}: {e}", src_dir.display()))?
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let src = entry.path();
            let dst = dst_dir.join(entry.file_name());
            let metadata = entry.metadata().map_err(|e| e.to_string())?;
            if metadata.is_dir() {
                stack.push((src, dst));
            } else if metadata.is_file() {
                let copied = fs::copy(&src, &dst).map_err(|e| {
                    format!("复制失败 {} -> {}: {e}", src.display(), dst.display())
                })?;
                files = files.saturating_add(1);
                bytes = bytes.saturating_add(copied);
            }
        }
    }
    Ok((files, bytes))
}

/// Store/外部注册切换为 BMCBL DevelopmentMode 前的强制安全门。
/// 无有效数据时返回 Ok(None)；有数据时只有完成并校验外部备份才允许继续卸载。
pub fn prepare_external_registration_backup(family_name: &str) -> Result<Option<PathBuf>, String> {
    let Some(local_state) = local_state_for_family(family_name) else {
        return Ok(None);
    };
    let source = local_state.join("games").join("com.mojang");
    if !source.is_dir() {
        return Ok(None);
    }

    let source_before = walk_stats(&source);
    if source_before.0 == 0 {
        return Ok(None);
    }

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let epoch = timestamp.as_secs();
    let root = pending_root().join(format!(
        "{}-{}",
        timestamp.as_nanos(),
        safe_family_name(family_name)
    ));
    let partial = root.with_extension("partial");
    if partial.exists() {
        fs::remove_dir_all(&partial)
            .map_err(|e| format!("清理旧迁移临时目录失败: {e}"))?;
    }

    let backup_local_state = partial.join("LocalState");
    let backup_com_mojang = backup_local_state.join("games").join("com.mojang");
    let copied = copy_tree(&source, &backup_com_mojang)?;
    let source_after = walk_stats(&source);
    let backup_after = walk_stats(&backup_com_mojang);

    if source_before != source_after {
        return Err(format!(
            "UWP 数据在备份过程中发生变化：开始时 {} 个文件/{} 字节，结束时 {} 个文件/{} 字节；可能仍有 Minecraft 进程正在写入，已阻止卸载",
            source_before.0, source_before.1, source_after.0, source_after.1
        ));
    }
    if copied != source_after || backup_after != source_after {
        return Err(format!(
            "UWP 数据备份校验失败：源 {} 个文件/{} 字节，复制统计 {} 个文件/{} 字节，备份复核 {} 个文件/{} 字节；已阻止卸载原版 Minecraft",
            source_after.0,
            source_after.1,
            copied.0,
            copied.1,
            backup_after.0,
            backup_after.1
        ));
    }

    let manifest = MigrationManifest {
        schema: 1,
        family_name: family_name.to_string(),
        created_at_epoch: epoch,
        source_local_state: local_state,
        backup_local_state: root.join("LocalState"),
        file_count: source_after.0,
        total_size: source_after.1,
        restored: false,
    };
    fs::write(
        partial.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入迁移清单失败: {e}"))?;
    fs::create_dir_all(root.parent().unwrap_or_else(|| Path::new(".")))
        .map_err(|e| e.to_string())?;
    fs::rename(&partial, &root).map_err(|e| format!("提交迁移备份失败: {e}"))?;

    let marker = pending_marker_for_family(family_name);
    fs::write(&marker, root.to_string_lossy().as_bytes())
        .map_err(|e| format!("写入迁移待恢复标记失败 {}: {e}", marker.display()))?;
    Ok(Some(root))
}

fn read_pending_root_for_family(family_name: &str) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let family_marker = pending_marker_for_family(family_name);
    match fs::read_to_string(&family_marker) {
        Ok(root) => return Ok(Some((family_marker, PathBuf::from(root.trim())))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!(
                "读取 UWP 迁移待恢复标记失败 {}: {error}",
                family_marker.display()
            ));
        }
    }

    // 兼容此前版本创建的全局 pending.txt。只有 manifest 的包家族匹配时才会消费它。
    let legacy_marker = legacy_pending_marker();
    match fs::read_to_string(&legacy_marker) {
        Ok(root) => Ok(Some((legacy_marker, PathBuf::from(root.trim())))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "读取旧版 UWP 迁移待恢复标记失败 {}: {error}",
            legacy_marker.display()
        )),
    }
}

/// DevelopmentMode 注册成功后恢复 Store/外部 UWP 的用户数据。
///
/// 恢复动作必须与刚注册的 Minecraft Identity 匹配。不同包家族使用独立 pending 标记，
/// Release/Preview 可各自保留待恢复迁移；同时拒绝覆盖已经出现新文件的目标 com.mojang。
pub fn restore_pending_backup_for_identity(
    identity_name: &str,
) -> Result<Option<PathBuf>, String> {
    let Some(expected_family) = package_family_for_identity(identity_name) else {
        return Ok(None);
    };

    let Some((marker, root)) = read_pending_root_for_family(expected_family)? else {
        return Ok(None);
    };
    let manifest_path = root.join("manifest.json");
    let mut manifest: MigrationManifest = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|e| format!("读取迁移清单失败: {e}"))?,
    )
    .map_err(|e| format!("解析迁移清单失败: {e}"))?;

    if manifest.family_name != expected_family {
        if marker == legacy_pending_marker() {
            tracing::info!(
                identity_name,
                expected_family,
                pending_family = %manifest.family_name,
                "旧版全局待恢复 UWP 数据与本次注册包家族不匹配，保留迁移备份等待正确版本注册"
            );
            return Ok(None);
        }
        return Err(format!(
            "UWP 迁移标记与清单包家族不一致：期望 {expected_family}，实际 {}",
            manifest.family_name
        ));
    }

    let Some(target_local_state) = local_state_for_family(&manifest.family_name) else {
        return Err("无法定位新注册 UWP 的 LocalState".to_string());
    };
    let source = manifest
        .backup_local_state
        .join("games")
        .join("com.mojang");
    let source_stats = walk_stats(&source);
    if source_stats != (manifest.file_count, manifest.total_size) {
        return Err(format!(
            "UWP 迁移备份在恢复前校验失败：清单 {} 个文件/{} 字节，当前备份 {} 个文件/{} 字节",
            manifest.file_count, manifest.total_size, source_stats.0, source_stats.1
        ));
    }

    let target = target_local_state.join("games").join("com.mojang");
    if target.is_dir() {
        let target_stats = walk_stats(&target);
        if target_stats.0 > 0 {
            return Err(format!(
                "新注册 UWP 的目标数据目录已存在 {} 个文件/{} 字节，为避免覆盖新生成或用户现有的数据，已拒绝自动恢复；迁移备份仍保留在 {}",
                target_stats.0,
                target_stats.1,
                root.display()
            ));
        }
    }

    let restored = copy_tree(&source, &target)?;
    let target_after = walk_stats(&target);
    let expected = (manifest.file_count, manifest.total_size);
    if restored != expected || target_after != expected {
        return Err(format!(
            "UWP 数据恢复校验失败：期望 {} 个文件/{} 字节，复制统计 {} 个文件/{} 字节，目标复核 {} 个文件/{} 字节",
            expected.0,
            expected.1,
            restored.0,
            restored.1,
            target_after.0,
            target_after.1
        ));
    }

    manifest.restored = true;
    fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("更新迁移清单失败: {e}"))?;
    fs::remove_file(&marker)
        .map_err(|e| format!("清理迁移待恢复标记失败 {}: {e}", marker.display()))?;
    Ok(Some(root))
}
