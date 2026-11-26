//! Compact listener storage shared by elements and dispatch nodes.
use std::{mem, ops::Deref, rc::Rc};

// LazyVec implementation from longbridge/gpui-fast at 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
// Copyright 2022 - 2025 Zed Industries, Inc.
// Copyright 2026 Longbridge.
// SPDX-License-Identifier: Apache-2.0
/// A list one pointer wide that allocates nothing while it is empty.
///
/// An [`crate::Interactivity`] holds a score of listener lists, nearly all of them
/// empty on any one element, and moves with its element through every call
/// of its builder. As `Vec`s they were 500 of its bytes, copied every time.
///
// A boxed `Vec` is one pointer where a `Vec` is three, which is the point;
// the second allocation is paid only by a list that has something in it.
#[allow(clippy::box_collection)]
pub(crate) struct LazyVec<T>(Option<Box<Vec<T>>>);

impl<T> Default for LazyVec<T> {
    fn default() -> Self {
        LazyVec(None)
    }
}

impl<T: Clone> Clone for LazyVec<T> {
    fn clone(&self) -> Self {
        LazyVec(self.0.clone())
    }
}

impl<T> LazyVec<T> {
    pub(crate) fn push(&mut self, item: T) {
        self.0.get_or_insert_with(Default::default).push(item);
    }

    /// Takes every item out, leaving the list empty.
    pub(crate) fn drain(&mut self, _: std::ops::RangeFull) -> std::vec::IntoIter<T> {
        mem::take(self).into_iter()
    }
}

impl<T> Deref for LazyVec<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}

impl<T> IntoIterator for LazyVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.map(|items| *items).unwrap_or_default().into_iter()
    }
}

impl<'a, T> IntoIterator for &'a LazyVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// One-pointer listener storage for committed dispatch nodes.
///
/// Adapt Fast's lazy listener representation to transactional frame replay:
/// a candidate frame and its committed source share the immutable payload
/// instead of moving listeners out of the source or cloning every callback.
/// Mutation uses copy-on-write; event dispatch takes an owned snapshot before
/// invoking callbacks that can reenter the window.
pub(crate) struct SharedListeners<T>(Option<Rc<Vec<T>>>);

impl<T> Default for SharedListeners<T> {
    fn default() -> Self {
        Self(None)
    }
}

impl<T> Clone for SharedListeners<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: Clone> SharedListeners<T> {
    pub(crate) fn push(&mut self, listener: T) {
        Rc::make_mut(self.0.get_or_insert_with(Default::default)).push(listener);
    }
}

impl<T> Deref for SharedListeners<T> {
    type Target = [T];

    fn deref(&self) -> &[T] {
        self.0.as_deref().map_or(&[], Vec::as_slice)
    }
}

impl<'a, T> IntoIterator for &'a SharedListeners<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_dispatch_storage_is_one_pointer() {
        assert_eq!(mem::size_of::<SharedListeners<usize>>(), mem::size_of::<usize>());
        assert!(SharedListeners::<usize>::default().is_empty());
    }

    #[test]
    fn replay_shares_listener_storage_without_mutating_committed_frame() {
        let mut committed = SharedListeners::default();
        committed.push(1);
        let mut candidate = committed.clone();
        assert_eq!(candidate.as_ptr(), committed.as_ptr());
        candidate.push(2);
        assert_ne!(candidate.as_ptr(), committed.as_ptr());
        assert_eq!(&*committed, &[1]);
        assert_eq!(&*candidate, &[1, 2]);
    }
}
