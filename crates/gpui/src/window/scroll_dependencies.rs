//! Scroll dependencies use state versions, not per-getter executable probes.
//! StateVersion follows longbridge/gpui-fast fast/dependencies.rs (Apache-2.0;
//! Copyright 2022-2025 Zed Industries, Inc.; Copyright 2026 Longbridge).
use crate::{EntityId, Pixels, Point, UniformListScroll, Window};
use collections::FxHashMap;
use std::{
    cell::{Cell, Ref, RefCell, RefMut},
    ops::{Deref, DerefMut},
    rc::{Rc, Weak},
};

#[derive(Clone, Default, Debug)]
pub(crate) struct StateVersion(Rc<Cell<u64>>);

impl StateVersion {
    #[inline]
    pub(crate) fn bump_if(&self, changed: bool) {
        if changed {
            self.0.set(self.0.get().wrapping_add(1));
        }
    }

    #[inline]
    pub(crate) fn bump(&self) {
        self.bump_if(true);
    }

    pub(crate) fn read(&self) {
        record(ScrollRead::Version(Rc::downgrade(&self.0), self.0.get()));
    }
}

/// Shared offset storage records actual changes at every mutable borrow,
/// including wheel, clamping, and uniform-list writes.
#[derive(Default, Debug)]
pub(crate) struct ScrollOffset {
    value: RefCell<Point<Pixels>>,
    pub(crate) version: StateVersion,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe { Unchanged, Changed, Dead }

#[derive(Clone)]
pub(crate) enum ScrollRead {
    Version(Weak<Cell<u64>>, u64),
    // The legacy public UniformListScroll fields still permit direct mutation.
    // One typed snapshot covers both getters until that public API is migrated.
    Uniform(Weak<RefCell<UniformListScroll>>, bool, bool),
}

impl ScrollRead {
    fn same_state(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Version(a, _), Self::Version(b, _)) => a.ptr_eq(b),
            (Self::Uniform(a, ..), Self::Uniform(b, ..)) => a.ptr_eq(b),
            _ => false,
        }
    }

    fn status(&self) -> Probe {
        match self {
            Self::Version(state, at) => match state.upgrade() {
                Some(state) if state.get() == *at => Probe::Unchanged,
                Some(_) => Probe::Changed,
                None => Probe::Dead,
            },
            Self::Uniform(state, flipped, scrollable) => {
                let Some(state) = state.upgrade() else { return Probe::Dead; };
                let Ok(state) = state.try_borrow() else { return Probe::Unchanged; };
                let now = state.last_item_size.is_some_and(|size| size.contents.height > size.item.height);
                if state.y_flipped == *flipped && now == *scrollable {
                    Probe::Unchanged
                } else {
                    Probe::Changed
                }
            }
        }
    }
}

pub(crate) type ScrollReads = FxHashMap<EntityId, Vec<ScrollRead>>;

#[derive(Clone)]
pub(crate) struct MountedScrollSource {
    listener_index: usize,
    owner: EntityId,
    read: ScrollRead,
}

impl MountedScrollSource {
    fn new(listener_index: usize, owner: EntityId, version: &StateVersion) -> Self {
        Self { listener_index, owner, read: ScrollRead::Version(Rc::downgrade(&version.0), version.0.get()) }
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

thread_local! {
    static CAPTURE: RefCell<Option<Vec<ScrollRead>>> = const { RefCell::new(None) };
}

/// Nested Render scopes restore the parent collector, including on unwind.
pub(crate) struct ScrollReadScope(Option<Option<Vec<ScrollRead>>>);

impl ScrollReadScope {
    pub(crate) fn begin() -> Self {
        Self(Some(CAPTURE.with(|capture| capture.replace(Some(Vec::new())))))
    }

    pub(crate) fn finish(mut self) -> Vec<ScrollRead> {
        CAPTURE.with(|capture| capture.replace(self.0.take().expect("active scroll read scope")).unwrap_or_default())
    }
}

impl Drop for ScrollReadScope {
    fn drop(&mut self) {
        // finish has already restored the parent (which may itself be None).
        if let Some(previous) = self.0.take() {
            CAPTURE.with(|capture| { capture.replace(previous); });
        }
    }
}

/// Keep the earliest observed version: a write between two reads still invalidates.
fn record(read: ScrollRead) {
    CAPTURE.with(|capture| {
        let mut capture = capture.borrow_mut();
        let Some(reads) = capture.as_mut() else { return; };
        if !reads.iter().any(|entry| entry.same_state(&read)) {
            reads.push(read);
        }
    });
}

pub(crate) fn observe_uniform(state: &Rc<RefCell<UniformListScroll>>, value: &UniformListScroll) {
    record(ScrollRead::Uniform(
        Rc::downgrade(state),
        value.y_flipped,
        value.last_item_size.is_some_and(|size| size.contents.height > size.item.height),
    ));
}

fn merge_reads(target: &mut Vec<ScrollRead>, incoming: Vec<ScrollRead>) {
    if target.is_empty() {
        *target = incoming;
        return;
    }
    for entry in incoming {
        if !target.iter().any(|current| current.same_state(&entry)) {
            target.push(entry);
        }
    }
}

impl Window {
    pub(crate) fn record_scroll_reads(&mut self, owner: EntityId, reads: Vec<ScrollRead>) {
        // Even an empty set is a new Render result. Keeping the previous
        // getters in that case resurrects dependencies Render stopped reading.
        if !reads.is_empty() || self.scroll_reads.contains_key(&owner) {
            merge_reads(self.next_scroll_reads.entry(owner).or_default(), reads);
        }
    }

    pub(in crate::window) fn commit_scroll_reads(&mut self) {
        for owner in &self.replayed_view_owners {
            if let Some(reads) = self.scroll_reads.remove(owner) {
                merge_reads(self.next_scroll_reads.entry(*owner).or_default(), reads);
            }
        }
        self.next_scroll_reads.retain(|_, reads| !reads.is_empty());
        std::mem::swap(&mut self.scroll_reads, &mut self.next_scroll_reads);
        self.next_scroll_reads.clear();
    }

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
        if self.invalidator.invalidate_dependent_views(std::iter::once(owner)) {
            self.schedule_dirty_frame();
        }
    }

    /// Poll mounted sources and actual Render reads at existing frame boundaries.
    /// No timer, new platform event, strong handle ownership or Notify effect.
    pub(crate) fn invalidate_scroll_reads(&mut self) {
        if !self.invalidator.not_drawing() {
            return;
        }
        let mut changed = smallvec::SmallVec::<[EntityId; 4]>::new();
        for source in &self.rendered_frame.mounted_scroll_sources {
            if source.read.status() == Probe::Changed && !changed.contains(&source.owner) {
                changed.push(source.owner);
            }
        }
        self.scroll_reads.retain(|&owner, reads| {
            let mut dirty = false;
            reads.retain(|entry| match entry.status() {
                Probe::Dead => false,
                Probe::Changed => { dirty = true; true }
                Probe::Unchanged => true,
            });
            if dirty && !changed.contains(&owner) { changed.push(owner); }
            !reads.is_empty()
        });
        self.invalidator.invalidate_dependent_views(changed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_keep_earliest_version_and_do_not_own_state() {
        let state = StateVersion::default();
        let scope = ScrollReadScope::begin();
        state.read();
        state.bump();
        state.read();
        let reads = scope.finish();
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].status(), Probe::Changed);
        drop(state);
        assert_eq!(reads[0].status(), Probe::Dead);
    }

    #[test]
    fn offset_only_bumps_on_change() {
        let state = ScrollOffset::default();
        let scope = ScrollReadScope::begin();
        state.version.read();
        *state.borrow_mut() = Point::default();
        let reads = scope.finish();
        assert_eq!(reads[0].status(), Probe::Unchanged);
        state.borrow_mut().y = crate::px(10.);
        assert_eq!(reads[0].status(), Probe::Changed);
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
        assert_eq!(target[0].read.status(), Probe::Changed);
    }

    #[test]
    fn nested_scopes_restore_outer_capture() {
        let a = StateVersion::default();
        let b = StateVersion::default();
        let outer = ScrollReadScope::begin();
        a.read();
        let inner = ScrollReadScope::begin();
        b.read();
        assert_eq!(inner.finish().len(), 1);
        b.read();
        assert_eq!(outer.finish().len(), 2);
    }
}
