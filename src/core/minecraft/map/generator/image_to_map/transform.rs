use image::{RgbaImage, imageops};

/// Source orientation. Mirrors are applied on the source axes before rotation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    /// Clockwise quarter turns, reduced modulo four.
    pub quarter_turns: u8,
    /// Reflect the west/east axis.
    pub mirror_x: bool,
    /// Reflect the north/south axis.
    pub mirror_z: bool,
}

pub(super) fn apply(mut image: RgbaImage, options: Options) -> RgbaImage {
    if options.mirror_x {
        imageops::flip_horizontal_in_place(&mut image);
    }
    if options.mirror_z {
        imageops::flip_vertical_in_place(&mut image);
    }
    match options.quarter_turns % 4 {
        1 => imageops::rotate90(&image),
        2 => imageops::rotate180(&image),
        3 => imageops::rotate270(&image),
        _ => image,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn rotation_swaps_rectangular_grid_axes_and_pixel_order() {
        let source = RgbaImage::from_fn(2, 3, |x, y| Rgba([(y * 2 + x) as u8, 0, 0, 255]));
        let rotated = apply(
            source.clone(),
            Options {
                quarter_turns: 1,
                ..Options::default()
            },
        );
        assert_eq!(rotated.dimensions(), (3, 2));
        assert_eq!(
            rotated.pixels().map(|pixel| pixel[0]).collect::<Vec<_>>(),
            [4, 2, 0, 5, 3, 1]
        );
        assert_eq!(
            apply(
                rotated,
                Options {
                    quarter_turns: 3,
                    ..Options::default()
                }
            ),
            source
        );
    }

    #[test]
    fn mirrors_precede_rotation_on_the_source_axes() {
        let source = RgbaImage::from_fn(2, 3, |x, y| Rgba([(y * 2 + x) as u8, 0, 0, 255]));
        let transformed = apply(
            source,
            Options {
                quarter_turns: 1,
                mirror_x: true,
                mirror_z: false,
            },
        );
        assert_eq!(
            transformed
                .pixels()
                .map(|pixel| pixel[0])
                .collect::<Vec<_>>(),
            [5, 3, 1, 4, 2, 0]
        );
    }

    #[test]
    fn mirroring_both_axes_matches_a_half_turn() {
        let source = RgbaImage::from_fn(2, 3, |x, y| Rgba([(y * 2 + x) as u8, 0, 0, 255]));
        let mirrored = apply(
            source.clone(),
            Options {
                mirror_x: true,
                mirror_z: true,
                ..Options::default()
            },
        );
        let rotated = apply(
            source,
            Options {
                quarter_turns: 2,
                ..Options::default()
            },
        );
        assert_eq!(mirrored, rotated);
    }
}
