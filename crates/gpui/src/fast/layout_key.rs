//! Stable layout-node identity, ported from Longbridge GPUI Fast.
use crate::{ElementId, SharedString, Window};
use collections::FxHasher;
use smallvec::SmallVec;
use std::{
    hash::{Hash, Hasher},
    mem,
};

/// Per-window state for matching elements to their previous frame's Taffy nodes.
pub(crate) struct WindowLayout {
    key_stack: SmallVec<[LayoutKeyFrame; 32]>,
    root_index: u32,
    prepaint_scope: u64,
    prepaint_scope_active: bool,
    list_rows: usize,
}

impl Default for WindowLayout {
    fn default() -> Self {
        Self {
            key_stack: SmallVec::new(),
            root_index: 0,
            prepaint_scope: LAYOUT_ROOT_SEED,
            prepaint_scope_active: false,
            list_rows: 0,
        }
    }
}

impl WindowLayout {
    pub(crate) fn end_frame(&mut self) {
        debug_assert!(self.key_stack.is_empty());
        debug_assert!(!self.prepaint_scope_active);
        self.root_index = 0;
        self.prepaint_scope = LAYOUT_ROOT_SEED;
        self.list_rows = 0;
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.key_stack.is_empty() && !self.prepaint_scope_active
    }
}

struct LayoutKeyFrame {
    key: u64,
    next_unidentified_child: u32,
}

const LAYOUT_ROOT_SEED: u64 = 0x9E37_79B9_7F4A_7C15;
const LAYOUT_PREPAINT_SALT: u64 = 0x5BF0_3635_931A_2E77;

#[inline]
fn mix(state: u64, value: u64) -> u64 {
    let mut z = state
        .rotate_left(27)
        .wrapping_add(value)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[inline(always)]
pub(crate) fn layout_key(window: &Window) -> Option<u64> {
    window.fast_layout.key_stack.last().map(|frame| frame.key)
}

#[inline(always)]
pub(crate) fn parent_layout_key(window: &Window) -> Option<u64> {
    let stack = &window.fast_layout.key_stack;
    stack.len().checked_sub(2).map(|parent| stack[parent].key)
}

#[inline(always)]
pub(crate) fn current_layout_key(window: &Window) -> Option<u64> {
    window
        .paint_layout_key
        .or_else(|| layout_key(window))
        .or_else(|| prepaint_layout_key(window))
}

#[inline(always)]
pub(crate) fn prepaint_layout_key(window: &Window) -> Option<u64> {
    window
        .fast_layout
        .prepaint_scope_active
        .then_some(window.fast_layout.prepaint_scope)
}

#[inline(always)]
pub(crate) fn is_laying_out_list_row(window: &Window) -> bool {
    window.fast_layout.list_rows != 0
}

#[inline]
pub(crate) fn push_layout_key(window: &mut Window, id: Option<&ElementId>) -> u64 {
    let layout = &mut window.fast_layout;
    let component = match id {
        Some(id) => {
            let mut hasher = FxHasher::default();
            id.hash(&mut hasher);
            mix(hasher.finish(), 1)
        }
        None => {
            let index = match layout.key_stack.last_mut() {
                Some(parent) => &mut parent.next_unidentified_child,
                None => &mut layout.root_index,
            };
            let component = mix(*index as u64, 2);
            *index += 1;
            component
        }
    };
    let parent = layout
        .key_stack
        .last()
        .map(|parent| parent.key)
        .unwrap_or_else(|| mix(layout.prepaint_scope, LAYOUT_PREPAINT_SALT));
    let key = mix(parent, component);
    layout.key_stack.push(LayoutKeyFrame {
        key,
        next_unidentified_child: 0,
    });
    key
}

#[inline(always)]
pub(crate) fn pop_layout_key(window: &mut Window) {
    window.fast_layout.key_stack.pop();
}

#[inline(always)]
pub(crate) fn enter_prepaint_scope(window: &mut Window, key: u64) -> (u64, u32, bool) {
    let layout = &mut window.fast_layout;
    (
        mem::replace(&mut layout.prepaint_scope, key),
        mem::replace(&mut layout.root_index, 0),
        mem::replace(&mut layout.prepaint_scope_active, true),
    )
}

#[inline(always)]
pub(crate) fn exit_prepaint_scope(window: &mut Window, enclosing: (u64, u32, bool)) {
    (
        window.fast_layout.prepaint_scope,
        window.fast_layout.root_index,
        window.fast_layout.prepaint_scope_active,
    ) = enclosing;
}

impl Window {
    pub(crate) fn with_parent_layout_key<R>(
        &mut self,
        parent: Option<u64>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let saved = mem::take(&mut self.fast_layout.key_stack);
        if let Some(parent) = parent {
            self.fast_layout.key_stack.push(LayoutKeyFrame {
                key: parent,
                next_unidentified_child: 0,
            });
        }
        let result = f(self);
        self.fast_layout.key_stack = saved;
        result
    }

    pub(crate) fn with_layout_key_of_prepainting_element<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let key = self.fast_layout.prepaint_scope;
        self.with_parent_layout_key(Some(key), f)
    }

    pub(crate) fn with_list_item_layout_key<R>(
        &mut self,
        index: usize,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let key = ElementId::NamedInteger(
            SharedString::new_static("gpui::list_item"),
            index as u64,
        );
        self.fast_layout.list_rows += 1;
        push_layout_key(self, Some(&key));
        let result = f(self);
        pop_layout_key(self);
        self.fast_layout.list_rows = self.fast_layout.list_rows.saturating_sub(1);
        result
    }
}

#[cfg(test)]
pub(crate) fn key_for_path(path: &[ElementId]) -> u64 {
    path.iter().fold(
        mix(LAYOUT_ROOT_SEED, LAYOUT_PREPAINT_SALT),
        |parent, id| {
            let mut hasher = FxHasher::default();
            id.hash(&mut hasher);
            mix(parent, mix(hasher.finish(), 1))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identified_elements_do_not_renumber_anonymous_siblings() {
        let fixed = ElementId::from("fixed");
        let parent = mix(LAYOUT_ROOT_SEED, LAYOUT_PREPAINT_SALT);
        let first = mix(parent, mix(0, 2));
        let named = key_for_path(&[fixed]);
        let second = mix(parent, mix(1, 2));
        assert_ne!(first, named);
        assert_ne!(second, named);
        assert_ne!(first, second);
    }
}
