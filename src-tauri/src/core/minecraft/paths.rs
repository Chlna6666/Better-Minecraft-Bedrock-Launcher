// src-tauri/src/core/minecraft/paths.rs
use std::path::{Path, PathBuf};
use std::env;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")] // 前端传 'uwp' 或 'gdk'
pub enum BuildType {
    Uwp,
    Gdk,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "snake_case")] // 前端传 'release', 'preview' 等
pub enum Edition {
    Release,
    Preview,
    Education, // 预留
    EducationPreview, // 预留
}

/// 标准化的请求参数结构
#[derive(Debug, Deserialize, Serialize)]
pub struct GamePathOptions {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,      // 版本文件夹名 (isolation_id)
    pub enable_isolation: bool,    // 是否开启隔离
    pub user_id: Option<String>,   // GDK 用户 ID (可选)
    pub allow_shared_fallback: bool, // 允许回退到 Shared
}

/// 获取游戏的根目录
/// UWP: .../LocalState
/// GDK: .../Users/<uid> (如果不传uid，返回上一级 Users)
/// Isolation: ./BMCBL/versions/<ver>/Minecraft Bedrock/...
pub fn get_game_root(options: &GamePathOptions) -> Option<PathBuf> {
    if options.enable_isolation {
        // === 隔离模式 ===
        // 路径: ./BMCBL/versions/<version_name>/Minecraft Bedrock
        let root = Path::new("./BMCBL/versions")
            .join(&options.version_name)
            .join("Minecraft Bedrock");
        return Some(root);
    }

    // === 系统模式 ===
    match options.build_type {
        BuildType::Uwp => {
            let local_appdata = env::var("LOCALAPPDATA").ok()?;
            let base = PathBuf::from(local_appdata).join("Packages");
            let package_name = match options.edition {
                Edition::Release => "Microsoft.MinecraftUWP_8wekyb3d8bbwe",
                Edition::Preview => "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe",
                Edition::Education => "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe",
                Edition::EducationPreview => "Microsoft.MinecraftEducationEditionBeta_8wekyb3d8bbwe",
            };
            Some(base.join(package_name).join("LocalState"))
        },
        BuildType::Gdk => {
            let appdata = env::var("APPDATA").ok()?;
            let base = PathBuf::from(appdata);
            let folder = match options.edition {
                Edition::Release => "Minecraft Bedrock",
                Edition::Preview => "Minecraft Bedrock Preview",
                Edition::Education => "Minecraft Education Edition",
                Edition::EducationPreview => "Minecraft Education Edition Preview",
            };
            Some(base.join(folder))
        }
    }
}

/// 扫描目标的具体路径 (返回列表，用于读取)
/// resource_sub_path 例如: "games/com.mojang/minecraftWorlds" (注意不要带 games/com.mojang 前缀，函数内部处理结构差异)
/// 实际上我们传入 "minecraftWorlds", "resource_packs" 这种顶层名称
pub fn scan_game_dirs(options: &GamePathOptions, target_dir_name: &str) -> Vec<PathBuf> {
    let root = match get_game_root(options) {
        Some(p) => p,
        None => return vec![],
    };

    let mut paths = Vec::new();

    match options.build_type {
        BuildType::Uwp => {
            // UWP 结构 (扁平): root/games/com.mojang/<target>
            let p = root.join("games").join("com.mojang").join(target_dir_name);
            if p.exists() && p.is_dir() {
                paths.push(p);
            }
        },
        BuildType::Gdk => {
            // GDK 结构 (用户层级): root/Users/<id>/games/com.mojang/<target>
            // 或者是 root/Users/Shared/...
            let users_base = root.join("Users");

            if !users_base.exists() { return vec![]; }

            // 1. 如果指定了 user_id，优先检查该用户
            if let Some(uid) = &options.user_id {
                let p = users_base.join(uid).join("games").join("com.mojang").join(target_dir_name);
                if p.exists() { paths.push(p); }

                // 如果是查找资源包，通常也要包含 Shared
                if target_dir_name.contains("resource_packs") || target_dir_name.contains("behavior_packs") {
                    let shared = users_base.join("Shared").join("games").join("com.mojang").join(target_dir_name);
                    if shared.exists() { paths.push(shared); }
                }
            } else {
                // 2. 如果没指定 user_id，扫描所有用户 (包括 Shared)
                if let Ok(entries) = std::fs::read_dir(&users_base) {
                    for entry in entries.flatten() {
                        let p = entry.path().join("games").join("com.mojang").join(target_dir_name);
                        if p.exists() { paths.push(p); }
                    }
                }
            }
        }
    }
    paths
}

/// 获取单个目标的父目录 (用于删除/写入操作)
/// 返回: (父目录路径, 是否是 GDK Shared 目录)
pub fn resolve_target_parent(options: &GamePathOptions, target_dir_name: &str, is_shared_preferred: bool) -> Option<PathBuf> {
    let root = get_game_root(options)?;

    match options.build_type {
        BuildType::Uwp => {
            Some(root.join("games").join("com.mojang").join(target_dir_name))
        },
        BuildType::Gdk => {
            let users_base = root.join("Users");

            // 优先检查 Shared
            if is_shared_preferred {
                let shared = users_base.join("Shared").join("games").join("com.mojang").join(target_dir_name);
                // 如果 Shared 存在或我们打算往里写，返回它
                // 这里简单判断，如果是删除操作，外面会检查 exists
                return Some(shared);
            }
            if options.allow_shared_fallback {
                return Some(users_base.join("Shared").join("games").join("com.mojang").join(target_dir_name));
            }

            // 指定用户
            if let Some(uid) = &options.user_id {
                return Some(users_base.join(uid).join("games").join("com.mojang").join(target_dir_name));
            }

            // 没指定用户时，回退到第一个非 Shared 的用户目录（避免导入失败）
            if let Ok(entries) = std::fs::read_dir(&users_base) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        if name != "Shared" {
                            return Some(path.join("games").join("com.mojang").join(target_dir_name));
                        }
                    }
                }
            }

            None
        }
    }
}
