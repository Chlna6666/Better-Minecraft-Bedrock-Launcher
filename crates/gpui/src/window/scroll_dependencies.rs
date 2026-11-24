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
    ListTop,
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
        if !reads.is_empty() {
            merge_reads(self.next_scroll_reads.entry(owner).or_default(), reads);
        }
    }

    pub(in crate::window) fn commit_scroll_reads(&mut self) {
        if self.replayed_dependency_ranges {
            for (owner, reads) in self.scroll_reads.drain() {
                if self.rendered_frame.dispatch_tree.view_path_reversed(owner).next().is_some() {
                    merge_reads(self.next_scroll_reads.entry(owner).or_default(), reads);
                }
            }
        }
        std::mem::swap(&mut self.scroll_reads, &mut self.next_scroll_reads);
        self.next_scroll_reads.clear();
    }

    /// Poll only observed properties at existing App/UI frame boundaries.
    /// No timer, new platform event, strong handle ownership or Notify effect.
    pub(crate) fn invalidate_scroll_reads(&mut self) {
        if self.scroll_reads.is_empty() || !self.invalidator.not_drawing() {
            return;
        }
        let mut changed = smallvec::SmallVec::<[EntityId; 4]>::new();
        self.scroll_reads.retain(|&owner, reads| {
            let mut dirty = false;
            reads.retain(|entry| match (entry.probe)() {
                Probe::Dead => false,
                Probe::Changed => { dirty = true; true }
                Probe::Unchanged => true,
            });
            if dirty { changed.push(owner); }
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
