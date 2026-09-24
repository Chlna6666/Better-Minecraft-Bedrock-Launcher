use super::*;
use crate::{
    AbsoluteLength, ContentMask, Length, Quad, ScaledPixels, Scene, Timer, point, rgb, rgba, size,
};
use std::collections::VecDeque;

const SURFACE_FLASH_HOLD: Duration = Duration::from_millis(90);
const ELEMENT_UPDATE_HOLD: Duration = Duration::from_millis(120);
const MAX_ELEMENT_PAINT_MARKERS: usize = 2048;
const MAX_VIEW_CACHE_MARKERS: usize = 512;
const MAX_FRAME_TIME_SAMPLES: usize = 240;
const FRAME_OVERLAY_GLYPH_WIDTH: usize = 5;
const FRAME_OVERLAY_GLYPH_HEIGHT: usize = 7;
const FRAME_OVERLAY_CELL: f32 = 2.0;
const FRAME_OVERLAY_CHAR_ADVANCE: f32 = 6.0;
const FRAME_OVERLAY_LINE_ADVANCE: f32 = 9.0;
const FRAME_OVERLAY_PADDING: f32 = 2.0;
const FRAME_OVERLAY_MARGIN: f32 = 4.0;

/// Lightweight frame-time overlay drawn directly into the Scene.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DebugFrameOverlayMode {
    /// Do not collect samples or paint an overlay.
    #[default]
    Hidden,
    /// Show only the most recent CPU frame-build duration.
    Minimal,
    /// Show current, P95, P99, max, and sample count.
    Full,
}

impl DebugFrameOverlayMode {
    /// Advances Hidden -> Minimal -> Full -> Hidden.
    pub fn next(self) -> Self {
        match self {
            Self::Hidden => Self::Minimal,
            Self::Minimal => Self::Full,
            Self::Full => Self::Hidden,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct DebugFrameTimeOverlay {
    samples: VecDeque<Duration>,
    total_frames: u64,
}

impl DebugFrameTimeOverlay {
    fn record(&mut self, duration: Duration) {
        self.total_frames = self.total_frames.saturating_add(1);
        if self.samples.len() >= MAX_FRAME_TIME_SAMPLES {
            self.samples.pop_front();
        }
        self.samples.push_back(duration);
    }

    fn reset(&mut self) {
        self.samples.clear();
    }

    fn lines(&self, mode: DebugFrameOverlayMode) -> Vec<String> {
        let current = self.samples.back().copied();
        if mode == DebugFrameOverlayMode::Hidden {
            return Vec::new();
        }
        if mode == DebugFrameOverlayMode::Minimal {
            return vec![format_frame_ms(current)];
        }

        let mut sorted = self.samples.iter().copied().collect::<Vec<_>>();
        sorted.sort_unstable();
        let percentile = |percent: usize| {
            if sorted.is_empty() {
                None
            } else {
                let index = ((sorted.len() - 1) * percent) / 100;
                sorted.get(index).copied()
            }
        };
        let frame_count = self.total_frames.min(99_999);
        vec![
            format!("CUR {}", format_frame_ms(current)),
            format!("P95 {}", format_frame_ms(percentile(95))),
            format!("P99 {}", format_frame_ms(percentile(99))),
            format!("MAX {}", format_frame_ms(sorted.last().copied())),
            format!("FRAMES {frame_count:>5}"),
        ]
    }
}

/// Why a retained render boundary was reused or rebuilt in the current frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewCacheDebugStatus {
    Hit,
    /// A parent lifecycle was traversed to reach a dirty child, but the parent contributes no own
    /// scene primitives and therefore did not repaint anything itself.
    TraversalOnly,
    /// A parent lifecycle was traversed to reach a dirty child, while the parent's own scene
    /// primitives were replayed unchanged.
    SelfSceneReplay,
    DeferredDirtyReuse,
    MissCold,
    MissBounds,
    MissContentMask,
    MissTextStyle,
    MissFingerprint,
    MissRefresh,
    MissDirty,
    /// The view was rebuilt to reach a dirty descendant, not for its own state.
    MissTraversalAncestor,
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
    /// Paint frame-build timing directly into the Scene without creating layout/text nodes.
    pub frame_time_overlay: DebugFrameOverlayMode,
    /// Flash the whole window whenever GPUI produces a new painted surface frame.
    pub flash_surface_updates: bool,
    /// Draw the box model and clipping boundary for styled elements.
    pub show_layout_bounds: bool,
    /// Outline elements whose own paint work executes in the current frame and show retained
    /// cache/repaint-boundary markers.
    ///
    /// A subtree restored through retained paint replay is not traversed, so its descendants do
    /// not get repaint markers. Structural ancestors that execute only to reach a dirty descendant
    /// are reclassified from provisional red/orange: cyan means traversal with no own scene work,
    /// while green means the parent's own scene primitives were replayed from the previous frame.
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
    frame_time: DebugFrameTimeOverlay,
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

    /// Sets the direct Scene frame-time overlay mode.
    pub fn set_debug_frame_overlay_mode(
        &mut self,
        mode: DebugFrameOverlayMode,
        cx: &mut App,
    ) {
        let mut options = self.debug_visualization(cx);
        options.frame_time_overlay = mode;
        self.set_debug_visualization(options, cx);
    }

    /// Cycles the frame-time overlay through hidden, minimal, and full modes.
    pub fn cycle_debug_frame_overlay_mode(&mut self, cx: &mut App) {
        self.set_debug_frame_overlay_mode(self.debug_visualization(cx).frame_time_overlay.next(), cx);
    }

    /// Clears the bounded frame-time sample window while keeping the overlay enabled.
    pub fn reset_debug_frame_overlay_stats(&mut self, cx: &mut App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
            if let Some(runtime) = registry.windows.get_mut(&window_id) {
                runtime.frame_time.reset();
            }
        });
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

    fn reclassify_debug_element_paint(
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

            if runtime
                .element_paint_markers
                .last()
                .is_some_and(|last| *last == bounds)
            {
                runtime.element_paint_markers.pop();
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

    /// Reclassify the current element as structural traversal with no own scene primitive work.
    /// This must be called before painting children while the provisional marker is still last.
    pub(crate) fn record_debug_element_traversal_only(
        &mut self,
        bounds: Bounds<Pixels>,
        cx: &mut App,
    ) {
        self.reclassify_debug_element_paint(bounds, ViewCacheDebugStatus::TraversalOnly, cx);
    }

    /// Reclassify the current element from a full own-paint marker into an ancestor traversal whose
    /// own scene primitives were retained. This must be called before painting any child so the
    /// last provisional marker still belongs to this parent.
    pub(crate) fn record_debug_element_self_scene_replay(
        &mut self,
        bounds: Bounds<Pixels>,
        cx: &mut App,
    ) {
        self.reclassify_debug_element_paint(bounds, ViewCacheDebugStatus::SelfSceneReplay, cx);
    }

    /// Record the result of one retained-view/boundary lookup. This is intentionally a no-op unless
    /// visual element diagnostics are enabled, so normal rendering does not allocate marker storage.
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

    /// Records one completed CPU frame build for the direct Scene overlay.
    pub(super) fn record_debug_frame_time(&mut self, duration: Duration, cx: &mut App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
            let Some(runtime) = registry.windows.get_mut(&window_id) else {
                return;
            };
            if runtime.options.frame_time_overlay != DebugFrameOverlayMode::Hidden {
                runtime.frame_time.record(duration);
            }
        });
    }

    /// Paints timing text directly into the Scene. This deliberately bypasses GPUI text/layout and
    /// does not request another frame, so the diagnostic cannot perturb invalidation cadence.
    pub(super) fn paint_debug_frame_time_overlay(&mut self, cx: &App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }

        let (mode, lines) = {
            let registry = cx.global::<WindowDebugVisualizationRegistry>();
            let Some(runtime) = registry.windows.get(&window_id) else {
                return;
            };
            let mode = runtime.options.frame_time_overlay;
            if mode == DebugFrameOverlayMode::Hidden {
                return;
            }
            (mode, runtime.frame_time.lines(mode))
        };
        debug_frame_overlay_paint(
            &mut self.next_frame.scene,
            self.viewport_size,
            self.scale_factor,
            mode,
            &lines,
        );
    }

    /// Paints window-level debug overlays above the completed tree. Cache markers are painted last
    /// so cached/traversal outlines remain visible over red child repaint outlines.
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

    /// Removes diagnostic pixels after the final stable debug frame without turning every animation
    /// sample into a second lifecycle frame.
    ///
    /// A generation check coalesces continuous animation/hover activity: only the last marker frame
    /// survives long enough to arm cleanup. Deferred overlays are deliberately not replayed solely
    /// for diagnostics; if one is visible, cleanup remains pending and the next real application
    /// frame erases the markers. Outside diagnostic mode this path is completely inactive.
    pub(super) fn finish_debug_visualization_frame(&mut self, cx: &mut App) {
        let window_id = self.handle.window_id().as_u64();
        if !cx.has_global::<WindowDebugVisualizationRegistry>() {
            return;
        }
        let (flash_this_frame, element_updates_this_frame, generation) = {
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
        if !flash_this_frame && !element_updates_this_frame {
            return;
        }

        let hold = if flash_this_frame {
            SURFACE_FLASH_HOLD.max(ELEMENT_UPDATE_HOLD)
        } else {
            ELEMENT_UPDATE_HOLD
        };
        let handle = self.handle;
        cx.spawn(async move |cx| {
            Timer::after(hold).await;
            let _ = cx.update(|cx| {
                if !cx.has_global::<WindowDebugVisualizationRegistry>() {
                    return;
                }

                let mut should_cleanup = false;
                cx.update_global(|registry: &mut WindowDebugVisualizationRegistry, _cx| {
                    let Some(runtime) = registry.windows.get_mut(&window_id) else {
                        return;
                    };
                    let cleanup_still_enabled = runtime.options.flash_surface_updates
                        || runtime.options.show_element_updates;
                    if runtime.overlay_generation == generation && cleanup_still_enabled {
                        runtime.cleanup_pending = true;
                        should_cleanup = true;
                    }
                });

                if should_cleanup {
                    let _ = ignore_window_not_found(handle.update(cx, |_root, window, _cx| {
                        // Never manufacture a diagnostic lifecycle frame while deferred overlays
                        // are alive; their draw descriptors are frame-local. The pending flag will
                        // be consumed by the next genuine application frame instead.
                        if !window.rendered_frame.deferred_draws.is_empty() {
                            return;
                        }
                        // If application work is already queued, let that real frame perform the
                        // cleanup. Otherwise request one replay-only frame: unchanged retained
                        // subtrees are copied and no application entity is notified.
                        if window.invalidator.is_dirty() || window.refreshing {
                            return;
                        }
                        window.invalidator.set_replay_only_dirty();
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
        ViewCacheDebugStatus::Hit | ViewCacheDebugStatus::SelfSceneReplay => (0x30d158, 0.98),
        ViewCacheDebugStatus::TraversalOnly | ViewCacheDebugStatus::MissTraversalAncestor => {
            (0x64d2ff, 0.98)
        }
        ViewCacheDebugStatus::DeferredDirtyReuse => (0x5ac8fa, 0.98),
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
    fn frame_overlay_has_glyphs_for_every_rendered_character() {
        let mut overlay = DebugFrameTimeOverlay::default();
        for duration in [
            Duration::ZERO,
            Duration::from_micros(4_167),
            Duration::from_micros(16_667),
            Duration::from_millis(123),
        ] {
            overlay.record(duration);
        }
        for line in overlay.lines(DebugFrameOverlayMode::Full) {
            for character in line.chars() {
                assert!(
                    character == ' ' || debug_frame_glyph(character).is_some(),
                    "missing debug overlay glyph for {character:?} in {line:?}"
                );
            }
        }
    }

    #[test]
    fn cache_status_palette_distinguishes_hit_bounds_dirty_and_traversal() {
        assert_ne!(
            cache_marker_color(ViewCacheDebugStatus::Hit).0,
            cache_marker_color(ViewCacheDebugStatus::MissBounds).0
        );
        assert_ne!(
            cache_marker_color(ViewCacheDebugStatus::Hit).0,
            cache_marker_color(ViewCacheDebugStatus::MissDirty).0
        );
        assert_ne!(
            cache_marker_color(ViewCacheDebugStatus::TraversalOnly).0,
            cache_marker_color(ViewCacheDebugStatus::MissDirty).0
        );
        assert_eq!(
            cache_marker_color(ViewCacheDebugStatus::Hit).0,
            cache_marker_color(ViewCacheDebugStatus::SelfSceneReplay).0
        );
    }
}


fn format_frame_ms(duration: Option<Duration>) -> String {
    duration
        .map(|duration| format!("{:>5.1}MS", duration.as_secs_f64() * 1000.0))
        .unwrap_or_else(|| "   --MS".to_owned())
}

fn debug_frame_overlay_paint(
    scene: &mut Scene,
    viewport_size: Size<Pixels>,
    scale_factor: f32,
    _mode: DebugFrameOverlayMode,
    lines: &[String],
) {
    if lines.is_empty() || scale_factor <= 0.0 || !scale_factor.is_finite() {
        return;
    }

    let max_chars = lines.iter().map(|line| line.chars().count()).max().unwrap_or(0);
    let cell = (FRAME_OVERLAY_CELL * scale_factor).max(1.0);
    let width = cell
        * (max_chars as f32 * FRAME_OVERLAY_CHAR_ADVANCE + FRAME_OVERLAY_PADDING * 2.0);
    let height = cell
        * (lines.len() as f32 * FRAME_OVERLAY_LINE_ADVANCE + FRAME_OVERLAY_PADDING * 2.0);
    let viewport = viewport_size.scale(scale_factor);
    let left = (viewport.width.0 - width - cell * FRAME_OVERLAY_MARGIN).max(0.0);
    let top = cell * FRAME_OVERLAY_MARGIN;
    let mask = ContentMask::new(Bounds::new(
        point(ScaledPixels(0.0), ScaledPixels(0.0)),
        viewport,
    ));

    insert_debug_quad(
        scene,
        Bounds::new(
            point(ScaledPixels(left), ScaledPixels(top)),
            size(ScaledPixels(width), ScaledPixels(height)),
        ),
        mask.clone(),
        rgba(0x000000cc).into(),
    );

    let foreground: Hsla = rgba(0x39ff6aff).into();
    for (line_index, line) in lines.iter().enumerate() {
        let row_top = top
            + cell * (FRAME_OVERLAY_PADDING + line_index as f32 * FRAME_OVERLAY_LINE_ADVANCE);
        for (char_index, character) in line.chars().enumerate() {
            if character == ' ' {
                continue;
            }
            let Some(rows) = debug_frame_glyph(character) else {
                continue;
            };
            let glyph_left = left
                + cell * (FRAME_OVERLAY_PADDING
                    + char_index as f32 * FRAME_OVERLAY_CHAR_ADVANCE);
            for (glyph_row, bits) in rows.iter().copied().enumerate() {
                let mut column = 0usize;
                while column < FRAME_OVERLAY_GLYPH_WIDTH {
                    if bits & (1 << (FRAME_OVERLAY_GLYPH_WIDTH - 1 - column)) == 0 {
                        column += 1;
                        continue;
                    }
                    let start = column;
                    while column < FRAME_OVERLAY_GLYPH_WIDTH
                        && bits & (1 << (FRAME_OVERLAY_GLYPH_WIDTH - 1 - column)) != 0
                    {
                        column += 1;
                    }
                    insert_debug_quad(
                        scene,
                        Bounds::new(
                            point(
                                ScaledPixels(glyph_left + start as f32 * cell),
                                ScaledPixels(row_top + glyph_row as f32 * cell),
                            ),
                            size(ScaledPixels((column - start) as f32 * cell), ScaledPixels(cell)),
                        ),
                        mask.clone(),
                        foreground,
                    );
                }
            }
        }
    }
}

fn insert_debug_quad(
    scene: &mut Scene,
    bounds: Bounds<ScaledPixels>,
    content_mask: ContentMask<ScaledPixels>,
    background: Hsla,
) {
    scene.insert_primitive(Quad {
        bounds,
        content_mask,
        background: background.into(),
        ..Quad::default()
    });
}

fn debug_frame_glyph(character: char) -> Option<[u8; FRAME_OVERLAY_GLYPH_HEIGHT]> {
    Some(match character {
        '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
        '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
        '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
        '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
        '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
        '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
        '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
        '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
        '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
        '.' => [0, 0, 0, 0, 0, 0x0c, 0x0c],
        '-' => [0, 0, 0, 0x1f, 0, 0, 0],
        'A' => [0x0e, 0x11, 0x11, 0x1f, 0x11, 0x11, 0x11],
        'C' => [0x0e, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0e],
        'E' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x1f],
        'F' => [0x1f, 0x10, 0x10, 0x1e, 0x10, 0x10, 0x10],
        'M' => [0x11, 0x1b, 0x15, 0x15, 0x11, 0x11, 0x11],
        'P' => [0x1e, 0x11, 0x11, 0x1e, 0x10, 0x10, 0x10],
        'R' => [0x1e, 0x11, 0x11, 0x1e, 0x14, 0x12, 0x11],
        'S' => [0x0f, 0x10, 0x10, 0x0e, 0x01, 0x01, 0x1e],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0e],
        'X' => [0x11, 0x11, 0x0a, 0x04, 0x0a, 0x11, 0x11],
        _ => return None,
    })
}
