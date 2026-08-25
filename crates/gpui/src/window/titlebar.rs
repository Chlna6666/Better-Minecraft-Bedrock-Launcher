use super::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use crate::{FontRun, FontWeight, GlyphRasterization, PlatformTextSystem, RenderGlyphParams, font};

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const PLATFORM_TITLE_FONT_SIZE: Pixels = px(14.0);
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const PLATFORM_TITLE_MASK_PADDING: i32 = 1;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) struct PlatformTitleMask {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) alpha: Vec<u8>,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
struct RasterizedTitleGlyph {
    origin: Point<i32>,
    size: Size<DevicePixels>,
    alpha: Vec<u8>,
}

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) fn rasterize_platform_title(
    text_system: &dyn PlatformTextSystem,
    title: &str,
    scale_factor: f32,
) -> Result<Option<PlatformTitleMask>> {
    if title.is_empty() || !scale_factor.is_finite() || scale_factor <= 0.0 {
        return Ok(None);
    }

    let mut title_font = font(".SystemUIFont");
    title_font.weight = FontWeight::NORMAL;
    let normal_font_id = text_system.font_id(&title_font)?;
    title_font.weight = FontWeight::SEMIBOLD;
    let font_id = match text_system.font_id(&title_font) {
        Ok(font_id) => font_id,
        Err(error) => {
            log::debug!("platform title font has no semibold face; using normal weight: {error:#}");
            normal_font_id
        }
    };
    let layout = text_system.layout_line(
        title,
        PLATFORM_TITLE_FONT_SIZE,
        &[FontRun {
            len: title.len(),
            font_id,
        }],
    );
    let baseline = px((layout.ascent.0 * scale_factor).round() / scale_factor);
    let mut glyph_origin = point(px(0.0), px(0.0));
    let mut previous_glyph_position = Point::default();
    let mut rasterized_glyphs = Vec::new();
    let mut minimum = point(i32::MAX, i32::MAX);
    let mut maximum = point(i32::MIN, i32::MIN);

    for run in &layout.runs {
        for glyph in &run.glyphs {
            glyph_origin.x += glyph.position.x - previous_glyph_position.x;
            previous_glyph_position = glyph.position;
            let paint_origin = glyph_origin + glyph.render_offset + point(px(0.0), baseline);
            let (_, subpixel_variant) =
                glyph_device_origin(paint_origin, Point::default(), scale_factor);
            let params = RenderGlyphParams {
                font_id: run.font_id,
                glyph_id: glyph.id,
                font_size: glyph.font_size,
                subpixel_variant,
                scale_factor,
                is_emoji: glyph.is_emoji,
                is_cjk: glyph.is_cjk,
            };
            let raster_bounds = text_system.glyph_raster_bounds(&params)?;
            if raster_bounds.size.width.0 <= 0 || raster_bounds.size.height.0 <= 0 {
                continue;
            }
            let rasterization = text_system.rasterize_glyph(&params, raster_bounds)?;
            let (size, alpha) = match rasterization {
                GlyphRasterization::Bitmap { size, bytes } => (size, bytes),
                GlyphRasterization::ColorLayers { fallback, .. } => (fallback.size, fallback.bytes),
            };
            if size.width.0 <= 0 || size.height.0 <= 0 {
                continue;
            }

            let (device_origin, _) =
                glyph_device_origin(paint_origin, raster_bounds.origin, scale_factor);
            let origin = point(device_origin.x.0 as i32, device_origin.y.0 as i32);
            minimum.x = minimum.x.min(origin.x);
            minimum.y = minimum.y.min(origin.y);
            maximum.x = maximum.x.max(origin.x.saturating_add(size.width.0));
            maximum.y = maximum.y.max(origin.y.saturating_add(size.height.0));
            rasterized_glyphs.push(RasterizedTitleGlyph {
                origin,
                size,
                alpha,
            });
        }
    }

    if rasterized_glyphs.is_empty() {
        return Ok(None);
    }

    let width = maximum
        .x
        .saturating_sub(minimum.x)
        .saturating_add(PLATFORM_TITLE_MASK_PADDING * 2);
    let height = maximum
        .y
        .saturating_sub(minimum.y)
        .saturating_add(PLATFORM_TITLE_MASK_PADDING * 2);
    let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) else {
        return Ok(None);
    };
    let Some(pixel_count) = usize::try_from(width).ok().and_then(|width| {
        usize::try_from(height)
            .ok()
            .and_then(|height| width.checked_mul(height))
    }) else {
        return Ok(None);
    };
    let mut title_alpha = vec![0; pixel_count];

    for glyph in rasterized_glyphs {
        let glyph_width = usize::try_from(glyph.size.width.0).unwrap_or_default();
        let glyph_height = usize::try_from(glyph.size.height.0).unwrap_or_default();
        if glyph_width == 0
            || glyph_height == 0
            || glyph.alpha.len() != glyph_width.saturating_mul(glyph_height)
        {
            continue;
        }
        let destination_x = glyph
            .origin
            .x
            .saturating_sub(minimum.x)
            .saturating_add(PLATFORM_TITLE_MASK_PADDING);
        let destination_y = glyph
            .origin
            .y
            .saturating_sub(minimum.y)
            .saturating_add(PLATFORM_TITLE_MASK_PADDING);
        let (Ok(destination_x), Ok(destination_y)) = (
            usize::try_from(destination_x),
            usize::try_from(destination_y),
        ) else {
            continue;
        };
        let output_width = width as usize;
        let output_height = height as usize;

        for row in 0..glyph_height {
            let output_y = destination_y.saturating_add(row);
            if output_y >= output_height {
                break;
            }
            for column in 0..glyph_width {
                let output_x = destination_x.saturating_add(column);
                if output_x >= output_width {
                    break;
                }
                let source = glyph.alpha[row * glyph_width + column];
                let destination = &mut title_alpha[output_y * output_width + output_x];
                let remaining = u16::from(*destination) * u16::from(255 - source) / 255;
                *destination =
                    u8::try_from(u16::from(source).saturating_add(remaining)).unwrap_or(u8::MAX);
            }
        }
    }

    Ok(Some(PlatformTitleMask {
        width,
        height,
        alpha: title_alpha,
    }))
}

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
pub struct TitlebarGesture {
    drag_armed: bool,
    drag_down_pos: Point<Pixels>,
    last_down_at: Option<Instant>,
    last_down_pos: Point<Pixels>,
    drag_threshold_px: f32,
}

impl Default for TitlebarGesture {
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

impl TitlebarGesture {
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
        let mut state = TitlebarGesture::default();
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
        let mut state = TitlebarGesture::with_drag_threshold(6.0);
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

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    #[test]
    fn rasterizes_native_title_with_gpui_text_system() {
        let text_system = crate::CosmicTextSystem::new();
        text_system.set_application_font_family(".SystemUIFont".into());
        let title = rasterize_platform_title(&text_system, "地图预览 - 我的世界", 1.0)
            .expect("GPUI title rasterization should succeed")
            .expect("non-empty title should produce a mask");

        assert!(title.width > title.height);
        assert!(title.alpha.iter().any(|alpha| *alpha != 0));
    }
}
