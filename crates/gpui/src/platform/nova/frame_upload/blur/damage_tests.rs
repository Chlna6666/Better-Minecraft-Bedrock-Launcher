use super::*;
use crate::platform::nova::DrawableSize;
use crate::platform::nova::blur_damage::blur_damage_scissors;
use crate::{DirtyRegion, ScaledPixels};
use gfx_core::ScissorRect;

#[test]
fn downsampled_damage_refreshes_composite_edge_sampling_dependencies() {
    let drawable = DrawableSize {
        width: 32,
        height: 32,
    };
    let config = BackdropBlurConfig::new(0, 0, 0, 2, 1, 2., [6., 6., 20., 20.], false);
    let mut damage = DirtyRegion::empty();
    damage.push(crate::bounds(
        crate::point(ScaledPixels(8.), ScaledPixels(8.)),
        crate::size(ScaledPixels(16.), ScaledPixels(16.)),
    ));
    let scissors = blur_damage_scissors(config, drawable, &damage).unwrap();
    // Final target texels 2 and 13 contribute to screen pixels 6 and 25 under linear sampling.
    assert_eq!(
        scissors.final_output,
        ScissorRect {
            x: 4,
            y: 4,
            width: 24,
            height: 24
        }
    );
    assert_eq!(
        scissors.horizontal_output,
        ScissorRect {
            x: 4,
            y: 0,
            width: 24,
            height: 32
        }
    );
    assert_eq!(
        scissors.source_capture,
        ScissorRect {
            x: 0,
            y: 0,
            width: 32,
            height: 32
        }
    );
}

#[test]
fn identity_damage_needs_no_downsample_sampling_halo() {
    let drawable = DrawableSize {
        width: 32,
        height: 32,
    };
    let config = BackdropBlurConfig::new(0, 0, 0, 1, 1, 2., [6., 6., 20., 20.], false);
    let mut damage = DirtyRegion::empty();
    damage.push(crate::bounds(
        crate::point(ScaledPixels(8.), ScaledPixels(8.)),
        crate::size(ScaledPixels(16.), ScaledPixels(16.)),
    ));
    assert_eq!(
        blur_damage_scissors(config, drawable, &damage)
            .unwrap()
            .final_output,
        ScissorRect {
            x: 6,
            y: 6,
            width: 20,
            height: 20
        }
    );
}
