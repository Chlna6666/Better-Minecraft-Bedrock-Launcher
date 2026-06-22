use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, lerp_theme_colors};
use gpui::{
    App, Bounds, BoxShadow, ClipboardItem, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels,
    Point, Render, RenderOnce, ShapedLine, SharedString, Style, Styled, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, hsla, point, px, relative, size,
};
use std::ops::Range;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(1000);
const CURSOR_VISIBLE_WINDOW: Duration = Duration::from_millis(530);

actions!(
    input,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        Enter,
    ]
);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Input")),
        KeyBinding::new("delete", Delete, Some("Input")),
        KeyBinding::new("left", Left, Some("Input")),
        KeyBinding::new("right", Right, Some("Input")),
        KeyBinding::new("shift-left", SelectLeft, Some("Input")),
        KeyBinding::new("shift-right", SelectRight, Some("Input")),
        KeyBinding::new("home", Home, Some("Input")),
        KeyBinding::new("end", End, Some("Input")),
        KeyBinding::new("enter", Enter, Some("Input")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Input")),
        KeyBinding::new("ctrl-v", Paste, Some("Input")),
        KeyBinding::new("ctrl-c", Copy, Some("Input")),
        KeyBinding::new("ctrl-x", Cut, Some("Input")),
    ]);
}

#[derive(Clone)]
pub enum InputEvent {
    Change,
    PressEnter { secondary: bool },
    Focus,
    Blur,
}

#[derive(Clone, Copy, Default)]
pub enum InputSize {
    Small,
    #[default]
    Medium,
}

#[derive(Clone, Copy)]
struct InputMetrics {
    height: f32,
    radius: f32,
    gap: f32,
    padding_x: f32,
    clear_slot: f32,
    clear_button: f32,
    clear_text_size: f32,
}

impl InputSize {
    fn metrics(self) -> InputMetrics {
        match self {
            Self::Small => InputMetrics {
                height: 32.0,
                radius: 10.0,
                gap: 2.0,
                padding_x: 4.0,
                clear_slot: 14.0,
                clear_button: 14.0,
                clear_text_size: 11.5,
            },
            Self::Medium => InputMetrics {
                height: 38.0,
                radius: 12.0,
                gap: 3.0,
                padding_x: 5.0,
                clear_slot: 16.0,
                clear_button: 14.0,
                clear_text_size: 12.5,
            },
        }
    }
}

pub struct InputState {
    focus_handle: FocusHandle,
    value: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_layout_display_text: SharedString,
    last_layout_marked_range: Option<Range<usize>>,
    last_layout_font_size: Option<Pixels>,
    last_layout_placeholder_active: bool,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    cursor_blink_started_at: Option<Instant>,
    cursor_blink_task_armed: bool,
    cursor_visible_last_frame: bool,
}

impl EventEmitter<InputEvent> for InputState {}

impl InputState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle().tab_stop(true);
        let _ = cx.on_focus(&focus_handle, window, |this, _, cx| {
            this.reset_cursor_blink();
            cx.emit(InputEvent::Focus);
            cx.notify();
            let _ = this;
        });
        let _ = cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.is_selecting = false;
            this.cursor_blink_task_armed = false;
            this.cursor_visible_last_frame = false;
            cx.emit(InputEvent::Blur);
            cx.notify();
        });

        Self {
            focus_handle,
            value: SharedString::from(""),
            placeholder: SharedString::from(""),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_layout_display_text: SharedString::from(""),
            last_layout_marked_range: None,
            last_layout_font_size: None,
            last_layout_placeholder_active: false,
            last_bounds: None,
            is_selecting: false,
            cursor_blink_started_at: None,
            cursor_blink_task_armed: false,
            cursor_visible_last_frame: false,
        }
    }

    pub fn value(&self) -> SharedString {
        self.value.clone()
    }

    pub fn set_value(
        &mut self,
        value: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.into();
        let end = value.len();
        if self.value == value
            && self.selected_range == (end..end)
            && !self.selection_reversed
            && self.marked_range.is_none()
        {
            return;
        }
        self.value = value;
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn set_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.placeholder == placeholder {
            return;
        }
        self.placeholder = placeholder;
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn update_selection_state(
        &mut self,
        selected_range: Range<usize>,
        selection_reversed: bool,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range == selected_range && self.selection_reversed == selection_reversed {
            return;
        }
        self.selected_range = selected_range;
        self.selection_reversed = selection_reversed;
        self.reset_cursor_blink();
        cx.notify();
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.update_selection_state(offset..offset, false, cx);
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let mut selected_range = self.selected_range.clone();
        let mut selection_reversed = self.selection_reversed;
        if selection_reversed {
            selected_range.start = offset;
        } else {
            selected_range.end = offset;
        }
        if selected_range.end < selected_range.start {
            selection_reversed = !selection_reversed;
            selected_range = selected_range.end..selected_range.start;
        }
        self.update_selection_state(selected_range, selection_reversed, cx);
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.value.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref())
        else {
            return 0;
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.value.len();
        }
        line.closest_index_for_x(position.x - bounds.left())
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.value
            .as_ref()
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.value
            .as_ref()
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.value.len())
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.value.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += character.len_utf16();
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for character in self.value.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += character.len_utf8();
            utf16_offset += character.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn build_replaced_value(&self, range: &Range<usize>, new_text: &str) -> SharedString {
        let mut value =
            String::with_capacity(self.value.len() - (range.end - range.start) + new_text.len());
        value.push_str(&self.value[..range.start]);
        value.push_str(new_text);
        value.push_str(&self.value[range.end..]);
        SharedString::from(value)
    }

    fn can_reuse_last_layout(
        &self,
        display_text: &SharedString,
        font_size: Pixels,
        placeholder_active: bool,
    ) -> bool {
        self.last_layout.is_some()
            && self.last_layout_display_text == *display_text
            && self.last_layout_font_size == Some(font_size)
            && self.last_layout_placeholder_active == placeholder_active
            && self.last_layout_marked_range == self.marked_range
    }

    fn reset_cursor_blink(&mut self) {
        self.cursor_blink_started_at = Some(Instant::now());
        self.cursor_blink_task_armed = false;
        self.cursor_visible_last_frame = true;
    }

    fn cursor_visible_now(&self, is_focused: bool) -> bool {
        if !is_focused {
            return false;
        }

        self.cursor_blink_started_at.is_none_or(|started_at| {
            let elapsed = Instant::now().saturating_duration_since(started_at);
            elapsed.is_zero()
                || elapsed.as_millis() % CURSOR_BLINK_PERIOD.as_millis()
                    < CURSOR_VISIBLE_WINDOW.as_millis()
        })
    }

    fn next_cursor_blink_delay(&self) -> Duration {
        let Some(started_at) = self.cursor_blink_started_at else {
            return CURSOR_VISIBLE_WINDOW;
        };
        let elapsed_ms = Instant::now()
            .saturating_duration_since(started_at)
            .as_millis()
            % CURSOR_BLINK_PERIOD.as_millis();
        let next_edge_ms = if elapsed_ms < CURSOR_VISIBLE_WINDOW.as_millis() {
            CURSOR_VISIBLE_WINDOW.as_millis() - elapsed_ms
        } else {
            CURSOR_BLINK_PERIOD.as_millis() - elapsed_ms
        };
        Duration::from_millis(next_edge_ms.max(16) as u64)
    }

    fn arm_cursor_blink_timer(
        state: Entity<InputState>,
        visible: bool,
        window: &Window,
        cx: &mut App,
    ) {
        let (focused, delay) = {
            let input = state.read(cx);
            if input.cursor_blink_task_armed {
                (false, Duration::ZERO)
            } else {
                (
                    input.focus_handle.is_focused(window),
                    input.next_cursor_blink_delay(),
                )
            }
        };
        if !focused {
            return;
        }

        state.update(cx, |input, _| {
            input.cursor_blink_task_armed = true;
            input.cursor_visible_last_frame = visible;
        });

        let handle = window.window_handle();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(delay).await;
            let _ = handle.update(cx, move |_, window, cx| {
                let _ = state.update(cx, |input, cx| {
                    input.cursor_blink_task_armed = false;
                    if input.focus_handle.is_focused(window)
                        && input.cursor_visible_now(true) != visible
                    {
                        cx.notify();
                    }
                });
            });
        })
        .detach();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.value.len();
        self.selection_reversed = false;
        self.reset_cursor_blink();
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.value.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.value[self.selected_range.clone()].to_string(),
        ));
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.value[self.selected_range.clone()].to_string(),
        ));
        self.replace_text_in_range(None, "", window, cx);
    }

    fn enter(&mut self, _: &Enter, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::PressEnter { secondary: false });
    }

    fn on_key_down(&mut self, _: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.reset_cursor_blink();
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = true;
        self.reset_cursor_blink();
        self.focus_handle.focus(window);
        if event.modifiers.shift {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        } else {
            self.move_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }
}

impl EntityInputHandler for InputState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.value[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|value| self.range_from_utf16(value))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        let end = range.start + new_text.len();
        let next_value = self.build_replaced_value(&range, new_text);
        let next_selected_range = end..end;
        if self.value == next_value
            && self.selected_range == next_selected_range
            && !self.selection_reversed
            && self.marked_range.is_none()
        {
            return;
        }

        self.value = next_value;
        self.selected_range = next_selected_range;
        self.selection_reversed = false;
        self.marked_range = None;
        self.reset_cursor_blink();
        tracing::debug!(
            range_start = range.start,
            range_end = range.end,
            inserted_len = new_text.len(),
            value_len = self.value.len(),
            "input text changed"
        );
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|value| self.range_from_utf16(value))
            .or(self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        let next_value = self.build_replaced_value(&range, new_text);
        let next_marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        let next_selected_range = new_selected_range_utf16
            .as_ref()
            .map(|value| self.range_from_utf16(value))
            .map(|selected| selected.start + range.start..selected.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        if self.value == next_value
            && self.selected_range == next_selected_range
            && !self.selection_reversed
            && self.marked_range == next_marked_range
        {
            return;
        }

        self.value = next_value;
        self.marked_range = next_marked_range;
        self.selected_range = next_selected_range;
        self.selection_reversed = false;
        self.reset_cursor_blink();
        tracing::debug!(
            range_start = range.start,
            range_end = range.end,
            inserted_len = new_text.len(),
            value_len = self.value.len(),
            marked = self.marked_range.is_some(),
            "input marked text changed"
        );
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;
        let line = self.last_layout.as_ref()?;
        let utf8_index = line.index_for_x(line_point.x)?;
        Some(self.offset_to_utf16(utf8_index))
    }
}

impl Focusable for InputState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct TextElement {
    input: Entity<InputState>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    text_bounds: Bounds<Pixels>,
    line_height: Pixels,
    display_text: SharedString,
    font_size: Pixels,
    placeholder_active: bool,
    marked_range: Option<Range<usize>>,
    cursor_visible: bool,
    cursor_blink_enabled: bool,
}

fn centered_text_bounds(bounds: Bounds<Pixels>, line_height: Pixels) -> Bounds<Pixels> {
    let text_height = line_height.min(bounds.size.height);
    let text_top = bounds.top() + (bounds.size.height - text_height) / 2.0;
    Bounds::new(
        point(bounds.left(), text_top),
        size(bounds.size.width, text_height),
    )
}

impl IntoElement for TextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let content = input.value.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let is_focused = input.focus_handle.is_focused(window);
        let text_style = window.text_style();
        let line_height = text_style.line_height_in_pixels(window.rem_size());
        let text_bounds = centered_text_bounds(bounds, line_height);
        let theme = cx.global::<ThemeState>();
        let theme_colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(Instant::now()),
            theme.accent,
        );
        let text_color = theme_colors.text_primary;

        let content_is_empty = content.is_empty();
        let display_text = if content_is_empty {
            input.placeholder.clone()
        } else {
            content.clone()
        };
        let placeholder_alpha = if theme_colors.bg.l < 0.5 { 0.58 } else { 0.54 };
        let text_color = if content_is_empty {
            Hsla {
                a: text_color.a * placeholder_alpha,
                ..text_color
            }
        } else {
            text_color
        };
        let font_size = text_style.font_size.to_pixels(window.rem_size());

        let base_run = TextRun {
            len: display_text.len(),
            font: text_style.font(),
            color: text_color,
            background_color: None,
            background_corner_radius: None,
            background_padding: None,
            underline: None,
            strikethrough: None,
        };

        let runs = if let Some(marked_range) = input.marked_range.as_ref() {
            vec![
                TextRun {
                    len: marked_range.start,
                    ..base_run.clone()
                },
                TextRun {
                    len: marked_range.end - marked_range.start,
                    underline: Some(UnderlineStyle {
                        color: Some(base_run.color),
                        thickness: px(1.0),
                        wavy: false,
                    }),
                    ..base_run.clone()
                },
                TextRun {
                    len: display_text.len() - marked_range.end,
                    ..base_run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect()
        } else {
            vec![base_run]
        };

        let line = if input.can_reuse_last_layout(&display_text, font_size, content_is_empty) {
            input
                .last_layout
                .clone()
                .expect("input cached line layout should exist")
        } else {
            window
                .text_system()
                .shape_line(display_text.clone(), font_size, &runs, None)
        };

        let cursor_pos = line.x_for_index(cursor);
        let cursor_blink_enabled = is_focused && selected_range.is_empty();
        let cursor_visible = input.cursor_visible_now(is_focused);
        let (selection, cursor) = if selected_range.is_empty() || content_is_empty {
            let cursor = cursor_visible.then_some(fill(
                Bounds::new(
                    point(text_bounds.left() + cursor_pos, text_bounds.top()),
                    size(px(1.5), text_bounds.size.height),
                ),
                text_color,
            ));
            (None, cursor)
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            text_bounds.left() + line.x_for_index(selected_range.start),
                            text_bounds.top(),
                        ),
                        point(
                            text_bounds.left() + line.x_for_index(selected_range.end),
                            text_bounds.bottom(),
                        ),
                    ),
                    if text_color.l > 0.6 {
                        hsla(214.0, 0.88, 0.68, 0.28)
                    } else {
                        hsla(214.0, 0.88, 0.58, 0.18)
                    },
                )),
                None,
            )
        };

        PrepaintState {
            line: Some(line),
            cursor,
            selection,
            text_bounds,
            line_height,
            display_text,
            font_size,
            placeholder_active: content_is_empty,
            marked_range: input.marked_range.clone(),
            cursor_visible,
            cursor_blink_enabled,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }
        let text_bounds = prepaint.text_bounds;
        let line_height = prepaint.line_height;
        let line = prepaint
            .line
            .take()
            .expect("input line layout should exist");
        let cached_line = line.clone();
        let display_text = prepaint.display_text.clone();
        let font_size = prepaint.font_size;
        let placeholder_active = prepaint.placeholder_active;
        let marked_range = prepaint.marked_range.clone();
        let cursor_visible = prepaint.cursor_visible;
        let cursor_blink_enabled = prepaint.cursor_blink_enabled;
        let _ = line.paint(text_bounds.origin, line_height, window, cx);
        if focus_handle.is_focused(window)
            && let Some(cursor) = prepaint.cursor.take()
        {
            window.paint_quad(cursor);
        }
        if cursor_blink_enabled {
            InputState::arm_cursor_blink_timer(self.input.clone(), cursor_visible, window, cx);
        }
        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(cached_line);
            input.last_layout_display_text = display_text;
            input.last_layout_marked_range = marked_range;
            input.last_layout_font_size = Some(font_size);
            input.last_layout_placeholder_active = placeholder_active;
            input.last_bounds = Some(text_bounds);
        });
    }
}

impl Render for InputState {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("Input")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .h_full()
            .w_full()
            .child(TextElement { input: cx.entity() })
    }
}

#[derive(IntoElement)]
pub struct Input {
    state: Entity<InputState>,
    base: gpui::Div,
    prefix: Option<gpui::AnyElement>,
    appearance: bool,
    bordered: bool,
    focus_bordered: bool,
    cleanable: bool,
    size: InputSize,
}

impl Input {
    pub fn new(state: &Entity<InputState>) -> Self {
        Self {
            state: state.clone(),
            base: div(),
            prefix: None,
            appearance: true,
            bordered: true,
            focus_bordered: true,
            cleanable: false,
            size: InputSize::Medium,
        }
    }

    pub fn appearance(mut self, appearance: bool) -> Self {
        self.appearance = appearance;
        self
    }

    pub fn bordered(mut self, bordered: bool) -> Self {
        self.bordered = bordered;
        self
    }

    pub fn focus_bordered(mut self, focus_bordered: bool) -> Self {
        self.focus_bordered = focus_bordered;
        self
    }

    pub fn cleanable(mut self, cleanable: bool) -> Self {
        self.cleanable = cleanable;
        self
    }

    pub fn prefix(mut self, prefix: impl IntoElement) -> Self {
        self.prefix = Some(prefix.into_any_element());
        self
    }

    pub fn with_size(mut self, size: InputSize) -> Self {
        self.size = size;
        self
    }
}

impl Styled for Input {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.base.style()
    }
}

impl RenderOnce for Input {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (value, is_focused) = {
            let snapshot = self.state.read(cx);
            (snapshot.value(), snapshot.focus_handle.is_focused(window))
        };
        let has_value = !value.is_empty();

        let metrics = self.size.metrics();
        let theme = cx.global::<ThemeState>();
        let theme_colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(Instant::now()),
            theme.accent,
        );
        let dark_mode = theme_colors.bg.l < 0.5;
        let shell_background = if dark_mode {
            hsla(222.0, 0.24, 0.14, 0.86)
        } else {
            hsla(0.0, 0.0, 1.0, 0.92)
        };
        let shell_hover_background = if dark_mode {
            hsla(220.0, 0.24, 0.17, 0.92)
        } else {
            hsla(210.0, 0.40, 0.98, 0.98)
        };
        let shell_border = if dark_mode {
            hsla(218.0, 0.20, 0.32, 0.95)
        } else {
            hsla(214.0, 0.22, 0.84, 1.0)
        };
        let focus_border = if dark_mode {
            hsla(214.0, 0.92, 0.68, 1.0)
        } else {
            hsla(214.0, 0.92, 0.56, 1.0)
        };
        let clear_button_text = if dark_mode {
            hsla(0.0, 0.0, 1.0, 0.64)
        } else {
            hsla(215.0, 0.18, 0.26, 0.68)
        };
        let clear_button_background = gpui::transparent_black();
        let clear_button_hover_background = if dark_mode {
            hsla(0.0, 0.0, 1.0, 0.08)
        } else {
            hsla(214.0, 0.40, 0.90, 0.70)
        };
        let show_shell = self.appearance || self.bordered || (self.focus_bordered && is_focused);
        let state = self.state.clone();

        let mut input = self
            .base
            .flex()
            .items_center()
            .gap(px(metrics.gap))
            .min_w(px(0.0))
            .overflow_hidden();

        if self.cleanable {
            input = input.relative();
        }

        if self.appearance {
            let right_padding = if self.cleanable && has_value {
                metrics.clear_slot
            } else {
                0.0
            };
            input = input
                .min_h(px(metrics.height))
                .px(px(metrics.padding_x))
                .pr(px(right_padding))
                .py(px(0.0))
                .rounded(px(metrics.radius))
                .bg(shell_background)
                .hover(|style| style.bg(shell_hover_background))
                .shadow(vec![BoxShadow {
                    color: if dark_mode {
                        hsla(0.0, 0.0, 0.0, 0.16)
                    } else {
                        hsla(220.0, 0.30, 0.24, 0.06)
                    },
                    blur_radius: px(14.0),
                    spread_radius: px(-10.0),
                    offset: point(px(0.0), px(3.0)),
                }]);
        } else if show_shell {
            input = input.rounded(px(metrics.radius));
        }

        if self.bordered || (self.focus_bordered && is_focused) {
            input = input
                .border_1()
                .border_color(if is_focused && self.focus_bordered {
                    focus_border
                } else {
                    shell_border
                });
        }

        if let Some(prefix) = self.prefix {
            input = input.child(
                div()
                    .flex_none()
                    .h_full()
                    .mr(px(2.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(prefix),
            );
        }

        input = input.child(div().flex_1().min_w(px(0.0)).h_full().child(self.state));

        if self.cleanable {
            let clear_button = div()
                .w(px(metrics.clear_button))
                .h(px(metrics.clear_button))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(metrics.clear_text_size))
                .text_color(clear_button_text)
                .bg(clear_button_background)
                .hover(|style| style.bg(clear_button_hover_background))
                .opacity(0.88)
                .child("×");

            if has_value {
                input = input.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right(px(2.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(clear_button.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            move |_ev, window, cx| {
                                state.update(cx, |input, cx| {
                                    input.set_value("", window, cx);
                                    cx.emit(InputEvent::Change);
                                });
                            },
                        )),
                );
            }
        }

        input
    }
}
