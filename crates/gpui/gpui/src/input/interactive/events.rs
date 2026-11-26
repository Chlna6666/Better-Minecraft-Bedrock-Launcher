use crate::seal::Sealed;

use super::PlatformInput;

/// An event from a platform input source.
pub trait InputEvent: Sealed + 'static {
    /// Convert this event into the platform input enum.
    fn to_platform_input(self) -> PlatformInput;
}

/// A key event from the platform.
pub trait KeyEvent: InputEvent {}

/// A mouse event from the platform.
pub trait MouseEvent: InputEvent {}

/// A semantic gesture event recognized from raw pointer or touch input.
pub trait GestureEvent: InputEvent {}
