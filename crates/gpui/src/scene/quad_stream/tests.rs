use super::*;
use crate::window::retained::PaintFixture;
use crate::{
    ContentMask, GlobalElementId, PrimitiveBatch, ScaledPixels, Scene, bounds, point, size,
};

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
    let mut previous = QuadStream::default();
    for order in [1, 3, 5, 7] {
        previous.push(quad(order));
    }
    previous.finish();
    let original = &previous[0] as *const Quad;
    let mut current = QuadStream::default();
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
        let mut previous = QuadStream::default();
        for index in 0..count {
            previous.push(quad(index * 2));
        }
        previous.finish();
        let mut current = QuadStream::default();
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
    let mut previous = QuadStream::default();
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
    let mut current = QuadStream::default();
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
    let mut previous = QuadStream::default();
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
    let mut current = QuadStream::default();
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
    let mut current = PaintFixture::default();
    current.replay(0..previous.len(), &previous);
    current.finish();
    assert_eq!(&current.quads[0] as *const Quad, original);
    assert_eq!(current.chunks()[0].id, previous.chunks()[0].id);
    assert!(current.chunks()[0].replayed);
    let mut reference = Scene::default();
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
    let mut reference = Scene::default();
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
            PrimitiveBatch::Shadows(shadows) => (1, shadows.len()),
            _ => panic!("unexpected fixture primitive"),
        })
        .collect();
    assert_eq!(kinds, [(0, 32), (1, 1), (0, 1)]);
}
