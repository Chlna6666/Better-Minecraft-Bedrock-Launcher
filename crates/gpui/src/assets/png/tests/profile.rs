use super::*;

#[test]
#[ignore = "isolated bitmap-pool profile; run explicitly with --test-threads=1"]
fn decode_buffer_reuse_profile() {
    const WIDTH: u32 = 1920;
    const HEIGHT: u32 = 1080;
    let pixels: Vec<u8> = (0..WIDTH as usize * HEIGHT as usize * 4)
        .map(|index| index.wrapping_mul(37) as u8)
        .collect();
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, WIDTH, HEIGHT);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .expect("header")
            .write_image_data(&pixels)
            .expect("pixels");
    }
    let mut expected = pixels;
    for pixel in expected.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }

    crate::trim_global_bitmap_pool_to(0);
    for _ in 0..32 {
        // Previous decoder storage policy: a fresh exact-size Vec is released into the pool.
        let mut decoder = png::Decoder::new(Cursor::new(&encoded));
        decoder.set_transformations(png::Transformations::normalize_to_color8());
        let mut reader = decoder.read_info().expect("reader");
        let mut pixels = vec![0; reader.output_buffer_size().expect("output size")];
        let info = reader.next_frame(&mut pixels).expect("frame");
        pixels.truncate(info.buffer_size());
        let output =
            png_pixels_to_bgra_bytes(pixels, info.color_type, WIDTH, HEIGHT).expect("valid pixels");
        assert_eq!(output, expected);
        crate::release_bitmap_buffer(output);
    }
    let before = crate::assets::bitmap_pool::global_bitmap_pool().snapshot();

    crate::trim_global_bitmap_pool_to(0);
    let mut address = None;
    for _ in 0..32 {
        let frame = frame(&encoded).expect("frame");
        assert_eq!(frame.bytes.as_slice(), expected);
        if let Some(address) = address {
            assert_eq!(frame.bytes.as_slice().as_ptr(), address);
        }
        address = Some(frame.bytes.as_slice().as_ptr());
    }
    let after = crate::assets::bitmap_pool::global_bitmap_pool().snapshot();
    assert_eq!(after.free_buffers, 1);
    assert!(after.retained_bytes < before.retained_bytes);
    println!(
        "PNG_POOL_REUSE decodes=32 before_buffers={} after_buffers={} before_bytes={} after_bytes={} pixel_exact=true pointer_reuse=true",
        before.free_buffers, after.free_buffers, before.retained_bytes, after.retained_bytes
    );
    crate::trim_global_bitmap_pool_to(0);
}
