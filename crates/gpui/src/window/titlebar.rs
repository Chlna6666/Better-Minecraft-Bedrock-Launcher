use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GlyphSubpixelBin {
    integer_position: i32,
    variant: u8,
}

fn glyph_subpixel_bin(position: f32) -> GlyphSubpixelBin {
    let trunc = position as i32;
    let fract = position - trunc as f32;

    let (integer_position, variant) = if position.is_sign_negative() {
        if fract > -0.125 {
            (trunc, 0)
        } else if fract > -0.375 {
            (trunc - 1, 3)
        } else if fract > -0.625 {
            (trunc - 1, 2)
        } else if fract > -0.875 {
            (trunc - 1, 1)
        } else {
            (trunc - 1, 0)
        }
    } else if fract < 0.125 {
        (trunc, 0)
    } else if fract < 0.375 {
        (trunc, 1)
    } else if fract < 0.625 {
        (trunc, 2)
    } else if fract < 0.875 {
        (trunc, 3)
    } else {
        (trunc + 1, 0)
    };

    GlyphSubpixelBin {
        integer_position,
        variant,
    }
}

fn glyph_y_subpixel_bin(position: f32) -> GlyphSubpixelBin {
    if SUBPIXEL_VARIANTS_Y == 1 {
        GlyphSubpixelBin {
            integer_position: position.round() as i32,
            variant: 0,
        }
    } else {
        glyph_subpixel_bin(position)
    }
}

pub(crate) fn glyph_device_origin(
    origin: Point<Pixels>,
    raster_origin: Point<DevicePixels>,
    scale_factor: f32,
) -> (Point<ScaledPixels>, Point<u8>) {
    let glyph_origin = origin.scale(scale_factor);
    let x_bin = glyph_subpixel_bin(glyph_origin.x.0);
    let y_bin = glyph_y_subpixel_bin(glyph_origin.y.0);
    (
        Point::new(
            ScaledPixels(x_bin.integer_position as f32),
            ScaledPixels(y_bin.integer_position as f32),
        ) + raster_origin.map(Into::into),
        Point::new(x_bin.variant, y_bin.variant),
    )
}

pub(crate) fn svg_paint_bounds_for_requested_bounds(
    bounds: Bounds<ScaledPixels>,
) -> Bounds<ScaledPixels> {
    bounds
        .map_origin(|origin| origin.round())
        .map_size(|size| size.ceil())
}

pub(crate) fn svg_raster_size_for_paint_bounds(bounds: Bounds<ScaledPixels>) -> Size<DevicePixels> {
    bounds
        .size
        .map(|pixels| DevicePixels((pixels.0 * SMOOTH_SVG_SCALE_FACTOR).round() as i32))
}

/// State for implementing a client-side titlebar with native drag and double-click behavior.
#[derive(Clone, Debug)]
pub struct TitlebarGestureState {
    drag_armed: bool,
    drag_down_pos: Point<Pixels>,
    last_down_at: Option<Instant>,
    last_down_pos: Point<Pixels>,
    drag_threshold_px: f32,
}

impl Default for TitlebarGestureState {
    fn default() -> Self {
        Self {
            drag_armed: false,
            drag_down_pos: Point::default(),
            last_down_at: None,
            last_down_pos: Point::default(),
            drag_threshold_px: 2.0,
        }
    }
}

impl TitlebarGestureState {
    /// Create a titlebar gesture state with the default drag threshold.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a titlebar gesture state with a custom drag threshold in logical pixels.
    pub fn with_drag_threshold(drag_threshold_px: f32) -> Self {
        Self {
            drag_threshold_px,
            ..Self::default()
        }
    }

    /// Returns whether a mouse down should trigger the platform titlebar double-click action.
    pub fn mouse_down(&mut self, event: &MouseDownEvent, now: Instant) -> bool {
        let (double_duration, double_delta_x, double_delta_y) = titlebar_double_click_settings();

        let mut is_double = event.click_count == 2;
        if !is_double && let Some(last_down_at) = self.last_down_at {
            let elapsed = now.saturating_duration_since(last_down_at);
            let delta_x = ((event.position.x - self.last_down_pos.x) / px(1.0)).abs();
            let delta_y = ((event.position.y - self.last_down_pos.y) / px(1.0)).abs();
            if elapsed <= double_duration && delta_x <= double_delta_x && delta_y <= double_delta_y
            {
                is_double = true;
            }
        }

        self.last_down_at = Some(now);
        self.last_down_pos = event.position;
        self.drag_armed = !is_double;
        self.drag_down_pos = event.position;
        is_double
    }

    /// Returns whether native window dragging should begin for this mouse move.
    pub fn should_start_drag(&self, event: &MouseMoveEvent) -> bool {
        if !self.drag_armed || !event.dragging() {
            return false;
        }

        let delta_x = ((event.position.x - self.drag_down_pos.x) / px(1.0)).abs();
        let delta_y = ((event.position.y - self.drag_down_pos.y) / px(1.0)).abs();
        delta_x.max(delta_y) >= self.drag_threshold_px
    }

    /// Disarm a pending titlebar drag.
    pub fn disarm(&mut self) {
        self.drag_armed = false;
    }

    /// Handle a titlebar mouse down against a window.
    pub fn handle_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, now: Instant) {
        if self.mouse_down(event, now) {
            window.titlebar_double_click();
        }
    }

    /// Handle a titlebar mouse move against a window.
    pub fn handle_mouse_move(&mut self, event: &MouseMoveEvent, window: &mut Window) {
        if self.should_start_drag(event) {
            self.disarm();
            window.start_window_move();
        }
    }

    /// Handle a titlebar mouse up.
    pub fn handle_mouse_up(&mut self) {
        self.disarm();
    }
}

fn titlebar_double_click_settings() -> (Duration, f32, f32) {
    (Duration::from_millis(500), 6.0, 6.0)
}

pub(crate) fn resize_edge_hit_test(
    window: &Window,
    position: Point<Pixels>,
    inset: Pixels,
) -> Option<ResizeEdge> {
    if inset <= px(0.) || window.is_maximized() || window.is_fullscreen() {
        return None;
    }

    let width = window.viewport_size.width;
    let height = window.viewport_size.height;

    if position.x < px(0.) || position.y < px(0.) || position.x > width || position.y > height {
        return None;
    }

    let left = position.x <= inset;
    let right = position.x >= width - inset;
    let top = position.y <= inset;
    let bottom = position.y >= height - inset;

    match (left, right, top, bottom) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, _, _, _) => Some(ResizeEdge::Left),
        (_, true, _, _) => Some(ResizeEdge::Right),
        (_, _, true, _) => Some(ResizeEdge::Top),
        (_, _, _, true) => Some(ResizeEdge::Bottom),
        _ => None,
    }
}

pub(crate) fn resize_edge_cursor_style(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

#[cfg(test)]
mod titlebar_gesture_tests {
    use super::*;

    fn mouse_down(position: Point<Pixels>, click_count: usize) -> MouseDownEvent {
        MouseDownEvent {
            button: MouseButton::Left,
            position,
            click_count,
            ..Default::default()
        }
    }

    fn mouse_move(position: Point<Pixels>) -> MouseMoveEvent {
        MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            ..Default::default()
        }
    }

    #[test]
    fn detects_platform_double_clicks_and_disarms_drag() {
        let mut state = TitlebarGestureState::default();
        let now = Instant::now();

        assert!(!state.mouse_down(&mouse_down(point(px(10.0), px(10.0)), 1), now));
        assert!(state.should_start_drag(&mouse_move(point(px(16.0), px(10.0)))));

        assert!(state.mouse_down(
            &mouse_down(point(px(10.0), px(10.0)), 2),
            now + Duration::from_millis(10)
        ));
        assert!(!state.should_start_drag(&mouse_move(point(px(20.0), px(10.0)))));
    }

    #[test]
    fn uses_configured_drag_threshold() {
        let mut state = TitlebarGestureState::with_drag_threshold(6.0);
        let now = Instant::now();

        assert!(!state.mouse_down(&mouse_down(point(px(10.0), px(10.0)), 1), now));
        assert!(!state.should_start_drag(&mouse_move(point(px(15.0), px(10.0)))));
        assert!(state.should_start_drag(&mouse_move(point(px(16.0), px(10.0)))));
    }

    #[test]
    fn glyph_subpixel_bins_match_cosmic_text_boundaries() {
        assert_eq!(
            glyph_subpixel_bin(0.124),
            GlyphSubpixelBin {
                integer_position: 0,
                variant: 0
            }
        );
        assert_eq!(
            glyph_subpixel_bin(0.125),
            GlyphSubpixelBin {
                integer_position: 0,
                variant: 1
            }
        );
        assert_eq!(
            glyph_subpixel_bin(0.625),
            GlyphSubpixelBin {
                integer_position: 0,
                variant: 3
            }
        );
        assert_eq!(
            glyph_subpixel_bin(0.875),
            GlyphSubpixelBin {
                integer_position: 1,
                variant: 0
            }
        );
        assert_eq!(
            glyph_subpixel_bin(-0.125),
            GlyphSubpixelBin {
                integer_position: -1,
                variant: 3
            }
        );
        assert_eq!(
            glyph_subpixel_bin(-0.875),
            GlyphSubpixelBin {
                integer_position: -1,
                variant: 0
            }
        );
    }

    #[test]
    fn glyph_y_subpixel_bin_rounds_when_y_subpixel_is_disabled() {
        if SUBPIXEL_VARIANTS_Y == 1 {
            assert_eq!(
                glyph_y_subpixel_bin(0.875),
                GlyphSubpixelBin {
                    integer_position: 1,
                    variant: 0
                }
            );
            assert_eq!(
                glyph_y_subpixel_bin(12.999),
                GlyphSubpixelBin {
                    integer_position: 13,
                    variant: 0
                }
            );
        }
    }

    #[test]
    fn glyph_device_origin_rounds_baseline_y_before_raster_offset() {
        let (origin, variant) = glyph_device_origin(
            point(px(10.25), px(20.875)),
            point(DevicePixels(-1), DevicePixels(-12)),
            1.0,
        );

        assert_eq!(variant, point(1, 0));
        assert_eq!(origin, point(ScaledPixels(9.0), ScaledPixels(9.0)));
    }

    #[test]
    fn svg_paint_bounds_preserve_requested_scaled_bounds() {
        let requested = Bounds {
            origin: point(ScaledPixels(10.25), ScaledPixels(20.5)),
            size: size(ScaledPixels(15.25), ScaledPixels(21.75)),
        };

        let paint_bounds = svg_paint_bounds_for_requested_bounds(requested);

        assert_eq!(
            paint_bounds,
            Bounds {
                origin: point(ScaledPixels(10.0), ScaledPixels(21.0)),
                size: size(ScaledPixels(16.0), ScaledPixels(22.0)),
            }
        );
        assert_eq!(
            svg_raster_size_for_paint_bounds(paint_bounds),
            size(DevicePixels(32), DevicePixels(44))
        );
    }
}
