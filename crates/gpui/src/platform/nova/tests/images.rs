use super::*;

pub(super) fn encode_images(upload: &mut FrameUpload, sprites: &[PolychromeSprite], target: u32) {
    let mut painter = crate::scene::Painter::default();
    for sprite in sprites {
        painter.push_layer(sprite.content_mask.bounds);
        painter.insert_primitive(*sprite);
        painter.pop_layer();
    }
    painter.finish();
    upload.encode(
        &mut RetainedChunkCache::default(),
        &painter,
        &crate::window::retained::RetainedPaint::default(),
        &[],
        DrawableSize {
            width: 256,
            height: 256,
        },
        DrawableSize {
            width: target,
            height: target,
        },
        &RenderingParameters::system(),
        false,
        BackdropBlurQuality::Full,
    );
}

fn image(edge: f32) -> PolychromeSprite {
    PolychromeSprite {
        order: 0,
        sampling: 0,
        animation_id: None,
        grayscale: false,
        opacity: 1.,
        bounds: bounds(point(px(0.), px(0.)), size(px(edge), px(edge))).scale(1.),
        content_mask: crate::ContentMask::new(
            bounds(point(px(0.), px(0.)), size(px(256.), px(256.))).scale(1.),
        ),
        corner_radii: Default::default(),
        tile: test_atlas_tile(),
    }
}

fn selected(upload: &FrameUpload, count: u32) -> bool {
    match upload.batches.as_slice() {
        [
            UploadedBatch::PolySprites {
                color_only,
                count: actual,
                ..
            },
        ] => {
            assert_eq!(*actual, count);
            *color_only
        }
        _ => panic!("expected one image batch"),
    }
}

#[test]
fn image_pipeline_tracks_visible_area_and_reencoding() {
    let mut upload = FrameUpload::default();
    for edge in [127., 128., 129., 32., 240.] {
        encode_images(&mut upload, &[image(edge)], 256);
        assert_eq!(selected(&upload, 1), edge >= 128.);
    }
    let mut sprite = image(240.);
    sprite.grayscale = true;
    encode_images(&mut upload, &[sprite], 256);
    assert!(!selected(&upload, 1));
    sprite.grayscale = false;
    sprite.animation_id = Some(crate::SceneAnimationId(7));
    encode_images(&mut upload, &[sprite], 256);
    assert!(!selected(&upload, 1));
    sprite.animation_id = None;
    for width in [60., 140.] {
        sprite.content_mask.bounds.size.width = crate::ScaledPixels(width);
        encode_images(&mut upload, &[sprite], 256);
        assert!(!selected(&upload, 1));
    }
    sprite = image(240.);
    sprite.bounds.origin.x = crate::ScaledPixels(200.);
    encode_images(&mut upload, &[sprite], 256);
    assert!(!selected(&upload, 1));
    sprite = image(240.);
    sprite.bounds.origin = point(crate::ScaledPixels(300.), crate::ScaledPixels(300.));
    sprite.content_mask = crate::ContentMask::new(sprite.bounds);
    encode_images(&mut upload, &[sprite], 256);
    assert!(!selected(&upload, 1));
    encode_images(&mut upload, &[image(240.)], 128);
    assert!(!selected(&upload, 1));
    encode_images(&mut upload, &[image(240.)], 256);
    assert!(selected(&upload, 1));
}

#[test]
fn mixed_image_batches_keep_one_original_draw_in_both_orders() {
    let mut gray = image(240.);
    gray.grayscale = true;
    let mut animated = image(240.);
    animated.animation_id = Some(crate::SceneAnimationId(7));
    for fallback in [image(32.), gray, animated] {
        for sprites in [[image(192.), fallback], [fallback, image(192.)]] {
            let mut upload = FrameUpload::default();
            encode_images(&mut upload, &sprites, 256);
            assert!(!selected(&upload, 2));
        }
    }
    let mut upload = FrameUpload::default();
    encode_images(&mut upload, &[image(192.), image(240.)], 256);
    assert!(selected(&upload, 2));
}

#[test]
fn image_specialization_preserves_merging_across_culled_batches() {
    for flags in [[true, false], [false, true], [true, true]] {
        let mut upload = FrameUpload::default();
        for (i, color_only) in flags.into_iter().enumerate() {
            if i == 1 {
                upload
                    .batches
                    .push(UploadedBatch::Quads { first: 0, count: 1 });
                upload.batch_visual_bounds.push(Some(image(8.).bounds));
            }
            upload.batches.push(UploadedBatch::PolySprites {
                texture_id: test_atlas_tile().texture_id,
                first: i as u32,
                count: 1,
                color_only,
            });
            upload.batch_visual_bounds.push(None);
        }
        let pipelines = test_pipelines();
        let bind = test_bind_group_id(1);
        let mut steps = Vec::new();
        encode_clipped_draw_commands(
            &upload,
            &pipelines,
            pipelines.alpha,
            bind,
            bind,
            bind,
            |_| Some(bind),
            bind,
            |_| Some(bind),
            DrawMode::Present,
            Some(gfx_core::ScissorRect {
                x: 100,
                y: 100,
                width: 100,
                height: 100,
            }),
            &mut steps,
        );
        assert_eq!(steps.len(), 1);
        let RenderCommand::Draw(draw) = &steps[0] else {
            panic!("expected draw")
        };
        assert_eq!(draw.instance_count, 2);
        assert_eq!(draw.first_instance, 0);
        assert_eq!(
            draw.pipeline,
            if flags == [true, true] {
                pipelines.alpha.poly_sprites_color
            } else {
                pipelines.alpha.poly_sprites
            }
        );
    }
}
