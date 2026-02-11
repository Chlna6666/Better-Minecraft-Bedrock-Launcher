use anyhow::{Context, Result};
use std::fs as stdfs;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use zip::write::SimpleFileOptions;
use crate::core::minecraft::map::McMapInfo;
use crate::core::minecraft::paths::{BuildType, Edition, GamePathOptions};
use super::nbt::{self, NbtTag, parse_root_nbt_with_header, serialize_root_nbt, read_level_dat, write_level_dat};
use crate::core::minecraft::resource_packs::{ McPackInfo};

#[tauri::command]
pub async fn read_level_dat_cmd(folder_path: String) -> Result<serde_json::Value, String> {
    let path = PathBuf::from(folder_path).join("level.dat");
    let tag = read_level_dat(&path).map_err(|e| e.to_string())?;
    serde_json::to_value(&tag).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn write_level_dat_cmd(folder_path: String, data: serde_json::Value, version: u32) -> Result<(), String> {
    if data.is_null() {
        return Err("后端拒绝写入：接收到的数据为 null".to_string());
    }

    let path = PathBuf::from(folder_path).join("level.dat");
    let tag: NbtTag = serde_json::from_value(data).map_err(|e| format!("JSON 转 NBT 失败: {}", e))?;

    if let NbtTag::Compound(map) = &tag {
        if map.is_empty() {
            println!("警告: 正在写入空的 Compound 到 level.dat");
        }
    } else {
        return Err("后端拒绝写入：根节点必须是 Compound 类型".to_string());
    }

    write_level_dat(&path, &tag, version).map_err(|e| e.to_string())?;
    Ok(())
}


/// 辅助函数：压缩文件夹
fn zip_directory(src_dir: &Path, dst_file: &Path) -> anyhow::Result<()> {
    if !src_dir.exists() {
        anyhow::bail!("源文件夹不存在");
    }

    let file = File::create(dst_file)?;
    let mut zip = zip::ZipWriter::new(file);

    // 使用 Deflated 压缩，设置权限
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    let walk = walkdir::WalkDir::new(src_dir);
    let prefix = src_dir; // 保持相对路径结构

    for entry in walk.into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();

        // 计算 zip 内的相对路径
        let name = path.strip_prefix(prefix)?.to_string_lossy().replace('\\', "/");

        if path.is_file() {
            zip.start_file(name, options)?;
            let mut f = File::open(path)?;
            let mut buffer = Vec::new();
            f.read_to_end(&mut buffer)?;
            zip.write_all(&buffer)?;
        } else if !name.is_empty() {
            zip.add_directory(name, options)?;
        }
    }
    zip.finish()?;
    Ok(())
}

/// 导出地图命令 (ZIP/McWorld)
#[tauri::command]
pub async fn export_map_cmd(folder_path: String, target_path: String) -> Result<(), String> {
    let src = PathBuf::from(folder_path);
    let dst = PathBuf::from(target_path);

    tauri::async_runtime::spawn_blocking(move || {
        zip_directory(&src, &dst)
    }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

/// 备份地图命令
#[tauri::command]
pub async fn backup_map_cmd(_app_handle: AppHandle, folder_path: String, map_name: String) -> Result<String, String> {
    let src = PathBuf::from(folder_path);
    let backup_dir = PathBuf::from("./BMCBL/backup");
    if !backup_dir.exists() {
        stdfs::create_dir_all(&backup_dir).map_err(|e| e.to_string())?;
    }
    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let safe_name: String = map_name.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let file_name = format!("{}_{}.mcworld", safe_name, timestamp);
    let target_path = backup_dir.join(&file_name);

    let target_path_clone = target_path.clone();

    tauri::async_runtime::spawn_blocking(move || {
        zip_directory(&src, &target_path_clone)
    }).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;

    let display_path = target_path.canonicalize().unwrap_or(target_path);

    Ok(display_path.to_string_lossy().to_string())
}


#[tauri::command]
pub async fn get_resource_packs(
    build_type: BuildType,
    edition: Edition,
    version_name: String,
    enable_isolation: bool,
    user_id: Option<String>,
    lang: Option<String>,
) -> Result<Vec<McPackInfo>, String> {
    let options = GamePathOptions { build_type, edition, version_name, enable_isolation, user_id, allow_shared_fallback: false };
    let lang_owned = lang.unwrap_or_else(|| "en-US".to_string());

    tokio::task::spawn_blocking(move || {
        crate::core::minecraft::resource_packs::read_packs_standard("resource_packs", &lang_owned, &options)
    })
        .await
        .map_err(|e| format!("Task error: {:?}", e))?
        .map_err(|e| format!("Error: {:?}", e))
}

#[tauri::command]
pub async fn get_behavior_packs(
    build_type: BuildType,
    edition: Edition,
    version_name: String,
    enable_isolation: bool,
    user_id: Option<String>,
    lang: Option<String>,
) -> Result<Vec<McPackInfo>, String> {
    let options = GamePathOptions { build_type, edition, version_name, enable_isolation, user_id, allow_shared_fallback: false };
    let lang_owned = lang.unwrap_or_else(|| "en-US".to_string());

    tokio::task::spawn_blocking(move || {
        crate::core::minecraft::resource_packs::read_packs_standard("behavior_packs", &lang_owned, &options)
    })
        .await
        .map_err(|e| format!("Task error: {:?}", e))?
        .map_err(|e| format!("Error: {:?}", e))
}


/// 获取 Minecraft 世界列表
///
/// - `source`: "uwp" | "gdk"
/// - `isolation_id`: 版本隔离 ID (可选，仅 GDK 有效)
/// - `gdk_user`: 指定 GDK 用户文件夹名 (可选，如 XUID 或 "Shared")。如果不传，则扫描 Users 下所有用户。
#[tauri::command]
pub async fn get_minecraft_worlds(
    build_type: BuildType,
    edition: Edition,
    version_name: String,
    enable_isolation: bool,
    user_id: Option<String>,
) -> Result<Vec<McMapInfo>, String> {
    let options = GamePathOptions { build_type, edition, version_name, enable_isolation, user_id, allow_shared_fallback: false };

    tokio::task::spawn_blocking(move || {
        crate::core::minecraft::map::list_worlds_standard(&options)
    })
        .await
        .map_err(|e| format!("Task error: {:?}", e))?
        .map_err(|e| format!("Error: {:?}", e))
}
