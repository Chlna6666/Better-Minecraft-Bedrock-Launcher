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
fn shared_ranges_survive_source_clear_and_preserve_new_insertions() {
    let mut source = operations(12);
    let first = &source[2] as *const _;
    let last = &source[9] as *const _;
    let mut current = Operations::default();
    current.share(&source, 2..6);
    current.push(PaintOperation::EndLayer);
    current.share(&source, 8..10);
    current.push(PaintOperation::EndBlur);
    current.finish();
    assert_eq!(current.len(), 8);
    assert!(matches!(current[4], PaintOperation::EndLayer));
    assert!(matches!(current[7], PaintOperation::EndBlur));
    source.clear();
    assert_eq!(first, &current[0] as *const _);
    assert_eq!(last, &current[6] as *const _);
    for start in 0..=current.len() {
        for end in start..=current.len() {
            let range = current.range(start..end);
            assert_eq!(range.iter().count(), end - start);
            let forward: Vec<_> = range.iter().map(|op| op as *const _).collect();
            let reverse: Vec<_> = range.iter().rev().map(|op| op as *const _).collect();
            assert!(forward.iter().rev().eq(&reverse));
            for (index, operation) in range.iter().enumerate() {
                assert!(std::ptr::eq(operation, &current[start + index]));
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
            assert!(matches!(current[index], PaintOperation::StartLayer(_)));
            assert!(matches!(source[index], PaintOperation::Primitive(_)));
        } else {
            assert!(std::ptr::eq(&current[index], &source[index]));
        }
    }
    assert!(matches!(current[12], PaintOperation::EndLayer));
    current.finish();
    assert_eq!(current.iter().count(), 13);
    current.edit(1..12, |op| *op = PaintOperation::EndBlur);
    assert!(
        current
            .range(1..12)
            .iter()
            .all(|op| matches!(op, PaintOperation::EndBlur))
    );
    assert!(
        source
            .iter()
            .all(|op| matches!(op, PaintOperation::Primitive(_)))
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
    assert!(std::ptr::eq(&source[20], &current[20]));
    let mut sparse = Operations::default();
    sparse.share(&source, 20..23);
    sparse.compact();
    assert_eq!(sparse.segments[0].owner.len(), 3);
    assert!(!std::ptr::eq(&source[20], &sparse[0]));
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
    for (index, operation) in current.range(2..9).iter().enumerate() {
        let PaintOperation::StartLayer(bounds) = operation else {
            panic!("edited layer")
        };
        assert_eq!(bounds.origin.x.0, index as f32);
    }
    for start in 0..=10 {
        for end in start..=10 {
            let view = current.range(start..end);
            assert_eq!(view.iter().count(), end - start);
            assert!(
                view.iter()
                    .zip(view.iter().rev().rev())
                    .all(|(a, b)| std::ptr::eq(a, b))
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
        .iter()
        .enumerate()
    {
        if index == range.start && (blur_depth != 0 || layer_depth != 0) {
            return false;
        }
        match operation {
            PaintOperation::StartBlur(_) => blur_depth = blur_depth.saturating_add(1),
            PaintOperation::StartLayer(_) => layer_depth = layer_depth.saturating_add(1),
            PaintOperation::EndBlur => {
                let Some(depth) = blur_depth.checked_sub(1) else {
                    return false;
                };
                blur_depth = depth;
            }
            PaintOperation::EndLayer => {
                let Some(depth) = layer_depth.checked_sub(1) else {
                    return false;
                };
                layer_depth = depth;
            }
            PaintOperation::Primitive(_) | PaintOperation::DeferredScroll(_) => {}
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
            assert!(current.range(start..end).matches_by(source.range(start..end), |_, _| {
                compared += 1;
                true
            }));
            let expected = (start..end).filter(|i| *i == 64 || *i >= 120).count();
            assert_eq!(compared, expected, "{start}..{end}");
        }
    }
    assert!(!current.range(63..66).matches_by(source.range(63..66), |_, _| false));
    assert!(!current.range(0..1).matches_by(source.range(0..2), |_, _| true));
    let mut compared = 0;
    assert!(current.range(1..32).matches_by(source.range(0..31), |_, _| {
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
            for slice in range.slices() {
                assert!(!slice.is_empty());
                for op in slice {
                    assert!(std::ptr::eq(op, &current[index]));
                    index += 1;
                }
            }
            assert_eq!(index, end);
            // A replay cursor may jump over a shared span crossing runs.
            for skip in start..=end {
                let mut runs = range.slices();
                let mut run: &[PaintOperation] = &[];
                let mut run_end = start;
                for index in skip..end {
                    while index >= run_end {
                        run = runs.next().unwrap();
                        run_end += run.len();
                    }
                    assert!(std::ptr::eq(&run[index - (run_end - run.len())], &current[index]));
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
    let mut compare = |a: &PaintOperation, b: &PaintOperation| {
        comparisons += 1;
        a.visually_eq(b)
    };
    assert_eq!(current.all().common_prefix_by(source.all(), &mut compare), 400);
    assert_eq!(current.all().common_suffix_by(source.all(), &mut compare), 624);
    assert_eq!(comparisons, 2);
    let mut calls = 0;
    assert_eq!(source.range(5..1000).common_prefix_by(current.range(5..400), |_, _| {
        calls += 1; false
    }), 395);
    assert_eq!(calls, 0);
    assert_eq!(source.range(400..1024).common_suffix_by(current.range(401..1025), |_, _| {
        panic!("shared suffix must not compare primitives")
    }), 624);
}
