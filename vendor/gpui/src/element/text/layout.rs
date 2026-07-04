use crate::{
    App, Bounds, LayoutId, Pixels, Point, SharedString, Size, TextOverflow, TextRun, TextStyle,
    WhiteSpace, Window, WrappedLine, WrappedLineLayout,
};
use smallvec::SmallVec;
use std::{
    cell::RefCell,
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    rc::Rc,
    sync::Arc,
};
use util::ResultExt;

/// The Layout for TextElement. This can be used to map indices to pixels and vice versa.
#[derive(Default, Clone)]
pub struct TextLayout(Rc<RefCell<Option<TextLayoutInner>>>);

struct TextLayoutInner {
    cache_key: u64,
    len: usize,
    lines: SmallVec<[WrappedLine; 1]>,
    line_height: Pixels,
    wrap_width: Option<Pixels>,
    size: Option<Size<Pixels>>,
    bounds: Option<Bounds<Pixels>>,
}

impl TextLayout {
    pub(super) fn layout(
        &self,
        text: SharedString,
        runs: Option<Vec<TextRun>>,
        window: &mut Window,
        _: &mut App,
    ) -> LayoutId {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let line_height = text_style
            .line_height
            .to_pixels(font_size.into(), window.rem_size());

        let mut runs = if let Some(runs) = runs {
            runs
        } else {
            vec![text_style.to_run(text.len())]
        };
        let cache_key = text_layout_cache_key(
            &text,
            &runs,
            &text_style,
            font_size,
            line_height,
            window.scale_factor(),
        );

        window.request_measured_layout_with_fingerprint(Default::default(), cache_key, {
            let element_state = self.clone();

            move |known_dimensions, available_space, window, cx| {
                let wrap_width = if text_style.white_space == WhiteSpace::Normal {
                    known_dimensions.width.or(match available_space.width {
                        crate::AvailableSpace::Definite(x) => Some(x),
                        _ => None,
                    })
                } else {
                    None
                };

                let (truncate_width, truncation_suffix) =
                    if let Some(text_overflow) = text_style.text_overflow.clone() {
                        let width = known_dimensions.width.or(match available_space.width {
                            crate::AvailableSpace::Definite(x) => match text_style.line_clamp {
                                Some(max_lines) => Some(x * max_lines),
                                None => Some(x),
                            },
                            _ => None,
                        });

                        match text_overflow {
                            TextOverflow::Truncate(s) => (width, s),
                        }
                    } else {
                        (None, "".into())
                    };

                if let Some(text_layout) = element_state.0.borrow().as_ref()
                    && text_layout.cache_key == cache_key
                    && text_layout.size.is_some()
                    && wrap_width == text_layout.wrap_width
                {
                    return text_layout.size.unwrap();
                }

                let mut line_wrapper = cx.text_system().line_wrapper(text_style.font(), font_size);
                let text = if let Some(truncate_width) = truncate_width {
                    line_wrapper.truncate_line(
                        text.clone(),
                        truncate_width,
                        &truncation_suffix,
                        &mut runs,
                    )
                } else {
                    text.clone()
                };
                let len = text.len();

                let Some(lines) = window
                    .text_system()
                    .shape_text(
                        text,
                        font_size,
                        &runs,
                        wrap_width,            // Wrap if we know the width.
                        text_style.line_clamp, // Limit the number of lines if line_clamp is set.
                    )
                    .log_err()
                else {
                    element_state.0.borrow_mut().replace(TextLayoutInner {
                        cache_key,
                        lines: Default::default(),
                        len: 0,
                        line_height,
                        wrap_width,
                        size: Some(Size::default()),
                        bounds: None,
                    });
                    return Size::default();
                };

                let mut size: Size<Pixels> = Size::default();
                for line in &lines {
                    let line_size = line.size(line_height);
                    size.height += line_size.height;
                    size.width = size.width.max(line_size.width).ceil();
                }

                element_state.0.borrow_mut().replace(TextLayoutInner {
                    cache_key,
                    lines,
                    len,
                    line_height,
                    wrap_width,
                    size: Some(size),
                    bounds: None,
                });

                size
            }
        })
    }

    pub(super) fn prepaint(&self, bounds: Bounds<Pixels>, text: &str) {
        let mut element_state = self.0.borrow_mut();
        let element_state = element_state.as_mut().unwrap_or_else(|| {
            panic!("measurement has not been performed on {text}");
        });
        element_state.bounds = Some(bounds);
    }

    pub(super) fn paint(&self, text: &str, window: &mut Window, cx: &mut App) {
        let element_state = self.0.borrow();
        let element_state = element_state.as_ref().unwrap_or_else(|| {
            panic!("measurement has not been performed on {text}");
        });
        let bounds = element_state.bounds.unwrap_or_else(|| {
            panic!("prepaint has not been performed on {text}");
        });

        let line_height = element_state.line_height;
        let mut line_origin = bounds.origin;
        let text_style = window.text_style();
        for line in &element_state.lines {
            line.paint_background(
                line_origin,
                line_height,
                text_style.text_align,
                Some(bounds),
                window,
                cx,
            )
            .log_err();
            line.paint(
                line_origin,
                line_height,
                text_style.text_align,
                Some(bounds),
                window,
                cx,
            )
            .log_err();
            line_origin.y += line.size(line_height).height;
        }
    }

    /// Get the byte index into the input of the pixel position.
    pub fn index_for_position(&self, mut position: Point<Pixels>) -> Result<usize, usize> {
        let element_state = self.0.borrow();
        let element_state = element_state
            .as_ref()
            .expect("measurement has not been performed");
        let bounds = element_state
            .bounds
            .expect("prepaint has not been performed");

        if position.y < bounds.top() {
            return Err(0);
        }

        let line_height = element_state.line_height;
        let mut line_origin = bounds.origin;
        let mut line_start_ix = 0;
        for line in &element_state.lines {
            let line_bottom = line_origin.y + line.size(line_height).height;
            if position.y > line_bottom {
                line_origin.y = line_bottom;
                line_start_ix += line.len() + 1;
            } else {
                let position_within_line = position - line_origin;
                match line.index_for_position(position_within_line, line_height) {
                    Ok(index_within_line) => return Ok(line_start_ix + index_within_line),
                    Err(index_within_line) => return Err(line_start_ix + index_within_line),
                }
            }
        }

        Err(line_start_ix.saturating_sub(1))
    }

    /// Get the pixel position for the given byte index.
    pub fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        let element_state = self.0.borrow();
        let element_state = element_state
            .as_ref()
            .expect("measurement has not been performed");
        let bounds = element_state
            .bounds
            .expect("prepaint has not been performed");
        let line_height = element_state.line_height;

        let mut line_origin = bounds.origin;
        let mut line_start_ix = 0;

        for line in &element_state.lines {
            let line_end_ix = line_start_ix + line.len();
            if index < line_start_ix {
                break;
            } else if index > line_end_ix {
                line_origin.y += line.size(line_height).height;
                line_start_ix = line_end_ix + 1;
                continue;
            } else {
                let ix_within_line = index - line_start_ix;
                return Some(line_origin + line.position_for_index(ix_within_line, line_height)?);
            }
        }

        None
    }

    /// Retrieve the layout for the line containing the given byte index.
    pub fn line_layout_for_index(&self, index: usize) -> Option<Arc<WrappedLineLayout>> {
        let element_state = self.0.borrow();
        let element_state = element_state
            .as_ref()
            .expect("measurement has not been performed");
        let bounds = element_state
            .bounds
            .expect("prepaint has not been performed");
        let line_height = element_state.line_height;

        let mut line_origin = bounds.origin;
        let mut line_start_ix = 0;

        for line in &element_state.lines {
            let line_end_ix = line_start_ix + line.len();
            if index < line_start_ix {
                break;
            } else if index > line_end_ix {
                line_origin.y += line.size(line_height).height;
                line_start_ix = line_end_ix + 1;
                continue;
            } else {
                return Some(line.layout.clone());
            }
        }

        None
    }

    /// The bounds of this layout.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.0.borrow().as_ref().unwrap().bounds.unwrap()
    }

    /// The line height for this layout.
    pub fn line_height(&self) -> Pixels {
        self.0.borrow().as_ref().unwrap().line_height
    }

    /// The UTF-8 length of the underlying text.
    pub fn len(&self) -> usize {
        self.0.borrow().as_ref().unwrap().len
    }

    /// The text for this layout.
    pub fn text(&self) -> String {
        self.0
            .borrow()
            .as_ref()
            .unwrap()
            .lines
            .iter()
            .map(|s| s.text.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The text for this layout (with soft-wraps as newlines)
    pub fn wrapped_text(&self) -> String {
        let mut lines = Vec::new();
        for wrapped in self.0.borrow().as_ref().unwrap().lines.iter() {
            let mut seen = 0;
            for boundary in wrapped.layout.wrap_boundaries.iter() {
                let index = wrapped.layout.unwrapped_layout.runs[boundary.run_ix].glyphs
                    [boundary.glyph_ix]
                    .index;

                lines.push(wrapped.text[seen..index].to_string());
                seen = index;
            }
            lines.push(wrapped.text[seen..].to_string());
        }

        lines.join("\n")
    }
}

fn text_layout_cache_key(
    text: &SharedString,
    runs: &[TextRun],
    text_style: &TextStyle,
    font_size: Pixels,
    line_height: Pixels,
    scale_factor: f32,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    runs.len().hash(&mut hasher);
    for run in runs {
        run.len.hash(&mut hasher);
        run.font.hash(&mut hasher);
    }
    text_style.font().hash(&mut hasher);
    font_size.0.to_bits().hash(&mut hasher);
    line_height.0.to_bits().hash(&mut hasher);
    scale_factor.to_bits().hash(&mut hasher);
    std::mem::discriminant(&text_style.white_space).hash(&mut hasher);
    std::mem::discriminant(&text_style.text_align).hash(&mut hasher);
    text_style.line_clamp.hash(&mut hasher);
    if let Some(text_overflow) = text_style.text_overflow.as_ref() {
        1u8.hash(&mut hasher);
        match text_overflow {
            TextOverflow::Truncate(suffix) => suffix.hash(&mut hasher),
        }
    } else {
        0u8.hash(&mut hasher);
    }
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::{TextLayout, text_layout_cache_key};
    use crate::element::text::StyledText;
    use crate::{
        AvailableSpace, IntoElement, ParentElement as _, Render, SharedString, TestAppContext,
        TextStyle, Window, div, point, px, size,
    };

    #[gpui::test]
    fn text_layout_cache_invalidates_when_text_changes(cx: &mut TestAppContext) {
        let (_, visual) = cx.add_window_view(|_, _| crate::Empty);
        let layout = TextLayout::default();
        let available_space = size(
            AvailableSpace::Definite(px(60.)),
            AvailableSpace::MinContent,
        );

        let mut short = StyledText::new("short");
        short.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| short);
        let short_size = layout.0.borrow().as_ref().unwrap().size.unwrap();

        let mut long =
            StyledText::new("short short short short short short short short short short short");
        long.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| long);
        let long_size = layout.0.borrow().as_ref().unwrap().size.unwrap();

        assert!(
            long_size.height > short_size.height,
            "changed text must be measured again instead of reusing stale height"
        );
    }

    #[test]
    fn text_layout_cache_key_changes_with_scale_factor() {
        let text = SharedString::from("scaled text");
        let text_style = TextStyle::default();
        let font_size = px(16.);
        let line_height = px(24.);
        let runs = vec![text_style.to_run(text.len())];

        let normal_scale_key =
            text_layout_cache_key(&text, &runs, &text_style, font_size, line_height, 1.0);
        let high_scale_key =
            text_layout_cache_key(&text, &runs, &text_style, font_size, line_height, 1.25);

        assert_ne!(
            normal_scale_key, high_scale_key,
            "text layout cache must be invalidated when the raster scale changes"
        );
    }

    #[gpui::test]
    fn text_layout_remains_measured_across_retained_layout_draws(cx: &mut TestAppContext) {
        let layout = TextLayout::default();
        let available_space = size(
            AvailableSpace::Definite(px(240.)),
            AvailableSpace::MinContent,
        );
        let first = StyledText::new("common.not_installed");
        let second = StyledText::new("common.not_installed");

        let (_, visual) = cx.add_window_view(|_, _| crate::Empty);

        let mut first = first;
        first.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| first);

        let first_size = layout.0.borrow().as_ref().and_then(|state| state.size);
        assert!(first_size.is_some());

        let mut second = second;
        second.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| second);

        let second_size = layout.0.borrow().as_ref().and_then(|state| state.size);
        assert!(second_size.is_some());
    }

    #[gpui::test]
    fn fresh_text_element_measures_after_retained_layout_cache_hit(cx: &mut TestAppContext) {
        struct TestView;

        impl Render for TestView {
            fn render(
                &mut self,
                _window: &mut Window,
                _cx: &mut crate::Context<Self>,
            ) -> impl IntoElement {
                div().child("启动")
            }
        }

        let (_view, visual) = cx.add_window_view(|_, _| TestView);

        visual.update(|window, cx| {
            window.draw(cx).clear();
            window.draw(cx).clear();
        });
    }
}
