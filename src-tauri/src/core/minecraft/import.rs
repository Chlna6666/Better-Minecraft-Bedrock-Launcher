// src/core/minecraft/import.rs

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{Read, Write, Seek, Cursor};
use std::path::{Path, PathBuf};
use zip::ZipArchive;
use tracing::{info, warn, debug, error};
use std::time::{SystemTime, UNIX_EPOCH};
use base64::{Engine as _, engine::general_purpose};
use rayon::prelude::*;
use walkdir::WalkDir;
use std::collections::HashMap;

use crate::core::minecraft::paths::{GamePathOptions, resolve_target_parent};
use crate::core::minecraft::nbt::{parse_root_nbt_with_header, NbtTag};

// [修改] 预览信息结构体，现在包含完整的 manifest
#[derive(Debug, Serialize, Clone)]
pub struct PackagePreview {
    pub name: String,
    pub description: String,
    pub icon: Option<String>, // Base64 data URI
    pub kind: String,         // "World", "Resource Pack", etc.
    pub version: Option<String>,
    pub size: u64,
    pub manifest: Option<PartialManifest>, // [新增]
    pub sub_packs: Option<Vec<PackagePreview>>, // [新增] 子包信息
    pub valid: bool, // [新增] 规范校验
    pub invalid_reason: Option<String>, // [新增]
}

// [新增] 导入检查结果
#[derive(Debug, Serialize)]
pub struct ImportCheckResult {
    pub has_conflict: bool,
    pub conflict_type: Option<String>, // "uuid_match"
    pub target_name: String,
    pub message: String,
    pub existing_pack_info: Option<PackagePreview>, // [新增]
}


#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub enum ImportTargetType {
    World,          // minecraftWorlds
    WorldTemplate,  // world_templates
    ResourcePack,   // resource_packs
    BehaviorPack,   // behavior_packs
    SkinPack,       // skin_packs
    Compound,       // 包含多个包的复合文件 (.mcaddon)
    Unknown,
}


impl ImportTargetType {
    pub fn to_dir_name(&self) -> &'static str {
        match self {
            ImportTargetType::World => "minecraftWorlds",
            ImportTargetType::WorldTemplate => "world_templates",
            ImportTargetType::ResourcePack => "resource_packs",
            ImportTargetType::BehaviorPack => "behavior_packs",
            ImportTargetType::SkinPack => "skin_packs",
            _ => "unknown_imported",
        }
    }
    // 用于前端显示的友好名称
    pub fn to_display_name(&self) -> &'static str {
        match self {
            ImportTargetType::World => "Import.minecraftWorlds",
            ImportTargetType::WorldTemplate => "Import.worldTemplates",
            ImportTargetType::ResourcePack => "Import.resourcePacks",
            ImportTargetType::BehaviorPack => "Import.behaviorPacks",
            ImportTargetType::SkinPack => "Import.skinPacks",
            ImportTargetType::Compound => "Import.addon",
            _ => "Import.unknown",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveScanResult {
    pub is_world: bool,
    pub is_compound: bool,
    pub has_nested_archive: bool,
    pub level_roots: Vec<String>,
    pub packs: Vec<PackEntry>,
}

#[derive(Debug, Clone)]
pub struct PackEntry {
    pub root: String,               // "" / "RP/" / "BP/"
    pub manifest_path: String,
    pub manifest: PartialManifest,
    pub pack_type: ImportTargetType,
}



// --- Manifest 结构定义 ---
#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ManifestHeader {
    pub name: Option<String>,
    pub uuid: Option<String>,
    pub description: Option<String>,
    pub version: Option<Vec<u32>>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ManifestModule {
    #[serde(rename = "type")]
    pub module_type: String,
    pub version: Option<Vec<u32>>, // [新增]
    pub uuid: Option<String>, // [新增]
    pub description: Option<String>, // [新增]
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct PartialManifest {
    pub header: Option<ManifestHeader>,
    pub modules: Option<Vec<ManifestModule>>,
}

// --- [核心功能] 检查包信息 (带 i18n 支持) ---
pub fn inspect_archive(path: &Path, preferred_lang: Option<&str>) -> Result<PackagePreview> {
    let file = File::open(path)?;
    let file_size = file.metadata()?.len();
    let mut archive = ZipArchive::new(file)?;

    // 1. 基础分析 (确定类型和默认名称)
    let (target_type, default_name, _uuid, scan) =
        analyze_archive(&mut archive, path)?;


    let mut name = default_name;
    let mut description = "".to_string();
    let mut icon = None;
    let mut version_str = None;
    let mut manifest_data = None;
    let mut sub_packs = Vec::new();
    let mut valid = true;
    let mut invalid_reason: Option<String> = None;
    let mut effective_type = target_type.clone();
    let mut template_pack: Option<PackEntry> = None;

    if target_type == ImportTargetType::Compound {
        if let Some(p) = resolve_world_template_primary(&scan) {
            effective_type = ImportTargetType::WorldTemplate;
            template_pack = Some(p);
        }
    }

    // 2. 尝试读取详细信息
    if effective_type == ImportTargetType::World {
        // --- 存档处理 ---
        // 读取 level.dat 中的名称
        if let Ok(mut level_file) = archive.by_name("level.dat") {
            let mut buf = Vec::new();
            if level_file.read_to_end(&mut buf).is_ok() {
                if let Ok(NbtTag::Compound(root)) = parse_root_nbt_with_header(&buf) {
                    if let Some(NbtTag::String(n)) = root.get("LevelName") {
                        name = n.clone();
                    }
                }
            }
        }
        // 读取 world_icon.jpeg
        if let Ok(mut icon_file) = archive.by_name("world_icon.jpeg") {
            let mut buf = Vec::new();
            if icon_file.read_to_end(&mut buf).is_ok() {
                let b64 = general_purpose::STANDARD.encode(&buf);
                icon = Some(format!("data:image/jpeg;base64,{}", b64));
            }
        }
    } else if effective_type == ImportTargetType::Compound {
        // --- 复合包处理（优先快速预览，避免全量解压）---
        if !scan.has_nested_archive {
            // 快速路径：外层 zip 已包含包目录
            let filtered_packs = filter_packs_excluding_template_internal(&scan.packs);
            let mut seen = std::collections::HashSet::new();
            for pack in &filtered_packs {
                if !seen.insert(pack.manifest_path.clone()) {
                    continue;
                }
                if let Ok(preview) = get_pack_info_from_zip(&mut archive, pack, preferred_lang) {
                    sub_packs.push(preview);
                }
            }
            let filtered_level_roots = filter_level_roots_excluding_template_internal(&scan.level_roots, &scan.packs);
            for root in &filtered_level_roots {
                if !root.is_empty() {
                    if let Ok(preview) = get_world_info_from_zip(&mut archive, root) {
                        sub_packs.push(preview);
                    }
                }
            }
        } else {
            // 激进路径：直接读取嵌套 zip 的 manifest/icon（避免落盘解压）
            if let Ok(mut fast_subs) = inspect_nested_archives_quick(&mut archive, preferred_lang) {
                if !fast_subs.is_empty() {
                    sub_packs.append(&mut fast_subs);
                }
            }

            if sub_packs.is_empty() {
                // 回退：解压外层 + 并行展开嵌套包到缓存目录
                let cache_key = compound_cache_key(path).ok();

                let (work_dir, pack_dirs) = match extract_to_cache_with_nested(&mut archive, "inspect") {
                    Ok(v) => v,
                    Err(e) => {
                        return Err(e).context("Failed to extract compound archive for inspection");
                    }
                };

                // 从“目录包”直接生成预览（不再打开子 Zip）
                for dir in pack_dirs {
                    if dir.join("manifest.json").is_file() {
                        if let Ok(preview) = get_pack_info_from_dir(&dir, &ImportTargetType::Unknown, preferred_lang) {
                            sub_packs.push(preview);
                        }
                    } else if dir.join("level.dat").is_file() {
                        if let Ok(preview) = get_world_info_from_dir(&dir) {
                            sub_packs.push(preview);
                        }
                    }
                }

                // 将缓存目录放入索引，供 import 复用（避免二次解压）
                if let Some(k) = cache_key {
                    cache_put_compound(k, work_dir);
                } else {
                    // key 生成失败，直接清理
                    let _ = fs::remove_dir_all(&work_dir);
                }
            }
        }

        // 复合包图标：优先使用子包中“第一个有图标的”，否则使用第一个子包的图标（可能为空）
        if icon.is_none() {
            if let Some(p) = sub_packs.iter().find(|p| p.icon.is_some()) {
                icon = p.icon.clone();
            } else if let Some(p) = sub_packs.first() {
                icon = p.icon.clone();
            }
        }

        if !sub_packs.is_empty() {
            let primary = sub_packs.iter().find(|p| p.kind == ImportTargetType::WorldTemplate.to_display_name())
                .or_else(|| sub_packs.first());
            if let Some(p) = primary {
                name = p.name.clone();
                description = p.description.clone();
                if icon.is_none() {
                    icon = p.icon.clone();
                }
            }
        }
        if name.is_empty() {
            name = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        }
        if description.is_empty() {
            description = format!("包含 {} 个子包", sub_packs.len());
        }
        if sub_packs.iter().any(|p| !p.valid) {
            valid = false;
            invalid_reason = Some("存在子包缺少 UUID".to_string());
        }

    } else {
        // --- 资源包/行为包处理 (支持 i18n) ---
        let mut manifest_content = None;
        let mut root_prefix = "".to_string(); // manifest 所在的目录前缀

        // A. 定位 manifest.json
        // 优先使用 scan 中的 WorldTemplate 清单（避免误选内部包）
        if effective_type == ImportTargetType::WorldTemplate {
            if let Some(pack) = template_pack.as_ref().or_else(|| scan.packs.iter().find(|p| p.pack_type == ImportTargetType::WorldTemplate)) {
                root_prefix = pack.root.clone();
                if let Some(s) = read_zip_text_by_name(&mut archive, &pack.manifest_path) {
                    manifest_content = Some(s);
                } else if let Some(s) = read_zip_text_case_insensitive(&mut archive, &pack.manifest_path) {
                    manifest_content = Some(s);
                }
            }
        }

        if manifest_content.is_none() {
            // 先尝试根目录
            if archive.by_name("manifest.json").is_ok() {
                root_prefix = "".to_string();
                if let Some(s) = read_zip_text_by_name(&mut archive, "manifest.json") {
                    manifest_content = Some(s);
                }
            } else {
                // 尝试遍历查找第一层子目录下的 manifest
                for i in 0..archive.len() {
                    // 借用检查修复：直接获取 file，不提前 drop
                    if let Ok(mut file) = archive.by_index(i) {
                        let fname = file.name().to_string();
                        if fname.ends_with("/manifest.json") || fname.ends_with("\\manifest.json") {
                            if let Some(parent) = Path::new(&fname).parent() {
                                // 统一转为 Unix 风格路径后缀
                                root_prefix = parent.to_string_lossy().replace('\\', "/") + "/";
                            }
                            let mut buf = Vec::new();
                            if file.read_to_end(&mut buf).is_ok() {
                                manifest_content = decode_text_bytes(&buf);
                            }
                            break;
                        }
                    }
                }
            }
        }

        // B. 读取语言文件 (如果找到了 manifest)
        let mut lang_map = HashMap::new();
        if manifest_content.is_some() {
            lang_map = read_lang_map_for_prefix(&mut archive, &root_prefix, preferred_lang);
        }

        // C. 解析 Manifest 并应用翻译
        if let Some(content) = manifest_content {
            let clean = strip_json_comments(&content);
            if let Ok(mut manifest) = serde_json::from_str::<PartialManifest>(&clean) {
                // 应用翻译
                if let Some(header) = manifest.header.as_mut() {
                    if let Some(n) = &header.name {
                        name = lang_map.get(n).cloned().unwrap_or_else(|| n.clone());
                        header.name = Some(name.clone());
                    }
                    if let Some(d) = &header.description {
                        description = lang_map.get(d).cloned().unwrap_or_else(|| d.clone());
                        header.description = Some(description.clone());
                    }
                    if let Some(v) = &header.version {
                        version_str = Some(v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("."));
                    }
                    if header.uuid.is_none() {
                        valid = false;
                        invalid_reason = Some("manifest.header.uuid 缺失".to_string());
                    }
                } else {
                    valid = false;
                    invalid_reason = Some("manifest.header 缺失".to_string());
                }
                manifest_data = Some(manifest);
            }
        }

        // D. 读取图标
        let icon_candidates: Vec<&str> = if effective_type == ImportTargetType::WorldTemplate {
            vec![
                "pack_icon.png",
                "pack_icon.jpg",
                "pack_icon.jpeg",
            ]
        } else {
            vec!["pack_icon.png", "pack_icon.jpg", "pack_icon.jpeg"]
        };
        let mut icon_buf = None;

        if effective_type == ImportTargetType::WorldTemplate {
            icon_buf = find_world_icon_under_root(&mut archive, &root_prefix);
            if icon_buf.is_none() {
                if let Some(pack) = template_pack.as_ref().or_else(|| scan.packs.iter().find(|p| p.pack_type == ImportTargetType::WorldTemplate)) {
                    icon_buf = find_world_icon_near_manifest(&mut archive, &pack.manifest_path);
                }
            }
        }

        for icon_name in icon_candidates {
            let target_path = format!("{}{}", root_prefix, icon_name);
            let alt_path = target_path.replace('/', "\\");

            // [修复] 同样拆分 if let ... else if let 逻辑
            {
                if let Ok(mut f) = archive.by_name(&target_path) {
                    let mut buf = Vec::new();
                    if f.read_to_end(&mut buf).is_ok() { icon_buf = Some(buf); }
                }
            }

            if icon_buf.is_some() { break; }

            {
                if let Ok(mut f) = archive.by_name(&alt_path) {
                    let mut buf = Vec::new();
                    if f.read_to_end(&mut buf).is_ok() { icon_buf = Some(buf); }
                }
            }

            if icon_buf.is_some() { break; }
        }

        if let Some(buf) = icon_buf {
            let mime = if buf.starts_with(&[0xFF, 0xD8, 0xFF]) { "image/jpeg" } else { "image/png" };
            let b64 = general_purpose::STANDARD.encode(&buf);
            icon = Some(format!("data:{};base64,{}", mime, b64));
        }
    }

    Ok(PackagePreview {
        name,
        description,
        icon,
        kind: if effective_type == ImportTargetType::Compound && !sub_packs.is_empty() {
            let primary = sub_packs.iter().find(|p| p.kind == ImportTargetType::WorldTemplate.to_display_name())
                .or_else(|| sub_packs.first());
            primary.map(|p| p.kind.clone()).unwrap_or_else(|| effective_type.to_display_name().to_string())
        } else {
            effective_type.to_display_name().to_string()
        },
        version: version_str,
        size: file_size,
        manifest: manifest_data,
        sub_packs: if sub_packs.is_empty() { None } else { Some(sub_packs) },
        valid,
        invalid_reason,
    })
}

// [新增] 检查导入冲突
pub fn check_import_file(file_path: &Path, options: &GamePathOptions) -> Result<ImportCheckResult> {
    let file = File::open(file_path)?;
    let mut archive = ZipArchive::new(file)?;
    let (target_type, internal_name, pack_uuid, scan) =
        analyze_archive(&mut archive, file_path)?;


    if target_type == ImportTargetType::Compound {
        return Ok(ImportCheckResult { has_conflict: false, conflict_type: None, target_name: internal_name, message: "Compound file".into(), existing_pack_info: None });
    }

    if let ImportTargetType::Unknown = target_type {
        return Ok(ImportCheckResult { has_conflict: false, conflict_type: None, target_name: internal_name, message: "Unknown type".into(), existing_pack_info: None });
    }

    if !matches!(target_type, ImportTargetType::World) && pack_uuid.is_none() {
        return Err(anyhow::anyhow!("manifest.header.uuid 缺失，无法导入"));
    }

    // [修改] 文件夹名称处理：优先使用 pack_<uuid>
    let base_folder_name = match &pack_uuid {
        Some(uuid) => pack_folder_name(uuid),
        None => sanitize_filename(&strip_minecraft_formatting(&internal_name)),
    };
    let target_dir_name = target_type.to_dir_name();
    let is_shared_preferred = matches!(target_type, ImportTargetType::ResourcePack | ImportTargetType::BehaviorPack);

    let parent_dir = match resolve_target_parent(options, target_dir_name, is_shared_preferred) {
        Some(p) => p,
        None => {
            if options.build_type == crate::core::minecraft::paths::BuildType::Gdk
                && !is_shared_preferred
                && !options.allow_shared_fallback
            {
                return Ok(ImportCheckResult {
                    has_conflict: true,
                    conflict_type: Some("shared_fallback".into()),
                    target_name: internal_name,
                    message: "目标目录不存在，是否导入到 Shared？".into(),
                    existing_pack_info: None,
                });
            }
            return Err(anyhow::anyhow!("Target dir not found"));
        }
    };
    if options.build_type == crate::core::minecraft::paths::BuildType::Gdk
        && !is_shared_preferred
        && !parent_dir.exists()
        && !options.allow_shared_fallback
    {
        return Ok(ImportCheckResult {
            has_conflict: true,
            conflict_type: Some("shared_fallback".into()),
            target_name: internal_name,
            message: "目标目录不存在，是否导入到 Shared？".into(),
            existing_pack_info: None,
        });
    }

    let dest_folder_name = base_folder_name.clone();
    let final_dest = parent_dir.join(&dest_folder_name);

    let mut current_dest = final_dest.clone();
    let mut current_name = dest_folder_name.clone();
    let mut counter = 1;

    loop {
        if !current_dest.exists() {
            return Ok(ImportCheckResult { has_conflict: false, conflict_type: None, target_name: current_name, message: "New import".into(), existing_pack_info: None });
        }

        if let Some(new_uuid) = &pack_uuid {
            if let Some(existing_uuid) = get_pack_uuid_from_dir(&current_dest) {
                if existing_uuid == *new_uuid {
                    let existing_pack_info = get_pack_info_from_dir(&current_dest, &target_type, None).ok();
                    let target_name = existing_pack_info.as_ref().map(|p| p.name.clone()).unwrap_or(current_name);

                    return Ok(ImportCheckResult {
                        has_conflict: true,
                        conflict_type: Some("uuid_match".into()),
                        target_name,
                        message: "Existing pack with same UUID found".into(),
                        existing_pack_info,
                    });
                }
            }
        }

        current_name = format!("{}_{}", base_folder_name, counter);
        current_dest = parent_dir.join(&current_name);
        counter += 1;

        if counter > 100 {
            return Ok(ImportCheckResult { has_conflict: false, conflict_type: None, target_name: current_name, message: "Renamed".into(), existing_pack_info: None });
        }
    }
}

// 简单的 .lang 文件解析器 (key=value)
fn parse_lang_config(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        // [关键修改] 先按 # 分割，只取前部分，用于屏蔽行尾注释 (如 "key=val #comment")
        let raw_line = if let Some((valid, _)) = line.split_once('#') {
            valid
        } else {
            line
        };

        let line_str = raw_line.trim();
        let line_str = line_str.trim_start_matches('\u{feff}');

        // 过滤无效行
        if line_str.is_empty() || line_str.starts_with("//") || line_str.starts_with('[') {
            continue;
        }

        // 解析 Key-Value
        if let Some((k, val)) = line_str.split_once('=') {
            map.insert(k.trim().to_string(), val.trim().to_string());
        } else if let Some((k, val)) = line_str.split_once(':') {
            map.insert(k.trim().to_string(), val.trim().to_string());
        } else {
            // 尝试 Tab 分割兼容
            let parts: Vec<&str> = line_str.splitn(2, '\t').collect();
            if parts.len() == 2 {
                map.insert(parts[0].trim().to_string(), parts[1].trim().to_string());
            }
        }
    }
    map
}

fn normalize_root_key(root: &str) -> String {
    let mut out = root.replace('\\', "/").to_ascii_lowercase();
    if !out.is_empty() && !out.ends_with('/') {
        out.push('/');
    }
    out
}

fn normalize_lang_code(lang: &str) -> String {
    lang.trim().replace('_', "-")
}

fn filter_level_roots_excluding_template_internal(
    level_roots: &[String],
    packs: &[PackEntry],
) -> Vec<String> {
    let template_roots: Vec<String> = packs
        .iter()
        .filter(|p| p.pack_type == ImportTargetType::WorldTemplate)
        .map(|p| normalize_root_key(&p.root))
        .collect();

    if template_roots.is_empty() {
        return level_roots.to_vec();
    }

    level_roots
        .iter()
        .filter(|root| {
            let normalized = normalize_root_key(root);
            template_roots.iter().all(|t| t != &normalized)
        })
        .cloned()
        .collect()
}

// --- 以下是原有的导入逻辑 (基本保持不变) ---

pub fn import_files_batch(
    files: Vec<String>,
    options: &GamePathOptions,
    overwrite: bool, // [新增] 覆盖选项
) -> Result<(usize, usize)> { // (success_count, fail_count)
    let mut success = 0;
    let mut fail = 0;

    debug!(
        "Import batch start: count={}, build={:?}, edition={:?}, version={}, isolation={}, user_id={:?}, overwrite={}",
        files.len(),
        options.build_type,
        options.edition,
        options.version_name,
        options.enable_isolation,
        options.user_id,
        overwrite
    );

    for file_path in files {
        let path = PathBuf::from(&file_path);
        if !path.exists() {
            warn!("Import skipped (file not found): {}", file_path);
            fail += 1;
            continue;
        }

        match process_single_archive(&path, options, overwrite) {
            Ok(_) => {
                debug!("Import success: {}", file_path);
                success += 1;
                cleanup_compound_cache_for_file(&path);
            }
            Err(e) => {
                error!("Failed to import {}: {:?}", file_path, e);
                fail += 1;
            }
        }
    }
    debug!("Import batch done: success={}, fail={}", success, fail);
    Ok((success, fail))
}

fn process_single_archive(file_path: &Path, options: &GamePathOptions, overwrite: bool) -> Result<()> {
    let file = File::open(file_path)?;
    let mut archive = ZipArchive::new(file)?;

    debug!("Import start: {:?}", file_path);

    // [修改] 递归处理：如果发现多个 manifest.json，说明是复合包，需要解压处理
    // analyze_archive 只能检测根目录或第一层，如果有多层嵌套，需要更强的检测
    let (target_type, internal_name, pack_uuid, scan) =
        analyze_archive(&mut archive, file_path)?;


    if target_type == ImportTargetType::Compound {
        info!("Detected compound archive: {:?}", file_path);
        return process_compound_archive(&mut archive, file_path, options, overwrite);
    }

    if let ImportTargetType::Unknown = target_type {
        return Err(anyhow::anyhow!("无法识别包类型: 缺少 manifest.json 或 level.dat"));
    }

    if !matches!(target_type, ImportTargetType::World) && pack_uuid.is_none() {
        return Err(anyhow::anyhow!("manifest.header.uuid 缺失，无法导入"));
    }

    // [修改] 文件夹名称处理：优先使用 pack_<uuid>
    let base_folder_name = match &pack_uuid {
        Some(uuid) => pack_folder_name(uuid),
        None => sanitize_filename(&strip_minecraft_formatting(&internal_name)),
    };
    let target_dir_name = target_type.to_dir_name();

    let is_shared_preferred = matches!(
        target_type,
        ImportTargetType::ResourcePack | ImportTargetType::BehaviorPack
    );

    let parent_dir = resolve_target_parent(options, target_dir_name, is_shared_preferred)
        .ok_or_else(|| anyhow::anyhow!("无法解析目标安装路径"))?;
    if options.build_type == crate::core::minecraft::paths::BuildType::Gdk
        && !is_shared_preferred
        && !parent_dir.exists()
        && !options.allow_shared_fallback
    {
        return Err(anyhow::anyhow!("目标目录不存在，需要确认后导入 Shared"));
    }

    let mut dest_folder_name = base_folder_name.clone();
    let mut final_dest = parent_dir.join(&dest_folder_name);
    let mut counter = 1;

    loop {
        if !final_dest.exists() { break; }

        let mut allow_overwrite = false;
        if let Some(new_uuid) = &pack_uuid {
            if let Some(existing_uuid) = get_pack_uuid_from_dir(&final_dest) {
                if existing_uuid == *new_uuid {
                    if overwrite {
                        info!("UUID match ({}), overwriting existing pack at {:?}", new_uuid, dest_folder_name);
                        allow_overwrite = true;
                    } else {
                        return Err(anyhow::anyhow!("Pack with same UUID exists (overwrite denied)"));
                    }
                }
            }
        }

        if allow_overwrite {
            break;
        } else {
            dest_folder_name = format!("{}_{}", base_folder_name, counter);
            final_dest = parent_dir.join(&dest_folder_name);
            counter += 1;
        }
    }

    debug!(
        "Import resolved: type={:?}, target_dir={}, dest={:?}",
        target_type,
        target_dir_name,
        final_dest
    );
    info!("Importing {:?} to {:?}", target_type, final_dest);
    extract_archive_parallel(file_path, &final_dest)?;

    Ok(())
}

pub fn scan_archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<ArchiveScanResult> {
    use std::collections::HashSet;

    enum ScanHit {
        LevelDat { root: String },
        Manifest(PackEntry),
    }

    let mut hits: Vec<ScanHit> = Vec::new();
    let mut has_nested_archive = false;

    for i in 0..archive.len() {
        // 顺序读取每个文件条目（避免并行 mutable borrow）
        if let Ok(mut file) = archive.by_index(i) {
            let name = file.name().to_string();
            if name.contains("__MACOSX") { continue; }

            let path = std::path::Path::new(&name);
            let root = path.parent()
                .map(|p| {
                    let mut s = p.to_string_lossy().replace('\\', "/");
                    if !s.ends_with('/') { s.push('/'); }
                    s
                })
                .unwrap_or_default();

            if path.file_name().map(|f| f == "level.dat").unwrap_or(false) {
                hits.push(ScanHit::LevelDat { root });
                continue;
            }

            if !path.file_name().map(|f| f == "manifest.json").unwrap_or(false) {
                if !file.is_dir() {
                    let ext = path
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if matches!(
                        ext.as_str(),
                        "mcpack" | "mcaddon" | "mcworld" | "mctemplate" | "zip"
                    ) {
                        has_nested_archive = true;
                    }
                }
                continue;
            }

            let mut json = String::new();
            if file.read_to_string(&mut json).is_err() { continue; }
            let clean = strip_json_comments(&json);
            if let Ok(manifest) = serde_json::from_str::<PartialManifest>(&clean) {
                let pack_type = detect_type_from_manifest(&manifest);
                hits.push(ScanHit::Manifest(PackEntry {
                    root,
                    manifest_path: name,
                    manifest,
                    pack_type,
                }));
            }
        }
    }

    let mut level_roots = HashSet::new();
    let mut packs = Vec::new();

    for h in hits {
        match h {
            ScanHit::LevelDat { root } => { level_roots.insert(root); }
            ScanHit::Manifest(p) => packs.push(p),
        }
    }

    let unique_roots: HashSet<_> = packs.iter().map(|p| p.root.clone()).collect();
    let is_compound = unique_roots.len() > 1;

    let is_world = level_roots.contains("")
        && !packs.iter().any(|p| p.root.is_empty());

    Ok(ArchiveScanResult {
        is_world,
        is_compound,
        has_nested_archive,
        level_roots: level_roots.into_iter().collect(),
        packs,
    })
}


pub fn group_packs(scan: &ArchiveScanResult)
                   -> std::collections::HashMap<ImportTargetType, Vec<PackEntry>>
{
    let mut map: std::collections::HashMap<ImportTargetType, Vec<PackEntry>> = std::collections::HashMap::new();
    for p in &scan.packs {
        map.entry(p.pack_type.clone()).or_default().push(p.clone());
    }
    map
}

fn group_compound_packs(packs: &[PackEntry]) -> HashMap<ImportTargetType, Vec<PackEntry>> {
    let mut map: HashMap<ImportTargetType, Vec<PackEntry>> = HashMap::new();
    for p in packs {
        map.entry(p.pack_type.clone())
            .or_insert_with(Vec::new)
            .push(p.clone());
    }
    map
}

pub fn split_mcaddon_groups(scan: &ArchiveScanResult) -> HashMap<ImportTargetType, Vec<PackEntry>> {
    let mut groups: HashMap<ImportTargetType, Vec<PackEntry>> = HashMap::new();

    for pack in &scan.packs {
        match pack.pack_type {
            ImportTargetType::ResourcePack
            | ImportTargetType::BehaviorPack
            | ImportTargetType::SkinPack
            | ImportTargetType::WorldTemplate => {
                groups.entry(pack.pack_type.clone())
                    .or_default()
                    .push(pack.clone());
            }
            _ => {}
        }
    }

    groups
}

pub fn analyze_archive(
    archive: &mut ZipArchive<File>,
    original_path: &Path,
) -> Result<(ImportTargetType, String, Option<String>, ArchiveScanResult)> {
    let start = std::time::Instant::now();
    let scan = scan_archive(archive)?;

    let default_name = original_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "Imported_Pack".to_string());

    let ext = original_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // mctemplate 强制按世界模板处理（优先其清单）
    if ext == "mctemplate" {
        if let Some(p) = scan
            .packs
            .iter()
            .find(|p| p.pack_type == ImportTargetType::WorldTemplate)
            .or_else(|| scan.packs.first())
        {
            let name = p
                .manifest
                .header
                .as_ref()
                .and_then(|h| h.name.clone())
                .unwrap_or(default_name.clone());
            let uuid = p
                .manifest
                .header
                .as_ref()
                .and_then(|h| h.uuid.clone());
            debug!(
                "Analyze archive: {:?}, result=WorldTemplate (by ext), packs={}, compound={}, nested={}, elapsed={} ms",
                original_path,
                scan.packs.len(),
                scan.is_compound,
                scan.has_nested_archive,
                start.elapsed().as_millis()
            );
            return Ok((
                ImportTargetType::WorldTemplate,
                name,
                uuid,
                scan,
            ));
        }
    }

    // World
    if scan.is_world {
        debug!(
            "Analyze archive: {:?}, result=World, packs={}, compound={}, nested={}, elapsed={} ms",
            original_path,
            scan.packs.len(),
            scan.is_compound,
            scan.has_nested_archive,
            start.elapsed().as_millis()
        );
        return Ok((
            ImportTargetType::World,
            default_name,
            None,
            scan,
        ));
    }

    // World template with internal packs should not be treated as compound.
    if let Some(primary) = resolve_world_template_primary(&scan) {
        let name = primary
            .manifest
            .header
            .as_ref()
            .and_then(|h| h.name.clone())
            .unwrap_or(default_name);
        let uuid = primary
            .manifest
            .header
            .as_ref()
            .and_then(|h| h.uuid.clone());
        debug!(
            "Analyze archive: {:?}, result=WorldTemplate, packs={}, compound={}, nested={}, elapsed={} ms",
            original_path,
            scan.packs.len(),
            scan.is_compound,
            scan.has_nested_archive,
            start.elapsed().as_millis()
        );
        return Ok((
            ImportTargetType::WorldTemplate,
            name,
            uuid,
            scan,
        ));
    }

    // Compound
    if scan.is_compound || scan.has_nested_archive {
        debug!(
            "Analyze archive: {:?}, result=Compound, packs={}, compound={}, nested={}, elapsed={} ms",
            original_path,
            scan.packs.len(),
            scan.is_compound,
            scan.has_nested_archive,
            start.elapsed().as_millis()
        );
        return Ok((
            ImportTargetType::Compound,
            default_name,
            None,
            scan,
        ));
    }

    // 单包
    let pack = scan.packs.first().ok_or_else(|| {
        anyhow::anyhow!("Invalid archive: no manifest found")
    })?;

    let name = pack
        .manifest
        .header
        .as_ref()
        .and_then(|h| h.name.clone())
        .unwrap_or(default_name);

    let uuid = pack
        .manifest
        .header
        .as_ref()
        .and_then(|h| h.uuid.clone());

    debug!(
        "Analyze archive: {:?}, result={:?}, packs={}, compound={}, nested={}, elapsed={} ms",
        original_path,
        pack.pack_type,
        scan.packs.len(),
        scan.is_compound,
        scan.has_nested_archive,
        start.elapsed().as_millis()
    );
    Ok((
        pack.pack_type.clone(),
        name,
        uuid,
        scan,
    ))
}

fn resolve_world_template_primary(scan: &ArchiveScanResult) -> Option<PackEntry> {
    let mut templates: Vec<PackEntry> = scan
        .packs
        .iter()
        .filter(|p| p.pack_type == ImportTargetType::WorldTemplate)
        .cloned()
        .collect();

    if templates.is_empty() {
        return None;
    }

    // Only accept if all templates share the same root.
    let template_root = normalize_root_key(&templates[0].root);
    if templates
        .iter()
        .any(|p| normalize_root_key(&p.root) != template_root)
    {
        return None;
    }

    // Allow internal packs under world_template root (resource_packs/behavior_packs/skin_packs).
    for p in &scan.packs {
        if p.pack_type == ImportTargetType::WorldTemplate {
            continue;
        }
        let root = normalize_root_key(&p.root);
        let ok = root.starts_with(&format!("{template_root}resource_packs/"))
            || root.starts_with(&format!("{template_root}behavior_packs/"))
            || root.starts_with(&format!("{template_root}skin_packs/"));
        if !ok {
            return None;
        }
    }

    Some(templates.remove(0))
}

fn detect_type_from_manifest(manifest: &PartialManifest) -> ImportTargetType {
    if let Some(modules) = &manifest.modules {
        for module in modules {
            match module.module_type.as_str() {
                "world_template" | "worldtemplate" => {
                    return ImportTargetType::WorldTemplate;
                }
                "resources" | "resourcepack" | "texturepack" => {
                    return ImportTargetType::ResourcePack;
                }
                "data"
                | "behaviorpack"
                | "script"
                | "javascript"
                | "client_data"
                | "interface" => {
                    return ImportTargetType::BehaviorPack;
                }
                "skin_pack" | "skinpack" => {
                    return ImportTargetType::SkinPack;
                }
                _ => {}
            }
        }
    }
    ImportTargetType::Unknown
}


fn process_compound_archive(
    archive: &mut ZipArchive<File>,
    original_file_path: &Path,
    options: &GamePathOptions,
    overwrite: bool,
) -> Result<()> {
    // 高性能策略：
    // 1) 优先复用 inspect 阶段生成的缓存目录（避免二次解压）。
    // 2) 若未命中缓存，则一次性解压外层 + 并行展开嵌套包到缓存目录。
    // 3) 导入阶段只处理“目录包”（manifest.json 或 level.dat），不再逐个打开子 Zip。

    let key = compound_cache_key(original_file_path).ok();

    // 1) 优先复用缓存
    if let Some(ref k) = key {
        if let Some(work_dir) = cache_take_compound(k) {
            if work_dir.exists() {
                debug!("Compound cache hit: {:?} -> {:?}", original_file_path, work_dir);
                let mut _inner_archives = Vec::new();
                let mut pack_dirs = Vec::new();
                collect_inner_archives_and_dirs(&work_dir, &mut _inner_archives, &mut pack_dirs, 2)?;

                debug!("Compound import (cache): pack_dirs={}", pack_dirs.len());
                let res = import_from_cache_dirs(&pack_dirs, options, overwrite);
                let _ = fs::remove_dir_all(&work_dir);
                return res;
            }
        }
    }

    // 2) 未命中缓存：自己展开一次
    let (work_dir, pack_dirs) = extract_to_cache_with_nested(archive, "import")?;
    debug!(
        "Compound cache miss: {:?} -> {:?}, pack_dirs={}",
        original_file_path,
        work_dir,
        pack_dirs.len()
    );
    let res = import_from_cache_dirs(&pack_dirs, options, overwrite);
    let _ = fs::remove_dir_all(&work_dir);
    res
}

fn extract_subdir_from_zip(
    archive: &mut ZipArchive<File>,
    root: &str,
    dest: &Path,
) -> Result<()> {

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name();

        if !name.starts_with(root) || name.contains("__MACOSX") {
            continue;
        }

        let rel = &name[root.len()..];
        let target = dest.join(rel);

        if file.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            let mut out = std::io::BufWriter::new(File::create(&target)?);
            std::io::copy(&mut file, &mut out)?;
        }
    }
    Ok(())
}

fn extract_pack_root(
    archive: &mut ZipArchive<File>,
    pack_root: &str,
    dest: &Path,
) -> Result<()> {
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name();

        if name.contains("__MACOSX") {
            continue;
        }

        // World（root = ""）特殊处理
        let relative = if pack_root.is_empty() {
            Path::new(name)
        } else if let Some(stripped) = name.strip_prefix(pack_root) {
            Path::new(stripped)
        } else {
            continue;
        };

        if relative.as_os_str().is_empty() {
            continue;
        }

        let out_path = dest.join(relative);
        if !out_path.starts_with(dest) {
            continue;
        }

        if file.is_dir() {
            std::fs::create_dir_all(&out_path)?;
        } else {
            if let Some(p) = out_path.parent() {
                std::fs::create_dir_all(p)?;
            }
            let mut out = std::io::BufWriter::new(File::create(&out_path)?);
            std::io::copy(&mut file, &mut out)?;
        }
    }
    Ok(())
}

fn extract_archive(archive: &mut ZipArchive<File>, dest_root: &Path) -> Result<()> {
    if !dest_root.exists() { fs::create_dir_all(dest_root)?; }

    let mut common_root: Option<PathBuf> = None;
    let mut is_first = true;
    let mut has_files_at_root = false;

    // 第一次遍历：检测公共根目录
    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let path = PathBuf::from(file.name());
        if path.to_string_lossy().contains("__MACOSX") || file.is_dir() { continue; }

        if let Some(parent) = path.parent() {
            if parent.as_os_str().is_empty() { has_files_at_root = true; }
            if is_first {
                if let Some(first_comp) = path.components().next() { common_root = Some(PathBuf::from(first_comp.as_os_str())); }
                is_first = false;
            } else if let Some(ref root) = common_root {
                if !path.starts_with(root) { common_root = None; has_files_at_root = true; }
            }
        } else {
            has_files_at_root = true;
            common_root = None;
        }
    }

    if has_files_at_root { common_root = None; }

    // 第二次遍历：解压
    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let original_path = PathBuf::from(file.name());
        if original_path.to_string_lossy().contains("__MACOSX") { continue; }

        let relative_path = if let Some(ref root) = common_root {
            if let Ok(stripped) = original_path.strip_prefix(root) { stripped.to_path_buf() } else { original_path }
        } else { original_path };

        if relative_path.as_os_str().is_empty() { continue; }
        let target_path = dest_root.join(&relative_path);
        if !target_path.starts_with(dest_root) { continue; }

        if file.is_dir() { fs::create_dir_all(&target_path)?; }
        else {
            if let Some(p) = target_path.parent() { if !p.exists() { fs::create_dir_all(p)?; } }
            let mut outfile = std::io::BufWriter::new(File::create(&target_path)?);
            std::io::copy(&mut file, &mut outfile)?;
        }
    }
    Ok(())
}

fn extract_archive_parallel(file_path: &Path, dest_root: &Path) -> Result<()> {
    if !dest_root.exists() { fs::create_dir_all(dest_root)?; }

    // 第一次遍历：计算公共根目录 + 记录索引与路径
    let file = File::open(file_path)?;
    let mut archive = ZipArchive::new(file)?;

    let mut common_root: Option<PathBuf> = None;
    let mut is_first = true;
    let mut has_files_at_root = false;
    let mut entries: Vec<(usize, PathBuf, bool)> = Vec::with_capacity(archive.len());

    for i in 0..archive.len() {
        let file = archive.by_index(i)?;
        let path = PathBuf::from(file.name());
        if path.to_string_lossy().contains("__MACOSX") { continue; }

        if !file.is_dir() {
            if let Some(parent) = path.parent() {
                if parent.as_os_str().is_empty() { has_files_at_root = true; }
                if is_first {
                    if let Some(first_comp) = path.components().next() {
                        common_root = Some(PathBuf::from(first_comp.as_os_str()));
                    }
                    is_first = false;
                } else if let Some(ref root) = common_root {
                    if !path.starts_with(root) { common_root = None; has_files_at_root = true; }
                }
            } else {
                has_files_at_root = true;
                common_root = None;
            }
        }

        entries.push((i, path, file.is_dir()));
    }

    if has_files_at_root { common_root = None; }

    // 并行解压（按块）：每个线程只打开一次 ZipArchive，避免“每个文件条目都重开 zip”导致的巨大开销。
    // 对于 .mctemplate 这种包含海量小文件的包，这个改动能显著加速。
    let common_root_cloned = common_root.clone();
    const CHUNK_SIZE: usize = 64;

    entries.par_chunks(CHUNK_SIZE).for_each(|chunk| {
        let Ok(file) = File::open(file_path) else { return; };
        let Ok(mut z) = ZipArchive::new(file) else { return; };

        for (idx, original_path, is_dir) in chunk {
            if original_path.to_string_lossy().contains("__MACOSX") { continue; }

            let relative_path = if let Some(ref root) = common_root_cloned {
                if let Ok(stripped) = original_path.strip_prefix(root) { stripped.to_path_buf() } else { original_path.clone() }
            } else {
                original_path.clone()
            };

            if relative_path.as_os_str().is_empty() { continue; }
            let target_path = dest_root.join(&relative_path);
            if !target_path.starts_with(dest_root) { continue; }

            if *is_dir {
                let _ = fs::create_dir_all(&target_path);
                continue;
            }

            if let Some(p) = target_path.parent() {
                let _ = fs::create_dir_all(p);
            }

            if let Ok(mut entry) = z.by_index(*idx) {
                if let Ok(out) = std::fs::File::create(&target_path) {
                    let mut out = std::io::BufWriter::new(out);
                    let _ = std::io::copy(&mut entry, &mut out);
                }
            }
        }
    });

    Ok(())
}


// ================================
// 缓存解压（系统缓存/BMCBL）相关工具
// ================================

fn bmcbl_cache_base_dir() -> PathBuf {
    // “系统缓存”这里用 std::env::temp_dir()：跨平台且无需额外依赖。
    // 最终路径类似：%TEMP%/BMCBL/ 或 /tmp/BMCBL/
    std::env::temp_dir().join("BMCBL")
}

fn create_bmcbl_cache_workdir(purpose: &str) -> Result<PathBuf> {
    let ts = SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis();
    let pid = std::process::id();
    let dir = bmcbl_cache_base_dir().join(format!("{}_{}_{}", purpose, pid, ts));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

// ================================
// 复合包缓存索引（inspect -> import 复用）
// ================================

#[derive(Debug, Clone)]
struct CacheEntry {
    dir: PathBuf,
    created_at: SystemTime,
}

static COMPOUND_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, CacheEntry>>> =
    std::sync::OnceLock::new();

fn cache_map() -> &'static std::sync::Mutex<std::collections::HashMap<String, CacheEntry>> {
    COMPOUND_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn compound_cache_key(file_path: &Path) -> Result<String> {
    let meta = fs::metadata(file_path)?;
    let size = meta.len();
    let mtime = meta.modified().ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // 绝对路径 + size + mtime：足够稳定且不需要额外依赖 hash crate
    Ok(format!("{}|{}|{}", file_path.canonicalize()?.to_string_lossy(), size, mtime))
}

fn prune_compound_cache_locked(map: &mut std::collections::HashMap<String, CacheEntry>) {
    // 简单上限：最多保留 8 个；超过则按 created_at 最旧淘汰
    const MAX_ENTRIES: usize = 8;
    if map.len() <= MAX_ENTRIES {
        return;
    }

    let mut items: Vec<(String, SystemTime)> = map
        .iter()
        .map(|(k, v)| (k.clone(), v.created_at))
        .collect();

    items.sort_by_key(|(_, t)| *t);

    let remove_count = map.len().saturating_sub(MAX_ENTRIES);
    for i in 0..remove_count {
        if let Some((k, _)) = items.get(i) {
            if let Some(entry) = map.remove(k) {
                let _ = fs::remove_dir_all(entry.dir);
            }
        }
    }
}

fn cache_put_compound(key: String, dir: PathBuf) {
    let mut map = cache_map().lock().unwrap();
    map.insert(key, CacheEntry { dir, created_at: SystemTime::now() });
    prune_compound_cache_locked(&mut map);
}

fn cache_take_compound(key: &str) -> Option<PathBuf> {
    let mut map = cache_map().lock().unwrap();
    map.remove(key).map(|e| e.dir)
}

fn cleanup_compound_cache_for_file(file_path: &Path) {
    if let Ok(key) = compound_cache_key(file_path) {
        if let Some(dir) = cache_take_compound(&key) {
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

// ================================
// 复合包一次性展开（外层 + 嵌套 zip）
// ================================

fn is_nested_archive_file(path: &Path) -> bool {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "mcpack" | "mcaddon" | "mcworld" | "mctemplate" | "zip")
}

fn extract_one_nested_archive_to_dir(nested_file: &Path, nested_root: &Path) -> Result<PathBuf> {
    // 给每个子包生成稳定但不依赖外部 crate 的“唯一目录名”
    let stem = nested_file.file_stem().unwrap_or_default().to_string_lossy();
    let meta = fs::metadata(nested_file)?;
    let size = meta.len();
    let mtime = meta.modified().ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let out_dir = nested_root.join(format!("{}_{}_{}", stem, size, mtime));
    if out_dir.exists() {
        return Ok(out_dir);
    }
    fs::create_dir_all(&out_dir)?;

    let f = File::open(nested_file)?;
    let mut z = ZipArchive::new(f)?;
    extract_archive(&mut z, &out_dir)?;
    Ok(out_dir)
}

fn extract_to_cache_with_nested(
    outer: &mut ZipArchive<File>,
    purpose: &str,
) -> Result<(PathBuf, Vec<PathBuf>)> {
    let work_dir = create_bmcbl_cache_workdir(purpose)?;
    extract_archive(outer, &work_dir)
        .with_context(|| format!("Failed to extract outer archive to {:?}", work_dir))?;

    // BFS 展开嵌套包：最多 2 层（按复合包规范），防 zip 炸弹 / 恶意递归
    let nested_root = work_dir.join(".nested");
    fs::create_dir_all(&nested_root)?;

    let mut pack_dirs: Vec<PathBuf> = Vec::new();
    let mut queue: Vec<PathBuf> = Vec::new();

    // 初次收集
    {
        let mut inner_archives: Vec<PathBuf> = Vec::new();
        let mut dirs: Vec<PathBuf> = Vec::new();
        collect_inner_archives_and_dirs(&work_dir, &mut inner_archives, &mut dirs, 2)?;
        pack_dirs.extend(dirs);
        queue.extend(inner_archives);
    }

    // 去掉 work_dir 自己产生的“嵌套缓存目录”，避免自扫描产生循环
    queue.retain(|p| is_nested_archive_file(p));

    for _depth in 0..2 {
        if queue.is_empty() {
            break;
        }

        // 本轮要展开的文件
        let current = std::mem::take(&mut queue);

        // 并行展开
        let extracted_dirs: Vec<PathBuf> = current
            .par_iter()
            .filter_map(|p| match extract_one_nested_archive_to_dir(p, &nested_root) {
                Ok(d) => Some(d),
                Err(e) => {
                    warn!("Failed to extract nested archive {:?}: {:?}", p, e);
                    None
                }
            })
            .collect();

        // 对展开目录继续收集（可能还有更深层嵌套）
        for d in extracted_dirs {
            let mut inner_archives: Vec<PathBuf> = Vec::new();
            let mut dirs: Vec<PathBuf> = Vec::new();
            if collect_inner_archives_and_dirs(&d, &mut inner_archives, &mut dirs, 2).is_ok() {
                pack_dirs.extend(dirs);
                // 继续递归嵌套
                inner_archives.retain(|p| is_nested_archive_file(p));
                queue.extend(inner_archives);
            }
        }
    }

    // 去重
    pack_dirs.sort();
    pack_dirs.dedup();

    Ok((work_dir, pack_dirs))
}

fn import_from_cache_dirs(
    pack_dirs: &[PathBuf],
    options: &GamePathOptions,
    overwrite: bool,
) -> Result<()> {
    // IO 密集：并行导入目录包（每个 pack 独立）
    pack_dirs.par_iter().for_each(|dir| {
        let res = if dir.join("manifest.json").is_file() {
            import_pack_dir(dir, options, overwrite)
        } else if dir.join("level.dat").is_file() {
            import_world_dir(dir, options, overwrite)
        } else {
            Ok(())
        };

        if let Err(e) = res {
            warn!("Failed to import from cache dir {:?}: {:?}", dir, e);
        }
    });

    Ok(())
}

fn get_world_info_from_dir(dir: &Path) -> Result<PackagePreview> {
    let mut name = dir.file_name().unwrap_or_default().to_string_lossy().to_string();
    let mut description = "".to_string();
    let mut icon = None;

    // level.dat
    let level_path = dir.join("level.dat");
    if level_path.exists() {
        let buf = fs::read(&level_path)?;
        if let Ok(NbtTag::Compound(root)) = parse_root_nbt_with_header(&buf) {
            if let Some(NbtTag::String(n)) = root.get("LevelName") {
                name = n.clone();
            }
        }
    }

    // world_icon.jpeg（如果存在）
    let icon_path = dir.join("world_icon.jpeg");
    if icon_path.exists() {
        let buf = fs::read(&icon_path)?;
        let b64 = general_purpose::STANDARD.encode(&buf);
        icon = Some(format!("data:image/jpeg;base64,{}", b64));
    }

    let size = dir_size(dir).unwrap_or(0);

    Ok(PackagePreview {
        name,
        description,
        icon,
        kind: ImportTargetType::World.to_display_name().to_string(),
        version: None,
        size,
        manifest: None,
        sub_packs: None,
        valid: true,
        invalid_reason: None,
    })
}

fn import_world_dir(dir: &Path, options: &GamePathOptions, overwrite: bool) -> Result<()> {
    let parent_dir = resolve_target_parent(options, ImportTargetType::World.to_dir_name(), false)
        .ok_or_else(|| anyhow::anyhow!("无法解析目标安装路径"))?;

    let preview = get_world_info_from_dir(dir).unwrap_or(PackagePreview {
        name: dir.file_name().unwrap_or_default().to_string_lossy().to_string(),
        description: "".into(),
        icon: None,
        kind: ImportTargetType::World.to_display_name().to_string(),
        version: None,
        size: 0,
        manifest: None,
        sub_packs: None,
        valid: true,
        invalid_reason: None,
    });

    let clean_name = sanitize_filename(&strip_minecraft_formatting(&preview.name));
    let base_folder_name = clean_name;
    let mut dest_folder_name = base_folder_name.clone();
    let mut final_dest = parent_dir.join(&dest_folder_name);
    let mut counter = 1;

    loop {
        if !final_dest.exists() {
            break;
        }
        if overwrite {
            break;
        }
        dest_folder_name = format!("{}_{}", base_folder_name, counter);
        final_dest = parent_dir.join(&dest_folder_name);
        counter += 1;
        if counter > 100 {
            break;
        }
    }

    if final_dest.exists() {
        fs::remove_dir_all(&final_dest)
            .with_context(|| format!("Failed to remove existing world dir {:?}", final_dest))?;
    }

    debug!("Import world dir: {:?} -> {:?}", dir, final_dest);
    copy_dir_recursive(dir, &final_dest)
        .with_context(|| format!("Failed to copy world dir {:?} -> {:?}", dir, final_dest))?;

    Ok(())
}

fn dir_size(path: &Path) -> Result<u64> {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];

    while let Some(p) = stack.pop() {
        for entry in fs::read_dir(&p)? {
            let entry = entry?;
            let ep = entry.path();
            let meta = entry.metadata()?;
            if meta.is_dir() {
                stack.push(ep);
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}


/// 递归收集（带剪枝与深度限制）：
/// - 内嵌压缩包文件：.mcpack/.mcaddon/.mcworld/.mctemplate/.zip
/// - 直接解压得到的包目录：包含 manifest.json 或 level.dat 的目录
/// 规则：
/// - 如果目录已包含 manifest.json 或 level.dat，则不再深入扫描其子目录
/// - 最大扫描深度可控，避免大目录遍历
fn collect_inner_archives_and_dirs(
    root: &Path,
    inner_archives: &mut Vec<PathBuf>,
    pack_dirs: &mut Vec<PathBuf>,
    max_depth: usize,
) -> Result<()> {
    use std::collections::HashSet;
    use std::sync::Mutex;

    let archives_out = Mutex::new(Vec::new());
    let pack_dirs_out = Mutex::new(Vec::new());

    let walker = WalkDir::new(root)
        .follow_links(false)
        .min_depth(0)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                if e.file_name()
                    .to_str()
                    .map(|s| s.eq_ignore_ascii_case("__MACOSX"))
                    .unwrap_or(false)
                {
                    return false;
                }
                let p = e.path();
                let has_manifest = p.join("manifest.json").is_file();
                let has_level = p.join("level.dat").is_file();
                if has_manifest || has_level {
                    let mut lock = pack_dirs_out.lock().unwrap();
                    lock.push(p.to_path_buf());
                    return false;
                }
            }
            true
        });

    walker.par_bridge().for_each(|entry| {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => return,
        };
        if !entry.file_type().is_file() {
            return;
        }
        let p = entry.path();
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        let is_archive = matches!(
            ext.as_str(),
            "mcpack" | "mcaddon" | "mcworld" | "mctemplate" | "zip"
        );
        if is_archive {
            let mut lock = archives_out.lock().unwrap();
            lock.push(p.to_path_buf());
        }
    });

    let mut seen_archives: HashSet<PathBuf> = HashSet::new();
    let mut seen_dirs: HashSet<PathBuf> = HashSet::new();

    for p in archives_out.into_inner().unwrap_or_default() {
        if seen_archives.insert(p.clone()) {
            inner_archives.push(p);
        }
    }
    for p in pack_dirs_out.into_inner().unwrap_or_default() {
        if seen_dirs.insert(p.clone()) {
            pack_dirs.push(p);
        }
    }

    Ok(())
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir_recursive(&path, &target)?;
        } else {
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}

fn import_pack_dir(dir: &Path, options: &GamePathOptions, overwrite: bool) -> Result<()> {
    // 读取 manifest.json
    let manifest_path = dir.join("manifest.json");
    if !manifest_path.exists() {
        return Err(anyhow::anyhow!("manifest.json not found in {:?}", dir));
    }
    let content = fs::read_to_string(&manifest_path)?;
    let clean = strip_json_comments(&content);
    let manifest: PartialManifest = serde_json::from_str(&clean)
        .with_context(|| format!("Failed to parse manifest.json in {:?}", dir))?;

    let target_type = detect_type_from_manifest(&manifest);
    if let ImportTargetType::Unknown = target_type {
        return Err(anyhow::anyhow!("无法识别包类型: {:?}", dir));
    }

    let internal_name = manifest
        .header
        .as_ref()
        .and_then(|h| h.name.clone())
        .unwrap_or_else(|| {
            dir.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Imported_Pack".to_string())
        });

    let pack_uuid = manifest.header.as_ref().and_then(|h| h.uuid.clone());
    if !matches!(target_type, ImportTargetType::World) && pack_uuid.is_none() {
        return Err(anyhow::anyhow!("manifest.header.uuid 缺失，无法导入"));
    }

    // 文件夹名称处理：优先使用 pack_<uuid>
    let base_folder_name = match &pack_uuid {
        Some(uuid) => pack_folder_name(uuid),
        None => sanitize_filename(&strip_minecraft_formatting(&internal_name)),
    };
    let target_dir_name = target_type.to_dir_name();

    let is_shared_preferred = matches!(
        target_type,
        ImportTargetType::ResourcePack | ImportTargetType::BehaviorPack
    );

    let parent_dir = resolve_target_parent(options, target_dir_name, is_shared_preferred)
        .ok_or_else(|| anyhow::anyhow!("无法解析目标安装路径"))?;
    if options.build_type == crate::core::minecraft::paths::BuildType::Gdk
        && !is_shared_preferred
        && !parent_dir.exists()
        && !options.allow_shared_fallback
    {
        return Err(anyhow::anyhow!("目标目录不存在，需要确认后导入 Shared"));
    }

    let mut dest_folder_name = base_folder_name.clone();
    let mut final_dest = parent_dir.join(&dest_folder_name);
    let mut counter = 1;

    loop {
        if !final_dest.exists() {
            break;
        }

        let mut allow_overwrite = false;
        if let Some(new_uuid) = &pack_uuid {
            if let Some(existing_uuid) = get_pack_uuid_from_dir(&final_dest) {
                if existing_uuid == *new_uuid {
                    if overwrite {
                        info!(
                            "UUID match ({}), overwriting existing pack at {:?}",
                            new_uuid, dest_folder_name
                        );
                        allow_overwrite = true;
                    } else {
                        return Err(anyhow::anyhow!("Pack with same UUID exists (overwrite denied)"));
                    }
                }
            }
        }

        if allow_overwrite {
            break;
        }

        dest_folder_name = format!("{}_{}", base_folder_name, counter);
        final_dest = parent_dir.join(&dest_folder_name);
        counter += 1;
    }

    if final_dest.exists() {
        // overwrite 路径：先清掉旧目录再复制
        fs::remove_dir_all(&final_dest)
            .with_context(|| format!("Failed to remove existing dir {:?}", final_dest))?;
    }

    debug!("Import pack dir: {:?} -> {:?}", dir, final_dest);
    copy_dir_recursive(dir, &final_dest)
        .with_context(|| format!("Failed to copy {:?} -> {:?}", dir, final_dest))?;

    Ok(())
}

fn get_pack_uuid_from_dir(dir: &Path) -> Option<String> {
    let manifest_path = dir.join("manifest.json");
    if !manifest_path.exists() { return None; }
    let content = fs::read_to_string(&manifest_path).ok()?;
    let clean = strip_json_comments(&content);
    let manifest: PartialManifest = serde_json::from_str(&clean).ok()?;
    manifest.header.and_then(|h| h.uuid)
}

fn get_pack_info_from_dir(
    dir: &Path,
    target_type: &ImportTargetType,
    preferred_lang: Option<&str>,
) -> Result<PackagePreview> {
    let manifest_path = dir.join("manifest.json");
    let content = fs::read_to_string(&manifest_path)?;
    let clean = strip_json_comments(&content);
    let mut manifest: PartialManifest = serde_json::from_str(&clean)?;

    let detected_type = if *target_type == ImportTargetType::Unknown {
        detect_type_from_manifest(&manifest)
    } else {
        target_type.clone()
    };

    let mut name = dir.file_name().unwrap().to_string_lossy().to_string();
    let mut description = "".to_string();
    let mut version_str = None;
    let mut valid = true;
    let mut invalid_reason: Option<String> = None;

    use crate::core::minecraft::resource_packs::load_lang_map_for_pack;
    let lang = preferred_lang
        .map(normalize_lang_code)
        .unwrap_or_else(|| "zh-CN".to_string());
    if let Some(lang_map) = load_lang_map_for_pack(&dir.to_path_buf(), &lang) {
        if let Some(header) = manifest.header.as_mut() {
            if let Some(n) = &header.name {
                name = lang_map.get(n).cloned().unwrap_or_else(|| n.clone());
                header.name = Some(name.clone());
            }
            if let Some(d) = &header.description {
                description = lang_map.get(d).cloned().unwrap_or_else(|| d.clone());
                header.description = Some(description.clone());
            }
        }
    } else {
        if let Some(header) = &manifest.header {
            name = header.name.clone().unwrap_or(name);
            description = header.description.clone().unwrap_or_default();
        }
    }

    if let Some(header) = &manifest.header {
        if let Some(v) = &header.version {
            version_str = Some(v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("."));
        }
        if header.uuid.is_none() && !matches!(detected_type, ImportTargetType::World) {
            valid = false;
            invalid_reason = Some("manifest.header.uuid 缺失".to_string());
        }
    } else {
        valid = false;
        invalid_reason = Some("manifest.header 缺失".to_string());
    }

    let icon_candidates: Vec<&str> = if detected_type == ImportTargetType::WorldTemplate {
        vec![
            "world_icon.jpeg",
            "world_icon.jpg",
            "world_icon.png",
            "pack_icon.png",
            "pack_icon.jpg",
            "pack_icon.jpeg",
        ]
    } else {
        vec!["pack_icon.png", "pack_icon.jpg", "pack_icon.jpeg"]
    };

    let mut icon = None;
    for icon_name in icon_candidates {
        let icon_path = dir.join(icon_name);
        if icon_path.exists() {
            let buf = fs::read(icon_path)?;
            let mime = if buf.starts_with(&[0xFF, 0xD8, 0xFF]) { "image/jpeg" } else { "image/png" };
            let b64 = general_purpose::STANDARD.encode(&buf);
            icon = Some(format!("data:{};base64,{}", mime, b64));
            break;
        }
    }

    let size = dir_size(dir).unwrap_or(0);

    Ok(PackagePreview {
        name,
        description,
        icon,
        kind: detected_type.to_display_name().to_string(),
        version: version_str,
        size,
        manifest: Some(manifest),
        sub_packs: None, // [新增] 默认为 None
        valid,
        invalid_reason,
    })
}

// [修改] Windows 文件夹名规范化 + 长度限制
fn sanitize_filename(name: &str) -> String {
    const MAX_LEN: usize = 80;
    const INVALID: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_control() || INVALID.contains(&c) {
            out.push('_');
        } else {
            out.push(c);
        }
    }

    // 去除首尾空格/点，避免 Windows 不允许的结尾
    let trimmed = out.trim().trim_end_matches('.');
    let mut out = trimmed.to_string();

    // 限制长度
    if out.chars().count() > MAX_LEN {
        out = out.chars().take(MAX_LEN).collect();
        out = out.trim().trim_end_matches('.').to_string();
    }

    // 处理保留设备名
    let upper = out.to_ascii_uppercase();
    let reserved = [
        "CON", "PRN", "AUX", "NUL",
        "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9",
        "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved.contains(&upper.as_str()) {
        out = format!("{}_pack", out);
    }

    if out.is_empty() {
        "Imported_Pack".to_string()
    } else {
        out
    }
}

fn pack_folder_name(uuid: &str) -> String {
    let clean = uuid.trim();
    if clean.is_empty() {
        "pack_".to_string()
    } else {
        format!("pack_{}", clean)
    }
}

// [新增] 去除 Minecraft 格式化代码 (§x)
fn strip_minecraft_formatting(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '§' {
            // 跳过下一个字符
            chars.next();
        } else {
            output.push(c);
        }
    }
    output
}

fn strip_json_comments(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    let mut in_string = false;
    let mut in_line_comment = false;
    let mut in_block_comment = false;

    while let Some(c) = chars.next() {
        if in_line_comment { if c == '\n' { in_line_comment = false; output.push(c); } continue; }
        if in_block_comment { if c == '*' { if let Some('/') = chars.peek() { chars.next(); in_block_comment = false; } } continue; }
        if in_string { output.push(c); if c == '\\' { if let Some(n) = chars.next() { output.push(n); } } else if c == '"' { in_string = false; } continue; }
        if c == '"' { in_string = true; output.push(c); continue; }
        if c == '/' { if let Some(&next) = chars.peek() { if next == '/' { chars.next(); in_line_comment = true; continue; } else if next == '*' { chars.next(); in_block_comment = true; continue; } } }
        output.push(c);
    }
    output
}

fn read_zip_text_case_insensitive<R: Read + Seek>(archive: &mut ZipArchive<R>, target_path: &str) -> Option<String> {
    let target = target_path.replace('\\', "/").to_ascii_lowercase();
    for i in 0..archive.len() {
        if let Ok(mut file) = archive.by_index(i) {
            let name = file.name().replace('\\', "/").to_ascii_lowercase();
            if name == target {
                let mut buf = Vec::new();
                if file.read_to_end(&mut buf).is_ok() {
                    if let Some(s) = decode_text_bytes(&buf) {
                        return Some(s);
                    }
                }
            }
        }
    }
    None
}

fn read_zip_text_by_name<R: Read + Seek>(archive: &mut ZipArchive<R>, path: &str) -> Option<String> {
    let mut file = archive.by_name(path).ok()?;
    let mut buf = Vec::new();
    if file.read_to_end(&mut buf).is_ok() {
        return decode_text_bytes(&buf);
    }
    None
}

fn read_lang_map_for_prefix<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    root_prefix: &str,
    preferred_lang: Option<&str>,
) -> HashMap<String, String> {
    let mut lang_map = HashMap::new();

    let mut preferred: Vec<String> = Vec::new();
    if let Some(lang) = preferred_lang {
        let norm = normalize_lang_code(lang);
        preferred.push(norm.clone());
        if lang.contains('-') {
            preferred.push(lang.to_string());
            preferred.push(lang.replace('-', "_"));
        }
        if lang.contains('_') {
            preferred.push(lang.replace('_', "-"));
        }
    }

    let mut from_languages_json: Vec<String> = Vec::new();
    let candidates = [
        format!("{}texts/languages.json", root_prefix),
        format!("{}languages.json", root_prefix),
    ];
    for p in candidates {
        if let Some(content) = read_zip_text_case_insensitive(archive, &p) {
            if let Ok(list) = serde_json::from_str::<Vec<String>>(&content) {
                from_languages_json = list;
                break;
            }
        }
    }

    if !from_languages_json.is_empty() {
        for l in from_languages_json {
            if !preferred.iter().any(|p| p.eq_ignore_ascii_case(&l)) {
                preferred.push(l);
            }
        }
    }

    if preferred.is_empty() {
        preferred = vec!["zh-CN".into(), "zh-TW".into(), "en-US".into()];
    }

    for lang in &preferred {
        let mut variants: Vec<String> = Vec::new();
        variants.push(lang.to_string());
        variants.push(normalize_lang_code(lang));
        variants.push(lang.replace('_', "-"));
        variants.push(lang.replace('-', "_"));
        variants.sort();
        variants.dedup();

        for v in &variants {
            let rel = format!("texts/{}.lang", v);
            let target_path = format!("{}{}", root_prefix, rel);
            let alt_path = target_path.replace('/', "\\");

            let mut found_content = read_zip_text_case_insensitive(archive, &target_path);
            if found_content.is_none() {
                found_content = read_zip_text_case_insensitive(archive, &alt_path);
            }
            if found_content.is_none() {
                let rel2 = format!("{}.lang", v);
                let target2 = format!("{}{}", root_prefix, rel2);
                let alt2 = target2.replace('/', "\\");
                found_content = read_zip_text_case_insensitive(archive, &target2)
                    .or_else(|| read_zip_text_case_insensitive(archive, &alt2));
            }

            if let Some(content) = found_content {
                lang_map = parse_lang_config(&content);
                if !lang_map.is_empty() {
                    return lang_map;
                }
            }
        }
    }

    // fallback: any .lang under texts/
    if let Some(content) = find_any_lang_in_texts(archive, root_prefix) {
        lang_map = parse_lang_config(&content);
    }

    if lang_map.is_empty() && !root_prefix.is_empty() {
        return read_lang_map_for_prefix(archive, "", preferred_lang);
    }

    lang_map
}

fn decode_text_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Some(String::from_utf8_lossy(&bytes[3..]).to_string());
    }
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let utf16: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&utf16).ok();
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let utf16: Vec<u16> = bytes[2..]
            .chunks(2)
            .filter(|c| c.len() == 2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&utf16).ok();
    }
    if let Ok(s) = String::from_utf8(bytes.to_vec()) {
        return Some(s);
    }
    if bytes.len() % 2 == 0 {
        let utf16: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if let Ok(s) = String::from_utf16(&utf16) {
            return Some(s);
        }
    }
    None
}

fn list_nested_archives<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Vec<String> {
    let mut paths = Vec::new();
    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            if file.is_dir() { continue; }
            let name = file.name().to_string();
            if name.contains("__MACOSX") { continue; }
            let path = Path::new(&name);
            let ext = path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if matches!(
                ext.as_str(),
                "mcpack" | "mcaddon" | "mcworld" | "mctemplate" | "zip"
            ) {
                paths.push(name);
            }
        }
    }
    paths
}

fn filter_packs_excluding_template_internal(packs: &[PackEntry]) -> Vec<PackEntry> {
    let template_roots: Vec<String> = packs
        .iter()
        .filter(|p| p.pack_type == ImportTargetType::WorldTemplate)
        .map(|p| p.root.clone())
        .collect();

    if template_roots.is_empty() {
        return packs.to_vec();
    }

    packs
        .iter()
        .filter(|p| {
            template_roots.iter().all(|root| {
                let normalized = p.root.replace('\\', "/").to_lowercase();
                let prefix = root.replace('\\', "/").to_lowercase();
                if normalized.starts_with(&format!("{}/resource_packs", prefix)) { return false; }
                if normalized.starts_with(&format!("{}/behavior_packs", prefix)) { return false; }
                true
            })
        })
        .cloned()
        .collect()
}

fn inspect_nested_archives_quick(
    archive: &mut ZipArchive<File>,
    preferred_lang: Option<&str>,
) -> Result<Vec<PackagePreview>> {
    const MAX_NESTED_SIZE: u64 = 200 * 1024 * 1024; // 200 MB
    let names = list_nested_archives(archive);
    if names.is_empty() {
        return Ok(Vec::new());
    }

    let mut previews = Vec::new();

    for name in names {
        let mut file = match archive.by_name(&name) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if file.size() > MAX_NESTED_SIZE {
            continue;
        }
        let mut buf = Vec::with_capacity(file.size() as usize);
        if file.read_to_end(&mut buf).is_err() {
            continue;
        }

        let cursor = Cursor::new(buf);
        let mut nested = match ZipArchive::new(cursor) {
            Ok(z) => z,
            Err(_) => continue,
        };

        let scan = scan_archive(&mut nested)?;
        let filtered_packs = filter_packs_excluding_template_internal(&scan.packs);
        let mut seen = std::collections::HashSet::new();
        for pack in &filtered_packs {
            if !seen.insert(pack.manifest_path.clone()) {
                continue;
            }
            if let Ok(p) = get_pack_info_from_zip(&mut nested, pack, preferred_lang) {
                previews.push(p);
            }
        }
        let filtered_level_roots = filter_level_roots_excluding_template_internal(&scan.level_roots, &scan.packs);
        for root in &filtered_level_roots {
            if let Ok(p) = get_world_info_from_zip(&mut nested, root) {
                previews.push(p);
            }
        }
    }

    Ok(previews)
}

fn read_zip_bytes_case_insensitive<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    target_path: &str,
) -> Option<Vec<u8>> {
    let target = target_path.replace('\\', "/").to_ascii_lowercase();
    for i in 0..archive.len() {
        if let Ok(mut file) = archive.by_index(i) {
            let name = file.name().replace('\\', "/").to_ascii_lowercase();
            if name == target {
                let mut buf = Vec::new();
                if file.read_to_end(&mut buf).is_ok() {
                    return Some(buf);
                }
            }
        }
    }
    None
}

fn find_world_icon_under_root<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    root_prefix: &str,
) -> Option<Vec<u8>> {
    let root = normalize_root_key(root_prefix);
    let exts = ["world_icon.jpeg", "world_icon.jpg", "world_icon.png"];

    for ext in &exts {
        let direct = format!("{}{}", root, ext);
        if let Some(buf) = read_zip_bytes_case_insensitive(archive, &direct) {
            return Some(buf);
        }
    }

    for i in 0..archive.len() {
        if let Ok(mut file) = archive.by_index(i) {
            if file.is_dir() { continue; }
            let name = file.name().replace('\\', "/").to_ascii_lowercase();
            if !root.is_empty() && !name.starts_with(&root) { continue; }
            if exts.iter().any(|ext| name.ends_with(ext)) {
                let mut buf = Vec::new();
                if file.read_to_end(&mut buf).is_ok() {
                    return Some(buf);
                }
            }
        }
    }
    None
}

fn find_world_icon_near_manifest<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest_path: &str,
) -> Option<Vec<u8>> {
    let parent = Path::new(manifest_path)
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let root = normalize_root_key(&parent);
    if let Some(buf) = find_world_icon_under_root(archive, &root) {
        return Some(buf);
    }

    // Fallback: search anywhere in archive
    let exts = ["world_icon.jpeg", "world_icon.jpg", "world_icon.png"];
    for i in 0..archive.len() {
        if let Ok(mut file) = archive.by_index(i) {
            if file.is_dir() { continue; }
            let name = file.name().replace('\\', "/").to_ascii_lowercase();
            if exts.iter().any(|ext| name.ends_with(ext)) {
                let mut buf = Vec::new();
                if file.read_to_end(&mut buf).is_ok() {
                    return Some(buf);
                }
            }
        }
    }
    None
}

fn find_any_lang_in_texts<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    root_prefix: &str,
) -> Option<String> {
    let prefer = ["zh-cn.lang", "zh-tw.lang", "en-us.lang"];
    let base = format!("{}texts/", root_prefix).replace('\\', "/").to_ascii_lowercase();

    for p in &prefer {
        let target = format!("{}{}", base, p);
        let alt = target.replace('-', "_");
        let mut content = read_zip_text_case_insensitive(archive, &target);
        if content.is_none() {
            content = read_zip_text_case_insensitive(archive, &alt);
        }
        if let Some(content) = content {
            if !content.is_empty() {
                return Some(content);
            }
        }
    }

    for i in 0..archive.len() {
        if let Ok(mut file) = archive.by_index(i) {
            let name = file.name().replace('\\', "/").to_ascii_lowercase();
            if name.starts_with(&base) && name.ends_with(".lang") {
                let mut buf = Vec::new();
                if file.read_to_end(&mut buf).is_ok() {
                    if let Some(s) = decode_text_bytes(&buf) {
                        return Some(s);
                    }
                }
            }
        }
    }
    None
}

fn get_pack_info_from_zip<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    pack: &PackEntry,
    preferred_lang: Option<&str>,
) -> Result<PackagePreview> {
    let root_prefix = pack.root.clone();
    let mut name = pack
        .manifest
        .header
        .as_ref()
        .and_then(|h| h.name.clone())
        .unwrap_or_else(|| "Imported_Pack".to_string());
    let mut description = pack
        .manifest
        .header
        .as_ref()
        .and_then(|h| h.description.clone())
        .unwrap_or_default();
    let mut version_str = None;
    let mut manifest = pack.manifest.clone();
    let mut valid = true;
    let mut invalid_reason: Option<String> = None;

    // i18n
    let lang_map = read_lang_map_for_prefix(archive, &root_prefix, preferred_lang);

    if let Some(header) = manifest.header.as_mut() {
        if let Some(n) = &header.name {
            name = lang_map.get(n).cloned().unwrap_or_else(|| n.clone());
            header.name = Some(name.clone());
        }
        if let Some(d) = &header.description {
            description = lang_map.get(d).cloned().unwrap_or_else(|| d.clone());
            header.description = Some(description.clone());
        }
        if let Some(v) = &header.version {
            version_str = Some(v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("."));
        }
        if header.uuid.is_none() {
            valid = false;
            invalid_reason = Some("manifest.header.uuid 缺失".to_string());
        }
    } else {
        valid = false;
        invalid_reason = Some("manifest.header 缺失".to_string());
    }

    // icon
    let icon_candidates: Vec<&str> = if pack.pack_type == ImportTargetType::WorldTemplate {
        vec![
            "pack_icon.png",
            "pack_icon.jpg",
            "pack_icon.jpeg",
        ]
    } else {
        vec!["pack_icon.png", "pack_icon.jpg", "pack_icon.jpeg"]
    };
    let mut icon = None;
    if pack.pack_type == ImportTargetType::WorldTemplate {
        if let Some(buf) = find_world_icon_under_root(archive, &root_prefix)
            .or_else(|| find_world_icon_near_manifest(archive, &pack.manifest_path))
        {
            let mime = if buf.starts_with(&[0xFF, 0xD8, 0xFF]) { "image/jpeg" } else { "image/png" };
            let b64 = general_purpose::STANDARD.encode(&buf);
            icon = Some(format!("data:{};base64,{}", mime, b64));
        }
    }
    for icon_name in icon_candidates {
        if icon.is_some() { break; }
        let target_path = format!("{}{}", root_prefix, icon_name);
        if let Ok(mut f) = archive.by_name(&target_path) {
            let mut buf = Vec::new();
            if f.read_to_end(&mut buf).is_ok() {
                let mime = if buf.starts_with(&[0xFF, 0xD8, 0xFF]) { "image/jpeg" } else { "image/png" };
                let b64 = general_purpose::STANDARD.encode(&buf);
                icon = Some(format!("data:{};base64,{}", mime, b64));
                break;
            }
        }
        if icon.is_some() { break; }
        if let Some(buf) = read_zip_bytes_case_insensitive(archive, &target_path) {
            let mime = if buf.starts_with(&[0xFF, 0xD8, 0xFF]) { "image/jpeg" } else { "image/png" };
            let b64 = general_purpose::STANDARD.encode(&buf);
            icon = Some(format!("data:{};base64,{}", mime, b64));
            break;
        }
    }

    Ok(PackagePreview {
        name,
        description,
        icon,
        kind: pack.pack_type.to_display_name().to_string(),
        version: version_str,
        size: 0,
        manifest: Some(manifest),
        sub_packs: None,
        valid,
        invalid_reason,
    })
}

fn get_world_info_from_zip<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    root_prefix: &str,
) -> Result<PackagePreview> {
    let mut name = root_prefix.trim_end_matches('/').to_string();
    let mut description = "".to_string();
    let mut icon = None;

    let level_path = format!("{}level.dat", root_prefix);
    if let Some(buf) = read_zip_bytes_case_insensitive(archive, &level_path) {
        if let Ok(NbtTag::Compound(root)) = parse_root_nbt_with_header(&buf) {
            if let Some(NbtTag::String(n)) = root.get("LevelName") {
                name = n.clone();
            }
        }
    }

    let icon_path = format!("{}world_icon.jpeg", root_prefix);
    if let Some(buf) = read_zip_bytes_case_insensitive(archive, &icon_path) {
        let b64 = general_purpose::STANDARD.encode(&buf);
        icon = Some(format!("data:image/jpeg;base64,{}", b64));
    }

    Ok(PackagePreview {
        name,
        description,
        icon,
        kind: ImportTargetType::World.to_display_name().to_string(),
        version: None,
        size: 0,
        manifest: None,
        sub_packs: None,
        valid: true,
        invalid_reason: None,
    })
}

pub fn import_archive_optimized(
    file_path: &Path,
    options: &GamePathOptions,
    overwrite: bool,
) -> Result<()> {
    let file = File::open(file_path)?;
    let mut archive = ZipArchive::new(file)?;

    let scan = scan_archive(&mut archive)?;

    // World
    if scan.is_world {
        let dest = resolve_target_parent(options, "minecraftWorlds", false)
            .ok_or_else(|| anyhow::anyhow!("World path not found"))?
            .join(file_path.file_stem().unwrap());

        extract_pack_root(&mut archive, "", &dest)?;
        return Ok(());
    }

    let groups = group_packs(&scan);

    for (pack_type, packs) in groups {
        let dir_name = pack_type.to_dir_name();
        let is_shared = matches!(
            pack_type,
            ImportTargetType::ResourcePack |
            ImportTargetType::BehaviorPack
        );

        let parent = resolve_target_parent(options, dir_name, is_shared)
            .ok_or_else(|| anyhow::anyhow!("Target dir not found"))?;

        for pack in packs {
            let name = pack.manifest.header
                .as_ref()
                .and_then(|h| h.name.clone())
                .unwrap_or_else(|| "Imported_Pack".into());
            let pack_uuid = pack.manifest.header.as_ref().and_then(|h| h.uuid.clone());
            if pack_uuid.is_none() {
                return Err(anyhow::anyhow!("manifest.header.uuid 缺失，无法导入"));
            }

            let folder_name = match &pack_uuid {
                Some(uuid) => pack_folder_name(uuid),
                None => sanitize_filename(&strip_minecraft_formatting(&name)),
            };
            let mut dest = parent.join(&folder_name);

            if dest.exists() && !overwrite {
                return Err(anyhow::anyhow!("Pack exists: {}", folder_name));
            }

            extract_pack_root(&mut archive, &pack.root, &dest)?;
        }
    }

    Ok(())
}
