use crate::{
    App, Bounds, DecorationRun, LayoutId, Pixels, Point, SharedString, Size, TextOverflow, TextRun,
    TextStyle, WhiteSpace, Window, WrappedLine, WrappedLineLayout,
};
use smallvec::SmallVec;
use std::{
    cell::RefCell,
    cmp,
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
    paint_key: u64,
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
        let cache_key = text_layout_cache_key(&text, &runs, &text_style, font_size, line_height);
        let paint_key = text_layout_paint_key(&runs);

        // Most UI text never uses truncation. When only paint decorations change, refresh the
        // DecorationRun sidecar in place and keep the measured/wrapped/shaped geometry intact.
        // This makes theme/color animation skip the measured-layout closure entirely instead of
        // calling shape_text again just to recover the same Arc<WrappedLineLayout> from its cache.
        // Truncation remains conservative because its synthetic suffix can change run boundaries.
        let mut paint_refresh_failed = false;
        if text_style.text_overflow.is_none() {
            let mut element_state = self.0.borrow_mut();
            if let Some(text_layout) = element_state.as_mut()
                && text_layout.cache_key == cache_key
                && text_layout.paint_key != paint_key
                && text_layout.size.is_some()
            {
                if refresh_cached_decoration_runs(&text, &mut text_layout.lines, &runs) {
                    text_layout.paint_key = paint_key;
                } else {
                    paint_refresh_failed = true;
                }
            }
        }

        let measurement_key = text_layout_measurement_fingerprint(
            cache_key,
            paint_key,
            text_style.text_overflow.is_some() || paint_refresh_failed,
        );

        window.request_measured_layout_with_fingerprint(Default::default(), measurement_key, {
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
                    && text_layout.paint_key == paint_key
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

                // Geometry changes and truncation-dependent paint changes reach this path. The
                // LineLayoutCache still reuses shaped glyph geometry when its font identity matches.
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
                        paint_key,
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
                    paint_key,
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

struct DecorationRunCursor<'a> {
    runs: &'a [TextRun],
    index: usize,
    offset: usize,
}

impl<'a> DecorationRunCursor<'a> {
    fn new(runs: &'a [TextRun]) -> Self {
        Self {
            runs,
            index: 0,
            offset: 0,
        }
    }

    fn append(&mut self, mut len: usize, target: &mut SmallVec<[DecorationRun; 32]>) -> bool {
        while len > 0 {
            self.skip_empty_runs();
            let Some(run) = self.runs.get(self.index) else {
                return false;
            };
            let available = run.len.saturating_sub(self.offset);
            if available == 0 {
                self.index += 1;
                self.offset = 0;
                continue;
            }
            let take = cmp::min(len, available);
            push_cached_decoration_run(target, run, take);
            self.offset += take;
            len -= take;
            if self.offset == run.len {
                self.index += 1;
                self.offset = 0;
            }
        }
        true
    }

    fn skip(&mut self, mut len: usize) -> bool {
        while len > 0 {
            self.skip_empty_runs();
            let Some(run) = self.runs.get(self.index) else {
                return false;
            };
            let available = run.len.saturating_sub(self.offset);
            if available == 0 {
                self.index += 1;
                self.offset = 0;
                continue;
            }
            let take = cmp::min(len, available);
            self.offset += take;
            len -= take;
            if self.offset == run.len {
                self.index += 1;
                self.offset = 0;
            }
        }
        true
    }

    fn is_exhausted(&mut self) -> bool {
        self.skip_empty_runs();
        self.index >= self.runs.len()
    }

    fn skip_empty_runs(&mut self) {
        while self
            .runs
            .get(self.index)
            .is_some_and(|run| run.len == self.offset)
        {
            self.index += 1;
            self.offset = 0;
        }
    }
}

fn refresh_cached_decoration_runs(
    text: &SharedString,
    lines: &mut [WrappedLine],
    runs: &[TextRun],
) -> bool {
    let mut source_lines = text.split('\n').peekable();
    let mut cursor = DecorationRunCursor::new(runs);

    for line in lines {
        let Some(source_line) = source_lines.next() else {
            return false;
        };
        if line.text.as_ref() != source_line {
            return false;
        }

        line.decoration_runs.clear();
        if !cursor.append(source_line.len(), &mut line.decoration_runs) {
            return false;
        }

        if source_lines.peek().is_some() && !cursor.skip(1) {
            return false;
        }
    }

    source_lines.next().is_none() && cursor.is_exhausted()
}

fn push_cached_decoration_run(
    decoration_runs: &mut SmallVec<[DecorationRun; 32]>,
    run: &TextRun,
    len: usize,
) {
    if let Some(last_run) = decoration_runs.last_mut()
        && last_run.color == run.color
        && last_run.underline == run.underline
        && last_run.strikethrough == run.strikethrough
        && last_run.background_color == run.background_color
        && last_run.background_corner_radius == run.background_corner_radius
        && last_run.background_padding == run.background_padding
    {
        last_run.len += len as u32;
    } else {
        decoration_runs.push(DecorationRun {
            len: len as u32,
            color: run.color,
            background_color: run.background_color,
            background_corner_radius: run.background_corner_radius,
            background_padding: run.background_padding,
            underline: run.underline,
            strikethrough: run.strikethrough,
        });
    }
}

fn text_layout_cache_key(
    text: &SharedString,
    runs: &[TextRun],
    text_style: &TextStyle,
    font_size: Pixels,
    line_height: Pixels,
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
    std::mem::discriminant(&text_style.white_space).hash(&mut hasher);
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

fn text_layout_paint_key(runs: &[TextRun]) -> u64 {
    let mut hasher = DefaultHasher::new();
    runs.len().hash(&mut hasher);
    for run in runs {
        run.len.hash(&mut hasher);
        run.color.hash(&mut hasher);
        run.background_color.hash(&mut hasher);
        run.background_corner_radius.hash(&mut hasher);
        if let Some(padding) = run.background_padding {
            1u8.hash(&mut hasher);
            padding.top.hash(&mut hasher);
            padding.right.hash(&mut hasher);
            padding.bottom.hash(&mut hasher);
            padding.left.hash(&mut hasher);
        } else {
            0u8.hash(&mut hasher);
        }
        run.underline.hash(&mut hasher);
        run.strikethrough.hash(&mut hasher);
    }
    hasher.finish()
}

fn text_layout_measurement_key(cache_key: u64, paint_key: u64) -> u64 {
    let mut hasher = DefaultHasher::new();
    cache_key.hash(&mut hasher);
    paint_key.hash(&mut hasher);
    hasher.finish()
}

fn text_layout_measurement_fingerprint(
    cache_key: u64,
    paint_key: u64,
    paint_requires_measurement: bool,
) -> u64 {
    if paint_requires_measurement {
        text_layout_measurement_key(cache_key, paint_key)
    } else {
        cache_key
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TextLayout, text_layout_cache_key, text_layout_measurement_fingerprint,
        text_layout_paint_key,
    };
    use crate::element::text::StyledText;
    use crate::{
        AvailableSpace, IntoElement, ParentElement as _, Render, SharedString, TestAppContext,
        TextStyle, TextOverflow, Window, div, point, px, rgb, size,
    };
    use std::sync::Arc;

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
    fn paint_only_changes_keep_geometry_measurement_identity() {
        let text = SharedString::from("paint cache");
        let text_style = TextStyle::default();
        let font_size = px(16.);
        let line_height = px(24.);
        let mut first = text_style.to_run(text.len());
        first.color = rgb(0xff0000).into();
        let mut second = first.clone();
        second.color = rgb(0x0000ff).into();

        let first_runs = [first];
        let second_runs = [second];
        let first_geometry =
            text_layout_cache_key(&text, &first_runs, &text_style, font_size, line_height);
        let second_geometry =
            text_layout_cache_key(&text, &second_runs, &text_style, font_size, line_height);
        let first_paint = text_layout_paint_key(&first_runs);
        let second_paint = text_layout_paint_key(&second_runs);

        assert_eq!(first_geometry, second_geometry);
        assert_ne!(first_paint, second_paint);
        assert_eq!(
            text_layout_measurement_fingerprint(first_geometry, first_paint, false),
            text_layout_measurement_fingerprint(second_geometry, second_paint, false)
        );
    }

    #[test]
    fn truncation_keeps_paint_in_measurement_identity() {
        let text = SharedString::from("truncate cache");
        let mut text_style = TextStyle::default();
        text_style.text_overflow = Some(TextOverflow::Truncate("…".into()));
        let font_size = px(16.);
        let line_height = px(24.);
        let mut first = text_style.to_run(text.len());
        first.color = rgb(0xff0000).into();
        let mut second = first.clone();
        second.color = rgb(0x0000ff).into();
        let first_runs = [first];
        let second_runs = [second];
        let geometry = text_layout_cache_key(&text, &first_runs, &text_style, font_size, line_height);
        let first_paint = text_layout_paint_key(&first_runs);
        let second_paint = text_layout_paint_key(&second_runs);

        assert_ne!(
            text_layout_measurement_fingerprint(geometry, first_paint, true),
            text_layout_measurement_fingerprint(geometry, second_paint, true)
        );
    }

    #[gpui::test]
    fn paint_change_refreshes_decorations_without_replacing_shaped_layout(
        cx: &mut TestAppContext,
    ) {
        let (_, visual) = cx.add_window_view(|_, _| crate::Empty);
        let layout = TextLayout::default();
        let available_space = size(
            AvailableSpace::Definite(px(240.)),
            AvailableSpace::MinContent,
        );
        let text = SharedString::from("cached color");
        let text_style = TextStyle::default();
        let mut first_run = text_style.to_run(text.len());
        first_run.color = rgb(0xff0000).into();

        let mut first = StyledText::new(text.clone()).with_runs(vec![first_run]);
        first.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| first);

        let (first_layout, first_color) = {
            let state = layout.0.borrow();
            let line = &state.as_ref().unwrap().lines[0];
            (line.layout.clone(), line.decoration_runs[0].color)
        };
        assert_eq!(first_color, rgb(0xff0000).into());

        let mut second_run = text_style.to_run(text.len());
        second_run.color = rgb(0x0000ff).into();
        let mut second = StyledText::new(text).with_runs(vec![second_run]);
        second.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| second);

        let state = layout.0.borrow();
        let line = &state.as_ref().unwrap().lines[0];
        assert_eq!(line.decoration_runs[0].color, rgb(0x0000ff).into());
        assert!(Arc::ptr_eq(&first_layout, &line.layout));
    }

    #[gpui::test]
    fn multiline_paint_change_refreshes_decorations_in_place(cx: &mut TestAppContext) {
        let (_, visual) = cx.add_window_view(|_, _| crate::Empty);
        let layout = TextLayout::default();
        let available_space = size(
            AvailableSpace::Definite(px(240.)),
            AvailableSpace::MinContent,
        );
        let text = SharedString::from("first\nsecond");
        let text_style = TextStyle::default();
        let mut first_run = text_style.to_run(text.len());
        first_run.color = rgb(0xff0000).into();
        let mut first = StyledText::new(text.clone()).with_runs(vec![first_run]);
        first.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| first);

        let first_layouts = {
            let state = layout.0.borrow();
            state
                .as_ref()
                .unwrap()
                .lines
                .iter()
                .map(|line| line.layout.clone())
                .collect::<Vec<_>>()
        };

        let mut second_run = text_style.to_run(text.len());
        second_run.color = rgb(0x0000ff).into();
        let mut second = StyledText::new(text).with_runs(vec![second_run]);
        second.layout = layout.clone();
        visual.draw(point(px(0.), px(0.)), available_space, |_, _| second);

        let state = layout.0.borrow();
        let lines = &state.as_ref().unwrap().lines;
        assert_eq!(lines.len(), 2);
        for (line, first_layout) in lines.iter().zip(first_layouts) {
            assert_eq!(line.decoration_runs[0].color, rgb(0x0000ff).into());
            assert!(Arc::ptr_eq(&first_layout, &line.layout));
        }
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
