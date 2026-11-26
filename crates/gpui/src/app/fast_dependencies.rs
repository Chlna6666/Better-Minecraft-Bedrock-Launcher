//! Shared dependency lists from longbridge/gpui-fast, fast/dependencies.rs.
//! Upstream revision: 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
//! Copyright 2022-2025 Zed Industries, Inc. Copyright 2026 Longbridge.
//! SPDX-License-Identifier: Apache-2.0

use collections::FxHashSet;
use std::rc::Rc;

/// Lists of what retained subtrees read, each kept once and shared by every
/// subtree that read the same: a view built again reads what it read last
/// frame, a recording's own reads are often all of its reads, and many rows
/// read the same few entities. A recording finished with a list already held
/// takes it without allocating.
///
/// Lists nothing else holds any more are dropped once the set has doubled
/// since it was last swept, so it stays about as large as what is in use.
pub(crate) struct Interned<T> {
    lists: FxHashSet<Rc<[T]>>,
    sweep_at: usize,
    empty: Rc<[T]>,
}

impl<T> Default for Interned<T> {
    fn default() -> Self {
        Self {
            lists: FxHashSet::default(),
            sweep_at: 1024,
            empty: Rc::from(Vec::new()),
        }
    }
}

impl<T: Eq + std::hash::Hash + Copy> Interned<T> {
    /// The shared list holding `items`, which are sorted without repeats.
    pub(crate) fn get(&mut self, items: &[T]) -> Rc<[T]> {
        if items.is_empty() {
            return self.empty.clone();
        }
        if let Some(list) = self.lists.get(items) {
            return list.clone();
        }
        if self.lists.len() >= self.sweep_at {
            self.lists.retain(|list| Rc::strong_count(list) > 1);
            self.sweep_at = (self.lists.len() * 2).max(1024);
        }
        let list: Rc<[T]> = Rc::from(items);
        self.lists.insert(list.clone());
        list
    }
}

/// Sorts `items` and removes repeats.
pub(crate) fn sort_unique<T: Ord>(items: &mut Vec<T>) {
    items.sort_unstable();
    items.dedup();
}

/// The union of two sorted lists without repeats, itself sorted and without
/// repeats. When one holds all of the other, which is the usual case — a
/// view's paint reads what its prepaint read — it is shared, not copied.
pub(crate) fn merge_sorted<T: Ord + Copy>(a: &Rc<[T]>, b: &Rc<[T]>) -> Rc<[T]> {
    fn contains_all<T: Ord>(all: &[T], some: &[T]) -> bool {
        let mut rest = all;
        some.iter().all(|item| match rest.binary_search(item) {
            Ok(index) => {
                rest = &rest[index + 1..];
                true
            }
            Err(_) => false,
        })
    }
    if b.is_empty() || Rc::ptr_eq(a, b) || contains_all(a, b) {
        return a.clone();
    }
    if a.is_empty() || contains_all(b, a) {
        return b.clone();
    }
    let mut merged = Vec::with_capacity(a.len() + b.len());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => {
                merged.push(a[i]);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                merged.push(b[j]);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                merged.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    merged.extend_from_slice(&a[i..]);
    merged.extend_from_slice(&b[j..]);
    merged.into()
}

