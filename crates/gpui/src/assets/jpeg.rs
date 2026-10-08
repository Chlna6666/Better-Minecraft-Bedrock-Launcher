use crate::{ObjectFit, Result, size};
use image::{DynamicImage, RgbaImage, metadata::Orientation};
use smallvec::SmallVec;
use std::io::Cursor;

use super::resample::resize_rgba_frame;
use crate::assets::{AnimatedFrame, RenderImage};
use crate::assets::{ImageRenderInfo, ImageRenderSize};

pub(super) fn frame(bytes: &[u8]) -> Result<AnimatedFrame> {
    let orientation = jpeg_orientation(bytes);
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    let pixels = decoder.decode()?;
    let info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report image dimensions"))?;
    let bgra = apply_orientation(jpeg_pixels_to_bgra_image(pixels, info)?, orientation);
    Ok(AnimatedFrame::from_bgra_image(0, bgra))
}

pub(super) fn render_sized(
    bytes: &[u8],
    target: ImageRenderSize,
    object_fit: ObjectFit,
) -> Result<(RenderImage, ImageRenderInfo)> {
    let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    decoder.read_info()?;
    let original_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report image dimensions"))?;
    let orientation = jpeg_orientation(bytes);
    let encoded_size = size(
        u32::from(original_info.width),
        u32::from(original_info.height),
    );
    let original_size = if orientation_swaps_axes(orientation) {
        size(encoded_size.height, encoded_size.width)
    } else {
        encoded_size
    };
    let fitted_target = target.fit(original_size, object_fit);

    // jpeg-decoder scales in encoded-image coordinates. For orientations that rotate the image by
    // 90/270 degrees, map the final display target back to encoded axes before decoding so the
    // optimized target-size path never undersamples and then has to upscale after rotation.
    let decode_width = if orientation_swaps_axes(orientation) {
        fitted_target.height
    } else {
        fitted_target.width
    };
    let decode_height = if orientation_swaps_axes(orientation) {
        fitted_target.width
    } else {
        fitted_target.height
    };
    let requested_width = u16::try_from(decode_width.min(u32::from(u16::MAX)))?;
    let requested_height = u16::try_from(decode_height.min(u32::from(u16::MAX)))?;
    decoder.scale(requested_width.max(1), requested_height.max(1))?;
    let pixels = decoder.decode()?;
    let scaled_info = decoder
        .info()
        .ok_or_else(|| anyhow::anyhow!("JPEG decoder did not report scaled dimensions"))?;
    let bgra = apply_orientation(jpeg_pixels_to_bgra_image(pixels, scaled_info)?, orientation);
    // Orientation and resizing operate on each channel independently, including BGRA buffers.
    let (bgra, render_path) = resize_rgba_frame(bgra, fitted_target, "jpeg_scaled")?;
    let frame = AnimatedFrame::from_bgra_image(0, bgra);
    let image = RenderImage::from_resident_frames(SmallVec::from_elem(frame, 1));

    Ok((
        image,
        ImageRenderInfo {
            original_width: original_size.width,
            original_height: original_size.height,
            size: fitted_target,
            render_path,
        },
    ))
}

#[inline]
fn orientation_swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

fn apply_orientation(rgba: RgbaImage, orientation: Orientation) -> RgbaImage {
    if orientation == Orientation::NoTransforms {
        return rgba;
    }

    let mut image = DynamicImage::ImageRgba8(rgba);
    image.apply_orientation(orientation);
    image.into_rgba8()
}

/// Reads EXIF orientation directly from JPEG APP1 metadata without decoding image pixels.
///
/// The scan stops at SOS because APP segments are metadata and should precede entropy-coded image
/// data. Invalid/truncated metadata is deliberately treated as no transform so a decodable JPEG is
/// never rejected solely because of malformed EXIF.
fn jpeg_orientation(bytes: &[u8]) -> Orientation {
    if bytes.len() < 4 || bytes[0..2] != [0xff, 0xd8] {
        return Orientation::NoTransforms;
    }

    let mut offset = 2usize;
    while offset < bytes.len() {
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        let Some(&marker) = bytes.get(offset) else {
            break;
        };
        offset += 1;

        match marker {
            0xd9 | 0xda => break,           // EOI / SOS
            0x01 | 0xd0..=0xd7 => continue, // TEM / restart markers have no payload length.
            0x00 => break,
            _ => {}
        }

        let Some(length_bytes) = bytes.get(offset..offset.saturating_add(2)) else {
            break;
        };
        let segment_len = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
        if segment_len < 2 {
            break;
        }
        offset += 2;
        let payload_len = segment_len - 2;
        let Some(payload) = bytes.get(offset..offset.saturating_add(payload_len)) else {
            break;
        };

        if marker == 0xe1
            && let Some(exif) = payload.strip_prefix(b"Exif\0\0")
            && let Some(orientation) = Orientation::from_exif_chunk(exif)
        {
            return orientation;
        }

        offset += payload_len;
    }

    Orientation::NoTransforms
}

fn jpeg_pixels_to_bgra_image(
    mut pixels: Vec<u8>,
    info: jpeg_decoder::ImageInfo,
) -> Result<RgbaImage> {
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    let pixel_count = width as usize * height as usize;
    anyhow::ensure!(
        pixels.len() == pixel_count * info.pixel_format.pixel_bytes(),
        "JPEG decoded buffer dimensions were invalid"
    );
    if info.pixel_format == jpeg_decoder::PixelFormat::CMYK32 {
        for pixel in pixels.chunks_exact_mut(4) {
            let c = u16::from(pixel[0]);
            let m = u16::from(pixel[1]);
            let y = u16::from(pixel[2]);
            let k = u16::from(pixel[3]);
            let convert = |channel: u16| {
                255u8.saturating_sub(((channel * (255 - k)) / 255 + k).min(255) as u8)
            };
            pixel.copy_from_slice(&[convert(y), convert(m), convert(c), 255]);
        }
        return RgbaImage::from_raw(width, height, pixels)
            .ok_or_else(|| anyhow::anyhow!("JPEG decoded buffer dimensions were invalid"));
    }
    let mut bgra = crate::acquire_bitmap_buffer_capacity(pixel_count * 4);

    match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => {
            for &luma in &pixels {
                bgra.extend_from_slice(&[luma, luma, luma, 255]);
            }
        }
        jpeg_decoder::PixelFormat::L16 => {
            for luma in pixels.chunks_exact(2) {
                let luma = luma[0];
                bgra.extend_from_slice(&[luma, luma, luma, 255]);
            }
        }
        jpeg_decoder::PixelFormat::RGB24 => {
            for pixel in pixels.chunks_exact(3) {
                bgra.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
            }
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            unreachable!("CMYK pixels are converted in place above");
        }
    }

    RgbaImage::from_raw(width, height, bgra)
        .ok_or_else(|| anyhow::anyhow!("JPEG decoded buffer dimensions were invalid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image_info(pixel_format: jpeg_decoder::PixelFormat) -> jpeg_decoder::ImageInfo {
        jpeg_decoder::ImageInfo {
            width: 1,
            height: 1,
            pixel_format,
            coding_process: jpeg_decoder::CodingProcess::DctSequential,
        }
    }

    #[test]
    fn decoder_colors_convert_directly_to_bgra() {
        use jpeg_decoder::PixelFormat::{CMYK32, L8, L16, RGB24};
        for (format, input, expected) in [
            (L8, vec![23], [23, 23, 23, 255]),
            (L16, vec![23, 127], [23, 23, 23, 255]),
            (RGB24, vec![11, 23, 37], [37, 23, 11, 255]),
            (CMYK32, vec![11, 23, 37, 79], [151, 161, 169, 255]),
        ] {
            let output = jpeg_pixels_to_bgra_image(input, image_info(format)).unwrap();
            assert_eq!(output.as_raw(), &expected);
        }
    }

    #[test]
    fn cmyk_conversion_preserves_allocation_and_extreme_values() {
        use jpeg_decoder::PixelFormat::CMYK32;
        let pixels = vec![0, 255, 127, 0];
        let pointer = pixels.as_ptr();
        let output = jpeg_pixels_to_bgra_image(pixels, image_info(CMYK32)).unwrap();
        assert_eq!(output.as_raw().as_ptr(), pointer);
        assert_eq!(output.as_raw(), &[128, 0, 255, 255]);
        let output = jpeg_pixels_to_bgra_image(vec![255, 0, 127, 255], image_info(CMYK32)).unwrap();
        assert_eq!(output.as_raw(), &[0, 0, 0, 255]);
        assert!(jpeg_pixels_to_bgra_image(vec![0; 3], image_info(CMYK32)).is_err());
        assert!(frame(b"not a JPEG").is_err());
    }

    #[test]
    fn all_exif_orientations_preserve_decoded_pixels() {
        use image::{ExtendedColorType, ImageEncoder as _};
        let mut encoded = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, 90)
            .write_image(
                &[
                    11, 23, 37, 47, 59, 71, 83, 97, 109, 127, 139, 151, 163, 179, 191, 211, 223,
                    239,
                ],
                3,
                2,
                ExtendedColorType::Rgb8,
            )
            .unwrap();
        let mut decoder = jpeg_decoder::Decoder::new(Cursor::new(&encoded));
        let rgb = decoder.decode().unwrap();
        for exif in 1..=8 {
            let mut oriented = jpeg_with_little_endian_orientation(exif);
            // The metadata-only fixture ends at EOI; replace it with the encoded frame body.
            oriented.truncate(oriented.len() - 2);
            oriented.extend_from_slice(&encoded[2..]);
            let rgba: Vec<u8> = rgb
                .chunks_exact(3)
                .flat_map(|pixel| [pixel[0], pixel[1], pixel[2], 255])
                .collect();
            let mut expected = apply_orientation(
                RgbaImage::from_raw(3, 2, rgba).unwrap(),
                jpeg_orientation(&oriented),
            );
            for pixel in expected.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let actual = frame(&oriented).unwrap();
            assert_eq!(
                actual.size,
                size(expected.width().into(), expected.height().into())
            );
            assert_eq!(actual.bytes.as_slice(), expected.as_raw());
        }
    }

    fn jpeg_with_little_endian_orientation(value: u16) -> Vec<u8> {
        // TIFF header + one IFD entry for tag 0x0112 (Orientation), type SHORT, count 1.
        let mut exif = vec![
            b'I',
            b'I',
            0x2a,
            0x00,
            0x08,
            0x00,
            0x00,
            0x00,
            0x01,
            0x00,
            0x12,
            0x01,
            0x03,
            0x00,
            0x01,
            0x00,
            0x00,
            0x00,
            value as u8,
            (value >> 8) as u8,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
            0x00,
        ];
        let mut payload = b"Exif\0\0".to_vec();
        payload.append(&mut exif);
        let segment_len = u16::try_from(payload.len() + 2).unwrap();

        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&segment_len.to_be_bytes());
        jpeg.extend_from_slice(&payload);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    #[test]
    fn reads_exif_orientation_without_decoding_pixels() {
        assert_eq!(
            jpeg_orientation(&jpeg_with_little_endian_orientation(6)),
            Orientation::Rotate90
        );
        assert_eq!(
            jpeg_orientation(&jpeg_with_little_endian_orientation(1)),
            Orientation::NoTransforms
        );
    }

    #[test]
    fn rotated_exif_orientations_swap_layout_axes() {
        for orientation in [
            Orientation::Rotate90,
            Orientation::Rotate270,
            Orientation::Rotate90FlipH,
            Orientation::Rotate270FlipH,
        ] {
            assert!(orientation_swaps_axes(orientation));
        }
        for orientation in [
            Orientation::NoTransforms,
            Orientation::Rotate180,
            Orientation::FlipHorizontal,
            Orientation::FlipVertical,
        ] {
            assert!(!orientation_swaps_axes(orientation));
        }
    }
}
