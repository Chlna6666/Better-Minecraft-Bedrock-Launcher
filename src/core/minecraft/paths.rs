// src-tauri/src/core/minecraft/paths.rs
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

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
    Education,        // 预留
    EducationPreview, // 预留
}

/// 标准化的请求参数结构
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct GamePathOptions {
    pub build_type: BuildType,
    pub edition: Edition,
    pub version_name: String,        // 版本文件夹名 (isolation_id)
    pub enable_isolation: bool,      // 是否开启隔离
    pub user_id: Option<String>,     // GDK 用户 ID (可选)
    pub allow_shared_fallback: bool, // 允许回退到 Shared
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameTargetDir {
    MinecraftWorlds,
    ResourcePacks,
    BehaviorPacks,
    SkinPacks,
    Screenshots,
    MinecraftPe,
}

impl GameTargetDir {
    pub const fn name(self) -> &'static str {
        match self {
            Self::MinecraftWorlds => "minecraftWorlds",
            Self::ResourcePacks => "resource_packs",
            Self::BehaviorPacks => "behavior_packs",
            Self::SkinPacks => "skin_packs",
            Self::Screenshots => "Screenshots",
            Self::MinecraftPe => "minecraftpe",
        }
    }

    pub const fn should_include_shared(self) -> bool {
        matches!(
            self,
            Self::ResourcePacks | Self::BehaviorPacks | Self::SkinPacks
        )
    }
}

pub fn com_mojang_dir(root: &Path) -> PathBuf {
    root.join("games").join("com.mojang")
}

pub fn user_com_mojang_dir(root: &Path, user_id: &str) -> PathBuf {
    root.join("Users")
        .join(user_id)
        .join("games")
        .join("com.mojang")
}

pub fn game_target_dirs(options: &GamePathOptions, target: GameTargetDir) -> Vec<PathBuf> {
    scan_game_dirs(options, target.name())
}

pub fn vanilla_resource_pack_roots(package_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_unique_path(&mut roots, package_path.to_path_buf());
    if path_file_name_eq(package_path, "textures") {
        if let Some(parent) = package_path.parent() {
            push_unique_path(&mut roots, parent.to_path_buf());
        }
    }
    push_vanilla_resource_pack_roots(
        &mut roots,
        &package_path.join("data").join("resource_packs"),
    );
    push_vanilla_resource_pack_roots(&mut roots, &package_path.join("data").join("resourcepacks"));
    roots
}

pub fn has_vanilla_resource_pack(package_path: &Path) -> bool {
    vanilla_resource_pack_roots(package_path)
        .into_iter()
        .any(|root| root.join("textures").is_dir())
}

pub fn discover_local_package_roots_with_vanilla() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    push_direct_child_package_roots_with_vanilla(
        crate::utils::file_ops::bmcbl_subdir("versions").as_path(),
        &mut roots,
    );

    if let Ok(appdata) = env::var("APPDATA") {
        let appdata = PathBuf::from(appdata);
        push_package_root_with_vanilla(&mut roots, appdata.join("Minecraft Bedrock"));
        push_package_root_with_vanilla(&mut roots, appdata.join("Minecraft Bedrock Preview"));
        push_direct_child_package_roots_with_vanilla(
            appdata.join("LeviLauncher.exe").join("versions").as_path(),
            &mut roots,
        );
    }

    if let Ok(local_appdata) = env::var("LOCALAPPDATA") {
        let packages = PathBuf::from(local_appdata).join("Packages");
        for package_name in [
            "Microsoft.MinecraftUWP_8wekyb3d8bbwe",
            "Microsoft.MinecraftWindowsBeta_8wekyb3d8bbwe",
            "Microsoft.MinecraftEducationEdition_8wekyb3d8bbwe",
            "Microsoft.MinecraftEducationEditionBeta_8wekyb3d8bbwe",
        ] {
            push_package_root_with_vanilla(&mut roots, packages.join(package_name));
            push_package_root_with_vanilla(
                &mut roots,
                packages.join(package_name).join("LocalState"),
            );
        }
    }

    roots
}

fn push_package_root_with_vanilla(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if has_vanilla_resource_pack(&path) {
        push_unique_path(paths, path);
    }
}

fn push_direct_child_package_roots_with_vanilla(parent: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            push_package_root_with_vanilla(paths, path.clone());
            push_package_root_with_vanilla(paths, path.join("Minecraft Bedrock"));
        }
    }
}

fn push_vanilla_resource_pack_roots(roots: &mut Vec<PathBuf>, resource_packs_dir: &Path) {
    let vanilla = resource_packs_dir.join("vanilla");
    push_unique_path(roots, vanilla.join("client"));
    push_unique_path(roots, vanilla);

    let Ok(entries) = fs::read_dir(resource_packs_dir) else {
        return;
    };
    let mut vanilla_overlay_roots = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_vanilla_resource_pack_overlay_name)
        })
        .collect::<Vec<_>>();
    vanilla_overlay_roots.sort_by(|left, right| {
        let left_version = left
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(vanilla_resource_pack_overlay_version);
        let right_version = right
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(vanilla_resource_pack_overlay_version);
        right_version
            .cmp(&left_version)
            .then_with(|| left.cmp(right))
    });

    for root in vanilla_overlay_roots {
        push_unique_path(roots, root.join("client"));
        push_unique_path(roots, root);
    }
}

fn is_vanilla_resource_pack_overlay_name(name: &str) -> bool {
    vanilla_resource_pack_overlay_version(name).is_some()
}

fn vanilla_resource_pack_overlay_version(name: &str) -> Option<Vec<u32>> {
    let version = name.strip_prefix("vanilla_").or_else(|| {
        name.get(..8)
            .filter(|prefix| prefix.eq_ignore_ascii_case("vanilla_"))
            .and_then(|_| name.get(8..))
    })?;
    let mut parts = Vec::new();
    for part in version.split('.') {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        parts.push(part.parse().ok()?);
    }
    (!parts.is_empty()).then_some(parts)
}

pub fn infer_package_roots_from_world_path(world_path: &Path) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for ancestor in world_path.ancestors() {
        if !path_file_name_eq(ancestor, "minecraftWorlds") {
            continue;
        }
        let Some(com_mojang_dir) = ancestor.parent() else {
            continue;
        };
        if !path_file_name_eq(com_mojang_dir, "com.mojang") {
            continue;
        }
        let Some(games_dir) = com_mojang_dir.parent() else {
            continue;
        };
        if !path_file_name_eq(games_dir, "games") {
            continue;
        }
        if let Some(root) = games_dir.parent() {
            push_unique_path(&mut roots, root.to_path_buf());
            if let Some(users_dir) = root
                .parent()
                .filter(|path| path_file_name_eq(path, "Users"))
            {
                if let Some(package_root) = users_dir.parent() {
                    push_unique_path(&mut roots, package_root.to_path_buf());
                }
            }
        }
    }
    roots
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.iter().any(|existing| existing == &path) {
        paths.push(path);
    }
}

fn path_file_name_eq(path: &Path, expected: &str) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(expected))
}

pub fn resolve_game_target_parent(
    options: &GamePathOptions,
    target: GameTargetDir,
    is_shared_preferred: bool,
) -> Option<PathBuf> {
    resolve_target_parent(options, target.name(), is_shared_preferred)
}

fn read_version_redirection_enabled(version_name: &str) -> Option<bool> {
    let config_path = Path::new("./BMCBL/versions")
        .join(version_name)
        .join("config.json");
    let content = fs::read_to_string(config_path).ok()?;
    let value = serde_json::from_str::<Value>(&content).ok()?;
    value.get("enable_redirection").and_then(Value::as_bool)
}

pub fn normalize_game_path_options(options: &GamePathOptions) -> GamePathOptions {
    let mut normalized = options.clone();
    if let Some(enable_redirection) = read_version_redirection_enabled(&options.version_name) {
        normalized.enable_isolation = enable_redirection;
    }
    normalized
}

/// 获取游戏的根目录
/// UWP: .../LocalState
/// GDK: .../Users/<uid> (如果不传uid，返回上一级 Users)
/// Isolation: ./BMCBL/versions/<ver>/Minecraft Bedrock/...
pub fn get_game_root(options: &GamePathOptions) -> Option<PathBuf> {
    let options = normalize_game_path_options(options);
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
                Edition::EducationPreview => {
                    "Microsoft.MinecraftEducationEditionBeta_8wekyb3d8bbwe"
                }
            };
            Some(base.join(package_name).join("LocalState"))
        }
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
    let options = normalize_game_path_options(options);
    let root = match get_game_root(&options) {
        Some(p) => p,
        None => return vec![],
    };

    let mut paths = Vec::new();

    match options.build_type {
        BuildType::Uwp => {
            // UWP 结构 (扁平): root/games/com.mojang/<target>
            let p = com_mojang_dir(&root).join(target_dir_name);
            if p.exists() && p.is_dir() {
                paths.push(p);
            }
        }
        BuildType::Gdk => {
            // GDK 结构 (用户层级): root/Users/<id>/games/com.mojang/<target>
            // 或者是 root/Users/Shared/...
            let users_base = root.join("Users");

            if !users_base.exists() {
                return vec![];
            }

            // 1. 如果指定了 user_id，优先检查该用户
            if let Some(uid) = &options.user_id {
                let p = users_base
                    .join(uid)
                    .join("games")
                    .join("com.mojang")
                    .join(target_dir_name);
                if p.exists() {
                    paths.push(p);
                }

                // 如果是查找资源包，通常也要包含 Shared
                if matches!(
                    target_dir_name,
                    "resource_packs" | "behavior_packs" | "skin_packs"
                ) {
                    let shared = user_com_mojang_dir(&root, "Shared").join(target_dir_name);
                    if shared.exists() {
                        paths.push(shared);
                    }
                }
            } else {
                // 2. 如果没指定 user_id，扫描所有用户 (包括 Shared)
                if let Ok(entries) = std::fs::read_dir(&users_base) {
                    for entry in entries.flatten() {
                        let p = entry
                            .path()
                            .join("games")
                            .join("com.mojang")
                            .join(target_dir_name);
                        if p.exists() {
                            paths.push(p);
                        }
                    }
                }
            }
        }
    }
    paths
}

/// 获取单个目标的父目录 (用于删除/写入操作)
/// 返回: (父目录路径, 是否是 GDK Shared 目录)
pub fn resolve_target_parent(
    options: &GamePathOptions,
    target_dir_name: &str,
    is_shared_preferred: bool,
) -> Option<PathBuf> {
    let options = normalize_game_path_options(options);
    let root = get_game_root(&options)?;

    match options.build_type {
        BuildType::Uwp => Some(com_mojang_dir(&root).join(target_dir_name)),
        BuildType::Gdk => {
            let users_base = root.join("Users");

            // 优先检查 Shared
            if is_shared_preferred {
                let shared = users_base
                    .join("Shared")
                    .join("games")
                    .join("com.mojang")
                    .join(target_dir_name);
                // 如果 Shared 存在或我们打算往里写，返回它
                // 这里简单判断，如果是删除操作，外面会检查 exists
                return Some(shared);
            }
            if options.allow_shared_fallback {
                return Some(
                    users_base
                        .join("Shared")
                        .join("games")
                        .join("com.mojang")
                        .join(target_dir_name),
                );
            }

            // 指定用户
            if let Some(uid) = &options.user_id {
                return Some(
                    users_base
                        .join(uid)
                        .join("games")
                        .join("com.mojang")
                        .join(target_dir_name),
                );
            }

            // 没指定用户时，回退到第一个非 Shared 的用户目录（避免导入失败）
            if let Ok(entries) = std::fs::read_dir(&users_base) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        if name != "Shared" {
                            return Some(
                                path.join("games").join("com.mojang").join(target_dir_name),
                            );
                        }
                    }
                }
            }

            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_target_dir_names_match_bedrock_folder_names() {
        assert_eq!(GameTargetDir::MinecraftWorlds.name(), "minecraftWorlds");
        assert_eq!(GameTargetDir::ResourcePacks.name(), "resource_packs");
        assert_eq!(GameTargetDir::BehaviorPacks.name(), "behavior_packs");
        assert_eq!(GameTargetDir::SkinPacks.name(), "skin_packs");
        assert_eq!(GameTargetDir::Screenshots.name(), "Screenshots");
        assert_eq!(GameTargetDir::MinecraftPe.name(), "minecraftpe");
    }

    #[test]
    fn common_game_path_helpers_build_canonical_com_mojang_paths() {
        let root = PathBuf::from(r"C:\Minecraft Bedrock");

        assert_eq!(
            com_mojang_dir(&root),
            PathBuf::from(r"C:\Minecraft Bedrock")
                .join("games")
                .join("com.mojang")
        );
        assert_eq!(
            user_com_mojang_dir(&root, "4173542688423936997"),
            PathBuf::from(r"C:\Minecraft Bedrock")
                .join("Users")
                .join("4173542688423936997")
                .join("games")
                .join("com.mojang")
        );
    }

    #[test]
    fn vanilla_resource_pack_roots_include_versioned_overlays_after_base_vanilla() {
        let root = unique_temp_dir("bmcbl-paths-vanilla-roots");
        let resource_packs = root.join("data").join("resource_packs");
        std::fs::create_dir_all(resource_packs.join("vanilla").join("textures"))
            .unwrap_or_else(|error| panic!("create vanilla pack: {error}"));
        std::fs::create_dir_all(resource_packs.join("vanilla_1.17.0").join("textures"))
            .unwrap_or_else(|error| panic!("create older vanilla overlay pack: {error}"));
        std::fs::create_dir_all(resource_packs.join("vanilla_1.21.90").join("textures"))
            .unwrap_or_else(|error| panic!("create newer vanilla overlay pack: {error}"));
        std::fs::create_dir_all(resource_packs.join("vanilla_music").join("textures"))
            .unwrap_or_else(|error| panic!("create vanilla music pack: {error}"));

        let roots = vanilla_resource_pack_roots(&root);

        let base_index = roots
            .iter()
            .position(|path| path == &resource_packs.join("vanilla"))
            .unwrap_or_else(|| panic!("missing base vanilla root"));
        let overlay_index = roots
            .iter()
            .position(|path| path == &resource_packs.join("vanilla_1.17.0"))
            .unwrap_or_else(|| panic!("missing versioned vanilla overlay root"));
        let newer_overlay_index = roots
            .iter()
            .position(|path| path == &resource_packs.join("vanilla_1.21.90"))
            .unwrap_or_else(|| panic!("missing newer versioned vanilla overlay root"));
        assert!(base_index < overlay_index);
        assert!(base_index < newer_overlay_index);
        assert!(newer_overlay_index < overlay_index);
        assert!(
            !roots
                .iter()
                .any(|path| path == &resource_packs.join("vanilla_music"))
        );

        if let Err(error) = std::fs::remove_dir_all(&root) {
            eprintln!("cleanup test package root {}: {error}", root.display());
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|error| panic!("system clock before unix epoch: {error}"))
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nonce}"))
    }
}
