//! Ported from longbridge/gpui-fast fast/layout_bounds.rs at 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
//! Copyright 2022-2025 Zed Industries, Inc. Copyright 2026 Longbridge.
//! SPDX-License-Identifier: Apache-2.0

//! The tables [`TaffyLayoutEngine::layout_bounds`] caches absolute bounds in,
//! indexed by the node's Taffy slot instead of hashed.
//!
//! Every element asks for its bounds in prepaint, thousands of them a frame,
//! and each first query of a node looks its parent up, then stores the node's
//! origin and bounds. Upstream keeps both in `FxHashMap<LayoutId, _>`s and
//! clears them at the end of every frame. [`LayoutIdMap`] answers the same
//! calls from a `Vec` indexed by the slot half of the node id, and forgets
//! everything at the end of a frame by starting a new pass instead of
//! clearing. What `layout_bounds` computes, pixel snapping included, is left
//! exactly as upstream wrote it.
//!
//! [`TaffyLayoutEngine::layout_bounds`]: crate::layout::TaffyLayoutEngine::layout_bounds

use crate::LayoutId;

/// One slot of a [`LayoutIdMap`].
#[derive(Clone, Copy, Default)]
struct Entry<V> {
    /// The whole node id, slot and version, since Taffy gives a removed node's
    /// slot to the next node it creates.
    node: u64,
    /// The pass the value was stored in. It stands only in the current one.
    pass: u32,
    value: V,
}

/// A map from layout node to `V`, for the few calls `layout_bounds` and
/// `compute_layout` make of their caches: `get`, `insert`, `remove` and
/// `clear`, as on the `FxHashMap` it replaces.
pub(crate) struct LayoutIdMap<V> {
    /// Indexed by the slot of the node's id. Slots are dense, as Taffy reuses
    /// those of removed nodes.
    entries: Vec<Entry<V>>,
    /// Entries of any other pass are absent. Never 0, the pass of an entry that
    /// was never stored or has been removed.
    pass: u32,
    // BMCBL's native mutation observer can skip work when no bounds were read.
    live: usize,
}

impl<V> Default for LayoutIdMap<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            pass: 1,
            live: 0,
        }
    }
}

/// The id Taffy gave the node: a slotmap key's `as_ffi` value, the version in
/// the high half and the slot in the low.
#[inline]
fn raw(id: &LayoutId) -> u64 {
    u64::from(taffy::NodeId::from(*id))
}

#[inline]
fn slot(raw: u64) -> usize {
    raw as u32 as usize
}

impl<V: Copy + Default> LayoutIdMap<V> {
    #[inline]
    pub(crate) fn get(&self, id: &LayoutId) -> Option<&V> {
        let raw = raw(id);
        self.entries
            .get(slot(raw))
            .filter(|entry| entry.pass == self.pass && entry.node == raw)
            .map(|entry| &entry.value)
    }

    #[inline]
    pub(crate) fn insert(&mut self, id: LayoutId, value: V) {
        let raw = raw(&id);
        let slot = slot(raw);
        if slot >= self.entries.len() {
            self.entries.resize(slot + 1, Entry::default());
        }
        if self.entries[slot].pass != self.pass {
            self.live += 1;
        }
        self.entries[slot] = Entry {
            node: raw,
            pass: self.pass,
            value,
        };
    }

    #[inline]
    pub(crate) fn remove(&mut self, id: &LayoutId) -> Option<V> {
        let raw = raw(id);
        if let Some(entry) = self.entries.get_mut(slot(raw))
            && entry.node == raw && entry.pass == self.pass
        {
            entry.pass = 0;
            self.live -= 1;
            return Some(entry.value);
        }
        None
    }

    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.live == 0
    }

    // Slot indices cannot be compacted while a pass is live. Explicit memory
    // pressure can discard an empty table without changing any node identity.
    pub(crate) fn trim_empty(&mut self, floor: usize) {
        if self.is_empty() && self.entries.capacity() > floor.saturating_mul(4) {
            self.entries.clear();
            self.entries.shrink_to(floor);
        }
    }

    #[cfg(test)]
    pub(crate) fn reserve(&mut self, additional: usize) {
        self.entries.reserve(additional);
    }

    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.entries.capacity()
    }

    /// Forgets every entry, at the end of a frame, without touching them.
    pub(crate) fn clear(&mut self) {
        self.live = 0;
        self.pass = self.pass.wrapping_add(1);
        if self.pass == 0 {
            // Four billion frames on, entries stored in pass 1 would be back.
            self.entries.fill(Entry::default());
            self.pass = 1;
        }
    }
}

