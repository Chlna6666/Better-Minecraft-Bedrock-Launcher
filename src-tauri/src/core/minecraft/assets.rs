// src-tauri/src/commands/assets.rs
use serde::Deserialize;
use serde_json::json;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::command;

#[derive(Debug, Deserialize)]
pub struct DeleteAssetPayload {
    pub version_type: String,    // "gdk" or "uwp"
    pub user_id: Option<String>, // gdk 时可传 user_id 或 user_folder（视前端）
    pub folder: Option<String>,  // 版本文件夹名（若适用）
    pub edition: Option<String>, // "release" 或 "preview"（可选）
    pub delete_type: String,     // maps|mapTemplates|resourcePacks|behaviorPacks|skins|mods
    pub name: String,            // 要删除的文件/文件夹名（禁止路径穿越）
}

fn sanitize_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("empty name".into());
    }
    // 基本拒绝路径穿越或包含绝对/相对路径分隔符
    if name.contains("..") || name.contains('/') || name.contains('\\') {
        return Err("invalid name (path traversal detected)".into());
    }
    Ok(())
}

fn map_delete_type_to_dir(delete_type: &str) -> Option<&'static str> {
    match delete_type {
        "maps" => Some("minecraftWorlds"),
        "mapTemplates" => Some("world_templates"),
        "skins" => Some("skin_packs"),
        "behaviorPacks" => Some("behavior_packs"),
        "resourcePacks" => Some("resource_packs"),
        "mods" => None, // mods handled specially / optional
        _ => None,
    }
}

#[command]
pub fn delete_game_asset(payload: DeleteAssetPayload) -> Result<serde_json::Value, String> {
    // sanitize name first
    sanitize_name(&payload.name)?;

    let delete_type = payload.delete_type.as_str();

    // mods deletion not implemented for now (per your note)
    if delete_type == "mods" {
        return Ok(json!({
            "success": false,
            "message": "mods deletion not implemented"
        }));
    }

    let dir_name =
        map_delete_type_to_dir(delete_type).ok_or_else(|| "unsupported delete_type".to_string())?;

    let version_type = payload.version_type.to_lowercase();
    let edition = payload.edition.as_deref().unwrap_or("release");
    let edition_folder_name = match edition {
        "preview" => "Minecraft Bedrock Preview",
        _ => "Minecraft Bedrock",
    };

    // function to delete a directory if exists and is inside parent
    let try_delete = |target: PathBuf| -> Result<serde_json::Value, String> {
        if !target.exists() {
            return Ok(
                json!({ "success": false, "message": format!("not found: {}", target.display()) }),
            );
        }
        if !target.is_dir() {
            return Ok(
                json!({ "success": false, "message": format!("not a directory: {}", target.display()) }),
            );
        }
        fs::remove_dir_all(&target).map_err(|e| format!("remove_dir_all failed: {}", e))?;
        Ok(json!({ "success": true }))
    };

    if version_type == "gdk" {
        // Prefer versions_root if present
        if let Some(folder) = payload.folder.clone() {
            let versions_root = Path::new("./BMCBL/versions").join(&folder);
            if versions_root.exists() {
                // versions_root + folder + edition_folder_name + \Users\ + user_id_or_folder + \games\com.mojang\<dir_name>\<name>
                let user_id_val = payload.user_id.clone().unwrap_or_default();
                if user_id_val.is_empty() {
                    // fallback to APPDATA
                } else {
                    let candidate = versions_root
                        .join(edition_folder_name)
                        .join("Users")
                        .join(&user_id_val)
                        .join("games")
                        .join("com.mojang")
                        .join(dir_name)
                        .join(&payload.name);

                    if candidate.exists() {
                        return try_delete(candidate);
                    }
                }
                // If not found under versions_root with user folder, also check Shared (for packs)
                let shared_candidate = versions_root
                    .join(edition_folder_name)
                    .join("Users")
                    .join("Shared")
                    .join("games")
                    .join("com.mojang")
                    .join(dir_name)
                    .join(&payload.name);
                if shared_candidate.exists() {
                    return try_delete(shared_candidate);
                }
            }
        }

        // fallback: use APPDATA path like normal installations
        let appdata = env::var("APPDATA").map_err(|e| format!("APPDATA missing: {}", e))?;
        let base = PathBuf::from(appdata);
        // for GDK fallback assume edition_folder_name under base
        // try per-user location first
        if let Some(user_id_val) = payload.user_id.clone() {
            if !user_id_val.is_empty() {
                let candidate = base
                    .join("..") // not necessary, but keep base rooted to roaming; we'll prefer Local/ Roaming typical structure
                    .join("Roaming") // some environments; but safest is to try known path
                    .join(edition_folder_name) // NOTE: this may not exist; we'll construct typical path below
                    .join("Users")
                    .join(&user_id_val)
                    .join("games")
                    .join("com.mojang")
                    .join(dir_name)
                    .join(&payload.name);

                // Candidate may not exist; don't fail here — fall through to more generic path
                if candidate.exists() {
                    return try_delete(candidate);
                }
            }
        }

        // Generic attempt: APPDATA/games/com.mojang/<dir_name>/<name> (common for some installs)
        let generic = base
            .join("games")
            .join("com.mojang")
            .join(dir_name)
            .join(&payload.name);
        if generic.exists() {
            return try_delete(generic);
        }

        // If reached here, not found
        return Ok(json!({ "success": false, "message": "target not found (gdk fallback)" }));
    } else if version_type == "uwp" {
        // UWP path example:
        // LocalAppData/Packages/Microsoft.MinecraftUWP_8wekyb3d8bbwe/LocalState/games/com.mojang
        let local_appdata =
            env::var("LOCALAPPDATA").map_err(|e| format!("LOCALAPPDATA missing: {}", e))?;
        let mut base = PathBuf::from(&local_appdata)
            .join("Packages")
            .join("Microsoft.MinecraftUWP_8wekyb3d8bbwe")
            .join("LocalState");
        // if preview edition, use preview package id
        if edition == "preview" {
            base = PathBuf::from(&local_appdata)
                .join("Packages")
                .join("Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe")
                .join("LocalState");
        }

        let candidate = base
            .join("games")
            .join("com.mojang")
            .join(dir_name)
            .join(&payload.name);
        if candidate.exists() {
            return try_delete(candidate);
        } else {
            return Ok(
                json!({ "success": false, "message": format!("not found: {}", candidate.display()) }),
            );
        }
    } else {
        return Err("unsupported version_type (must be 'gdk' or 'uwp')".into());
    }
}
