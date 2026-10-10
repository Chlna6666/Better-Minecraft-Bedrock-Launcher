use super::*;
use crate::input::{ActionRegistry, Keymap};

fn frame() -> Frame {
    Frame::new(DispatchTree::new(
        Rc::new(RefCell::new(Keymap::default())),
        Rc::new(ActionRegistry::default()),
    ))
}

fn segment(entity: u64, x: f32) -> RetainedSceneSegment {
    RetainedSceneSegment {
        entity_id: EntityId::from(entity),
        bounds: crate::bounds(
            crate::point(crate::px(x), crate::px(0.0)),
            crate::size(crate::px(10.0), crate::px(10.0)),
        )
        .scale(1.0),
        scene_range: 0..1,
        paint_range: Default::default(),
        prepaint_range: Default::default(),
    }
}

#[test]
fn repeated_entity_keeps_noncontiguous_segment_indices_and_union_bounds() {
    let mut frame = frame();
    frame.push_retained_scene_segment(segment(1, 0.0));
    frame.push_retained_scene_segment(segment(2, 10.0));
    frame.push_retained_scene_segment(segment(1, 20.0));
    let entry = &frame.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!(entry.indices.as_slice(), &[0, 2]);
    assert_eq!(entry.len(), 2);
    assert_eq!(
        entry.bounds,
        segment(1, 0.0).bounds.union(&segment(1, 20.0).bounds)
    );
    assert!(
        entry
            .segments(&frame.retained_scene_segments)
            .all(|segment| segment.entity_id == EntityId::from(1_u64))
    );
    assert!(frame.retained_scene_index_spill_capacity > 0);
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
    assert_eq!(next.retained_scene_index_spill_capacity, 0);
    assert_eq!(rendered.retained_scene_segments.len(), 1);
    assert_eq!(rendered.retained_scene_index.len(), 1);
    rendered.clear();
    rendered.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Aggressive);
    assert_eq!(rendered.retained_scene_index.capacity(), 0);
}

#[test]
fn memory_pressure_trims_spilled_indices_and_updates_accounting() {
    let mut frame = frame();
    for index in 0..65 {
        frame.push_retained_scene_segment(segment(1, index as f32));
    }
    let capacity_before = frame.retained_scene_index_spill_capacity;
    assert!(capacity_before > 65);
    frame.trim_retained_capacity_for_level(GpuiMemoryTrimLevel::Moderate);
    assert_eq!(frame.retained_scene_index_spill_capacity, 65);
    assert_eq!(frame.retained_scene_segments.len(), 65);
    let entry = &frame.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!(entry.len(), 65);
    assert_eq!(
        entry.spill_capacity(),
        frame.retained_scene_index_spill_capacity
    );
    frame.push_retained_scene_segment(segment(1, 65.0));
    let entry = &frame.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!(
        entry.spill_capacity(),
        frame.retained_scene_index_spill_capacity
    );
}
