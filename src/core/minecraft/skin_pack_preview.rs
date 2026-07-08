use anyhow::{Context as _, Result};
use image::imageops::{self, FilterType};
use image::{DynamicImage, GenericImageView as _, ImageFormat, ImageReader, RgbaImage};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const SKIN_PREVIEW_CANVAS_SIZE: u32 = 64;
const SKIN_PREVIEW_HEAD_OFFSET: u32 = 0;
const SKIN_PREVIEW_SCALE: u32 = 8;

pub(crate) fn generate_skin_preview(texture_path: &Path) -> Result<PathBuf> {
    let output_path = preview_cache_path(texture_path, "head-front-v2")?;
    if output_path.is_file() {
        return Ok(output_path);
    }

    let image = open_skin_texture(texture_path)?;
    let preview = build_skin_preview_image(&image)?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(&output_path)?;
    DynamicImage::ImageRgba8(preview).write_to(&mut file, ImageFormat::Png)?;
    Ok(output_path)
}

pub(crate) fn open_skin_texture(texture_path: &Path) -> Result<DynamicImage> {
    ImageReader::open(texture_path)
        .with_context(|| format!("读取皮肤纹理失败: {}", texture_path.display()))?
        .with_guessed_format()
        .with_context(|| format!("识别皮肤纹理格式失败: {}", texture_path.display()))?
        .decode()
        .with_context(|| format!("解码皮肤纹理失败: {}", texture_path.display()))
}

pub(crate) fn skin_texture_dimensions(texture_path: &Path) -> Result<(u32, u32)> {
    ImageReader::open(texture_path)
        .with_context(|| format!("读取皮肤纹理失败: {}", texture_path.display()))?
        .with_guessed_format()
        .with_context(|| format!("识别皮肤纹理格式失败: {}", texture_path.display()))?
        .into_dimensions()
        .with_context(|| format!("读取皮肤纹理尺寸失败: {}", texture_path.display()))
}

fn preview_cache_path(texture_path: &Path, variant: &str) -> Result<PathBuf> {
    let mut hasher = DefaultHasher::new();
    variant.hash(&mut hasher);
    texture_path.hash(&mut hasher);
    let metadata = fs::metadata(texture_path)?;
    metadata.len().hash(&mut hasher);
    if let Ok(modified) = metadata.modified()
        && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
    {
        duration.as_nanos().hash(&mut hasher);
    }
    let file_name = format!("{:016x}.png", hasher.finish());
    Ok(crate::utils::file_ops::cache_subdir("skin_previews").join(file_name))
}

fn build_skin_preview_image(source: &DynamicImage) -> Result<RgbaImage> {
    let (width, height) = source.dimensions();
    if width < 64 || height < 32 {
        anyhow::bail!("skin texture is too small: {width}x{height}");
    }
    let unit = (width / 64).max(1);
    let mut canvas = RgbaImage::new(SKIN_PREVIEW_CANVAS_SIZE, SKIN_PREVIEW_CANVAS_SIZE);

    draw_part(
        &mut canvas,
        source,
        SkinPart::new(
            8,
            8,
            8,
            8,
            SKIN_PREVIEW_HEAD_OFFSET,
            SKIN_PREVIEW_HEAD_OFFSET,
        ),
        unit,
    )?;
    draw_part(
        &mut canvas,
        source,
        SkinPart::new(
            40,
            8,
            8,
            8,
            SKIN_PREVIEW_HEAD_OFFSET,
            SKIN_PREVIEW_HEAD_OFFSET,
        ),
        unit,
    )?;
    Ok(canvas)
}

#[derive(Clone, Copy)]
struct SkinPart {
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    dest_x: u32,
    dest_y: u32,
}

impl SkinPart {
    const fn new(
        source_x: u32,
        source_y: u32,
        source_width: u32,
        source_height: u32,
        dest_x: u32,
        dest_y: u32,
    ) -> Self {
        Self {
            source_x,
            source_y,
            source_width,
            source_height,
            dest_x,
            dest_y,
        }
    }
}

fn draw_part(
    canvas: &mut RgbaImage,
    source: &DynamicImage,
    part: SkinPart,
    unit: u32,
) -> Result<()> {
    let crop_x = part.source_x.saturating_mul(unit);
    let crop_y = part.source_y.saturating_mul(unit);
    let crop_width = part.source_width.saturating_mul(unit);
    let crop_height = part.source_height.saturating_mul(unit);
    let (source_width, source_height) = source.dimensions();
    if crop_x.saturating_add(crop_width) > source_width
        || crop_y.saturating_add(crop_height) > source_height
    {
        anyhow::bail!("skin atlas region is out of bounds");
    }

    let crop = imageops::crop_imm(source, crop_x, crop_y, crop_width, crop_height).to_image();
    let resized = imageops::resize(
        &crop,
        part.source_width.saturating_mul(SKIN_PREVIEW_SCALE),
        part.source_height.saturating_mul(SKIN_PREVIEW_SCALE),
        FilterType::Nearest,
    );
    imageops::overlay(
        canvas,
        &resized,
        i64::from(part.dest_x),
        i64::from(part.dest_y),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView as _, ImageBuffer, Rgb, Rgba};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn build_skin_preview_accepts_classic_skin_size() {
        let source =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(64, 32, Rgba([128, 64, 32, 255])));

        let preview = build_skin_preview_image(&source)
            .unwrap_or_else(|error| panic!("preview should render: {error}"));

        assert_eq!(
            preview.dimensions(),
            (SKIN_PREVIEW_CANVAS_SIZE, SKIN_PREVIEW_CANVAS_SIZE)
        );
    }

    #[test]
    fn open_skin_texture_uses_file_signature_before_extension() {
        let source = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(64, 32, Rgb([128, 64, 32])));
        let path = std::env::temp_dir().join(format!(
            "bmcbl-skin-texture-{}-{}.png",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or_default()
        ));
        let mut file = fs::File::create(&path)
            .unwrap_or_else(|error| panic!("test texture should be created: {error}"));
        source
            .write_to(&mut file, ImageFormat::Jpeg)
            .unwrap_or_else(|error| panic!("jpeg test texture should be written: {error}"));
        drop(file);

        let decoded = open_skin_texture(&path).unwrap_or_else(|error| {
            panic!("jpeg content with png extension should decode: {error}")
        });

        assert_eq!(decoded.dimensions(), (64, 32));
        fs::remove_file(&path)
            .unwrap_or_else(|error| panic!("test texture should be removed: {error}"));
    }
}
