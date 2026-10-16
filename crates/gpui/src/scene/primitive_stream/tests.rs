use super::*;
use crate::window::retained::PaintFixture;
use crate::{ContentMask, GlobalElementId, PrimitiveBatch, ScaledPixels, bounds, point, size};

fn quad(order: u32) -> Quad {
    let area = bounds(
        point(ScaledPixels(0.), ScaledPixels(0.)),
        size(ScaledPixels(20.), ScaledPixels(20.)),
    );
    Quad {
        order,
        bounds: area,
        content_mask: ContentMask::new(area),
        ..Default::default()
    }
}

#[test]
fn immutable_runs_merge_with_new_orders_without_copying_shared_payload() {
    let mut previous = PrimitiveStream::default();
    for order in [1, 3, 5, 7] {
        previous.push(quad(order));
    }
    previous.finish();
    let original = &previous[0] as *const Quad;
    let mut current = PrimitiveStream::default();
    current.share(&previous, 0..4);
    for order in [8, 2, 4, 6] {
        current.push(quad(order));
    }
    current.finish();
    assert_eq!(
        current.iter().map(|quad| quad.order).collect::<Vec<_>>(),
        (1..=8).collect::<Vec<_>>()
    );
    assert_eq!(&current[0] as *const Quad, original);
    assert_eq!(&current[2] as *const Quad, &previous[1] as *const Quad);
    assert_eq!(current.partition_point(|quad| quad.order < 5), 4);
    let mut offset = 0;
    for slice in current.slices(0..current.len()) {
        assert_eq!(slice.as_ptr(), &current[offset] as *const Quad);
        offset += slice.len();
    }
    previous.clear();
    current.compact();
    assert_eq!(current[2].order, 3);
    assert_eq!(&current[2] as *const Quad, original.wrapping_add(1));
    assert_eq!(
        &current[0] as *const Quad, original,
        "source retirement must not copy live Paint"
    );
}

#[test]
fn every_segmented_range_matches_a_flat_sorted_reference() {
    for count in 0..=12 {
        let mut previous = PrimitiveStream::default();
        for index in 0..count {
            previous.push(quad(index * 2));
        }
        previous.finish();
        let mut current = PrimitiveStream::default();
        current.share(&previous, 0..previous.len());
        let mut expected: Vec<_> = (0..count).map(|index| index * 2).collect();
        for order in [9, 1, 5, 3, 3] {
            current.push(quad(order));
            expected.push(order);
        }
        expected.sort_unstable();
        current.finish();
        current.compact();
        for first in 0..=expected.len() {
            for end in first..=expected.len() {
                let actual: Vec<_> = current
                    .slices(first..end)
                    .flatten()
                    .map(|quad| quad.order)
                    .collect();
                assert_eq!(actual, expected[first..end]);
            }
        }
    }
}

#[test]
fn new_quad_between_shared_ranges_keeps_equal_layer_order() {
    let mut previous = PrimitiveStream::default();
    let marker = |position| Quad {
        bounds: bounds(
            point(ScaledPixels(position), ScaledPixels(0.)),
            size(ScaledPixels(20.), ScaledPixels(20.)),
        ),
        ..quad(1)
    };
    previous.push(marker(1.));
    previous.push(marker(3.));
    previous.finish();
    let mut current = PrimitiveStream::default();
    current.share(&previous, 0..1);
    current.push(marker(2.));
    current.share(&previous, 1..2);
    current.finish();
    current.compact();
    assert_eq!(
        current
            .iter()
            .map(|quad| quad.bounds.origin.x.0)
            .collect::<Vec<_>>(),
        [1., 2., 3.]
    );
    assert_eq!(&current[0] as *const Quad, &previous[0] as *const Quad);
    assert_eq!(&current[2] as *const Quad, &previous[1] as *const Quad);
}

#[test]
fn new_paint_stays_owned_and_sparse_ranges_release_unrelated_page() {
    let mut previous = PrimitiveStream::default();
    for order in 1..=1024 {
        previous.push(quad(order));
    }
    previous.finish();
    let original = &previous[0] as *const Quad;
    previous.compact();
    assert_eq!(previous.segments.len(), 1);
    assert_eq!(previous.segments[0].owner.len(), 1024);
    assert_eq!(
        &previous[0] as *const Quad, original,
        "new payload must not be copied to manufacture Chunk boundaries"
    );
    let mut current = PrimitiveStream::default();
    current.share(&previous, 0..32);
    current.finish();
    current.compact();
    assert_eq!(
        current.segments[0].owner.len(),
        32,
        "small retained fragments must not pin an old page"
    );
    previous.clear();
    assert_eq!(
        current.iter().map(|quad| quad.order).collect::<Vec<_>>(),
        (1..=32).collect::<Vec<_>>()
    );
}

fn retained() -> PaintFixture {
    let mut paint = PaintFixture::default();
    for _ in 0..32 {
        paint.insert_primitive(quad(0));
    }
    paint.record(
        GlobalElementId::from_path(&["persistent-quads".into()]),
        1,
        0..paint.len(),
    );
    paint.finish();
    paint
}

#[test]
fn window_replay_preserves_cpu_payload_and_gpu_chunk_identity() {
    let mut previous = retained();
    let original = &previous.quads[0] as *const Quad;
    let operation = &previous.paint_operations[0] as *const _;
    let mut current = PaintFixture::default();
    current.replay(0..previous.len(), &previous);
    current.finish();
    assert_eq!(&current.quads[0] as *const Quad, original);
    assert_eq!(&current.paint_operations[0] as *const _, operation);
    assert_eq!(current.chunks()[0].id, previous.chunks()[0].id);
    assert!(current.chunks()[0].replayed);
    let mut reference = crate::scene::Painter::default();
    reference.replay(0..previous.len(), &previous, &[]);
    reference.finish();
    assert_ne!(
        &reference.quads[0] as *const Quad, original,
        "basic replay copies; the qualified path must share instead"
    );
    assert!(
        reference
            .quads
            .iter()
            .zip(current.quads.iter())
            .all(|(left, right)| left == right)
    );
    previous.clear();
    assert_eq!(&current.quads[0] as *const Quad, original);
    assert_eq!(&current.paint_operations[0] as *const _, operation);
    assert_eq!(current.quads.len(), 32);
}

#[test]
fn changed_ordering_prefix_rebinds_payload_instead_of_sharing_stale_orders() {
    let previous = retained();
    let mut current = PaintFixture::default();
    current.insert_primitive(quad(0));
    current.replay(0..previous.len(), &previous);
    current.finish();
    assert!(current.chunks().is_empty());
    assert_ne!(
        &current.quads[1] as *const Quad,
        &previous.quads[0] as *const Quad
    );
    let mut reference = crate::scene::Painter::default();
    reference.insert_primitive(quad(0));
    reference.replay(0..previous.len(), &previous, &[]);
    reference.finish();
    assert!(
        reference
            .quads
            .iter()
            .zip(current.quads.iter())
            .all(|(left, right)| left == right)
    );
}

#[test]
fn new_shadow_barrier_keeps_shared_and_new_quad_batches_in_painter_order() {
    let previous = retained();
    let mut current = PaintFixture::default();
    current.replay(0..previous.len(), &previous);
    current.insert_primitive(crate::Shadow {
        bounds: quad(0).bounds,
        content_mask: quad(0).content_mask,
        ..Default::default()
    });
    current.insert_primitive(quad(0));
    current.finish();
    let kinds: Vec<_> = current
        .batches()
        .map(|batch| match batch {
            PrimitiveBatch::Quads { quads, .. } => (0, quads.len()),
            PrimitiveBatch::Shadows { shadows, .. } => (1, shadows.len()),
            _ => panic!("unexpected fixture primitive"),
        })
        .collect();
    assert_eq!(kinds, [(0, 32), (1, 1), (0, 1)]);
}

fn shadow(x: f32) -> crate::Shadow {
    let quad = quad(0);
    crate::Shadow {
        bounds: bounds(point(ScaledPixels(x), ScaledPixels(0.)), quad.bounds.size),
        content_mask: ContentMask::new(bounds(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(640.), ScaledPixels(480.)),
        )),
        ..Default::default()
    }
}

#[test]
fn shadow_replay_shares_operations_and_typed_payload_after_source_release() {
    let mut previous = crate::scene::Painter::default();
    for x in 0..32 {
        previous.insert_primitive(shadow(x as f32));
    }
    previous.finish();
    let payload = &previous.shadows[0] as *const crate::Shadow;
    let operation = &previous.paint_operations[0] as *const _;
    let mut current = crate::scene::Painter::default();
    assert!(current.replay(0..previous.len(), &previous, &[]));
    current.finish();
    assert_eq!(&current.shadows[0] as *const _, payload);
    assert_eq!(&current.paint_operations[0] as *const _, operation);
    previous.clear();
    assert_eq!(current.shadows.len(), 32);
    assert_eq!(current.shadows[31].bounds.origin.x, ScaledPixels(31.));
    assert_eq!(
        current.prepared_batches(),
        &[crate::PreparedSceneBatch::Shadows(0..32)]
    );
}

#[test]
fn shadow_replay_rebinds_changed_prefix_and_partial_equal_order_ranges() {
    let mut previous = crate::scene::Painter::default();
    let layer = shadow(0.).content_mask.bounds;
    previous.push_layer(layer);
    for x in 0..32 {
        previous.insert_primitive(shadow(x as f32));
    }
    previous.pop_layer();
    previous.finish();
    let mut current = crate::scene::Painter::default();
    current.push_layer(layer);
    assert!(current.replay(1..17, &previous, &[]));
    current.pop_layer();
    current.finish();
    assert_eq!(current.shadows.len(), 16);
    assert_ne!(
        &current.shadows[0] as *const _,
        &previous.shadows[0] as *const _
    );
    let mut changed = crate::scene::Painter::default();
    changed.insert_primitive(quad(0));
    assert!(!changed.replay(0..previous.len(), &previous, &[]));
    changed.finish();
    assert_ne!(
        &changed.shadows[0] as *const _,
        &previous.shadows[0] as *const _
    );
}

#[test]
fn shadow_segments_preserve_equal_order_insertion_and_share_no_animation() {
    let mut previous = PrimitiveStream::default();
    previous.push(shadow(1.));
    previous.push(shadow(3.));
    previous.finish();
    let mut current = PrimitiveStream::default();
    current.share(&previous, 0..1);
    current.push(shadow(2.));
    current.share(&previous, 1..2);
    current.finish();
    current.compact();
    assert_eq!(
        current
            .iter()
            .map(|shadow| shadow.bounds.origin.x.0)
            .collect::<Vec<_>>(),
        [1., 2., 3.]
    );
    let mut animated = crate::scene::Painter::default();
    animated.insert_animated_primitive(shadow(0.), crate::SceneAnimationId(7));
    animated.finish();
    let mut replayed = crate::scene::Painter::default();
    assert!(replayed.replay(0..animated.len(), &animated, &[]));
    replayed.finish();
    assert_ne!(
        &replayed.shadows[0] as *const _,
        &animated.shadows[0] as *const _
    );
    assert_eq!(
        replayed.shadows[0].animation_id,
        Some(crate::SceneAnimationId(7))
    );
}

#[test]
fn weak_payload_identity_normalizes_ranges_without_pinning_retired_paint() {
    let mut previous = PrimitiveStream::default();
    for x in 0..32 {
        previous.push(shadow(x as f32));
    }
    previous.finish();
    let identity = previous.identity().expect("finished stream");
    let mut current = PrimitiveStream::default();
    current.share(&previous, 0..16);
    current.share(&previous, 16..32);
    current.finish();
    assert!(
        identity.matches(&current),
        "adjacent logical views share the same payload"
    );
    previous.clear();
    assert!(identity.matches(&current));
    current.clear();
    assert_eq!(
        identity.segments[0].0.strong_count(),
        0,
        "identity must not keep Paint alive"
    );
    for x in 0..32 {
        current.push(shadow(x as f32));
    }
    assert!(
        !identity.matches(&current),
        "pending/mutable runs have no immutable identity"
    );
    current.finish();
    assert!(
        !identity.matches(&current),
        "replacement allocation must never match a retired control block"
    );
}
