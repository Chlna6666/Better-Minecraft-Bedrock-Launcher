use super::*;
use crate::{Bounds, Primitive, Quad};

fn operations(count: usize) -> Operations {
    let mut operations = Operations::default();
    operations
        .extend((0..count).map(|_| PaintOperation::Primitive(Primitive::Quad(Quad::default()))));
    operations.finish();
    operations
}

#[test]
fn borrowed_runs_preserve_payload_identity_and_shared_comparison() {
    let source = operations(16);
    let mut target = Operations::default();
    target.share(&source, 3..10);
    target.push(PaintOperation::EndLayer);
    target.share(&source, 12..16);
    target.push(PaintOperation::StartLayer(Bounds::default()));
    let all = target.all();
    assert_eq!(all.views().count(), target.len());
    for start in 0..=target.len() {
        for end in start..=target.len() {
            let range = target.range(start..end);
            let mut index = start;
            for run in range.runs() {
                for view in run.iter() {
                    assert!(view.same_paint(target.view(index).unwrap()));
                    if let PaintOperationRef::Primitive(crate::scene::PrimitiveRef::Quad(quad)) = view {
                        let PaintOperationRef::Primitive(crate::scene::PrimitiveRef::Quad(expected)) = target.view(index).unwrap() else {
                            panic!("view kind differs from owned kind");
                        };
                        assert!(std::ptr::eq(quad, expected));
                    }
                    index += 1;
                }
            }
            assert_eq!(index, end);
            assert_eq!(range.views().rev().count(), end - start);
        }
    }
    assert_eq!(target.range(0..7).common_prefix_views(source.range(3..10), |_, _| {
        panic!("shared views must not visit individual primitives")
    }), 7);
    assert!(target.range(8..12).matches_views(source.range(12..16), |_, _| {
        panic!("shared views must preserve the storage identity proof")
    }));
}

#[test]
fn tightening_pending_capture_does_not_freeze_or_copy_primitives() {
    let mut operations = Operations::default();
    let bounds = crate::bounds(crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
        crate::size(crate::ScaledPixels(20.), crate::ScaledPixels(20.)));
    let tight = crate::bounds(bounds.origin, crate::size(crate::ScaledPixels(5.), crate::ScaledPixels(5.)));
    operations.push(PaintOperation::StartBlur(crate::BlurCapture {
        animation_id: None, layer_id: None, bounds, content_mask: crate::ContentMask::default(),
        radius: crate::ScaledPixels(0.), opacity: 1.,
    }));
    operations.push(PaintOperation::StartLayer(bounds));
    operations.push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
    operations.push(PaintOperation::EndLayer);
    let identity = operations.storage_identity(2);
    operations.tighten_capture(0, 4, tight);
    assert!(operations.segments.is_empty());
    assert_eq!(operations.storage_identity(2), identity);
    assert!(matches!(operations.view(0), Some(PaintOperationRef::StartBlur(capture)) if capture.bounds == tight));
    assert!(matches!(operations.view(1), Some(PaintOperationRef::StartLayer(bounds)) if *bounds == tight));
    operations.finish();
    let mut shared = Operations::default();
    shared.share(&operations, 0..4);
    let smaller = crate::bounds(bounds.origin, crate::size(crate::ScaledPixels(2.), crate::ScaledPixels(2.)));
    shared.tighten_capture(0, 4, smaller);
    assert_eq!(shared.storage_identity(2), identity);
    assert!(matches!(shared.view(0), Some(PaintOperationRef::StartBlur(capture)) if capture.bounds == smaller));
    assert!(matches!(operations.view(0), Some(PaintOperationRef::StartBlur(capture)) if capture.bounds == tight));
}

#[test]
fn shared_ranges_survive_source_clear_and_preserve_new_insertions() {
    let mut source = operations(12);
    let first = source.storage_identity(2);
    let last = source.storage_identity(9);
    let mut current = Operations::default();
    current.share(&source, 2..6);
    current.push(PaintOperation::EndLayer);
    current.share(&source, 8..10);
    current.push(PaintOperation::EndBlur);
    current.finish();
    assert_eq!(current.len(), 8);
    assert!(matches!(current.view(4).unwrap(), PaintOperationRef::EndLayer));
    assert!(matches!(current.view(7).unwrap(), PaintOperationRef::EndBlur));
    source.clear();
    assert_eq!(first, current.storage_identity(0));
    assert_eq!(last, current.storage_identity(6));
    for start in 0..=current.len() {
        for end in start..=current.len() {
            let range = current.range(start..end);
            assert_eq!(range.views().count(), end - start);
            let forward: Vec<_> = range.views().map(|op| op.visual_bounds()).collect();
            let reverse: Vec<_> = range.views().rev().map(|op| op.visual_bounds()).collect();
            assert!(forward.iter().rev().eq(&reverse));
            for (index, operation) in range.views().enumerate() {
                assert!(operation.same_paint(current.view(start + index).unwrap()));
            }
        }
    }
}

#[test]
fn editing_a_shared_range_detaches_only_changed_operations() {
    let source = operations(12);
    let mut current = Operations::default();
    current.share(&source, 0..12);
    current.push(PaintOperation::EndBlur);
    current.edit(4..7, |op| {
        *op = PaintOperation::StartLayer(Bounds::default())
    });
    current.edit(12..13, |op| *op = PaintOperation::EndLayer);
    for index in 0..12 {
        if (4..7).contains(&index) {
            assert!(matches!(current.view(index).unwrap(), PaintOperationRef::StartLayer(_)));
            assert!(matches!(source.view(index).unwrap(), PaintOperationRef::Primitive(_)));
        } else {
            assert_eq!(current.storage_identity(index), source.storage_identity(index));
        }
    }
    assert!(matches!(current.view(12).unwrap(), PaintOperationRef::EndLayer));
    current.finish();
    assert_eq!(current.views().count(), 13);
    current.edit(1..12, |op| *op = PaintOperation::EndBlur);
    assert!(
        current
            .range(1..12)
            .views()
            .all(|op| matches!(op, PaintOperationRef::EndBlur))
    );
    assert!(
        source
            .views()
            .all(|op| matches!(op, PaintOperationRef::Primitive(_)))
    );
}

#[test]
fn compaction_counts_all_views_and_releases_sparse_old_pages() {
    let source = operations(100);
    let mut current = Operations::default();
    for index in 0..100 {
        current.share(&source, index..index + 1);
    }
    current.compact();
    assert_eq!(current.segments.len(), 1);
    assert_eq!(source.storage_identity(20), current.storage_identity(20));
    let mut sparse = Operations::default();
    sparse.share(&source, 20..23);
    sparse.compact();
    assert_eq!(sparse.segments[0].owner.len(), 3);
    assert_ne!(source.storage_identity(20), sparse.storage_identity(0));
}

#[test]
fn repeated_sparse_views_compact_the_unique_payload_only_once() {
    let source = operations(1024);
    let mut target = Operations::default();
    for _ in 0..128 {
        target.share(&source, 20..24);
        target.share(&source, 22..26);
        target.share(&source, 800..802);
    }
    target.compact();
    assert_eq!(target.len(), 128 * 10);
    assert_eq!(target.segments[0].owner.len(), 8);
    assert!(target.segments.iter().all(|segment| Arc::ptr_eq(&segment.owner, &target.segments[0].owner)));
    assert_eq!(target.storage_identity(2), target.storage_identity(4));
    assert_eq!(target.storage_identity(0), target.storage_identity(10));
    drop(source);
    assert!(target.views().all(|op| matches!(op, PaintOperationRef::Primitive(_))));
}

#[test]
fn edits_and_ranges_follow_logical_order_across_shared_and_pending_runs() {
    let source = operations(10);
    let mut current = Operations::default();
    current.share(&source, 0..6);
    current.extend((0..4).map(|_| PaintOperation::EndLayer));
    let mut visited = 0;
    current.edit(2..9, |operation| {
        *operation = PaintOperation::StartLayer(crate::bounds(
            crate::point(crate::ScaledPixels(visited as f32), crate::ScaledPixels(0.)),
            crate::size(crate::ScaledPixels(1.), crate::ScaledPixels(1.)),
        ));
        visited += 1;
    });
    for (index, operation) in current.range(2..9).views().enumerate() {
        let PaintOperationRef::StartLayer(bounds) = operation else {
            panic!("edited layer")
        };
        assert_eq!(bounds.origin.x.0, index as f32);
    }
    for start in 0..=10 {
        for end in start..=10 {
            let view = current.range(start..end);
            assert_eq!(view.views().count(), end - start);
            assert!(
                view.views()
                    .zip(view.views().rev().rev())
                    .all(|(a, b)| a.same_paint(b))
            );
        }
    }
    assert!(current.get_range(9..11).is_none());
    assert!(current.get_range(2..1).is_none());
}

fn scan_range(operations: &Operations, range: Range<usize>) -> bool {
    if range.start > range.end || range.end > operations.len() {
        return false;
    }
    if range.is_empty() {
        return true;
    }
    let mut blur_depth = 0usize;
    let mut layer_depth = 0usize;
    for (index, operation) in operations
        .range(0..range.end)
        .views()
        .enumerate()
    {
        if index == range.start && (blur_depth != 0 || layer_depth != 0) {
            return false;
        }
        match operation {
            PaintOperationRef::StartBlur(_) => blur_depth = blur_depth.saturating_add(1),
            PaintOperationRef::StartLayer(_) => layer_depth = layer_depth.saturating_add(1),
            PaintOperationRef::EndBlur => {
                let Some(depth) = blur_depth.checked_sub(1) else {
                    return false;
                };
                blur_depth = depth;
            }
            PaintOperationRef::EndLayer => {
                let Some(depth) = layer_depth.checked_sub(1) else {
                    return false;
                };
                layer_depth = depth;
            }
            PaintOperationRef::Primitive(_) | PaintOperationRef::DeferredScroll(_) => {}
        }
    }
    blur_depth == 0 && layer_depth == 0
}

fn check_ranges(operations: &Operations) {
    for start in 0..=operations.len() {
        for end in start..=operations.len() {
            assert_eq!(operations.range_is_replayable(start..end), scan_range(operations, start..end),
                "range {start}..{end}");
        }
    }
    assert!(!operations.range_is_replayable(operations.len()+1..operations.len()+1));
}

#[test]
fn scope_boundaries_follow_append_share_compact_edit_and_clear() {
    let mut source = operations(12);
    source.push(PaintOperation::StartLayer(Bounds::default()));
    source.push(PaintOperation::StartBlur(crate::BlurCapture {
        animation_id: None, layer_id: None, bounds: Bounds::default(),
        content_mask: crate::ContentMask::default(), radius: crate::ScaledPixels(2.),
        opacity: 1.,
    }));
    source.push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
    source.push(PaintOperation::EndBlur);
    source.push(PaintOperation::EndLayer);
    source.extend((0..16).map(|_| PaintOperation::Primitive(Primitive::Quad(Quad::default()))));
    check_ranges(&source);
    source.finish();
    let mut target = Operations::default();
    target.push(PaintOperation::StartLayer(Bounds::default()));
    target.share(&source, 12..17);
    target.push(PaintOperation::EndLayer);
    target.share(&source, 17..20);
    target.finish();
    check_ranges(&target);
    target.compact();
    check_ranges(&target);
    target.edit(3..4, |op| *op = PaintOperation::EndLayer);
    check_ranges(&target);
    target.edit(3..4, |op| *op = PaintOperation::Primitive(Primitive::Quad(Quad::default())));
    check_ranges(&target);
    target.clear();
    target.share(&source, 12..17);
    check_ranges(&target);
    target.clear();
    target.share(&source, 15..17);
    check_ranges(&target);
}

#[test]
fn compact_scope_metadata_tracks_layer_presence_and_marker_identity() {
    let mut source = operations(8);
    source.push(PaintOperation::StartLayer(Bounds::default()));
    source.push(PaintOperation::EndLayer);
    source.push(PaintOperation::StartLayer(Bounds::default()));
    source.push(PaintOperation::EndLayer);
    source.finish();
    assert!(source.has_layer_scopes());
    assert!(!source.has_deferred_scroll());
    assert_eq!(source.deferred_scroll_indices().count(), 0);
    let mut target = Operations::default();
    target.share(&source, 0..8);
    assert!(!target.has_layer_scopes());
    target.share(&source, 8..12);
    assert!(target.has_layer_scopes());
    assert_eq!(target.boundaries.len(), 4);
    target.edit(8..12, |op| *op = PaintOperation::Primitive(Primitive::Quad(Quad::default())));
    assert!(!target.has_layer_scopes());
    assert!(target.boundaries.is_empty());
    target.clear();
    target.push(PaintOperation::EndLayer);
    assert!(!target.has_layer_scopes());
    assert!(!target.range_is_replayable(0..1));
    #[cfg(target_pointer_width = "64")]
    assert_eq!(std::mem::size_of::<ReplayBoundary>(), 24);
}

#[test]
fn range_comparison_skips_shared_runs_even_with_pending_tails() {
    let source = operations(128);
    let mut current = Operations::default();
    current.share(&source, 0..64);
    current.push(PaintOperation::EndLayer);
    current.share(&source, 65..120);
    current.extend((0..8).map(|_| PaintOperation::Primitive(Primitive::Quad(Quad::default()))));

    for start in 0..=128 {
        for end in start..=128 {
            let mut compared = 0;
            assert!(current.range(start..end).matches_views(source.range(start..end), |_, _| {
                compared += 1;
                true
            }));
            let expected = (start..end).filter(|i| *i == 64 || *i >= 120).count();
            assert_eq!(compared, expected, "{start}..{end}");
        }
    }
    assert!(!current.range(63..66).matches_views(source.range(63..66), |_, _| false));
    assert!(!current.range(0..1).matches_views(source.range(0..2), |_, _| true));
    let mut compared = 0;
    assert!(current.range(1..32).matches_views(source.range(0..31), |_, _| {
        compared += 1;
        true
    }));
    assert_eq!(compared, 31);
}

#[test]
fn range_slices_match_logical_indices_across_shared_and_pending_runs() {
    let source = operations(32);
    let mut current = Operations::default();
    current.share(&source, 3..10);
    current.push(PaintOperation::EndLayer);
    current.share(&source, 15..27);
    current.push(PaintOperation::EndBlur);
    for start in 0..=current.len() {
        for end in start..=current.len() {
            let range = current.range(start..end);
            let mut index = start;
            for run in range.runs() {
                assert!(!run.is_empty());
                for op in run.iter() {
                    assert!(op.same_paint(current.view(index).unwrap()));
                    index += 1;
                }
            }
            assert_eq!(index, end);
            // A replay cursor may jump over a shared span crossing runs.
            for skip in start..=end {
                let mut runs = range.runs();
                let mut run = OperationRun::default();
                let mut run_end = start;
                for index in skip..end {
                    while index >= run_end {
                        run = runs.next().unwrap();
                        run_end += run.len();
                    }
                    assert!(run.get(index - (run_end - run.len())).unwrap().same_paint(current.view(index).unwrap()));
                }
            }
        }
    }
}

#[test]
fn damage_edges_skip_shared_pages_and_stop_at_first_real_mismatch() {
    let source = operations(1024);
    let mut current = Operations::default();
    current.share(&source, 0..400);
    current.push(PaintOperation::EndLayer);
    current.share(&source, 400..1024);
    current.finish();
    let mut comparisons = 0;
    let mut compare = |a: PaintOperationRef<'_>, b: PaintOperationRef<'_>| {
        comparisons += 1;
        a.visually_eq(b)
    };
    assert_eq!(current.all().common_prefix_views(source.all(), &mut compare), 400);
    assert_eq!(current.all().common_suffix_views(source.all(), &mut compare), 624);
    assert_eq!(comparisons, 2);
    let mut calls = 0;
    assert_eq!(source.range(5..1000).common_prefix_views(current.range(5..400), |_, _| {
        calls += 1; false
    }), 395);
    assert_eq!(calls, 0);
    assert_eq!(source.range(400..1024).common_suffix_views(current.range(401..1025), |_, _| {
        panic!("shared suffix must not compare primitives")
    }), 624);
}
