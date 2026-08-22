#![cfg(target_os = "windows")]

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const RELEASE_FAMILY: &str = "Microsoft.MinecraftUWP_8wekyb3d8bbwe";
const PREVIEW_FAMILY: &str = "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MinecraftDataSummary {
    pub family_name: String,
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

fn local_state_for_family(family_name: &str) -> Option<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(local).join("Packages").join(family_name).join("LocalState"))
}

fn count_directories(path: &Path) -> u64 {
    fs::read_dir(path)
        .map(|entries| entries.flatten().filter(|entry| entry.path().is_dir()).count() as u64)
        .unwrap_or(0)
}

fn walk_stats(path: &Path) -> (u64, u64) {
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else { continue };
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
    let local_state = local_state_for_family(family_name).unwrap_or_default();
    let com_mojang = local_state.join("games").join("com.mojang");
    let data_present = com_mojang.is_dir();
    let (file_count, total_size) = if data_present { walk_stats(&com_mojang) } else { (0, 0) };
    MinecraftDataSummary {
        family_name: family_name.to_string(),
        local_state,
        data_present,
        file_count,
        total_size,
        worlds: count_directories(&com_mojang.join("minecraftWorlds")),
        resource_packs: count_directories(&com_mojang.join("resource_packs")),
        behavior_packs: count_directories(&com_mojang.join("behavior_packs")),
        skin_packs: count_directories(&com_mojang.join("skin_packs")),
        screenshots: fs::read_dir(com_mojang.join("Screenshots"))
            .map(|entries| entries.flatten().filter(|entry| entry.path().is_file()).count() as u64)
            .unwrap_or(0),
    }
}

pub fn scan_onboarding_environment() -> OnboardingEnvironmentSummary {
    let versions = crate::utils::file_ops::bmcbl_subdir("versions");
    let bmcbl_versions = fs::read_dir(versions)
        .map(|entries| entries.flatten().filter(|entry| entry.path().is_dir()).count() as u64)
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

fn pending_root() -> PathBuf {
    crate::utils::file_ops::bmcbl_subdir("backups")
        .join("migrations")
        .join("uwp")
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(u64, u64), String> {
    fs::create_dir_all(destination).map_err(|e| format!("创建迁移目录失败: {e}"))?;
    let mut files = 0u64;
    let mut bytes = 0u64;
    let mut stack = vec![(source.to_path_buf(), destination.to_path_buf())];
    while let Some((src_dir, dst_dir)) = stack.pop() {
        fs::create_dir_all(&dst_dir).map_err(|e| format!("创建目录失败 {}: {e}", dst_dir.display()))?;
        for entry in fs::read_dir(&src_dir).map_err(|e| format!("读取目录失败 {}: {e}", src_dir.display()))? {
            let entry = entry.map_err(|e| e.to_string())?;
            let src = entry.path();
            let dst = dst_dir.join(entry.file_name());
            let metadata = entry.metadata().map_err(|e| e.to_string())?;
            if metadata.is_dir() {
                stack.push((src, dst));
            } else if metadata.is_file() {
                let copied = fs::copy(&src, &dst)
                    .map_err(|e| format!("复制失败 {} -> {}: {e}", src.display(), dst.display()))?;
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
    let Some(local_state) = local_state_for_family(family_name) else { return Ok(None) };
    let source = local_state.join("games").join("com.mojang");
    if !source.is_dir() {
        return Ok(None);
    }

    let (source_files, source_bytes) = walk_stats(&source);
    if source_files == 0 {
        return Ok(None);
    }

    let epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let safe_family = family_name.replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', "_");
    let root = pending_root().join(format!("{epoch}-{safe_family}"));
    let partial = root.with_extension("partial");
    if partial.exists() {
        fs::remove_dir_all(&partial).map_err(|e| format!("清理旧迁移临时目录失败: {e}"))?;
    }
    let backup_local_state = partial.join("LocalState");
    let backup_com_mojang = backup_local_state.join("games").join("com.mojang");
    let (copied_files, copied_bytes) = copy_tree(&source, &backup_com_mojang)?;
    if copied_files != source_files || copied_bytes != source_bytes {
        return Err(format!(
            "UWP 数据备份校验失败：源 {source_files} 个文件/{source_bytes} 字节，备份 {copied_files} 个文件/{copied_bytes} 字节；已阻止卸载原版 Minecraft"
        ));
    }

    let manifest = MigrationManifest {
        schema: 1,
        family_name: family_name.to_string(),
        created_at_epoch: epoch,
        source_local_state: local_state,
        backup_local_state: root.join("LocalState"),
        file_count: copied_files,
        total_size: copied_bytes,
        restored: false,
    };
    fs::write(
        partial.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("写入迁移清单失败: {e}"))?;
    fs::create_dir_all(root.parent().unwrap_or_else(|| Path::new("."))).map_err(|e| e.to_string())?;
    fs::rename(&partial, &root).map_err(|e| format!("提交迁移备份失败: {e}"))?;
    fs::write(pending_root().join("pending.txt"), root.to_string_lossy().as_bytes())
        .map_err(|e| format!("写入迁移待恢复标记失败: {e}"))?;
    Ok(Some(root))
}

/// DevelopmentMode 注册成功后恢复 Store/外部 UWP 的用户数据。
pub fn restore_pending_backup() -> Result<Option<PathBuf>, String> {
    let marker = pending_root().join("pending.txt");
    let Ok(root_text) = fs::read_to_string(&marker) else { return Ok(None) };
    let root = PathBuf::from(root_text.trim());
    let manifest_path = root.join("manifest.json");
    let mut manifest: MigrationManifest = serde_json::from_slice(
        &fs::read(&manifest_path).map_err(|e| format!("读取迁移清单失败: {e}"))?,
    )
    .map_err(|e| format!("解析迁移清单失败: {e}"))?;
    let Some(target_local_state) = local_state_for_family(&manifest.family_name) else {
        return Err("无法定位新注册 UWP 的 LocalState".to_string());
    };
    let source = manifest.backup_local_state.join("games").join("com.mojang");
    let target = target_local_state.join("games").join("com.mojang");
    let (restored_files, restored_bytes) = copy_tree(&source, &target)?;
    if restored_files != manifest.file_count || restored_bytes != manifest.total_size {
        return Err(format!(
            "UWP 数据恢复校验失败：期望 {} 个文件/{} 字节，实际 {restored_files} 个文件/{restored_bytes} 字节",
            manifest.file_count, manifest.total_size
        ));
    }
    manifest.restored = true;
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?)
        .map_err(|e| format!("更新迁移清单失败: {e}"))?;
    let _ = fs::remove_file(marker);
    Ok(Some(root))
}
