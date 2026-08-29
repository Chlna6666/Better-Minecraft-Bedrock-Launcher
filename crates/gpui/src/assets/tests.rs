use super::*;
use crate::assets::render_image::RenderImageStorage;
use crate::{ObjectFit, Result, performance_metrics_snapshot, size};
use image::{
    Delay, ExtendedColorType, Frame, ImageBuffer, ImageEncoder as _, ImageFormat, RgbaImage,
    codecs::gif::{GifEncoder, Repeat},
};
use std::io::Cursor;
use std::sync::{Arc, atomic::Ordering};
use std::time::{Duration, Instant};

mod webp_tests;

fn render_image(
    bytes: &[u8],
    format: ImageFormat,
    config: AnimatedImageConfig,
) -> Result<RenderImage> {
    EncodedImage::new(format, Arc::<[u8]>::from(bytes)).render(config)
}

fn render_image_at(
    bytes: &[u8],
    format: ImageFormat,
    config: AnimatedImageConfig,
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, ImageRenderInfo)> {
    EncodedImage::new(format, Arc::<[u8]>::from(bytes)).render_sized(target, object_fit, config)
}

fn frame(width: u32, height: u32) -> Frame {
    let image: RgbaImage = ImageBuffer::from_pixel(width, height, image::Rgba([0, 0, 0, 255]));
    Frame::new(image)
}

fn rgba_frame(color: [u8; 4]) -> Frame {
    Frame::from_parts(
        ImageBuffer::from_pixel(1, 1, image::Rgba(color)),
        0,
        0,
        Delay::from_saturating_duration(Duration::from_millis(20)),
    )
}

#[test]
fn animated_frame_slots_are_bounded() {
    let image = RenderImage::new(vec![frame(2, 2), frame(2, 2), frame(2, 2), frame(2, 2)]);
    let config = AnimatedImageConfig {
        max_gpu_frame_slots: 3,
        ..AnimatedImageConfig::default()
    };

    assert_eq!(image.gpu_frame_slot_for_frame(0, config), 0);
    assert_eq!(image.gpu_frame_slot_for_frame(1, config), 1);
    assert_eq!(image.gpu_frame_slot_for_frame(2, config), 2);
    assert_eq!(image.gpu_frame_slot_for_frame(3, config), 0);
}

#[test]
fn resident_byte_len_counts_all_frames() {
    let image = RenderImage::new(vec![frame(2, 3), frame(4, 5)]);

    assert_eq!(image.frame_byte_len(0), 2 * 3 * 4);
    assert_eq!(image.frame_byte_len(1), 4 * 5 * 4);
    assert_eq!(image.resident_byte_len(), (2 * 3 * 4) + (4 * 5 * 4));
}

#[test]
fn raw_rgba_image_retains_rgba_bytes() {
    let pixels = vec![1, 2, 3, 255];
    let image =
        RenderImage::from_raw_pixels(1, 1, ImagePixelFormat::Rgba8, pixels.clone()).unwrap();

    assert_eq!(image.as_bytes(0).unwrap(), pixels);
    assert_eq!(image.pixel_format(0), Some(ImagePixelFormat::Rgba8));
}

#[test]
fn raw_pixel_bytes_reuses_shared_storage() {
    let pixels: Arc<[u8]> = Arc::<[u8]>::from([1, 2, 3, 255]);
    let image =
        RenderImage::from_raw_pixel_bytes(1, 1, ImagePixelFormat::Rgba8, pixels.clone()).unwrap();

    assert!(std::ptr::eq(
        image.as_bytes(0).unwrap().as_ptr(),
        pixels.as_ptr()
    ));
    assert_eq!(image.pixel_format(0), Some(ImagePixelFormat::Rgba8));
}

#[test]
fn new_image_keeps_existing_bgra_semantics() {
    let image = RenderImage::new(vec![rgba_frame([1, 2, 3, 255])]);

    assert_eq!(image.as_bytes(0).unwrap(), &[1, 2, 3, 255]);
    assert_eq!(image.pixel_format(0), Some(ImagePixelFormat::Bgra8));
}

#[test]
fn animated_config_clamps_runtime_values() {
    let config = AnimatedImageConfig {
        play: false,
        max_gpu_frame_slots: 0,
        max_fps: 999.0,
        inactive_max_fps: 999.0,
        prefetch_frames: 1,
        max_resident_frames: 0,
    }
    .clamped();

    assert!(!config.play);
    assert_eq!(config.max_gpu_frame_slots, 1);
    assert_eq!(config.max_fps, 999.0);
    assert_eq!(config.inactive_max_fps, 240.0);
    assert_eq!(config.prefetch_frames, 2);
    assert_eq!(config.max_resident_frames, 1);
}

#[test]
fn animated_config_uses_safe_defaults_for_non_finite_frame_rates() {
    let config = AnimatedImageConfig {
        max_fps: f32::NAN,
        inactive_max_fps: f32::INFINITY,
        ..AnimatedImageConfig::default()
    }
    .clamped();

    assert_eq!(config.max_fps, 90.0);
    assert_eq!(config.inactive_max_fps, 4.0);
}

#[test]
fn cover_target_preserves_aspect_ratio() {
    let target = ImageRenderSize::new(800, 600).unwrap();
    let fitted = target.fit(size(3840, 2160), ObjectFit::Cover);

    assert_eq!(fitted.width, 1067);
    assert_eq!(fitted.height, 600);
}

#[test]
fn contain_target_preserves_aspect_ratio() {
    let target = ImageRenderSize::new(44, 44).unwrap();
    let fitted = target.fit(size(1465, 1496), ObjectFit::Contain);

    assert_eq!(fitted.width, 44);
    assert_eq!(fitted.height, 44);
}

#[test]
fn gif_render_keeps_multiple_bgra_frames() {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(Repeat::Infinite).unwrap();
        encoder
            .encode_frames([rgba_frame([255, 0, 0, 255]), rgba_frame([0, 255, 0, 255])])
            .unwrap();
    }

    let image = render_image(&bytes, ImageFormat::Gif, AnimatedImageConfig::default()).unwrap();

    assert!(image.is_animated());
    assert_eq!(image.frame_count(), 2);
    assert_eq!(image.as_bytes(0).unwrap(), &[0, 0, 255, 255]);
}

#[test]
fn apng_render_keeps_multiple_frames() {
    let bytes = animated_png_bytes();
    let image = render_image(&bytes, ImageFormat::Png, AnimatedImageConfig::default()).unwrap();

    assert!(image.is_animated());
    assert_eq!(image.frame_count(), 2);
}

#[test]
fn static_png_is_not_treated_as_animation() {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[255, 0, 0, 255], 1, 1, ExtendedColorType::Rgba8)
        .unwrap();

    let image = render_image(&bytes, ImageFormat::Png, AnimatedImageConfig::default()).unwrap();

    assert!(!image.is_animated());
    assert_eq!(image.frame_count(), 1);
}

#[test]
fn png_target_render_uses_element_sized_resident_buffer() {
    let bytes = encoded_rgba_image(128, 96, |writer| {
        image::codecs::png::PngEncoder::new(writer).write_image(
            &solid_rgba_pixels(128, 96),
            128,
            96,
            ExtendedColorType::Rgba8,
        )
    });
    let target = ImageRenderSize::new(32, 24).unwrap();
    let (image, metadata) = render_image_at(
        &bytes,
        ImageFormat::Png,
        AnimatedImageConfig::default(),
        target,
        ObjectFit::Fill,
    )
    .unwrap();

    assert_eq!(image.size(0), target.size());
    assert_eq!(image.resident_byte_len(), 32 * 24 * 4);
    assert!(
        metadata.render_path == "png_row_sample" || metadata.render_path == "scaled_then_resized"
    );
}

#[test]
fn jpeg_target_render_scales_before_resizing() {
    let bytes = encoded_rgba_image(128, 96, |writer| {
        image::codecs::jpeg::JpegEncoder::new_with_quality(writer, 90).write_image(
            &solid_rgb_pixels(128, 96),
            128,
            96,
            ExtendedColorType::Rgb8,
        )
    });
    let target = ImageRenderSize::new(32, 24).unwrap();
    let (image, metadata) = render_image_at(
        &bytes,
        ImageFormat::Jpeg,
        AnimatedImageConfig::default(),
        target,
        ObjectFit::Fill,
    )
    .unwrap();

    assert_eq!(image.size(0), target.size());
    assert_eq!(image.resident_byte_len(), 32 * 24 * 4);
    assert!(metadata.render_path == "jpeg_scaled" || metadata.render_path == "scaled_then_resized");
}

#[test]
fn bmp_target_render_samples_rows_without_retaining_original_size() {
    let bytes = encoded_rgba_image(128, 96, |writer| {
        image::codecs::bmp::BmpEncoder::new(writer).write_image(
            &solid_rgba_pixels(128, 96),
            128,
            96,
            ExtendedColorType::Rgba8,
        )
    });
    let target = ImageRenderSize::new(32, 24).unwrap();
    let (image, metadata) = render_image_at(
        &bytes,
        ImageFormat::Bmp,
        AnimatedImageConfig::default(),
        target,
        ObjectFit::Fill,
    )
    .unwrap();

    assert_eq!(image.size(0), target.size());
    assert_eq!(image.resident_byte_len(), 32 * 24 * 4);
    assert!(
        metadata.render_path == "bmp_rect_sample" || metadata.render_path == "scaled_then_resized"
    );
}

#[test]
fn target_render_keeps_animated_png_playable_after_resize() {
    let bytes = animated_png_bytes_with_size(64, 64);
    let config = AnimatedImageConfig {
        max_resident_frames: 2,
        ..AnimatedImageConfig::default()
    };
    let target = ImageRenderSize::new(4, 4).unwrap();
    let (image, metadata) =
        render_image_at(&bytes, ImageFormat::Png, config, target, ObjectFit::Fill).unwrap();

    assert!(image.is_animated());
    assert_eq!(image.frame_count(), 2);
    assert_eq!(image.size(0), target.size());
    assert_eq!(image.size(1), target.size());
    assert_eq!(image.resident_byte_len(), 4 * 4 * 4 * 2);
    assert_eq!(metadata.render_path, "animated_frame_sample");
}

#[test]
fn target_render_streams_large_animation_after_resize() {
    let bytes = animated_png_bytes_with_size(64, 64);
    let config = AnimatedImageConfig {
        max_resident_frames: 1,
        ..AnimatedImageConfig::default()
    };
    let target = ImageRenderSize::new(4, 4).unwrap();
    let (image, _) =
        render_image_at(&bytes, ImageFormat::Png, config, target, ObjectFit::Fill).unwrap();

    assert!(matches!(image.storage, RenderImageStorage::Streaming(_)));
    assert!(image.is_animated());
    assert_eq!(image.frame_count(), usize::MAX);
    assert_eq!(image.size(0), target.size());
}

#[test]
fn large_animation_enters_streaming_mode() {
    let bytes = animated_png_bytes();
    let config = AnimatedImageConfig {
        max_resident_frames: 1,
        ..AnimatedImageConfig::default()
    };
    let image = render_image(&bytes, ImageFormat::Png, config).unwrap();

    assert!(matches!(image.storage, RenderImageStorage::Streaming(_)));
}

#[test]
fn streaming_animation_releases_stale_frames() {
    let bytes = animated_png_bytes();
    let config = AnimatedImageConfig {
        max_resident_frames: 1,
        ..AnimatedImageConfig::default()
    };
    let image = render_image(&bytes, ImageFormat::Png, config).unwrap();

    assert!(matches!(image.storage, RenderImageStorage::Streaming(_)));
    assert_eq!(image.frame_count(), usize::MAX);
    let first_frame_bytes = image.frame_byte_len(0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while image.resident_byte_len() == first_frame_bytes && Instant::now() < deadline {
        std::thread::yield_now();
    }
    assert!(image.resident_byte_len() > first_frame_bytes);
    let stale_frames = performance_metrics_snapshot().animation.stale_frame_count;
    assert!(image.next_streaming_frame(usize::MAX).is_none());
    assert!(performance_metrics_snapshot().animation.stale_frame_count > stale_frames);
}

#[test]
fn streaming_animation_prefetch_frames_are_bounded() {
    let bytes = animated_png_bytes_with_frame_count(8);
    let config = AnimatedImageConfig {
        prefetch_frames: 2,
        max_resident_frames: 1,
        ..AnimatedImageConfig::default()
    };
    let image = render_image(&bytes, ImageFormat::Png, config).unwrap();
    let first_frame_bytes = image.frame_byte_len(0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while image.resident_byte_len() == first_frame_bytes && Instant::now() < deadline {
        std::thread::yield_now();
    }

    assert!(image.resident_byte_len() <= first_frame_bytes * 3);
}

#[test]
fn streaming_animation_keeps_worker_running_while_queue_is_full() {
    let bytes = animated_png_bytes_with_frame_count(4);
    let config = AnimatedImageConfig {
        prefetch_frames: 2,
        max_resident_frames: 1,
        ..AnimatedImageConfig::default()
    };
    let image = render_image(&bytes, ImageFormat::Png, config).unwrap();
    let RenderImageStorage::Streaming(state) = &image.storage else {
        panic!("large animation should use streaming decode");
    };

    assert!(state.stream_task_running.load(Ordering::Acquire));
}

#[test]
fn streaming_animation_records_loop_restart() {
    let before = performance_metrics_snapshot().animation.loop_restarts;
    let bytes = animated_png_bytes();
    let config = AnimatedImageConfig {
        prefetch_frames: 2,
        max_resident_frames: 1,
        ..AnimatedImageConfig::default()
    };
    let image = render_image(&bytes, ImageFormat::Png, config).unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut sequence = 0usize;
    while sequence < 2 && Instant::now() < deadline {
        if let Some(frame) = image.next_streaming_frame(sequence) {
            sequence = frame.sequence();
        } else {
            std::thread::yield_now();
        }
    }

    assert_eq!(sequence, 2);
    assert!(performance_metrics_snapshot().animation.loop_restarts > before);
}

fn animated_png_bytes() -> Vec<u8> {
    animated_png_bytes_with_size(1, 1)
}

fn animated_png_bytes_with_size(width: u32, height: u32) -> Vec<u8> {
    animated_png_bytes_with_size_and_frame_count(width, height, 2)
}

fn animated_png_bytes_with_frame_count(frame_count: u32) -> Vec<u8> {
    animated_png_bytes_with_size_and_frame_count(1, 1, frame_count)
}

fn animated_png_bytes_with_size_and_frame_count(
    width: u32,
    height: u32,
    frame_count: u32,
) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = ::png::Encoder::new(Cursor::new(&mut bytes), width, height);
        encoder.set_color(::png::ColorType::Rgba);
        encoder.set_depth(::png::BitDepth::Eight);
        encoder.set_animated(frame_count, 0).unwrap();
        encoder.set_frame_delay(20, 1000).unwrap();
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&solid_color_rgba_pixels(width, height, [255, 0, 0, 255]))
            .unwrap();
        for index in 1..frame_count {
            let color = if index % 2 == 0 {
                [0, 0, 255, 255]
            } else {
                [0, 255, 0, 255]
            };
            writer.set_frame_delay(20, 1000).unwrap();
            writer
                .write_image_data(&solid_color_rgba_pixels(width, height, color))
                .unwrap();
        }
        writer.finish().unwrap();
    }
    bytes
}

fn encoded_rgba_image(
    width: u32,
    height: u32,
    encode: impl FnOnce(&mut Vec<u8>) -> image::ImageResult<()>,
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity((width * height) as usize);
    encode(&mut bytes).unwrap();
    bytes
}

fn solid_color_rgba_pixels(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    color
        .into_iter()
        .cycle()
        .take(width as usize * height as usize * 4)
        .collect()
}

fn solid_rgba_pixels(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[x as u8, y as u8, 192, 255]);
        }
    }
    pixels
}

fn solid_rgb_pixels(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            pixels.extend_from_slice(&[x as u8, y as u8, 192]);
        }
    }
    pixels
}
