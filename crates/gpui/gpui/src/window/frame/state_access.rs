//! Immutable, UI-thread-owned element-state access pages. State values
//! remain in the existing Frame map; only ownership records are shared.
use crate::GlobalElementId;
use std::{any::TypeId, mem, ops::Range, rc::Rc};

type Key = (GlobalElementId, TypeId);
type Page = Rc<Vec<Option<Key>>>;

// Bound the amount of unrelated ownership retained by a shared subrange.
const PAGE_LEN: usize = 64;

#[derive(Clone)]
struct Block {
    page: Option<Page>,
    start: usize,
    len: usize,
    end: usize,
}

impl Block {
    fn begin(&self) -> usize { self.end - self.len }
    fn slice(&self, range: Range<usize>) -> Self {
        debug_assert!(range.start >= self.begin() && range.end <= self.end);
        Self { page: self.page.clone(), start: self.start + range.start - self.begin(),
            len: range.len(), end: range.end }
    }
    fn keys(&self, range: Range<usize>) -> &[Option<Key>] {
        let Some(page) = &self.page else { return &[]; };
        let start = self.start + range.start.max(self.begin()) - self.begin();
        let end = self.start + range.end.min(self.end) - self.begin();
        &page[start..end]
    }
}

#[derive(Default)]
pub(in crate::window) struct StateAccessLog {
    blocks: Vec<Block>,
    pending: Vec<Option<Key>>,
    len: usize,
}

impl StateAccessLog {
    pub(in crate::window) fn len(&self) -> usize { self.len }

    pub(in crate::window) fn contains(&self, range: Range<usize>) -> bool {
        range.start <= range.end && range.end <= self.len
    }

    fn blocks_in(&self, range: Range<usize>) -> &[Block] {
        let start = self.blocks.partition_point(|block| block.end <= range.start);
        let end = self.blocks.partition_point(|block| block.begin() < range.end);
        &self.blocks[start..end.max(start)]
    }

    pub(in crate::window) fn keys(&self, range: Range<usize>) -> impl Iterator<Item = &Key> {
        assert!(self.contains(range.clone()));
        let pending_start = self.len - self.pending.len();
        let tail = &self.pending[range.start.saturating_sub(pending_start)
            ..range.end.saturating_sub(pending_start)];
        self.blocks_in(range.clone()).iter()
            .flat_map(move |block| block.keys(range.clone()))
            .chain(tail.iter()).flatten()
    }

    /// Search the clean complement of ordered, disjoint excluded spans.
    /// Traverse contiguous key slices rather than resolving every logical
    /// index through the page table (and searching gaps) independently.
    pub(in crate::window) fn any_keys_outside(
        &self, range: Range<usize>, excluded: &[Range<usize>],
        mut predicate: impl FnMut(&Key) -> bool,
    ) -> bool {
        assert!(self.contains(range.clone()));
        if range.is_empty() { return false; }
        let mut cursor = range.start;
        let first = excluded.partition_point(|gap| gap.end <= cursor);
        for gap in &excluded[first..] {
            if gap.start >= range.end { break; }
            let clean_end = gap.start.min(range.end);
            if cursor < clean_end && self.keys(cursor..clean_end).any(&mut predicate) {
                return true;
            }
            cursor = cursor.max(gap.end);
            if cursor >= range.end { return false; }
        }
        cursor < range.end && self.keys(cursor..range.end).any(predicate)
    }

    pub(in crate::window) fn key(&self, index: usize) -> Option<&Key> {
        assert!(index < self.len);
        let pending_start = self.len - self.pending.len();
        if index >= pending_start { return self.pending[index - pending_start].as_ref(); }
        let block = &self.blocks[self.blocks.partition_point(|block| block.end <= index)];
        block.page.as_ref()?.get(block.start + index - block.begin())?.as_ref()
    }

    pub(in crate::window) fn push(&mut self, key: Option<Key>) {
        self.pack_sparse_tail();
        self.push_pending(key);
    }

    #[inline]
    fn push_pending(&mut self, key: Option<Key>) {
        self.pending.push(key);
        self.len += 1;
        if self.pending.len() == PAGE_LEN { self.seal_pending(); }
    }

    fn pack_sparse_tail(&mut self) {
        if !self.pending.is_empty() { return; }
        let Some(last) = self.blocks.last() else { return; };
        let Some(page) = &last.page else { return; };
        if last.len * 4 >= page.len() { return; }
        let last = self.blocks.pop().unwrap();
        let page = last.page.unwrap();
        self.pending.extend_from_slice(&page[last.start..last.start + last.len]);
        // Existing logical entries: do not advance len.
    }

    pub(in crate::window) fn freeze(&mut self) {
        self.pack_sparse_tail();
        self.seal_pending();
    }

    fn seal_pending(&mut self) {
        if self.pending.is_empty() { return; }
        // A bounded page is cheaper than a shrink/reallocate on every short
        // fragment. Capacity is at most PAGE_LEN on the normal build path.
        let pending = mem::take(&mut self.pending);
        let len = pending.len();
        self.blocks.push(Block { page: Some(Rc::new(pending)), start: 0, len, end: self.len });
    }

    fn append(&mut self, mut block: Block) {
        self.len += block.len;
        block.end = self.len;
        if let Some(last) = self.blocks.last_mut() {
            let contiguous = match (&last.page, &block.page) {
                (Some(a), Some(b)) => Rc::ptr_eq(a, b) && last.start + last.len == block.start,
                (None, None) => true,
                _ => false,
            };
            if contiguous {
                last.len += block.len;
                last.end = block.end;
                return;
            }
        }
        self.blocks.push(block);
    }

    pub(in crate::window) fn share(&mut self, source: &Self, range: Range<usize>) {
        assert!(source.contains(range.clone()));
        if range.is_empty() { return; }
        for block in source.blocks_in(range.clone()) {
            let block = block.slice(range.start.max(block.begin())..range.end.min(block.end));
            // A tiny fragment must not retain an almost entirely unused old
            // page. Pack it together with fresh accesses instead. Large or
            // contiguous fragments keep the zero-key-copy sharing path.
            let adjacent = self.pending.is_empty() && self.blocks.last().is_some_and(|last|
                match (&last.page, &block.page) {
                    (Some(a), Some(b)) => Rc::ptr_eq(a, b) && last.start + last.len == block.start,
                    (None, None) => true,
                    _ => false,
                });
            if adjacent {
                self.append(block);
                continue;
            }
            self.pack_sparse_tail();
            if let Some(page) = &block.page
                && !self.pending.is_empty() && block.len * 4 < page.len()
            {
                for key in &page[block.start..block.start + block.len] {
                    self.push_pending(key.clone());
                }
            } else {
                self.seal_pending();
                self.append(block);
            }
        }
        let pending_start = source.len - source.pending.len();
        for key in &source.pending[range.start.saturating_sub(pending_start)
            ..range.end.saturating_sub(pending_start)] {
            self.push(key.clone());
        }
    }

    pub(in crate::window) fn cancel(&mut self, range: Range<usize>) {
        assert!(self.contains(range.clone()));
        if range.is_empty() { return; }
        let pending_start = self.len - self.pending.len();
        if range.start >= pending_start {
            self.pending[range.start - pending_start..range.end - pending_start].fill(None);
            return;
        }
        self.freeze();
        let first = self.blocks.partition_point(|block| block.end <= range.start);
        let end = self.blocks.partition_point(|block| block.begin() < range.end);
        let mut replacement = smallvec::SmallVec::<[Block; 3]>::new();
        let left = &self.blocks[first];
        if left.begin() < range.start { replacement.push(left.slice(left.begin()..range.start)); }
        replacement.push(Block { page: None, start: 0, len: range.len(), end: range.end });
        let right = &self.blocks[end - 1];
        if range.end < right.end { replacement.push(right.slice(range.end..right.end)); }
        self.blocks.splice(first..end, replacement);
    }

    pub(in crate::window) fn move_to_end(&mut self, range: Range<usize>) {
        assert!(self.contains(range.clone()));
        if range.is_empty() { return; }
        self.freeze();
        let blocks: smallvec::SmallVec<[Block; 4]> = self.blocks_in(range.clone()).iter()
            .map(|block| block.slice(range.start.max(block.begin())..range.end.min(block.end)))
            .collect();
        self.cancel(range);
        for block in blocks { self.append(block); }
    }

    pub(in crate::window) fn truncate(&mut self, len: usize) {
        if len >= self.len { return; }
        let pending_start = self.len - self.pending.len();
        if len >= pending_start {
            self.pending.truncate(len - pending_start);
        } else {
            self.pending.clear();
            self.blocks.truncate(self.blocks.partition_point(|block| block.begin() < len));
            if let Some(last) = self.blocks.last_mut() {
                if last.end > len {
                    last.len -= last.end - len;
                    last.end = len;
                }
            }
        }
        self.len = len;
    }

    pub(in crate::window) fn clear(&mut self) {
        self.pending.clear();
        for block in self.blocks.drain(..) {
            if let Some(page) = block.page
                && let Ok(mut keys) = Rc::try_unwrap(page)
                && keys.capacity() > self.pending.capacity()
            {
                keys.clear();
                self.pending = keys;
            }
        }
        self.len = 0;
    }

    // Capacity diagnostics are in backing-storage slots, not logical records.
    // Shared payloads are deliberately not counted twice here.
    pub(in crate::window) fn capacity(&self) -> usize {
        self.blocks.capacity() + self.pending.capacity()
    }

    pub(in crate::window) fn trim(&mut self, force: bool) {
        if force || self.blocks.capacity() > self.blocks.len().max(4) * 4 {
            self.blocks.shrink_to(self.blocks.len().max(if force { 0 } else { 4 }));
        }
        if force { self.pending.shrink_to_fit(); }
    }

    pub(in crate::window) fn trim_for_reuse(&mut self, current: &Self) {
        let target = current.blocks.len().max(4);
        if self.blocks.capacity() > target * 4 { self.blocks.shrink_to(target); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(i: usize) -> Key {
        (GlobalElementId::from_path(&[crate::ElementId::Integer(i as u64)]), TypeId::of::<u64>())
    }

    #[test]
    fn replay_shares_bounded_pages_and_cancel_does_not_edit_source() {
        let mut source = StateAccessLog::default();
        for i in 0..130 { source.push(Some(key(i))); }
        source.freeze();
        let mut candidate = StateAccessLog::default();
        candidate.share(&source, 10..120);
        assert!(Rc::ptr_eq(source.blocks[0].page.as_ref().unwrap(), candidate.blocks[0].page.as_ref().unwrap()));
        assert_eq!(candidate.keys(0..110).cloned().collect::<Vec<_>>(), (10..120).map(key).collect::<Vec<_>>());
        candidate.cancel(20..90);
        assert_eq!(candidate.len(), 110);
        assert!(candidate.key(20).is_none());
        assert_eq!(candidate.key(90), Some(&key(100)));
        assert_eq!(source.key(30), Some(&key(30)));
        candidate.move_to_end(10..30);
        assert_eq!(candidate.len(), 130);
        assert!(candidate.key(10).is_none());
        assert_eq!(candidate.key(110), Some(&key(20)));
        assert!(candidate.key(129).is_none());
        candidate.truncate(115);
        assert_eq!(candidate.keys(110..115).cloned().collect::<Vec<_>>(), (20..25).map(key).collect::<Vec<_>>());
        source.clear();
        assert_eq!(candidate.key(114), Some(&key(24)));
    }

    #[test]
    fn clean_complement_search_matches_per_index_reference() {
        let mut source = StateAccessLog::default();
        for i in 0..150 { source.push(Some(key(i % 37))); }
        source.cancel(20..25);
        source.cancel(65..90);
        let excluded = [0..8, 10..22, 40..65, 90..100, 128..150];
        for start in [0, 1, 8, 20, 63, 64, 90, 100, 149, 150] {
            for end in [start, (start+1).min(150), 150] {
                for needle in 0..37 {
                    let expected = (start..end).any(|index| {
                        !excluded.iter().any(|gap| gap.contains(&index))
                            && source.key(index) == Some(&key(needle))
                    });
                    assert_eq!(source.any_keys_outside(start..end, &excluded, |value| *value == key(needle)), expected);
                }
            }
        }
    }

    #[test]
    fn adjacent_single_access_replays_share_one_page() {
        let mut source = StateAccessLog::default();
        for i in 0..PAGE_LEN { source.push(Some(key(i))); }
        source.freeze();
        let mut target = StateAccessLog::default();
        for i in 0..PAGE_LEN {
            target.share(&source, i..i+1);
            assert_eq!(target.len(), i+1);
        }
        target.freeze();
        assert_eq!(target.blocks.len(), 1);
        assert!(Rc::ptr_eq(source.blocks[0].page.as_ref().unwrap(), target.blocks[0].page.as_ref().unwrap()));
        assert_eq!(target.keys(0..PAGE_LEN).cloned().collect::<Vec<_>>(), (0..PAGE_LEN).map(key).collect::<Vec<_>>());
        target.cancel(3..5);
        assert_eq!(source.key(3), Some(&key(3)));
        assert!(target.key(3).is_none());
        target.move_to_end(0..6);
        assert_eq!(target.key(PAGE_LEN), Some(&key(0)));
        assert!(target.key(PAGE_LEN+3).is_none());
    }

    #[test]
    fn sparse_replay_packs_small_fragments_instead_of_pinning_old_pages() {
        let mut source = StateAccessLog::default();
        for i in 0..PAGE_LEN * 8 { source.push(Some(key(i))); }
        source.freeze();
        let mut target = StateAccessLog::default();
        for i in 0..8 {
            target.share(&source, i * PAGE_LEN..i * PAGE_LEN + 1);
        }
        target.freeze();
        assert_eq!(target.blocks.len(), 1);
        assert_eq!(target.blocks[0].page.as_ref().unwrap().len(), 8);
        assert!(source.blocks.iter().all(|block| Rc::strong_count(block.page.as_ref().unwrap()) == 1));
        assert_eq!(target.keys(0..8).cloned().collect::<Vec<_>>(),
            (0..8).map(|i| key(i * PAGE_LEN)).collect::<Vec<_>>());
    }

    #[test]
    fn pending_tail_and_empty_ranges_keep_exact_offsets() {
        let mut source = StateAccessLog::default();
        for i in 0..70 { source.push(Some(key(i))); }
        let mut target = StateAccessLog::default();
        target.share(&source, 62..70);
        target.cancel(4..6);
        assert_eq!(target.key(0), Some(&key(62)));
        assert!(target.key(4).is_none());
        assert_eq!(target.key(7), Some(&key(69)));
        assert_eq!(target.keys(1..1).count(), 0);
        target.truncate(1);
        target.push(Some(key(100)));
        assert_eq!(target.keys(0..2).cloned().collect::<Vec<_>>(), vec![key(62), key(100)]);
    }
}
