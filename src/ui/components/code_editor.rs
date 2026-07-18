use crate::ui::theme::colors::ThemeColors;
use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font,
    GlobalElementId, Hsla, InteractiveElement, IntoElement, KeyBinding, KeyDownEvent, LayoutId,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement, Pixels,
    Point, ScrollWheelEvent, ShapedLine, SharedString, Size, Style, Styled, TextRun,
    UnderlineStyle, Utf16Selection, Window, actions, black, div, fill, hsla, point, px, relative,
    size,
};
use std::ops::Range;
use std::time::{Duration, Instant};
use unicode_segmentation::UnicodeSegmentation;

const HORIZONTAL_PADDING: f32 = 14.0;
const VERTICAL_PADDING: f32 = 12.0;
const GUTTER_GAP: f32 = 12.0;
const MIN_EDITOR_WIDTH: f32 = 420.0;
const MONO_ADVANCE_PX: f32 = 7.3;
const INDENT_TEXT: &str = "  ";
const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(1000);
const CURSOR_VISIBLE_WINDOW: Duration = Duration::from_millis(530);
const MAX_EDIT_HISTORY: usize = 128;
const FOLD_MARKER_WIDTH: f32 = 14.0;
const SCROLLBAR_THICKNESS: f32 = 10.0;
const SCROLLBAR_MARGIN: f32 = 4.0;
const SCROLLBAR_MIN_THUMB: f32 = 36.0;
const SCROLLBAR_VISIBLE_WINDOW: Duration = Duration::from_millis(1250);

actions!(
    code_editor,
    [
        EditorBackspace,
        EditorDelete,
        EditorLeft,
        EditorRight,
        EditorUp,
        EditorDown,
        EditorSelectLeft,
        EditorSelectRight,
        EditorSelectUp,
        EditorSelectDown,
        EditorSelectAll,
        EditorHome,
        EditorEnd,
        EditorEnter,
        EditorIndent,
        EditorOutdent,
        EditorPaste,
        EditorCut,
        EditorCopy,
        EditorUndo,
        EditorRedo,
        EditorSave,
        EditorFormat,
    ]
);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodeEditorLanguage {
    Auto,
    JsonNbt,
    GenericCode,
}

#[derive(Clone)]
pub enum CodeEditorEvent {
    Change,
    PointerInteractionStarted,
    PointerInteractionEnded,
    SaveRequested,
    FormatRequested,
}

#[derive(Clone, Copy)]
struct LineRange {
    start: usize,
    end: usize,
}

#[derive(Clone)]
struct EditSnapshot {
    value: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

#[derive(Clone, Copy)]
struct VisualLine {
    range: LineRange,
    source_line: usize,
    foldable: bool,
    folded: bool,
}

#[derive(Clone, Copy)]
struct FoldRegion {
    start_line: usize,
    end_line: usize,
}

#[derive(Clone, Copy)]
struct HighlightRange {
    start: usize,
    end: usize,
    color: Hsla,
}

#[derive(Clone, Copy)]
struct DiagnosticRange {
    start: usize,
    end: usize,
    color: Hsla,
}

#[derive(Clone, Copy)]
struct SyntaxPalette {
    key: Hsla,
    string: Hsla,
    number: Hsla,
    keyword: Hsla,
    function: Hsla,
    punctuation: Hsla,
    comment: Hsla,
    diagnostic: Hsla,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScrollbarAxis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy)]
struct ScrollbarDrag {
    axis: ScrollbarAxis,
    pointer_origin: Point<Pixels>,
    scroll_origin: Point<Pixels>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CodeEditorPointerRelease {
    interaction: bool,
    scrollbar_drag: bool,
}

pub struct CodeEditorState {
    focus_handle: FocusHandle,
    language: CodeEditorLanguage,
    value: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_bounds: Option<Bounds<Pixels>>,
    last_line_ranges: Vec<LineRange>,
    last_visual_lines: Vec<VisualLine>,
    last_line_layouts: Vec<ShapedLine>,
    layout_revision: u64,
    last_layout_revision: u64,
    last_layout_colors: Option<ThemeColors>,
    last_layout_font: Option<Font>,
    last_layout_font_size: Pixels,
    last_content_width: Pixels,
    last_line_height: Pixels,
    last_gutter_width: Pixels,
    is_selecting: bool,
    cursor_blink_started_at: Option<Instant>,
    preferred_column: Option<usize>,
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    collapsed_lines: Vec<usize>,
    scroll_offset: Point<Pixels>,
    content_size: Size<Pixels>,
    viewport_size: Size<Pixels>,
    scrollbar_visible_until: Option<Instant>,
    scrollbar_drag: Option<ScrollbarDrag>,
    mouse_over: bool,
}

impl EventEmitter<CodeEditorEvent> for CodeEditorState {}

pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", EditorBackspace, Some("CodeEditor")),
        KeyBinding::new("delete", EditorDelete, Some("CodeEditor")),
        KeyBinding::new("left", EditorLeft, Some("CodeEditor")),
        KeyBinding::new("right", EditorRight, Some("CodeEditor")),
        KeyBinding::new("up", EditorUp, Some("CodeEditor")),
        KeyBinding::new("down", EditorDown, Some("CodeEditor")),
        KeyBinding::new("shift-left", EditorSelectLeft, Some("CodeEditor")),
        KeyBinding::new("shift-right", EditorSelectRight, Some("CodeEditor")),
        KeyBinding::new("shift-up", EditorSelectUp, Some("CodeEditor")),
        KeyBinding::new("shift-down", EditorSelectDown, Some("CodeEditor")),
        KeyBinding::new("home", EditorHome, Some("CodeEditor")),
        KeyBinding::new("end", EditorEnd, Some("CodeEditor")),
        KeyBinding::new("enter", EditorEnter, Some("CodeEditor")),
        KeyBinding::new("tab", EditorIndent, Some("CodeEditor")),
        KeyBinding::new("shift-tab", EditorOutdent, Some("CodeEditor")),
        KeyBinding::new("ctrl-a", EditorSelectAll, Some("CodeEditor")),
        KeyBinding::new("ctrl-v", EditorPaste, Some("CodeEditor")),
        KeyBinding::new("ctrl-c", EditorCopy, Some("CodeEditor")),
        KeyBinding::new("ctrl-x", EditorCut, Some("CodeEditor")),
        KeyBinding::new("ctrl-z", EditorUndo, Some("CodeEditor")),
        KeyBinding::new("ctrl-y", EditorRedo, Some("CodeEditor")),
        KeyBinding::new("ctrl-shift-z", EditorRedo, Some("CodeEditor")),
        KeyBinding::new("ctrl-s", EditorSave, Some("CodeEditor")),
        KeyBinding::new("ctrl-shift-f", EditorFormat, Some("CodeEditor")),
    ]);
}

impl CodeEditorState {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle().tab_stop(true),
            language: CodeEditorLanguage::Auto,
            value: SharedString::from(""),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_bounds: None,
            last_line_ranges: Vec::new(),
            last_visual_lines: Vec::new(),
            last_line_layouts: Vec::new(),
            layout_revision: 1,
            last_layout_revision: 0,
            last_layout_colors: None,
            last_layout_font: None,
            last_layout_font_size: px(0.0),
            last_content_width: px(MIN_EDITOR_WIDTH),
            last_line_height: px(20.0),
            last_gutter_width: px(40.0),
            is_selecting: false,
            cursor_blink_started_at: None,
            preferred_column: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            collapsed_lines: Vec::new(),
            scroll_offset: point(px(0.0), px(0.0)),
            content_size: size(px(MIN_EDITOR_WIDTH), px(0.0)),
            viewport_size: size(px(0.0), px(0.0)),
            scrollbar_visible_until: None,
            scrollbar_drag: None,
            mouse_over: false,
        }
    }

    pub fn value(&self) -> SharedString {
        self.value.clone()
    }

    pub fn set_language(&mut self, language: CodeEditorLanguage, cx: &mut Context<Self>) {
        if self.language == language {
            return;
        }
        self.language = language;
        self.invalidate_layout();
        cx.notify();
    }

    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        let value = value.into();
        let end = value.len();
        self.value = value;
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.preferred_column = None;
        self.cursor_blink_started_at = Some(Instant::now());
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.collapsed_lines.clear();
        self.invalidate_layout();
        self.clamp_scroll_offset();
        self.ensure_cursor_visible();
        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn invalidate_layout(&mut self) {
        self.layout_revision = self.layout_revision.saturating_add(1);
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.value.len());
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.preferred_column = None;
        self.cursor_blink_started_at = Some(Instant::now());
        self.ensure_cursor_visible();
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.value.len());
        let mut next_range = self.selected_range.clone();
        let mut next_reversed = self.selection_reversed;
        if next_reversed {
            next_range.start = offset;
        } else {
            next_range.end = offset;
        }
        if next_range.end < next_range.start {
            next_reversed = !next_reversed;
            next_range = next_range.end..next_range.start;
        }
        self.selected_range = next_range;
        self.selection_reversed = next_reversed;
        self.cursor_blink_started_at = Some(Instant::now());
        self.ensure_cursor_visible();
        cx.notify();
    }

    fn update_selection_state(
        &mut self,
        selected_range: Range<usize>,
        selection_reversed: bool,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = selected_range;
        self.selection_reversed = selection_reversed;
        self.cursor_blink_started_at = Some(Instant::now());
        self.ensure_cursor_visible();
        cx.notify();
    }

    fn cursor_content_position(&self, offset: usize) -> Option<Point<Pixels>> {
        if self.last_line_ranges.is_empty() || self.last_line_layouts.is_empty() {
            return None;
        }

        let line_index = line_index_for_offset(&self.last_line_ranges, offset);
        let layout = self.last_line_layouts.get(line_index)?;
        let line = self.last_line_ranges.get(line_index)?;
        let local_index = offset.saturating_sub(line.start).min(line.end - line.start);

        Some(point(
            self.last_gutter_width
                + px(GUTTER_GAP)
                + px(HORIZONTAL_PADDING)
                + layout.x_for_index(local_index),
            px(VERTICAL_PADDING) + self.last_line_height * line_index as f32,
        ))
    }

    fn max_scroll_offset(&self) -> Point<Pixels> {
        point(
            (self.content_size.width - self.viewport_size.width).max(px(0.0)),
            (self.content_size.height - self.viewport_size.height).max(px(0.0)),
        )
    }

    fn clamp_scroll_offset(&mut self) {
        self.scroll_offset =
            clamp_scroll_offset(self.scroll_offset, self.content_size, self.viewport_size);
    }

    fn scroll_by(&mut self, delta: Point<Pixels>, cx: &mut Context<Self>) {
        let previous = self.scroll_offset;
        self.scroll_offset.x += delta.x;
        self.scroll_offset.y += delta.y;
        self.clamp_scroll_offset();
        if self.scroll_offset != previous {
            self.show_scrollbars();
            cx.notify();
        }
    }

    fn show_scrollbars(&mut self) {
        self.scrollbar_visible_until = Some(Instant::now() + SCROLLBAR_VISIBLE_WINDOW);
    }

    fn scrollbars_visible(&self) -> bool {
        self.mouse_over
            || self.scrollbar_drag.is_some()
            || self
                .scrollbar_visible_until
                .is_some_and(|deadline| Instant::now() <= deadline)
    }

    fn ensure_cursor_visible(&mut self) {
        let Some(cursor_position) = self.cursor_content_position(self.cursor_offset()) else {
            return;
        };

        let viewport = self.viewport_size;
        if viewport.width <= px(0.0) || viewport.height <= px(0.0) {
            return;
        }

        let current_offset = self.scroll_offset;
        let mut target_x = current_offset.x;
        let mut target_y = current_offset.y;
        let max_offset = self.max_scroll_offset();
        let horizontal_padding = px(36.0);
        let vertical_padding = px(28.0);
        let cursor_right = cursor_position.x + px(2.0);
        let cursor_bottom = cursor_position.y + self.last_line_height;

        if cursor_position.x < target_x + horizontal_padding {
            target_x = (cursor_position.x - horizontal_padding).max(px(0.0));
        } else if cursor_right > target_x + viewport.width - horizontal_padding {
            target_x = (cursor_right - viewport.width + horizontal_padding).max(px(0.0));
        }

        if cursor_position.y < target_y + vertical_padding {
            target_y = (cursor_position.y - vertical_padding).max(px(0.0));
        } else if cursor_bottom > target_y + viewport.height - vertical_padding {
            target_y = (cursor_bottom - viewport.height + vertical_padding).max(px(0.0));
        }

        target_x = target_x.min(max_offset.x).max(px(0.0));
        target_y = target_y.min(max_offset.y).max(px(0.0));

        if target_x != current_offset.x || target_y != current_offset.y {
            self.scroll_offset = point(target_x, target_y);
            self.show_scrollbars();
        }
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

    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            value: self.value.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn push_undo_snapshot(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > MAX_EDIT_HISTORY {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn restore_snapshot(&mut self, snapshot: EditSnapshot, cx: &mut Context<Self>) {
        self.value = snapshot.value;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.preferred_column = None;
        self.cursor_blink_started_at = Some(Instant::now());
        self.invalidate_layout();
        self.ensure_cursor_visible();
        cx.emit(CodeEditorEvent::Change);
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        self.redo_stack.push(self.snapshot());
        self.restore_snapshot(snapshot, cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.redo_stack.pop() else {
            return;
        };
        self.undo_stack.push(self.snapshot());
        self.restore_snapshot(snapshot, cx);
    }

    fn replace_range_with_history(
        &mut self,
        range: Range<usize>,
        new_text: &str,
        record_history: bool,
        cx: &mut Context<Self>,
    ) {
        if self.value[range.clone()] == *new_text {
            return;
        }
        if record_history {
            self.push_undo_snapshot();
        }
        let end = range.start + new_text.len();
        self.value = self.build_replaced_value(&range, new_text);
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.preferred_column = None;
        self.cursor_blink_started_at = Some(Instant::now());
        self.invalidate_layout();
        self.ensure_cursor_visible();
        cx.emit(CodeEditorEvent::Change);
        cx.notify();
    }

    fn replace_range(&mut self, range: Range<usize>, new_text: &str, cx: &mut Context<Self>) {
        self.replace_range_with_history(range, new_text, true, cx);
    }

    fn copy_selection(&self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.value[self.selected_range.clone()].to_string(),
        ));
    }

    fn cut_selection(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.value[self.selected_range.clone()].to_string(),
        ));
        self.replace_range(self.selected_range.clone(), "", cx);
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_range(self.selected_range.clone(), &text, cx);
        }
    }

    fn move_left(&mut self, selecting: bool, cx: &mut Context<Self>) {
        self.preferred_column = None;
        if self.selected_range.is_empty() {
            let offset = self.previous_boundary(self.cursor_offset());
            if selecting {
                self.select_to(offset, cx);
            } else {
                self.move_to(offset, cx);
            }
            return;
        }

        if selecting {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn move_right(&mut self, selecting: bool, cx: &mut Context<Self>) {
        self.preferred_column = None;
        if self.selected_range.is_empty() {
            let offset = self.next_boundary(self.cursor_offset());
            if selecting {
                self.select_to(offset, cx);
            } else {
                self.move_to(offset, cx);
            }
            return;
        }

        if selecting {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn move_home(&mut self, selecting: bool, document_edge: bool, cx: &mut Context<Self>) {
        let lines = collect_line_ranges(self.value.as_ref());
        let target = if document_edge {
            0
        } else {
            let line_index = line_index_for_offset(&lines, self.cursor_offset());
            lines[line_index].start
        };
        self.preferred_column = None;
        if selecting {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_end(&mut self, selecting: bool, document_edge: bool, cx: &mut Context<Self>) {
        let lines = collect_line_ranges(self.value.as_ref());
        let target = if document_edge {
            self.value.len()
        } else {
            let line_index = line_index_for_offset(&lines, self.cursor_offset());
            lines[line_index].end
        };
        self.preferred_column = None;
        if selecting {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_vertical(&mut self, selecting: bool, direction: isize, cx: &mut Context<Self>) {
        let lines = collect_line_ranges(self.value.as_ref());
        if lines.is_empty() {
            return;
        }

        let current_offset = self.cursor_offset();
        let current_line = line_index_for_offset(&lines, current_offset);
        let target_line = current_line.saturating_add_signed(direction);
        let target_line = target_line.min(lines.len().saturating_sub(1));
        if target_line == current_line && !selecting && self.selected_range.is_empty() {
            return;
        }

        let preferred_column = self.preferred_column.unwrap_or_else(|| {
            grapheme_column(
                self.value.as_ref(),
                lines[current_line].start,
                current_offset,
            )
        });
        let target = offset_for_grapheme_column(
            self.value.as_ref(),
            lines[target_line].start,
            lines[target_line].end,
            preferred_column,
        );

        self.preferred_column = Some(preferred_column);
        if selecting {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        self.preferred_column = None;
        if self.selected_range.is_empty() {
            let offset = self.previous_boundary(self.cursor_offset());
            self.replace_range(offset..self.cursor_offset(), "", cx);
        } else {
            self.replace_range(self.selected_range.clone(), "", cx);
        }
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        self.preferred_column = None;
        if self.selected_range.is_empty() {
            let offset = self.next_boundary(self.cursor_offset());
            self.replace_range(self.cursor_offset()..offset, "", cx);
        } else {
            self.replace_range(self.selected_range.clone(), "", cx);
        }
    }

    fn insert_newline(&mut self, cx: &mut Context<Self>) {
        self.replace_range(self.selected_range.clone(), "\n", cx);
    }

    fn insert_indent(&mut self, cx: &mut Context<Self>) {
        self.replace_range(self.selected_range.clone(), INDENT_TEXT, cx);
    }

    fn remove_indent(&mut self, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let lines = collect_line_ranges(self.value.as_ref());
        let line = lines[line_index_for_offset(&lines, cursor)];
        let line_prefix = &self.value[line.start..cursor];
        let removable = if line_prefix.ends_with(INDENT_TEXT) {
            INDENT_TEXT.len()
        } else if line_prefix.ends_with(' ') {
            line_prefix
                .as_bytes()
                .iter()
                .rev()
                .take(INDENT_TEXT.len())
                .take_while(|byte| **byte == b' ')
                .count()
        } else if line_prefix.ends_with('\t') {
            1
        } else {
            0
        };

        if removable == 0 {
            return;
        }

        self.replace_range(cursor - removable..cursor, "", cx);
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return self.value.len();
        };
        if self.last_line_ranges.is_empty() || self.last_line_layouts.is_empty() {
            return self.value.len();
        }

        let local_x = position.x - bounds.left() + self.scroll_offset.x;
        let local_y = position.y - bounds.top() + self.scroll_offset.y;
        let content_y = (local_y - px(VERTICAL_PADDING)).max(px(0.0));
        let line_index =
            ((content_y / self.last_line_height) as usize).min(self.last_line_ranges.len() - 1);
        let x = (local_x - self.last_gutter_width - px(GUTTER_GAP) - px(HORIZONTAL_PADDING))
            .max(px(0.0));
        let layout = &self.last_line_layouts[line_index];
        let line = self.last_line_ranges[line_index];
        let local_index = layout
            .closest_index_for_x(x)
            .min(line.end.saturating_sub(line.start));
        line.start + local_index
    }

    fn position_for_offset(&self, offset: usize) -> Option<Point<Pixels>> {
        let bounds = self.last_bounds?;
        let line_index = line_index_for_offset(&self.last_line_ranges, offset);
        let layout = self.last_line_layouts.get(line_index)?;
        let line = self.last_line_ranges.get(line_index)?;
        let local_index = offset.saturating_sub(line.start).min(line.end - line.start);
        Some(point(
            bounds.left()
                + self.last_gutter_width
                + px(GUTTER_GAP)
                + px(HORIZONTAL_PADDING)
                + layout.x_for_index(local_index)
                - self.scroll_offset.x,
            bounds.top() + px(VERTICAL_PADDING) + self.last_line_height * line_index as f32
                - self.scroll_offset.y,
        ))
    }

    fn bounds_for_utf8_range(&self, range: &Range<usize>) -> Option<Bounds<Pixels>> {
        let start = self.position_for_offset(range.start)?;
        let end = self.position_for_offset(range.end)?;
        if start.y == end.y {
            return Some(Bounds::from_corners(
                start,
                point(
                    (end.x).max(start.x + px(1.0)),
                    start.y + self.last_line_height,
                ),
            ));
        }

        let bounds = self.last_bounds?;
        Some(Bounds::from_corners(
            start,
            point(
                (bounds.right() - px(HORIZONTAL_PADDING)).max(start.x + px(1.0)),
                end.y + self.last_line_height,
            ),
        ))
    }

    fn handle_key_down(
        &mut self,
        _event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cursor_blink_started_at = Some(Instant::now());
        cx.notify();
    }

    fn action_backspace(&mut self, _: &EditorBackspace, _: &mut Window, cx: &mut Context<Self>) {
        self.backspace(cx);
    }

    fn action_delete(&mut self, _: &EditorDelete, _: &mut Window, cx: &mut Context<Self>) {
        self.delete(cx);
    }

    fn action_left(&mut self, _: &EditorLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_left(false, cx);
    }

    fn action_right(&mut self, _: &EditorRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_right(false, cx);
    }

    fn action_up(&mut self, _: &EditorUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(false, -1, cx);
    }

    fn action_down(&mut self, _: &EditorDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(false, 1, cx);
    }

    fn action_select_left(&mut self, _: &EditorSelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_left(true, cx);
    }

    fn action_select_right(
        &mut self,
        _: &EditorSelectRight,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_right(true, cx);
    }

    fn action_select_up(&mut self, _: &EditorSelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(true, -1, cx);
    }

    fn action_select_down(&mut self, _: &EditorSelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertical(true, 1, cx);
    }

    fn action_select_all(&mut self, _: &EditorSelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.update_selection_state(0..self.value.len(), false, cx);
    }

    fn action_home(&mut self, _: &EditorHome, _: &mut Window, cx: &mut Context<Self>) {
        self.move_home(false, false, cx);
    }

    fn action_end(&mut self, _: &EditorEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_end(false, false, cx);
    }

    fn action_enter(&mut self, _: &EditorEnter, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_newline(cx);
    }

    fn action_indent(&mut self, _: &EditorIndent, _: &mut Window, cx: &mut Context<Self>) {
        self.insert_indent(cx);
    }

    fn action_outdent(&mut self, _: &EditorOutdent, _: &mut Window, cx: &mut Context<Self>) {
        self.remove_indent(cx);
    }

    fn action_paste(&mut self, _: &EditorPaste, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_clipboard(cx);
    }

    fn action_cut(&mut self, _: &EditorCut, _: &mut Window, cx: &mut Context<Self>) {
        self.cut_selection(cx);
    }

    fn action_copy(&mut self, _: &EditorCopy, _: &mut Window, cx: &mut Context<Self>) {
        self.copy_selection(cx);
    }

    fn action_undo(&mut self, _: &EditorUndo, _: &mut Window, cx: &mut Context<Self>) {
        self.undo(cx);
    }

    fn action_redo(&mut self, _: &EditorRedo, _: &mut Window, cx: &mut Context<Self>) {
        self.redo(cx);
    }

    fn action_save(&mut self, _: &EditorSave, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(CodeEditorEvent::SaveRequested);
    }

    fn action_format(&mut self, _: &EditorFormat, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(CodeEditorEvent::FormatRequested);
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.emit(CodeEditorEvent::PointerInteractionStarted);
        self.cursor_blink_started_at = Some(Instant::now());
        self.focus_handle.focus(window);
        self.mouse_over = self
            .last_bounds
            .is_some_and(|bounds| bounds.contains(&event.position));
        if self.start_scrollbar_interaction(event.position, cx) {
            cx.stop_propagation();
            return;
        }
        self.is_selecting = true;
        if self.toggle_fold_for_mouse_position(event.position, cx) {
            cx.stop_propagation();
            return;
        }
        let offset = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(offset, cx);
        } else {
            self.move_to(offset, cx);
        }
        cx.stop_propagation();
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mouse_over = self
            .last_bounds
            .is_some_and(|bounds| bounds.contains(&event.position));
        let release = release_stale_code_editor_pointer_interaction(
            &mut self.is_selecting,
            &mut self.scrollbar_drag,
            event.pressed_button,
        );
        if release.interaction {
            if release.scrollbar_drag {
                self.show_scrollbars();
            }
            cx.emit(CodeEditorEvent::PointerInteractionEnded);
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if self.update_scrollbar_drag(event.position, cx) {
            cx.stop_propagation();
            return;
        }
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
        cx.stop_propagation();
    }

    fn handle_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.is_selecting = false;
        if self.scrollbar_drag.take().is_some() {
            self.show_scrollbars();
            cx.notify();
        }
        cx.emit(CodeEditorEvent::PointerInteractionEnded);
        cx.stop_propagation();
    }

    fn handle_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(bounds) = self.last_bounds else {
            return;
        };
        if !bounds.contains(&event.position) {
            return;
        }

        self.mouse_over = true;
        let delta = event.delta.pixel_delta(window.line_height());
        let scroll_delta = scroll_offset_delta_from_wheel_delta(delta, event.modifiers.shift);
        self.is_selecting = false;
        self.scroll_by(scroll_delta, cx);
        cx.stop_propagation();
    }

    fn start_scrollbar_interaction(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(axis) = scrollbar_axis_at_position(
            position,
            self.scrollbar_thumb_bounds(ScrollbarAxis::Vertical),
            self.scrollbar_thumb_bounds(ScrollbarAxis::Horizontal),
        ) else {
            return false;
        };

        self.is_selecting = false;
        self.scrollbar_drag = Some(ScrollbarDrag {
            axis,
            pointer_origin: position,
            scroll_origin: self.scroll_offset,
        });
        self.show_scrollbars();
        cx.notify();
        true
    }

    fn update_scrollbar_drag(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.scrollbar_drag else {
            return false;
        };

        let previous = self.scroll_offset;
        match drag.axis {
            ScrollbarAxis::Vertical => {
                let Some(track) = self.scrollbar_track_bounds(ScrollbarAxis::Vertical) else {
                    return true;
                };
                let Some(thumb) = self.scrollbar_thumb_bounds(ScrollbarAxis::Vertical) else {
                    return true;
                };
                let max_scroll = self.max_scroll_offset().y;
                let track_range = (track.size.height - thumb.size.height).max(px(1.0));
                let pointer_delta = position.y - drag.pointer_origin.y;
                self.scroll_offset.y =
                    drag.scroll_origin.y + max_scroll * (pointer_delta / track_range);
            }
            ScrollbarAxis::Horizontal => {
                let Some(track) = self.scrollbar_track_bounds(ScrollbarAxis::Horizontal) else {
                    return true;
                };
                let Some(thumb) = self.scrollbar_thumb_bounds(ScrollbarAxis::Horizontal) else {
                    return true;
                };
                let max_scroll = self.max_scroll_offset().x;
                let track_range = (track.size.width - thumb.size.width).max(px(1.0));
                let pointer_delta = position.x - drag.pointer_origin.x;
                self.scroll_offset.x =
                    drag.scroll_origin.x + max_scroll * (pointer_delta / track_range);
            }
        }
        self.clamp_scroll_offset();
        self.show_scrollbars();
        if self.scroll_offset != previous {
            cx.notify();
        }
        true
    }

    fn scrollbar_track_bounds(&self, axis: ScrollbarAxis) -> Option<Bounds<Pixels>> {
        let bounds = self.last_bounds?;
        let thickness = px(SCROLLBAR_THICKNESS);
        let margin = px(SCROLLBAR_MARGIN);
        match axis {
            ScrollbarAxis::Vertical => {
                if self.content_size.height <= self.viewport_size.height {
                    return None;
                }
                Some(Bounds::new(
                    point(bounds.right() - thickness - margin, bounds.top() + margin),
                    size(
                        thickness,
                        (bounds.size.height - margin * 2.0 - thickness).max(thickness),
                    ),
                ))
            }
            ScrollbarAxis::Horizontal => {
                if self.content_size.width <= self.viewport_size.width {
                    return None;
                }
                Some(Bounds::new(
                    point(bounds.left() + margin, bounds.bottom() - thickness - margin),
                    size(
                        (bounds.size.width - margin * 2.0 - thickness).max(thickness),
                        thickness,
                    ),
                ))
            }
        }
    }

    fn scrollbar_thumb_bounds(&self, axis: ScrollbarAxis) -> Option<Bounds<Pixels>> {
        let track = self.scrollbar_track_bounds(axis)?;
        match axis {
            ScrollbarAxis::Vertical => {
                let max_scroll = self.max_scroll_offset().y;
                if max_scroll <= px(0.0) {
                    return None;
                }
                let ratio = (self.viewport_size.height / self.content_size.height).clamp(0.0, 1.0);
                let thumb_height = (track.size.height * ratio)
                    .max(px(SCROLLBAR_MIN_THUMB))
                    .min(track.size.height);
                let travel = (track.size.height - thumb_height).max(px(0.0));
                let top = track.top() + travel * (self.scroll_offset.y / max_scroll);
                Some(Bounds::new(
                    point(track.left(), top),
                    size(track.size.width, thumb_height),
                ))
            }
            ScrollbarAxis::Horizontal => {
                let max_scroll = self.max_scroll_offset().x;
                if max_scroll <= px(0.0) {
                    return None;
                }
                let ratio = (self.viewport_size.width / self.content_size.width).clamp(0.0, 1.0);
                let thumb_width = (track.size.width * ratio)
                    .max(px(SCROLLBAR_MIN_THUMB))
                    .min(track.size.width);
                let travel = (track.size.width - thumb_width).max(px(0.0));
                let left = track.left() + travel * (self.scroll_offset.x / max_scroll);
                Some(Bounds::new(
                    point(left, track.top()),
                    size(thumb_width, track.size.height),
                ))
            }
        }
    }

    fn toggle_fold_for_mouse_position(
        &mut self,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(bounds) = self.last_bounds else {
            return false;
        };
        let local_x = position.x - bounds.left();
        if local_x
            > self
                .last_gutter_width
                .min(px(FOLD_MARKER_WIDTH + HORIZONTAL_PADDING))
        {
            return false;
        }

        let local_y = position.y - bounds.top() + self.scroll_offset.y;
        let content_y = (local_y - px(VERTICAL_PADDING)).max(px(0.0));
        let line_index =
            ((content_y / self.last_line_height) as usize).min(self.last_line_ranges.len());
        let Some(line) = self.last_line_ranges.get(line_index).copied() else {
            return false;
        };
        let full_lines = collect_line_ranges(self.value.as_ref());
        let source_line = line_index_for_offset(&full_lines, line.start);
        let regions = collect_fold_regions(self.value.as_ref(), &full_lines);
        if !regions
            .iter()
            .any(|region| region.start_line == source_line && region.end_line > source_line)
        {
            return false;
        }

        if let Some(index) = self
            .collapsed_lines
            .iter()
            .position(|line| *line == source_line)
        {
            self.collapsed_lines.remove(index);
        } else {
            self.collapsed_lines.push(source_line);
            self.collapsed_lines.sort_unstable();
            self.collapsed_lines.dedup();
        }
        self.invalidate_layout();
        self.clamp_scroll_offset();
        self.show_scrollbars();
        cx.notify();
        true
    }
}

fn release_stale_code_editor_pointer_interaction(
    is_selecting: &mut bool,
    scrollbar_drag: &mut Option<ScrollbarDrag>,
    pressed_button: Option<MouseButton>,
) -> CodeEditorPointerRelease {
    if pressed_button == Some(MouseButton::Left) || (!*is_selecting && scrollbar_drag.is_none()) {
        return CodeEditorPointerRelease::default();
    }
    *is_selecting = false;
    CodeEditorPointerRelease {
        interaction: true,
        scrollbar_drag: scrollbar_drag.take().is_some(),
    }
}

impl EntityInputHandler for CodeEditorState {
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
    ) -> Option<Utf16Selection> {
        Some(Utf16Selection {
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
        self.invalidate_layout();
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

        self.replace_range_with_history(range, new_text, self.marked_range.is_none(), cx);
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

        self.value = self.build_replaced_value(&range, new_text);
        self.marked_range =
            (!new_text.is_empty()).then_some(range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|value| self.range_from_utf16(value))
            .map(|selected| selected.start + range.start..selected.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.selection_reversed = false;
        self.preferred_column = None;
        self.cursor_blink_started_at = Some(Instant::now());
        self.invalidate_layout();
        self.ensure_cursor_visible();
        cx.emit(CodeEditorEvent::Change);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        self.bounds_for_utf8_range(&range)
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(point)))
    }
}

impl Focusable for CodeEditorState {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

struct EditorElement {
    editor: Entity<CodeEditorState>,
    colors: ThemeColors,
}

struct PrepaintState {
    current_line: Option<PaintQuad>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    diagnostics: Vec<PaintQuad>,
    gutter_background: Option<PaintQuad>,
    divider: Option<PaintQuad>,
    gutter_width: Pixels,
    line_height: Pixels,
    line_ranges: Vec<LineRange>,
    visual_lines: Vec<VisualLine>,
    line_layouts: Vec<ShapedLine>,
    line_numbers: Vec<(usize, ShapedLine)>,
    visible_lines: Range<usize>,
    layout_revision: u64,
    layout_colors: ThemeColors,
    layout_font: Font,
    layout_font_size: Pixels,
    content_width: Pixels,
}

struct EditorLayoutSnapshot {
    line_ranges: Vec<LineRange>,
    visual_lines: Vec<VisualLine>,
    line_layouts: Vec<ShapedLine>,
    diagnostic_ranges: Vec<DiagnosticRange>,
    gutter_width: Pixels,
    line_height: Pixels,
    content_width: Pixels,
    content_height: Pixels,
}

impl Default for EditorLayoutSnapshot {
    fn default() -> Self {
        Self {
            line_ranges: Vec::new(),
            visual_lines: Vec::new(),
            line_layouts: Vec::new(),
            diagnostic_ranges: Vec::new(),
            gutter_width: px(0.0),
            line_height: px(0.0),
            content_width: px(0.0),
            content_height: px(0.0),
        }
    }
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = EditorLayoutSnapshot;
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
        let editor = self.editor.read(cx);
        let layout_snapshot = build_layout_snapshot(&editor, &self.colors, window);

        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        style.min_size.width = px(0.0).into();
        style.min_size.height = px(0.0).into();
        (window.request_layout(style, [], cx), layout_snapshot)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        let layout_snapshot = std::mem::replace(request_layout, EditorLayoutSnapshot::default());
        let line_ranges = layout_snapshot.line_ranges;
        let visual_lines = layout_snapshot.visual_lines;
        let line_layouts = layout_snapshot.line_layouts;
        let gutter_width = layout_snapshot.gutter_width;
        let line_height = layout_snapshot.line_height;
        self.editor.update(cx, |editor, _cx| {
            editor.content_size = size(
                layout_snapshot.content_width,
                layout_snapshot.content_height,
            );
            editor.viewport_size = bounds.size;
            editor.last_bounds = Some(bounds);
            editor.clamp_scroll_offset();
        });
        let editor = self.editor.read(cx);
        let visible_lines = visible_line_range(
            editor.scroll_offset.y,
            bounds.size.height,
            line_height,
            line_ranges.len(),
        );
        let diagnostic_ranges = layout_snapshot.diagnostic_ranges;
        let scroll_offset = editor.scroll_offset;
        let content_x = bounds.left() + gutter_width + px(GUTTER_GAP) + px(HORIZONTAL_PADDING)
            - scroll_offset.x;
        let content_y = bounds.top() + px(VERTICAL_PADDING) - scroll_offset.y;

        let gutter_background = Some(fill(
            Bounds::new(
                bounds.origin,
                size(gutter_width + px(HORIZONTAL_PADDING), bounds.size.height),
            ),
            hsla(
                self.colors.surface.h,
                self.colors.surface.s,
                self.colors.surface.l,
                if self.colors.bg.l < 0.5 { 0.84 } else { 0.96 },
            ),
        ));
        let divider = Some(fill(
            Bounds::new(
                point(
                    bounds.left() + gutter_width + px(HORIZONTAL_PADDING * 0.5),
                    bounds.top(),
                ),
                size(px(1.0), bounds.size.height),
            ),
            hsla(
                self.colors.border.h,
                self.colors.border.s,
                self.colors.border.l,
                0.42,
            ),
        ));

        let mut line_numbers = Vec::with_capacity(visible_lines.len());
        let mut selection = Vec::new();
        let mut diagnostics = Vec::new();
        let cursor_line = line_index_for_offset(&line_ranges, editor.cursor_offset());
        let selection_range = editor.selected_range.clone();
        let cursor_visible = editor.focus_handle.is_focused(window)
            && editor.cursor_blink_started_at.is_none_or(|started_at| {
                let elapsed = Instant::now().saturating_duration_since(started_at);
                elapsed.is_zero()
                    || elapsed.as_millis() % CURSOR_BLINK_PERIOD.as_millis()
                        < CURSOR_VISIBLE_WINDOW.as_millis()
            });

        for index in visible_lines.clone() {
            let line = line_ranges[index];
            let visual_line = visual_lines[index];
            let layout = &line_layouts[index];
            if !selection_range.is_empty() {
                let overlap_start = selection_range.start.max(line.start);
                let overlap_end = selection_range.end.min(line.end);
                if overlap_start < overlap_end {
                    let start_x = layout.x_for_index(overlap_start - line.start);
                    let end_x = layout.x_for_index(overlap_end - line.start);
                    selection.push(fill(
                        Bounds::new(
                            point(content_x + start_x, content_y + line_height * index as f32),
                            size((end_x - start_x).max(px(1.0)), line_height),
                        ),
                        if self.colors.bg.l < 0.5 {
                            hsla(215.0, 0.92, 0.62, 0.22)
                        } else {
                            hsla(215.0, 0.92, 0.56, 0.16)
                        },
                    ));
                }
            }

            for diagnostic in &diagnostic_ranges {
                let overlap_start = diagnostic.start.max(line.start);
                let overlap_end = diagnostic.end.min(line.end);
                if overlap_start >= overlap_end {
                    continue;
                }

                let start_x = layout.x_for_index(overlap_start - line.start);
                let end_x = layout.x_for_index(
                    overlap_end
                        .saturating_sub(line.start)
                        .min(line.end.saturating_sub(line.start)),
                );
                diagnostics.push(fill(
                    Bounds::new(
                        point(
                            content_x + start_x,
                            content_y + line_height * index as f32 + line_height - px(2.5),
                        ),
                        size((end_x - start_x).max(px(2.0)), px(2.0)),
                    ),
                    diagnostic.color,
                ));
            }

            let marker = if visual_line.folded {
                "▸ "
            } else if visual_line.foldable {
                "⌄ "
            } else {
                ""
            };
            let number_text =
                SharedString::from(format!("{}{}", marker, visual_line.source_line + 1));
            line_numbers.push((
                index,
                window.text_system().shape_line(
                    number_text,
                    font_size,
                    &[TextRun {
                        len: marker.len() + (visual_line.source_line + 1).to_string().len(),
                        font: text_style.font(),
                        color: if index == cursor_line {
                            self.colors.text_primary
                        } else {
                            self.colors.text_muted
                        },
                        background_color: None,
                        background_corner_radius: None,
                        background_padding: None,
                        underline: None,
                        strikethrough: None,
                    }],
                    None,
                ),
            ));
        }

        let current_line = Some(fill(
            Bounds::new(
                point(bounds.left(), content_y + line_height * cursor_line as f32),
                size(bounds.size.width, line_height),
            ),
            hsla(
                self.colors.accent.h,
                self.colors.accent.s,
                self.colors.accent.l,
                if self.colors.bg.l < 0.5 { 0.08 } else { 0.05 },
            ),
        ));

        let cursor = if editor.selected_range.is_empty() && cursor_visible {
            editor
                .position_for_offset(editor.cursor_offset())
                .map(|position| {
                    fill(
                        Bounds::new(position, size(px(1.5), line_height)),
                        if self.colors.bg.l < 0.5 {
                            hsla(0.0, 0.0, 1.0, 0.92)
                        } else {
                            black()
                        },
                    )
                })
        } else {
            None
        };

        PrepaintState {
            current_line,
            cursor,
            selection,
            diagnostics,
            gutter_background,
            divider,
            gutter_width,
            line_height,
            line_ranges,
            visual_lines,
            line_layouts,
            line_numbers,
            visible_lines,
            layout_revision: editor.layout_revision,
            layout_colors: self.colors,
            layout_font: text_style.font(),
            layout_font_size: font_size,
            content_width: layout_snapshot.content_width,
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
        let focus_handle = self.editor.read(cx).focus_handle.clone();
        window.set_input_handler(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );

        if let Some(gutter_background) = prepaint.gutter_background.take() {
            window.paint_quad(gutter_background);
        }
        if let Some(divider) = prepaint.divider.take() {
            window.paint_quad(divider);
        }

        let content_x =
            bounds.left() + prepaint.gutter_width + px(GUTTER_GAP) + px(HORIZONTAL_PADDING)
                - self.editor.read(cx).scroll_offset.x;
        let content_y = bounds.top() + px(VERTICAL_PADDING) - self.editor.read(cx).scroll_offset.y;
        let number_x = bounds.left() + prepaint.gutter_width - px(HORIZONTAL_PADDING * 0.5);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(current_line) = prepaint.current_line.take() {
                window.paint_quad(current_line);
            }

            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad);
            }

            for quad in prepaint.diagnostics.drain(..) {
                window.paint_quad(quad);
            }

            for index in prepaint.visible_lines.clone() {
                let layout = &prepaint.line_layouts[index];
                let origin_y = content_y + prepaint.line_height * index as f32;
                let _ = layout.paint(point(content_x, origin_y), prepaint.line_height, window, cx);
                if let Some((_, number)) = prepaint
                    .line_numbers
                    .iter()
                    .find(|(line_index, _)| *line_index == index)
                {
                    let _ = number.paint(
                        point(number_x - number.width, origin_y),
                        prepaint.line_height,
                        window,
                        cx,
                    );
                }
            }

            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        });

        self.paint_scrollbars(bounds, window, cx);

        let line_ranges = std::mem::take(&mut prepaint.line_ranges);
        let visual_lines = std::mem::take(&mut prepaint.visual_lines);
        let line_layouts = std::mem::take(&mut prepaint.line_layouts);

        self.editor.update(cx, |editor, _cx| {
            editor.last_bounds = Some(bounds);
            editor.last_line_ranges = line_ranges;
            editor.last_visual_lines = visual_lines;
            editor.last_line_layouts = line_layouts;
            editor.last_layout_revision = prepaint.layout_revision;
            editor.last_layout_colors = Some(prepaint.layout_colors);
            editor.last_layout_font = Some(prepaint.layout_font.clone());
            editor.last_layout_font_size = prepaint.layout_font_size;
            editor.last_content_width = prepaint.content_width;
            editor.last_line_height = prepaint.line_height;
            editor.last_gutter_width = prepaint.gutter_width;
        });
    }
}

impl EditorElement {
    fn paint_scrollbars(&self, _bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let editor = self.editor.read(cx);
        if !editor.scrollbars_visible() {
            return;
        }

        let track_color = hsla(
            self.colors.border.h,
            self.colors.border.s,
            self.colors.border.l,
            if self.colors.bg.l < 0.5 { 0.20 } else { 0.16 },
        );
        let thumb_color = hsla(
            self.colors.accent.h,
            self.colors.accent.s,
            self.colors.accent.l,
            if self.colors.bg.l < 0.5 { 0.58 } else { 0.42 },
        );

        for axis in [ScrollbarAxis::Vertical, ScrollbarAxis::Horizontal] {
            let Some(track) = editor.scrollbar_track_bounds(axis) else {
                continue;
            };
            let Some(thumb) = editor.scrollbar_thumb_bounds(axis) else {
                continue;
            };
            window.paint_quad(fill(track, track_color).corner_radii(px(SCROLLBAR_THICKNESS / 2.0)));
            window.paint_quad(fill(thumb, thumb_color).corner_radii(px(SCROLLBAR_THICKNESS / 2.0)));
        }
    }
}

#[derive(IntoElement)]
pub struct CodeEditor {
    state: Entity<CodeEditorState>,
    colors: ThemeColors,
    base: gpui::Div,
}

impl CodeEditor {
    pub fn new(state: &Entity<CodeEditorState>, colors: &ThemeColors) -> Self {
        Self {
            state: state.clone(),
            colors: *colors,
            base: div(),
        }
    }
}

impl Styled for CodeEditor {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.base.style()
    }
}

impl gpui::RenderOnce for CodeEditor {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus_handle = self.state.read(cx).focus_handle.clone();
        let state_for_key = self.state.clone();
        let state_for_mouse_down = self.state.clone();
        let state_for_mouse_move = self.state.clone();
        let state_for_mouse_up = self.state.clone();
        let state_for_mouse_up_out = self.state.clone();
        let state_for_scroll = self.state.clone();

        self.base
            .key_context("CodeEditor")
            .track_focus(&focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action({
                let state = self.state.clone();
                move |_: &EditorBackspace, _window, cx| {
                    state.update(cx, |editor, cx| editor.backspace(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorDelete, _window, cx| {
                    state.update(cx, |editor, cx| editor.delete(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorLeft, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_left(false, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorRight, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_right(false, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorUp, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_vertical(false, -1, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorDown, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_vertical(false, 1, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorSelectLeft, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_left(true, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorSelectRight, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_right(true, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorSelectUp, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_vertical(true, -1, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorSelectDown, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_vertical(true, 1, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorSelectAll, _window, cx| {
                    state.update(cx, |editor, cx| {
                        editor.update_selection_state(0..editor.value.len(), false, cx);
                    });
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorHome, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_home(false, false, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorEnd, _window, cx| {
                    state.update(cx, |editor, cx| editor.move_end(false, false, cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorEnter, _window, cx| {
                    state.update(cx, |editor, cx| editor.insert_newline(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorIndent, _window, cx| {
                    state.update(cx, |editor, cx| editor.insert_indent(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorOutdent, _window, cx| {
                    state.update(cx, |editor, cx| editor.remove_indent(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorPaste, _window, cx| {
                    state.update(cx, |editor, cx| editor.paste_clipboard(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorCut, _window, cx| {
                    state.update(cx, |editor, cx| editor.cut_selection(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorCopy, _window, cx| {
                    state.update(cx, |editor, cx| editor.copy_selection(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorUndo, _window, cx| {
                    state.update(cx, |editor, cx| editor.undo(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorRedo, _window, cx| {
                    state.update(cx, |editor, cx| editor.redo(cx));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorSave, _window, cx| {
                    state.update(cx, |editor, cx| cx.emit(CodeEditorEvent::SaveRequested));
                }
            })
            .on_action({
                let state = self.state.clone();
                move |_: &EditorFormat, _window, cx| {
                    state.update(cx, |editor, cx| cx.emit(CodeEditorEvent::FormatRequested));
                }
            })
            .text_size(px(12.5))
            .line_height(px(20.0))
            .on_key_down(move |event, window, cx| {
                state_for_key.update(cx, |editor, cx| {
                    editor.handle_key_down(event, window, cx);
                });
            })
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                state_for_mouse_down.update(cx, |editor, cx| {
                    editor.handle_mouse_down(event, window, cx);
                });
            })
            .on_mouse_move(move |event, window, cx| {
                state_for_mouse_move.update(cx, |editor, cx| {
                    editor.handle_mouse_move(event, window, cx);
                });
            })
            .on_mouse_up(MouseButton::Left, move |event, window, cx| {
                state_for_mouse_up.update(cx, |editor, cx| {
                    editor.handle_mouse_up(event, window, cx);
                });
            })
            .on_mouse_up_out(MouseButton::Left, move |event, window, cx| {
                state_for_mouse_up_out.update(cx, |editor, cx| {
                    editor.handle_mouse_up(event, window, cx);
                });
            })
            .on_scroll_wheel(move |event, window, cx| {
                state_for_scroll.update(cx, |editor, cx| {
                    editor.handle_scroll_wheel(event, window, cx);
                });
            })
            .overflow_hidden()
            .child(EditorElement {
                editor: self.state,
                colors: self.colors,
            })
    }
}

fn build_layout_snapshot(
    editor: &CodeEditorState,
    colors: &ThemeColors,
    window: &mut Window,
) -> EditorLayoutSnapshot {
    let text_style = window.text_style();
    let line_height = text_style.line_height_in_pixels(window.rem_size());
    let font_size = text_style.font_size.to_pixels(window.rem_size());
    let font = text_style.font();
    if editor.last_layout_revision == editor.layout_revision
        && editor.last_layout_colors == Some(*colors)
        && editor.last_layout_font.as_ref() == Some(&font)
        && editor.last_layout_font_size == font_size
        && !editor.last_line_ranges.is_empty()
        && editor.last_line_ranges.len() == editor.last_visual_lines.len()
        && editor.last_line_ranges.len() == editor.last_line_layouts.len()
    {
        let line_count = editor.last_line_ranges.len().max(1);
        return EditorLayoutSnapshot {
            line_ranges: editor.last_line_ranges.clone(),
            visual_lines: editor.last_visual_lines.clone(),
            line_layouts: editor.last_line_layouts.clone(),
            diagnostic_ranges: format_diagnostics(
                editor.value.as_ref(),
                editor.language,
                &syntax_palette(colors),
            ),
            gutter_width: editor.last_gutter_width,
            line_height,
            content_width: editor.last_content_width,
            content_height: line_height * line_count as f32 + px(VERTICAL_PADDING * 2.0),
        };
    }
    let all_line_ranges = collect_line_ranges(editor.value.as_ref());
    let fold_regions = collect_fold_regions(editor.value.as_ref(), &all_line_ranges);
    let visual_lines =
        collect_visual_lines(&all_line_ranges, &fold_regions, &editor.collapsed_lines);
    let line_ranges = visual_lines
        .iter()
        .map(|line| line.range)
        .collect::<Vec<_>>();
    let line_count = line_ranges.len().max(1);
    let digits = all_line_ranges.len().to_string().len().max(2);
    let gutter_width =
        px(18.0 + FOLD_MARKER_WIDTH + digits as f32 * MONO_ADVANCE_PX + HORIZONTAL_PADDING);
    let palette = syntax_palette(colors);
    let highlights = code_highlights(editor.value.as_ref(), editor.language, &palette);
    let diagnostic_ranges = format_diagnostics(editor.value.as_ref(), editor.language, &palette);
    let line_layouts = line_ranges
        .iter()
        .zip(visual_lines.iter())
        .map(|line| {
            let (line, visual_line) = line;
            let mut line_string = editor.value[line.start..line.end].to_string();
            if visual_line.folded {
                line_string.push_str("  ...");
            }
            let line_text = SharedString::from(line_string);
            let runs = line_runs(
                &line_text,
                line.start,
                editor.marked_range.as_ref(),
                &highlights,
                font.clone(),
                text_style.color,
            );
            window
                .text_system()
                .shape_line(line_text, font_size, &runs, None)
        })
        .collect::<Vec<_>>();
    let widest_line_width = line_layouts
        .iter()
        .map(|layout| layout.width)
        .max()
        .unwrap_or(px(0.0));

    EditorLayoutSnapshot {
        line_ranges,
        visual_lines,
        line_layouts,
        diagnostic_ranges,
        gutter_width,
        line_height,
        content_width: (widest_line_width
            + gutter_width
            + px(GUTTER_GAP + HORIZONTAL_PADDING * 2.0))
        .max(px(MIN_EDITOR_WIDTH)),
        content_height: line_height * line_count as f32 + px(VERTICAL_PADDING * 2.0),
    }
}

fn collect_line_ranges(text: &str) -> Vec<LineRange> {
    let mut line_ranges = Vec::new();
    let mut start = 0;
    for (index, character) in text.char_indices() {
        if character == '\n' {
            line_ranges.push(LineRange { start, end: index });
            start = index + character.len_utf8();
        }
    }
    line_ranges.push(LineRange {
        start,
        end: text.len(),
    });
    line_ranges
}

fn line_index_for_offset(line_ranges: &[LineRange], offset: usize) -> usize {
    if line_ranges.is_empty() {
        return 0;
    }
    line_ranges
        .partition_point(|line| line.start <= offset)
        .saturating_sub(1)
        .min(line_ranges.len().saturating_sub(1))
}

fn grapheme_column(text: &str, line_start: usize, offset: usize) -> usize {
    text[line_start..offset].graphemes(true).count()
}

fn offset_for_grapheme_column(
    text: &str,
    line_start: usize,
    line_end: usize,
    column: usize,
) -> usize {
    let line_text = &text[line_start..line_end];
    let mut graphemes = line_text.grapheme_indices(true);
    for _ in 0..column {
        let Some((index, _)) = graphemes.next() else {
            return line_end;
        };
        if index == line_text.len() {
            return line_end;
        }
    }
    match graphemes.next() {
        Some((index, _)) => line_start + index,
        None => line_end,
    }
}

fn clamp_scroll_offset(
    offset: Point<Pixels>,
    content_size: Size<Pixels>,
    viewport_size: Size<Pixels>,
) -> Point<Pixels> {
    let max_x = (content_size.width - viewport_size.width).max(px(0.0));
    let max_y = (content_size.height - viewport_size.height).max(px(0.0));
    point(
        offset.x.min(max_x).max(px(0.0)),
        offset.y.min(max_y).max(px(0.0)),
    )
}

fn visible_line_range(
    scroll_y: Pixels,
    viewport_height: Pixels,
    line_height: Pixels,
    line_count: usize,
) -> Range<usize> {
    if line_count == 0 || line_height <= px(0.0) || viewport_height <= px(0.0) {
        return 0..0;
    }
    const OVERSCAN_LINES: usize = 2;
    let content_y = (scroll_y - px(VERTICAL_PADDING)).max(px(0.0));
    let first = (content_y / line_height) as usize;
    let visible_count = (viewport_height / line_height) as usize + 1;
    first.saturating_sub(OVERSCAN_LINES)
        ..first
            .saturating_add(visible_count)
            .saturating_add(OVERSCAN_LINES)
            .min(line_count)
}

fn scroll_offset_delta_from_wheel_delta(delta: Point<Pixels>, shift: bool) -> Point<Pixels> {
    if shift && delta.x == px(0.0) {
        return point(px(0.0) - delta.y, px(0.0));
    }

    point(px(0.0) - delta.x, px(0.0) - delta.y)
}

fn scrollbar_axis_at_position(
    position: Point<Pixels>,
    vertical_thumb: Option<Bounds<Pixels>>,
    horizontal_thumb: Option<Bounds<Pixels>>,
) -> Option<ScrollbarAxis> {
    if vertical_thumb.is_some_and(|bounds| bounds.contains(&position)) {
        return Some(ScrollbarAxis::Vertical);
    }
    if horizontal_thumb.is_some_and(|bounds| bounds.contains(&position)) {
        return Some(ScrollbarAxis::Horizontal);
    }
    None
}

fn collect_visual_lines(
    line_ranges: &[LineRange],
    fold_regions: &[FoldRegion],
    collapsed_lines: &[usize],
) -> Vec<VisualLine> {
    let mut visual_lines = Vec::with_capacity(line_ranges.len());
    let mut index = 0;
    while index < line_ranges.len() {
        let folded_region = fold_regions
            .iter()
            .find(|region| region.start_line == index && collapsed_lines.contains(&index));
        let foldable = fold_regions.iter().any(|region| region.start_line == index);
        visual_lines.push(VisualLine {
            range: line_ranges[index],
            source_line: index,
            foldable,
            folded: folded_region.is_some(),
        });
        if let Some(region) = folded_region {
            index = region.end_line.saturating_add(1);
        } else {
            index += 1;
        }
    }
    visual_lines
}

fn collect_fold_regions(text: &str, line_ranges: &[LineRange]) -> Vec<FoldRegion> {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut regions = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for (offset, character) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            continue;
        }

        if matches!(character, '{' | '[' | '(') {
            stack.push((character, offset));
        } else if matches!(character, '}' | ']' | ')') {
            let Some((open, open_offset)) = stack.pop() else {
                continue;
            };
            if !brackets_match(open, character) {
                continue;
            }
            let start_line = line_index_for_offset(line_ranges, open_offset);
            let end_line = line_index_for_offset(line_ranges, offset);
            if end_line > start_line {
                regions.push(FoldRegion {
                    start_line,
                    end_line,
                });
            }
        }
    }

    regions.sort_by_key(|region| (region.start_line, region.end_line));
    regions
}

fn brackets_match(open: char, close: char) -> bool {
    matches!((open, close), ('{', '}') | ('[', ']') | ('(', ')'))
}

fn syntax_palette(colors: &ThemeColors) -> SyntaxPalette {
    if colors.bg.l < 0.5 {
        SyntaxPalette {
            key: colors.accent,
            string: colors.badge_stable_text,
            number: colors.badge_beta_text,
            keyword: hsla(colors.accent.h + 0.16, 0.68, 0.74, 1.0),
            function: hsla(colors.accent.h + 0.08, 0.78, 0.70, 1.0),
            punctuation: hsla(colors.text_secondary.h, 0.30, 0.74, 1.0),
            comment: hsla(colors.text_muted.h, 0.18, 0.58, 1.0),
            diagnostic: hsla(colors.danger.h, 0.78, 0.66, 1.0),
        }
    } else {
        SyntaxPalette {
            key: hsla(colors.accent.h, 0.72, 0.46, 1.0),
            string: hsla(colors.stat_green_text.h, 0.56, 0.36, 1.0),
            number: hsla(colors.stat_orange_text.h, 0.78, 0.46, 1.0),
            keyword: hsla(colors.accent.h + 0.18, 0.58, 0.52, 1.0),
            function: hsla(colors.badge_beta_text.h, 0.72, 0.40, 1.0),
            punctuation: hsla(colors.text_secondary.h, 0.24, 0.46, 1.0),
            comment: hsla(colors.text_muted.h, 0.14, 0.52, 1.0),
            diagnostic: hsla(colors.danger.h, 0.78, 0.54, 1.0),
        }
    }
}

fn code_highlights(
    text: &str,
    language: CodeEditorLanguage,
    palette: &SyntaxPalette,
) -> Vec<HighlightRange> {
    let language = match language {
        CodeEditorLanguage::Auto if looks_like_json(text) => CodeEditorLanguage::JsonNbt,
        CodeEditorLanguage::Auto => CodeEditorLanguage::GenericCode,
        language => language,
    };
    let mut ranges = match language {
        CodeEditorLanguage::JsonNbt => json_nbt_highlights(text, palette),
        CodeEditorLanguage::GenericCode | CodeEditorLanguage::Auto => {
            generic_code_highlights(text, palette)
        }
    };
    ranges.sort_by_key(|range| (range.start, range.end));
    ranges
}

fn looks_like_json(text: &str) -> bool {
    text.trim_start()
        .chars()
        .next()
        .is_some_and(|character| matches!(character, '{' | '['))
}

fn json_nbt_highlights(text: &str, palette: &SyntaxPalette) -> Vec<HighlightRange> {
    syntax_highlights(text, true, palette)
}

fn generic_code_highlights(text: &str, palette: &SyntaxPalette) -> Vec<HighlightRange> {
    let mut ranges = syntax_highlights(text, false, palette);
    ranges.extend(comment_highlights(text, palette));
    ranges
}

fn syntax_highlights(text: &str, json_keys: bool, palette: &SyntaxPalette) -> Vec<HighlightRange> {
    let mut ranges = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if character.is_whitespace() {
            index += character.len_utf8();
            continue;
        }

        if character == '"' {
            let end = string_end(text, index).unwrap_or(text.len());
            let color = if json_keys && is_json_key(text, end) {
                palette.key
            } else {
                palette.string
            };
            ranges.push(HighlightRange {
                start: index,
                end,
                color,
            });
            index = end;
            continue;
        }

        if character.is_ascii_digit()
            || (character == '-'
                && text[index + character.len_utf8()..]
                    .chars()
                    .next()
                    .is_some_and(|next| next.is_ascii_digit()))
        {
            let end = number_end(text, index);
            ranges.push(HighlightRange {
                start: index,
                end,
                color: palette.number,
            });
            index = end;
            continue;
        }

        if is_identifier_start(character) {
            let end = identifier_end(text, index);
            let identifier = &text[index..end];
            let color = if matches!(
                identifier,
                "true"
                    | "false"
                    | "null"
                    | "let"
                    | "const"
                    | "fn"
                    | "function"
                    | "return"
                    | "if"
                    | "else"
                    | "for"
                    | "while"
                    | "pub"
                    | "struct"
                    | "enum"
                    | "impl"
                    | "use"
                    | "mod"
            ) {
                Some(palette.keyword)
            } else if next_non_whitespace(text, end) == Some('(') {
                Some(palette.function)
            } else {
                None
            };
            if let Some(color) = color {
                ranges.push(HighlightRange {
                    start: index,
                    end,
                    color,
                });
            }
            index = end;
            continue;
        }

        if matches!(
            character,
            '{' | '}' | '[' | ']' | '(' | ')' | ':' | ',' | ';'
        ) {
            let end = index + character.len_utf8();
            ranges.push(HighlightRange {
                start: index,
                end,
                color: palette.punctuation,
            });
            index = end;
            continue;
        }

        index += character.len_utf8();
    }
    ranges
}

fn comment_highlights(text: &str, palette: &SyntaxPalette) -> Vec<HighlightRange> {
    let mut ranges = Vec::new();
    let mut index = 0;
    let bytes = text.as_bytes();
    let mut in_string = false;
    let mut escaped = false;

    while index < bytes.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };

        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            index += character.len_utf8();
            continue;
        }

        if character == '"' {
            in_string = true;
            index += character.len_utf8();
            continue;
        }

        if text[index..].starts_with("//") {
            let end = text[index..]
                .find('\n')
                .map_or(text.len(), |offset| index + offset);
            ranges.push(HighlightRange {
                start: index,
                end,
                color: palette.comment,
            });
            index = end;
            continue;
        }

        if text[index..].starts_with("/*") {
            let end = text[index + 2..]
                .find("*/")
                .map_or(text.len(), |offset| index + 2 + offset + 2);
            ranges.push(HighlightRange {
                start: index,
                end,
                color: palette.comment,
            });
            index = end;
            continue;
        }

        index += character.len_utf8();
    }

    ranges
}

fn format_diagnostics(
    text: &str,
    language: CodeEditorLanguage,
    palette: &SyntaxPalette,
) -> Vec<DiagnosticRange> {
    let mut ranges = Vec::new();
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;
    let mut string_start = 0;

    for (offset, character) in text.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_string = false;
            }
            continue;
        }

        if character == '"' {
            in_string = true;
            string_start = offset;
        } else if matches!(character, '{' | '[' | '(') {
            stack.push((character, offset));
        } else if matches!(character, '}' | ']' | ')') {
            match stack.pop() {
                Some((open, _)) if brackets_match(open, character) => {}
                Some((_, open_offset)) => {
                    ranges.push(diagnostic_range(open_offset, open_offset + 1, palette));
                    ranges.push(diagnostic_range(
                        offset,
                        offset + character.len_utf8(),
                        palette,
                    ));
                }
                None => ranges.push(diagnostic_range(
                    offset,
                    offset + character.len_utf8(),
                    palette,
                )),
            }
        }
    }

    if language == CodeEditorLanguage::JsonNbt && !text.trim().is_empty() {
        let trimmed = text.trim_start();
        if !matches!(trimmed.chars().next(), Some('{') | Some('[')) {
            ranges.push(diagnostic_range(0, text.len().min(1), palette));
        }
    }

    if in_string {
        ranges.push(diagnostic_range(
            string_start,
            text.len().max(string_start + 1),
            palette,
        ));
    }
    for (_, offset) in stack {
        ranges.push(diagnostic_range(offset, offset + 1, palette));
    }
    ranges
}

fn diagnostic_range(start: usize, end: usize, palette: &SyntaxPalette) -> DiagnosticRange {
    DiagnosticRange {
        start,
        end,
        color: palette.diagnostic,
    }
}

fn string_end(text: &str, start: usize) -> Option<usize> {
    let mut escaped = false;
    for (offset, character) in text[start + 1..].char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '"' {
            return Some(start + 1 + offset + character.len_utf8());
        }
    }
    None
}

fn is_json_key(text: &str, string_end: usize) -> bool {
    next_non_whitespace(text, string_end) == Some(':')
}

fn next_non_whitespace(text: &str, start: usize) -> Option<char> {
    text[start..]
        .chars()
        .find(|character| !character.is_whitespace())
}

fn number_end(text: &str, start: usize) -> usize {
    let mut end = start;
    for (offset, character) in text[start..].char_indices() {
        if offset == 0 && character == '-' {
            end = start + character.len_utf8();
            continue;
        }
        if character.is_ascii_digit() || matches!(character, '.' | 'e' | 'E' | '+' | '-') {
            end = start + offset + character.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn identifier_end(text: &str, start: usize) -> usize {
    let mut end = start;
    for (offset, character) in text[start..].char_indices() {
        if offset == 0 {
            end = start + character.len_utf8();
            continue;
        }
        if is_identifier_continue(character) {
            end = start + offset + character.len_utf8();
        } else {
            break;
        }
    }
    end
}

fn is_identifier_start(character: char) -> bool {
    character == '_' || character.is_ascii_alphabetic()
}

fn is_identifier_continue(character: char) -> bool {
    is_identifier_start(character) || character.is_ascii_digit()
}

fn line_runs(
    line_text: &SharedString,
    line_start: usize,
    marked_range: Option<&Range<usize>>,
    highlights: &[HighlightRange],
    font: gpui::Font,
    text_color: Hsla,
) -> Vec<TextRun> {
    let base_run = TextRun {
        len: line_text.len(),
        font,
        color: text_color,
        background_color: None,
        background_corner_radius: None,
        background_padding: None,
        underline: None,
        strikethrough: None,
    };

    let line_end = line_start + line_text.len();
    let mut boundaries = vec![line_start, line_end];
    for highlight in highlights {
        let overlap_start = highlight.start.max(line_start);
        let overlap_end = highlight.end.min(line_end);
        if overlap_start < overlap_end {
            boundaries.push(overlap_start);
            boundaries.push(overlap_end);
        }
    }
    if let Some(marked_range) = marked_range {
        let overlap_start = marked_range.start.max(line_start);
        let overlap_end = marked_range.end.min(line_end);
        if overlap_start < overlap_end {
            boundaries.push(overlap_start);
            boundaries.push(overlap_end);
        }
    }
    boundaries.sort_unstable();
    boundaries.dedup();

    if boundaries.len() <= 2 {
        return vec![TextRun {
            font: base_run.font.clone(),
            ..base_run
        }];
    }

    let mut runs = Vec::new();
    for range in boundaries.windows(2) {
        let start = range[0];
        let end = range[1];
        if start == end {
            continue;
        }
        let highlight = highlights
            .iter()
            .rev()
            .find(|highlight| highlight.start <= start && highlight.end >= end);
        let marked = marked_range
            .is_some_and(|marked_range| marked_range.start <= start && marked_range.end >= end);
        runs.push(TextRun {
            len: end - start,
            font: base_run.font.clone(),
            color: highlight.map_or(base_run.color, |highlight| highlight.color),
            background_color: None,
            background_corner_radius: None,
            background_padding: None,
            underline: marked.then_some(UnderlineStyle {
                color: Some(base_run.color),
                thickness: px(1.0),
                wavy: false,
            }),
            strikethrough: None,
        });
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_nbt_highlights_keys_and_literals() {
        let text = r#"{"Name": true, "Count": 3}"#;
        let colors = crate::ui::theme::colors::LightColors::colors();
        let highlights =
            code_highlights(text, CodeEditorLanguage::JsonNbt, &syntax_palette(&colors));

        assert!(
            highlights
                .iter()
                .any(|range| &text[range.start..range.end] == r#""Name""#)
        );
        assert!(
            highlights
                .iter()
                .any(|range| &text[range.start..range.end] == "true")
        );
        assert!(
            highlights
                .iter()
                .any(|range| &text[range.start..range.end] == "3")
        );
    }

    #[test]
    fn diagnostics_mark_unclosed_string_and_bad_bracket() {
        let text = r#"{"Name": "Steve]"#;
        let colors = crate::ui::theme::colors::LightColors::colors();
        let diagnostics =
            format_diagnostics(text, CodeEditorLanguage::JsonNbt, &syntax_palette(&colors));

        assert!(!diagnostics.is_empty());
    }

    #[test]
    fn json_highlights_use_multiple_colors() {
        let text = r#"{"Name": true, "Count": 3}"#;
        let colors = crate::ui::theme::colors::LightColors::colors();
        let highlights =
            code_highlights(text, CodeEditorLanguage::JsonNbt, &syntax_palette(&colors));

        let key_color = highlights
            .iter()
            .find(|range| &text[range.start..range.end] == r#""Name""#)
            .map(|range| range.color)
            .expect("json key highlight should exist");
        let bool_color = highlights
            .iter()
            .find(|range| &text[range.start..range.end] == "true")
            .map(|range| range.color)
            .expect("json bool highlight should exist");
        let number_color = highlights
            .iter()
            .find(|range| &text[range.start..range.end] == "3")
            .map(|range| range.color)
            .expect("json number highlight should exist");

        assert_ne!(key_color, bool_color);
        assert_ne!(bool_color, number_color);
    }

    #[test]
    fn syntax_palette_changes_between_light_and_dark_themes() {
        let light = syntax_palette(&crate::ui::theme::colors::LightColors::colors());
        let dark = syntax_palette(&crate::ui::theme::colors::DarkColors::colors());

        assert_ne!(light.key, dark.key);
        assert_ne!(light.string, dark.string);
        assert_ne!(light.diagnostic, dark.diagnostic);
    }

    #[test]
    fn fold_regions_cover_nested_multiline_blocks() {
        let text = "{\n  \"Outer\": {\n    \"Inner\": 1\n  }\n}";
        let lines = collect_line_ranges(text);
        let regions = collect_fold_regions(text, &lines);

        assert!(
            regions
                .iter()
                .any(|region| region.start_line == 0 && region.end_line == 4)
        );
        assert!(
            regions
                .iter()
                .any(|region| region.start_line == 1 && region.end_line == 3)
        );
    }

    #[test]
    fn collapsed_visual_lines_skip_hidden_region() {
        let text = "{\n  \"Outer\": {\n    \"Inner\": 1\n  }\n}";
        let lines = collect_line_ranges(text);
        let regions = collect_fold_regions(text, &lines);
        let visual_lines = collect_visual_lines(&lines, &regions, &[1]);

        assert_eq!(visual_lines.len(), 3);
        assert!(visual_lines[1].folded);
        assert_eq!(visual_lines[2].source_line, 4);
    }

    #[test]
    fn scroll_offset_is_clamped_to_content_bounds() {
        let offset = point(px(900.0), px(-20.0));
        let content = size(px(1000.0), px(800.0));
        let viewport = size(px(320.0), px(240.0));

        assert_eq!(
            clamp_scroll_offset(offset, content, viewport),
            point(px(680.0), px(0.0))
        );
    }

    #[test]
    fn visible_line_range_limits_work_to_viewport_with_overscan() {
        assert_eq!(
            visible_line_range(px(2_000.0), px(200.0), px(20.0), 10_000),
            97..112
        );
        assert_eq!(visible_line_range(px(0.0), px(200.0), px(20.0), 5), 0..5);
    }

    #[test]
    fn wheel_delta_maps_to_positive_scroll_offset_when_scrolling_down() {
        assert_eq!(
            scroll_offset_delta_from_wheel_delta(point(px(0.0), px(-40.0)), false),
            point(px(0.0), px(40.0))
        );
        assert_eq!(
            scroll_offset_delta_from_wheel_delta(point(px(0.0), px(-40.0)), true),
            point(px(40.0), px(0.0))
        );
    }

    #[test]
    fn scrollbar_axis_detection_prefers_scrollbars_over_text_selection() {
        let vertical = Bounds::new(point(px(286.0), px(12.0)), size(px(10.0), px(64.0)));
        let horizontal = Bounds::new(point(px(12.0), px(186.0)), size(px(72.0), px(10.0)));

        assert_eq!(
            scrollbar_axis_at_position(
                point(px(290.0), px(40.0)),
                Some(vertical),
                Some(horizontal)
            ),
            Some(ScrollbarAxis::Vertical)
        );
        assert_eq!(
            scrollbar_axis_at_position(
                point(px(40.0), px(190.0)),
                Some(vertical),
                Some(horizontal)
            ),
            Some(ScrollbarAxis::Horizontal)
        );
        assert_eq!(
            scrollbar_axis_at_position(
                point(px(120.0), px(80.0)),
                Some(vertical),
                Some(horizontal)
            ),
            None
        );
    }

    #[test]
    fn stale_pointer_release_keeps_active_left_drag() {
        let mut is_selecting = true;
        let mut scrollbar_drag = Some(ScrollbarDrag {
            axis: ScrollbarAxis::Vertical,
            pointer_origin: point(px(0.0), px(0.0)),
            scroll_origin: point(px(0.0), px(0.0)),
        });

        let release = release_stale_code_editor_pointer_interaction(
            &mut is_selecting,
            &mut scrollbar_drag,
            Some(MouseButton::Left),
        );

        assert_eq!(release, CodeEditorPointerRelease::default());
        assert!(is_selecting);
        assert!(scrollbar_drag.is_some());
    }

    #[test]
    fn stale_pointer_release_clears_selection_and_scrollbar_drag() {
        let mut is_selecting = true;
        let mut scrollbar_drag = Some(ScrollbarDrag {
            axis: ScrollbarAxis::Horizontal,
            pointer_origin: point(px(1.0), px(2.0)),
            scroll_origin: point(px(3.0), px(4.0)),
        });

        let release = release_stale_code_editor_pointer_interaction(
            &mut is_selecting,
            &mut scrollbar_drag,
            None,
        );

        assert_eq!(
            release,
            CodeEditorPointerRelease {
                interaction: true,
                scrollbar_drag: true,
            }
        );
        assert!(!is_selecting);
        assert!(scrollbar_drag.is_none());
    }
}
