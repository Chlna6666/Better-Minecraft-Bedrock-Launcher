// src-tauri/src/commands/gdk_users.rs
use serde::Serialize;
use std::{fs, path::PathBuf};

use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize)]
pub struct GdkUser {
    pub path: String,
    pub edition: String,
    pub edition_label: String,
    pub user_folder: String,
    pub user_id: Option<u64>,
}

#[tauri::command]
pub fn get_gdk_users(edition: String) -> Result<Vec<GdkUser>, String> {
    let mut res: Vec<GdkUser> = Vec::new();

    debug!("get_gdk_users called (edition='{}')", edition);

    let roaming_os = std::env::var_os("APPDATA");
    let roaming = match roaming_os {
        Some(v) => {
            let p = PathBuf::from(v);
            debug!("APPDATA (Roaming) detected: {}", p.to_string_lossy());
            p
        }
        None => {
            warn!("APPDATA environment variable not found; returning empty result");
            return Ok(res);
        }
    };

    let edition_lower = edition.to_lowercase();
    let candidate_dir_name = if edition_lower.contains("preview") || edition_lower.contains("beta") {
        "Minecraft Bedrock Preview"
    } else {
        "Minecraft Bedrock"
    };
    let edition_label = if candidate_dir_name == "Minecraft Bedrock Preview" {
        "预览版"
    } else {
        "正式版"
    };

    let users_dir = roaming.join(candidate_dir_name).join("Users");

    if !users_dir.exists() || !users_dir.is_dir() {
        return Ok(res);
    }

    let rd = match fs::read_dir(&users_dir) {
        Ok(it) => it,
        Err(e) => {
            error!("Failed to read Users dir: {}", e);
            return Ok(res);
        }
    };

    for entry_res in rd {
        let entry = match entry_res {
            Ok(e) => e,
            Err(_) => continue,
        };

        // 快速判断是否目录
        if let Ok(ft) = entry.file_type() {
            if !ft.is_dir() { continue; }
        } else {
            continue;
        }

        let user_folder_name_os = entry.file_name();
        let user_folder_name = user_folder_name_os.to_string_lossy().into_owned();
        let name_lower = user_folder_name.to_lowercase();

        // [修改] 仅跳过不相关的 Public 文件夹，允许 Shared
        // GDK 的公共存档（如导入的资源包/世界）通常在 Shared 目录下
        if name_lower == "public" {
            continue;
        }

        let user_path = entry.path();
        let path_str = user_path.to_string_lossy().into_owned();

        // 解析数字 ID (Shared 文件夹这里会解析失败返回 None，这是符合预期的)
        let parsed_id = user_folder_name.parse::<u64>().ok();

        res.push(GdkUser {
            path: path_str,
            edition: candidate_dir_name.to_string(),
            edition_label: edition_label.to_string(),
            user_folder: user_folder_name,
            user_id: parsed_id,
        });
    }

    Ok(res)
}