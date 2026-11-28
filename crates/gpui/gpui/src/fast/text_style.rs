// Copyright 2022-2025 Zed Industries, Inc.; Copyright 2026 Longbridge.
// SPDX-License-Identifier: Apache-2.0
// Adapted from longbridge/gpui-fast 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
//! The window's text style stack, which remembers the styles it resolves.

use crate::{TextAlign, TextStyle, TextStyleRefinement};
use refineable::Refineable;
use std::{cell::{Cell, RefCell}, rc::Rc};

/// The text style refinements pushed while drawing, as `Window` keeps them,
/// and the text styles they resolve to.
///
/// Every text element asks for the text style in effect where it is, which
/// refines the default style by every refinement on the stack, and gets a
/// copy of its own. Elements under the same refinements, like the cells of a
/// table, all ask for the same style, so the style resolved at each depth is
/// kept until the stack changes there, and handed out as an `Rc`.
pub(crate) struct TextStyleStack {
    refinements: Vec<TextStyleRefinement>,
    inherited: Option<Rc<TextStyle>>,
    // Default style changes invalidate even suspended deferred-draw stacks.
    generation: Cell<u32>,
    /// `resolved[i]`, when known, is the default text style refined by the
    /// first `i` refinements. It always holds one more entry than
    /// `refinements`.
    resolved: RefCell<Vec<Option<Rc<TextStyle>>>>,
}

impl Default for TextStyleStack {
    fn default() -> Self {
        Self {
            refinements: Vec::new(),
            inherited: None,
            generation: Cell::new(0),
            resolved: RefCell::new(vec![None]),
        }
    }
}

impl Clone for TextStyleStack {
    fn clone(&self) -> Self {
        Self {
            refinements: self.refinements.clone(),
            inherited: self.inherited.clone(),
            generation: self.generation.clone(),
            resolved: self.resolved.clone(),
        }
    }

    fn clone_from(&mut self, source: &Self) {
        self.refinements.clone_from(&source.refinements);
        self.inherited.clone_from(&source.inherited);
        self.generation.set(source.generation.get());
        self.resolved
            .get_mut()
            .clone_from(&source.resolved.borrow());
    }
}

impl TextStyleStack {
    pub(crate) fn with_inherited(style: &TextStyle) -> Self {
        Self { inherited: Some(Rc::new(style.clone())), ..Self::default() }
    }

    pub(crate) fn push(&mut self, refinement: TextStyleRefinement) {
        self.refinements.push(refinement);
        self.resolved.get_mut().push(None);
    }

    /// Pops the refinement pushed last, dropping it where it lies rather
    /// than handing it back: nothing wants it, and it is 200-odd bytes.
    pub(crate) fn pop(&mut self) {
        if let Some(depth) = self.refinements.len().checked_sub(1) {
            self.refinements.truncate(depth);
            self.resolved.get_mut().truncate(depth + 1);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.refinements.clear();
        self.resolved.get_mut().truncate(1);
    }

    pub(crate) fn trim(&mut self, floor: usize, eager: bool) {
        let target = floor.max(self.refinements.len());
        if eager || self.refinements.capacity() > target.saturating_mul(4) {
            self.refinements.shrink_to(target);
        }
        let resolved = self.resolved.get_mut();
        let target = floor.max(resolved.len());
        if eager || resolved.capacity() > target.saturating_mul(4) {
            resolved.shrink_to(target);
        }
    }

    /// Fast paint-only path: the last refinement wins, without resolving or
    /// copying a full TextStyle. The window default can be customized.
    #[inline]
    pub(crate) fn text_align(&self, default: &TextStyle, generation: u32) -> TextAlign {
        if self.generation.get() == generation {
            if let Some(style) = self.resolved.borrow().last().and_then(Option::as_ref) {
                return style.text_align;
            }
        }
        self.refinements.iter().rev()
            .find_map(|refinement| refinement.text_align)
            .unwrap_or_else(|| self.inherited.as_deref().unwrap_or(default).text_align)
    }

    /// The default text style refined by every refinement on the stack, as
    /// `Window::text_style` returns it.
    pub(crate) fn resolve(&self, default: &TextStyle, generation: u32) -> Rc<TextStyle> {
        let mut resolved = self.resolved.borrow_mut();
        if self.generation.replace(generation) != generation {
            resolved.fill(None);
        }
        let depth = self.refinements.len();
        if let Some(style) = &resolved[depth] {
            return style.clone();
        }
        // `Rc::make_mut` clones the known style straight into the new
        // style's allocation, where a clone refined and then wrapped would be
        // copied twice more.
        let (mut style, from) = match resolved[..depth].iter().rposition(|style| style.is_some()) {
            Some(known) => (resolved[known].clone().unwrap(), known),
            None => (self.inherited.clone().unwrap_or_else(|| Rc::new(default.clone())), 0),
        };
        if from == depth {
            resolved[depth] = Some(style.clone());
            return style;
        }
        let refined = Rc::make_mut(&mut style);
        for refinement in &self.refinements[from..] {
            refined.refine(refinement);
        }
        resolved[depth] = Some(style.clone());
        style
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sibling_scopes_reuse_parent_resolution() {
        let mut stack = TextStyleStack::default();
        let default = TextStyle::default();
        stack.push(TextStyleRefinement::default());
        let parent = stack.resolve(&default, 0);
        stack.push(TextStyleRefinement::default());
        let _ = stack.resolve(&default, 0);
        stack.pop();
        assert!(Rc::ptr_eq(&parent, &stack.resolve(&default, 0)));
        stack.push(TextStyleRefinement::default());
        let _ = stack.resolve(&default, 0);
        stack.pop();
        assert!(Rc::ptr_eq(&parent, &stack.resolve(&default, 0)));
    }

    #[test]
    fn deferred_stack_invalidates_on_default_generation_change() {
        let mut stack = TextStyleStack::default();
        let default = TextStyle::default();
        stack.push(TextStyleRefinement::default());
        let old = stack.resolve(&default, 0);
        let deferred = stack.clone();
        assert!(Rc::ptr_eq(&old, &deferred.resolve(&default, 0)));
        assert!(!Rc::ptr_eq(&old, &deferred.resolve(&default, 1)));
    }
}
