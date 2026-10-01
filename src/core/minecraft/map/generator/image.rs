//! Bounded image decoding and Flat or luminance-relief image structure export.

use std::path::{Path, PathBuf};

use bedrock_voxel::{
    FlatImageInput, FlatImageOptions, ReliefImageOptions, default_block_candidates,
    flat_block_plan_with_progress, relief_block_plan_with_progress,
};
use bedrock_world::{editor::BlockPlacementPlan, structure::McStructureFile, surface::CancelFlag};
use image::{ImageReader, Limits, RgbaImage, imageops};

use crate::core::minecraft::map::image::MapResample;
use crate::tasks::{runtime, task_manager};

pub use bedrock_voxel::Dithering;
pub use bedrock_voxel::ReliefImageFill;

const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;

/// Whether the whole image is stretched or its center is cropped to the target ratio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageFit {
    /// Preserve every source pixel, stretching when aspect ratios differ.
    Stretch,
    /// Preserve the source aspect ratio and remove equal margins from opposite edges.
    CenterCrop,
}

/// Vertical treatment for an image converted into blocks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageBlockHeight {
    /// Place one image plane at local Y=0.
    Flat,
    /// Convert linear-light image luminance into stepped local height.
    ///
    /// This creates a useful 2.5D relief, but does not predict Bedrock map-item shadow colors.
    LuminanceRelief {
        /// Maximum number of vertical levels, including the base level.
        max_height: u16,
        /// Keep only surface blocks or fill each column with its matched block.
        fill: ReliefImageFill,
    },
}

/// Settings for one north-up image structure.
#[derive(Clone, Debug, PartialEq)]
pub struct ImageBlockOptions {
    /// Output width in blocks, from 1 to 512.
    pub width: u32,
    /// Output height in blocks, from 1 to 512; this becomes world Z length.
    pub height: u32,
    /// Aspect-ratio handling before resizing.
    pub fit: ImageFit,
    /// Nearest for pixel art, or Lanczos3 for photographic input.
    pub resample: MapResample,
    /// Pixels below this alpha are omitted.
    pub alpha_threshold: u8,
    /// Background behind retained semitransparent pixels.
    pub background: [u8; 3],
    /// Approved Bedrock block ID for pixels below the alpha threshold; `None` keeps air.
    pub transparent_fill: Option<String>,
    /// Flat plane or a luminance-derived block relief.
    pub depth: ImageBlockHeight,
    /// Color matching mode for the generated block pixels.
    pub dithering: Dithering,
}

impl Default for ImageBlockOptions {
    fn default() -> Self {
        Self {
            width: 128,
            height: 128,
            fit: ImageFit::CenterCrop,
            resample: MapResample::Lanczos3,
            alpha_threshold: 1,
            background: [255; 3],
            transparent_fill: None,
            depth: ImageBlockHeight::Flat,
            dithering: Dithering::None,
        }
    }
}

impl ImageBlockOptions {
    /// Returns the largest relief height allowed by the in-memory structure volume limit.
    ///
    /// Invalid zero dimensions are treated as one pixel here; [`validate_options`] remains the
    /// authority that rejects invalid output dimensions before conversion.
    #[must_use]
    pub fn max_relief_height(&self) -> u16 {
        let pixels = u64::from(self.width) * u64::from(self.height);
        (bedrock_voxel::MAX_RELIEF_BLOCK_VOLUME / pixels.max(1)).clamp(2, 128) as u16
    }
}

/// Local image, structure output, and conversion settings.
pub struct ImageBlockExportRequest {
    /// PNG, JPEG, TGA, or WebP source image, read without modifying it.
    pub source: PathBuf,
    /// Destination `.mcstructure` file; no world is opened or modified.
    pub output: PathBuf,
    /// Flat or luminance-relief image settings.
    pub options: ImageBlockOptions,
}

/// Starts a visible, cancellable image conversion task.
///
/// The source image is bounded to 64 MiB compressed and 256 MiB decoded.
/// Cancellation before output writing discards the conversion; after writing
/// begins, the worker reports the actual write result. No Minecraft record is
/// modified. This path uses block appearance colors, not Bedrock map colors.
///
/// # Errors
/// Returns a runtime submission error; decode, conversion, and file errors
/// are reported on the returned task.
pub fn start_image_block_export(request: ImageBlockExportRequest) -> Result<String, String> {
    validate_options(&request.options)?;
    let detail = request
        .source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let title = match request.options.depth {
        ImageBlockHeight::Flat => "图片转 Flat 方块并导出结构",
        ImageBlockHeight::LuminanceRelief { .. } => "图片转亮度阶梯并导出结构",
    };
    let task_id =
        task_manager::create_task_with_details(None, title, detail, "解码图片", None, false);
    task_manager::register_task_cooperative_cancel(task_id.clone());
    let cancel = CancelFlag::new();
    task_manager::register_task_cancel_hook(task_id.clone(), {
        let cancel = cancel.clone();
        move || cancel.cancel()
    });
    let worker_id = task_id.clone();
    if let Err(error) = runtime::spawn_io(async move {
        let result = export_image(request, &worker_id, &cancel).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(error) if error.contains("已取消") || error.contains("cancelled") => "cancelled",
            Err(_) => "error",
        };
        task_manager::finish_task(
            &worker_id,
            status,
            Some(match result {
                Ok(message) | Err(message) => message,
            }),
        );
    }) {
        task_manager::finish_task(&task_id, "error", Some(error.clone()));
        return Err(error);
    }
    Ok(task_id)
}

async fn export_image(
    request: ImageBlockExportRequest,
    task_id: &str,
    cancel: &CancelFlag,
) -> Result<String, String> {
    let source = request.source;
    let decoded = runtime::run_io_blocking(move || decode_image(&source)).await??;
    if cancel.is_cancelled() {
        return Err("图片转换已取消".to_owned());
    }
    let total = u64::from(request.options.width) * u64::from(request.options.height);
    task_manager::reset_progress(task_id, Some(total), Some("裁剪、缩放与方块匹配"));
    let worker_cancel = cancel.clone();
    let worker_id = task_id.to_owned();
    let options = request.options;
    let (width, height) = (options.width, options.height);
    let mode_name = match options.depth {
        ImageBlockHeight::Flat => "Flat",
        ImageBlockHeight::LuminanceRelief { .. } => "2.5D 亮度阶梯",
    };
    let (structure, block_count) = runtime::run_cpu(move || {
        if worker_cancel.is_cancelled() {
            return Err("图片转换已取消".to_owned());
        }
        let mut reported = 0_u64;
        let mut reported_total = total;
        let stage = match options.depth {
            ImageBlockHeight::Flat => "裁剪、缩放与方块匹配",
            ImageBlockHeight::LuminanceRelief { .. } => "匹配方块并构建亮度阶梯",
        };
        let plan = image_block_plan_with_progress(
            &decoded,
            &options,
            &worker_cancel,
            |done, progress_total| {
                if progress_total != reported_total {
                    task_manager::reset_progress(&worker_id, Some(progress_total), Some(stage));
                    reported = 0;
                    reported_total = progress_total;
                }
                if done > reported {
                    task_manager::update_progress(
                        &worker_id,
                        done - reported,
                        Some(progress_total),
                        Some(stage),
                    );
                    reported = done;
                }
            },
        )?;
        if worker_cancel.is_cancelled() {
            return Err("图片转换已取消".to_owned());
        }
        let block_count = plan.blocks().len();
        let structure =
            McStructureFile::from_placement_plan(&plan).map_err(|error| error.to_string())?;
        Ok::<_, String>((structure, block_count))
    })
    .await??;
    if cancel.is_cancelled() {
        return Err("图片转换已取消".to_owned());
    }
    task_manager::reset_progress(task_id, Some(1), Some("写入 .mcstructure"));
    let output = request.output;
    let worker_output = output.clone();
    runtime::run_io_blocking(move || structure.write_to_path(&worker_output))
        .await?
        .map_err(|error| error.to_string())?;
    task_manager::update_progress(task_id, 1, Some(1), Some("写入 .mcstructure"));
    Ok(format!(
        "已生成 {}×{} {} 图片、{} 个方块到 {}",
        width,
        height,
        mode_name,
        block_count,
        output.display()
    ))
}

/// Returns the safe appearance-color block IDs available for image matching and transparent fill.
///
/// This only reads the bundled render palette. It does not open or modify a Minecraft world.
pub fn approved_block_names() -> Vec<String> {
    default_block_candidates()
        .iter()
        .map(|candidate| candidate.state.name.clone())
        .collect()
}

pub(super) fn image_block_plan_with_progress(
    source: &RgbaImage,
    options: &ImageBlockOptions,
    cancel: &CancelFlag,
    mut progress: impl FnMut(u64, u64),
) -> Result<BlockPlacementPlan, String> {
    validate_options(options)?;
    let scaled = resize_image(source, options)?;
    let candidates = default_block_candidates();
    let transparent_fill = options
        .transparent_fill
        .as_ref()
        .map(|name| {
            candidates
                .iter()
                .find(|candidate| candidate.state.name == *name)
                .map(|candidate| &candidate.state)
                .ok_or_else(|| format!("透明填充方块不在可用候选中：{name}"))
        })
        .transpose()?;
    let image_options = FlatImageOptions {
        dithering: options.dithering,
        alpha_threshold: options.alpha_threshold,
        background: options.background,
        transparent_fill,
    };
    let input = FlatImageInput {
        rgba: scaled.as_raw(),
        width: options.width,
        height: options.height,
    };
    match options.depth {
        ImageBlockHeight::Flat => flat_block_plan_with_progress(
            input,
            candidates,
            image_options,
            cancel,
            |done, total| progress(u64::from(done), u64::from(total)),
        )
        .map_err(|error| error.to_string()),
        ImageBlockHeight::LuminanceRelief { max_height, fill } => relief_block_plan_with_progress(
            input,
            candidates,
            image_options,
            ReliefImageOptions { max_height, fill },
            cancel,
            &mut progress,
        )
        .map_err(|error| error.to_string()),
    }
}

fn validate_options(options: &ImageBlockOptions) -> Result<(), String> {
    if !(1..=512).contains(&options.width) || !(1..=512).contains(&options.height) {
        return Err("Flat 图片宽高必须各为 1–512 个方块".to_owned());
    }
    if let ImageBlockHeight::LuminanceRelief { max_height, .. } = options.depth {
        if !(2..=128).contains(&max_height) {
            return Err("亮度阶梯高度必须为 2–128 层".to_owned());
        }
        let max_allowed = options.max_relief_height();
        if max_height > max_allowed {
            return Err(format!("当前图片尺寸最多支持 {max_allowed} 层亮度阶梯"));
        }
    }
    if let Some(name) = options.transparent_fill.as_ref() {
        if !default_block_candidates()
            .iter()
            .any(|candidate| candidate.state.name == *name)
        {
            return Err(format!("透明填充方块不在可用候选中：{name}"));
        }
    }
    Ok(())
}

pub(super) fn decode_image(path: &Path) -> Result<RgbaImage, String> {
    let size = path.metadata().map_err(|error| error.to_string())?.len();
    if size > MAX_SOURCE_BYTES {
        return Err("图片文件超过 64 MiB 输入限制".to_owned());
    }
    let mut reader = ImageReader::open(path).map_err(|error| error.to_string())?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    reader
        .decode()
        .map(|image| image.to_rgba8())
        .map_err(|error| error.to_string())
}

pub(super) fn resize_image(
    source: &RgbaImage,
    options: &ImageBlockOptions,
) -> Result<RgbaImage, String> {
    validate_options(options)?;
    let (source_width, source_height) = source.dimensions();
    if source_width == 0 || source_height == 0 {
        return Err("图片没有有效像素".to_owned());
    }
    let (x, y, width, height) = match options.fit {
        ImageFit::Stretch => (0, 0, source_width, source_height),
        ImageFit::CenterCrop => {
            let source_ratio = u64::from(source_width) * u64::from(options.height);
            let target_ratio = u64::from(source_height) * u64::from(options.width);
            if source_ratio > target_ratio {
                let width = (u64::from(source_height) * u64::from(options.width)
                    / u64::from(options.height)) as u32;
                ((source_width - width) / 2, 0, width.max(1), source_height)
            } else {
                let height = (u64::from(source_width) * u64::from(options.height)
                    / u64::from(options.width)) as u32;
                (0, (source_height - height) / 2, source_width, height.max(1))
            }
        }
    };
    let cropped = imageops::crop_imm(source, x, y, width, height).to_image();
    let filter = match options.resample {
        MapResample::Nearest => imageops::FilterType::Nearest,
        MapResample::Lanczos3 => imageops::FilterType::Lanczos3,
    };
    Ok(resize_rgba(&cropped, options.width, options.height, filter))
}

fn resize_rgba(
    source: &RgbaImage,
    width: u32,
    height: u32,
    filter: imageops::FilterType,
) -> RgbaImage {
    let mut premultiplied = source.clone();
    for pixel in premultiplied.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }

    let mut scaled = imageops::resize(&premultiplied, width, height, filter);
    for pixel in scaled.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        if alpha == 0 {
            pixel.0 = [0, 0, 0, 0];
            continue;
        }
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
    scaled
}

#[cfg(test)]
mod tests {
    use image::{Rgba, RgbaImage};

    use super::*;

    #[test]
    fn center_crop_keeps_aspect_ratio_and_center_pixels() {
        let source = RgbaImage::from_fn(4, 2, |x, _| Rgba([x as u8, 0, 0, 255]));
        let image = resize_image(
            &source,
            &ImageBlockOptions {
                width: 2,
                height: 2,
                resample: MapResample::Nearest,
                ..ImageBlockOptions::default()
            },
        )
        .expect("center crop");
        assert_eq!(image.get_pixel(0, 0)[0], 1);
        assert_eq!(image.get_pixel(1, 0)[0], 2);
    }

    #[test]
    fn stretch_keeps_full_horizontal_extent() {
        let source = RgbaImage::from_fn(4, 2, |x, _| Rgba([x as u8, 0, 0, 255]));
        let image = resize_image(
            &source,
            &ImageBlockOptions {
                width: 2,
                height: 2,
                fit: ImageFit::Stretch,
                resample: MapResample::Nearest,
                ..ImageBlockOptions::default()
            },
        )
        .expect("stretch");
        assert_eq!(image.get_pixel(0, 0)[0], 1);
        assert_eq!(image.get_pixel(1, 0)[0], 3);
    }

    #[test]
    fn lanczos_keeps_transparent_edge_color_premultiplied() {
        let source = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgba([0, 0, 0, 0])
            } else {
                Rgba([120, 220, 80, 255])
            }
        });
        let image = resize_image(
            &source,
            &ImageBlockOptions {
                width: 16,
                height: 1,
                fit: ImageFit::Stretch,
                resample: MapResample::Lanczos3,
                ..ImageBlockOptions::default()
            },
        )
        .expect("transparent edge resize");
        for pixel in image.pixels() {
            if (16..=239).contains(&pixel[3]) {
                assert!(pixel[0] >= 118);
                assert!(pixel[1] >= 218);
                assert!(pixel[2] >= 78);
            }
        }
    }

    #[test]
    fn invalid_output_size_is_rejected_before_task_creation() {
        assert!(
            validate_options(&ImageBlockOptions {
                width: 0,
                ..ImageBlockOptions::default()
            })
            .is_err()
        );
        assert!(
            validate_options(&ImageBlockOptions {
                height: 513,
                ..ImageBlockOptions::default()
            })
            .is_err()
        );
    }

    #[test]
    fn relief_height_limit_tracks_target_area() {
        assert_eq!(
            ImageBlockOptions {
                width: 512,
                height: 512,
                ..ImageBlockOptions::default()
            }
            .max_relief_height(),
            30
        );
        assert_eq!(ImageBlockOptions::default().max_relief_height(), 128);
    }

    #[test]
    fn transparent_fill_uses_only_approved_color_candidates() {
        let names = approved_block_names();
        for name in [
            "minecraft:white_wool",
            "minecraft:quartz_block",
            "minecraft:coal_block",
        ] {
            assert!(names.iter().any(|candidate| candidate == name));
            assert!(
                validate_options(&ImageBlockOptions {
                    transparent_fill: Some(name.to_owned()),
                    ..ImageBlockOptions::default()
                })
                .is_ok()
            );
        }
        assert!(
            validate_options(&ImageBlockOptions {
                transparent_fill: Some("minecraft:chest".to_owned()),
                ..ImageBlockOptions::default()
            })
            .is_err()
        );
    }
}
