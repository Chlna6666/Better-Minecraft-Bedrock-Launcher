//! Immutable callback pages. Callbacks retain their existing interior
//! mutability; a page never needs to be copied for event dispatch.
use crate::{MouseMoveEvent, window::hitbox::MouseListener};
use std::{any::TypeId, ops::Range, rc::Rc};

const PAGE_SIZE: usize = 64;

struct Page {
    items: Rc<Vec<MouseListener>>,
    end: usize,
    continuous_move: bool,
}

impl Page {
    fn start(&self) -> usize { self.end - self.items.len() }
    fn slice(&self, range: Range<usize>) -> &[MouseListener] {
        let start = self.start();
        &self.items[range.start.max(start) - start..range.end.min(self.end) - start]
    }
}

#[derive(Default)]
pub(crate) struct MouseListenerStore {
    pages: Vec<Page>,
    pending: Vec<MouseListener>,
    len: usize,
}

impl MouseListenerStore {
    pub(crate) fn len(&self) -> usize { self.len }
    pub(crate) fn is_empty(&self) -> bool { self.len == 0 }

    pub(crate) fn contains(&self, range: &Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.len
    }

    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &MouseListener> + '_ {
        self.pages.iter().flat_map(|page| page.items.iter()).chain(self.pending.iter())
    }

    fn pages_in(&self, range: &Range<usize>) -> &[Page] {
        let start = self.pages.partition_point(|page| page.end <= range.start);
        let end = self.pages.partition_point(|page| page.start() < range.end);
        &self.pages[start..end.max(start)]
    }

    pub(crate) fn range(&self, range: Range<usize>) -> impl Iterator<Item = &MouseListener> + '_ {
        assert!(self.contains(&range));
        let pending_start = self.len - self.pending.len();
        let tail = &self.pending[range.start.saturating_sub(pending_start)..range.end.saturating_sub(pending_start)];
        self.pages_in(&range).iter().flat_map(move |page| page.slice(range.clone())).chain(tail.iter())
    }

    pub(crate) fn push(&mut self, listener: MouseListener) {
        self.pending.push(listener);
        self.len += 1;
        if self.pending.len() >= PAGE_SIZE { self.freeze(); }
    }

    pub(crate) fn freeze(&mut self) {
        if self.pending.is_empty() { return; }
        let items = Rc::new(std::mem::take(&mut self.pending));
        let continuous_move = items.iter().any(|listener|
            listener.handles(TypeId::of::<MouseMoveEvent>(), true));
        self.pages.push(Page { items, end: self.len, continuous_move });
    }

    pub(crate) fn share(&mut self, source: &Self, range: Range<usize>) {
        assert!(source.contains(&range));
        if range.is_empty() { return; }
        for page in source.pages_in(&range) {
            if range.start <= page.start() && range.end >= page.end
                && page.items.len() >= PAGE_SIZE / 2
            {
                self.freeze();
                self.len += page.items.len();
                self.pages.push(Page {
                    items: Rc::clone(&page.items), end: self.len,
                    continuous_move: page.continuous_move,
                });
            } else {
                // Partial pages must not retain the retired callbacks in
                // the complement. Copy only the bounded survivor handles.
                for listener in page.slice(range.clone()) { self.push(listener.clone()); }
            }
        }
        let pending_start = source.len - source.pending.len();
        for listener in &source.pending[range.start.saturating_sub(pending_start)..range.end.saturating_sub(pending_start)] {
            self.push(listener.clone());
        }
    }

    pub(crate) fn has_continuous_move(&self) -> bool {
        self.pages.iter().any(|page| page.continuous_move)
            || self.pending.iter().any(|listener| listener.handles(TypeId::of::<MouseMoveEvent>(), true))
    }

    pub(crate) fn clear(&mut self) {
        self.pages.clear();
        self.pending.clear();
        self.len = 0;
    }

    pub(crate) fn capacity(&self) -> usize {
        // Match the previous diagnostic's listener-record units.
        self.pending.capacity() + self.pages.iter().map(|page| page.items.capacity()).sum::<usize>()
    }

    pub(crate) fn shrink_to(&mut self, floor: usize) {
        self.pages.shrink_to(floor.div_ceil(PAGE_SIZE));
        self.pending.shrink_to(floor.min(PAGE_SIZE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MouseDownEvent;

    #[test]
    fn mouse_boundary_shares_full_pages_and_releases_partial_retirements() {
        let mut old = MouseListenerStore::default();
        for _ in 0..PAGE_SIZE * 3 {
            old.push(MouseListener::new::<MouseDownEvent>(Box::new(|_, _, _, _| {})));
        }
        old.freeze();
        let middle = Rc::clone(&old.pages[1].items);
        let first = Rc::downgrade(&old.pages[0].items);
        let mut next = MouseListenerStore::default();
        next.share(&old, 1..PAGE_SIZE * 3);
        next.freeze();
        assert_eq!(next.len(), PAGE_SIZE * 3 - 1);
        assert!(next.pages.iter().any(|page| Rc::ptr_eq(&page.items, &middle)));
        assert_eq!(next.iter().rev().count(), next.len());
        assert_eq!(next.range(0..1).count(), 1);
        drop(old);
        assert!(first.upgrade().is_none(), "partial page must not pin retired callback handles");
    }
}
