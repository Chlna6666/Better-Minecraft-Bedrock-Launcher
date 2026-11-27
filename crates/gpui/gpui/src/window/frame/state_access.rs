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

    pub(in crate::window) fn key(&self, index: usize) -> Option<&Key> {
        assert!(index < self.len);
        let pending_start = self.len - self.pending.len();
        if index >= pending_start { return self.pending[index - pending_start].as_ref(); }
        let block = &self.blocks[self.blocks.partition_point(|block| block.end <= index)];
        block.page.as_ref()?.get(block.start + index - block.begin())?.as_ref()
    }

    pub(in crate::window) fn push(&mut self, key: Option<Key>) {
        self.pending.push(key);
        self.len += 1;
        if self.pending.len() == PAGE_LEN { self.freeze(); }
    }

    pub(in crate::window) fn freeze(&mut self) {
        if self.pending.is_empty() { return; }
        let mut pending = mem::take(&mut self.pending);
        // A short tail should not pin a full page capacity.
        if pending.capacity() > pending.len().saturating_mul(2) {
            pending.shrink_to_fit();
        }
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
        self.freeze();
        for block in source.blocks_in(range.clone()) {
            self.append(block.slice(range.start.max(block.begin())..range.end.min(block.end)));
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
        self.blocks.clear();
        self.pending.clear();
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
