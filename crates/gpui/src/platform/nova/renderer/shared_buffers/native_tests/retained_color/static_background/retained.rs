//! The same static Paint must retain both Chunk bytes and offscreen pixels across UI commits.
use super::*;

fn page(
    previous: Option<&crate::window::Paint>,
    background: crate::Rgba,
    chunk_size: usize,
    revision: u64,
) -> Arc<crate::window::Paint> {
    let viewport = bounds(0., 0., SIDE as f32, SIDE as f32);
    let mut paint = crate::scene::Painter::default();
    let mut retention = crate::window::retained::RetainedPaint::default();
    if let Some(previous) = previous {
        let spans = previous
            .retained_paint
            .replay_quads(&previous.scene, 0..128);
        assert!(paint.replay(0..128, &previous.scene, spans));
        retention.replay(0..128, &previous.retained_paint, 0);
    } else {
        for index in 0..128 {
            paint.insert_primitive(quad(viewport, background));
            if (index + 1) % chunk_size == 0 {
                retention.record(
                    crate::GlobalElementId::from_path(&[("static", index / chunk_size).into()]),
                    1,
                    index + 1 - chunk_size..index + 1,
                );
            }
        }
    }
    paint.insert_animated_primitive(
        quad(
            bounds(20., 20., 16., 16.),
            crate::rgb(if revision == 101 { 0xff0000 } else { 0x0000ff }),
        ),
        crate::SceneAnimationId(1),
    );
    paint.finish();
    paint.revision = revision;
    retention.finish(&mut paint.scene);
    Arc::new(crate::window::Paint::new(
        paint.into_scene(),
        Arc::new(retention),
        Default::default(),
    ))
}

pub(super) fn verify(renderer: &mut NovaRenderer, target: (TextureId, TextureViewId)) {
    let first = page(None, crate::rgb(0x204060), 32, 101);
    pixels(renderer, &first, target, 0., [sample(0.)], true);
    assert!(renderer.frame_upload.root_color.is_none());
    let replayed = page(Some(&first), crate::rgb(0x204060), 32, 102);
    let original_quad = first.quads.iter().next().unwrap() as *const _;
    assert_eq!(
        original_quad,
        replayed.quads.iter().next().unwrap() as *const _,
        "CPU Paint payload stays shared"
    );
    let local = pixels(renderer, &replayed, target, 0., [sample(0.8)], false);
    let metrics = crate::performance_metrics_snapshot();
    assert_eq!(
        metrics.backdrop_blur_source_pixels, 0,
        "a new Paint revision must retain background color"
    );
    assert_eq!(metrics.retained_chunk_hits, 4);
    assert_eq!(metrics.retained_chunk_reused_bytes, 128 * PACKED_QUAD_BYTES);
    assert_eq!(renderer.frame_upload.quads.owned_len(), PACKED_QUAD_BYTES);
    assert!(renderer.frame_upload.root_color.is_none());
    assert!(renderer.frame_upload.has_element_blurs());

    let direct = page(None, crate::rgb(0x204060), 32, 0);
    assert_eq!(
        local,
        pixels(renderer, &direct, target, 0.8, [sample(0.8)], true),
        "a replayed source must match direct raster pixels"
    );
    pixels(renderer, &replayed, target, 0.8, [sample(0.8)], true);

    // Repartitioning identical pixels changes Chunk IDs and packed batches, not Filter input.
    let split = page(None, crate::rgb(0x204060), 64, 103);
    assert_eq!(
        local,
        pixels(renderer, &split, target, 0.8, [sample(0.8)], false)
    );
    assert_eq!(
        crate::performance_metrics_snapshot().backdrop_blur_source_pixels,
        0
    );
    let changed = page(None, crate::rgb(0x406080), 64, 104);
    let updated = pixels(renderer, &changed, target, 0.8, [sample(0.2)], false);
    assert!(crate::performance_metrics_snapshot().backdrop_blur_source_pixels > 0);
    let direct = page(None, crate::rgb(0x406080), 32, 0);
    assert_eq!(
        updated,
        pixels(renderer, &direct, target, 0.2, [sample(0.2)], true),
        "retained Source refresh matches direct raster pixels"
    );
    eprintln!(
        "retained AutoStatic gate backend={} chunk_hits=4 reused_bytes={} owned_gap={} new_revision_source_pixels=0 repartition_source_pixels=0 changed_source_refreshed=true pixels_equal=true",
        renderer.backend_info.label(),
        128 * PACKED_QUAD_BYTES,
        PACKED_QUAD_BYTES
    );
}
