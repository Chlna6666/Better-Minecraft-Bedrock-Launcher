use super::*;
use image::Delay;

fn varied_pixels() -> RgbaImage {
    RgbaImage::from_fn(9, 7, |x, y| {
        Rgba([
            (x * 23 + y * 7) as u8,
            (y * 31 + x * 3) as u8,
            (x * 11 + y * 17) as u8,
            ((x + y) * 15) as u8,
        ])
    })
}

#[test]
fn pooled_resize_preserves_bgra_pixels_and_sequence() {
    for dimension in [2, 6] {
        let frame = AnimatedFrame::from_bgra_bytes(
            7,
            size(3.into(), 3.into()),
            [201, 93, 17, 255].repeat(9),
        );
        let target = ImageRenderSize::new(dimension, dimension).unwrap();
        let resized = resample_bgra_frame(frame, target).unwrap();
        assert_eq!(resized.sequence(), 7);
        assert_eq!(resized.size(), target.size());
        assert_eq!(
            resized.bytes(),
            [201, 93, 17, 255].repeat((dimension * dimension) as usize)
        );
    }
}

#[test]
fn pooled_resize_rejects_incomplete_source_pixels() {
    let frame = AnimatedFrame::from_bgra_bytes(0, size(3.into(), 3.into()), vec![0; 35]);
    assert!(resample_bgra_frame(frame, ImageRenderSize::new(2, 2).unwrap()).is_err());
}

#[test]
fn direct_bgra_lanczos_matches_channel_conversion_path() {
    let rgba = varied_pixels();
    let source = ImageRenderSize::new(rgba.width(), rgba.height()).unwrap();
    let bgra = AnimatedFrame::from_rgba_image(0, rgba.clone());
    for (width, height) in [(9, 7), (3, 2), (17, 11)] {
        let target = ImageRenderSize::new(width, height).unwrap();
        let (reference, reference_path) = resize_rgba_frame(rgba.clone(), target, "test").unwrap();
        let reference = AnimatedFrame::from_rgba_image(0, reference);
        let (actual, actual_path) =
            resize_bgra_bytes(bgra.bytes().to_vec(), source, target, "test").unwrap();
        assert_eq!(actual.bytes(), reference.bytes());
        assert_eq!(actual.pixel_format(), ImagePixelFormat::Bgra8);
        assert_eq!(actual.size(), reference.size());
        assert_eq!(actual_path, reference_path);
    }
}

#[test]
fn fused_rgba_sampling_preserves_pixels_sequence_and_delay() {
    let pixels = varied_pixels();
    let delay = Delay::from_numer_denom_ms(1000, 60);
    for (width, height) in [(9, 7), (3, 2), (17, 11)] {
        let frame = Frame::from_parts(pixels.clone(), 3, 5, delay);
        let target = ImageRenderSize::new(width, height).unwrap();
        let reference =
            resample_bgra_frame(AnimatedFrame::from_rgba_frame(42, frame.clone()), target).unwrap();
        let actual = resample_rgba_frame(42, frame, target).unwrap();
        assert_eq!(actual.bytes(), reference.bytes());
        assert_eq!(actual.size(), reference.size());
        assert_eq!(actual.sequence(), reference.sequence());
        assert_eq!(actual.delay().numer_denom_ms(), delay.numer_denom_ms());
        assert_eq!(actual.pixel_format(), ImagePixelFormat::Bgra8);
    }
}

#[test]
fn direct_bgra_resize_rejects_incomplete_pixels() {
    let source = ImageRenderSize::new(3, 3).unwrap();
    assert!(resize_bgra_bytes(vec![0; 35], source, source, "test").is_err());
}

#[test]
fn fused_rgba_sampling_rejects_empty_source() {
    let frame = Frame::new(RgbaImage::new(0, 0));
    assert!(resample_rgba_frame(0, frame, ImageRenderSize::new(2, 2).unwrap()).is_err());
}
