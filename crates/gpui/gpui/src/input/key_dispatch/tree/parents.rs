//! Relative parent edges survive relocation of an entire retained segment.
//! Share certified immutable pages; only boundary edges require remapping.

use super::{DispatchNodeId, NO_PARENT};
use std::{ops::Range, rc::Rc};

const PAGE_LEN: usize = 512;

fn encode(index: usize, parent: Option<DispatchNodeId>) -> u32 {
    parent.map_or(NO_PARENT, |parent| {
        assert!(parent.0 < index, "dispatch parent must precede child");
        u32::try_from(index - parent.0).expect("dispatch parent distance overflow")
    })
}

fn decode(index: usize, distance: u32) -> Option<DispatchNodeId> {
    (distance != NO_PARENT).then(|| DispatchNodeId(index - distance as usize))
}

struct Page {
    edges: Vec<u32>,
    // Earliest parent relative to the physical page's first child.
    // A root/no-parent edge deliberately prevents wholesale certification.
    min_parent: i64,
}

impl Page {
    fn new(edges: Vec<u32>) -> Self {
        let min_parent = edges.iter().enumerate().map(|(index, &distance)| {
            if distance == NO_PARENT {
                i64::MIN / 2
            } else {
                index as i64 - i64::from(distance)
            }
        }).min().expect("nonempty parent page");
        Self { edges, min_parent }
    }
}

struct Span {
    page: Rc<Page>,
    start: usize,
    len: usize,
    end: usize,
}

impl Span {
    fn begin(&self) -> usize {
        self.end - self.len
    }

    fn slice(&self, range: &Range<usize>) -> (usize, &[u32]) {
        let begin = self.begin().max(range.start);
        let end = self.end.min(range.end);
        let start = self.start + begin - self.begin();
        (begin, &self.page.edges[start..start + end - begin])
    }
}

#[derive(Default)]
pub(super) struct ParentStore {
    spans: Vec<Span>,
    pending: Vec<u32>,
    len: usize,
}

impl ParentStore {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn capacity(&self) -> usize {
        self.spans.capacity() + self.pending.capacity()
    }

    pub(super) fn push(&mut self, parent: Option<DispatchNodeId>) {
        self.push_encoded(encode(self.len, parent));
    }

    fn push_encoded(&mut self, distance: u32) {
        self.pending.push(distance);
        self.len += 1;
        if self.pending.len() == PAGE_LEN {
            self.freeze();
        }
    }

    fn append_encoded(&mut self, mut edges: &[u32]) {
        while !edges.is_empty() {
            let count = (PAGE_LEN - self.pending.len()).min(edges.len());
            self.pending.extend_from_slice(&edges[..count]);
            self.len += count;
            edges = &edges[count..];
            if self.pending.len() == PAGE_LEN {
                self.freeze();
            }
        }
    }

    pub(super) fn freeze(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let edges = std::mem::take(&mut self.pending);
        let len = edges.len();
        self.spans.push(Span { page: Rc::new(Page::new(edges)), start: 0, len, end: self.len });
    }

    fn spans_in(&self, range: &Range<usize>) -> &[Span] {
        let start = self.spans.partition_point(|span| span.end <= range.start);
        let end = self.spans.partition_point(|span| span.begin() < range.end);
        &self.spans[start..end.max(start)]
    }

    pub(super) fn parent(&self, index: usize) -> Option<DispatchNodeId> {
        if index >= self.len {
            return None;
        }
        let pending_start = self.len - self.pending.len();
        let distance = if index >= pending_start {
            self.pending[index - pending_start]
        } else {
            let span = &self.spans[self.spans.partition_point(|span| span.end <= index)];
            span.page.edges[span.start + index - span.begin()]
        };
        decode(index, distance)
    }

    pub(super) fn iter(&self, range: Range<usize>) -> impl Iterator<Item = Option<DispatchNodeId>> + '_ {
        assert!(range.start <= range.end && range.end <= self.len);
        let pending_start = self.len - self.pending.len();
        let tail_start = range.start.max(pending_start);
        let tail = &self.pending[range.start.saturating_sub(pending_start)..range.end.saturating_sub(pending_start)];
        self.spans_in(&range).iter()
            .map(move |span| span.slice(&range))
            .chain(std::iter::once((tail_start, tail)))
            .flat_map(|(start, edges)| edges.iter().enumerate().map(move |(offset, &distance)| decode(start + offset, distance)))
    }

    fn share(&mut self, span: &Span, start: usize, edges: &[u32]) {
        if self.pending.is_empty()
            && let Some(last) = self.spans.last_mut()
            && Rc::ptr_eq(&last.page, &span.page)
            && last.start + last.len == start
        {
            self.len += edges.len();
            last.len += edges.len();
            last.end = self.len;
            return;
        }
        // Small fragments must not pin large physical allocations.
        if edges.len() * 4 < span.page.edges.len() {
            self.append_encoded(edges);
            return;
        }
        self.freeze();
        self.len += edges.len();
        self.spans.push(Span {
            page: span.page.clone(), start, len: edges.len(), end: self.len,
        });
    }

    fn replay_slice(
        &mut self,
        begin: usize,
        edges: &[u32],
        range: &Range<usize>,
        external_parent: &mut impl FnMut(Option<DispatchNodeId>) -> Option<DispatchNodeId>,
    ) {
        for (offset, &distance) in edges.iter().enumerate() {
            let parent = decode(begin + offset, distance);
            if parent.is_some_and(|parent| range.contains(&parent.0)) {
                self.push_encoded(distance);
            } else {
                self.push(external_parent(parent));
            }
        }
    }

    /// Rebase a retained range without walking internal edges on certified pages.
    pub(super) fn replay(
        &mut self,
        source: &Self,
        range: Range<usize>,
        mut external_parent: impl FnMut(Option<DispatchNodeId>) -> Option<DispatchNodeId>,
    ) {
        assert!(range.start <= range.end && range.end <= source.len);
        if range.is_empty() {
            return;
        }
        for span in source.spans_in(&range) {
            let (begin, edges) = span.slice(&range);
            let earliest = span.begin() as i64 - span.start as i64 + span.page.min_parent;
            if earliest >= range.start as i64 {
                // Every edge in the physical page stays inside the retained
                // range. Distances remain valid after a logical relocation.
                self.share(span, span.start + begin - span.begin(), edges);
            } else {
                self.replay_slice(begin, edges, &range, &mut external_parent);
            }
        }
        let pending_start = source.len - source.pending.len();
        let tail = &source.pending[range.start.saturating_sub(pending_start)..range.end.saturating_sub(pending_start)];
        self.replay_slice(range.start.max(pending_start), tail, &range, &mut external_parent);
    }

    pub(super) fn truncate(&mut self, len: usize) {
        assert!(len <= self.len);
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

    pub(super) fn clear(&mut self) {
        self.pending.clear();
        for span in self.spans.drain(..) {
            if let Ok(mut page) = Rc::try_unwrap(span.page)
                && page.edges.capacity() > self.pending.capacity()
            {
                page.edges.clear();
                self.pending = page.edges;
            }
        }
        self.len = 0;
    }

    pub(super) fn trim_for_reuse(&mut self, current: &Self) {
        let target = current.spans.len().max(4);
        if self.spans.capacity() > target.saturating_mul(4) {
            self.spans.shrink_to(target);
        }
    }

    pub(super) fn trim(&mut self, aggressive: bool) {
        self.spans.shrink_to(self.spans.len().max(if aggressive { 0 } else { 4 }));
        if aggressive {
            self.pending.shrink_to_fit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relocated_internal_pages_share_and_external_edges_remap() {
        let mut source = ParentStore::default();
        source.push(None);
        for _ in 1..PAGE_LEN * 3 {
            source.push(Some(DispatchNodeId(0)));
        }
        source.freeze();

        let mut target = ParentStore::default();
        target.push(None);
        target.push(Some(DispatchNodeId(0)));
        let mut external = 0;
        target.replay(&source, 0..source.len(), |parent| {
            assert!(parent.is_none());
            external += 1;
            Some(DispatchNodeId(1))
        });
        assert_eq!(external, 1);
        assert_eq!(target.parent(2), Some(DispatchNodeId(1)));
        assert_eq!(target.parent(PAGE_LEN * 2 + 1), Some(DispatchNodeId(2)));
        assert!(target.spans.iter().any(|span| Rc::ptr_eq(&span.page, &source.spans[1].page)));
        assert_eq!(target.iter(3..target.len()).collect::<Vec<_>>(),
            vec![Some(DispatchNodeId(2)); source.len() - 1]);

        target.truncate(PAGE_LEN + 17);
        target.push(Some(DispatchNodeId(0)));
        assert_eq!(target.parent(PAGE_LEN + 17), Some(DispatchNodeId(0)));
        source.clear();
        assert_eq!(target.parent(PAGE_LEN + 16), Some(DispatchNodeId(2)));
    }

    #[test]
    fn partial_ranges_resolve_all_external_edges_after_repeated_relocation() {
        let mut source = ParentStore::default();
        source.push(None);
        for index in 1..PAGE_LEN * 3 + 7 {
            source.push(Some(DispatchNodeId(if index % 17 == 0 { 0 } else { index - 1 })));
        }
        let mut first = ParentStore::default();
        first.push(None);
        let range = 91..source.len() - 3;
        let expected: Vec<_> = source.iter(range.clone()).map(|parent| {
            Some(DispatchNodeId(parent.filter(|parent| range.contains(&parent.0))
                .map_or(0, |parent| 1 + parent.0 - range.start)))
        }).collect();
        first.replay(&source, range, |_| Some(DispatchNodeId(0)));
        assert_eq!(first.iter(1..first.len()).collect::<Vec<_>>(), expected);
        first.freeze();
        let mut second = ParentStore::default();
        second.replay(&first, 0..first.len(), |parent| parent);
        assert_eq!(first.iter(0..first.len()).collect::<Vec<_>>(),
            second.iter(0..second.len()).collect::<Vec<_>>());
    }
}
