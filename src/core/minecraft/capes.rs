//! Local cape packs stored under the game's `capes` directory.
//!
//! Packs follow the v1 layout documented in `docs/CAPE_PACK_FORMAT.md`: an official-style
//! `manifest.json` plus a `cape.json` manifest with `texts/*.lang` localization, optional
//! per-cape icons and elytra textures. Archives (`.zip`/`.mccapes`) and single PNG textures are
//! imported by unpacking them into folder packs; the game-side loader keeps reading archives.
//! The earlier dev-only `pack.json` layout is retired without a compatibility shim.

use serde::Deserialize;
use serde_json::json;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use zip::ZipArchive;

use super::paths::{GamePathOptions, GameTargetDir, game_target_dirs, resolve_target_parent};

/// Pack format revision. The project is unreleased, so the redesigned manifest keeps v1.
pub const CAPE_PACK_FORMAT_VERSION: u32 = 1;
const MAX_PACK_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TEXTURE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_PACK_MEMBERS: usize = 256;
const MAX_CAPES_PER_PACK: usize = 64;
const MAX_NAME_CHARS: usize = 90;
const MAX_LOCALIZATION_CHARS: usize = 64;

/// Preview canvas and mannequin placeholder colors, matched to the launcher surface palette.
const PREVIEW_CANVAS_COLOR: image::Rgba<u8> = image::Rgba([40, 43, 48, 255]);
const MANNEQUIN_HEAD_COLOR: image::Rgba<u8> = image::Rgba([145, 153, 161, 255]);
const MANNEQUIN_TORSO_COLOR: image::Rgba<u8> = image::Rgba([93, 104, 113, 255]);

const ALLOWED_RARITIES: [&str; 5] = ["common", "uncommon", "rare", "epic", "legendary"];

#[derive(Debug, Clone)]
pub struct CapeInfo {
    pub name: String,
    pub texture_path: Option<PathBuf>,
    pub elytra_texture_path: Option<PathBuf>,
    pub preview_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct CapePackInfo {
    pub name: String,
    pub path: PathBuf,
    pub icon_path: Option<PathBuf>,
    pub capes: Vec<CapeInfo>,
}

#[derive(Deserialize)]
struct PackManifest {
    format_version: u32,
    header: PackHeader,
    modules: Vec<PackModule>,
}

#[derive(Deserialize)]
struct PackHeader {
    name: String,
    uuid: String,
    version: PackVersion,
}

#[derive(Deserialize)]
struct PackModule {
    #[serde(rename = "type")]
    module_type: String,
    uuid: String,
    version: PackVersion,
}

#[derive(Deserialize)]
struct CapeManifest {
    #[serde(default)]
    serialize_name: Option<String>,
    localization_name: String,
    capes: Vec<CapeEntry>,
}

#[derive(Deserialize)]
struct CapeEntry {
    localization_name: String,
    texture: String,
    #[serde(default)]
    elytra: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    rarity: Option<String>,
    #[serde(default, alias = "startDate")]
    start_date: Option<String>,
    #[serde(default, alias = "uuid")]
    item_uuid: Option<String>,
    #[serde(default)]
    persona_zip: Option<String>,
}

type PackVersion = [u16; 3];

fn safe_member(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 192
        && !name.contains(['\\', ':'])
        && !matches!(
            name.rsplit('/')
                .next()
                .unwrap_or_default()
                .to_ascii_uppercase()
                .as_str(),
            "CON"
                | "PRN"
                | "AUX"
                | "NUL"
                | "COM1"
                | "COM2"
                | "COM3"
                | "COM4"
                | "COM5"
                | "COM6"
                | "COM7"
                | "COM8"
                | "COM9"
                | "LPT1"
                | "LPT2"
                | "LPT3"
                | "LPT4"
                | "LPT5"
                | "LPT6"
                | "LPT7"
                | "LPT8"
                | "LPT9"
        )
        && name.split('/').all(|part| !matches!(part, "" | "." | ".."))
}

fn read_zip_member(
    archive: &mut ZipArchive<File>,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, String> {
    if !safe_member(name) {
        return Err(format!("披风包包含无效路径：{name}"));
    }
    let Ok(mut member) = archive.by_name(name) else {
        return Ok(None);
    };
    if member.size() > limit {
        return Err(format!("披风包文件过大：{name}"));
    }
    let mut bytes = Vec::new();
    member
        .by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("读取披风包文件失败 {name}：{error}"))?;
    if bytes.len() as u64 > limit {
        return Err(format!("披风包文件过大：{name}"));
    }
    Ok(Some(bytes))
}

fn read_folder_member(root: &Path, name: &str, limit: u64) -> Result<Option<Vec<u8>>, String> {
    if !safe_member(name) {
        return Err(format!("披风包包含无效路径：{name}"));
    }
    let path = root.join(name);
    if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file()) {
        return Ok(None);
    }
    if fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .len()
        > limit
    {
        return Err(format!("披风包文件过大：{name}"));
    }
    fs::read(path).map(Some).map_err(|error| error.to_string())
}

fn read_member(
    path: &Path,
    archive: &mut Option<ZipArchive<File>>,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, String> {
    match archive {
        Some(archive) => read_zip_member(archive, name, limit),
        None => read_folder_member(path, name, limit),
    }
}

fn has_member(
    path: &Path,
    archive: &mut Option<ZipArchive<File>>,
    name: &str,
    limit: u64,
) -> Result<bool, String> {
    if !safe_member(name) {
        return Err(format!("披风包包含无效路径：{name}"));
    }
    let size = match archive {
        Some(archive) => archive.by_name(name).ok().map(|member| member.size()),
        None => {
            let member = path.join(name);
            fs::symlink_metadata(member)
                .ok()
                .filter(|metadata| metadata.file_type().is_file())
                .map(|metadata| metadata.len())
        }
    };
    match size {
        Some(size) if size > 0 && size <= limit => Ok(true),
        Some(_) => Err(format!("披风包文件尺寸无效：{name}")),
        None => Ok(false),
    }
}

fn zip_member_names(archive: &mut ZipArchive<File>) -> Result<Vec<String>, String> {
    let mut names = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let member = archive
            .by_index(index)
            .map_err(|error| format!("读取披风包目录失败：{error}"))?;
        let name = member
            .name()
            .map_err(|error| format!("读取披风包目录失败：{error}"))?;
        names.push(name.into_owned());
    }
    Ok(names)
}

fn folder_member_names(root: &Path) -> Result<Vec<String>, String> {
    fn walk(dir: &Path, prefix: &str, names: &mut Vec<String>, depth: usize) -> Result<(), String> {
        if depth > 4 {
            return Ok(());
        }
        for entry in fs::read_dir(dir).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let relative = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                walk(&entry.path(), &relative, names, depth + 1)?;
            } else {
                names.push(relative);
            }
        }
        Ok(())
    }
    let mut names = Vec::new();
    walk(root, "", &mut names, 0)?;
    Ok(names)
}

fn is_cape_texture_size(width: u32, height: u32) -> bool {
    width > 0
        && width <= 2048
        && height > 0
        && height <= 2048
        && width % 64 == 0
        && height % 32 == 0
        && width / 64 == height / 32
}

fn is_icon_size(width: u32, height: u32) -> bool {
    width > 0 && width <= 2048 && height > 0 && height <= 2048
}

fn decode_png(bytes: &[u8]) -> Result<image::DynamicImage, String> {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("披风纹理解码失败：{error}"))
}

/// Validates and stores one pack PNG in the launcher cache, returning the cached path.
fn cache_png(
    pack_path: &Path,
    member_name: &str,
    bytes: &[u8],
    require_cape_ratio: bool,
) -> Result<PathBuf, String> {
    let image = decode_png(bytes)?;
    let (width, height) = (image.width(), image.height());
    let size_valid = if require_cape_ratio {
        is_cape_texture_size(width, height)
    } else {
        is_icon_size(width, height)
    };
    if !size_valid {
        return Err(format!("披风纹理尺寸无效：{width}x{height}"));
    }
    let metadata = fs::metadata(pack_path).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
    let key = format!(
        "{}|{member_name}|{}|{modified:?}",
        pack_path.display(),
        metadata.len()
    );
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let cache_dir = crate::utils::file_ops::cache_subdir("cape_previews");
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let target = cache_dir.join(format!("{:016x}.png", hasher.finish()));
    if !target.is_file() {
        fs::write(&target, bytes).map_err(|error| error.to_string())?;
    }
    Ok(target)
}

/// Renders the cached cape-on-mannequin preview used by pack and cape rows.
fn cache_preview(pack_path: &Path, member_name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let image = decode_png(bytes)?;
    let (width, height) = (image.width(), image.height());
    if !is_cape_texture_size(width, height) {
        return Err(format!("披风纹理尺寸无效：{width}x{height}"));
    }
    let metadata = fs::metadata(pack_path).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
    let key = format!(
        "{}|{member_name}|{}|{modified:?}|preview",
        pack_path.display(),
        metadata.len()
    );
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let cache_dir = crate::utils::file_ops::cache_subdir("cape_previews");
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let preview = cache_dir.join(format!("{:016x}.preview.png", hasher.finish()));
    if !preview.is_file() {
        let scale = width / 64;
        // Bedrock's cape bone is rotated 180°, so the outer face seen from behind is the
        // unwrap's front region at (1, 1)-(11, 17).
        let face =
            image::imageops::crop_imm(&image, scale, scale, 10 * scale, 16 * scale).to_image();
        let face = image::imageops::resize(&face, 34, 43, image::imageops::FilterType::Nearest);
        let mut canvas = image::RgbaImage::from_pixel(64, 64, PREVIEW_CANVAS_COLOR);
        for y in 2..17 {
            for x in 25..39 {
                canvas.put_pixel(x, y, MANNEQUIN_HEAD_COLOR);
            }
        }
        for y in 17..54 {
            for x in 10..54 {
                canvas.put_pixel(x, y, MANNEQUIN_TORSO_COLOR);
            }
        }
        image::imageops::overlay(&mut canvas, &face, 15, 17);
        canvas.save(&preview).map_err(|error| error.to_string())?;
    }
    Ok(preview)
}

/// Locale candidates for `texts/<locale>.lang`, from the most to the least specific.
fn lang_candidates(lang: &str) -> Vec<String> {
    let mut candidates = vec![lang.to_string()];
    if let Some((base, _region)) = lang.split_once(['-', '_']) {
        candidates.push(base.to_string());
    }
    candidates.push("en_US".to_string());
    candidates.dedup();
    candidates
}

fn parse_lang_content(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    for line in content.lines() {
        let line = line.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_string(), value.trim().to_string());
        }
    }
    map
}

fn lang_file_name(candidate: &str) -> String {
    format!("{}.lang", candidate.replace('-', "_"))
}

/// Reads the pack's `texts/*.lang` map, trying the requested locale, then `en_US`.
fn load_lang_map(
    path: &Path,
    archive: &mut Option<ZipArchive<File>>,
    lang: &str,
    prefix: &str,
) -> Result<HashMap<String, String>, String> {
    let available: Vec<String> = match archive {
        Some(archive) => zip_member_names(archive)?
            .into_iter()
            .filter_map(|name| name.strip_prefix(prefix).map(str::to_string))
            .filter(|name| name.starts_with("texts/") && name.ends_with(".lang"))
            .collect(),
        None => folder_member_names(path)?
            .into_iter()
            .filter(|name| name.starts_with("texts/") && name.ends_with(".lang"))
            .collect(),
    };
    if available.is_empty() {
        return Ok(HashMap::new());
    }
    let mut picked = None;
    for candidate in lang_candidates(lang) {
        let wanted = format!("texts/{}", lang_file_name(&candidate));
        if available
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&wanted))
        {
            picked = Some(wanted);
            break;
        }
    }
    let picked = picked.unwrap_or_else(|| available[0].clone());
    let picked = join_member(prefix, &picked);
    read_member(path, archive, &picked, MAX_MANIFEST_BYTES)?
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map(|content| parse_lang_content(&content))
        .ok_or_else(|| format!("披风包语言文件读取失败：{picked}"))
}

/// Joins an archive root prefix (folder pack inside a zip) with a member path.
fn join_member(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}{name}")
    }
}

/// Detects a single shared top-level folder inside a cape archive, e.g. `inner/manifest.json`.
fn zip_pack_root_prefix(archive: &mut ZipArchive<File>) -> Result<String, String> {
    if archive.by_name("manifest.json").is_ok() {
        return Ok(String::new());
    }
    let names = zip_member_names(archive)?;
    let mut candidate = None;
    for name in &names {
        let Some((root, rest)) = name.split_once('/') else {
            continue;
        };
        if rest == "manifest.json" {
            candidate = Some(root);
            break;
        }
    }
    let Some(root) = candidate else {
        return Ok(String::new());
    };
    let prefix = format!("{root}/");
    let all_under_root = names
        .iter()
        .filter(|name| !name.is_empty())
        .all(|name| name.starts_with(&prefix));
    if all_under_root {
        Ok(prefix)
    } else {
        Ok(String::new())
    }
}

fn is_localization_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_LOCALIZATION_CHARS
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn is_display_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && !name.chars().any(char::is_control)
}

fn validate_start_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 20 || !value.ends_with('Z') {
        return false;
    }
    let digits = |start: usize, end: usize| bytes[start..end].iter().all(u8::is_ascii_digit);
    digits(0, 4)
        && bytes[4] == b'-'
        && digits(5, 7)
        && bytes[7] == b'-'
        && digits(8, 10)
        && bytes[10] == b'T'
        && digits(11, 13)
        && bytes[13] == b':'
        && digits(14, 16)
        && bytes[16] == b':'
        && digits(17, 19)
}

fn validate_manifest(pack: &PackManifest) -> Result<(), String> {
    if pack.format_version != CAPE_PACK_FORMAT_VERSION {
        return Err(format!(
            "不支持的披风包格式版本 {}（需要 {}）",
            pack.format_version, CAPE_PACK_FORMAT_VERSION
        ));
    }
    if !is_display_name(&pack.header.name) {
        return Err("披风包需要有效的名称".into());
    }
    let header_uuid = uuid::Uuid::parse_str(&pack.header.uuid)
        .map_err(|_| "披风包需要有效的 UUID".to_string())?;
    if pack.modules.len() != 1 {
        return Err("披风包必须包含一个 cape_pack 模块".into());
    }
    let module = &pack.modules[0];
    if module.module_type != "cape_pack" {
        return Err(format!("披风包模块类型无效：{}", module.module_type));
    }
    let module_uuid =
        uuid::Uuid::parse_str(&module.uuid).map_err(|_| "披风包模块 UUID 无效".to_string())?;
    if header_uuid == module_uuid {
        return Err("披风包头与模块的 UUID 不能相同".into());
    }
    Ok(())
}

fn read_pack_at(
    path: &Path,
    archive: &mut Option<ZipArchive<File>>,
    lang: &str,
    prefix: &str,
) -> Result<CapePackInfo, String> {
    let member = |name: &str| join_member(prefix, name);
    let manifest_bytes = read_member(path, archive, &member("manifest.json"), MAX_MANIFEST_BYTES)?
        .ok_or("披风包缺少 manifest.json")?;
    let manifest: PackManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("无效的 manifest.json：{error}"))?;
    validate_manifest(&manifest)?;

    let cape_manifest_bytes = read_member(path, archive, &member("cape.json"), MAX_MANIFEST_BYTES)?
        .ok_or("披风包缺少 cape.json")?;
    let cape_manifest: CapeManifest = serde_json::from_slice(&cape_manifest_bytes)
        .map_err(|error| format!("无效的 cape.json：{error}"))?;
    if !is_localization_name(&cape_manifest.localization_name) {
        return Err("披风包本地化名称无效".into());
    }
    if let Some(serialize_name) = cape_manifest.serialize_name.as_deref()
        && !is_display_name(serialize_name)
    {
        return Err("披风包 serialize_name 无效".into());
    }
    if cape_manifest.capes.is_empty() || cape_manifest.capes.len() > MAX_CAPES_PER_PACK {
        return Err(format!("披风包需要 1 到 {MAX_CAPES_PER_PACK} 个披风"));
    }

    let lang_map = load_lang_map(path, archive, lang, prefix)?;
    let pack_key = cape_manifest.localization_name.as_str();
    let pack_name = lang_map
        .get(&format!("capepack.{pack_key}"))
        .filter(|name| !name.is_empty())
        .cloned()
        .unwrap_or_else(|| manifest.header.name.clone());

    let mut seen_capes = HashSet::new();
    let mut capes = Vec::with_capacity(cape_manifest.capes.len());
    let mut pack_icon_path = None;
    for entry in &cape_manifest.capes {
        if !is_localization_name(&entry.localization_name)
            || !seen_capes.insert(&entry.localization_name)
        {
            return Err("披风本地化名称无效或重复".into());
        }
        if let Some(rarity) = entry.rarity.as_deref()
            && !ALLOWED_RARITIES.contains(&rarity)
        {
            return Err(format!("披风稀有度无效：{rarity}"));
        }
        if let Some(start_date) = entry.start_date.as_deref()
            && !validate_start_date(start_date)
        {
            return Err(format!("披风发布时间无效：{start_date}"));
        }
        if let Some(item_uuid) = entry.item_uuid.as_deref()
            && uuid::Uuid::parse_str(item_uuid).is_err()
        {
            return Err(format!("披风物品 UUID 无效：{item_uuid}"));
        }

        let texture_name = entry.texture.as_str();
        let texture_bytes = read_member(path, archive, &member(texture_name), MAX_TEXTURE_BYTES)?
            .ok_or_else(|| format!("披风缺少纹理：{}", entry.localization_name))?;
        let texture_path = cache_png(path, texture_name, &texture_bytes, true)?;
        let preview_path = match &entry.icon {
            Some(icon_name) => {
                let icon_bytes = read_member(path, archive, &member(icon_name), MAX_TEXTURE_BYTES)?
                    .ok_or_else(|| format!("披风图标缺失：{icon_name}"))?;
                Some(cache_png(path, icon_name, &icon_bytes, false)?)
            }
            None => Some(cache_preview(path, texture_name, &texture_bytes)?),
        };
        if pack_icon_path.is_none() {
            pack_icon_path = preview_path.clone();
        }
        let elytra_texture_path = match &entry.elytra {
            Some(elytra_name) => {
                let elytra_bytes =
                    read_member(path, archive, &member(elytra_name), MAX_TEXTURE_BYTES)?
                        .ok_or_else(|| format!("披风鞘翅纹理缺失：{elytra_name}"))?;
                Some(cache_png(path, elytra_name, &elytra_bytes, true)?)
            }
            None => None,
        };
        if entry.persona_zip.is_some() {
            let present = entry
                .persona_zip
                .as_deref()
                .map(|name| has_member(path, archive, &member(name), MAX_PACK_BYTES))
                .transpose()?
                .unwrap_or(false);
            if !present {
                return Err(format!(
                    "披风 Persona 数据缺失：{}",
                    entry.localization_name
                ));
            }
        }

        let cape_name = lang_map
            .get(&format!("cape.{pack_key}.{}", entry.localization_name))
            .filter(|name| !name.is_empty())
            .cloned()
            .unwrap_or_else(|| entry.localization_name.clone());
        capes.push(CapeInfo {
            name: cape_name,
            texture_path: Some(texture_path),
            elytra_texture_path,
            preview_path,
        });
    }

    Ok(CapePackInfo {
        name: pack_name,
        path: path.to_path_buf(),
        icon_path: pack_icon_path,
        capes,
    })
}

fn open_zip_pack(path: &Path) -> Result<Option<ZipArchive<File>>, String> {
    if path.is_dir() {
        return Ok(None);
    }
    if fs::metadata(path).map_err(|error| error.to_string())?.len() > MAX_PACK_BYTES {
        return Err("披风包超过 64 MiB".into());
    }
    let archive = ZipArchive::new(File::open(path).map_err(|error| error.to_string())?)
        .map_err(|error| format!("无效的披风 ZIP 包：{error}"))?;
    if archive.len() > MAX_PACK_MEMBERS {
        return Err("披风包文件数量过多".into());
    }
    Ok(Some(archive))
}

pub fn read_pack(path: &Path, lang: &str) -> Result<CapePackInfo, String> {
    let mut archive = open_zip_pack(path)?;
    let prefix = match archive.as_mut() {
        Some(archive) => zip_pack_root_prefix(archive)?,
        None => String::new(),
    };
    read_pack_at(path, &mut archive, lang, &prefix)
}

pub fn list_packs(options: &GamePathOptions, lang: &str) -> Vec<CapePackInfo> {
    let mut packs = Vec::new();
    for root in game_target_dirs(options, GameTargetDir::Capes) {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir()
                && !path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("zip") || ext.eq_ignore_ascii_case("mccapes")
                    })
            {
                continue;
            }
            match read_pack(&path, lang) {
                Ok(pack) => packs.push(pack),
                Err(error) => tracing::warn!(path = %path.display(), %error, "invalid cape pack"),
            }
        }
    }
    packs
}

/// Strips a single shared top-level directory from archive member paths.
fn strip_common_root(names: &[&str]) -> Vec<String> {
    let first_component = names.iter().filter_map(|name| name.split('/').next());
    let root = match first_component.clone().next() {
        Some(root) if root.is_empty() => {
            return names.iter().map(|name| name.to_string()).collect();
        }
        Some(root) => root,
        None => return Vec::new(),
    };
    let all_under_root = names
        .iter()
        .all(|name| name.starts_with(root) && name[root.len()..].starts_with('/'));
    if all_under_root && names.iter().any(|name| name[root.len()..].contains('/')) {
        names
            .iter()
            .map(|name| name[root.len() + 1..].to_string())
            .collect()
    } else {
        names.iter().map(|name| name.to_string()).collect()
    }
}

fn extract_zip_pack(source: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(source).map_err(|error| error.to_string())?;
    let mut archive =
        ZipArchive::new(file).map_err(|error| format!("无效的披风 ZIP 包：{error}"))?;
    if archive.len() > MAX_PACK_MEMBERS {
        return Err("披风包文件数量过多".into());
    }
    let raw_names = zip_member_names(&mut archive)?;
    let names = strip_common_root(&raw_names.iter().map(String::as_str).collect::<Vec<_>>());
    let mut total_bytes = 0u64;
    for (relative, raw_name) in names.iter().zip(raw_names.iter()) {
        if relative.is_empty() {
            continue;
        }
        if !safe_member(relative) {
            return Err(format!("披风包包含无效路径：{relative}"));
        }
        let member = archive
            .by_name(raw_name)
            .map_err(|error| format!("读取披风包文件失败：{error}"))?;
        if member.is_dir() {
            continue;
        }
        total_bytes = total_bytes.saturating_add(member.size());
        if total_bytes > MAX_PACK_BYTES || member.size() > MAX_PACK_BYTES {
            return Err("披风包超过 64 MiB".into());
        }
        let destination = target.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut bytes = Vec::new();
        member
            .take(MAX_PACK_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("读取披风包文件失败 {relative}：{error}"))?;
        fs::write(&destination, bytes).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn import_archive(options: &GamePathOptions, source: &Path) -> Result<PathBuf, String> {
    read_pack(source, "en_US")?;
    let target_dir = resolve_target_parent(options, "capes", true).ok_or("无法确定披风包目录")?;
    fs::create_dir_all(&target_dir).map_err(|error| error.to_string())?;
    let stem = source
        .file_stem()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or("无效的披风包文件名")?;
    let target = target_dir.join(stem);
    if target.exists() {
        return Err(format!("披风包已存在：{}", target.display()));
    }
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;
    if let Err(error) = extract_zip_pack(source, &target) {
        let _ = fs::remove_dir_all(&target);
        return Err(error);
    }
    if let Err(error) = read_pack(&target, "en_US") {
        let _ = fs::remove_dir_all(&target);
        return Err(format!("披风包导入后校验失败：{error}"));
    }
    Ok(target)
}

fn sanitize_pack_name(name: &str) -> String {
    let trimmed = name.trim();
    if is_display_name(trimmed) {
        trimmed.to_string()
    } else {
        "Local Cape".to_string()
    }
}

fn write_pack_file(path: &Path, relative: &str, content: &str) -> Result<(), String> {
    let target = path.join(relative);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::write(target, content).map_err(|error| error.to_string())
}

/// Wraps a single cape PNG into a folder pack so imports always land as standard packs.
fn import_png(options: &GamePathOptions, source: &Path) -> Result<PathBuf, String> {
    let bytes = {
        let metadata = fs::metadata(source).map_err(|error| error.to_string())?;
        if metadata.len() > MAX_TEXTURE_BYTES {
            return Err("披风纹理超过 8 MiB".into());
        }
        fs::read(source).map_err(|error| error.to_string())?
    };
    let image = decode_png(&bytes)?;
    if !is_cape_texture_size(image.width(), image.height()) {
        return Err(format!(
            "披风纹理尺寸无效：{}x{}（需要 64x32 的整数倍）",
            image.width(),
            image.height()
        ));
    }
    let name = source
        .file_stem()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(sanitize_pack_name)
        .ok_or("无效的披风文件名")?;
    let target_dir = resolve_target_parent(options, "capes", true).ok_or("无法确定披风包目录")?;
    fs::create_dir_all(&target_dir).map_err(|error| error.to_string())?;
    let folder = source
        .file_stem()
        .and_then(|value| value.to_str())
        .and_then(|stem| {
            let safe: String = stem
                .chars()
                .map(|character| {
                    if character.is_ascii_alphanumeric()
                        || matches!(character, '-' | '_' | '(' | ')' | ' ' | '.')
                            && !matches!(character, '.' | ' ')
                    {
                        character
                    } else {
                        '_'
                    }
                })
                .collect();
            let safe = safe.trim_matches(['.', ' ']);
            (!safe.is_empty()).then(|| safe.to_string())
        })
        .unwrap_or_else(|| "cape".to_string());
    let target = target_dir.join(folder);
    if target.exists() {
        return Err(format!("披风包已存在：{}", target.display()));
    }
    fs::create_dir_all(&target).map_err(|error| error.to_string())?;

    let manifest = json!({
        "format_version": CAPE_PACK_FORMAT_VERSION,
        "header": {
            "name": name,
            "uuid": uuid::Uuid::new_v4().to_string(),
            "version": [1, 0, 0],
        },
        "modules": [{
            "type": "cape_pack",
            "uuid": uuid::Uuid::new_v4().to_string(),
            "version": [1, 0, 0],
        }],
    });
    let cape_manifest = json!({
        "serialize_name": name,
        "localization_name": "local",
        "capes": [{
            "localization_name": "cape",
            "texture": "cape.png",
        }],
    });
    let lang_content = format!("capepack.local={name}\ncape.local.cape={name}\n");
    let result = (|| -> Result<(), String> {
        write_pack_file(
            &target,
            "manifest.json",
            &serde_json::to_string_pretty(&manifest).map_err(|error| error.to_string())?,
        )?;
        write_pack_file(
            &target,
            "cape.json",
            &serde_json::to_string_pretty(&cape_manifest).map_err(|error| error.to_string())?,
        )?;
        write_pack_file(&target, "cape.png", "")?;
        fs::write(target.join("cape.png"), &bytes).map_err(|error| error.to_string())?;
        write_pack_file(&target, "texts/en_US.lang", &lang_content)?;
        write_pack_file(&target, "texts/zh_CN.lang", &lang_content)?;
        read_pack(&target, "en_US").map(|_| ())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&target);
        return Err(error);
    }
    Ok(target)
}

pub fn import_pack(options: &GamePathOptions, source: &Path) -> Result<PathBuf, String> {
    match source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("zip") | Some("mccapes") => import_archive(options, source),
        Some("png") => import_png(options, source),
        _ => Err("仅支持 .mccapes/.zip 披风包或 PNG 披风纹理".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GenericImageView as _, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;
    use std::io::Write;
    use zip::{ZipWriter, write::SimpleFileOptions};

    fn cape_png() -> Vec<u8> {
        let mut image = RgbaImage::from_pixel(64, 32, Rgba([0, 0, 0, 0]));
        for y in 1..17 {
            for x in 1..11 {
                image.put_pixel(x, y, Rgba([0, 200, 0, 255]));
            }
            for x in 12..22 {
                image.put_pixel(x, y, Rgba([200, 0, 0, 255]));
            }
        }
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut png, ImageFormat::Png)
            .ok();
        png.into_inner()
    }

    fn manifest_json() -> String {
        json!({
            "format_version": 1,
            "header": {"name": "Test Capes", "uuid": "11111111-2222-4333-8444-555555555555", "version": [1, 0, 0]},
            "modules": [{
                "type": "cape_pack",
                "uuid": "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
                "version": [1, 0, 0],
            }],
        })
        .to_string()
    }

    fn cape_manifest_json() -> String {
        json!({
            "serialize_name": "Test Capes",
            "localization_name": "test_capes",
            "capes": [{"localization_name": "green", "texture": "cape.png"}],
        })
        .to_string()
    }

    fn write_pack_folder(dir: &Path) {
        fs::create_dir_all(dir.join("texts")).ok();
        fs::write(dir.join("manifest.json"), manifest_json()).ok();
        fs::write(dir.join("cape.json"), cape_manifest_json()).ok();
        fs::write(dir.join("cape.png"), cape_png()).ok();
        fs::write(
            dir.join("texts/en_US.lang"),
            "capepack.test_capes=Test Capes\ncape.test_capes.green=Green Cape\n",
        )
        .ok();
        fs::write(
            dir.join("texts/zh_CN.lang"),
            "cape.test_capes.green=绿色披风\n",
        )
        .ok();
    }

    fn write_pack_zip(path: &Path, nested: bool) {
        let mut zip = ZipWriter::new(File::create(path).expect("zip file"));
        let options = SimpleFileOptions::default();
        let prefix = if nested { "inner/" } else { "" };
        let mut members: Vec<(&str, Vec<u8>)> = vec![
            ("manifest.json", manifest_json().into_bytes()),
            ("cape.json", cape_manifest_json().into_bytes()),
            ("cape.png", cape_png()),
            (
                "texts/en_US.lang",
                "capepack.test_capes=Test Capes\ncape.test_capes.green=Green Cape\n"
                    .as_bytes()
                    .to_vec(),
            ),
        ];
        for (name, content) in members.drain(..) {
            zip.start_file(format!("{prefix}{name}"), options)
                .expect("zip member");
            zip.write_all(&content).expect("zip write");
        }
        zip.finish().expect("zip finish");
    }

    #[test]
    fn folder_pack_resolves_localized_names() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let pack = dir.path().join("Sample");
        write_pack_folder(&pack);

        let localized = read_pack(&pack, "zh-CN")?;
        assert_eq!(localized.name, "Test Capes");
        assert_eq!(localized.capes[0].name, "绿色披风");

        let fallback = read_pack(&pack, "de-DE")?;
        assert_eq!(fallback.name, "Test Capes");
        assert_eq!(fallback.capes[0].name, "Green Cape");
        assert!(localized.icon_path.is_some());
        assert!(localized.capes[0].elytra_texture_path.is_none());
        Ok(())
    }

    #[test]
    fn zip_pack_with_inner_root_reads_texture_and_preview() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("sample.mccapes");
        write_pack_zip(&path, true);

        let pack = read_pack(&path, "en-US")?;
        assert_eq!(pack.name, "Test Capes");
        assert_eq!(pack.capes.len(), 1);
        let preview = image::open(
            pack.capes[0]
                .preview_path
                .as_ref()
                .ok_or("missing preview")?,
        )?;
        assert_eq!(preview.get_pixel(32, 33), Rgba([0, 200, 0, 255]));
        assert_eq!(preview.get_pixel(32, 10), Rgba([145, 153, 161, 255]));
        Ok(())
    }

    #[test]
    fn packs_missing_v2_manifest_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let legacy = dir.path().join("legacy.mccapes");
        let mut zip = ZipWriter::new(File::create(&legacy)?);
        let options = SimpleFileOptions::default();
        zip.start_file("pack.json", options)?;
        zip.write_all(br#"{"format_version":1,"id":"11111111-2222-4333-8444-555555555555","name":"Legacy","capes":[{"id":"a","name":"A","texture":"cape.png"}]}"#)?;
        zip.start_file("cape.png", options)?;
        zip.write_all(&cape_png())?;
        zip.finish()?;

        assert!(read_pack(&legacy, "en_US").is_err());
        Ok(())
    }

    #[test]
    fn zip_extraction_strips_inner_root_and_reads_as_pack() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let archive_path = dir.path().join("packed.zip");
        write_pack_zip(&archive_path, true);
        let target = dir.path().join("extracted");

        extract_zip_pack(&archive_path, &target)?;
        let pack = read_pack(&target, "en-US")?;
        assert_eq!(pack.name, "Test Capes");
        assert!(target.join("manifest.json").is_file());
        assert!(!target.join("inner").exists());
        Ok(())
    }

    #[test]
    fn flat_zip_archives_extract_without_stripping() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let archive_path = dir.path().join("flat.zip");
        write_pack_zip(&archive_path, false);
        let target = dir.path().join("flat");

        extract_zip_pack(&archive_path, &target)?;
        assert_eq!(read_pack(&target, "en-US")?.capes.len(), 1);
        Ok(())
    }

    #[test]
    fn strip_common_root_only_removes_shared_folders() {
        assert_eq!(
            strip_common_root(&["inner/manifest.json", "inner/cape.png"]),
            vec!["manifest.json".to_string(), "cape.png".to_string()]
        );
        assert_eq!(
            strip_common_root(&["manifest.json", "inner/cape.png"]),
            vec!["manifest.json".to_string(), "inner/cape.png".to_string()]
        );
        assert_eq!(
            strip_common_root(&["manifest.json", "cape.json"]),
            vec!["manifest.json".to_string(), "cape.json".to_string()]
        );
    }

    #[test]
    fn start_date_requires_iso_utc() {
        assert!(validate_start_date("2020-06-23T08:09:10Z"));
        assert!(!validate_start_date("2020-06-23 08:09:10Z"));
        assert!(!validate_start_date("2020-06-23T08:09:10"));
        assert!(!validate_start_date("2020-6-23T08:09:10Z"));
    }

    #[test]
    fn localization_names_are_restricted() {
        assert!(is_localization_name("green_cape-2"));
        assert!(!is_localization_name(""));
        assert!(!is_localization_name("绿"));
        assert!(!is_localization_name(&"a".repeat(65)));
    }
}
