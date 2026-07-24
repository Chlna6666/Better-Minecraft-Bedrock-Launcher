use crate::{Modifiers, Pixels, Point, point, seal::Sealed};
use std::ops::Deref;

use super::{InputEvent, MouseEvent, PlatformInput};

/// The phase of a touch motion event.
/// Based on the winit enum of the same name.
#[derive(Clone, Copy, Debug, Default)]
pub enum TouchPhase {
    /// The touch started.
    Started,
    /// The touch event is moving.
    #[default]
    Moved,
    /// The touch phase has ended
    Ended,
}

/// A mouse down event from the platform
#[derive(Clone, Debug, Default)]
pub struct MouseDownEvent {
    /// Which mouse button was pressed.
    pub button: MouseButton,

    /// The position of the mouse on the window.
    pub position: Point<Pixels>,

    /// The modifiers that were held down when the mouse was pressed.
    pub modifiers: Modifiers,

    /// The number of times the button has been clicked.
    pub click_count: usize,

    /// Whether this is the first, focusing click.
    pub first_mouse: bool,
}

impl Sealed for MouseDownEvent {}
impl InputEvent for MouseDownEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::MouseDown(self)
    }
}
impl MouseEvent for MouseDownEvent {}

/// A mouse up event from the platform
#[derive(Clone, Debug, Default)]
pub struct MouseUpEvent {
    /// Which mouse button was released.
    pub button: MouseButton,

    /// The position of the mouse on the window.
    pub position: Point<Pixels>,

    /// The modifiers that were held down when the mouse was released.
    pub modifiers: Modifiers,

    /// The number of times the button has been clicked.
    pub click_count: usize,
}

impl Sealed for MouseUpEvent {}
impl InputEvent for MouseUpEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::MouseUp(self)
    }
}
impl MouseEvent for MouseUpEvent {}
/// An enum representing the keyboard button that was pressed for a click event.
#[derive(Hash, PartialEq, Eq, Copy, Clone, Debug, Default)]
pub enum KeyboardButton {
    /// Enter key was clicked
    #[default]
    Enter,
    /// Space key was clicked
    Space,
}

/// An enum representing the mouse button that was pressed.
#[derive(Hash, PartialEq, Eq, Copy, Clone, Debug, Default)]
pub enum MouseButton {
    /// The left mouse button.
    #[default]
    Left,

    /// The right mouse button.
    Right,

    /// The middle mouse button.
    Middle,

    /// A navigation button, such as back or forward.
    Navigate(NavigationDirection),
}

impl MouseButton {
    /// Get all the mouse buttons in a list.
    pub fn all() -> Vec<Self> {
        vec![
            MouseButton::Left,
            MouseButton::Right,
            MouseButton::Middle,
            MouseButton::Navigate(NavigationDirection::Back),
            MouseButton::Navigate(NavigationDirection::Forward),
        ]
    }
}

/// A navigation direction, such as back or forward.
#[derive(Hash, PartialEq, Eq, Copy, Clone, Debug, Default)]
pub enum NavigationDirection {
    /// The back button.
    #[default]
    Back,

    /// The forward button.
    Forward,
}

/// A mouse move event from the platform
#[derive(Clone, Debug, Default)]
pub struct MouseMoveEvent {
    /// The position of the mouse on the window.
    pub position: Point<Pixels>,

    /// The mouse button that was pressed, if any.
    pub pressed_button: Option<MouseButton>,

    /// The modifiers that were held down when the mouse was moved.
    pub modifiers: Modifiers,
}

impl Sealed for MouseMoveEvent {}
impl InputEvent for MouseMoveEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::MouseMove(self)
    }
}
impl MouseEvent for MouseMoveEvent {}

impl MouseMoveEvent {
    /// Returns true if the left mouse button is currently held down.
    pub fn dragging(&self) -> bool {
        self.pressed_button == Some(MouseButton::Left)
    }
}

/// A mouse wheel event from the platform
#[derive(Clone, Debug, Default)]
pub struct ScrollWheelEvent {
    /// The position of the mouse on the window.
    pub position: Point<Pixels>,

    /// The change in scroll wheel position for this event.
    pub delta: ScrollDelta,

    /// The modifiers that were held down when the mouse was moved.
    pub modifiers: Modifiers,

    /// The phase of the touch event.
    pub touch_phase: TouchPhase,
}

impl Sealed for ScrollWheelEvent {}
impl InputEvent for ScrollWheelEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::ScrollWheel(self)
    }
}
impl MouseEvent for ScrollWheelEvent {}

impl Deref for ScrollWheelEvent {
    type Target = Modifiers;

    fn deref(&self) -> &Self::Target {
        &self.modifiers
    }
}

/// The scroll delta for a scroll wheel event.
#[derive(Clone, Copy, Debug)]
pub enum ScrollDelta {
    /// An exact scroll delta in pixels.
    Pixels(Point<Pixels>),
    /// An inexact scroll delta in lines.
    Lines(Point<f32>),
}

impl Default for ScrollDelta {
    fn default() -> Self {
        Self::Lines(Default::default())
    }
}

impl ScrollDelta {
    /// Returns true if this is a precise scroll delta in pixels.
    pub fn precise(&self) -> bool {
        match self {
            ScrollDelta::Pixels(_) => true,
            ScrollDelta::Lines(_) => false,
        }
    }

    /// Converts this scroll event into exact pixels.
    pub fn pixel_delta(&self, line_height: Pixels) -> Point<Pixels> {
        match self {
            ScrollDelta::Pixels(delta) => *delta,
            ScrollDelta::Lines(delta) => point(line_height * delta.x, line_height * delta.y),
        }
    }

    /// Combines two scroll deltas into one.
    /// If the signs of the deltas are the same (both positive or both negative),
    /// the deltas are added together. If the signs are opposite, the second delta
    /// (other) is used, effectively overriding the first delta.
    pub fn coalesce(self, other: ScrollDelta) -> ScrollDelta {
        match (self, other) {
            (ScrollDelta::Pixels(a), ScrollDelta::Pixels(b)) => {
                let x = if a.x.signum() == b.x.signum() {
                    a.x + b.x
                } else {
                    b.x
                };

                let y = if a.y.signum() == b.y.signum() {
                    a.y + b.y
                } else {
                    b.y
                };

                ScrollDelta::Pixels(point(x, y))
            }

            (ScrollDelta::Lines(a), ScrollDelta::Lines(b)) => {
                let x = if a.x.signum() == b.x.signum() {
                    a.x + b.x
                } else {
                    b.x
                };

                let y = if a.y.signum() == b.y.signum() {
                    a.y + b.y
                } else {
                    b.y
                };

                ScrollDelta::Lines(point(x, y))
            }

            _ => other,
        }
    }
}

/// A mouse exit event from the platform, generated when the mouse leaves the window.
#[derive(Clone, Debug, Default)]
pub struct MouseExitEvent {
    /// The position of the mouse relative to the window.
    pub position: Point<Pixels>,
    /// The mouse button that was pressed, if any.
    pub pressed_button: Option<MouseButton>,
    /// The modifiers that were held down when the mouse was moved.
    pub modifiers: Modifiers,
}

impl Sealed for MouseExitEvent {}
impl InputEvent for MouseExitEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::MouseExited(self)
    }
}
impl MouseEvent for MouseExitEvent {}

impl Deref for MouseExitEvent {
    type Target = Modifiers;

    fn deref(&self) -> &Self::Target {
        &self.modifiers
    }
}
