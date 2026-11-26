use super::*;
use crate::input::{ActionRegistry, Keymap};

fn frame() -> Frame {
    Frame::new(DispatchTree::new(
        Rc::new(RefCell::new(Keymap::default())),
        Rc::new(ActionRegistry::default()),
    ))
}

fn segment(entity: u64, x: f32) -> RetainedSceneSegment {
    RetainedSceneSegment::new(
        EntityId::from(entity),
        crate::bounds(
            crate::point(crate::px(x), crate::px(0.0)),
            crate::size(crate::px(10.0), crate::px(10.0)),
        ).scale(1.0),
        0..1,
    )
}

#[test]
fn repeated_entity_keeps_noncontiguous_segment_indices_and_union_bounds() {
    let mut frame = frame();
    frame.push_retained_scene_segment(segment(1, 0.0));
    frame.push_retained_scene_segment(segment(2, 10.0));
    frame.push_retained_scene_segment(segment(1, 20.0));
    let entry = &frame.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!(entry.first, 0);
    assert_eq!(entry.last, 2);
    assert_eq!(frame.retained_scene_segments[0].next_for_entity.unwrap().get() - 1, 2);
    assert!(frame.retained_scene_segments[2].next_for_entity.is_none());
    assert_eq!(entry.len(), 2);
    assert_eq!(
        entry.bounds(&frame.retained_scene_segments),
        segment(1, 0.0).bounds.union(&segment(1, 20.0).bounds)
    );
    assert!(
        entry
            .segments(&frame.retained_scene_segments)
            .all(|segment| segment.entity_id == EntityId::from(1_u64))
    );
}

#[test]
fn swapping_and_clearing_scratch_preserves_committed_index() {
    let mut rendered = frame();
    let mut next = frame();
    next.push_retained_scene_segment(segment(1, 0.0));
    std::mem::swap(&mut rendered, &mut next);
    next.push_retained_scene_segment(segment(2, 10.0));
    next.clear_for_reuse(&rendered);
    assert!(next.retained_scene_segments.is_empty());
    assert!(next.retained_scene_index.is_empty());
    assert_eq!(rendered.retained_scene_segments.len(), 1);
    assert_eq!(rendered.retained_scene_index.len(), 1);
    rendered.clear();
    rendered.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Aggressive);
    assert_eq!(rendered.retained_scene_index.capacity(), 0);
}

#[test]
fn interleaved_fragments_keep_order_without_per_owner_allocations() {
    let mut frame = frame();
    for i in 0..65 {
        frame.push_retained_scene_segment(segment(1, i as f32));
        frame.push_retained_scene_segment(segment(2, i as f32));
    }
    frame.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Moderate);
    let entry = &frame.retained_scene_index[&EntityId::from(1_u64)];
    let mut fragments = entry.segments(&frame.retained_scene_segments);
    assert_eq!(fragments.len(), 65);
    for i in 0..65 {
        assert_eq!(fragments.next().unwrap().bounds, segment(1, i as f32).bounds);
        assert_eq!(fragments.len(), 64 - i);
    }
    assert!(fragments.next().is_none());
    assert_eq!(std::mem::size_of::<Entry>(), 3 * std::mem::size_of::<usize>());
}

#[test]
fn replayed_fragment_does_not_keep_source_link() {
    let mut source = frame();
    source.push_retained_scene_segment(segment(1, 0.0));
    source.push_retained_scene_segment(segment(2, 10.0));
    source.push_retained_scene_segment(segment(1, 20.0));
    let mut destination = frame();
    destination.push_retained_scene_segment(source.retained_scene_segments[0].clone());
    let entry = &destination.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!(entry.segments(&destination.retained_scene_segments).count(), 1);
    destination.push_retained_scene_segment(source.retained_scene_segments[2].clone());
    let entry = &destination.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!(entry.segments(&destination.retained_scene_segments).count(), 2);
}
