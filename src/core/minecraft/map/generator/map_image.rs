//! Bounded direct Bedrock map-pixel preview from an image.

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use crate::core::minecraft::map::image::{
    MapImageCrop, MapImageFit, MapImageOptions, MapImageTile, MapResample, tile_map_image_with_progress,
};
use bedrock_world::surface::CancelFlag;
use image::{Rgba, RgbaImage, imageops};

use super::image::decode_image;
use crate::tasks::{runtime, task_manager};

const RESULT_LIMIT_BYTES: usize = 160 * 1024 * 1024;
const RESULT_TTL: Duration = Duration::from_secs(15 * 60);

/// Crop and map-grid settings for directly written Bedrock RGBA map pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MapPreviewOptions {
    /// West-to-east map count, 1–64.
    pub columns: u32,
    /// North-to-south map count, 1–64; product must be at most 1024.
    pub rows: u32,
    /// Fit the grid to the decoded source dimensions before splitting.
    pub auto_fit: bool,
    /// Complete undistorted images by default; cropping and stretching are opt-in.
    pub fit: MapImageFit,
    /// Ignore fully transparent outer padding when fitting a PNG to the tile mosaic.
    /// Nontransparent source pixels are never discarded.
    pub trim_transparent_edges: bool,
    /// Image resampling before 128×128 splitting.
    pub resample: MapResample,
    /// Pixels below this alpha become transparent; others become opaque.
    pub alpha_threshold: u8,
}

impl Default for MapPreviewOptions {
    fn default() -> Self {
        Self {
            columns: 1,
            rows: 1,
            auto_fit: false,
            fit: MapImageFit::Contain,
            trim_transparent_edges: true,
            resample: MapResample::Lanczos3,
            alpha_threshold: 1,
        }
    }
}

/// A task-owned preview of actual 128×128 RGBA map records and visible tile seams.
///
/// The seam pixels exist only in `preview_rgba`; [`Self::tiles`] remain exact map
/// pixels. This value contains no map IDs and does not modify a world or player.
#[derive(Debug)]
pub struct MapPreviewResult {
    /// Source image width before crop.
    pub source_width: u32,
    /// Source image height before crop.
    pub source_height: u32,
    /// Source crop used by the map conversion.
    pub crop: MapImageCrop,
    /// West-to-east tile count.
    pub columns: u32,
    /// North-to-south tile count.
    pub rows: u32,
    /// Preview bitmap width, including visual seams.
    pub preview_width: u32,
    /// Preview bitmap height, including visual seams.
    pub preview_height: u32,
    /// RGBA preview bitmap with checkerboard transparency and seam lines.
    pub preview_rgba: Vec<u8>,
    /// Seam-free, uncomposited bitmap for world placement projection.
    pub placement_width: u32,
    /// Seam-free placement bitmap height.
    pub placement_height: u32,
    /// Real RGBA map pixels at bounded preview resolution (transparent stays transparent).
    pub placement_rgba: Vec<u8>,
    /// Exact RGBA map tiles in north-up row-major order.
    pub tiles: Vec<MapImageTile>,
}

/// A bounded intermediate mosaic; seam lines are visual guides only.
#[derive(Debug)]
pub struct MapPreviewFrame {
    /// Number of completed tiles in north-up row-major order.
    pub done: u32,
    /// Total number of columns in the configured grid.
    pub columns: u32,
    /// Total number of rows in the configured grid.
    pub rows: u32,
    /// Decoded source-image dimensions before crop.
    pub source_width: u32,
    /// Decoded source-image dimensions before crop.
    pub source_height: u32,
    /// Exact source rectangle planned for conversion.
    pub crop: MapImageCrop,
    /// Width including visual seams.
    pub width: u32,
    /// Height including visual seams.
    pub height: u32,
    /// Checkerboard-composited RGBA pixels with visual seam lines.
    pub rgba: Vec<u8>,
}

struct StoredPreview {
    result: Arc<MapPreviewResult>,
    bytes: usize,
    inserted: Instant,
}

#[derive(Default)]
struct PreviewStore {
    entries: HashMap<String, StoredPreview>,
    frames: HashMap<String, (Arc<MapPreviewFrame>, Instant)>,
    order: VecDeque<String>,
    discarded: HashMap<String, Instant>,
    bytes: usize,
}

static PREVIEWS: OnceLock<Mutex<PreviewStore>> = OnceLock::new();

fn previews() -> &'static Mutex<PreviewStore> {
    PREVIEWS.get_or_init(|| Mutex::new(PreviewStore::default()))
}

impl PreviewStore {
    fn remove(&mut self, id: &str) {
        self.frames.remove(id);
        if let Some(entry) = self.entries.remove(id) {
            self.bytes -= entry.bytes;
        }
        self.order.retain(|queued| queued != id);
    }

    fn expire(&mut self) {
        self.frames
            .retain(|_, (_, inserted)| inserted.elapsed() <= RESULT_TTL);
        self.discarded
            .retain(|_, discarded| discarded.elapsed() <= RESULT_TTL);
        let expired = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.inserted.elapsed() > RESULT_TTL)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in expired {
            self.remove(&id);
        }
    }

    fn insert(&mut self, id: String, result: MapPreviewResult) -> Result<(), String> {
        if self.discarded.contains_key(&id) {
            return Ok(());
        }
        self.expire();
        self.frames.remove(&id);
        let bytes = result.preview_rgba.len()
            + result.placement_rgba.len()
            + result
                .tiles
                .iter()
                .map(|tile| tile.pixels.rgba.len())
                .sum::<usize>();
        if bytes > RESULT_LIMIT_BYTES {
            return Err("地图预览超过 160 MiB 结果上限".to_owned());
        }
        while self.bytes + bytes > RESULT_LIMIT_BYTES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.remove(&oldest);
        }
        self.bytes += bytes;
        self.order.push_back(id.clone());
        self.entries.insert(
            id,
            StoredPreview {
                result: Arc::new(result),
                bytes,
                inserted: Instant::now(),
            },
        );
        Ok(())
    }

    fn publish_frame(&mut self, id: &str, frame: MapPreviewFrame) {
        if self.discarded.contains_key(id) {
            return;
        }
        self.expire();
        if !self.frames.contains_key(id) && self.frames.len() == 8 {
            if let Some(oldest) = self
                .frames
                .iter()
                .min_by_key(|(_, (_, at))| at)
                .map(|(id, _)| id.clone())
            {
                self.frames.remove(&oldest);
            }
        }
        self.frames
            .insert(id.to_owned(), (Arc::new(frame), Instant::now()));
    }
}

/// Reads an unexpired conversion result by its visible TaskManager task ID.
///
/// This is intended for foreground event consumers, not `Render`. The returned
/// Arc keeps the immutable result alive only while the caller needs it.
#[must_use]
pub fn get_map_preview(task_id: &str) -> Option<Arc<MapPreviewResult>> {
    let mut store = previews().lock().ok()?;
    store.expire();
    store.entries.get(task_id).map(|entry| entry.result.clone())
}

/// Reads the latest intermediate mosaic for a running conversion task.
///
/// At most eight frames are cached, each at most 611×611 RGBA pixels. This is a
/// read-only UI snapshot; the returned pixels never become `map_<id>` record bytes.
#[must_use]
pub fn get_map_preview_frame(task_id: &str) -> Option<Arc<MapPreviewFrame>> {
    let mut store = previews().lock().ok()?;
    store.expire();
    store.frames.get(task_id).map(|(frame, _)| frame.clone())
}

/// Releases a preview and prevents a still-running conversion from storing it.
///
/// Call this when a panel closes or a newer conversion supersedes the task.
pub fn release_map_preview(task_id: &str) {
    if let Ok(mut store) = previews().lock() {
        store.remove(task_id);
        store.discarded.insert(task_id.to_owned(), Instant::now());
    }
}

/// Starts a visible conversion that leaves the original image and world untouched.
///
/// The result is held in a 160 MiB task-ID store for up to 15 minutes; closing
/// the panel or starting a newer conversion should call [`release_map_preview`].
///
/// # Errors
/// Returns invalid grid settings or a runtime submission error. Decode and
/// conversion failures are reported on the returned task.
pub fn start_map_image_preview(
    source: PathBuf,
    options: MapPreviewOptions,
) -> Result<String, String> {
    validate_options(options)?;
    let detail = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    let task_id = task_manager::create_task_with_details(
        None,
        "图片转 Bedrock 地图像素预览",
        detail,
        "解码图片",
        None,
        false,
    );
    task_manager::register_task_cooperative_cancel(task_id.clone());
    let cancel = CancelFlag::new();
    task_manager::register_task_cancel_hook(task_id.clone(), {
        let cancel = cancel.clone();
        move || cancel.cancel()
    });
    let worker_id = task_id.clone();
    if let Err(error) = runtime::spawn_io(async move {
        let result = convert(source, options, &worker_id, &cancel).await;
        let status = match &result {
            Ok(_) => "completed",
            Err(error) if error.contains("已取消") => "cancelled",
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

async fn convert(
    source: PathBuf,
    mut options: MapPreviewOptions,
    task_id: &str,
    cancel: &CancelFlag,
) -> Result<String, String> {
    let image = runtime::run_io_blocking(move || decode_image(&source)).await??;
    if cancel.is_cancelled() {
        return Err("地图图片转换已取消".to_owned());
    }
    // The source file's transparent margins are not actual picture content.
    // Measure visible pixels BEFORE auto-grid sizing, or the grid allocates whole map
    // records to empty padding. Never remove any pixel above the alpha threshold.
    let visible_crop = if options.fit == MapImageFit::Contain && options.trim_transparent_edges {
        visible_alpha_crop(&image, options.alpha_threshold)
    } else {
        None
    };
    let dimensions = visible_crop.map_or_else(
        || (image.width(), image.height()),
        |bounds| (bounds.width, bounds.height),
    );
    if options.auto_fit {
        (options.columns, options.rows) = fitted_grid(dimensions.0, dimensions.1);
    }
    let mut crop = crop_for_grid(image.width(), image.height(), options)?;
    if let Some(bounds) = visible_crop {
        crop.rect = bounds;
    }
    if let Ok(mut store) = previews().lock() {
        store.publish_frame(
            task_id,
            MapPreviewFrame {
                done: 0,
                columns: options.columns,
                rows: options.rows,
                source_width: crop.source_width,
                source_height: crop.source_height,
                crop: crop.rect,
                width: 0,
                height: 0,
                rgba: Vec::new(),
            },
        );
    }
    let count = options.columns * options.rows;
    task_manager::reset_progress(task_id, Some(u64::from(count)), Some("缩放并分片地图"));
    let worker_cancel = cancel.clone();
    let worker_id = task_id.to_owned();
    let result = runtime::run_cpu(move || {
        let tiles = tile_map_image_with_progress(
            image.into_raw(),
            crop.source_width,
            crop.source_height,
            MapImageOptions {
                crop: crop.rect,
                columns: options.columns,
                rows: options.rows,
                resample: options.resample,
                fit: options.fit,
                alpha_threshold: options.alpha_threshold,
            },
            |_| {
                task_manager::update_progress(
                    &worker_id,
                    1,
                    Some(u64::from(count)),
                    Some("逐张生成地图"),
                );
            },
        )
        .map_err(|error| error.to_string())?;
        if worker_cancel.is_cancelled() {
            return Err("地图图片转换已取消".to_owned());
        }
        task_manager::reset_progress(&worker_id, Some(u64::from(count)), Some("逐张合并预览"));
        let mut last_frame = Instant::now();
        let frame_id = worker_id.clone();
        let (preview_width, preview_height, preview_rgba) =
            build_preview(&tiles, options, |done, width, height, rgba| {
                if worker_cancel.is_cancelled() {
                    return false;
                }
                task_manager::update_progress(
                    &worker_id,
                    1,
                    Some(u64::from(count)),
                    Some("逐张合并预览"),
                );
                if done <= 16 || last_frame.elapsed() >= Duration::from_millis(80) {
                    if let Ok(mut store) = previews().lock() {
                        store.publish_frame(
                            &frame_id,
                            MapPreviewFrame {
                                done,
                                columns: options.columns,
                                rows: options.rows,
                                source_width: crop.source_width,
                                source_height: crop.source_height,
                                crop: crop.rect,
                                width,
                                height,
                                rgba: rgba.to_vec(),
                            },
                        );
                    }
                    last_frame = Instant::now();
                }
                true
            })?;
        if worker_cancel.is_cancelled() {
            return Err("地图图片转换已取消".to_owned());
        }
        let (placement_width, placement_height, placement_rgba) =
            build_placement_preview(&tiles, options)?;
        Ok::<_, String>(MapPreviewResult {
            source_width: crop.source_width,
            source_height: crop.source_height,
            crop: crop.rect,
            columns: options.columns,
            rows: options.rows,
            preview_width,
            preview_height,
            preview_rgba,
            placement_width,
            placement_height,
            placement_rgba,
            tiles,
        })
    })
    .await??;
    if cancel.is_cancelled() {
        return Err("地图图片转换已取消".to_owned());
    }
    let mut store = previews()
        .lock()
        .map_err(|_| "地图预览存储已损坏".to_owned())?;
    store.insert(task_id.to_owned(), result)?;
    Ok(format!(
        "已生成 {}×{} 张地图像素预览；未修改世界或玩家背包",
        options.columns, options.rows
    ))
}

fn validate_options(options: MapPreviewOptions) -> Result<(), String> {
    if !(1..=64).contains(&options.columns)
        || !(1..=64).contains(&options.rows)
        || options.columns * options.rows > 1024
    {
        return Err("地图列数与行数各须为 1–64，合计最多 1024 张".to_owned());
    }
    Ok(())
}

/// Allocate no more than the source-resolution map budget, but optimize the
/// *mosaic aspect ratio*, not just two independently rounded pixel dimensions.
/// A small loss of total map count is acceptable when it prevents large empty bands.
fn fitted_grid(width: u32, height: u32) -> (u32, u32) {
    let suggested_columns = width.div_ceil(128).clamp(1, 64);
    let suggested_rows = height.div_ceil(128).clamp(1, 64);
    let budget = (suggested_columns * suggested_rows).min(1024);
    let mut best = (1, 1);
    let mut best_score = f64::NEG_INFINITY;
    for columns in 1..=64_u32 {
        for rows in 1..=(1024 / columns).min(64) {
            let count = columns * rows;
            if count > budget {
                continue;
            }
            let numerator = f64::from(width) * f64::from(rows);
            let denominator = f64::from(height) * f64::from(columns);
            let ratio = numerator / denominator;
            let coverage = ratio.min(1.0 / ratio);
            let detail = f64::from(count) / f64::from(budget);
            let score = coverage * 0.8 + detail * 0.2;
            if score > best_score + 1e-12
                || ((score - best_score).abs() <= 1e-12 && count > best.0 * best.1)
            {
                best = (columns, rows);
                best_score = score;
            }
        }
    }
    best
}

/// Return the exact bounding rectangle of pixels that can survive alpha thresholding.
/// A completely transparent source keeps its original rectangle for deterministic behavior.
fn visible_alpha_crop(image: &RgbaImage, alpha_threshold: u8) -> Option<MapImageCrop> {
    let mut min_x = image.width();
    let mut min_y = image.height();
    let mut max_x = 0;
    let mut max_y = 0;
    let threshold = alpha_threshold.max(1);
    let mut found = false;
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] < threshold {
            continue;
        }
        found = true;
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    found.then_some(MapImageCrop {
        x: min_x,
        y: min_y,
        width: max_x - min_x + 1,
        height: max_y - min_y + 1,
    })
}

struct GridCrop {
    source_width: u32,
    source_height: u32,
    rect: MapImageCrop,
}

fn crop_for_grid(width: u32, height: u32, options: MapPreviewOptions) -> Result<GridCrop, String> {
    validate_options(options)?;
    if width == 0 || height == 0 {
        return Err("图片没有有效像素".to_owned());
    }
    let (x, y, crop_width, crop_height) = if options.fit == MapImageFit::CenterCrop {
        let source_ratio = u64::from(width) * u64::from(options.rows);
        let grid_ratio = u64::from(height) * u64::from(options.columns);
        if source_ratio > grid_ratio {
            let crop_width =
                (u64::from(height) * u64::from(options.columns) / u64::from(options.rows)) as u32;
            ((width - crop_width) / 2, 0, crop_width.max(1), height)
        } else {
            let crop_height =
                (u64::from(width) * u64::from(options.rows) / u64::from(options.columns)) as u32;
            (0, (height - crop_height) / 2, width, crop_height.max(1))
        }
    } else {
        (0, 0, width, height)
    };
    Ok(GridCrop {
        source_width: width,
        source_height: height,
        rect: MapImageCrop {
            x,
            y,
            width: crop_width,
            height: crop_height,
        },
    })
}

/// Assemble a compact, seam-free RGBA mosaic from the exact tile pixels.
/// The largest edge stays within 512 pixels even with a 64-map axis.
fn build_placement_preview(
    tiles: &[MapImageTile],
    options: MapPreviewOptions,
) -> Result<(u32, u32, Vec<u8>), String> {
    let cell = (512 / options.columns.max(options.rows)).clamp(1, 128);
    let width = options.columns * cell;
    let height = options.rows * cell;
    let mut rgba = vec![0_u8; (width * height * 4) as usize];
    for tile in tiles {
        if tile.column >= options.columns
            || tile.row >= options.rows
            || tile.pixels.rgba.len() != (128 * 128 * 4) as usize
        {
            return Err("地图放置预览包含无效分片".to_owned());
        }
        for local_y in 0..cell {
            let source_y = (local_y * 128 / cell) as usize;
            let output_y = tile.row * cell + local_y;
            for local_x in 0..cell {
                let source_x = (local_x * 128 / cell) as usize;
                let output_x = tile.column * cell + local_x;
                let src = (source_y * 128 + source_x) * 4;
                let dst = ((output_y * width + output_x) * 4) as usize;
                rgba[dst..dst + 4].copy_from_slice(&tile.pixels.rgba[src..src + 4]);
            }
        }
    }
    Ok((width, height, rgba))
}

fn build_preview(
    tiles: &[MapImageTile],
    options: MapPreviewOptions,
    mut completed: impl FnMut(u32, u32, u32, &[u8]) -> bool,
) -> Result<(u32, u32, Vec<u8>), String> {
    const SEAM: u32 = 3;
    let cell = (512 / options.columns.max(options.rows)).clamp(16, 128);
    let width = options.columns * (cell + SEAM) + SEAM;
    let height = options.rows * (cell + SEAM) + SEAM;
    let mut preview = RgbaImage::from_pixel(width, height, Rgba([36, 38, 43, 255]));
    for (index, tile) in tiles.iter().enumerate() {
        let source = RgbaImage::from_raw(128, 128, tile.pixels.rgba.clone())
            .ok_or_else(|| "地图分片像素长度不正确".to_owned())?;
        let scaled = imageops::resize(&source, cell, cell, imageops::FilterType::Nearest);
        let origin_x = SEAM + tile.column * (cell + SEAM);
        let origin_y = SEAM + tile.row * (cell + SEAM);
        for (x, y, pixel) in scaled.enumerate_pixels() {
            let background = if ((x / 8) + (y / 8)) % 2 == 0 {
                192_u8
            } else {
                160_u8
            };
            let alpha = u16::from(pixel[3]);
            let color = [0, 1, 2].map(|channel| {
                ((u16::from(pixel[channel]) * alpha + u16::from(background) * (255 - alpha) + 127)
                    / 255) as u8
            });
            preview.put_pixel(
                origin_x + x,
                origin_y + y,
                Rgba([color[0], color[1], color[2], 255]),
            );
        }
        if !completed(index as u32 + 1, width, height, preview.as_raw()) {
            return Err("地图图片转换已取消".to_owned());
        }
    }
    Ok((width, height, preview.into_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::minecraft::map::image::tile_map_image;

    #[test]
    fn center_crop_matches_map_grid_ratio() {
        let crop = crop_for_grid(
            400,
            200,
            MapPreviewOptions {
                columns: 1,
                rows: 1,
                fit: MapImageFit::CenterCrop,
                ..MapPreviewOptions::default()
            },
        )
        .expect("crop");
        assert_eq!(
            crop.rect,
            MapImageCrop {
                x: 100,
                y: 0,
                width: 200,
                height: 200
            }
        );
    }

    #[test]
    fn automatic_grid_uses_the_source_ratio_within_its_map_budget() {
        for (width, height) in [(1200, 640), (8192, 8192), (8192, 1), (600, 540)] {
            let (columns, rows) = fitted_grid(width, height);
            let budget = (width.div_ceil(128).clamp(1, 64)
                * height.div_ceil(128).clamp(1, 64)).min(1024);
            assert!((1..=64).contains(&columns));
            assert!((1..=64).contains(&rows));
            assert!(columns * rows <= budget);
        }
        assert_eq!(fitted_grid(8192, 8192), (32, 32));
        assert_eq!(fitted_grid(8192, 1), (64, 1));
    }

    #[test]
    fn contain_trims_only_fully_transparent_outer_padding() {
        let mut image = RgbaImage::from_pixel(8, 6, Rgba([0, 0, 0, 0]));
        image.put_pixel(2, 1, Rgba([255, 0, 0, 255]));
        image.put_pixel(5, 4, Rgba([0, 0, 255, 128]));
        assert_eq!(
            visible_alpha_crop(&image, 1),
            Some(MapImageCrop { x: 2, y: 1, width: 4, height: 4 })
        );
        assert_eq!(
            visible_alpha_crop(&image, 255),
            Some(MapImageCrop { x: 2, y: 1, width: 1, height: 1 })
        );
        assert_eq!(
            visible_alpha_crop(&RgbaImage::from_pixel(8, 6, Rgba([0, 0, 0, 0])), 1),
            None,
        );
    }

    #[test]
    fn custom_grid_allows_sixty_four_tiles_per_axis_with_total_cap() {
        assert!(
            validate_options(MapPreviewOptions {
                columns: 64,
                rows: 16,
                auto_fit: false,
                ..MapPreviewOptions::default()
            })
            .is_ok()
        );
        assert!(
            validate_options(MapPreviewOptions {
                columns: 64,
                rows: 17,
                auto_fit: false,
                ..MapPreviewOptions::default()
            })
            .is_err()
        );
    }

    #[test]
    fn preview_grid_lines_do_not_change_tile_pixels() {
        let source = vec![255, 0, 0, 255, 0, 0, 255, 255];
        let options = MapPreviewOptions {
            columns: 2,
            rows: 1,
            resample: MapResample::Nearest,
            ..MapPreviewOptions::default()
        };
        let tiles = tile_map_image(
            source,
            2,
            1,
            MapImageOptions {
                crop: MapImageCrop {
                    x: 0,
                    y: 0,
                    width: 2,
                    height: 1,
                },
                columns: 2,
                rows: 1,
                resample: MapResample::Nearest,
                fit: MapImageFit::Stretch,
                alpha_threshold: 1,
            },
        )
        .expect("tiles");
        let mut frames = Vec::new();
        let (width, _, preview) = build_preview(&tiles, options, |done, width, _, rgba| {
            let second_tile = ((3 * width + 134) * 4) as usize;
            frames.push((done, rgba[second_tile..second_tile + 4].to_vec()));
            true
        })
        .expect("preview");
        assert_eq!(frames[0], (1, vec![36, 38, 43, 255]));
        assert_eq!(frames[1], (2, vec![0, 0, 255, 255]));
        assert!(build_preview(&tiles, options, |_, _, _, _| false).is_err());
        assert_eq!(&tiles[0].pixels.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&tiles[1].pixels.rgba[..4], &[0, 0, 255, 255]);
        let seam = (3 + 128) as usize * 4;
        assert_eq!(&preview[seam..seam + 4], &[36, 38, 43, 255]);
        assert_eq!(width, 265);
    }

    #[test]
    fn placement_preview_has_no_fake_seams_or_checkerboard() {
        let options = MapPreviewOptions {
            columns: 2,
            rows: 1,
            ..MapPreviewOptions::default()
        };
        let tiles = tile_map_image(
            vec![0, 0, 0, 0, 50, 100, 150, 255],
            2,
            1,
            MapImageOptions {
                crop: MapImageCrop { x: 0, y: 0, width: 2, height: 1 },
                columns: 2,
                rows: 1,
                fit: MapImageFit::Stretch,
                resample: MapResample::Nearest,
                alpha_threshold: 1,
            },
        ).expect("tiles");
        let (width, height, rgba) = build_placement_preview(&tiles, options).expect("preview");
        assert_eq!((width, height), (256, 128));
        assert_eq!(&rgba[0..4], &[0, 0, 0, 0]);
        let right = ((0 * width + 128) * 4) as usize;
        assert_eq!(&rgba[right..right + 4], &[50, 100, 150, 255]);
        assert_eq!(rgba.len(), (width * height * 4) as usize);
    }

    #[test]
    fn released_task_cannot_reinsert_late_result() {
        let id = "map-preview-release-test";
        release_map_preview(id);
        let result = MapPreviewResult {
            source_width: 1,
            source_height: 1,
            crop: MapImageCrop {
                x: 0,
                y: 0,
                width: 1,
                height: 1,
            },
            columns: 1,
            rows: 1,
            preview_width: 1,
            preview_height: 1,
            preview_rgba: vec![0; 4],
            placement_width: 1,
            placement_height: 1,
            placement_rgba: vec![0; 4],
            tiles: Vec::new(),
        };
        previews()
            .lock()
            .expect("store")
            .insert(id.to_owned(), result)
            .expect("discard");
        assert!(get_map_preview(id).is_none());
        previews().lock().expect("store").publish_frame(
            id,
            MapPreviewFrame {
                done: 1,
                columns: 1,
                rows: 1,
                source_width: 1,
                source_height: 1,
                crop: MapImageCrop {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            },
        );
        assert!(get_map_preview_frame(id).is_none());
    }
}
