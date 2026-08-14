use super::*;
use crate::{AbsoluteLength, Length, Timer, rgb};

const SURFACE_FLASH_HOLD: Duration = Duration::from_millis(90);

/// Window-scoped visual diagnostics used by GUI debugging tools.
///
/// These options deliberately add extra paint work and may force full-window presentation. They
/// should only be enabled while diagnosing rendering or layout behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WindowDebugVisualization {
    /// Flash the whole window whenever GPUI produces a new painted surface frame.
    pub flash_surface_updates: bool,
    /// Draw the box model and clipping boundary for styled elements.
    pub show_layout_bounds: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct WindowDebugVisualizationRuntime {
    options: WindowDebugVisualization,
    surface_flash_generation: u64,
    flash_this_frame: bool,
    cleanup_pending: bool,
    cleanup_this_frame: bool,
}

#[derive(Default)]
struct WindowDebugVisualizationRegistry {
    windows: FxHashMap<u64, WindowDebugVisualizationRuntime>,
}

impl Global for WindowDebugVisualizationRegistry {}

impl Window {
    /// Enables or disables visual diagnostics for this window.
    ///
    /// Passing [`WindowDebugVisualization::default`] removes all diagnostic state for the window.
    pub fn set_debug_visualization(
        &mut self,
        options: WindowDebugVisualization,
        cx: &mut App,
    ) {
        cx.default_global::<WindowDebugVisualizationRegistry>();
        let window_id = self.handle.window_id().as_u64();
        cx.update_global(
            |registry: &mut WindowDebugVisualizationRegistry, _cx| {
                if options == WindowDebugVisualization::default() {
                    registry.windows.remove(&window_id);
                    return;
                }

                let runtime = registry.windows.entry(window_id).or_default();
                if runtime.options != options {
                    runtime.options = options;
                    runtime.surface_flash_generation =
                        runtime.surface_flash_generation.wrapping_add(1);
                    runtime.flash_this_frame = false;
                    runtime.cleanup_pending = false;
                    runtime.cleanup_this_frame = false;
                }
            },
        );

        // Turning an overlay on or off must also clear pixels produced by the previous state.
        self.force_full_redraw.set(true);
        self.refresh();
    }

    /// Returns the visual diagnostics currently configured for this window.
    pub fn debug_visualization(&self, cx: &App) -> WindowDebugVisualization {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return WindowDebugVisualization::default();
        }
        cx.global::<WindowDebugVisualizationRegistry>()
            .windows
            .get(&window_id)
            .map(|runtime| runtime.options)
            .unwrap_or_default()
    }

    /// Prepares per-frame visual diagnostics and reports whether this frame must present the full
    /// window. Full presentation is intentional here: layout outlines can span the entire tree and
    /// a surface flash is defined as a window-wide signal.
    pub(super) fn begin_debug_visualization_frame(&mut self, cx: &mut App) -> bool {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return false;
        }

        let mut requires_full_redraw = false;
        cx.update_global(
            |registry: &mut WindowDebugVisualizationRegistry, _cx| {
                let Some(runtime) = registry.windows.get_mut(&window_id) else {
                    return;
                };

                runtime.cleanup_this_frame = runtime.cleanup_pending;
                runtime.cleanup_pending = false;
                runtime.flash_this_frame =
                    runtime.options.flash_surface_updates && !runtime.cleanup_this_frame;
                if runtime.flash_this_frame {
                    runtime.surface_flash_generation =
                        runtime.surface_flash_generation.wrapping_add(1);
                }

                requires_full_redraw = runtime.options.show_layout_bounds
                    || runtime.flash_this_frame
                    || runtime.cleanup_this_frame;
            },
        );
        requires_full_redraw
    }

    /// Paints the surface-update flash above the completed window tree.
    pub(super) fn paint_debug_surface_update_flash(&mut self, cx: &App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        let Some(runtime) = cx
            .global::<WindowDebugVisualizationRegistry>()
            .windows
            .get(&window_id)
            .copied()
        else {
            return;
        };
        if !runtime.flash_this_frame {
            return;
        }

        // Alternate the tint so continuously updating surfaces still visibly pulse instead of
        // settling into one permanent translucent overlay.
        let (hex, alpha) = if runtime.surface_flash_generation & 1 == 0 {
            (0xff2d55, 0.16)
        } else {
            (0xff9500, 0.12)
        };
        let mut color: Hsla = rgb(hex).into();
        color.a = alpha;
        self.paint_quad(fill(
            Bounds::new(Point::default(), self.viewport_size),
            color,
        ));
    }

    /// Schedules a single cleanup frame after the latest flash. A newer real surface update bumps
    /// the generation and supersedes older cleanup timers, avoiding a timer-driven flash loop.
    pub(super) fn finish_debug_visualization_frame(&mut self, cx: &mut App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        let Some(runtime) = cx
            .global::<WindowDebugVisualizationRegistry>()
            .windows
            .get(&window_id)
            .copied()
        else {
            return;
        };
        if !runtime.flash_this_frame {
            return;
        }

        let generation = runtime.surface_flash_generation;
        let handle = self.handle;
        cx.spawn(async move |cx| {
            Timer::after(SURFACE_FLASH_HOLD).await;
            let _ = cx.update(|cx| {
                if !cx.has_global::<WindowDebugVisualizationRegistry>() {
                    return;
                }

                let mut should_refresh = false;
                cx.update_global(
                    |registry: &mut WindowDebugVisualizationRegistry, _cx| {
                        let Some(runtime) = registry.windows.get_mut(&window_id) else {
                            return;
                        };
                        if runtime.options.flash_surface_updates
                            && runtime.surface_flash_generation == generation
                        {
                            runtime.cleanup_pending = true;
                            should_refresh = true;
                        }
                    },
                );

                if should_refresh {
                    let _ = ignore_window_not_found(handle.update(cx, |_root, window, _cx| {
                        window.refresh();
                    }));
                }
            });
        })
        .detach();
    }
}

pub(crate) fn paint_layout_bounds(
    style: &Style,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &App,
) {
    if !window.debug_visualization(cx).show_layout_bounds || bounds.is_empty() {
        return;
    }

    let rem_size = window.rem_size();
    let basis = Size {
        width: AbsoluteLength::Pixels(bounds.size.width),
        height: AbsoluteLength::Pixels(bounds.size.height),
    };
    let margin = resolve_margin(style.margin, basis, rem_size);
    let border = style.border_widths.to_pixels(rem_size);
    // Percentage padding is resolved against the element box here. Absolute px/rem values, which
    // make up the application's normal spacing system, remain exact. This diagnostic path must not
    // perturb the real layout engine merely to recover a parent's percentage basis.
    let padding = style.padding.to_pixels(basis, rem_size);

    let margin_bounds = expand_bounds(bounds, &margin);
    let padding_bounds = inset_bounds(bounds, &border);
    let content_bounds = inset_bounds(padding_bounds, &padding);

    // Box model palette follows the conventional devtools ordering while remaining readable over
    // both light and dark themes: margin/orange, border/blue, padding/green, content/purple.
    if has_non_zero_edges(&margin) {
        paint_outline(window, margin_bounds, 0xff9500, 0.92);
    }
    paint_outline(window, bounds, 0x0a84ff, 0.92);
    if has_non_zero_edges(&border) {
        paint_outline(window, padding_bounds, 0x30d158, 0.92);
    }
    if has_non_zero_edges(&padding) {
        paint_outline(window, content_bounds, 0xbf5af2, 0.92);
    }

    if let Some(mask) = style.overflow_mask(bounds, rem_size) {
        paint_outline(window, mask.bounds, 0xff453a, 0.98);
    }
}

fn resolve_margin(
    margin: Edges<Length>,
    basis: Size<AbsoluteLength>,
    rem_size: Pixels,
) -> Edges<Pixels> {
    Edges {
        top: resolve_margin_length(margin.top, basis.height, rem_size),
        right: resolve_margin_length(margin.right, basis.width, rem_size),
        bottom: resolve_margin_length(margin.bottom, basis.height, rem_size),
        left: resolve_margin_length(margin.left, basis.width, rem_size),
    }
}

fn resolve_margin_length(value: Length, basis: AbsoluteLength, rem_size: Pixels) -> Pixels {
    match value {
        Length::Definite(length) => length.to_pixels(basis, rem_size),
        Length::Auto => Pixels::ZERO,
    }
}

fn expand_bounds(bounds: Bounds<Pixels>, edges: &Edges<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: point(bounds.origin.x - edges.left, bounds.origin.y - edges.top),
        size: size(
            (bounds.size.width + edges.left + edges.right).max(Pixels::ZERO),
            (bounds.size.height + edges.top + edges.bottom).max(Pixels::ZERO),
        ),
    }
}

fn inset_bounds(bounds: Bounds<Pixels>, edges: &Edges<Pixels>) -> Bounds<Pixels> {
    Bounds {
        origin: point(bounds.origin.x + edges.left, bounds.origin.y + edges.top),
        size: size(
            (bounds.size.width - edges.left - edges.right).max(Pixels::ZERO),
            (bounds.size.height - edges.top - edges.bottom).max(Pixels::ZERO),
        ),
    }
}

fn has_non_zero_edges(edges: &Edges<Pixels>) -> bool {
    edges.top != Pixels::ZERO
        || edges.right != Pixels::ZERO
        || edges.bottom != Pixels::ZERO
        || edges.left != Pixels::ZERO
}

fn paint_outline(window: &mut Window, bounds: Bounds<Pixels>, hex: u32, alpha: f32) {
    if bounds.is_empty() {
        return;
    }
    let mut color: Hsla = rgb(hex).into();
    color.a = alpha;
    window.paint_quad(outline(bounds, color, BorderStyle::default()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_model_inset_and_expand_are_inverse_for_positive_edges() {
        let bounds = Bounds::new(point(px(10.0), px(20.0)), size(px(100.0), px(60.0)));
        let edges = Edges {
            top: px(2.0),
            right: px(4.0),
            bottom: px(6.0),
            left: px(8.0),
        };
        assert_eq!(inset_bounds(expand_bounds(bounds, &edges), &edges), bounds);
    }
}
