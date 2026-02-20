// src/commands/mc_dll_mods.rs

use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
};
use tauri::command;
use tokio::{fs, io::AsyncWriteExt};
use tracing::{warn, info}; // 引入 info 用于常规日志
use crate::utils::file_ops;

// 与 preloader.dll 保持一致的 Manifest 结构
#[derive(Serialize, Deserialize, Debug, Clone)]
struct ModManifest {
    name: String,
    entry: String,
    #[serde(rename = "type")]
    mod_type: String,
    /// Only meaningful for `type = "hot-inject"`; handled by `BLoader.dll`.
    #[serde(default)]
    inject_delay_ms: Option<u64>,
}

#[derive(Serialize, Debug)]
struct ModItem {
    id: String,       // 文件夹名称
    name: String,     // Mod 名称 (来自 manifest)
    enabled: bool,
    path: String,
    mod_type: String,
    inject_delay_ms: u64,
    description: Option<String>,
}

// ---------------- Helper functions ----------------

fn versions_base() -> PathBuf {
    file_ops::bmcbl_subdir("versions")
}

fn mods_dir_for(folder_name: &str) -> PathBuf {
    versions_base().join(folder_name).join("mods")
}

// ---------------- Commands ----------------

#[command]
pub async fn get_mod_list(folder_name: String) -> Result<serde_json::Value, String> {
    let mods_dir = mods_dir_for(&folder_name);

    if !mods_dir.exists() {
        // 如果目录不存在，尝试创建
        if let Err(e) = fs::create_dir_all(&mods_dir).await {
            return Err(format!("无法创建 mods 目录: {}", e));
        }
        return Ok(serde_json::json!([]));
    }

    let mut result: Vec<ModItem> = Vec::new();
    let mut entries = fs::read_dir(&mods_dir).await.map_err(|e| e.to_string())?;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        // 彻底放弃旧版本 DLL 方式，只处理文件夹
        if !path.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();

        // 检查 Manifest
        let enabled_manifest = path.join("manifest.json");
        let disabled_manifest = path.join(".manifest.json");

        let (is_enabled, manifest_path) = if enabled_manifest.exists() {
            (true, enabled_manifest)
        } else if disabled_manifest.exists() {
            (false, disabled_manifest)
        } else {
            // 没有 manifest，可能是空文件夹或损坏，跳过
            continue;
        };

        // 读取 Manifest 内容
        match fs::read_to_string(&manifest_path).await {
            Ok(content) => {
                match serde_json::from_str::<ModManifest>(&content) {
                    Ok(manifest) => {
                        result.push(ModItem {
                            id: dir_name, // ID 使用文件夹名
                            name: manifest.name,
                            enabled: is_enabled,
                            path: path.join(&manifest.entry).to_string_lossy().to_string(),
                            mod_type: manifest.mod_type,
                            inject_delay_ms: manifest.inject_delay_ms.unwrap_or(0),
                            description: None,
                        });
                    },
                    Err(e) => {
                        warn!("Manifest 解析失败 {}: {}", manifest_path.display(), e);
                    }
                }
            },
            Err(e) => {
                warn!("读取 Manifest 失败 {}: {}", manifest_path.display(), e);
            }
        }
    }

    Ok(serde_json::json!(result))
}

#[command]
pub async fn set_mod(
    folder_name: String,
    mod_id: String, // 文件夹名
    enabled: bool,
    _delay: u64,    // Delay 弃用
) -> Result<String, String> {
    let mods_dir = mods_dir_for(&folder_name);
    let mod_dir = mods_dir.join(&mod_id);

    if !mod_dir.exists() {
        return Err(format!("Mod 目录不存在: {}", mod_id));
    }

    let enabled_path = mod_dir.join("manifest.json");
    let disabled_path = mod_dir.join(".manifest.json");

    if enabled {
        // --- 启用操作 ---
        if enabled_path.exists() {
            return Ok(format!("Mod {} 已经是启用状态", mod_id));
        }

        if disabled_path.exists() {
            // 重命名 .manifest.json -> manifest.json
            fs::rename(&disabled_path, &enabled_path).await
                .map_err(|e| format!("启用失败 (重命名出错): {}", e))?;
        } else {
            return Err("未找到 .manifest.json，无法启用".to_string());
        }
    } else {
        // --- 禁用操作 ---
        if disabled_path.exists() {
            // 如果 .manifest.json 已经存在，检查 manifest.json 是否也存在 (异常状态)
            if enabled_path.exists() {
                warn!("Mod {} 存在重复的 manifest 文件，正在清理 enabled 文件以强制禁用...", mod_id);
                // 删除 manifest.json，保留 .manifest.json
                fs::remove_file(&enabled_path).await
                    .map_err(|e| format!("强制禁用失败 (清理冲突文件出错): {}", e))?;
            }
            return Ok(format!("Mod {} 已经是禁用状态", mod_id));
        }

        if enabled_path.exists() {
            // 重命名 manifest.json -> .manifest.json
            fs::rename(&enabled_path, &disabled_path).await
                .map_err(|e| format!("禁用失败 (重命名出错): {}", e))?;
        } else {
            return Err("未找到 manifest.json，无法禁用".to_string());
        }
    }

    info!("Mod {} 状态更新: enabled={}", mod_id, enabled);
    Ok(format!("Mod {} 状态已更新", mod_id))
}

#[command]
pub async fn set_mod_inject_delay(
    folder_name: String,
    mod_id: String, // 文件夹名
    inject_delay_ms: u64,
) -> Result<String, String> {
    let mods_dir = mods_dir_for(&folder_name);
    let mod_dir = mods_dir.join(&mod_id);

    if !mod_dir.exists() {
        return Err(format!("Mod 目录不存在: {}", mod_id));
    }

    let enabled_path = mod_dir.join("manifest.json");
    let disabled_path = mod_dir.join(".manifest.json");
    let manifest_path = if enabled_path.exists() {
        enabled_path
    } else if disabled_path.exists() {
        disabled_path
    } else {
        return Err("未找到 manifest.json 或 .manifest.json，无法修改延迟".to_string());
    };

    let content = fs::read_to_string(&manifest_path)
        .await
        .map_err(|e| format!("读取 Manifest 失败 {}: {}", manifest_path.display(), e))?;

    let mut value: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Manifest 解析失败: {}", e))?;

    let obj = value
        .as_object_mut()
        .ok_or_else(|| "Manifest 内容不是 JSON 对象".to_string())?;

    obj.insert(
        "inject_delay_ms".to_string(),
        serde_json::Value::Number(serde_json::Number::from(inject_delay_ms)),
    );

    let new_content =
        serde_json::to_string_pretty(&value).map_err(|e| format!("序列化 Manifest 失败: {}", e))?;

    let mut f = fs::File::create(&manifest_path)
        .await
        .map_err(|e| format!("写入 Manifest 失败 {}: {}", manifest_path.display(), e))?;
    f.write_all(new_content.as_bytes())
        .await
        .map_err(|e| format!("写入 Manifest 失败 {}: {}", manifest_path.display(), e))?;

    info!(
        "Mod {} 延迟更新: inject_delay_ms={} (file={})",
        mod_id,
        inject_delay_ms,
        manifest_path.display()
    );
    Ok(format!("Mod {} inject_delay_ms 已更新", mod_id))
}

#[command]
pub async fn set_mod_type(
    folder_name: String,
    mod_id: String, // 文件夹名
    mod_type: String,
) -> Result<String, String> {
    let mods_dir = mods_dir_for(&folder_name);
    let mod_dir = mods_dir.join(&mod_id);

    if !mod_dir.exists() {
        return Err(format!("Mod 目录不存在: {}", mod_id));
    }

    let mod_type = mod_type.trim().to_string();
    if mod_type.is_empty() {
        return Err("Mod 类型不能为空".to_string());
    }

    let enabled_path = mod_dir.join("manifest.json");
    let disabled_path = mod_dir.join(".manifest.json");
    let manifest_path = if enabled_path.exists() {
        enabled_path
    } else if disabled_path.exists() {
        disabled_path
    } else {
        return Err("未找到 manifest.json 或 .manifest.json，无法修改类型".to_string());
    };

    let content = fs::read_to_string(&manifest_path)
        .await
        .map_err(|e| format!("读取 Manifest 失败 {}: {}", manifest_path.display(), e))?;

    let mut value: serde_json::Value =
        serde_json::from_str(&content).map_err(|e| format!("Manifest 解析失败: {}", e))?;

    let obj = value
        .as_object_mut()
        .ok_or_else(|| "Manifest 内容不是 JSON 对象".to_string())?;

    obj.insert(
        "type".to_string(),
        serde_json::Value::String(mod_type.clone()),
    );

    let new_content =
        serde_json::to_string_pretty(&value).map_err(|e| format!("序列化 Manifest 失败: {}", e))?;

    let mut f = fs::File::create(&manifest_path)
        .await
        .map_err(|e| format!("写入 Manifest 失败 {}: {}", manifest_path.display(), e))?;
    f.write_all(new_content.as_bytes())
        .await
        .map_err(|e| format!("写入 Manifest 失败 {}: {}", manifest_path.display(), e))?;

    info!(
        "Mod {} 类型更新: type={} (file={})",
        mod_id,
        mod_type,
        manifest_path.display()
    );
    Ok(format!("Mod {} type 已更新", mod_id))
}

#[command]
pub async fn import_mods(
    folder_name: String,
    paths: Vec<String>,
) -> Result<serde_json::Value, String> {
    let mods_dir = mods_dir_for(&folder_name);
    if let Err(e) = fs::create_dir_all(&mods_dir).await {
        return Err(format!("无法创建 mods 目录: {}", e));
    }

    let mut imported: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for p in paths {
        let src_path = PathBuf::from(&p);
        if !src_path.exists() {
            errors.push(format!("文件不存在: {}", p));
            continue;
        }

        let file_name = match src_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => { errors.push(format!("无效路径: {}", p)); continue; }
        };
        let file_stem = src_path.file_stem().and_then(|n| n.to_str()).unwrap_or("UnknownMod").to_string();

        // 1. 创建 Mod 文件夹
        let target_dir = mods_dir.join(&file_stem);
        if !target_dir.exists() {
            if let Err(e) = fs::create_dir_all(&target_dir).await {
                errors.push(format!("创建目录失败 {}: {}", file_stem, e));
                continue;
            }
        }

        // 2. 复制 DLL
        let target_dll = target_dir.join(&file_name);
        if let Err(e) = fs::copy(&src_path, &target_dll).await {
            errors.push(format!("复制文件失败 {}: {}", file_name, e));
            continue;
        }

        // 3. 创建 manifest.json (默认启用)
        let manifest = ModManifest {
            name: file_stem.clone(),
            entry: file_name.clone(),
            mod_type: "preload-native".to_string(),
            inject_delay_ms: None,
        };

        let manifest_path = target_dir.join("manifest.json");
        // 关键逻辑：如果已禁用 (.manifest.json 存在)，则不覆盖创建，保持禁用状态
        if !target_dir.join(".manifest.json").exists() {
            match serde_json::to_string_pretty(&manifest) {
                Ok(json) => {
                    // 如果 manifest.json 已存在，将被覆盖
                    if let Err(e) = fs::write(&manifest_path, json).await {
                        errors.push(format!("写入配置失败 {}: {}", file_stem, e));
                    } else {
                        imported.push(file_stem);
                    }
                },
                Err(e) => errors.push(format!("序列化配置失败: {}", e)),
            }
        } else {
            imported.push(format!("{} (保持禁用)", file_stem));
        }
    }

    Ok(serde_json::json!({ "imported": imported, "errors": errors }))
}

#[command]
pub async fn delete_mods(folder_name: String, mod_ids: Vec<String>) -> Result<String, String> {
    let mods_dir = mods_dir_for(&folder_name);
    let mut deleted_count = 0;
    let mut errors = Vec::new();

    for id in mod_ids {
        let target_dir = mods_dir.join(&id);
        if target_dir.exists() && target_dir.is_dir() {
            if let Err(e) = fs::remove_dir_all(&target_dir).await {
                errors.push(format!("删除 {} 失败: {}", id, e));
            } else {
                deleted_count += 1;
            }
        } else {
            // 已移除对旧版本 DLL 文件的删除支持
            errors.push(format!("未找到 Mod 目录: {}", id));
        }
    }

    if !errors.is_empty() {
        Err(errors.join("; "))
    } else {
        Ok(format!("成功删除 {} 个 Mod", deleted_count))
    }
}
