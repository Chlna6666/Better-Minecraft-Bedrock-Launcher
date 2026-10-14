//! Reusable bounded numeric input for precision editing alongside presets and sliders.
//! The GPUI Input owns the edit buffer: parsers never mutate business state on every keystroke.

use super::input::{Input, InputSize, InputState};
use gpui::{App, AppContext as _, Context, Entity, Styled, Window, px};

/// Independent text edit state; callers commit on Enter/blur and retain their domain constraints.
#[derive(Clone)]
pub(crate) struct NumericField {
    pub(crate) state: Entity<InputState>,
}

impl NumericField {
    pub(crate) fn new<V: 'static>(
        window: &mut Window,
        cx: &mut Context<V>,
        initial_value: u32,
    ) -> Self {
        let state = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_text(initial_value.to_string(), cx);
            input
        });
        Self { state }
    }

    /// Styled identically in the OBJ, image-block, and map-image parameter inspectors.
    pub(crate) fn control(&self) -> Input {
        Input::new(&self.state)
            .with_size(InputSize::Small)
            .w(px(88.0))
    }

    pub(crate) fn parse(&self, cx: &App, min: u32, max: u32) -> Option<u32> {
        parse_bounded_u32(self.state.read(cx).value().as_ref(), min, max)
    }

    /// Explicit setting changes keep edit text synchronized. No Change event is emitted.
    pub(crate) fn sync<V: 'static>(&self, value: u32, cx: &mut Context<V>) {
        let value = value.to_string();
        self.state.update(cx, |input, cx| {
            if input.value().as_ref() != value {
                input.set_text(value, cx);
            }
        });
    }
}

/// Strict decimal integer parsing: reject negative values, fractions, overflows and out-of-range
/// values instead of silently clamping a user's intended size.
pub(crate) fn parse_bounded_u32(text: &str, min: u32, max: u32) -> Option<u32> {
    let trimmed = text.trim();
    if trimmed.is_empty() || !trimmed.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    trimmed.parse::<u32>().ok().filter(|value| (min..=max).contains(value))
}

#[cfg(test)]
mod tests {
    use super::parse_bounded_u32;

    #[test]
    fn accepts_exact_dimensions_in_range() {
        assert_eq!(parse_bounded_u32(" 137 ", 1, 512), Some(137));
        assert_eq!(parse_bounded_u32("64", 1, 64), Some(64));
    }

    #[test]
    fn rejects_invalid_values_without_clamping() {
        for value in ["", "0", "-1", "2.5", "65", "99999999999999", "1x"] {
            assert_eq!(parse_bounded_u32(value, 1, 64), None);
        }
    }
}
