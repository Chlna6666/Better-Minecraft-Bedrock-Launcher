from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def edit(path: str, replacements):
    p = ROOT / path
    text = p.read_text(encoding="utf-8")
    for old, new in replacements:
        count = text.count(old)
        if count != 1:
            raise SystemExit(f"{path}: expected one anchor, got {count}: {old[:160]!r}")
        text = text.replace(old, new, 1)
    p.write_text(text, encoding="utf-8", newline="\n")


edit(
    "src/ui/window/map_viewer/canvas.rs",
    [
        (
            '''                            .id(("player-map-marker", marker_index))
                            .w(px(28.0))
                            .h(px(28.0))
                            .flex_none()
                            .rounded(px(4.0))
                            .overflow_hidden()
                            .border_2()
                            .border_color(rgb(0xffffff))
                            .bg(Hsla {
                                a: 0.90,
                                ..snapshot.colors.surface
                            })
                            .cursor(CursorStyle::PointingHand)
                            .child(img("images/map/entity/player.png").w(px(28.0)).h(px(28.0)))
''',
            '''                            .id(("player-map-marker", marker_index))
                            // Keep the image inside the border box. Previously the 28x28 image
                            // was placed inside a 28x28 element with a 2px border, so GPUI clipped
                            // four pixels from the avatar on each axis at some DPI scales.
                            .w(px(32.0))
                            .h(px(32.0))
                            .flex_none()
                            .p(px(2.0))
                            .rounded(px(5.0))
                            .overflow_hidden()
                            .border_2()
                            .border_color(rgb(0xffffff))
                            .bg(Hsla {
                                a: 0.90,
                                ..snapshot.colors.surface
                            })
                            .cursor(CursorStyle::PointingHand)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(img("images/map/entity/player.png").w(px(24.0)).h(px(24.0)))
''',
        ),
    ],
)

edit(
    "src/ui/window/map_viewer/paint.rs",
    [
        (
            '''            for point in &overlay_paint.entity_points {
                paint_point_marker(
                    bounds,
                    viewport,
                    layout,
                    point.block_x,
                    point.block_z,
                    rgb(0xf97316).into(),
                    window,
                );
                if avatar_requests.len() >= MAX_ENTITY_AVATAR_REQUESTS {
                    continue;
                }
''',
            '''            for point in &overlay_paint.entity_points {
                // BedrockMap uses either the entity image or its unknown fallback. Do not
                // unconditionally paint our orange fallback underneath a known transparent
                // avatar: transparent pixels (for example shulker.png) otherwise expose the
                // orange square and make a valid icon look like the fallback marker.
                let avatar = point
                    .identifier
                    .as_ref()
                    .and_then(|id| overlay_paint.entity_avatars.get(id));
                if avatar.is_none() || avatar_requests.len() >= MAX_ENTITY_AVATAR_REQUESTS {
                    paint_point_marker(
                        bounds,
                        viewport,
                        layout,
                        point.block_x,
                        point.block_z,
                        rgb(0xf97316).into(),
                        window,
                    );
                }
                if avatar_requests.len() >= MAX_ENTITY_AVATAR_REQUESTS {
                    continue;
                }
''',
        ),
        (
            '''                if let Some(image) = point
                    .identifier
                    .as_ref()
                    .and_then(|id| overlay_paint.entity_avatars.get(id))
                {
''',
            '''                if let Some(image) = avatar {
''',
        ),
    ],
)

edit(
    "src/core/minecraft/entity_avatar.rs",
    [
        (
            '''            "slime",
            "silverfish",
            "magma_cube",
''',
            '''            "slime",
            "silverfish",
            "shulker",
            "magma_cube",
''',
        ),
    ],
)

edit(
    "src/ui/window/map_viewer/tests.rs",
    [
        (
            '''    assert_eq!(
        normalize_entity_avatar_key("entity.minecraft:glow-squid"),
        Some("glow_squid".to_string())
    );
    assert_eq!(normalize_entity_avatar_key("  "), None);
''',
            '''    assert_eq!(
        normalize_entity_avatar_key("entity.minecraft:glow-squid"),
        Some("glow_squid".to_string())
    );
    assert_eq!(
        normalize_entity_avatar_key("minecraft:shulker"),
        Some("shulker".to_string())
    );
    assert_eq!(normalize_entity_avatar_key("  "), None);
''',
        ),
    ],
)

edit(
    "src/ui/components/input.rs",
    [
        (
            '''const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(1000);
const CURSOR_VISIBLE_WINDOW: Duration = Duration::from_millis(530);
''',
            '''const CURSOR_BLINK_PERIOD: Duration = Duration::from_millis(1000);
const CURSOR_VISIBLE_WINDOW: Duration = Duration::from_millis(530);
const INPUT_UNDO_LIMIT: usize = 128;
const INPUT_UNDO_COALESCE_WINDOW: Duration = Duration::from_millis(650);
''',
        ),
        (
            '''        Paste,
        Cut,
        Copy,
        Enter,
''',
            '''        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        Enter,
''',
        ),
        (
            '''        KeyBinding::new("ctrl-v", Paste, Some("Input")),
        KeyBinding::new("ctrl-c", Copy, Some("Input")),
        KeyBinding::new("ctrl-x", Cut, Some("Input")),
''',
            '''        KeyBinding::new("ctrl-v", Paste, Some("Input")),
        KeyBinding::new("ctrl-c", Copy, Some("Input")),
        KeyBinding::new("ctrl-x", Cut, Some("Input")),
        KeyBinding::new("ctrl-z", Undo, Some("Input")),
        KeyBinding::new("ctrl-y", Redo, Some("Input")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("Input")),
''',
        ),
        (
            '''struct InputMetrics {
    height: f32,
    radius: f32,
    gap: f32,
    padding_x: f32,
    clear_slot: f32,
    clear_button: f32,
    clear_text_size: f32,
    text_size: f32,
}
''',
            '''struct InputMetrics {
    height: f32,
    radius: f32,
    gap: f32,
    padding_x: f32,
    clear_slot: f32,
    clear_button: f32,
    clear_text_size: f32,
    text_size: f32,
}

#[derive(Clone)]
struct InputEditSnapshot {
    value: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputEditKind {
    Insert,
    Delete,
    Replace,
}
''',
        ),
        (
            '''    cursor_blink_started_at: Option<Instant>,
    cursor_blink_task_armed: bool,
    cursor_visible_last_frame: bool,
}
''',
            '''    cursor_blink_started_at: Option<Instant>,
    cursor_blink_task_armed: bool,
    cursor_visible_last_frame: bool,
    undo_stack: Vec<InputEditSnapshot>,
    redo_stack: Vec<InputEditSnapshot>,
    last_edit_checkpoint_at: Option<Instant>,
    last_edit_kind: Option<InputEditKind>,
}
''',
        ),
        (
            '''            cursor_blink_started_at: None,
            cursor_blink_task_armed: false,
            cursor_visible_last_frame: false,
        }
''',
            '''            cursor_blink_started_at: None,
            cursor_blink_task_armed: false,
            cursor_visible_last_frame: false,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_edit_checkpoint_at: None,
            last_edit_kind: None,
        }
''',
        ),
        (
            '''        self.value = value;
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn set_text(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
''',
            '''        self.clear_edit_history();
        self.value = value;
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn set_text(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
''',
        ),
        (
            '''        self.value = value;
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn set_placeholder(
''',
            '''        self.clear_edit_history();
        self.value = value;
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn set_placeholder(
''',
        ),
        (
            '''    fn cursor_offset(&self) -> usize {
''',
            '''    fn edit_snapshot(&self) -> InputEditSnapshot {
        InputEditSnapshot {
            value: self.value.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn clear_edit_history(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_edit_checkpoint_at = None;
        self.last_edit_kind = None;
    }

    fn push_edit_checkpoint(&mut self, kind: InputEditKind) {
        let now = Instant::now();
        let coalesce = self.last_edit_kind == Some(kind)
            && !matches!(kind, InputEditKind::Replace)
            && self.last_edit_checkpoint_at.is_some_and(|last| {
                now.saturating_duration_since(last) <= INPUT_UNDO_COALESCE_WINDOW
            });
        if !coalesce {
            if self.undo_stack.len() >= INPUT_UNDO_LIMIT {
                self.undo_stack.remove(0);
            }
            self.undo_stack.push(self.edit_snapshot());
        }
        self.redo_stack.clear();
        self.last_edit_checkpoint_at = Some(now);
        self.last_edit_kind = Some(kind);
    }

    fn restore_edit_snapshot(&mut self, snapshot: InputEditSnapshot, cx: &mut Context<Self>) {
        self.value = snapshot.value;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.is_selecting = false;
        self.reset_cursor_blink();
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.undo_stack.pop() else {
            return;
        };
        let current = self.edit_snapshot();
        if self.redo_stack.len() >= INPUT_UNDO_LIMIT {
            self.redo_stack.remove(0);
        }
        self.redo_stack.push(current);
        self.last_edit_checkpoint_at = None;
        self.last_edit_kind = None;
        self.restore_edit_snapshot(snapshot, cx);
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        let Some(snapshot) = self.redo_stack.pop() else {
            return;
        };
        let current = self.edit_snapshot();
        if self.undo_stack.len() >= INPUT_UNDO_LIMIT {
            self.undo_stack.remove(0);
        }
        self.undo_stack.push(current);
        self.last_edit_checkpoint_at = None;
        self.last_edit_kind = None;
        self.restore_edit_snapshot(snapshot, cx);
    }

    fn cursor_offset(&self) -> usize {
''',
        ),
        (
            '''        if self.value == next_value
            && self.selected_range == next_selected_range
            && !self.selection_reversed
            && self.marked_range.is_none()
        {
            return;
        }

        self.value = next_value;
''',
            '''        if self.value == next_value
            && self.selected_range == next_selected_range
            && !self.selection_reversed
            && self.marked_range.is_none()
        {
            return;
        }

        let edit_kind = if new_text.is_empty() {
            InputEditKind::Delete
        } else if range.is_empty() && new_text.graphemes(true).count() == 1 {
            InputEditKind::Insert
        } else {
            InputEditKind::Replace
        };
        self.push_edit_checkpoint(edit_kind);
        self.value = next_value;
''',
        ),
        (
            '''            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::enter))
''',
            '''            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::enter))
''',
        ),
    ],
)

print("map avatar/input/entity fixes applied")
