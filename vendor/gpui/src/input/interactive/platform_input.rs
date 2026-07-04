use std::any::Any;

use super::{
    FileDropEvent, KeyDownEvent, KeyUpEvent, ModifiersChangedEvent, MouseDownEvent, MouseExitEvent,
    MouseMoveEvent, MouseUpEvent, ScrollWheelEvent,
};

/// An enum corresponding to all kinds of platform input events.
#[derive(Clone, Debug)]
pub enum PlatformInput {
    /// A key was pressed.
    KeyDown(KeyDownEvent),
    /// A key was released.
    KeyUp(KeyUpEvent),
    /// The keyboard modifiers were changed.
    ModifiersChanged(ModifiersChangedEvent),
    /// The mouse was pressed.
    MouseDown(MouseDownEvent),
    /// The mouse was released.
    MouseUp(MouseUpEvent),
    /// The mouse was moved.
    MouseMove(MouseMoveEvent),
    /// The mouse exited the window.
    MouseExited(MouseExitEvent),
    /// The scroll wheel was used.
    ScrollWheel(ScrollWheelEvent),
    /// Files were dragged and dropped onto the window.
    FileDrop(FileDropEvent),
}

impl PlatformInput {
    pub(crate) fn dispatch_class(&self) -> PlatformInputDispatchClass {
        match self {
            PlatformInput::MouseMove(event) if event.pressed_button.is_none() => {
                PlatformInputDispatchClass::PassivePointerMove
            }
            PlatformInput::MouseMove(_) => PlatformInputDispatchClass::InteractivePointerMove,
            PlatformInput::MouseDown(_) | PlatformInput::MouseUp(_) => {
                PlatformInputDispatchClass::PointerButton
            }
            PlatformInput::ScrollWheel(_) => PlatformInputDispatchClass::Scroll,
            PlatformInput::KeyDown(_)
            | PlatformInput::KeyUp(_)
            | PlatformInput::ModifiersChanged(_) => PlatformInputDispatchClass::Keyboard,
            PlatformInput::FileDrop(_) => PlatformInputDispatchClass::DragDrop,
            PlatformInput::MouseExited(_) => PlatformInputDispatchClass::PassivePointerMove,
        }
    }

    pub(crate) fn unconditionally_extends_recent_input_present(&self) -> bool {
        self.dispatch_class().extends_recent_input_present()
    }

    pub(crate) fn mouse_event(&self) -> Option<&dyn Any> {
        match self {
            PlatformInput::KeyDown { .. } => None,
            PlatformInput::KeyUp { .. } => None,
            PlatformInput::ModifiersChanged { .. } => None,
            PlatformInput::MouseDown(event) => Some(event),
            PlatformInput::MouseUp(event) => Some(event),
            PlatformInput::MouseMove(event) => Some(event),
            PlatformInput::MouseExited(event) => Some(event),
            PlatformInput::ScrollWheel(event) => Some(event),
            PlatformInput::FileDrop(event) => Some(event),
        }
    }

    pub(crate) fn keyboard_event(&self) -> Option<&dyn Any> {
        match self {
            PlatformInput::KeyDown(event) => Some(event),
            PlatformInput::KeyUp(event) => Some(event),
            PlatformInput::ModifiersChanged(event) => Some(event),
            PlatformInput::MouseDown(_) => None,
            PlatformInput::MouseUp(_) => None,
            PlatformInput::MouseMove(_) => None,
            PlatformInput::MouseExited(_) => None,
            PlatformInput::ScrollWheel(_) => None,
            PlatformInput::FileDrop(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PlatformInputDispatchClass {
    PassivePointerMove,
    InteractivePointerMove,
    PointerButton,
    Scroll,
    Keyboard,
    DragDrop,
}

impl PlatformInputDispatchClass {
    pub(crate) fn extends_recent_input_present(self) -> bool {
        match self {
            Self::PassivePointerMove | Self::PointerButton => false,
            Self::InteractivePointerMove | Self::Scroll | Self::Keyboard | Self::DragDrop => true,
        }
    }
}
