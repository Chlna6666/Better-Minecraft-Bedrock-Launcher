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
