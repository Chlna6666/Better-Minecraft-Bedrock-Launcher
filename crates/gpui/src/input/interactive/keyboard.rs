use crate::{Capslock, Keystroke, Modifiers, seal::Sealed};
use std::ops::Deref;

use super::{InputEvent, KeyEvent, PlatformInput};

/// The key down event equivalent for the platform.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyDownEvent {
    /// The keystroke that was generated.
    pub keystroke: Keystroke,

    /// Whether the key is currently held down.
    pub is_held: bool,
}

impl Sealed for KeyDownEvent {}
impl InputEvent for KeyDownEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::KeyDown(self)
    }
}
impl KeyEvent for KeyDownEvent {}

/// The key up event equivalent for the platform.
#[derive(Clone, Debug)]
pub struct KeyUpEvent {
    /// The keystroke that was released.
    pub keystroke: Keystroke,
}

impl Sealed for KeyUpEvent {}
impl InputEvent for KeyUpEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::KeyUp(self)
    }
}
impl KeyEvent for KeyUpEvent {}

/// The modifiers changed event equivalent for the platform.
#[derive(Clone, Debug, Default)]
pub struct ModifiersChangedEvent {
    /// The new state of the modifier keys
    pub modifiers: Modifiers,
    /// The new state of the capslock key
    pub capslock: Capslock,
}

impl Sealed for ModifiersChangedEvent {}
impl InputEvent for ModifiersChangedEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::ModifiersChanged(self)
    }
}
impl KeyEvent for ModifiersChangedEvent {}

impl Deref for ModifiersChangedEvent {
    type Target = Modifiers;

    fn deref(&self) -> &Self::Target {
        &self.modifiers
    }
}
