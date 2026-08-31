use super::*;
use crate::{AbsoluteLength, Length, Timer, rgb};

const SURFACE_FLASH_HOLD: Duration = Duration::from_millis(90);
const ELEMENT_UPDATE_HOLD: Duration = Duration::from_millis(140);
const MAX_ELEMENT_PAINT_MARKERS: usize = 2048;
const MAX_VIEW_CACHE_MARKERS: usize = 512;

/// Why an [`AnyView`](crate::AnyView) cache entry was reused or rebuilt in the current frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewCacheDebugStatus {
    Hit,
    DeferredDirtyReuse,
    MissCold,
    MissBounds,
    MissContentMask,
    MissTextStyle,
    MissFingerprint,
    MissRefresh,
    MissDirty,
    MissPrepaintRange,
    MissPaintRange,
    ReuseFailed,
}

/// Window-scoped visual diagnostics used by GUI debugging tools.
///
/// These options deliberately add extra paint work. They should only be enabled while diagnosing
/// rendering, caching, or layout behavior.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WindowDebugVisualization {
    /// Flash the whole window whenever GPUI produces a new painted surface frame.
    pub flash_surface_updates: bool,
    /// Draw the box model and clipping boundary for styled elements.
    pub show_layout_bounds: bool,
    /// Outline elements whose paint lifecycle actually executes in the current frame and show
    /// cached-view hit/miss markers.
    ///
    /// A subtree restored through retained paint replay is not traversed, so its descendants do
    /// not get repaint markers. Cached AnyView wrappers receive a cache-status outline on top.
    pub show_element_updates: bool,
}

#[derive(Clone, Copy, Debug)]
struct ViewCacheDebugMarker {
    bounds: Bounds<Pixels>,
    status: ViewCacheDebugStatus,
}

#[derive(Clone, Debug, Default)]
struct WindowDebugVisualizationRuntime {
    options: WindowDebugVisualization,
    surface_flash_generation: u64,
    element_update_generation: u64,
    overlay_generation: u64,
    flash_this_frame: bool,
    element_update_painted_this_frame: bool,
    cleanup_pending: bool,
    cleanup_this_frame: bool,
    element_paint_markers: Vec<Bounds<Pixels>>,
    view_cache_markers: Vec<ViewCacheDebugMarker>,
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
    pub fn set_debug_visualization(&mut self, options: WindowDebugVisualization, cx: &mut App) {
        cx.default_global::<WindowDebugVisualizationRegistry>();
        let window_id = self.handle.window_id().as_u64();
        cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
            if options == WindowDebugVisualization::default() {
                registry.windows.remove(&window_id);
                return;
            }

            let runtime = registry.windows.entry(window_id).or_default();
            if runtime.options != options {
                runtime.options = options;
                runtime.surface_flash_generation = runtime.surface_flash_generation.wrapping_add(1);
                runtime.element_update_generation =
                    runtime.element_update_generation.wrapping_add(1);
                runtime.overlay_generation = runtime.overlay_generation.wrapping_add(1);
                runtime.flash_this_frame = false;
                runtime.element_update_painted_this_frame = false;
                runtime.cleanup_pending = false;
                runtime.cleanup_this_frame = false;
                runtime.element_paint_markers.clear();
                runtime.view_cache_markers.clear();
            }
        });

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

    /// Record one element whose paint lifecycle actually executes this frame. Called from the
    /// type-erased Drawable lifecycle so custom Elements and canvas-like primitives are covered in
    /// addition to styled divs.
    pub(crate) fn record_debug_element_paint(&mut self, bounds: Bounds<Pixels>, cx: &mut App) {
        if bounds.is_empty() || !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        let window_id = self.handle.window_id().as_u64();
        cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
            let Some(runtime) = registry.windows.get_mut(&window_id) else {
                return;
            };
            if !runtime.options.show_element_updates || runtime.cleanup_this_frame {
                return;
            }
            if runtime.element_paint_markers.len() >= MAX_ELEMENT_PAINT_MARKERS {
                return;
            }
            if !runtime.element_update_painted_this_frame {
                runtime.element_update_painted_this_frame = true;
                runtime.overlay_generation = runtime.overlay_generation.wrapping_add(1);
            }
            runtime.element_paint_markers.push(bounds);
        });
    }

    /// Record the result of one cached-view lookup. This is intentionally a no-op unless visual
    /// element diagnostics are enabled, so normal rendering does not allocate marker storage.
    pub(crate) fn record_debug_view_cache_status(
        &mut self,
        bounds: Bounds<Pixels>,
        status: ViewCacheDebugStatus,
        cx: &mut App,
    ) {
        if bounds.is_empty() || !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        let window_id = self.handle.window_id().as_u64();
        cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
            let Some(runtime) = registry.windows.get_mut(&window_id) else {
                return;
            };
            if !runtime.options.show_element_updates || runtime.cleanup_this_frame {
                return;
            }
            if runtime.view_cache_markers.len() >= MAX_VIEW_CACHE_MARKERS {
                return;
            }
            if !runtime.element_update_painted_this_frame {
                runtime.element_update_painted_this_frame = true;
                runtime.overlay_generation = runtime.overlay_generation.wrapping_add(1);
            }
            runtime
                .view_cache_markers
                .push(ViewCacheDebugMarker { bounds, status });
        });
    }

    /// Prepares per-frame visual diagnostics and reports whether this frame must present the full
    /// window. Layout outlines and surface flashing intentionally require full presentation. The
    /// element-update overlay does not: it follows the real dirty frame so it does not turn a local
    /// update into a full-window redraw while it is being measured.
    pub(super) fn begin_debug_visualization_frame(&mut self, cx: &mut App) -> bool {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return false;
        }

        let mut requires_full_redraw = false;
        cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
            let Some(runtime) = registry.windows.get_mut(&window_id) else {
                return;
            };

            runtime.cleanup_this_frame = runtime.cleanup_pending;
            runtime.cleanup_pending = false;
            runtime.element_update_painted_this_frame = false;
            runtime.element_paint_markers.clear();
            runtime.view_cache_markers.clear();
            runtime.flash_this_frame = runtime.options.flash_surface_updates
                && !runtime.cleanup_this_frame;

            if runtime.flash_this_frame {
                runtime.surface_flash_generation = runtime.surface_flash_generation.wrapping_add(1);
                runtime.overlay_generation = runtime.overlay_generation.wrapping_add(1);
            }
            if runtime.options.show_element_updates && !runtime.cleanup_this_frame {
                runtime.element_update_generation =
                    runtime.element_update_generation.wrapping_add(1);
            }

            requires_full_redraw = runtime.options.show_layout_bounds
                || runtime.flash_this_frame
                || runtime.cleanup_this_frame;
        });
        requires_full_redraw
    }

    /// Paints window-level debug overlays above the completed tree. Cache markers are painted last
    /// so a green/yellow/purple cached-view outline remains visible over red child repaint outlines.
    pub(super) fn paint_debug_surface_update_flash(&mut self, cx: &App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        let (flash_this_frame, surface_flash_generation, element_generation, elements, caches) = {
            let registry = cx.global::<WindowDebugVisualizationRegistry>();
            let Some(runtime) = registry.windows.get(&window_id) else {
                return;
            };
            (
                runtime.flash_this_frame,
                runtime.surface_flash_generation,
                runtime.element_update_generation,
                runtime.element_paint_markers.clone(),
                runtime.view_cache_markers.clone(),
            )
        };

        if flash_this_frame {
            // Alternate the tint so continuously updating surfaces still visibly pulse instead of
            // settling into one permanent translucent overlay.
            let (hex, alpha) = if surface_flash_generation & 1 == 0 {
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

        let (element_hex, element_alpha) = if element_generation & 1 == 0 {
            (0xff3b30, 0.92)
        } else {
            (0xff9f0a, 0.92)
        };
        for bounds in elements {
            paint_outline(self, bounds, element_hex, element_alpha);
        }

        for marker in caches {
            let (hex, alpha) = cache_marker_color(marker.status);
            let edges = Edges {
                top: px(2.0),
                right: px(2.0),
                bottom: px(2.0),
                left: px(2.0),
            };
            paint_outline(self, expand_bounds(marker.bounds, &edges), hex, alpha);
        }
    }

    /// Schedules one cleanup frame after the newest debug overlay. Cleanup frames deliberately do
    /// not call [`Window::refresh`]: doing so would set `force_view_cache_refresh` and make this
    /// diagnostic manufacture the cache misses it is intended to reveal.
    pub(super) fn finish_debug_visualization_frame(&mut self, cx: &mut App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        let (flash_this_frame, element_update_painted_this_frame, generation) = {
            let registry = cx.global::<WindowDebugVisualizationRegistry>();
            let Some(runtime) = registry.windows.get(&window_id) else {
                return;
            };
            (
                runtime.flash_this_frame,
                runtime.element_update_painted_this_frame,
                runtime.overlay_generation,
            )
        };
        if !flash_this_frame && !element_update_painted_this_frame {
            return;
        }

        let hold = if element_update_painted_this_frame {
            ELEMENT_UPDATE_HOLD.max(SURFACE_FLASH_HOLD)
        } else {
            SURFACE_FLASH_HOLD
        };
        let handle = self.handle;
        cx.spawn(async move |cx| {
            Timer::after(hold).await;
            let _ = cx.update(|cx| {
                if !cx.has_global::<WindowDebugVisualizationRegistry>() {
                    return;
                }

                let mut should_schedule = false;
                cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
                    let Some(runtime) = registry.windows.get_mut(&window_id) else {
                        return;
                    };
                    if runtime.overlay_generation == generation
                        && (runtime.options.flash_surface_updates
                            || runtime.options.show_element_updates)
                    {
                        runtime.cleanup_pending = true;
                        should_schedule = true;
                    }
                });

                if should_schedule {
                    let _ = ignore_window_not_found(handle.update(cx, |_root, window, _cx| {
                        // Schedule a diagnostic cleanup frame without invalidating retained view
                        // caches. `Window::refresh()` would force every cached AnyView to miss.
                        window.invalidator.set_dirty(true);
                        window.schedule_dirty_frame();
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

fn cache_marker_color(status: ViewCacheDebugStatus) -> (u32, f32) {
    match status {
        ViewCacheDebugStatus::Hit => (0x30d158, 0.98),
        ViewCacheDebugStatus::DeferredDirtyReuse => (0x64d2ff, 0.98),
        ViewCacheDebugStatus::MissBounds => (0xffcc00, 0.98),
        ViewCacheDebugStatus::MissRefresh
        | ViewCacheDebugStatus::MissDirty
        | ViewCacheDebugStatus::ReuseFailed => (0xff453a, 0.98),
        ViewCacheDebugStatus::MissCold
        | ViewCacheDebugStatus::MissContentMask
        | ViewCacheDebugStatus::MissTextStyle
        | ViewCacheDebugStatus::MissFingerprint
        | ViewCacheDebugStatus::MissPrepaintRange
        | ViewCacheDebugStatus::MissPaintRange => (0xbf5af2, 0.98),
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

    #[test]
    fn cache_status_palette_distinguishes_hit_bounds_and_dirty_miss() {
        assert_ne!(
            cache_marker_color(ViewCacheDebugStatus::Hit).0,
            cache_marker_color(ViewCacheDebugStatus::MissBounds).0
        );
        assert_ne!(
            cache_marker_color(ViewCacheDebugStatus::Hit).0,
            cache_marker_color(ViewCacheDebugStatus::MissDirty).0
        );
    }
}
