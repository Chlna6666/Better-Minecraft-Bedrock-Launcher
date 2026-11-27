//! Scroll dependencies use state versions, not per-getter executable probes.
//! StateVersion follows longbridge/gpui-fast fast/dependencies.rs (Apache-2.0;
//! Copyright 2022-2025 Zed Industries, Inc.; Copyright 2026 Longbridge).
use crate::{EntityId, Pixels, Point, Window, StateRead, StateReadStatus, state_generation};
pub(crate) use crate::StateVersion;
use std::{
    cell::{Ref, RefCell, RefMut},
    ops::{Deref, DerefMut},
};

/// Shared offset storage records actual changes at every mutable borrow,
/// including wheel, clamping, and uniform-list writes.
#[derive(Debug)]
pub(crate) struct ScrollOffset {
    value: RefCell<Point<Pixels>>,
    pub(crate) version: StateVersion,
}

impl Default for ScrollOffset {
    fn default() -> Self {
        Self {
            value: RefCell::default(),
            version: StateVersion::placement(),
        }
    }
}

impl ScrollOffset {
    pub(crate) fn borrow(&self) -> Ref<'_, Point<Pixels>> {
        self.value.borrow()
    }

    pub(crate) fn borrow_mut(&self) -> OffsetWrite<'_> {
        let value = self.value.borrow_mut();
        let before = *value;
        OffsetWrite { value, before, version: &self.version }
    }
}

pub(crate) struct OffsetWrite<'a> {
    value: RefMut<'a, Point<Pixels>>,
    before: Point<Pixels>,
    version: &'a StateVersion,
}

impl Deref for OffsetWrite<'_> {
    type Target = Point<Pixels>;
    fn deref(&self) -> &Self::Target { &self.value }
}

impl DerefMut for OffsetWrite<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.value }
}

impl Drop for OffsetWrite<'_> {
    fn drop(&mut self) {
        self.version.bump_if(*self.value != self.before);
    }
}

#[derive(Clone)]
pub(crate) struct MountedScrollSource {
    listener_index: usize,
    owner: EntityId,
    read: StateRead,
}

impl MountedScrollSource {
    fn new(listener_index: usize, owner: EntityId, version: &StateVersion) -> Self {
        Self { listener_index, owner, read: StateRead::new(version) }
    }

    pub(super) fn changed_for(&self, owner: EntityId, version: &StateVersion) -> bool {
        self.owner == owner && self.read.belongs_to(version)
            && self.read.status() == StateReadStatus::Changed
    }

    pub(super) fn unchanged(&self) -> bool {
        self.read.status() == StateReadStatus::Unchanged
    }

    pub(super) fn acknowledge(&mut self, version: &StateVersion) {
        debug_assert!(self.read.belongs_to(version));
        self.read = StateRead::new(version);
    }
}

/// Copy only the sources belonging to a replayed listener span. Indices
/// stay ordered and speculative frames never mutate the committed probes.
pub(crate) fn carry_scroll_sources(
    source: &[MountedScrollSource],
    target: &mut Vec<MountedScrollSource>,
    listeners: std::ops::Range<usize>,
    target_start: usize,
) {
    if source.is_empty() || listeners.is_empty() {
        return;
    }
    let start = source.partition_point(|entry| entry.listener_index < listeners.start);
    let end = source.partition_point(|entry| entry.listener_index < listeners.end);
    target.extend(source[start..end].iter().map(|entry| MountedScrollSource {
        listener_index: target_start + (entry.listener_index - listeners.start),
        owner: entry.owner,
        read: entry.read.clone(),
    }));
}

impl Window {
    /// Register immediately before the native wheel listener is appended.
    /// Its committed snapshot survives whole-view and deferred paint replay.
    pub(crate) fn watch_mounted_scroll(&mut self, owner: EntityId, version: &StateVersion) {
        self.next_frame.mounted_scroll_sources.push(MountedScrollSource::new(
            self.next_frame.mouse_listeners.len(), owner, version,
        ));
    }

    /// Scrolling is window-owned state, not a write to an application entity.
    /// Real model writes or user notifications still travel through App.
    pub(crate) fn invalidate_native_scroll(&mut self, owner: EntityId) {
        self.record_frame_request_reason(super::state::FrameRequestReason::Input);
        if self.invalidator.invalidate_scroll_views(std::iter::once(owner)) {
            self.schedule_dirty_frame();
        }
    }

    /// Poll mounted sources and actual Render reads at existing frame boundaries.
    /// No timer, new platform event, strong handle ownership or Notify effect.
    pub(crate) fn invalidate_scroll_reads(&mut self) {
        if !self.invalidator.not_drawing() {
            return;
        }
        let generation = state_generation();
        if self.checked_scroll_generation == Some(generation) {
            return;
        }
        self.checked_scroll_generation = Some(generation);
        let mut scrolled = smallvec::SmallVec::<[EntityId; 4]>::new();
        for source in &self.rendered_frame.mounted_scroll_sources {
            if source.read.status() == StateReadStatus::Changed && !scrolled.contains(&source.owner) {
                scrolled.push(source.owner);
            }
        }
        self.invalidator.invalidate_scroll_views(scrolled);
        let mut changed = smallvec::SmallVec::<[EntityId; 4]>::new();
        if self.invalidator.changed_state_owners(&mut changed) {
            self.invalidator.set_dirty(true);
        } else {
            self.invalidator.invalidate_dependent_views(changed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppDependencies;

    #[test]
    fn reads_keep_earliest_version_and_do_not_own_state() {
        let state = StateVersion::default();
        let mut dependencies = AppDependencies::default();
        let scope = dependencies.begin_state_reads();
        state.read();
        state.bump();
        state.read();
        drop(scope);
        let sets = dependencies.into_sets();
        let reads = &sets[&None].states;
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].status(), StateReadStatus::Changed);
        drop(state);
        assert_eq!(reads[0].status(), StateReadStatus::Dead);
    }

    #[test]
    fn offset_only_bumps_on_change() {
        let state = ScrollOffset::default();
        let mut dependencies = AppDependencies::default();
        let scope = dependencies.begin_state_reads();
        state.version.read();
        *state.borrow_mut() = Point::default();
        drop(scope);
        let sets = dependencies.into_sets();
        let reads = &sets[&None].states;
        assert_eq!(reads[0].status(), StateReadStatus::Unchanged);
        state.borrow_mut().y = crate::px(10.);
        assert_eq!(reads[0].status(), StateReadStatus::Changed);
    }

    #[test]
    fn mounted_scroll_sources_follow_replayed_listener_spans() {
        let state = StateVersion::default();
        let owner = EntityId::from(1);
        let source = vec![
            MountedScrollSource::new(2, owner, &state),
            MountedScrollSource::new(5, owner, &state),
            MountedScrollSource::new(9, owner, &state),
        ];
        let mut target = Vec::new();
        carry_scroll_sources(&source, &mut target, 3..9, 11);
        assert_eq!(target.len(), 1);
        assert_eq!(target[0].listener_index, 13);
        state.bump();
        assert_eq!(target[0].read.status(), StateReadStatus::Changed);
    }

    #[test]
    fn nested_scopes_restore_outer_capture() {
        let a = StateVersion::default();
        let b = StateVersion::default();
        let mut outer = AppDependencies::default();
        let outer_scope = outer.begin_state_reads();
        a.read();
        let mut inner = AppDependencies::default();
        let inner_scope = inner.begin_state_reads();
        b.read();
        drop(inner_scope);
        assert_eq!(inner.into_sets()[&None].states.len(), 1);
        b.read();
        drop(outer_scope);
        assert_eq!(outer.into_sets()[&None].states.len(), 2);
    }
}
