use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use zip::ZipArchive;

use super::paths::{GamePathOptions, GameTargetDir, game_target_dirs, resolve_target_parent};

const MAX_PACK_BYTES: u64 = 64 * 1024 * 1024;
const MAX_TEXTURE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone)]
pub struct CapeInfo {
    pub name: String,
    pub texture_path: Option<PathBuf>,
    pub preview_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct CapePackInfo {
    pub name: String,
    pub path: PathBuf,
    pub capes: Vec<CapeInfo>,
}

#[derive(Deserialize)]
struct PackManifest {
    format_version: u32,
    id: String,
    name: String,
    version: Option<String>,
    capes: Vec<PackCape>,
}

#[derive(Deserialize)]
struct PackCape {
    id: String,
    name: String,
    texture: Option<String>,
    persona_zip: Option<String>,
}

#[derive(Default, Deserialize)]
struct SingleCapeManifest {
    name: Option<String>,
}

fn safe_member(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 192
        && !name.contains(['\\', ':'])
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
            fs::symlink_metadata(member).ok().filter(|metadata| metadata.file_type().is_file()).map(|metadata| metadata.len())
        }
    };
    match size {
        Some(size) if size > 0 && size <= limit => Ok(true),
        Some(_) => Err(format!("披风包文件尺寸无效：{name}")),
        None => Ok(false),
    }
}

fn cache_texture(path: &Path, name: &str, bytes: &[u8]) -> Result<(PathBuf, PathBuf), String> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| format!("披风纹理解码失败：{error}"))?;
    let (width, height) = (image.width(), image.height());
    if width == 0
        || width > 2048
        || height == 0
        || height > 2048
        || width % 64 != 0
        || height % 32 != 0
        || width / 64 != height / 32
    {
        return Err(format!("披风纹理尺寸无效：{width}x{height}"));
    }
    let metadata = fs::metadata(path).map_err(|error| error.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
    let key = format!("{}|{name}|{}|{modified:?}", path.display(), metadata.len());
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let cache_dir = crate::utils::file_ops::cache_subdir("cape_previews");
    fs::create_dir_all(&cache_dir).map_err(|error| error.to_string())?;
    let target = cache_dir.join(format!("{:016x}.png", hasher.finish()));
    if !target.is_file() {
        fs::write(&target, bytes).map_err(|error| error.to_string())?;
    }
    let preview = target.with_extension("preview.png");
    if !preview.is_file() {
        let scale = width / 64;
        let face = image::imageops::crop_imm(&image, scale, scale, 10 * scale, 16 * scale).to_image();
        let face = image::imageops::resize(&face, 34, 43, image::imageops::FilterType::Nearest);
        let mut canvas = image::RgbaImage::from_pixel(64, 64, image::Rgba([40, 43, 48, 255]));
        for y in 2..17 {
            for x in 25..39 { canvas.put_pixel(x, y, image::Rgba([145, 153, 161, 255])); }
        }
        for y in 17..54 {
            for x in 10..54 { canvas.put_pixel(x, y, image::Rgba([93, 104, 113, 255])); }
        }
        image::imageops::overlay(&mut canvas, &face, 15, 17);
        canvas.save(&preview).map_err(|error| error.to_string())?;
    }
    Ok((target, preview))
}

pub fn read_pack(path: &Path) -> Result<CapePackInfo, String> {
    let mut archive = if path.is_dir() {
        None
    } else {
        if fs::metadata(path).map_err(|error| error.to_string())?.len() > MAX_PACK_BYTES {
            return Err("披风包超过 64 MiB".into());
        }
        let archive = ZipArchive::new(File::open(path).map_err(|error| error.to_string())?)
            .map_err(|error| format!("无效的披风 ZIP 包：{error}"))?;
        if archive.len() > 256 {
            return Err("披风包文件数量过多".into());
        }
        Some(archive)
    };
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Cape");
    let manifest = read_member(path, &mut archive, "pack.json", MAX_MANIFEST_BYTES)?;
    let (name, entries) = if let Some(manifest) = manifest {
        let pack: PackManifest = serde_json::from_slice(&manifest)
            .map_err(|error| format!("无效的 pack.json：{error}"))?;
        if pack.format_version != 1 || pack.capes.is_empty() || pack.capes.len() > 64 {
            return Err("不支持的披风包格式或披风数量".into());
        }
        if uuid::Uuid::parse_str(&pack.id).is_err() || pack.name.is_empty() || pack.name.chars().count() > 90 {
            return Err("披风包需要有效的固定 UUID 和名称".into());
        }
        if let Some(version) = pack.version.as_deref() {
            let parts = version.split('.').collect::<Vec<_>>();
            if parts.len() != 3 || parts.iter().any(|part| part.parse::<u16>().is_err()) {
                return Err("披风包版本必须为 major.minor.patch".into());
            }
        }
        let mut ids = HashSet::new();
        for cape in &pack.capes {
            if cape.id.is_empty() || cape.id.len() > 64 || !cape.id.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte)) || !ids.insert(&cape.id) {
                return Err("披风 ID 无效或重复".into());
            }
        }
        (pack.name, pack.capes)
    } else {
        let single = read_member(path, &mut archive, "cape.json", MAX_MANIFEST_BYTES)?
            .map(|bytes| {
                serde_json::from_slice::<SingleCapeManifest>(&bytes)
                    .map_err(|error| format!("无效的 cape.json：{error}"))
            })
            .transpose()?
            .unwrap_or_default();
        let name = single.name.unwrap_or_else(|| stem.to_string());
        (
            name.clone(),
            vec![PackCape {
                id: "cape".into(),
                name,
                texture: Some("cape.png".into()),
                persona_zip: Some("persona.zip".into()),
            }],
        )
    };
    let mut capes = Vec::with_capacity(entries.len());
    for entry in entries {
        let texture = entry
            .texture
            .as_deref()
            .map(|name| read_member(path, &mut archive, name, MAX_TEXTURE_BYTES))
            .transpose()?
            .flatten();
        let persona = entry
            .persona_zip
            .as_deref()
            .map(|name| has_member(path, &mut archive, name, MAX_PACK_BYTES))
            .transpose()?
            .unwrap_or(false);
        if texture.is_none() && !persona {
            return Err(format!("披风缺少纹理或 Persona 数据：{}", entry.name));
        }
        let texture_paths = texture
            .as_deref()
            .map(|bytes| cache_texture(path, entry.texture.as_deref().unwrap_or("cape.png"), bytes))
            .transpose()?;
        capes.push(CapeInfo {
            name: entry.name,
            texture_path: texture_paths.as_ref().map(|(path, _)| path.clone()),
            preview_path: texture_paths.map(|(_, path)| path),
        });
    }
    Ok(CapePackInfo {
        name,
        path: path.to_path_buf(),
        capes,
    })
}

pub fn list_packs(options: &GamePathOptions) -> Vec<CapePackInfo> {
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
            match read_pack(&path) {
                Ok(pack) => packs.push(pack),
                Err(error) => tracing::warn!(path = %path.display(), %error, "invalid cape pack"),
            }
        }
    }
    packs
}

pub fn import_pack(options: &GamePathOptions, source: &Path) -> Result<PathBuf, String> {
    if !source
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("mccapes"))
    {
        return Err("仅支持 .mccapes 披风包".into());
    }
    read_pack(source)?;
    let target_dir = resolve_target_parent(options, "capes", true).ok_or("无法确定披风包目录")?;
    fs::create_dir_all(&target_dir).map_err(|error| error.to_string())?;
    let name = source.file_name().ok_or("无效的披风包文件名")?;
    let target = target_dir.join(name);
    let mut input = File::open(source).map_err(|error| error.to_string())?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|error| format!("无法创建披风包（可能已存在）：{error}"))?;
    if let Err(error) = std::io::copy(&mut input, &mut output) {
        drop(output);
        let _ = fs::remove_file(&target);
        return Err(format!("复制披风包失败：{error}"));
    }
    output.flush().map_err(|error| error.to_string())?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GenericImageView as _, ImageFormat, Rgba, RgbaImage};
    use std::io::Cursor;
    use zip::{ZipWriter, write::SimpleFileOptions};

    #[test]
    fn mccapes_pack_reads_texture_and_outer_back_preview() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("sample.mccapes");
        let mut image = RgbaImage::from_pixel(64, 32, Rgba([0, 0, 0, 0]));
        for y in 1..17 {
            for x in 1..11 { image.put_pixel(x, y, Rgba([0, 200, 0, 255])); }
            for x in 12..22 { image.put_pixel(x, y, Rgba([200, 0, 0, 255])); }
        }
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image).write_to(&mut png, ImageFormat::Png)?;
        let mut zip = ZipWriter::new(File::create(&path)?);
        let options = SimpleFileOptions::default();
        zip.start_file("pack.json", options)?;
        zip.write_all(br#"{"format_version":1,"id":"11111111-2222-4333-8444-555555555555","name":"Test Capes","capes":[{"id":"green","name":"Green","texture":"cape.png"}]}"#)?;
        zip.start_file("cape.png", options)?;
        zip.write_all(png.get_ref())?;
        zip.finish()?;

        let pack = read_pack(&path)?;
        assert_eq!(pack.name, "Test Capes");
        assert_eq!(pack.capes.len(), 1);
        let preview = image::open(pack.capes[0].preview_path.as_ref().ok_or("missing preview")?)?;
        assert_eq!(preview.get_pixel(32, 33), Rgba([0, 200, 0, 255]));
        Ok(())
    }
}
