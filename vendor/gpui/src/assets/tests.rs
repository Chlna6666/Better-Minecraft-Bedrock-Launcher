use super::*;
use crate::assets::render_image::RenderImageData;
use crate::{BackgroundExecutor, ObjectFit, size};
use image::{
    Delay, ExtendedColorType, Frame, ImageBuffer, ImageEncoder as _, ImageFormat, RgbaImage,
    codecs::gif::{GifEncoder, Repeat},
};
use rand::SeedableRng as _;
use std::io::Cursor;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;

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
fn decoded_byte_len_counts_all_frames() {
    let image = RenderImage::new(vec![frame(2, 3), frame(4, 5)]);

    assert_eq!(image.frame_byte_len(0), 2 * 3 * 4);
    assert_eq!(image.frame_byte_len(1), 4 * 5 * 4);
    assert_eq!(image.decoded_byte_len(), (2 * 3 * 4) + (4 * 5 * 4));
}

#[test]
fn raw_rgba_image_retains_rgba_bytes() {
    let pixels = vec![1, 2, 3, 255];
    let image =
        RenderImage::from_raw_pixels(1, 1, RenderImagePixelFormat::Rgba8, pixels.clone()).unwrap();

    assert_eq!(image.as_bytes(0).unwrap(), pixels);
    assert_eq!(image.pixel_format(0), Some(RenderImagePixelFormat::Rgba8));
}

#[test]
fn raw_pixel_bytes_reuses_shared_storage() {
    let pixels: Arc<[u8]> = Arc::<[u8]>::from([1, 2, 3, 255]);
    let image =
        RenderImage::from_raw_pixel_bytes(1, 1, RenderImagePixelFormat::Rgba8, pixels.clone())
            .unwrap();

    assert!(std::ptr::eq(
        image.as_bytes(0).unwrap().as_ptr(),
        pixels.as_ptr()
    ));
    assert_eq!(image.pixel_format(0), Some(RenderImagePixelFormat::Rgba8));
}

#[test]
fn new_image_keeps_existing_bgra_semantics() {
    let image = RenderImage::new(vec![rgba_frame([1, 2, 3, 255])]);

    assert_eq!(image.as_bytes(0).unwrap(), &[1, 2, 3, 255]);
    assert_eq!(image.pixel_format(0), Some(RenderImagePixelFormat::Bgra8));
}

#[test]
fn animated_config_clamps_runtime_values() {
    let config = AnimatedImageConfig {
        play: false,
        max_gpu_frame_slots: 0,
        max_fps: 999.0,
        inactive_max_fps: 999.0,
        decode_ahead_frames: 1,
        max_resident_frames: 0,
        max_resident_bytes: 0,
    }
    .clamped();

    assert!(!config.play);
    assert_eq!(config.max_gpu_frame_slots, 1);
    assert_eq!(config.max_fps, 60.0);
    assert_eq!(config.inactive_max_fps, 30.0);
    assert_eq!(config.decode_ahead_frames, 2);
    assert_eq!(config.max_resident_frames, 1);
    assert_eq!(config.max_resident_bytes, 4);
}

#[test]
fn cover_target_preserves_aspect_ratio() {
    let target = ImageDecodeTarget::new(800, 600).unwrap();
    let fitted = fitted_target_size(size(3840, 2160), target, ObjectFit::Cover);

    assert_eq!(fitted.width, 1067);
    assert_eq!(fitted.height, 600);
}

#[test]
fn contain_target_preserves_aspect_ratio() {
    let target = ImageDecodeTarget::new(44, 44).unwrap();
    let fitted = fitted_target_size(size(1465, 1496), target, ObjectFit::Contain);

    assert_eq!(fitted.width, 44);
    assert_eq!(fitted.height, 44);
}

#[test]
fn gif_decode_helper_keeps_multiple_bgra_frames() {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        encoder.set_repeat(Repeat::Infinite).unwrap();
        encoder
            .encode_frames([rgba_frame([255, 0, 0, 255]), rgba_frame([0, 255, 0, 255])])
            .unwrap();
    }

    let image = decode_image_bytes(
        &bytes,
        ImageFormat::Gif,
        AnimatedImageConfig::default(),
        None,
    )
    .unwrap();

    assert!(image.is_animated());
    assert_eq!(image.frame_count(), 2);
    assert_eq!(image.as_bytes(0).unwrap(), &[0, 0, 255, 255]);
}

#[test]
fn apng_decode_helper_keeps_multiple_frames() {
    let bytes = animated_png_bytes();
    let image = decode_image_bytes(
        &bytes,
        ImageFormat::Png,
        AnimatedImageConfig::default(),
        None,
    )
    .unwrap();

    assert!(image.is_animated());
    assert_eq!(image.frame_count(), 2);
}

#[test]
fn static_png_is_not_treated_as_animation() {
    let mut bytes = Vec::new();
    image::codecs::png::PngEncoder::new(&mut bytes)
        .write_image(&[255, 0, 0, 255], 1, 1, ExtendedColorType::Rgba8)
        .unwrap();

    let image = decode_image_bytes(
        &bytes,
        ImageFormat::Png,
        AnimatedImageConfig::default(),
        None,
    )
    .unwrap();

    assert!(!image.is_animated());
    assert_eq!(image.frame_count(), 1);
}

#[test]
fn png_target_decode_uses_element_sized_resident_buffer() {
    let bytes = encoded_rgba_image(128, 96, |writer| {
        image::codecs::png::PngEncoder::new(writer).write_image(
            &solid_rgba_pixels(128, 96),
            128,
            96,
            ExtendedColorType::Rgba8,
        )
    });
    let target = ImageDecodeTarget::new(32, 24).unwrap();
    let (image, metadata) = decode_image_bytes_to_target(
        &bytes,
        ImageFormat::Png,
        AnimatedImageConfig::default(),
        target,
        ObjectFit::Fill,
    )
    .unwrap();

    assert_eq!(image.size(0), target.size());
    assert_eq!(image.decoded_byte_len(), 32 * 24 * 4);
    assert!(
        metadata.decode_mode == "png_row_sample_decode"
            || metadata.decode_mode == "decoder_scaled_then_resized"
    );
}

#[test]
fn jpeg_target_decode_uses_scaled_decoder_before_resizing() {
    let bytes = encoded_rgba_image(128, 96, |writer| {
        image::codecs::jpeg::JpegEncoder::new_with_quality(writer, 90).write_image(
            &solid_rgb_pixels(128, 96),
            128,
            96,
            ExtendedColorType::Rgb8,
        )
    });
    let target = ImageDecodeTarget::new(32, 24).unwrap();
    let (image, metadata) = decode_image_bytes_to_target(
        &bytes,
        ImageFormat::Jpeg,
        AnimatedImageConfig::default(),
        target,
        ObjectFit::Fill,
    )
    .unwrap();

    assert_eq!(image.size(0), target.size());
    assert_eq!(image.decoded_byte_len(), 32 * 24 * 4);
    assert!(
        metadata.decode_mode == "jpeg_scaled_decode"
            || metadata.decode_mode == "decoder_scaled_then_resized"
    );
}

#[test]
fn bmp_target_decode_samples_rows_without_retaining_original_size() {
    let bytes = encoded_rgba_image(128, 96, |writer| {
        image::codecs::bmp::BmpEncoder::new(writer).write_image(
            &solid_rgba_pixels(128, 96),
            128,
            96,
            ExtendedColorType::Rgba8,
        )
    });
    let target = ImageDecodeTarget::new(32, 24).unwrap();
    let (image, metadata) = decode_image_bytes_to_target(
        &bytes,
        ImageFormat::Bmp,
        AnimatedImageConfig::default(),
        target,
        ObjectFit::Fill,
    )
    .unwrap();

    assert_eq!(image.size(0), target.size());
    assert_eq!(image.decoded_byte_len(), 32 * 24 * 4);
    assert!(
        metadata.decode_mode == "bmp_rect_sample_decode"
            || metadata.decode_mode == "decoder_scaled_then_resized"
    );
}

#[test]
fn target_decode_keeps_animated_png_playable_after_resize() {
    let bytes = animated_png_bytes_with_size(64, 64);
    let config = AnimatedImageConfig {
        max_resident_bytes: 4 * 4 * 4 * 2,
        ..AnimatedImageConfig::default()
    };
    let target = ImageDecodeTarget::new(4, 4).unwrap();
    let (image, metadata) =
        decode_image_bytes_to_target(&bytes, ImageFormat::Png, config, target, ObjectFit::Fill)
            .unwrap();

    assert!(image.is_animated());
    assert_eq!(image.frame_count(), 2);
    assert_eq!(image.size(0), target.size());
    assert_eq!(image.size(1), target.size());
    assert_eq!(image.decoded_byte_len(), 4 * 4 * 4 * 2);
    assert_eq!(metadata.decode_mode, "animated_frame_sample_decode");
}

#[test]
fn target_decode_streams_large_animation_after_resize() {
    let bytes = animated_png_bytes_with_size(64, 64);
    let config = AnimatedImageConfig {
        max_resident_frames: 1,
        max_resident_bytes: 4 * 4 * 4,
        ..AnimatedImageConfig::default()
    };
    let target = ImageDecodeTarget::new(4, 4).unwrap();
    let (image, _) =
        decode_image_bytes_to_target(&bytes, ImageFormat::Png, config, target, ObjectFit::Fill)
            .unwrap();

    assert!(matches!(image.data, RenderImageData::Streaming(_)));
    assert!(image.is_animated());
    assert_eq!(image.frame_count(), usize::MAX);
    assert_eq!(image.size(0), target.size());
}

#[test]
fn large_animation_enters_streaming_mode() {
    let bytes = animated_png_bytes();
    let config = AnimatedImageConfig {
        max_resident_frames: 1,
        max_resident_bytes: 4,
        ..AnimatedImageConfig::default()
    };
    let image = decode_image_bytes(&bytes, ImageFormat::Png, config, None).unwrap();

    assert!(matches!(image.data, RenderImageData::Resident(_)));

    let image = decode_image_bytes(
        &bytes,
        ImageFormat::Png,
        config,
        Some(BackgroundExecutor::new(std::sync::Arc::new(
            crate::TestDispatcher::new(rand::rngs::StdRng::seed_from_u64(1)),
        ))),
    )
    .unwrap();

    assert!(matches!(image.data, RenderImageData::Streaming(_)));
}

#[test]
fn streaming_animation_decoded_byte_len_uses_resident_frames_only() {
    let bytes = animated_png_bytes();
    let config = AnimatedImageConfig {
        max_resident_frames: 1,
        max_resident_bytes: 4,
        ..AnimatedImageConfig::default()
    };
    let image = decode_image_bytes(
        &bytes,
        ImageFormat::Png,
        config,
        Some(BackgroundExecutor::new(std::sync::Arc::new(
            crate::TestDispatcher::new(rand::rngs::StdRng::seed_from_u64(2)),
        ))),
    )
    .unwrap();

    assert!(matches!(image.data, RenderImageData::Streaming(_)));
    assert_eq!(image.frame_count(), usize::MAX);
    assert_eq!(image.decoded_byte_len(), image.frame_byte_len(0));
}

#[test]
fn streaming_animation_keeps_decoder_running_while_queue_is_full() {
    let bytes = animated_png_bytes_with_frame_count(4);
    let config = AnimatedImageConfig {
        decode_ahead_frames: 2,
        max_resident_frames: 1,
        max_resident_bytes: 4,
        ..AnimatedImageConfig::default()
    };
    let executor = BackgroundExecutor::new(std::sync::Arc::new(crate::TestDispatcher::new(
        rand::rngs::StdRng::seed_from_u64(3),
    )));
    let image =
        decode_image_bytes(&bytes, ImageFormat::Png, config, Some(executor.clone())).unwrap();

    executor.run_until_parked();
    let RenderImageData::Streaming(state) = &image.data else {
        panic!("large animation should use streaming decode");
    };

    assert!(state.decode_task_running.load(Ordering::Acquire));
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
        let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
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
