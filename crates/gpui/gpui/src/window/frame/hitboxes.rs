//! UI-owned immutable hitbox pages. A retained scroll range shares the
//! original geometry and publishes only its placement, not a second bitmap
//! of hitbox coordinates. Clips remain in window space.
use crate::{Bounds, Hitbox, HitboxId, Pixels, Point, point, px};
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

// One conservative spatial summary per physical page, not per replay span.
struct HitboxPage {
    records: Vec<Hitbox>,
    envelope: Option<PageEnvelope>,
}

struct PageEnvelope {
    min: Point<Pixels>,
    max_origin: Point<Pixels>,
    max_size: crate::Size<Pixels>,
}

impl PageEnvelope {
    fn new(records: &[Hitbox]) -> Option<Self> {
        // Tiny pages are cheaper to scan than to index.
        if records.len() < 16 {
            return None;
        }
        let first = records.first()?.bounds;
        let mut result = Self {
            min: first.origin,
            max_origin: first.origin,
            max_size: first.size,
        };
        for hitbox in records {
            let bounds = hitbox.bounds;
            let mask = hitbox.content_mask.bounds;
            if ![
                bounds.origin.x.0, bounds.origin.y.0, bounds.size.width.0, bounds.size.height.0,
                mask.origin.x.0, mask.origin.y.0, mask.size.width.0, mask.size.height.0,
            ].iter().all(|value| value.is_finite())
                || bounds.size.width.0 < 0.0 || bounds.size.height.0 < 0.0
                || mask.size.width.0 < 0.0 || mask.size.height.0 < 0.0
                || !mask.right().0.is_finite() || !mask.bottom().0.is_finite()
            {
                return None;
            }
            result.min.x.0 = result.min.x.0.min(bounds.origin.x.0);
            result.min.y.0 = result.min.y.0.min(bounds.origin.y.0);
            result.max_origin.x.0 = result.max_origin.x.0.max(bounds.origin.x.0);
            result.max_origin.y.0 = result.max_origin.y.0.max(bounds.origin.y.0);
            result.max_size.width.0 = result.max_size.width.0.max(bounds.size.width.0);
            result.max_size.height.0 = result.max_size.height.0.max(bounds.size.height.0);
        }
        Some(result)
    }

    fn may_hit(&self, position: Point<Pixels>, translation: Translation) -> bool {
        let low = translation.bounds(Bounds::new(self.min, self.max_size)).origin;
        let high = translation.bounds(Bounds::new(self.max_origin, self.max_size)).bottom_right();
        // The exact test intersects rectangles and reconstructs their width.
        // Allow conservative floating-point roundoff at that final edge.
        fn axis(value: f32, low: f32, high: f32) -> bool {
            if !low.is_finite() || !high.is_finite() || !value.is_finite() {
                return true;
            }
            let margin = f64::from(low.abs().max(high.abs()).max(1.0))
                * f64::from(f32::EPSILON) * 8.0;
            f64::from(value) >= f64::from(low) - margin
                && f64::from(value) <= f64::from(high) + margin
        }
        axis(position.x.0, low.x.0, high.x.0)
            && axis(position.y.0, low.y.0, high.y.0)
    }
}

struct Span {
    owner: Rc<HitboxPage>,
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
        &self.owner.records[start..end]
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
    pub(in crate::window) fn account_storage(&self, accounting: &mut crate::diagnostics::frame_storage::Accounting) {
        use crate::diagnostics::frame_storage::Kind;
        accounting.add(&self.spans, Kind::Hitbox);
        accounting.add(&self.pending, Kind::Hitbox);
        for span in &self.spans {
            accounting.add(&span.owner.records, Kind::Hitbox);
        }
    }

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
        if last.len * 4 >= last.owner.records.len() { return; }
        let span = self.spans.pop().unwrap();
        self.pending.extend(span.owner.records[span.start..span.start + span.len].iter().map(|hitbox| {
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
        let records = std::mem::take(&mut self.pending);
        let envelope = PageEnvelope::new(&records);
        let owner = Rc::new(HitboxPage { records, envelope });
        self.spans.push(Span {
            start: 0, len: owner.records.len(), end: self.len,
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

    /// Filter physical pages before projecting individual rectangles. Sparse
    /// slices use the same page envelope; it can only admit extra candidates.
    pub(in crate::window) fn candidate_slices(
        &self,
        range: Range<usize>,
        position: Point<Pixels>,
        delta: Point<Pixels>,
    ) -> impl DoubleEndedIterator<Item = (&[Hitbox], Translation)> {
        assert!(self.contains(&range));
        let pending_start = self.len - self.pending.len();
        let tail = &self.pending[range.start.saturating_sub(pending_start)
            ..range.end.saturating_sub(pending_start)];
        self.spans_in(&range).iter()
            .filter_map(move |span| {
                let translation = span.translation.shifted(delta);
                span.owner.envelope.as_ref()
                    .is_none_or(|envelope| envelope.may_hit(position, translation))
                    .then(|| (span.slice(range.clone()), translation))
            })
            .chain(std::iter::once((tail, Translation::default().shifted(delta))))
            .filter(|(slice, _)| !slice.is_empty())
    }

    // Geometry is materialized only for APIs that return individual hitboxes.
    // Hit testing and admission use borrowed slices directly.
    #[cfg(test)]
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

    #[cfg(test)]
    pub(in crate::window) fn iter(&self) -> impl DoubleEndedIterator<Item = Hitbox> + '_ {
        self.range(0..self.len)
    }

    /// Find the first logical occurrence, as the old flat iterator did.
    /// Only the matching rectangle needs projection, not every preceding
    /// hitbox. Returning a value also ends the frame borrow before paint.
    pub(in crate::window) fn bounds_for_id(&self, id: HitboxId) -> Option<Bounds<Pixels>> {
        for (slice, translation) in self.slices(0..self.len) {
            if let Some(hitbox) = slice.iter().find(|hitbox| hitbox.id == id) {
                return Some(translation.bounds(hitbox.bounds));
            }
        }
        None
    }

    pub(in crate::window) fn any_at(&self, range: Range<usize>, position: Point<Pixels>, delta: Point<Pixels>) -> bool {
        self.candidate_slices(range, position, delta).any(|(slice, translation)| {
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
            if !self.pending.is_empty() && len * 4 < span.owner.records.len() {
                for hitbox in &span.owner.records[start..start + len] {
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

    /// Split at most two page descriptors and translate the retained
    /// range in place. Hitbox payloads, IDs, order and fixed clips remain
    /// untouched. Called only after the Frame transaction validates it.
    pub(in crate::window) fn translate_committed(&mut self, range: Range<usize>, delta: Point<Pixels>) {
        assert!(self.pending.is_empty() && self.contains(&range));
        if range.is_empty() || delta == Point::default() { return; }
        self.split_at(range.start);
        self.split_at(range.end);
        let first = self.spans.partition_point(|span| span.end <= range.start);
        let end = self.spans.partition_point(|span| span.end <= range.end);
        for span in &mut self.spans[first..end] {
            span.translation = span.translation.shifted(delta);
        }
    }

    fn split_at(&mut self, index: usize) {
        let slot = self.spans.partition_point(|span| span.end <= index);
        let Some(span) = self.spans.get_mut(slot) else { return; };
        if index == span.begin() { return; }
        let left = index - span.begin();
        let right = Span { owner: span.owner.clone(), start: span.start + left,
            len: span.len - left, end: span.end, translation: span.translation };
        span.len = left;
        span.end = index;
        self.spans.insert(slot + 1, right);
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
            if let Ok(owner) = Rc::try_unwrap(span.owner)
                && owner.records.capacity() > self.pending.capacity()
            {
                self.pending = owner.records;
                self.pending.clear();
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
    use crate::{ContentMask, HitboxBehavior, size, window::hitbox::HitboxId};

    fn hitbox(index: usize) -> Hitbox {
        let bounds = Bounds::new(point(px(0.), px(index as f32)), size(px(10.), px(1.)));
        Hitbox {
            id: HitboxId(index as u64), bounds,
            content_mask: ContentMask::new(Bounds::new(point(px(0.), px(0.)), size(px(100.), px(1000.)))),
            behavior: HitboxBehavior::Normal,
        }
    }

    #[test]
    fn candidate_pages_preserve_exact_hits_after_translation_and_splitting() {
        let mut store = HitboxStore::default();
        for index in 0..PAGE_LEN * 4 {
            store.push(hitbox(index));
        }
        store.freeze();
        store.translate_committed(17..PAGE_LEN * 3 + 5, point(px(2.25), px(-19.75)));
        for y in -30..600 {
            let position = point(px(5.), px(y as f32 + 0.25));
            let expected: Vec<_> = store.slices(0..store.len()).flat_map(|(slice, translation)| {
                slice.iter().filter(move |hitbox| {
                    translation.bounds(hitbox.bounds).intersect(&hitbox.content_mask.bounds)
                        .contains(&position)
                }).map(|hitbox| hitbox.id)
            }).collect();
            let actual: Vec<_> = store.candidate_slices(0..store.len(), position, Point::default())
                .flat_map(|(slice, translation)| {
                    slice.iter().filter(move |hitbox| {
                        translation.bounds(hitbox.bounds).intersect(&hitbox.content_mask.bounds)
                            .contains(&position)
                    }).map(|hitbox| hitbox.id)
                }).collect();
            assert_eq!(actual, expected);
        }
        let position = point(px(5.), px(450.));
        let candidates: usize = store.candidate_slices(0..store.len(), position, Point::default())
            .map(|(slice, _)| slice.len()).sum();
        assert!(candidates < PAGE_LEN * 2, "unrelated pages should not be projected");
    }

    #[test]
    fn invalid_geometry_disables_page_rejection() {
        let mut records: Vec<_> = (0..PAGE_LEN).map(hitbox).collect();
        records[4].bounds.origin.y = px(f32::NAN);
        assert!(PageEnvelope::new(&records).is_none());
    }

    #[test]
    fn committed_translation_splits_only_descriptors_and_keeps_fixed_clips() {
        let mut store = HitboxStore::default();
        for index in 0..PAGE_LEN * 2 { store.push(hitbox(index)); }
        store.freeze();
        let owners = [store.spans[0].owner.clone(), store.spans[1].owner.clone()];
        let before = store.iter().collect::<Vec<_>>();
        let moved = 17..PAGE_LEN + 11;
        store.translate_committed(moved.clone(), point(px(0.), px(-5.)));
        assert_eq!(store.spans.len(), 4);
        assert_eq!(store.len(), before.len());
        assert!(store.spans.iter().all(|span| owners.iter().any(|owner| Rc::ptr_eq(owner, &span.owner))));
        for (index, actual) in store.iter().enumerate() {
            assert_eq!(actual.id, before[index].id);
            assert_eq!(actual.content_mask, before[index].content_mask);
            assert_eq!(actual.bounds.origin.y, before[index].bounds.origin.y
                + if moved.contains(&index) { px(-5.) } else { px(0.) });
        }
        store.translate_committed(moved, point(px(0.), px(5.)));
        assert_eq!(store.spans.len(), 4, "stable range never fragments further");
        for (actual, before) in store.iter().zip(before) {
            assert_eq!(actual.bounds, before.bounds);
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
        assert_eq!(target.bounds_for_id(first.id), Some(first.bounds));
        assert_eq!(target.bounds_for_id(HitboxId(u64::MAX)), None);
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
