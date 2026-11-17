use super::Interactivity;
use crate::{ArenaBox, window::with_element_arena};
use std::ops::{Deref, DerefMut};

/// Keeps builder moves small, using the frame arena only inside an active draw scope.
pub(in crate::element::div) enum InteractivityBox {
    Arena(ArenaBox<Interactivity>),
    Heap(Box<Interactivity>),
}

impl InteractivityBox {
    pub(in crate::element::div) fn new() -> Self {
        with_element_arena(|arena| {
            if arena.is_in_scope() {
                Self::Arena(arena.alloc(Interactivity::default))
            } else {
                Self::Heap(Box::default())
            }
        })
    }
}

impl Deref for InteractivityBox {
    type Target = Interactivity;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Arena(value) => value,
            Self::Heap(value) => value,
        }
    }
}

impl DerefMut for InteractivityBox {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Arena(value) => value,
            Self::Heap(value) => value,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Div, InteractiveElement, Styled, div, window::ElementArenaScope};

    #[test]
    fn frame_external_div_survives_arena_clear() {
        let mut element = div().id("delayed");
        with_element_arena(|arena| arena.clear());
        element.style().opacity = Some(0.5);
        assert_eq!(element.interactivity().element_id, Some("delayed".into()));
    }

    #[test]
    fn nested_draw_clear_preserves_outer_interactivity() {
        let outer = ElementArenaScope::enter();
        let mut interactivity = InteractivityBox::new();
        assert!(matches!(interactivity, InteractivityBox::Arena(_)));
        {
            let _inner = ElementArenaScope::enter();
            with_element_arena(|arena| arena.clear());
        }
        interactivity.tab_stop = true;
        assert!(interactivity.tab_stop);
        drop(outer);
        with_element_arena(|arena| arena.clear());
    }

    #[test]
    fn div_builders_move_less_than_inline_interactivity() {
        assert!(std::mem::size_of::<Div>() < std::mem::size_of::<Interactivity>());
    }
}
