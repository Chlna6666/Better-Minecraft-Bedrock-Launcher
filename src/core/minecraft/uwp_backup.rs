#![cfg(target_os = "windows")]

use serde::Serialize;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;
use zip::write::SimpleFileOptions;

use super::uwp_migration::{MinecraftDataSummary, summarize_family};

#[derive(Clone, Debug)]
pub struct ManualUwpBackupResult {
    pub archive_path: PathBuf,
    pub summary: MinecraftDataSummary,
}

#[derive(Serialize)]
struct ManualUwpBackupManifest {
    schema: u32,
    kind: &'static str,
    family_name: String,
    created_at_epoch: u64,
    source_local_state: PathBuf,
    file_count: u64,
    total_size: u64,
    worlds: u64,
    resource_packs: u64,
    behavior_packs: u64,
    skin_packs: u64,
    screenshots: u64,
}

pub fn migration_backup_root() -> PathBuf {
    crate::utils::file_ops::bmcbl_subdir("backups")
        .join("migrations")
        .join("uwp")
}

pub fn user_data_path(summary: &MinecraftDataSummary) -> PathBuf {
    summary.local_state.join("games").join("com.mojang")
}

fn walk_stats(path: &Path) -> Result<(u64, u64), String> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in WalkDir::new(path) {
        let entry = entry.map_err(|error| format!("读取 UWP 数据失败: {error}"))?;
        if entry.file_type().is_file() {
            let metadata = entry
                .metadata()
                .map_err(|error| format!("读取文件信息失败 {}: {error}", entry.path().display()))?;
            files = files.saturating_add(1);
            bytes = bytes.saturating_add(metadata.len());
        }
    }
    Ok((files, bytes))
}

fn remove_partial(path: &Path) {
    if let Err(error) = fs::remove_file(path) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(path = %path.display(), %error, "清理 UWP 手动备份临时文件失败");
        }
    }
}

fn archive_entry_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// 将当前 Windows Minecraft UWP 的 `LocalState/games/com.mojang` 导出为用户可保存的 ZIP。
///
/// 这是独立的手动保险备份：不会创建迁移 pending 标记，也不会参与自动恢复。
/// 自动切换 Store/外部注册时仍会重新执行 `uwp_migration` 的强制备份与校验。
pub fn export_user_data_backup(
    family_name: &str,
    destination_directory: &Path,
) -> Result<ManualUwpBackupResult, String> {
    let summary = summarize_family(family_name);
    let source = user_data_path(&summary);
    if !summary.data_present || !source.is_dir() || summary.file_count == 0 {
        return Err("未检测到可导出的 Minecraft UWP 用户数据".to_string());
    }

    fs::create_dir_all(destination_directory).map_err(|error| {
        format!(
            "创建备份目标目录失败 {}: {error}",
            destination_directory.display()
        )
    })?;

    let source_before = walk_stats(&source)?;
    if source_before.0 == 0 {
        return Err("Minecraft UWP 数据目录中没有可备份文件".to_string());
    }

    let now = chrono::Local::now();
    let timestamp = now.format("%Y%m%d-%H%M%S");
    let archive_path = destination_directory.join(format!("BMCBL-UWP-Backup-{timestamp}.zip"));
    let partial_path = destination_directory.join(format!(
        ".BMCBL-UWP-Backup-{timestamp}-{}.zip.partial",
        std::process::id()
    ));
    remove_partial(&partial_path);

    let result = (|| -> Result<(), String> {
        let output = File::create(&partial_path)
            .map_err(|error| format!("创建备份文件失败 {}: {error}", partial_path.display()))?;
        let mut zip = zip::ZipWriter::new(output);
        let file_options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);
        let directory_options = SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o755);
        let archive_root = Path::new("LocalState").join("games").join("com.mojang");
        let mut buffer = vec![0u8; 128 * 1024];

        for entry in WalkDir::new(&source) {
            let entry = entry.map_err(|error| format!("遍历 UWP 数据失败: {error}"))?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&source)
                .map_err(|error| format!("生成备份相对路径失败 {}: {error}", path.display()))?;
            if relative.as_os_str().is_empty() {
                continue;
            }

            let archive_name = archive_entry_path(&archive_root.join(relative));
            if entry.file_type().is_dir() {
                zip.add_directory(format!("{archive_name}/"), directory_options)
                    .map_err(|error| format!("写入备份目录失败 {archive_name}: {error}"))?;
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }

            zip.start_file(&archive_name, file_options)
                .map_err(|error| format!("写入备份文件失败 {archive_name}: {error}"))?;
            let mut input = File::open(path)
                .map_err(|error| format!("读取 UWP 文件失败 {}: {error}", path.display()))?;
            loop {
                let read = input
                    .read(&mut buffer)
                    .map_err(|error| format!("读取 UWP 文件失败 {}: {error}", path.display()))?;
                if read == 0 {
                    break;
                }
                zip.write_all(&buffer[..read])
                    .map_err(|error| format!("写入 ZIP 失败 {archive_name}: {error}"))?;
            }
        }

        let created_at_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let manifest = ManualUwpBackupManifest {
            schema: 1,
            kind: "bmcbl-manual-uwp-user-data-backup",
            family_name: family_name.to_string(),
            created_at_epoch,
            source_local_state: summary.local_state.clone(),
            file_count: source_before.0,
            total_size: source_before.1,
            worlds: summary.worlds,
            resource_packs: summary.resource_packs,
            behavior_packs: summary.behavior_packs,
            skin_packs: summary.skin_packs,
            screenshots: summary.screenshots,
        };
        zip.start_file("bmcbl-uwp-backup.json", file_options)
            .map_err(|error| format!("写入备份清单失败: {error}"))?;
        let manifest = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| format!("生成备份清单失败: {error}"))?;
        zip.write_all(&manifest)
            .map_err(|error| format!("写入备份清单失败: {error}"))?;
        zip.finish()
            .map_err(|error| format!("完成 ZIP 备份失败: {error}"))?;

        let source_after = walk_stats(&source)?;
        if source_before != source_after {
            return Err(format!(
                "备份期间 Minecraft 数据发生变化：开始 {} 个文件/{} 字节，结束 {} 个文件/{} 字节。请先完全退出 Minecraft 后重试",
                source_before.0, source_before.1, source_after.0, source_after.1
            ));
        }

        let archive_size = fs::metadata(&partial_path)
            .map_err(|error| format!("校验备份文件失败: {error}"))?
            .len();
        if archive_size == 0 {
            return Err("备份文件为空，已取消导出".to_string());
        }

        if archive_path.exists() {
            fs::remove_file(&archive_path)
                .map_err(|error| format!("覆盖旧备份失败 {}: {error}", archive_path.display()))?;
        }
        fs::rename(&partial_path, &archive_path)
            .map_err(|error| format!("提交备份文件失败 {}: {error}", archive_path.display()))?;
        Ok(())
    })();

    if let Err(error) = result {
        remove_partial(&partial_path);
        return Err(error);
    }

    Ok(ManualUwpBackupResult {
        archive_path,
        summary,
    })
}
