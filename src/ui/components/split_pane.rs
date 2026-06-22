use gpui::{CursorStyle, Div, Hsla, InteractiveElement, Pixels, Styled, div, hsla, px};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitPaneAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug)]
pub struct SplitPaneLimits {
    pub min: f32,
    pub max: f32,
}

impl SplitPaneLimits {
    #[must_use]
    pub const fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    #[must_use]
    pub fn clamp(self, value: f32) -> f32 {
        value.clamp(self.min, self.max.max(self.min))
    }
}

#[must_use]
pub fn clamp_split_size(value: f32, limits: SplitPaneLimits, available: f32) -> f32 {
    let max = limits.max.min(available.max(limits.min));
    value.clamp(limits.min, max.max(limits.min))
}

#[must_use]
pub fn split_handle(axis: SplitPaneAxis, color: Hsla) -> Div {
    let hover = hsla(color.h, color.s, (color.l + 0.12).min(1.0), 0.78);
    let base = hsla(color.h, color.s, color.l, 0.38);
    match axis {
        SplitPaneAxis::Horizontal => div()
            .w(px(6.0))
            .h_full()
            .flex_none()
            .cursor(CursorStyle::ResizeColumn)
            .bg(base)
            .hover(|style| style.bg(hover)),
        SplitPaneAxis::Vertical => div()
            .w_full()
            .h(px(6.0))
            .flex_none()
            .cursor(CursorStyle::ResizeRow)
            .bg(base)
            .hover(|style| style.bg(hover)),
    }
}

#[must_use]
pub fn splitter_line(axis: SplitPaneAxis, color: Hsla) -> Div {
    match axis {
        SplitPaneAxis::Horizontal => div()
            .w(px(1.0))
            .h_full()
            .flex_none()
            .bg(hsla(color.h, color.s, color.l, 0.42)),
        SplitPaneAxis::Vertical => div()
            .w_full()
            .h(px(1.0))
            .flex_none()
            .bg(hsla(color.h, color.s, color.l, 0.42)),
    }
}

#[allow(dead_code)]
#[must_use]
pub fn pixels_to_f32(value: Pixels) -> f32 {
    value / px(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_split_size_respects_available_width() {
        let limits = SplitPaneLimits::new(300.0, 900.0);
        assert_eq!(clamp_split_size(700.0, limits, 500.0), 500.0);
        assert_eq!(clamp_split_size(120.0, limits, 500.0), 300.0);
    }
}
