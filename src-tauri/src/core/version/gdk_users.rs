// src/gdk_users.rs
use serde::Serialize;
use std::{fs, path::PathBuf};

use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize)]
pub struct GdkUser {
    /// Users\<user_folder> 的路径（例如 C:\Users\...\AppData\Roaming\Minecraft Bedrock\Users\12345）
    pub path: String,
    /// 原始目录名（例如 "Minecraft Bedrock" 或 "Minecraft Bedrock Preview"）
    pub edition: String,
    /// 可读标签（例如 "正式版" / "预览版"）
    pub edition_label: String,
    /// Users 下的用户文件夹名（例如 "User"、"12345"）
    pub user_folder: String,
    /// 若用户文件夹名是纯数字则返回 Some(id)，否则 None
    pub user_id: Option<u64>,
}

/// Tauri command: 只根据 edition 参数（"release" | "preview" | "beta" 等）
/// 扫描对应的 Minecraft Bedrock (Preview) Users 目录，返回用户列表。
///
/// 性能说明：为减少系统调用与阻塞，使用 `fs::read_dir` + `entry.file_type()`
/// 并避免对每个 entry 调用 `canonicalize` 或额外的 `metadata`。
#[tauri::command]
pub fn get_gdk_users(edition: String) -> Result<Vec<GdkUser>, String> {
    let mut res: Vec<GdkUser> = Vec::new();

    debug!("get_gdk_users called (edition='{}')", edition);

    // 只在 Windows 上读取 APPDATA（Roaming）；如果不存在，返回空数组
    let roaming_os = std::env::var_os("APPDATA");
    let roaming = match roaming_os {
        Some(v) => {
            let p = PathBuf::from(v);
            debug!("APPDATA (Roaming) detected: {}", p.to_string_lossy());
            p
        }
        None => {
            warn!("APPDATA environment variable not found; returning empty result (non-Windows?)");
            return Ok(res);
        }
    };

    // 选择目录：preview/beta -> Minecraft Bedrock Preview，否则 Minecraft Bedrock
    let edition_lower = edition.to_lowercase();
    let candidate_dir_name = if edition_lower.contains("preview") || edition_lower.contains("beta")
    {
        "Minecraft Bedrock Preview"
    } else {
        "Minecraft Bedrock"
    };
    let edition_label = if candidate_dir_name == "Minecraft Bedrock Preview" {
        "预览版"
    } else {
        "正式版"
    };

    debug!(
        "Selected edition dir: '{}' -> label '{}'",
        candidate_dir_name, edition_label
    );

    let users_dir = roaming.join(candidate_dir_name).join("Users");
    debug!("Checking users_dir: {}", users_dir.to_string_lossy());

    // 如果 Users 目录不存在或不是目录，直接返回空列表
    if !users_dir.exists() || !users_dir.is_dir() {
        debug!(
            "Users dir not found or not a dir: {}",
            users_dir.to_string_lossy()
        );
        return Ok(res);
    }

    // 读取 Users 目录
    let rd = match fs::read_dir(&users_dir) {
        Ok(it) => it,
        Err(e) => {
            error!(
                "Failed to read Users dir '{}': {}",
                users_dir.to_string_lossy(),
                e
            );
            return Ok(res);
        }
    };

    let mut checked: usize = 0;
    let mut returned: usize = 0;

    for entry_res in rd {
        let entry = match entry_res {
            Ok(e) => e,
            Err(e) => {
                debug!(
                    "Failed to read a dir entry under '{}': {}",
                    users_dir.to_string_lossy(),
                    e
                );
                continue;
            }
        };

        checked += 1;

        // 快速判断是否目录（避免额外 metadata 调用）
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(e) => {
                debug!(
                    "Failed to get file_type for entry {:?}: {}",
                    entry.path(),
                    e
                );
                continue;
            }
        };
        if !ft.is_dir() {
            debug!(
                "Skipping non-dir entry under Users: {}",
                entry.path().to_string_lossy()
            );
            continue;
        }

        // 获取文件夹名
        let user_folder_name_os = entry.file_name();
        let user_folder_name = user_folder_name_os.to_string_lossy().into_owned();
        let name_lower = user_folder_name.to_lowercase();

        // 跳过常见公共/共享文件夹
        if name_lower == "shared" || name_lower == "public" {
            debug!("Skipping shared/public folder: {}", user_folder_name);
            continue;
        }

        // 使用 entry.path()（Users\<user_folder>），不 canonicalize，减少开销
        let user_path = entry.path();
        let path_str = user_path.to_string_lossy().into_owned();

        // 尝试解析数字 ID（如果文件夹名为纯数字）
        let parsed_id = match user_folder_name.parse::<u64>() {
            Ok(v) => {
                debug!(
                    "Parsed numeric user_id {} from folder '{}'",
                    v, user_folder_name
                );
                Some(v)
            }
            Err(_) => None,
        };

        debug!(
            "Found user folder: '{}' -> path='{}'",
            user_folder_name, path_str
        );

        res.push(GdkUser {
            path: path_str,
            edition: candidate_dir_name.to_string(),
            edition_label: edition_label.to_string(),
            user_folder: user_folder_name,
            user_id: parsed_id,
        });

        returned += 1;
    }

    info!(
        "Scanned Users dir '{}' — checked: {}, returned: {} user entries",
        users_dir.to_string_lossy(),
        checked,
        returned
    );

    Ok(res)
}
