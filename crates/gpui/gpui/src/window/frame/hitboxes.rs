//! UI-owned immutable hitbox pages. A retained scroll range shares the
//! original geometry and publishes only its placement, not a second bitmap
//! of hitbox coordinates. Clips remain in window space.
use crate::{Bounds, Hitbox, Pixels, Point, point, px};
use std::{ops::Range, rc::Rc};

const PAGE_LEN: usize = 128;

#[derive(Clone, Copy, Default, PartialEq)]
pub(in crate::window) struct Translation {
    x: f64,
    y: f64,
}

impl Translation {
    fn shifted(self, delta: Point<Pixels>) -> Self {
        Self { x: self.x + f64::from(delta.x.0), y: self.y + f64::from(delta.y.0) }
    }

    #[inline]
    pub(in crate::window) fn bounds(self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        if self == Self::default() { return bounds; }
        Bounds::new(point(
            px((f64::from(bounds.origin.x.0) + self.x) as f32),
            px((f64::from(bounds.origin.y.0) + self.y) as f32),
        ), bounds.size)
    }
}

struct Span {
    owner: Rc<Vec<Hitbox>>,
    start: usize,
    len: usize,
    end: usize,
    translation: Translation,
}

impl Span {
    fn begin(&self) -> usize { self.end - self.len }
    fn slice(&self, range: Range<usize>) -> &[Hitbox] {
        let start = self.start + range.start.max(self.begin()) - self.begin();
        let end = self.start + range.end.min(self.end) - self.begin();
        &self.owner[start..end]
    }
}

#[derive(Default)]
pub(crate) struct HitboxStore {
    spans: Vec<Span>,
    pending: Vec<Hitbox>,
    len: usize,
}

impl HitboxStore {
    pub(crate) fn len(&self) -> usize { self.len }
    pub(crate) fn is_empty(&self) -> bool { self.len == 0 }
    pub(in crate::window) fn capacity(&self) -> usize {
        self.spans.capacity() + self.pending.capacity()
    }
    pub(in crate::window) fn contains(&self, range: &Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.len
    }

    pub(crate) fn push(&mut self, hitbox: Hitbox) {
        self.pack_sparse_tail();
        self.push_pending(hitbox);
    }

    #[inline]
    fn push_pending(&mut self, hitbox: Hitbox) {
        self.pending.push(hitbox);
        self.len += 1;
        if self.pending.len() == PAGE_LEN { self.seal_pending(); }
    }

    // Keep the last borrowed fragment provisional. Adjacent row replays
    // can extend it to a dense run before it ever needs materialization.
    // Only a noncontiguous append or publication closes that opportunity.
    fn pack_sparse_tail(&mut self) {
        if !self.pending.is_empty() { return; }
        let Some(last) = self.spans.last() else { return; };
        if last.len * 4 >= last.owner.len() { return; }
        let span = self.spans.pop().unwrap();
        self.pending.extend(span.owner[span.start..span.start + span.len].iter().map(|hitbox| {
            let mut hitbox = hitbox.clone();
            hitbox.bounds = span.translation.bounds(hitbox.bounds);
            hitbox
        }));
        // These are existing logical records, not newly appended ones.
        debug_assert!(self.pending.len() < PAGE_LEN);
    }

    pub(in crate::window) fn freeze(&mut self) {
        self.pack_sparse_tail();
        self.seal_pending();
    }

    fn seal_pending(&mut self) {
        if self.pending.is_empty() { return; }
        let owner = Rc::new(std::mem::take(&mut self.pending));
        self.spans.push(Span {
            start: 0, len: owner.len(), end: self.len,
            owner, translation: Translation::default(),
        });
    }

    fn spans_in(&self, range: &Range<usize>) -> &[Span] {
        let start = self.spans.partition_point(|span| span.end <= range.start);
        let end = self.spans.partition_point(|span| span.begin() < range.end);
        &self.spans[start..end.max(start)]
    }

    pub(in crate::window) fn slices(&self, range: Range<usize>)
        -> impl DoubleEndedIterator<Item = (&[Hitbox], Translation)>
    {
        assert!(self.contains(&range));
        let pending_start = self.len - self.pending.len();
        let tail = &self.pending[range.start.saturating_sub(pending_start)
            ..range.end.saturating_sub(pending_start)];
        self.spans_in(&range).iter()
            .map(move |span| (span.slice(range.clone()), span.translation))
            .chain(std::iter::once((tail, Translation::default())))
            .filter(|(slice, _)| !slice.is_empty())
    }

    // Geometry is materialized only for APIs that return individual hitboxes.
    // Hit testing and admission use borrowed slices directly.
    pub(in crate::window) fn range(&self, range: Range<usize>)
        -> impl DoubleEndedIterator<Item = Hitbox> + '_
    {
        self.slices(range).flat_map(|(slice, translation)| {
            slice.iter().map(move |hitbox| {
                let mut hitbox = hitbox.clone();
                hitbox.bounds = translation.bounds(hitbox.bounds);
                hitbox
            })
        })
    }

    pub(in crate::window) fn iter(&self) -> impl DoubleEndedIterator<Item = Hitbox> + '_ {
        self.range(0..self.len)
    }

    pub(in crate::window) fn any_at(&self, range: Range<usize>, position: Point<Pixels>, delta: Point<Pixels>) -> bool {
        self.slices(range).any(|(slice, translation)| {
            let translation = translation.shifted(delta);
            slice.iter().any(|hitbox| {
                hitbox.content_mask.bounds.contains(&position)
                    && translation.bounds(hitbox.bounds).contains(&position)
            })
        })
    }

    pub(in crate::window) fn share(&mut self, source: &Self, range: Range<usize>, delta: Point<Pixels>) {
        assert!(source.contains(&range));
        if range.is_empty() { return; }
        for span in source.spans_in(&range) {
            let begin = range.start.max(span.begin());
            let end = range.end.min(span.end);
            let start = span.start + begin - span.begin();
            let len = end - begin;
            let translation = span.translation.shifted(delta);
            let adjacent = self.pending.is_empty() && self.spans.last().is_some_and(|last|
                Rc::ptr_eq(&last.owner, &span.owner) && last.start + last.len == start
                    && last.translation == translation);
            if adjacent {
                self.len += len;
                let last = self.spans.last_mut().unwrap();
                last.len += len;
                last.end = self.len;
                continue;
            }

            self.pack_sparse_tail();
            // Pack isolated sparse fragments together, but leave the first
            // fragment of a possible contiguous run borrowed. No whole-log
            // compaction pass is needed at frame publication.
            if !self.pending.is_empty() && len * 4 < span.owner.len() {
                for hitbox in &span.owner[start..start + len] {
                    let mut hitbox = hitbox.clone();
                    hitbox.bounds = translation.bounds(hitbox.bounds);
                    self.push_pending(hitbox);
                }
                continue;
            }
            self.seal_pending();
            self.len += len;
            self.spans.push(Span { owner: span.owner.clone(), start, len,
                end: self.len, translation });
        }
        let pending_start = source.len - source.pending.len();
        let translation = Translation::default().shifted(delta);
        for hitbox in &source.pending[range.start.saturating_sub(pending_start)
            ..range.end.saturating_sub(pending_start)]
        {
            let mut hitbox = hitbox.clone();
            hitbox.bounds = translation.bounds(hitbox.bounds);
            self.push(hitbox);
        }
    }

    pub(in crate::window) fn truncate(&mut self, len: usize) {
        if len >= self.len { return; }
        let pending_start = self.len - self.pending.len();
        if len >= pending_start {
            self.pending.truncate(len - pending_start);
        } else {
            self.pending.clear();
            self.spans.truncate(self.spans.partition_point(|span| span.begin() < len));
            if let Some(last) = self.spans.last_mut() && last.end > len {
                last.len -= last.end - len;
                last.end = len;
            }
        }
        self.len = len;
    }

    pub(in crate::window) fn clear(&mut self) {
        self.pending.clear();
        for span in self.spans.drain(..) {
            if let Ok(mut owner) = Rc::try_unwrap(span.owner)
                && owner.capacity() > self.pending.capacity()
            {
                owner.clear();
                self.pending = owner;
            }
        }
        self.len = 0;
    }

    pub(in crate::window) fn trim_for_reuse(&mut self, current: &Self) {
        let target = current.spans.len().max(4);
        if self.spans.capacity() > target.saturating_mul(4) { self.spans.shrink_to(target); }
    }

    pub(in crate::window) fn trim(&mut self, force: bool, floor: usize) {
        let target = self.spans.len().max(if floor == 0 { 0 } else { 4 });
        if force || self.spans.capacity() > target.saturating_mul(4) {
            self.spans.shrink_to(target);
        }
        if force { self.pending.shrink_to(floor.min(PAGE_LEN)); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentMask, HitboxBehavior, HitboxId, size};

    fn hitbox(index: usize) -> Hitbox {
        let bounds = Bounds::new(point(px(0.), px(index as f32)), size(px(10.), px(1.)));
        Hitbox {
            id: HitboxId(index as u64), bounds,
            content_mask: ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(1000.)))),
            behavior: HitboxBehavior::Normal,
        }
    }

    #[test]
    fn projected_pages_share_storage_and_keep_fixed_clip() {
        let mut source = HitboxStore::default();
        for index in 0..PAGE_LEN * 2 { source.push(hitbox(index)); }
        source.freeze();
        let mut target = HitboxStore::default();
        target.share(&source, 0..source.len(), point(px(0.), px(-10.)));
        assert!(Rc::ptr_eq(&source.spans[0].owner, &target.spans[0].owner));
        assert!(Rc::ptr_eq(&source.spans[1].owner, &target.spans[1].owner));
        let first = target.iter().next().unwrap();
        assert_eq!(first.bounds.origin.y, px(-10.));
        assert_eq!(first.content_mask, source.iter().next().unwrap().content_mask);
        assert_eq!(source.iter().next().unwrap().bounds.origin.y, px(0.));
        let mut second = HitboxStore::default();
        second.share(&target, 0..target.len(), point(px(0.), px(12.)));
        assert!(Rc::ptr_eq(&source.spans[0].owner, &second.spans[0].owner));
        assert_eq!(second.iter().next().unwrap().bounds.origin.y, px(2.));
        assert!(second.any_at(0..1, point(px(5.), px(2.5)), Point::default()));
        assert!(!second.any_at(0..1, point(px(5.), px(2.5)), point(px(0.), px(10.))));
        second.truncate(PAGE_LEN + 1);
        source.clear();
        target.clear();
        assert_eq!(second.iter().rev().next().unwrap().bounds.origin.y, px(PAGE_LEN as f32 + 2.));
    }

    #[test]
    fn consecutive_single_row_replays_coalesce_before_materializing() {
        let mut source = HitboxStore::default();
        for index in 0..PAGE_LEN { source.push(hitbox(index)); }
        source.freeze();
        let mut target = HitboxStore::default();
        let delta = point(px(0.), px(5.));
        for index in 0..PAGE_LEN {
            target.share(&source, index..index + 1, delta);
            assert_eq!(target.len(), index + 1);
        }
        target.freeze();
        assert_eq!(target.spans.len(), 1);
        assert!(Rc::ptr_eq(&source.spans[0].owner, &target.spans[0].owner));
        assert_eq!(target.iter().last().unwrap().bounds.origin.y, px(PAGE_LEN as f32 + 4.));
    }

    #[test]
    fn sparse_fragments_pack_without_retaining_unrelated_pages() {
        let mut source = HitboxStore::default();
        for index in 0..PAGE_LEN * 2 { source.push(hitbox(index)); }
        let mut target = HitboxStore::default();
        target.share(&source, 1..2, point(px(0.), px(3.)));
        target.share(&source, PAGE_LEN..PAGE_LEN + 1, Point::default());
        assert_eq!(target.pending.len(), 2);
        assert!(source.spans.iter().all(|span| Rc::strong_count(&span.owner) == 1));
        assert_eq!(target.iter().map(|hitbox| hitbox.bounds.origin.y).collect::<Vec<_>>(),
            vec![px(4.), px(PAGE_LEN as f32)]);
    }
}
