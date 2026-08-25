use crate::{FocusHandle, MouseDownEvent, Pixels, Point};
use std::{cell::RefCell, rc::Rc};

use super::tooltip::ActiveTooltip;

/// The per-frame state of an interactive element. Used for tracking stateful
/// interactions like clicks and scroll offsets.
#[derive(Default)]
pub struct InteractiveElementState {
    pub(crate) focus_handle: Option<FocusHandle>,
    pub(crate) clicked_state: Option<Rc<RefCell<ElementClickedState>>>,
    pub(crate) hover_state: Option<Rc<RefCell<bool>>>,
    pub(crate) pending_mouse_down: Option<Rc<RefCell<Option<MouseDownEvent>>>>,
    pub(crate) scroll_offset: Option<Rc<RefCell<Point<Pixels>>>>,
    pub(crate) active_tooltip: Option<Rc<RefCell<Option<ActiveTooltip>>>>,
}

/// Whether or not the element or a group that contains it is clicked by the mouse.
#[derive(Copy, Clone, Default, Eq, PartialEq)]
pub struct ElementClickedState {
    /// True if this element's group has been clicked, false otherwise.
    pub group: bool,

    /// True if this element has been clicked, false otherwise.
    pub element: bool,
}

impl ElementClickedState {
    pub(crate) fn is_clicked(&self) -> bool {
        self.group || self.element
    }
}

fn ensure_default<T: Default>(option: &mut Option<Rc<RefCell<T>>>) -> Rc<RefCell<T>> {
    match option {
        Some(value) => value.clone(),
        None => option.insert(Default::default()).clone(),
    }
}

impl InteractiveElementState {
    pub(crate) fn ensure_clicked_state(&mut self) -> Rc<RefCell<ElementClickedState>> {
        ensure_default(&mut self.clicked_state)
    }

    pub(crate) fn ensure_hover_state(&mut self) -> Rc<RefCell<bool>> {
        ensure_default(&mut self.hover_state)
    }

    pub(crate) fn ensure_pending_mouse_down(&mut self) -> Rc<RefCell<Option<MouseDownEvent>>> {
        ensure_default(&mut self.pending_mouse_down)
    }

    pub(crate) fn ensure_scroll_offset(&mut self) -> Rc<RefCell<Point<Pixels>>> {
        ensure_default(&mut self.scroll_offset)
    }

    pub(crate) fn ensure_active_tooltip(&mut self) -> Rc<RefCell<Option<ActiveTooltip>>> {
        ensure_default(&mut self.active_tooltip)
    }
}
