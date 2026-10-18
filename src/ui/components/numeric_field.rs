//! Reusable bounded numeric input for precision editing alongside presets and sliders.
//! The GPUI Input owns the edit buffer: parsers never mutate business state on every keystroke.

use super::input::{Input, InputSize, InputState};
use gpui::{App, AppContext as _, Context, Div, Entity, IntoElement, ParentElement, SharedString, Styled, Window, div, px};
use crate::ui::theme::colors::ThemeColors;
use std::{cell::Cell, rc::Rc};

/// Independent text edit state; callers commit on Enter/blur and retain their domain constraints.
#[derive(Clone)]
pub(crate) struct NumericField {
    pub(crate) state: Entity<InputState>,
    /// The last value displayed by the owner, separate from the user's in-progress edit.
    last_synced: Rc<Cell<u32>>,
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
        Self {
            state,
            last_synced: Rc::new(Cell::new(initial_value)),
        }
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
        self.last_synced.set(value);
        let value = value.to_string();
        self.state.update(cx, |input, cx| {
            if input.value().as_ref() != value {
                input.set_text(value, cx);
            }
        });
    }

    /// Background task results must not erase a partially edited number.
    /// Preset buttons and explicit commits continue using `sync`, which is authoritative.
    pub(crate) fn sync_if_pristine<V: 'static>(&self, value: u32, cx: &mut Context<V>) {
        let current = self.state.read(cx).value();
        if is_pristine_numeric_input(current.as_ref(), self.last_synced.get()) {
            self.sync(value, cx);
        }
    }
}

/// Shared large hit target for increment/decrement controls. Keep the
/// clickable area independent of the glyph's visual size and avoid inline
/// ad-hoc button sizing in individual converter panels.
pub(crate) fn numeric_step_button(
    colors: &ThemeColors,
    label: impl Into<SharedString>,
    enabled: bool,
) -> Div {
    div()
        .h(px(36.0))
        .min_w(px(38.0))
        .px(px(10.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .text_size(px(16.0))
        .text_color(if enabled { colors.text_primary } else { colors.text_secondary })
        .opacity(if enabled { 1.0 } else { 0.45 })
        .child(label.into())
}

/// Shared compact dimension editor: one input-first stepper and one independent slider
/// track. The caller owns range checks and conversion callbacks; the component owns spacing.
pub(crate) fn numeric_dimension_editor(
    field: &NumericField,
    colors: &ThemeColors,
    unit: impl Into<SharedString>,
    decrease: impl IntoElement,
    increase: impl IntoElement,
    slider: impl IntoElement,
) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(decrease)
                .child(field.control())
                .child(increase)
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(colors.text_secondary)
                        .child(unit.into()),
                ),
        )
        .child(div().w_full().min_w(px(0.0)).child(slider))
}

fn is_pristine_numeric_input(text: &str, synced_value: u32) -> bool {
    text == synced_value.to_string()
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
    use super::{is_pristine_numeric_input, parse_bounded_u32};

    #[test]
    fn accepts_exact_dimensions_in_range() {
        assert_eq!(parse_bounded_u32(" 137 ", 1, 512), Some(137));
        assert_eq!(parse_bounded_u32("64", 1, 64), Some(64));
    }

    #[test]
    fn background_sync_preserves_partial_edits() {
        assert!(is_pristine_numeric_input("128", 128));
        assert!(!is_pristine_numeric_input("12", 128));
        assert!(!is_pristine_numeric_input("", 128));
        assert!(!is_pristine_numeric_input("0128", 128));
    }

    #[test]
    fn rejects_invalid_values_without_clamping() {
        for value in ["", "0", "-1", "2.5", "65", "99999999999999", "1x"] {
            assert_eq!(parse_bounded_u32(value, 1, 64), None);
        }
    }
}
