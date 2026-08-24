use super::*;
use image::{ExtendedColorType, ImageEncoder as _, codecs::webp::WebPEncoder};
use std::{thread, time::Instant};

#[test]
fn animated_webp_decode_keeps_multiple_frames() {
    let bytes = animated_webp_bytes(8, 6);
    let image = decode_image_bytes(
        &bytes,
        ImageFormat::WebP,
        AnimatedImageConfig::default(),
        None,
    )
    .unwrap();

    assert!(image.is_animated());
    assert_eq!(image.frame_count(), 2);
    assert_eq!(image.size(0), size(8.into(), 6.into()));
}

#[test]
fn animated_webp_target_decode_resizes_streamed_frames() {
    let bytes = animated_webp_bytes(8, 6);
    let config = AnimatedImageConfig {
        max_resident_frames: 1,
        max_resident_bytes: 4 * 3 * 4,
        ..AnimatedImageConfig::default()
    };
    let target = ImageDecodeTarget::new(4, 3).unwrap();
    let (image, _) =
        decode_image_bytes_to_target(&bytes, ImageFormat::WebP, config, target, ObjectFit::Fill)
            .unwrap();

    assert!(matches!(image.data, RenderImageData::Streaming(_)));
    assert_eq!(image.size(0), target.size());
}

#[test]
fn animated_webp_streaming_sequences_remain_monotonic_across_cycles() {
    let bytes = animated_webp_bytes(1, 1);
    let config = AnimatedImageConfig {
        decode_ahead_frames: 2,
        max_resident_frames: 1,
        max_resident_bytes: 4,
        ..AnimatedImageConfig::default()
    };
    let executor = BackgroundExecutor::new(Arc::new(crate::TestDispatcher::new(
        rand::rngs::StdRng::seed_from_u64(4),
    )));
    let image =
        decode_image_bytes(&bytes, ImageFormat::WebP, config, Some(executor.clone())).unwrap();

    let mut current_sequence = 0;
    for expected_sequence in 1..=4 {
        current_sequence = wait_for_streaming_sequence(&image, current_sequence, &executor);
        assert_eq!(current_sequence, expected_sequence);
        assert!(
            image.decoded_byte_len()
                >= image
                    .frame_byte_len(0)
                    .saturating_add(image.frame_byte_len(current_sequence))
        );
    }
}

fn wait_for_streaming_sequence(
    image: &RenderImage,
    current_sequence: usize,
    executor: &BackgroundExecutor,
) -> usize {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(frame) = image.next_streaming_frame(current_sequence, executor) {
            return frame.sequence();
        }
        assert!(
            Instant::now() < deadline,
            "streaming animation did not produce a frame after sequence {current_sequence}"
        );
        thread::yield_now();
    }
}

fn animated_webp_bytes(width: u32, height: u32) -> Vec<u8> {
    let red = encoded_lossless_webp(width, height, [255, 0, 0, 255]);
    let green = encoded_lossless_webp(width, height, [0, 255, 0, 255]);

    let mut webp = Vec::new();
    webp.extend_from_slice(b"WEBP");

    let mut vp8x = vec![0x02, 0, 0, 0];
    push_u24(&mut vp8x, width.saturating_sub(1));
    push_u24(&mut vp8x, height.saturating_sub(1));
    push_chunk(&mut webp, *b"VP8X", &vp8x);
    push_chunk(&mut webp, *b"ANIM", &[0, 0, 0, 0, 0, 0]);
    push_animation_frame(&mut webp, width, height, &red[12..]);
    push_animation_frame(&mut webp, width, height, &green[12..]);

    let mut riff = Vec::with_capacity(webp.len() + 8);
    riff.extend_from_slice(b"RIFF");
    riff.extend_from_slice(&u32::try_from(webp.len()).unwrap().to_le_bytes());
    riff.extend_from_slice(&webp);
    riff
}

fn encoded_lossless_webp(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let pixels = solid_color_rgba_pixels(width, height, color);
    let mut bytes = Vec::new();
    WebPEncoder::new_lossless(&mut bytes)
        .write_image(&pixels, width, height, ExtendedColorType::Rgba8)
        .unwrap();
    bytes
}

fn push_animation_frame(output: &mut Vec<u8>, width: u32, height: u32, frame_chunks: &[u8]) {
    let mut frame = Vec::with_capacity(16 + frame_chunks.len());
    push_u24(&mut frame, 0);
    push_u24(&mut frame, 0);
    push_u24(&mut frame, width.saturating_sub(1));
    push_u24(&mut frame, height.saturating_sub(1));
    push_u24(&mut frame, 20);
    frame.push(0);
    frame.extend_from_slice(frame_chunks);
    push_chunk(output, *b"ANMF", &frame);
}

fn push_chunk(output: &mut Vec<u8>, kind: [u8; 4], payload: &[u8]) {
    output.extend_from_slice(&kind);
    output.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    output.extend_from_slice(payload);
    if payload.len() % 2 != 0 {
        output.push(0);
    }
}

fn push_u24(output: &mut Vec<u8>, value: u32) {
    let bytes = value.to_le_bytes();
    output.extend_from_slice(&bytes[..3]);
}
