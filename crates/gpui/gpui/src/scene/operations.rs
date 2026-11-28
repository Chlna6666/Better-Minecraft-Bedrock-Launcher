//! Ordered Paint storage; shared payloads do not retain earlier Scenes.
use super::{DrawOrder, PaintOperation, PaintOperationRef, operation_page::OperationPage};
use crate::{Bounds, ScaledPixels};
use std::{
    ops::Range,
    sync::Arc,
};

#[cfg(test)]
mod tests;

#[derive(Clone)]
struct Segment {
    owner: Arc<OperationPage>,
    range: Range<usize>,
    end: usize,
}
impl Segment {
    fn start(&self) -> usize {
        self.end - self.range.len()
    }
}

#[derive(Default)]
struct SegmentTable(Option<Arc<Vec<Segment>>>);

impl std::ops::Deref for SegmentTable {
    type Target = [Segment];

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}

impl SegmentTable {
    fn make_mut(&mut self) -> &mut Vec<Segment> {
        Arc::make_mut(self.0.get_or_insert_with(Arc::default))
    }

    fn take(&mut self) -> Vec<Segment> {
        self.0.take().map_or_else(Vec::new, Arc::unwrap_or_clone)
    }

    fn shares_storage(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        }
    }
}

/// The only ordered operation payload, with immutable retained ranges and new append storage.
#[derive(Default)]
pub(crate) struct Operations {
    segments: SegmentTable,
    pending: OperationPage,
    len: usize,
    // Sparse prefix states belong to the authoritative operation sequence.
    boundaries: ReplayBoundaries,
}

/// A bounded logical view that can cross allocation boundaries.
#[derive(Clone, Copy)]
pub(crate) struct OperationRange<'a> {
    operations: &'a Operations,
    start: usize,
    end: usize,
}

/// A logical range within one immutable owner. No consumer can access the
/// physical index array, or assume a full enum lives at its address.
#[derive(Clone, Copy, Default)]
pub(crate) struct OperationRun<'a> {
    owner: Option<&'a OperationPage>,
    start: usize,
    end: usize,
}

impl<'a> OperationRun<'a> {
    fn new(owner: &'a OperationPage, range: Range<usize>) -> Self {
        Self { owner: Some(owner), start: range.start, end: range.end }
    }

    pub(crate) fn len(self) -> usize { self.end - self.start }

    fn is_empty(self) -> bool { self.start == self.end }

    pub(super) fn is_scroll_only(self) -> bool {
        !self.is_empty() && self.owner.is_some_and(OperationPage::is_scroll_only)
    }

    pub(crate) fn get(self, index: usize) -> Option<PaintOperationRef<'a>> {
        if index >= self.len() { return None; }
        Some(self.owner?.view(self.start + index))
    }

    pub(crate) fn range(self, range: Range<usize>) -> Self {
        assert!(range.start <= range.end && range.end <= self.len());
        Self { owner: self.owner, start: self.start + range.start, end: self.start + range.end }
    }

    pub(crate) fn iter(self) -> impl DoubleEndedIterator<Item = PaintOperationRef<'a>> + Clone {
        (self.start..self.end).map(move |index| self.owner.expect("nonempty operation run").view(index))
    }

    pub(super) fn share_quad_run(self, stream: &mut super::PrimitiveStream<super::Quad>) -> bool {
        self.owner.is_some_and(|owner| owner.share_quad_run(self.start..self.end, stream))
    }

    pub(super) fn share_mono_run(self, stream: &mut super::PrimitiveStream<super::MonochromeSprite>) -> bool {
        self.owner.is_some_and(|owner| owner.share_mono_run(self.start..self.end, stream))
    }

    pub(super) fn share_poly_run(self, stream: &mut super::PrimitiveStream<super::PolychromeSprite>) -> bool {
        self.owner.is_some_and(|owner| owner.share_poly_run(self.start..self.end, stream))
    }

    fn same_storage(self, other: OperationRun<'_>) -> bool {
        self.len() == other.len() && self.start == other.start
            && self.owner.zip(other.owner).is_some_and(|(a, b)| std::ptr::eq(a, b))
    }
}

// Immutable placement variants have identical scope tags and prefix
// states. Share their sparse index, retaining a unique append buffer
// only for ordinary builds. Empty scenes allocate no boundary table.
#[derive(Clone, Default)]
struct ReplayBoundaries(Option<Arc<Vec<ReplayBoundary>>>);

impl std::ops::Deref for ReplayBoundaries {
    type Target = [ReplayBoundary];

    fn deref(&self) -> &Self::Target {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}

impl ReplayBoundaries {
    fn push(&mut self, boundary: ReplayBoundary) {
        Arc::make_mut(self.0.get_or_insert_with(Arc::default)).push(boundary);
    }

    fn clear(&mut self) {
        if let Some(boundaries) = self.0.as_mut().and_then(Arc::get_mut) {
            boundaries.clear();
        } else {
            self.0 = None;
        }
    }

    fn capacity(&self) -> usize {
        self.0.as_ref().filter(|owner| Arc::strong_count(owner) == 1)
            .map_or(0, |owner| owner.capacity())
    }

    fn shrink_to_fit(&mut self) {
        if let Some(boundaries) = self.0.as_mut().and_then(Arc::get_mut) {
            boundaries.shrink_to_fit();
        }
    }
}

#[derive(Clone, Copy, Default)]
struct ReplayBoundary {
    after: usize,
    // Logical operation offsets remain usize. A scope depth has a much
    // smaller practical bound and is checked on increment.
    blur_depth: u32,
    layer_depth: u32,
    change: ScopeChange,
    invalid: bool,
    has_layer: bool,
    has_deferred: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
#[repr(u8)]
enum ScopeChange {
    #[default]
    None,
    DeferredScroll,
    StartBlur,
    EndBlur,
    StartLayer,
    EndLayer,
}

impl ScopeChange {
    fn of(operation: PaintOperationRef<'_>) -> Self {
        match operation {
            PaintOperationRef::Primitive(_) => Self::None,
            PaintOperationRef::DeferredScroll(_) => Self::DeferredScroll,
            PaintOperationRef::StartBlur(_) => Self::StartBlur,
            PaintOperationRef::EndBlur => Self::EndBlur,
            PaintOperationRef::StartLayer(_) => Self::StartLayer,
            PaintOperationRef::EndLayer => Self::EndLayer,
        }
    }
}

impl ReplayBoundary {
    fn advance(mut self, after: usize, change: ScopeChange) -> Self {
        self.after = after;
        self.change = change;
        match change {
            ScopeChange::StartBlur => {
                self.blur_depth = self.blur_depth.checked_add(1).expect("paint blur scope depth overflow");
            }
            ScopeChange::StartLayer => {
                self.layer_depth = self.layer_depth.checked_add(1).expect("paint layer scope depth overflow");
                self.has_layer = true;
            }
            ScopeChange::EndBlur => match self.blur_depth.checked_sub(1) {
                Some(depth) => self.blur_depth = depth,
                None => self.invalid = true,
            },
            ScopeChange::EndLayer => match self.layer_depth.checked_sub(1) {
                Some(depth) => self.layer_depth = depth,
                None => self.invalid = true,
            },
            ScopeChange::DeferredScroll => self.has_deferred = true,
            ScopeChange::None => {}
        }
        self
    }

    fn independent(self) -> bool {
        !self.invalid && self.blur_depth == 0 && self.layer_depth == 0
    }
}

impl Operations {
    fn record_boundary(&mut self, after: usize, change: ScopeChange) {
        if change != ScopeChange::None {
            let state = self.boundaries.last().copied().unwrap_or_default();
            self.boundaries.push(state.advance(after, change));
        }
    }

    fn boundary_at(&self, index: usize) -> ReplayBoundary {
        let end = self.boundaries.partition_point(|boundary| boundary.after <= index);
        end.checked_sub(1).map_or(ReplayBoundary::default(), |i| self.boundaries[i])
    }

    /// A fragment is replayable when neither endpoint inherits an external
    /// layer or capture, and no unmatched close has poisoned its prefix.
    pub(crate) fn range_is_replayable(&self, range: Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.len
            && (range.is_empty()
                || (self.boundary_at(range.start).independent()
                    && self.boundary_at(range.end).independent()))
    }

    #[cfg(test)]
    fn rebuild_boundaries(&mut self) {
        let mut boundaries = std::mem::take(&mut self.boundaries);
        boundaries.clear();
        let mut state = ReplayBoundary::default();
        for (index, operation) in self.views().enumerate() {
            let change = ScopeChange::of(operation);
            if change != ScopeChange::None {
                state = state.advance(index + 1, change);
                boundaries.push(state);
            }
        }
        self.boundaries = boundaries;
    }

    /// Deferred operations participate in the existing sparse prefix
    /// metadata. Sharing, editing and clearing ranges preserve the proof.
    pub(crate) fn has_deferred_scroll(&self) -> bool {
        self.boundaries.last().is_some_and(|boundary| boundary.has_deferred)
    }

    /// A flat root's scroll placements have no inherited ordering scope.
    /// The sparse replay metadata avoids scanning ordinary primitives.
    pub(crate) fn has_layer_scopes(&self) -> bool {
        self.boundaries.last().is_some_and(|boundary| boundary.has_layer)
    }

    pub(crate) fn deferred_scroll_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.boundaries.iter().filter_map(|boundary| {
            (boundary.change == ScopeChange::DeferredScroll).then_some(boundary.after - 1)
        })
    }

    pub(crate) fn deferred_scrolls(&self) -> impl Iterator<Item = &crate::window::scroll_paint::ScrollPlacement> {
        self.deferred_scroll_indices().filter_map(|index| match self.view(index) {
            Some(PaintOperationRef::DeferredScroll(placement)) => Some(placement),
            _ => None,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn capacity(&self) -> usize {
        self.len - self.pending.len() + self.pending.capacity()
    }

    /// Private backing storage only; immutable shared operation pages
    /// belong to their Arc owners and must not be counted per view.
    pub(crate) fn private_capacity_bytes(&self) -> u64 {
        (self.segments.0.as_ref().filter(|table| Arc::strong_count(table) == 1)
            .map_or(0, |table| table.capacity()) as u64).saturating_mul(std::mem::size_of::<Segment>() as u64)
            .saturating_add(self.pending.capacity_bytes() as u64)
            .saturating_add((self.boundaries.capacity() as u64).saturating_mul(std::mem::size_of::<ReplayBoundary>() as u64))
    }
    pub(super) fn account_storage(&self, accounting: &mut crate::diagnostics::frame_storage::Accounting) {
        use crate::diagnostics::frame_storage::Kind;
        if let Some(table) = &self.segments.0 {
            accounting.add(table, Kind::Descriptor);
        }
        if let Some(boundaries) = &self.boundaries.0 {
            accounting.add(boundaries, Kind::Descriptor);
        }
        for segment in self.segments.iter() {
            segment.owner.account_storage(accounting);
        }
        self.pending.account_storage(accounting);
    }

    pub(crate) fn shrink_to(&mut self, floor: usize) {
        self.pending.shrink_to(floor);
        if let Some(table) = self.segments.0.as_mut().and_then(Arc::get_mut) {
            table.shrink_to_fit();
        }
        self.boundaries.shrink_to_fit();
    }
    pub(super) fn collect_pending_poly_tiles(&self, ids: &mut collections::FxHashSet<(crate::AtlasTextureId, u32)>) {
        self.pending.collect_pending_poly_tiles(ids);
    }

    pub(crate) fn push(&mut self, operation: PaintOperation) {
        let change = ScopeChange::of(operation.as_ref());
        // Seal a marker-only run before the first ordinary append. A long
        // sequence of adjacent scroll rows still uses one marker page.
        if change != ScopeChange::DeferredScroll
            && !self.pending.is_empty() && self.pending.is_scroll_only()
        {
            self.finish();
        }
        self.len += 1;
        self.record_boundary(self.len, change);
        self.pending.push(operation);
    }

    pub(super) fn replay_glyphs(&mut self, run: OperationRun<'_>, order: DrawOrder) -> usize {
        if !self.pending.is_empty() && self.pending.is_scroll_only() {
            self.finish();
        }
        let count = self.pending.replay_glyphs(
            run.owner.expect("nonempty glyph replay"),
            run.start..run.end,
            order,
        );
        // Glyphs do not change the sparse layer/blur scope table.
        self.len += count;
        count
    }

    /// Caller has published any pending typed draw payloads. Keep mutable
    /// placements separate from immutable ordinary drawing allocations.
    pub(super) fn push_scroll_placement(&mut self, placement: crate::window::scroll_paint::ScrollPlacement) {
        if !self.pending.is_empty() && !self.pending.is_scroll_only() {
            self.finish();
        }
        self.push(PaintOperation::DeferredScroll(placement));
    }
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.boundaries.clear();
        if let Some(table) = self.segments.0.as_mut().and_then(Arc::get_mut) {
            for segment in table.drain(..) {
                if let Ok(mut owner) = Arc::try_unwrap(segment.owner)
                    && owner.capacity_bytes() > self.pending.capacity_bytes()
                {
                    owner.clear();
                    self.pending = owner;
                }
            }
        } else {
            self.segments.0 = None;
        }
        self.len = 0;
    }
    pub(crate) fn view(&self, index: usize) -> Option<PaintOperationRef<'_>> {
        if index >= self.len { return None; }
        let segment = self.segments.partition_point(|segment| segment.end <= index);
        Some(if let Some(segment) = self.segments.get(segment) {
            segment.owner.view(segment.range.start + index - segment.start())
        } else {
            self.pending.view(index - (self.len - self.pending.len()))
        })
    }

    #[cfg(test)]
    pub(crate) fn storage_identity(&self, index: usize) -> *const () {
        let segment = self.segments.partition_point(|segment| segment.end <= index);
        if let Some(segment) = self.segments.get(segment) {
            let local = segment.range.start + index - segment.start();
            segment.owner.range_identity(local..local + 1)
        } else {
            let local = index - (self.len - self.pending.len());
            self.pending.range_identity(local..local + 1)
        }
    }

    pub(crate) fn get_range(&self, range: Range<usize>) -> Option<OperationRange<'_>> {
        (range.start <= range.end && range.end <= self.len).then_some(OperationRange {
            operations: self,
            start: range.start,
            end: range.end,
        })
    }
    pub(crate) fn range(&self, range: Range<usize>) -> OperationRange<'_> {
        self.get_range(range)
            .expect("Paint range must belong to this generation")
    }
    pub(crate) fn all(&self) -> OperationRange<'_> {
        self.range(0..self.len)
    }
    pub(crate) fn views(&self) -> impl DoubleEndedIterator<Item = PaintOperationRef<'_>> + Clone {
        self.all().views()
    }

    pub(crate) fn matches_views(
        &self, other: &Self,
        compare: impl FnMut(PaintOperationRef<'_>, PaintOperationRef<'_>) -> bool,
    ) -> bool {
        self.all().matches_views(other.all(), compare)
    }

    /// Compares Paint values and, after the first visual change, checks whether the remaining
    /// operations still address the same prepared batch ranges.
    ///
    /// Immutable ranges shared by both Scenes prove both comparisons without visiting their
    /// operations. When batch topology is not needed, the comparison stops at the first visual
    /// change instead of adding another full Scene walk.
    pub(crate) fn scene_reuse_matches(
        &self,
        other: &Self,
        compare_batch_topology: bool,
    ) -> (bool, bool) {
        if self.len != other.len {
            return (false, false);
        }

        let mut paint_matches = true;
        let mut batch_topology_matches = true;
        let operations_match = self.matches_views(other, |left, right| {
            if paint_matches && left.same_paint(right) {
                return true;
            }

            paint_matches = false;
            if compare_batch_topology && left.has_same_batch_topology(right) {
                true
            } else {
                batch_topology_matches = false;
                false
            }
        });
        (
            paint_matches && operations_match,
            operations_match
                && (paint_matches || (compare_batch_topology && batch_topology_matches)),
        )
    }

    /// A fresh operation page and its typed draw builders can share their
    /// published allocations. Existing retained pages must never be resorted.
    #[cfg(test)]
    pub(super) fn publish_fresh_draw_payloads(
        &mut self,
        shadows: &mut super::PrimitiveStream<super::Shadow>,
        quads: &mut super::PrimitiveStream<super::Quad>,
        mono: &mut super::PrimitiveStream<super::MonochromeSprite>,
    ) {
        self.publish_draw_payloads(
            shadows, quads, mono, &mut super::PrimitiveStream::default(),
            &mut super::primitive_stream::DrawPermutation::default(), false,
        );
    }

    pub(super) fn publish_draw_payloads(
        &mut self,
        shadows: &mut super::PrimitiveStream<super::Shadow>,
        quads: &mut super::PrimitiveStream<super::Quad>,
        mono: &mut super::PrimitiveStream<super::MonochromeSprite>,
        poly: &mut super::PrimitiveStream<super::PolychromeSprite>,
        scratch: &mut super::primitive_stream::DrawPermutation,
        exclusive: bool,
    ) {
        if exclusive || self.segments.is_empty() {
            self.pending.publish_draw_payloads(shadows, quads, mono, poly, scratch, exclusive);
        }
    }

    /// Freeze newly appended operations without rewriting any existing immutable ranges.
    pub(crate) fn finish(&mut self) {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            let count = pending.len();
            self.len -= count;
            self.append(Arc::new(pending), 0..count);
        }
    }
    fn append(&mut self, owner: Arc<OperationPage>, range: Range<usize>) {
        self.len += range.len();
        let segments = self.segments.make_mut();
        if let Some(previous) = segments.last_mut()
            && Arc::ptr_eq(&previous.owner, &owner)
            && previous.range.end == range.start
        {
            previous.range.end = range.end;
            previous.end = self.len;
            return;
        }
        segments.push(Segment {
            owner,
            range,
            end: self.len,
        });
    }
    /// Release unrelated old page payload when the combined live views are sparse.
    pub(crate) fn compact(&mut self) {
        self.compact_pages(false);
    }

    pub(super) fn same_physical_ranges(&self, previous: &Self) -> bool {
        self.pending.is_empty() && previous.pending.is_empty()
            && self.len == previous.len
            && (self.segments.shares_storage(&previous.segments)
                || (self.segments.len() == previous.segments.len()
                    && self.segments.iter().zip(previous.segments.iter()).all(|(a, b)| {
                        Arc::ptr_eq(&a.owner, &b.owner) && a.range == b.range
                    })))
    }

    /// Replace sorted, distinct scroll markers in one pass. All changed
    /// markers share one new physical page; untouched ranges keep their
    /// owners and prefix states. Failure never mutates the source.
    pub(super) fn with_scroll_placements(
        &self,
        placements: &[(usize, crate::window::scroll_paint::ScrollPlacement)],
    ) -> Option<Self> {
        if !self.pending.is_empty() || placements.is_empty()
            || placements.windows(2).any(|pair| pair[0].0 >= pair[1].0)
            || placements.last()?.0 >= self.len
        {
            return None;
        }
        let mut marker_page = OperationPage::default();
        // The replacements are already sorted. Seek once, then advance
        // across immutable segment boundaries rather than binary searching
        // the complete table for every visible row.
        let first = self.segments.partition_point(|segment| segment.end <= placements[0].0);
        let mut segments = self.segments[first..].iter().peekable();
        for (index, placement) in placements {
            while segments.peek().is_some_and(|segment| segment.end <= *index) {
                segments.next();
            }
            let segment = *segments.peek()?;
            if !segment.owner.is_scroll_only() {
                return None;
            }
            let local = segment.range.start + *index - segment.start();
            let PaintOperationRef::DeferredScroll(old) = segment.owner.view(local) else {
                return None;
            };
            if !placement.same_source_placement_domain(old)
                || (!placement.is_row_placement() && placement.visual_bounds() != old.visual_bounds())
                || placement.same_paint(old)
            {
                return None;
            }
            marker_page.push(PaintOperation::DeferredScroll(placement.clone()));
        }
        let marker_page = Arc::new(marker_page);
        let mut result = Self {
            boundaries: self.boundaries.clone(),
            ..Self::default()
        };
        result.segments.make_mut().reserve(self.segments.len().saturating_add(placements.len().saturating_mul(2)));
        let mut next = 0;
        for segment in self.segments.iter() {
            let mut cursor = segment.range.start;
            while let Some((index, _)) = placements.get(next) {
                if *index >= segment.end {
                    break;
                }
                let local = segment.range.start + *index - segment.start();
                if cursor < local {
                    result.append(Arc::clone(&segment.owner), cursor..local);
                }
                result.append(Arc::clone(&marker_page), next..next + 1);
                cursor = local + 1;
                next += 1;
            }
            if cursor < segment.range.end {
                result.append(Arc::clone(&segment.owner), cursor..segment.range.end);
            }
        }
        debug_assert_eq!(next, placements.len());
        debug_assert_eq!(result.len, self.len);
        Some(result)
    }

    #[cfg(test)]
    pub(crate) fn shares_boundary_storage(&self, other: &Self) -> bool {
        self.boundaries.0.as_ref().zip(other.boundaries.0.as_ref())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
    }

    /// A placement-only variant retains exactly the same ordinary storage.
    /// New marker pages must contain only scroll records; otherwise skipping
    /// compaction could pin unrelated primitive payload from an old frame.
    /// Returns whether the actual placements are unchanged as well.
    pub(crate) fn placement_storage_matches(&self, previous: &Self) -> Option<bool> {
        if !self.pending.is_empty() || !previous.pending.is_empty()
            || self.len != previous.len || !self.has_deferred_scroll()
            || !previous.has_deferred_scroll()
        {
            return None;
        }
        let mut left = self.all().runs();
        let mut right = previous.all().runs();
        let mut a = left.next().unwrap_or_default();
        let mut b = right.next().unwrap_or_default();
        let mut same_paint = true;
        while !a.is_empty() && !b.is_empty() {
            let count = a.len().min(b.len());
            let current = a.range(0..count);
            let old = b.range(0..count);
            if !current.same_storage(old) {
                if !current.owner.is_some_and(OperationPage::is_scroll_only)
                    || !old.owner.is_some_and(OperationPage::is_scroll_only)
                {
                    return None;
                }
                for (current, old) in current.iter().zip(old.iter()) {
                    let (PaintOperationRef::DeferredScroll(current), PaintOperationRef::DeferredScroll(old)) =
                        (current, old) else { return None; };
                    if !current.same_source_placement_domain(old) { return None; }
                    same_paint &= current.same_paint(old);
                }
            }
            a = if count == a.len() { left.next().unwrap_or_default() } else { a.range(count..a.len()) };
            b = if count == b.len() { right.next().unwrap_or_default() } else { b.range(count..b.len()) };
        }
        (a.is_empty() && b.is_empty()).then_some(same_paint)
    }

    /// Compact operation indices first, then compact each typed allocation
    /// from the union of operation and draw coverage. The two views receive
    /// the exact same replacement allocation and physical slot mapping.
    pub(super) fn compact_with_draws(
        &mut self,
        shadows: &mut super::PrimitiveStream<super::Shadow>,
        quads: &mut super::PrimitiveStream<super::Quad>,
        mono: &mut super::PrimitiveStream<super::MonochromeSprite>,
        poly: &mut super::PrimitiveStream<super::PolychromeSprite>,
    ) {
        self.compact_pages(true);
        self.compact_draw(shadows);
        self.compact_draw(quads);
        self.compact_draw(mono);
        self.compact_draw(poly);
    }

    fn compact_draw<T: super::primitive_stream::Ordered>(
        &mut self, stream: &mut super::PrimitiveStream<T>,
    )
    where
        OperationPage: super::operation_page::DrawPayload<T>,
    {
        use super::operation_page::DrawPayload;
        use collections::{FxHashMap, FxHashSet};

        // A fresh dense scene has no sparse typed coverage at all.
        // Avoid allocating temporary coverage maps on this hot path.
        if stream.has_full_physical_ranges()
            && self.segments.iter().all(|segment| {
                !<OperationPage as super::operation_page::DrawPayload<T>>::has_sparse_draw_coverage(&segment.owner)
            })
        {
            return;
        }

        // Multiple logical segments may retain the same operation page.
        // Count each physical page once. Fully covered typed payload is
        // represented by a single range rather than walking all operations.
        let compacted = {
            let mut seen = FxHashSet::default();
            let mut coverage = Vec::new();
            for segment in self.segments.iter() {
                if seen.insert(Arc::as_ptr(&segment.owner)) {
                    segment.owner.draw_coverage(&mut coverage);
                }
            }
            super::page_compaction::compact_sparse_pages(
                coverage.iter().map(|(owner, range)| (*owner, range))
                    .chain(stream.physical_ranges()),
            )
        };
        if compacted.is_empty() { return; }
        stream.remap_pages(&compacted);
        let mut replacements = FxHashMap::default();
        for segment in self.segments.make_mut() {
            let identity = Arc::as_ptr(&segment.owner);
            if let Some(replacement) = replacements.get(&identity) {
                segment.owner = Arc::clone(replacement);
                continue;
            }
            let Some(page) = segment.owner.draw_owner()
                .and_then(|owner| compacted.get(&Arc::as_ptr(owner))) else { continue };
            let mut replacement = (*segment.owner).clone();
            replacement.remap_draw(page);
            let replacement = Arc::new(replacement);
            replacements.insert(identity, Arc::clone(&replacement));
            segment.owner = replacement;
        }
    }

    fn compact_pages(&mut self, share_draw_payload: bool) {
        assert!(self.pending.is_empty(), "compact only finalized Paint");
        if self
            .segments
            .iter()
            .all(|segment| segment.range.len() == segment.owner.len())
        {
            return;
        }
        let compacted = super::page_compaction::compact_sparse_pages_with(
            self.segments.iter().map(|segment| (&segment.owner, &segment.range)),
            |target, source, range| {
                if share_draw_payload {
                    target.append_shared_range(source, range);
                } else {
                    target.append_range(source, range);
                }
            },
        );
        if compacted.is_empty() {
            return;
        }
        // Logical offsets and existing descriptor capacity stay intact.
        for segment in self.segments.make_mut() {
            if let Some(page) = compacted.get(&Arc::as_ptr(&segment.owner)) {
                segment.range = page.remap(segment.range.clone());
                segment.owner = Arc::clone(&page.owner);
            }
        }
    }
    /// Prepare a variable-length replacement of independent, finalized operation
    /// ranges. All source coordinates refer to this stream before any edit;
    /// replacement coordinates refer to the independent materialization stream.
    ///
    /// Retain immutable payload pages, not the old Scene. Sparse replay
    /// boundaries are reconstructed by `share` at the new logical offsets.
    /// This prepares operation storage only: the caller must also prepare the
    /// draw streams, input/dispatch ranges and retained records before publishing.
    /// The existing equal-length scroll placement path deliberately keeps its
    /// shared boundary table instead of using this more general transaction.
    #[allow(dead_code)] // Boundary publication must adopt the other Frame records first.
    pub(crate) fn with_spliced_ranges(
        &self,
        replacement: &Self,
        edits: &[(Range<usize>, Range<usize>)],
    ) -> Option<Self> {
        if !self.pending.is_empty() || !replacement.pending.is_empty()
            || !self.boundary_at(self.len).independent()
        {
            return None;
        }

        // Validate the entire batch before sharing any page. In particular,
        // an empty insertion range still needs an independent scope boundary.
        let mut previous_end = 0;
        let mut len = self.len;
        for (old, new) in edits {
            if old.start < previous_end || old.start > old.end || old.end > self.len
                || new.start > new.end || new.end > replacement.len
                || !self.boundary_at(old.start).independent()
                || !self.boundary_at(old.end).independent()
                || !replacement.boundary_at(new.start).independent()
                || !replacement.boundary_at(new.end).independent()
            {
                return None;
            }
            len = len.checked_sub(old.len())?.checked_add(new.len())?;
            previous_end = old.end;
        }

        let mut result = Self::default();
        let mut cursor = 0;
        for (old, new) in edits {
            if old.is_empty() && new.is_empty() {
                continue;
            }
            result.share(self, cursor..old.start);
            result.share(replacement, new.clone());
            cursor = old.end;
        }
        result.share(self, cursor..self.len);
        debug_assert_eq!(result.len, len);
        debug_assert!(result.pending.is_empty());
        debug_assert!(result.boundary_at(result.len).independent());
        Some(result)
    }

    /// Share a finished range without copying operations or retaining its source Scene.
    pub(crate) fn share(&mut self, previous: &Self, range: Range<usize>) {
        assert!(previous.pending.is_empty(), "shared Paint must be finished");
        assert!(range.start <= range.end && range.end <= previous.len);
        if range.is_empty() {
            return;
        }
        self.finish();
        if self.len == 0 && range.start == 0 && range.end == previous.len {
            self.segments.0 = previous.segments.0.clone();
            self.boundaries = previous.boundaries.clone();
            self.len = previous.len;
            return;
        }
        let destination = self.len;
        let first_boundary = previous.boundaries.partition_point(|b| b.after <= range.start);
        for boundary in previous.boundaries[first_boundary..].iter().take_while(|b| b.after <= range.end) {
            // The immutable sparse record already owns the exact marker
            // tag. No operation-page lookup or enum dispatch is needed.
            self.record_boundary(destination + (boundary.after - range.start), boundary.change);
        }
        let first = previous
            .segments
            .partition_point(|segment| segment.end <= range.start);
        for segment in previous.segments[first..]
            .iter()
            .take_while(|segment| segment.start() < range.end)
        {
            let start = range.start.max(segment.start());
            let end = range.end.min(segment.end);
            if start < end {
                let local = segment.range.start + start - segment.start();
                self.append(Arc::clone(&segment.owner), local..local + end - start);
            }
        }
    }
    /// Modify scope payloads without rebuilding the ordinary primitive stream.
    /// Shared pages detach only the affected scope record. The sparse index
    /// identifies precisely which operations need modification.
    pub(crate) fn tighten_capture(&mut self, start: usize, end: usize, tight: Bounds<ScaledPixels>) {
        debug_assert!(start < end && end <= self.len);
        let pending_start = self.len - self.pending.len();
        if start >= pending_start {
            // Most fresh captures are wholly in the current builder. Keep
            // that page mutable and visit only its sparse scope records.
            let first = self.boundaries.partition_point(|boundary| boundary.after <= start);
            for boundary in self.boundaries[first..].iter().take_while(|b| b.after <= end) {
                let index = boundary.after - 1;
                let local = index - pending_start;
                if index == start && boundary.change == ScopeChange::StartBlur {
                    self.pending.set_blur_bounds(local, tight);
                } else if index > start && boundary.change == ScopeChange::StartLayer {
                    self.pending.clip_layer(local, tight);
                }
            }
            return;
        }
        self.finish();
        let boundaries = std::mem::take(&mut self.boundaries);
        let first = boundaries.partition_point(|boundary| boundary.after <= start);
        let mut scopes = boundaries[first..].iter()
            .take_while(|boundary| boundary.after <= end)
            .filter_map(|boundary| {
                let index = boundary.after - 1;
                ((index == start && boundary.change == ScopeChange::StartBlur)
                    || (index > start && boundary.change == ScopeChange::StartLayer))
                    .then_some((index, boundary.change))
            }).peekable();
        if scopes.peek().is_none() {
            drop(scopes);
            self.boundaries = boundaries;
            return;
        }
        let segments = self.segments.take();
        self.len = 0;
        for mut segment in segments {
            let begin = segment.start();
            if scopes.peek().is_none_or(|(index, _)| *index >= segment.end) {
                self.append(segment.owner, segment.range);
                continue;
            }
            if let Some(owner) = Arc::get_mut(&mut segment.owner) {
                while let Some((index, change)) = scopes.next_if(|(index, _)| *index < segment.end) {
                    let local = segment.range.start + index - begin;
                    match change {
                        ScopeChange::StartBlur => owner.set_blur_bounds(local, tight),
                        ScopeChange::StartLayer => owner.clip_layer(local, tight),
                        _ => unreachable!(),
                    }
                }
                self.append(segment.owner, segment.range);
                continue;
            }
            let mut cursor = segment.range.start;
            while let Some((index, change)) = scopes.next_if(|(index, _)| *index < segment.end) {
                let local = segment.range.start + index - begin;
                if cursor < local {
                    self.append(Arc::clone(&segment.owner), cursor..local);
                }
                let mut page = OperationPage::default();
                page.push(segment.owner.view(local).to_operation());
                match change {
                    ScopeChange::StartBlur => page.set_blur_bounds(0, tight),
                    ScopeChange::StartLayer => page.clip_layer(0, tight),
                    _ => unreachable!(),
                }
                self.append(Arc::new(page), 0..1);
                cursor = local + 1;
            }
            if cursor < segment.range.end {
                self.append(segment.owner, cursor..segment.range.end);
            }
        }
        drop(scopes);
        self.boundaries = boundaries;
    }

    /// Test-only generic editor for arbitrary changes of operation kind.
    #[cfg(test)]
    pub(crate) fn edit(&mut self, range: Range<usize>, mut edit: impl FnMut(&mut PaintOperation)) {
        assert!(range.start <= range.end && range.end <= self.len);
        if range.is_empty() { return; }
        self.finish();
        let segments = self.segments.take();
        self.len = 0;
        for segment in segments {
            let start = range.start.max(segment.start());
            let end = range.end.min(segment.end);
            if start >= end {
                self.append(segment.owner, segment.range);
                continue;
            }
            let local = segment.range.start + start - segment.start();
            let local_end = local + end - start;
            if segment.range.start < local {
                self.append(Arc::clone(&segment.owner), segment.range.start..local);
            }
            let mut changed = OperationPage::default();
            for index in local..local_end {
                let mut operation = segment.owner.view(index).to_operation();
                edit(&mut operation);
                changed.push(operation);
            }
            self.append(Arc::new(changed), 0..end - start);
            if local_end < segment.range.end {
                self.append(segment.owner, local_end..segment.range.end);
            }
        }
        self.rebuild_boundaries();
    }
}

impl<'a> OperationRange<'a> {
    pub(crate) fn len(self) -> usize { self.end - self.start }

    pub(crate) fn range(self, range: Range<usize>) -> Self {
        assert!(range.start <= range.end && range.end <= self.len());
        Self { operations: self.operations, start: self.start + range.start, end: self.start + range.end }
    }

    pub(crate) fn view(self, index: usize) -> Option<PaintOperationRef<'a>> {
        if index >= self.len() { return None; }
        self.operations.view(self.start + index)
    }

    pub(crate) fn runs(self) -> impl DoubleEndedIterator<Item = OperationRun<'a>> + Clone {
        let first = self.operations.segments.partition_point(|segment| segment.end <= self.start);
        let end = self.operations.segments.partition_point(|segment| segment.start() < self.end);
        let pending_start = self.operations.len - self.operations.pending.len();
        let pending = if self.end > pending_start && self.start < self.end {
            OperationRun::new(&self.operations.pending,
                self.start.saturating_sub(pending_start)..self.end - pending_start)
        } else {
            OperationRun::default()
        };
        self.operations.segments[first..end].iter().map(move |segment| {
            let start = segment.range.start + self.start.saturating_sub(segment.start());
            let end = segment.range.start + self.end.min(segment.end) - segment.start();
            OperationRun::new(&segment.owner, start..end)
        }).chain(std::iter::once(pending)).filter(|run| !run.is_empty())
    }

    pub(crate) fn views(self) -> impl DoubleEndedIterator<Item = PaintOperationRef<'a>> + Clone {
        self.runs().flat_map(OperationRun::iter)
    }

    pub(crate) fn matches_views(
        self, other: OperationRange<'_>,
        compare: impl FnMut(PaintOperationRef<'_>, PaintOperationRef<'_>) -> bool,
    ) -> bool {
        self.len() == other.len() && self.common_prefix_views(other, compare) == self.len()
    }

    pub(crate) fn common_prefix_views(
        self, other: OperationRange<'_>,
        compare: impl FnMut(PaintOperationRef<'_>, PaintOperationRef<'_>) -> bool,
    ) -> usize {
        common_shared_edge(self.runs(), other.runs(), false, compare)
    }

    pub(crate) fn common_suffix_views(
        self, other: OperationRange<'_>,
        compare: impl FnMut(PaintOperationRef<'_>, PaintOperationRef<'_>) -> bool,
    ) -> usize {
        common_shared_edge(self.runs().rev(), other.runs().rev(), true, compare)
    }
}

fn common_shared_edge<'a, 'b>(
    mut left_runs: impl Iterator<Item = OperationRun<'a>>,
    mut right_runs: impl Iterator<Item = OperationRun<'b>>,
    reverse: bool,
    mut compare: impl FnMut(PaintOperationRef<'_>, PaintOperationRef<'_>) -> bool,
) -> usize {
    let mut left = left_runs.next().unwrap_or_default();
    let mut right = right_runs.next().unwrap_or_default();
    let mut matched = 0;
    while !left.is_empty() && !right.is_empty() {
        let count = left.len().min(right.len());
        let (a, left_tail) = if reverse {
            (left.range(left.len()-count..left.len()), left.range(0..left.len()-count))
        } else {
            (left.range(0..count), left.range(count..left.len()))
        };
        let (b, right_tail) = if reverse {
            (right.range(right.len()-count..right.len()), right.range(0..right.len()-count))
        } else {
            (right.range(0..count), right.range(count..right.len()))
        };
        if a.same_storage(b) {
            matched += count;
        } else if reverse {
            for (a,b) in a.iter().rev().zip(b.iter().rev()) {
                if !compare(a,b) { return matched; }
                matched += 1;
            }
        } else {
            for (a,b) in a.iter().zip(b.iter()) {
                if !compare(a,b) { return matched; }
                matched += 1;
            }
        }
        left = if left_tail.is_empty() { left_runs.next().unwrap_or_default() } else { left_tail };
        right = if right_tail.is_empty() { right_runs.next().unwrap_or_default() } else { right_tail };
    }
    matched
}

impl Extend<PaintOperation> for Operations {
    fn extend<T: IntoIterator<Item = PaintOperation>>(&mut self, operations: T) {
        for operation in operations {
            self.push(operation);
        }
    }
}

#[cfg(test)]
mod boundary_splice_tests {
    use super::*;
    use crate::{Primitive, Quad};

    fn primitives(count: usize) -> Operations {
        let mut stream = Operations::default();
        stream.extend((0..count).map(|_| PaintOperation::Primitive(Primitive::Quad(Quad::default()))));
        stream.finish();
        stream
    }

    #[test]
    fn variable_splice_preserves_physical_pages_and_relocates_suffix() {
        let mut old = primitives(10);
        let mut inserted = primitives(5);
        let next = old.with_spliced_ranges(&inserted, &[
            (0..0, 0..1), (2..5, 1..3), (7..9, 3..3), (10..10, 3..5),
        ]).unwrap();
        let expected = [
            inserted.storage_identity(0),
            old.storage_identity(0), old.storage_identity(1),
            inserted.storage_identity(1), inserted.storage_identity(2),
            old.storage_identity(5), old.storage_identity(6), old.storage_identity(9),
            inserted.storage_identity(3), inserted.storage_identity(4),
        ];
        assert_eq!(next.len(), expected.len());
        old.clear();
        inserted.clear();
        for (index, identity) in expected.into_iter().enumerate() {
            assert_eq!(next.storage_identity(index), identity);
        }
    }

    #[test]
    fn variable_splice_rebases_sparse_scope_boundaries() {
        let mut old = primitives(1);
        old.push(PaintOperation::StartLayer(Bounds::default()));
        old.push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
        old.push(PaintOperation::EndLayer);
        old.push(PaintOperation::StartLayer(Bounds::default()));
        old.push(PaintOperation::EndLayer);
        old.finish();

        let inserted = primitives(2);
        let next = old.with_spliced_ranges(&inserted, &[(1..4, 0..2)]).unwrap();
        assert_eq!(next.len(), 5);
        assert_eq!(next.storage_identity(3), old.storage_identity(4));
        assert_eq!(next.storage_identity(4), old.storage_identity(5));
        assert_eq!(next.boundaries.len(), 2);
        assert_eq!(next.boundaries[0].after, 4);
        assert_eq!(next.boundaries[1].after, 5);
        assert!(!next.boundary_at(4).independent());
        assert!(next.range_is_replayable(3..5));
        assert!(old.with_spliced_ranges(&inserted, &[(2..2, 0..1)]).is_none());
        assert!(old.with_spliced_ranges(&inserted, &[(1..2, 0..0)]).is_none());
        assert!(inserted.with_spliced_ranges(&old, &[(0..0, 2..2)]).is_none());
    }

    #[test]
    fn variable_splice_rejects_invalid_batch_without_source_mutation() {
        let old = primitives(8);
        let replacement = primitives(3);
        let identity = old.storage_identity(4);
        for edits in [
            vec![(1..4, 0..1), (3..5, 1..2)],
            vec![(4..5, 0..1), (1..2, 1..2)],
            vec![(0..9, 0..1)],
            vec![(0..1, 0..4)],
            vec![(3..2, 0..1)],
            vec![(0..1, 2..1)],
        ] {
            assert!(old.with_spliced_ranges(&replacement, &edits).is_none());
            assert_eq!(old.len(), 8);
            assert_eq!(old.storage_identity(4), identity);
        }
        let mut pending = Operations::default();
        pending.push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
        assert!(old.with_spliced_ranges(&pending, &[(0..1, 0..1)]).is_none());
        assert!(pending.with_spliced_ranges(&replacement, &[]).is_none());
        let unchanged = old.with_spliced_ranges(&replacement, &[]).unwrap();
        assert!(unchanged.same_physical_ranges(&old));
        let deleted = old.with_spliced_ranges(&replacement, &[(0..8, 0..0)]).unwrap();
        assert_eq!(deleted.len(), 0);
    }

    #[test]
    fn variable_splice_matches_exhaustive_two_edit_reference() {
        let old = primitives(6);
        let replacement = primitives(3);
        for a in 0..=6 {
            for b in a..=6 {
                for c in b..=6 {
                    for d in c..=6 {
                        for first in 0..=3 {
                            for second in 0..=3 {
                                let next = old.with_spliced_ranges(&replacement, &[
                                    (a..b, 0..first), (c..d, 0..second),
                                ]).unwrap();
                                let expected = (0..a).map(|i| old.storage_identity(i))
                                    .chain((0..first).map(|i| replacement.storage_identity(i)))
                                    .chain((b..c).map(|i| old.storage_identity(i)))
                                    .chain((0..second).map(|i| replacement.storage_identity(i)))
                                    .chain((d..6).map(|i| old.storage_identity(i)));
                                assert!(expected.eq((0..next.len()).map(|i| next.storage_identity(i))));
                            }
                        }
                    }
                }
            }
        }
    }
}
