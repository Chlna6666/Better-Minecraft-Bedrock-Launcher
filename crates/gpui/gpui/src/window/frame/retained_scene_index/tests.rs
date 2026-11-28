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

#[test]
fn fragment_splice_relinks_interleaved_owners_and_retires_last_reader() {
    let mut old = frame();
    let mut replacement = frame();
    for (position, entity) in [1, 2, 1, 3, 1].into_iter().enumerate() {
        let mut fragment = segment(entity, position as f32);
        fragment.scene_range = position..position + 1;
        old.push_retained_scene_segment(fragment);
    }
    let mut fragment = segment(1, 10.);
    fragment.scene_range = 0..1;
    replacement.push_retained_scene_segment(fragment);
    let mut fragment = segment(4, 11.);
    fragment.scene_range = 1..2;
    replacement.push_retained_scene_segment(fragment);

    let plan = RecordSplicePlan::new(5, 2, &[(1..2, 0..2), (3..4, 0..0)]).unwrap();
    let candidate = SegmentSplice::prepare(&mut old, &replacement, &plan, &plan).unwrap();
    assert_eq!(old.retained_scene_index[&EntityId::from(1_u64)].len(), 3);
    assert_eq!(old.retained_scene_segments[0].next_for_entity.unwrap().get() - 1, 2);
    let retired = candidate.adopt(&mut old, &mut replacement);
    assert_eq!(retired.len(), 2);
    assert!(replacement.retained_scene_segments.is_empty());
    assert!(replacement.retained_scene_index.is_empty());
    assert!(!old.retained_scene_index.contains_key(&EntityId::from(2_u64)));
    assert!(!old.retained_scene_index.contains_key(&EntityId::from(3_u64)));
    let entry = &old.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!((entry.first, entry.last, entry.len()), (0, 4, 4));
    assert_eq!(
        entry.segments(&old.retained_scene_segments)
            .map(|fragment| fragment.scene_range.clone())
            .collect::<Vec<_>>(),
        vec![0..1, 1..2, 3..4, 4..5]
    );
    assert_eq!(old.retained_scene_index[&EntityId::from(4_u64)].first, 2);
    assert!(old.retained_scene_segments[4].next_for_entity.is_none());
}

#[test]
fn fragment_delta_preserves_stable_prefix_and_coordinate_only_links() {
    let mut old = frame();
    let mut replacement = frame();
    for (position, entity) in [9, 1, 2, 1, 8].into_iter().enumerate() {
        let mut fragment = segment(entity, position as f32);
        fragment.scene_range = position..position + 1;
        old.push_retained_scene_segment(fragment);
    }
    let mut fragment = segment(3, 10.);
    fragment.scene_range = 0..1;
    replacement.push_retained_scene_segment(fragment);
    let fragments = RecordSplicePlan::new(5, 1, &[(2..3, 0..1)]).unwrap();
    let scene = RecordSplicePlan::new(5, 2, &[(2..3, 0..2)]).unwrap();
    let candidate = SegmentSplice::prepare(&mut old, &replacement, &fragments, &scene).unwrap();
    assert_eq!(candidate.affected.len(), 2);
    assert!(candidate.affected.contains(&EntityId::from(2_u64)));
    assert!(candidate.affected.contains(&EntityId::from(3_u64)));
    assert!(!candidate.index.contains_key(&EntityId::from(1_u64)));
    assert!(!candidate.index.contains_key(&EntityId::from(9_u64)));
    assert_eq!(candidate.links.len(), 1);
    let retired = candidate.adopt(&mut old, &mut replacement);
    assert_eq!(retired.len(), 1);
    let entry = &old.retained_scene_index[&EntityId::from(1_u64)];
    assert_eq!((entry.first, entry.last, entry.len()), (1, 3, 2));
    assert_eq!(old.retained_scene_segments[1].next_for_entity.unwrap().get() - 1, 3);
    assert_eq!(old.retained_scene_segments[3].scene_range, 4..5);
    assert_eq!(old.retained_scene_segments[4].scene_range, 5..6);
    assert!(!old.retained_scene_index.contains_key(&EntityId::from(2_u64)));
}
