#![expect(
    unsafe_code,
    reason = "the arena encapsulates raw allocation and pointer lifetime invariants"
)]

use std::{
    alloc::{self, handle_alloc_error},
    cell::Cell,
    num::NonZeroUsize,
    ops::{Deref, DerefMut},
    ptr::{self, NonNull},
    rc::Rc,
};

use crate::record_arena_chunk_expansion;

struct ArenaElement {
    value: *mut u8,
    drop: unsafe fn(*mut u8),
}

impl Drop for ArenaElement {
    #[inline(always)]
    fn drop(&mut self) {
        unsafe { (self.drop)(self.value) };
    }
}

struct Chunk {
    start: *mut u8,
    end: *mut u8,
    offset: *mut u8,
    layout: alloc::Layout,
}

const DEFAULT_CHUNK_ALIGNMENT: usize = 64;

impl Drop for Chunk {
    fn drop(&mut self) {
        unsafe {
            alloc::dealloc(self.start, self.layout);
        }
    }
}

impl Chunk {
    fn new(chunk_size: NonZeroUsize, alignment: usize) -> Self {
        // this only fails if chunk_size is unreasonably huge
        let layout = alloc::Layout::from_size_align(chunk_size.get(), alignment.max(1)).unwrap();
        let start = unsafe { alloc::alloc(layout) };
        if start.is_null() {
            handle_alloc_error(layout);
        }
        let end = unsafe { start.add(chunk_size.get()) };
        Self {
            start,
            end,
            offset: start,
            layout,
        }
    }

    fn size(&self) -> usize {
        self.layout.size()
    }

    fn allocate(&mut self, layout: alloc::Layout) -> Option<NonNull<u8>> {
        // Keep pointer arithmetic inside the allocation. Constructing an out-of-bounds pointer
        // with `ptr::add` is UB even when that pointer is only used to decide that this chunk is
        // full. Compute addresses first and materialize pointers only after the bounds check.
        let base = self.offset.addr();
        let aligned_addr = base.checked_add(self.offset.align_offset(layout.align()))?;
        let next_addr = aligned_addr.checked_add(layout.size())?;
        if next_addr > self.end.addr() {
            return None;
        }

        let aligned = self.offset.with_addr(aligned_addr);
        self.offset = self.offset.with_addr(next_addr);
        NonNull::new(aligned)
    }

    fn reset(&mut self) {
        self.offset = self.start;
    }
}

pub struct Arena {
    chunks: Vec<Chunk>,
    elements: Vec<ArenaElement>,
    valid: Rc<Cell<bool>>,
    current_chunk_index: usize,
    chunk_size: NonZeroUsize,
    /// Number of active draw scopes using allocations from this arena.
    scope_depth: usize,
}

impl Drop for Arena {
    fn drop(&mut self) {
        self.force_clear();
    }
}

impl Arena {
    pub fn new(chunk_size: usize) -> Self {
        let chunk_size = NonZeroUsize::try_from(chunk_size).unwrap();
        Self {
            chunks: vec![Chunk::new(chunk_size, DEFAULT_CHUNK_ALIGNMENT)],
            elements: Vec::new(),
            valid: Rc::new(Cell::new(true)),
            current_chunk_index: 0,
            chunk_size,
            scope_depth: 0,
        }
    }

    pub fn capacity(&self) -> usize {
        self.chunks.iter().map(Chunk::size).sum()
    }

    /// Marks the start of a draw whose arena allocations must remain valid until it exits.
    pub(crate) fn begin_scope(&mut self) {
        self.scope_depth = self.scope_depth.saturating_add(1);
    }

    /// Marks the end of the innermost draw scope.
    pub(crate) fn end_scope(&mut self) {
        self.scope_depth = self
            .scope_depth
            .checked_sub(1)
            .expect("Arena::end_scope called without a matching begin_scope");
    }

    /// Clears completed-frame allocations unless an enclosing draw is still using them.
    ///
    /// A nested draw may finish and drop its `ArenaClearNeeded` while the outer draw still owns
    /// `AnyElement` values in this arena. Deferring that clear keeps those references valid; the
    /// outer draw's token performs the real clear after the final scope exits.
    pub fn clear(&mut self) {
        if self.scope_depth == 0 {
            self.force_clear();
        } else {
            log::trace!(
                "deferring element arena clear while {} draw scope(s) remain active",
                self.scope_depth
            );
        }
    }

    fn force_clear(&mut self) {
        self.valid.set(false);
        self.valid = Rc::new(Cell::new(true));
        self.elements.clear();
        for chunk_index in 0..=self.current_chunk_index {
            self.chunks[chunk_index].reset();
        }
        self.current_chunk_index = 0;
    }

    /// Release capacity retained by a completed frame after an explicit memory trim.
    /// An arena with live elements must keep its chunks until the frame clears them.
    pub(crate) fn trim(&mut self) {
        if !self.elements.is_empty() {
            return;
        }
        self.chunks.truncate(1);
        self.chunks.shrink_to_fit();
        self.elements.shrink_to_fit();
        self.chunks[0].reset();
        self.current_chunk_index = 0;
    }

    #[inline(always)]
    pub fn alloc<T>(&mut self, f: impl FnOnce() -> T) -> ArenaBox<T> {
        #[inline(always)]
        unsafe fn inner_writer<T, F>(ptr: *mut T, f: F)
        where
            F: FnOnce() -> T,
        {
            unsafe { ptr::write(ptr, f()) };
        }

        unsafe fn drop<T>(ptr: *mut u8) {
            unsafe { std::ptr::drop_in_place(ptr.cast::<T>()) };
        }

        let layout = alloc::Layout::new::<T>();
        let ptr = if let Some(ptr) = self.chunks[self.current_chunk_index].allocate(layout) {
            ptr.as_ptr()
        } else {
            self.allocate_next_chunk(layout)
        };

        unsafe { inner_writer(ptr.cast(), f) };
        self.elements.push(ArenaElement {
            value: ptr,
            drop: drop::<T>,
        });

        ArenaBox {
            ptr: ptr.cast(),
            valid: self.valid.clone(),
        }
    }

    #[inline(never)]
    fn allocate_next_chunk(&mut self, layout: alloc::Layout) -> *mut u8 {
        self.current_chunk_index += 1;
        if self.current_chunk_index >= self.chunks.len() {
            let chunk_size = self.chunk_size.max(
                NonZeroUsize::new(layout.size().max(1))
                    .expect("layout size or alignment should be non-zero"),
            );
            self.chunks.push(Chunk::new(
                chunk_size,
                layout.align().max(DEFAULT_CHUNK_ALIGNMENT),
            ));
            record_arena_chunk_expansion(1);
            assert_eq!(self.current_chunk_index, self.chunks.len() - 1);
            log::trace!(
                "increased element arena capacity to {}kb",
                self.capacity() / 1024,
            );
        }

        let current_chunk = &mut self.chunks[self.current_chunk_index];
        if let Some(ptr) = current_chunk.allocate(layout) {
            ptr.as_ptr()
        } else {
            panic!(
                "Arena chunk_size of {} is too small to allocate {} bytes",
                self.chunk_size,
                layout.size()
            );
        }
    }
}

pub struct ArenaBox<T: ?Sized> {
    ptr: *mut T,
    valid: Rc<Cell<bool>>,
}

impl<T: ?Sized> ArenaBox<T> {
    #[inline(always)]
    pub fn map<U: ?Sized>(mut self, f: impl FnOnce(&mut T) -> &mut U) -> ArenaBox<U> {
        ArenaBox {
            ptr: f(&mut self),
            valid: self.valid,
        }
    }

    #[track_caller]
    fn validate(&self) {
        assert!(
            self.valid.get(),
            "attempted to dereference an ArenaRef after its Arena was cleared"
        );
    }
}

impl<T: ?Sized> Deref for ArenaBox<T> {
    type Target = T;

    #[inline(always)]
    fn deref(&self) -> &Self::Target {
        self.validate();
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> DerefMut for ArenaBox<T> {
    #[inline(always)]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.validate();
        unsafe { &mut *self.ptr }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    #[test]
    fn test_arena() {
        let mut arena = Arena::new(1024);
        let a = arena.alloc(|| 1u64);
        let b = arena.alloc(|| 2u32);
        let c = arena.alloc(|| 3u16);
        let d = arena.alloc(|| 4u8);
        assert_eq!(*a, 1);
        assert_eq!(*b, 2);
        assert_eq!(*c, 3);
        assert_eq!(*d, 4);

        arena.clear();
        let a = arena.alloc(|| 5u64);
        let b = arena.alloc(|| 6u32);
        let c = arena.alloc(|| 7u16);
        let d = arena.alloc(|| 8u8);
        assert_eq!(*a, 5);
        assert_eq!(*b, 6);
        assert_eq!(*c, 7);
        assert_eq!(*d, 8);

        // Ensure drop gets called.
        let dropped = Rc::new(Cell::new(false));
        struct DropGuard(Rc<Cell<bool>>);
        impl Drop for DropGuard {
            fn drop(&mut self) {
                self.0.set(true);
            }
        }
        arena.alloc(|| DropGuard(dropped.clone()));
        arena.clear();
        assert!(dropped.get());
    }

    #[test]
    fn test_arena_grow() {
        let mut arena = Arena::new(8);
        arena.alloc(|| 1u64);
        arena.alloc(|| 2u64);

        assert_eq!(arena.capacity(), 16);

        arena.alloc(|| 3u32);
        arena.alloc(|| 4u32);

        assert_eq!(arena.capacity(), 24);
    }

    #[test]
    fn trim_releases_chunks_only_after_clear() {
        let mut arena = Arena::new(8);
        arena.alloc(|| 1u64);
        arena.alloc(|| 2u64);
        assert_eq!(arena.capacity(), 16);

        arena.trim();
        assert_eq!(arena.capacity(), 16);

        arena.clear();
        arena.trim();
        assert_eq!(arena.capacity(), 8);
        assert_eq!(*arena.alloc(|| 3u64), 3);
    }

    #[test]
    fn clear_is_deferred_while_an_enclosing_scope_is_active() {
        let mut arena = Arena::new(64);
        arena.begin_scope();
        let outer = arena.alloc(|| 42u64);

        arena.begin_scope();
        let inner = arena.alloc(|| 7u64);
        arena.end_scope();
        arena.clear();

        assert_eq!(*outer, 42);
        assert_eq!(*inner, 7);

        arena.end_scope();
        arena.clear();
        assert!(!outer.valid.get());
        assert!(!inner.valid.get());
    }

    #[test]
    #[should_panic(expected = "Arena::end_scope called without a matching begin_scope")]
    fn unbalanced_arena_scope_panics() {
        let mut arena = Arena::new(64);
        arena.begin_scope();
        arena.end_scope();
        arena.end_scope();
    }

    #[test]
    fn test_arena_alignment() {
        let mut arena = Arena::new(256);
        let x1 = arena.alloc(|| 1u8);
        let x2 = arena.alloc(|| 2u16);
        let x3 = arena.alloc(|| 3u32);
        let x4 = arena.alloc(|| 4u64);
        let x5 = arena.alloc(|| 5u64);

        assert_eq!(*x1, 1);
        assert_eq!(*x2, 2);
        assert_eq!(*x3, 3);
        assert_eq!(*x4, 4);
        assert_eq!(*x5, 5);

        assert_eq!(x1.ptr.align_offset(std::mem::align_of_val(&*x1)), 0);
        assert_eq!(x2.ptr.align_offset(std::mem::align_of_val(&*x2)), 0);
    }

    #[test]
    #[should_panic(expected = "attempted to dereference an ArenaRef after its Arena was cleared")]
    fn test_arena_use_after_clear() {
        let mut arena = Arena::new(16);
        let value = arena.alloc(|| 1u64);

        arena.clear();
        let _read_value = *value;
    }
}
