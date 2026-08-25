use super::upload_encoding::encode_bgra_upload_with_padding;
use super::*;
use crate::size;

fn expected_upload(
    source: &[u8],
    width: usize,
    height: usize,
    padding: usize,
    texture_kind: AtlasTextureKind,
) -> Vec<u8> {
    let upload_width = width + padding * 2;
    let upload_height = height + padding * 2;
    let mut expected = Vec::with_capacity(upload_width * upload_height * 4);
    for upload_y in 0..upload_height {
        let source_y = upload_y.saturating_sub(padding).min(height - 1);
        for upload_x in 0..upload_width {
            let source_x = upload_x.saturating_sub(padding).min(width - 1);
            append_expected_pixel(
                &mut expected,
                source,
                source_y * width + source_x,
                texture_kind,
            );
        }
    }
    expected
}

fn append_expected_pixel(
    expected: &mut Vec<u8>,
    source: &[u8],
    source_pixel: usize,
    texture_kind: AtlasTextureKind,
) {
    if texture_kind == AtlasTextureKind::Monochrome {
        expected.extend_from_slice(&[0, 0, source[source_pixel], 255]);
        return;
    }
    let index = source_pixel * 4;
    let pixel = &source[index..index + 4];
    let output = match texture_kind {
        AtlasTextureKind::Bgra => [pixel[0], pixel[1], pixel[2], pixel[3]],
        AtlasTextureKind::Rgba => [pixel[2], pixel[1], pixel[0], pixel[3]],
        AtlasTextureKind::Subpixel => {
            let alpha = u16::from(pixel[3]);
            let channel = |value| u8::try_from(u16::from(value) * alpha / 255).unwrap_or(u8::MAX);
            [channel(pixel[2]), channel(pixel[1]), channel(pixel[0]), 255]
        }
        AtlasTextureKind::Monochrome => unreachable!(),
    };
    expected.extend_from_slice(&output);
}

#[test]
fn atlas_padding_replicates_converted_edge_pixels() {
    for (width, height) in [(1, 1), (2, 3), (7, 4)] {
        for padding in [0, 1, 2] {
            for texture_kind in [
                AtlasTextureKind::Monochrome,
                AtlasTextureKind::Bgra,
                AtlasTextureKind::Rgba,
                AtlasTextureKind::Subpixel,
            ] {
                let source_bytes_per_pixel =
                    usize::from(texture_kind != AtlasTextureKind::Monochrome) * 3 + 1;
                let source = (0..width * height * source_bytes_per_pixel)
                    .map(|index| u8::try_from(index % 251).expect("modulo 251 must fit u8"))
                    .collect::<Vec<_>>();
                let mut destination = vec![0; (width + padding * 2) * (height + padding * 2) * 4];
                encode_bgra_upload_with_padding(
                    &mut destination,
                    size(
                        DevicePixels(i32::try_from(width).expect("test width must fit i32")),
                        DevicePixels(i32::try_from(height).expect("test height must fit i32")),
                    ),
                    &source,
                    texture_kind,
                    u32::try_from(padding).expect("test padding must fit u32"),
                )
                .expect("valid atlas upload must encode");

                assert_eq!(
                    destination,
                    expected_upload(&source, width, height, padding, texture_kind)
                );
            }
        }
    }
}
