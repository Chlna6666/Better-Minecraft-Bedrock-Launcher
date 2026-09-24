use crate::{Pixels, Point, seal::Sealed};

use super::{GestureEvent, InputEvent, MouseEvent, PlatformInput, TouchPhase};

/// Identifies one touch contact for its lifetime.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TouchId(
    /// Opaque platform-supplied identifier.
    pub u64,
);

/// Raw touch input supplied by a platform backend.
#[derive(Clone, Debug, Default)]
pub struct TouchEvent {
    /// Contact identifier.
    pub id: TouchId,
    /// Current contact phase.
    pub phase: TouchPhase,
    /// Raw position in window logical pixels.
    pub position: Point<Pixels>,
    /// Optional best-effort position prediction roughly one presentation frame ahead.
    pub predicted_position: Option<Point<Pixels>>,
    /// Normalized pressure in the 0.0..=1.0 range when the platform exposes it.
    pub force: Option<f32>,
}

impl Sealed for TouchEvent {}
impl InputEvent for TouchEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::Touch(self)
    }
}

/// A phased long-press gesture recognized from raw touch input.
///
/// Hit testing stays anchored to start_position while position reports the current contact.
#[derive(Clone, Debug)]
pub struct LongPressEvent {
    /// Gesture phase.
    pub phase: TouchPhase,
    /// Position where the touch began.
    pub start_position: Point<Pixels>,
    /// Current raw touch position.
    pub position: Point<Pixels>,
}

impl Default for LongPressEvent {
    fn default() -> Self {
        Self {
            phase: TouchPhase::Started,
            start_position: Point::default(),
            position: Point::default(),
        }
    }
}

impl Sealed for LongPressEvent {}
impl InputEvent for LongPressEvent {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::LongPress(self)
    }
}
impl GestureEvent for LongPressEvent {}
impl MouseEvent for LongPressEvent {}
