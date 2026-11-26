//! Render-time scroll reads. Weak probes observe actual values, including
//! direct RefCell writes by native scrolling, layout clamp and public handles.
use crate::{EntityId, Window};
use collections::FxHashMap;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScrollReadKind {
    Offset,
    MaxOffset,
    Bounds,
    ItemBounds,
    ChildCount,
    TopItem,
    BottomItem,
    LogicalTop,
    LogicalBottom,
    ListTop,
    ListScrollbarOffset,
    ListScrollbarMaxOffset,
    ListItemBounds,
    ListCount,
    ListViewport,
    UniformFlipped,
    UniformScrollable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ReadKey {
    state: usize,
    kind: ScrollReadKind,
    index: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Probe {
    Unchanged,
    Changed,
    Dead,
}

pub(crate) struct ScrollRead {
    key: ReadKey,
    probe: Box<dyn Fn() -> Probe>,
}

pub(crate) type ScrollReads = FxHashMap<EntityId, Vec<ScrollRead>>;

// A native scroll source belongs to the same immutable frame span as its
// wheel listener. Keep it sparse, rather than adding another index to every
// PaintIndex or retaining an element metadata tree.
#[derive(Clone)]
pub(crate) struct MountedScrollSource {
    listener_index: usize,
    owner: EntityId,
    probe: Rc<dyn Fn() -> Probe>,
}

impl MountedScrollSource {
    fn new<S: 'static, T: PartialEq + 'static>(
        listener_index: usize,
        owner: EntityId,
        state: &Rc<RefCell<S>>,
        read: impl Fn(&S) -> T + 'static,
    ) -> Self {
        let value = read(&state.borrow());
        let state = Rc::downgrade(state);
        Self {
            listener_index,
            owner,
            probe: Rc::new(move || {
                let Some(state) = state.upgrade() else { return Probe::Dead; };
                let Ok(state) = state.try_borrow() else { return Probe::Unchanged; };
                if read(&state) == value { Probe::Unchanged } else { Probe::Changed }
            }),
        }
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
        probe: Rc::clone(&entry.probe),
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

/// A read kind and optional item index uniquely identify a getter on a
/// particular live allocation. No probe is allocated outside Render.
#[inline]
pub(crate) fn observe<S: 'static, T: Copy + PartialEq + 'static>(
    state: &Rc<RefCell<S>>,
    kind: ScrollReadKind,
    index: usize,
    value: T,
    read: impl Fn(&S) -> T + 'static,
) -> T {
    CAPTURE.with(|capture| {
        let mut capture = capture.borrow_mut();
        let Some(reads) = capture.as_mut() else { return; };
        let key = ReadKey { state: Rc::as_ptr(state) as usize, kind, index };
        if reads.iter().any(|entry| entry.key == key) {
            return;
        }
        let state = Rc::downgrade(state);
        reads.push(ScrollRead {
            key,
            probe: Box::new(move || {
                let Some(state) = state.upgrade() else { return Probe::Dead; };
                let Ok(state) = state.try_borrow() else { return Probe::Unchanged; };
                if read(&state) == value { Probe::Unchanged } else { Probe::Changed }
            }),
        });
    });
    value
}

fn merge_reads(target: &mut Vec<ScrollRead>, incoming: Vec<ScrollRead>) {
    for entry in incoming {
        if !target.iter().any(|current| current.key == entry.key) {
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
    pub(crate) fn watch_mounted_scroll<S: 'static, T: PartialEq + 'static>(
        &mut self,
        owner: EntityId,
        state: &Rc<RefCell<S>>,
        read: impl Fn(&S) -> T + 'static,
    ) {
        self.next_frame.mounted_scroll_sources.push(MountedScrollSource::new(
            self.next_frame.mouse_listeners.len(), owner, state, read,
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
            if (source.probe)() == Probe::Changed && !changed.contains(&source.owner) {
                changed.push(source.owner);
            }
        }
        self.scroll_reads.retain(|&owner, reads| {
            let mut dirty = false;
            reads.retain(|entry| match (entry.probe)() {
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
    fn reads_detect_direct_mutation_without_owning_the_handle() {
        let state = Rc::new(RefCell::new(1u32));
        let scope = ScrollReadScope::begin();
        observe(&state, ScrollReadKind::Offset, 0, 1, |value| *value);
        observe(&state, ScrollReadKind::Offset, 0, 1, |value| *value);
        let reads = scope.finish();
        assert_eq!(reads.len(), 1);
        assert_eq!((reads[0].probe)(), Probe::Unchanged);
        *state.borrow_mut() = 2;
        assert_eq!((reads[0].probe)(), Probe::Changed);
        drop(state);
        assert_eq!((reads[0].probe)(), Probe::Dead);
    }

    #[test]
    fn mounted_scroll_sources_follow_replayed_listener_spans() {
        let state = Rc::new(RefCell::new(1u32));
        let owner = EntityId::from(1);
        let source = vec![
            MountedScrollSource::new(2, owner, &state, |value| *value),
            MountedScrollSource::new(5, owner, &state, |value| *value),
            MountedScrollSource::new(9, owner, &state, |value| *value),
        ];
        let mut target = Vec::new();
        carry_scroll_sources(&source, &mut target, 3..9, 11);
        assert_eq!(target.len(), 1);
        assert_eq!(target[0].listener_index, 13);
        assert_eq!((target[0].probe)(), Probe::Unchanged);
        *state.borrow_mut() = 2;
        assert_eq!((target[0].probe)(), Probe::Changed);
        assert_eq!((source[1].probe)(), Probe::Changed);
        drop(state);
        assert_eq!((target[0].probe)(), Probe::Dead);
    }

    #[test]
    fn nested_scopes_restore_outer_capture() {
        let state = Rc::new(RefCell::new(1u32));
        let outer = ScrollReadScope::begin();
        observe(&state, ScrollReadKind::Offset, 0, 1, |value| *value);
        let inner = ScrollReadScope::begin();
        observe(&state, ScrollReadKind::MaxOffset, 0, 1, |value| *value);
        assert_eq!(inner.finish().len(), 1);
        observe(&state, ScrollReadKind::ChildCount, 0, 1, |value| *value);
        assert_eq!(outer.finish().len(), 2);
    }
}
