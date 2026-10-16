use super::*;

fn shadow_paint() -> crate::scene::Painter {
    let mut painter = crate::scene::Painter::default();
    for _ in 0..2 {
        painter.push_layer(area(4.));
        for _ in 0..32 {
            painter.insert_primitive(crate::Shadow {
                bounds: area(4.),
                content_mask: crate::ContentMask::new(area(4.)),
                ..Default::default()
            });
        }
        painter.pop_layer();
        painter.insert_primitive(crate::Quad {
            bounds: area(4.),
            content_mask: crate::ContentMask::new(area(4.)),
            ..Default::default()
        });
    }
    painter.finish();
    painter
}

fn encode_paint(upload: &mut FrameUpload, painter: &crate::scene::Painter) -> FrameUploadSummary {
    upload.encode(
        &mut RetainedQuadChunkCache::default(),
        painter,
        &crate::window::retained::RetainedPaint::default(),
        &[],
        DrawableSize {
            width: 64,
            height: 64,
        },
        &RenderingParameters::system(),
        false,
        BackdropBlurQuality::Full,
    )
}

#[test]
fn persistent_shadow_payload_reuses_the_only_packed_buffer_across_paint_commits() {
    let previous = shadow_paint();
    let mut upload = FrameUpload::default();
    assert_eq!(encode_paint(&mut upload, &previous).shadow_count, 64);
    assert_eq!(upload.reused_shadow_bytes, 0);
    let bytes = upload.shadows.clone();
    let allocation = upload.shadows.as_ptr();
    let ranges = |upload: &FrameUpload| {
        upload
            .batches
            .iter()
            .filter_map(|batch| match batch {
                UploadedBatch::Shadows { first, count } => Some((*first, *count)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let batches = ranges(&upload);
    let mut current = crate::scene::Painter::default();
    assert!(current.replay(0..previous.len(), &previous, &[]));
    current.finish();
    assert_ne!(current.revision, previous.revision);
    assert_eq!(encode_paint(&mut upload, &current).shadow_count, 64);
    assert_eq!(upload.reused_shadow_bytes, 64 * PACKED_SHADOW_BYTES);
    assert_eq!(upload.shadows.as_ptr(), allocation);
    assert_eq!(upload.shadows, bytes);
    assert_eq!(ranges(&upload), batches);
    assert_eq!(batches, [(0, 32), (32, 32)]);
    let mut rebound = crate::scene::Painter::default();
    rebound.insert_primitive(crate::Quad {
        bounds: area(4.),
        content_mask: crate::ContentMask::new(area(4.)),
        ..Default::default()
    });
    assert!(!rebound.replay(0..current.len(), &current, &[]));
    rebound.finish();
    encode_paint(&mut upload, &rebound);
    assert_eq!(
        upload.reused_shadow_bytes, 0,
        "rebound/new sources must rebuild"
    );
}

#[test]
fn animated_and_nested_shadow_sources_never_reuse_unproven_packed_indices() {
    let mut animated = crate::scene::Painter::default();
    animated.insert_animated_primitive(
        crate::Shadow {
            bounds: area(4.),
            content_mask: crate::ContentMask::new(area(4.)),
            ..Default::default()
        },
        crate::SceneAnimationId(8),
    );
    animated.finish();
    let mut upload = FrameUpload::default();
    for _ in 0..2 {
        encode_paint(&mut upload, &animated);
        assert_eq!(upload.reused_shadow_bytes, 0);
    }
    let mut nested = crate::scene::Painter::default();
    nested.insert_primitive(crate::PaintBlur {
        order: 0,
        animation_id: None,
        layer_id: None,
        bounds: area(4.),
        content_mask: crate::ContentMask::new(area(4.)),
        radius: crate::ScaledPixels(3.),
        opacity: 1.,
        content: Arc::new(shadow_paint().into_scene()),
    });
    nested.finish();
    for _ in 0..2 {
        encode_paint(&mut upload, &nested);
        assert_eq!(upload.reused_shadow_bytes, 0);
        assert_eq!(upload.shadows.len(), 64 * PACKED_SHADOW_BYTES);
    }
}

#[test]
fn clipped_or_truncated_shadows_never_publish_a_packed_identity() {
    for count in [1, MAX_SHADOWS + 1] {
        let mut painter = crate::scene::Painter::default();
        let mut shadow = crate::Shadow {
            bounds: area(4.),
            content_mask: crate::ContentMask::new(area(4.)),
            ..Default::default()
        };
        if count == 1 {
            shadow.content_mask.bounds.size.width = crate::ScaledPixels(0.);
        }
        // Exercise the encoder's own clipping/limit boundary, independently of Painter culling.
        for _ in 0..count {
            painter.shadows.push(shadow);
        }
        painter.finish();
        let mut upload = FrameUpload::default();
        for _ in 0..2 {
            encode_paint(&mut upload, &painter);
            assert!(upload.shadow_source.is_none());
            assert_eq!(upload.reused_shadow_bytes, 0);
            assert_eq!(
                upload.shadows.len() / PACKED_SHADOW_BYTES,
                if count == 1 { 0 } else { MAX_SHADOWS }
            );
        }
    }
}

fn area(x: f32) -> crate::Bounds<crate::ScaledPixels> {
    crate::bounds(
        crate::point(crate::ScaledPixels(x), crate::ScaledPixels(4.)),
        crate::size(crate::ScaledPixels(8.), crate::ScaledPixels(8.)),
    )
}

fn glyph(x: f32, sampling: crate::MonochromeSpriteSampling) -> crate::MonochromeSprite {
    crate::MonochromeSprite {
        order: 0,
        pad: sampling as u32,
        animation_id: None,
        bounds: area(x),
        content_mask: crate::ContentMask::new(area(x)),
        color: crate::rgb(0xffffff),
        tile: crate::AtlasTile {
            texture_id: crate::AtlasTextureId {
                index: 0,
                kind: crate::AtlasTextureKind::Monochrome,
            },
            tile_id: crate::TileId(0),
            padding: 1,
            bounds: crate::bounds(
                crate::point(crate::DevicePixels(1), crate::DevicePixels(1)),
                crate::size(crate::DevicePixels(8), crate::DevicePixels(8)),
            ),
        },
        transformation: crate::TransformationMatrix::unit(),
    }
}

fn encode(sprites: &[crate::MonochromeSprite]) -> FrameUpload {
    let mut painter = crate::scene::Painter::default();
    for sprite in sprites {
        painter.push_layer(sprite.content_mask.bounds);
        painter.insert_primitive(*sprite);
        painter.pop_layer();
    }
    painter.finish();
    assert_eq!(painter.prepared_batches().len(), sprites.len());
    let mut upload = FrameUpload::default();
    upload.encode(
        &mut RetainedQuadChunkCache::default(),
        &painter,
        &crate::window::retained::RetainedPaint::default(),
        &[],
        DrawableSize {
            width: 64,
            height: 64,
        },
        &RenderingParameters::system(),
        false,
        BackdropBlurQuality::Full,
    );
    upload
}

#[test]
fn merged_glyph_batches_cover_every_sampling_run() {
    let first = glyph(4., crate::MonochromeSpriteSampling::Glyph);
    let second = glyph(40., crate::MonochromeSpriteSampling::Linear);
    let upload = encode(&[first, second]);
    assert_eq!(upload.batches.len(), 1);
    let bounds = upload.batch_visual_bounds[0].expect("static merged batch");
    assert_eq!(bounds.union(&first.bounds), bounds);
    assert_eq!(
        bounds.union(&second.bounds),
        bounds,
        "merged draw must cover the second sampling run"
    );
}

#[test]
fn merged_glyph_batches_keep_animation_and_transform_uncullable() {
    for transformed in [false, true] {
        let first = glyph(4., crate::MonochromeSpriteSampling::Glyph);
        let mut second = glyph(40., crate::MonochromeSpriteSampling::Linear);
        if transformed {
            second.transformation = crate::TransformationMatrix::unit().translate(crate::point(
                crate::ScaledPixels(12.),
                crate::ScaledPixels(0.),
            ));
        } else {
            second.animation_id = Some(crate::SceneAnimationId(7));
        }
        let upload = encode(&[first, second]);
        assert_eq!(upload.batches.len(), 1);
        assert_eq!(upload.batch_visual_bounds, vec![None]);
    }
}

#[test]
fn relocated_sprite_draws_keep_independent_bounds_and_merge_coverage() {
    let source = glyph(4., crate::MonochromeSpriteSampling::Glyph);
    let sprite = |x, id, animation_id| crate::PolychromeSprite {
        order: 0,
        sampling: 0,
        grayscale: false,
        opacity: 1.,
        animation_id,
        bounds: area(x),
        content_mask: crate::ContentMask::new(area(x)),
        corner_radii: Default::default(),
        tile: crate::AtlasTile {
            tile_id: crate::TileId(id),
            texture_id: crate::AtlasTextureId {
                kind: crate::AtlasTextureKind::Rgba,
                ..source.tile.texture_id
            },
            ..source.tile
        },
    };
    let left = sprite(4., 1, None);
    let moving = sprite(24., 2, Some(crate::SceneAnimationId(5)));
    let right = sprite(44., 3, None);
    let mut upload = FrameUpload::default();
    upload.atlas_placements.insert(
        moving.tile.tile_id,
        crate::AtlasTile {
            texture_id: crate::AtlasTextureId {
                index: 7,
                ..moving.tile.texture_id
            },
            ..moving.tile
        },
    );
    let mut summary = FrameUploadSummary::default();
    upload.encode_polychrome_sprites(&[left, moving, right], &mut summary);
    assert_eq!(upload.batches.len(), 3);
    assert_eq!(
        upload.batch_visual_bounds,
        vec![
            Some(left.bounds.dilate(crate::ScaledPixels(2.))),
            None,
            Some(right.bounds.dilate(crate::ScaledPixels(2.))),
        ]
    );
    // A second prepared run relocated onto the last draw's page must extend it.
    let tail = sprite(54., 4, None);
    upload.encode_polychrome_sprites(&[tail], &mut summary);
    assert_eq!(upload.batches.len(), 3);
    let coverage = upload.batch_visual_bounds[2].expect("merged static image draw");
    assert_eq!(coverage.union(&tail.bounds), coverage);
}

#[test]
fn underline_bounds_ignore_clipped_instances_but_preserve_animation() {
    let line = |x, animation_id| crate::Underline {
        order: 0,
        pad: 0,
        animation_id,
        bounds: area(x),
        content_mask: crate::ContentMask::new(area(x)),
        color: crate::rgb(0xffffff),
        thickness: crate::ScaledPixels(1.),
        wavy: 0,
    };
    let first = line(4., None);
    let mut clipped = line(40., Some(crate::SceneAnimationId(3)));
    clipped.content_mask.bounds.size.width = crate::ScaledPixels(0.);
    let mut upload = FrameUpload::default();
    let mut summary = FrameUploadSummary::default();
    upload.encode_underlines(&[first, clipped], &mut summary);
    assert_eq!(summary.underline_count, 1);
    assert_eq!(
        upload.batch_visual_bounds,
        vec![Some(first.bounds.dilate(crate::ScaledPixels(2.)))]
    );
    upload.encode_underlines(
        &[line(20., Some(crate::SceneAnimationId(3))), first],
        &mut summary,
    );
    assert_eq!(upload.batch_visual_bounds[1], None);
}

#[test]
fn nonfinite_source_or_clip_never_becomes_cullable_after_intersection() {
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        for component in 0..8 {
            let mut source = area(4.);
            let mut mask = area(4.);
            let coordinates = [
                &mut source.origin.x.0,
                &mut source.origin.y.0,
                &mut source.size.width.0,
                &mut source.size.height.0,
                &mut mask.origin.x.0,
                &mut mask.origin.y.0,
                &mut mask.size.width.0,
                &mut mask.size.height.0,
            ];
            *coordinates
                .into_iter()
                .nth(component)
                .expect("eight geometry components") = invalid;
            assert_eq!(static_raster_visual_bounds(source, mask, None, false), None);
        }
    }
}
