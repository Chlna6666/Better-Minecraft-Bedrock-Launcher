#[cfg(any(feature = "inspector", debug_assertions))]
use crate::Inspector;
use crate::{
    Action, AnimatedFrame, AnyDrag, AnyElement, AnyImageCache, AnyTooltip, AnyView, App,
    AppContext, Arena, Asset, AsyncApp, AsyncWindowContext, AvailableSpace, BackdropBlurStyle,
    Background, BorderStyle, Bounds, BoxShadow, Capslock, Context, Corners, CursorStyle,
    Decorations, DevicePixels, DirtyRegion, DispatchActionListener, DispatchNodeId, DispatchTree,
    DisplayId, Edges, Effect, Entity, EntityId, EventEmitter, FileDropEvent, FontId,
    FrameRenderPlan, Global, GlobalElementId, GlyphId, GpuMesh3d, GpuMesh3dCamera, GpuSpecs,
    GpuiMemoryPolicy, GpuiMemoryTrimLevel, Hsla, ImagePipelineConfig, InputHandler, IsZero,
    KeyBinding, KeyContext, KeyDownEvent, KeyEvent, Keystroke, KeystrokeEvent, LayoutId,
    LineLayoutIndex, Modifiers, ModifiersChangedEvent, MonochromeSprite, MonochromeSpriteSampling,
    MouseButton, MouseDownEvent, MouseEvent, MouseMoveEvent, MouseUpEvent, PartialPresentMode,
    Path, Pixels, PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler,
    PlatformWindow, Point, PolychromeSprite, Priority, PromptButton, PromptLevel, Quad, Render,
    RenderGlyphParams, RenderImage, RenderImageParams, RenderImagePixelFormat, RenderSvgParams,
    Replay, RequestFrameOptions, ResizeEdge, RetainedResourceTrimPolicy, SMOOTH_SVG_SCALE_FACTOR,
    SUBPIXEL_VARIANTS_Y, ScaledPixels, Scene, Shadow, SharedString, Size, StrikethroughStyle,
    Style, SubpixelSprite, SubscriberSet, Subscription, SystemWindowTab, SystemWindowTabController,
    TabStopMap, TaffyLayoutEngine, Task, TextRenderingMode, TextStyle, TextStyleRefinement,
    TransformationMatrix, Underline, UnderlineStyle, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowControls, WindowDecorations, WindowOptions, WindowParams, WindowTextSystem,
    point, prelude::*, px, record_coalesced_refresh, record_dirty_region_metrics,
    record_draw_budget_miss, record_draw_degrade, record_frame_decision,
    record_frame_retained_capacity, record_image_drop, record_inactive_dirty_defer,
    record_inactive_present_skip, record_layout_cache_metrics, record_layout_frame_metrics,
    record_retained_frame_skip, record_scene_frame_metrics, record_skipped_pointer_frame,
    record_window_frame_result, record_window_layout_recompute, rems, size, transparent_black,
};
use anyhow::{Context as _, Result, anyhow};
use collections::{FxHashMap, FxHashSet};
#[cfg(target_os = "macos")]
use core_video::pixel_buffer::CVPixelBuffer;
use derive_more::{Deref, DerefMut};
use futures::FutureExt;
use futures::channel::oneshot;
use itertools::FoldWhile::{Continue, Done};
use itertools::Itertools;
use parking_lot::RwLock;
use raw_window_handle::{HandleError, HasDisplayHandle, HasWindowHandle};
use refineable::Refineable;
use slotmap::SlotMap;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    borrow::Cow,
    cell::{Cell, RefCell},
    cmp,
    fmt::{Debug, Display},
    hash::{Hash, Hasher},
    marker::PhantomData,
    mem,
    ops::{DerefMut, Range},
    rc::Rc,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering::SeqCst},
    },
    time::{Duration, Instant},
};
use util::post_inc;
use util::{ResultExt, measure};
use uuid::Uuid;

mod prompts;

use crate::util::atomic_incr_if_not_zero;
pub use prompts::*;

pub(crate) const DEFAULT_WINDOW_SIZE: Size<Pixels> = size(px(1536.), px(864.));
pub(crate) const SLOW_FRAME_REQUEST: Duration = Duration::from_millis(16);
pub(crate) const TARGET_FRAME_GENERATION_BUDGET: Duration = Duration::from_millis(4);
const BACKGROUND_PROGRESSIVE_FRAME_RETRY: Duration = Duration::from_millis(250);
const MINIMIZED_PROGRESSIVE_FRAME_RETRY: Duration = Duration::from_secs(1);
const SLOW_INPUT_DISPATCH: Duration = Duration::from_millis(16);
const FRAME_REQUEST_WATCHDOG_TIMEOUT: Duration = Duration::from_millis(100);
const STALLED_WINDOW_FRAME_RETRY: Duration = Duration::from_millis(33);
const RECENT_INPUT_DIRTY_FRAME_GRACE: Duration = Duration::from_millis(500);
const HIGH_REFRESH_FRAME_BUDGET_HEADROOM: f32 = 0.85;
const HIGH_REFRESH_FRAME_INTERVAL: Duration = Duration::from_millis(8);
const MIN_DYNAMIC_FRAME_BUDGET: Duration = Duration::from_millis(2);

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

fn glyph_device_origin(
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

fn svg_paint_bounds_for_requested_bounds(bounds: Bounds<ScaledPixels>) -> Bounds<ScaledPixels> {
    bounds
        .map_origin(|origin| origin.round())
        .map_size(|size| size.ceil())
}

fn svg_raster_size_for_paint_bounds(bounds: Bounds<ScaledPixels>) -> Size<DevicePixels> {
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

fn resize_edge_hit_test(
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

fn resize_edge_cursor_style(edge: ResizeEdge) -> CursorStyle {
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AnimatedImageSlotKey {
    image_id: crate::ImageId,
    frame_slot: usize,
}

fn ignore_window_not_found<T>(result: Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) if error.to_string() == "window not found" => None,
        Err(error) => Err::<T, _>(error).log_err(),
    }
}

fn platform_input_name(event: &PlatformInput) -> &'static str {
    match event {
        PlatformInput::KeyDown(_) => "key_down",
        PlatformInput::KeyUp(_) => "key_up",
        PlatformInput::ModifiersChanged(_) => "modifiers_changed",
        PlatformInput::MouseDown(_) => "mouse_down",
        PlatformInput::MouseUp(_) => "mouse_up",
        PlatformInput::MouseMove(event) if event.pressed_button.is_some() => "mouse_drag",
        PlatformInput::MouseMove(_) => "mouse_move",
        PlatformInput::MouseExited(_) => "mouse_exited",
        PlatformInput::ScrollWheel(_) => "scroll_wheel",
        PlatformInput::FileDrop(_) => "file_drop",
    }
}

fn log_timed_gpui_event(
    message: &'static str,
    elapsed: Duration,
    log_fields: impl FnOnce() -> String,
) {
    if elapsed >= SLOW_INPUT_DISPATCH {
        if log::log_enabled!(log::Level::Warn) {
            log::warn!("{} elapsed={:?} {}", message, elapsed, log_fields());
        }
    } else if log::log_enabled!(log::Level::Trace) {
        log::trace!("{} elapsed={:?} {}", message, elapsed, log_fields());
    }
}

/// Represents the two different phases when dispatching events.
#[derive(Default, Copy, Clone, Debug, Eq, PartialEq)]
pub enum DispatchPhase {
    /// After the capture phase comes the bubble phase, in which mouse event listeners are
    /// invoked front to back and keyboard event listeners are invoked from the focused element
    /// to the root of the element tree. This is the phase you'll most commonly want to use when
    /// registering event listeners.
    #[default]
    Bubble,
    /// During the initial capture phase, mouse event listeners are invoked back to front, and keyboard
    /// listeners are invoked from the root of the tree downward toward the focused element. This phase
    /// is used for special purposes such as clearing the "pressed" state for click events. If
    /// you stop event propagation during this phase, you need to know what you're doing. Handlers
    /// outside of the immediate region may rely on detecting non-local events during this phase.
    Capture,
}

impl DispatchPhase {
    /// Returns true if this represents the "bubble" phase.
    #[inline]
    pub fn bubble(self) -> bool {
        self == DispatchPhase::Bubble
    }

    /// Returns true if this represents the "capture" phase.
    #[inline]
    pub fn capture(self) -> bool {
        self == DispatchPhase::Capture
    }
}

struct WindowInvalidatorInner {
    pub dirty: bool,
    pub draw_phase: DrawPhase,
    pub dirty_views: FxHashSet<EntityId>,
}

#[derive(Clone)]
pub(crate) struct WindowInvalidator {
    inner: Rc<RefCell<WindowInvalidatorInner>>,
}

impl WindowInvalidator {
    pub fn new() -> Self {
        WindowInvalidator {
            inner: Rc::new(RefCell::new(WindowInvalidatorInner {
                dirty: true,
                draw_phase: DrawPhase::None,
                dirty_views: FxHashSet::default(),
            })),
        }
    }

    pub fn invalidate_view(&self, entity: EntityId, cx: &mut App) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.dirty_views.insert(entity);
        if inner.draw_phase == DrawPhase::None {
            inner.dirty = true;
            cx.push_effect(Effect::Notify { emitter: entity });
            true
        } else {
            false
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    pub fn set_dirty(&self, dirty: bool) {
        self.inner.borrow_mut().dirty = dirty
    }

    pub fn set_phase(&self, phase: DrawPhase) {
        self.inner.borrow_mut().draw_phase = phase
    }

    pub fn phase(&self) -> DrawPhase {
        self.inner.borrow().draw_phase
    }

    pub fn take_views(&self) -> FxHashSet<EntityId> {
        mem::take(&mut self.inner.borrow_mut().dirty_views)
    }

    pub fn replace_views(&self, views: FxHashSet<EntityId>) {
        self.inner.borrow_mut().dirty_views = views;
    }

    pub fn not_drawing(&self) -> bool {
        self.inner.borrow().draw_phase == DrawPhase::None
    }

    #[track_caller]
    pub fn debug_assert_paint(&self) {
        debug_assert!(
            matches!(self.inner.borrow().draw_phase, DrawPhase::Paint),
            "this method can only be called during paint"
        );
    }

    #[track_caller]
    pub fn debug_assert_prepaint(&self) {
        debug_assert!(
            matches!(self.inner.borrow().draw_phase, DrawPhase::Prepaint),
            "this method can only be called during request_layout, or prepaint"
        );
    }

    #[track_caller]
    pub fn debug_assert_paint_or_prepaint(&self) {
        debug_assert!(
            matches!(
                self.inner.borrow().draw_phase,
                DrawPhase::Paint | DrawPhase::Prepaint
            ),
            "this method can only be called during request_layout, prepaint, or paint"
        );
    }
}

type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;

pub(crate) type AnyWindowFocusListener =
    Box<dyn FnMut(&WindowFocusEvent, &mut Window, &mut App) -> bool + 'static>;

pub(crate) struct WindowFocusEvent {
    pub(crate) previous_focus_path: SmallVec<[FocusId; 8]>,
    pub(crate) current_focus_path: SmallVec<[FocusId; 8]>,
}

impl WindowFocusEvent {
    pub fn is_focus_in(&self, focus_id: FocusId) -> bool {
        !self.previous_focus_path.contains(&focus_id) && self.current_focus_path.contains(&focus_id)
    }

    pub fn is_focus_out(&self, focus_id: FocusId) -> bool {
        self.previous_focus_path.contains(&focus_id) && !self.current_focus_path.contains(&focus_id)
    }
}

/// This is provided when subscribing for `Context::on_focus_out` events.
pub struct FocusOutEvent {
    /// A weak focus handle representing what was blurred.
    pub blurred: WeakFocusHandle,
}

slotmap::new_key_type! {
    /// A globally unique identifier for a focusable element.
    pub struct FocusId;
}

thread_local! {
    pub(crate) static ELEMENT_ARENA: RefCell<Arena> = RefCell::new(Arena::new(64 * 1024));
}

/// Returned when the element arena has been used and so must be cleared before the next draw.
#[must_use]
pub struct ArenaClearNeeded;

impl ArenaClearNeeded {
    /// Clear the element arena.
    pub fn clear(self) {
        ELEMENT_ARENA.with_borrow_mut(|element_arena| {
            element_arena.clear();
        });
    }
}

impl Drop for ArenaClearNeeded {
    fn drop(&mut self) {
        ELEMENT_ARENA.with_borrow_mut(|element_arena| {
            element_arena.clear();
        });
    }
}

pub(crate) fn trim_element_arena(max_capacity: usize) -> bool {
    ELEMENT_ARENA.with_borrow_mut(|element_arena| element_arena.trim_to_max_capacity(max_capacity))
}

pub(crate) type FocusMap = RwLock<SlotMap<FocusId, FocusRef>>;
pub(crate) struct FocusRef {
    pub(crate) ref_count: AtomicUsize,
    pub(crate) tab_index: isize,
    pub(crate) tab_stop: bool,
}

impl FocusId {
    /// Obtains whether the element associated with this handle is currently focused.
    pub fn is_focused(&self, window: &Window) -> bool {
        window.focus == Some(*self)
    }

    /// Obtains whether the element associated with this handle contains the focused
    /// element or is itself focused.
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        window
            .focused(cx)
            .is_some_and(|focused| self.contains(focused.id, window))
    }

    /// Obtains whether the element associated with this handle is contained within the
    /// focused element or is itself focused.
    pub fn within_focused(&self, window: &Window, cx: &App) -> bool {
        let focused = window.focused(cx);
        focused.is_some_and(|focused| focused.id.contains(*self, window))
    }

    /// Obtains whether this handle contains the given handle in the most recently rendered frame.
    pub(crate) fn contains(&self, other: Self, window: &Window) -> bool {
        window
            .rendered_frame
            .dispatch_tree
            .focus_contains(*self, other)
    }
}

/// A handle which can be used to track and manipulate the focused element in a window.
pub struct FocusHandle {
    pub(crate) id: FocusId,
    handles: Arc<FocusMap>,
    /// The index of this element in the tab order.
    pub tab_index: isize,
    /// Whether this element can be focused by tab navigation.
    pub tab_stop: bool,
}

impl std::fmt::Debug for FocusHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!("FocusHandle({:?})", self.id))
    }
}

impl FocusHandle {
    pub(crate) fn new(handles: &Arc<FocusMap>) -> Self {
        let id = handles.write().insert(FocusRef {
            ref_count: AtomicUsize::new(1),
            tab_index: 0,
            tab_stop: false,
        });

        Self {
            id,
            tab_index: 0,
            tab_stop: false,
            handles: handles.clone(),
        }
    }

    pub(crate) fn for_id(id: FocusId, handles: &Arc<FocusMap>) -> Option<Self> {
        let lock = handles.read();
        let focus = lock.get(id)?;
        if atomic_incr_if_not_zero(&focus.ref_count) == 0 {
            return None;
        }
        Some(Self {
            id,
            tab_index: focus.tab_index,
            tab_stop: focus.tab_stop,
            handles: handles.clone(),
        })
    }

    /// Sets the tab index of the element associated with this handle.
    pub fn tab_index(mut self, index: isize) -> Self {
        self.tab_index = index;
        if let Some(focus) = self.handles.write().get_mut(self.id) {
            focus.tab_index = index;
        }
        self
    }

    /// Sets whether the element associated with this handle is a tab stop.
    ///
    /// When `false`, the element will not be included in the tab order.
    pub fn tab_stop(mut self, tab_stop: bool) -> Self {
        self.tab_stop = tab_stop;
        if let Some(focus) = self.handles.write().get_mut(self.id) {
            focus.tab_stop = tab_stop;
        }
        self
    }

    /// Converts this focus handle into a weak variant, which does not prevent it from being released.
    pub fn downgrade(&self) -> WeakFocusHandle {
        WeakFocusHandle {
            id: self.id,
            handles: Arc::downgrade(&self.handles),
        }
    }

    /// Moves the focus to the element associated with this handle.
    pub fn focus(&self, window: &mut Window) {
        window.focus(self)
    }

    /// Obtains whether the element associated with this handle is currently focused.
    pub fn is_focused(&self, window: &Window) -> bool {
        self.id.is_focused(window)
    }

    /// Obtains whether the element associated with this handle contains the focused
    /// element or is itself focused.
    pub fn contains_focused(&self, window: &Window, cx: &App) -> bool {
        self.id.contains_focused(window, cx)
    }

    /// Obtains whether the element associated with this handle is contained within the
    /// focused element or is itself focused.
    pub fn within_focused(&self, window: &Window, cx: &mut App) -> bool {
        self.id.within_focused(window, cx)
    }

    /// Obtains whether this handle contains the given handle in the most recently rendered frame.
    pub fn contains(&self, other: &Self, window: &Window) -> bool {
        self.id.contains(other.id, window)
    }

    /// Dispatch an action on the element that rendered this focus handle
    pub fn dispatch_action(&self, action: &dyn Action, window: &mut Window, cx: &mut App) {
        if let Some(node_id) = window
            .rendered_frame
            .dispatch_tree
            .focusable_node_id(self.id)
        {
            window.dispatch_action_on_node(node_id, action, cx)
        }
    }
}

impl Clone for FocusHandle {
    fn clone(&self) -> Self {
        Self::for_id(self.id, &self.handles).unwrap()
    }
}

impl PartialEq for FocusHandle {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for FocusHandle {}

impl Drop for FocusHandle {
    fn drop(&mut self) {
        self.handles
            .read()
            .get(self.id)
            .unwrap()
            .ref_count
            .fetch_sub(1, SeqCst);
    }
}

/// A weak reference to a focus handle.
#[derive(Clone, Debug)]
pub struct WeakFocusHandle {
    pub(crate) id: FocusId,
    pub(crate) handles: Weak<FocusMap>,
}

impl WeakFocusHandle {
    /// Attempts to upgrade the [WeakFocusHandle] to a [FocusHandle].
    pub fn upgrade(&self) -> Option<FocusHandle> {
        let handles = self.handles.upgrade()?;
        FocusHandle::for_id(self.id, &handles)
    }
}

impl PartialEq for WeakFocusHandle {
    fn eq(&self, other: &WeakFocusHandle) -> bool {
        self.id == other.id
    }
}

impl Eq for WeakFocusHandle {}

impl PartialEq<FocusHandle> for WeakFocusHandle {
    fn eq(&self, other: &FocusHandle) -> bool {
        self.id == other.id
    }
}

impl PartialEq<WeakFocusHandle> for FocusHandle {
    fn eq(&self, other: &WeakFocusHandle) -> bool {
        self.id == other.id
    }
}

/// Focusable allows users of your view to easily
/// focus it (using window.focus_view(cx, view))
pub trait Focusable: 'static {
    /// Returns the focus handle associated with this view.
    fn focus_handle(&self, cx: &App) -> FocusHandle;
}

impl<V: Focusable> Focusable for Entity<V> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }
}

/// ManagedView is a view (like a Modal, Popover, Menu, etc.)
/// where the lifecycle of the view is handled by another view.
pub trait ManagedView: Focusable + EventEmitter<DismissEvent> + Render {}

impl<M: Focusable + EventEmitter<DismissEvent> + Render> ManagedView for M {}

/// Emitted by implementers of [`ManagedView`] to indicate the view should be dismissed, such as when a view is presented as a modal.
pub struct DismissEvent;

type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;

pub(crate) type AnyMouseListener =
    Box<dyn FnMut(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static>;

pub(crate) struct MouseListener {
    event_type: TypeId,
    listener: Option<AnyMouseListener>,
}

impl MouseListener {
    pub(crate) fn new<Event: MouseEvent>(listener: AnyMouseListener) -> Self {
        Self {
            event_type: TypeId::of::<Event>(),
            listener: Some(listener),
        }
    }

    fn handles(&self, event_type: TypeId) -> bool {
        self.event_type == event_type
    }

    fn listener_mut(&mut self) -> Option<&mut AnyMouseListener> {
        self.listener.as_mut()
    }

    fn take(&mut self) -> Self {
        Self {
            event_type: self.event_type,
            listener: self.listener.take(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct CursorStyleRequest {
    pub(crate) hitbox_id: Option<HitboxId>,
    pub(crate) style: CursorStyle,
}

#[derive(Default, Eq, PartialEq)]
pub(crate) struct HitTest {
    pub(crate) ids: SmallVec<[HitboxId; 8]>,
    pub(crate) hover_hitbox_count: usize,
}

/// A type of window control area that corresponds to the platform window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowControlArea {
    /// An area that allows dragging of the platform window.
    Drag,
    /// An area that allows closing of the platform window.
    Close,
    /// An area that allows maximizing of the platform window.
    Max,
    /// An area that allows minimizing of the platform window.
    Min,
}

/// An identifier for a [Hitbox] which also includes [HitboxBehavior].
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub struct HitboxId(u64);

impl HitboxId {
    /// Checks if the hitbox with this ID is currently hovered. Except when handling
    /// `ScrollWheelEvent`, this is typically what you want when determining whether to handle mouse
    /// events or paint hover styles.
    ///
    /// See [`Hitbox::is_hovered`] for details.
    pub fn is_hovered(self, window: &Window) -> bool {
        let hit_test = &window.mouse_hit_test;
        for id in hit_test.ids.iter().take(hit_test.hover_hitbox_count) {
            if self == *id {
                return true;
            }
        }
        false
    }

    /// Checks if the hitbox with this ID contains the mouse and should handle scroll events.
    /// Typically this should only be used when handling `ScrollWheelEvent`, and otherwise
    /// `is_hovered` should be used. See the documentation of `Hitbox::is_hovered` for details about
    /// this distinction.
    pub fn should_handle_scroll(self, window: &Window) -> bool {
        window.mouse_hit_test.ids.contains(&self)
    }

    fn next(mut self) -> HitboxId {
        HitboxId(self.0.wrapping_add(1))
    }
}

/// A rectangular region that potentially blocks hitboxes inserted prior.
/// See [Window::insert_hitbox] for more details.
#[derive(Clone, Debug, Deref)]
pub struct Hitbox {
    /// A unique identifier for the hitbox.
    pub id: HitboxId,
    /// The bounds of the hitbox.
    #[deref]
    pub bounds: Bounds<Pixels>,
    /// The content mask when the hitbox was inserted.
    pub content_mask: ContentMask<Pixels>,
    /// Flags that specify hitbox behavior.
    pub behavior: HitboxBehavior,
}

impl Hitbox {
    /// Checks if the hitbox is currently hovered. Except when handling `ScrollWheelEvent`, this is
    /// typically what you want when determining whether to handle mouse events or paint hover
    /// styles.
    ///
    /// This can return `false` even when the hitbox contains the mouse, if a hitbox in front of
    /// this sets `HitboxBehavior::BlockMouse` (`InteractiveElement::occlude`) or
    /// `HitboxBehavior::BlockMouseExceptScroll` (`InteractiveElement::block_mouse_except_scroll`).
    ///
    /// Handling of `ScrollWheelEvent` should typically use `should_handle_scroll` instead.
    /// Concretely, this is due to use-cases like overlays that cause the elements under to be
    /// non-interactive while still allowing scrolling. More abstractly, this is because
    /// `is_hovered` is about element interactions directly under the mouse - mouse moves, clicks,
    /// hover styling, etc. In contrast, scrolling is about finding the current outer scrollable
    /// container.
    pub fn is_hovered(&self, window: &Window) -> bool {
        self.id.is_hovered(window)
    }

    /// Checks if the hitbox contains the mouse and should handle scroll events. Typically this
    /// should only be used when handling `ScrollWheelEvent`, and otherwise `is_hovered` should be
    /// used. See the documentation of `Hitbox::is_hovered` for details about this distinction.
    ///
    /// This can return `false` even when the hitbox contains the mouse, if a hitbox in front of
    /// this sets `HitboxBehavior::BlockMouse` (`InteractiveElement::occlude`).
    pub fn should_handle_scroll(&self, window: &Window) -> bool {
        self.id.should_handle_scroll(window)
    }
}

/// How the hitbox affects mouse behavior.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum HitboxBehavior {
    /// Normal hitbox mouse behavior, doesn't affect mouse handling for other hitboxes.
    #[default]
    Normal,

    /// All hitboxes behind this hitbox will be ignored and so will have `hitbox.is_hovered() ==
    /// false` and `hitbox.should_handle_scroll() == false`. Typically for elements this causes
    /// skipping of all mouse events, hover styles, and tooltips. This flag is set by
    /// [`InteractiveElement::occlude`].
    ///
    /// For mouse handlers that check those hitboxes, this behaves the same as registering a
    /// bubble-phase handler for every mouse event type:
    ///
    /// ```ignore
    /// window.on_mouse_event(move |_: &EveryMouseEventTypeHere, phase, window, cx| {
    ///     if phase == DispatchPhase::Capture && hitbox.is_hovered(window) {
    ///         cx.stop_propagation();
    ///     }
    /// })
    /// ```
    ///
    /// This has effects beyond event handling - any use of hitbox checking, such as hover
    /// styles and tooltops. These other behaviors are the main point of this mechanism. An
    /// alternative might be to not affect mouse event handling - but this would allow
    /// inconsistent UI where clicks and moves interact with elements that are not considered to
    /// be hovered.
    BlockMouse,

    /// All hitboxes behind this hitbox will have `hitbox.is_hovered() == false`, even when
    /// `hitbox.should_handle_scroll() == true`. Typically for elements this causes all mouse
    /// interaction except scroll events to be ignored - see the documentation of
    /// [`Hitbox::is_hovered`] for details. This flag is set by
    /// [`InteractiveElement::block_mouse_except_scroll`].
    ///
    /// For mouse handlers that check those hitboxes, this behaves the same as registering a
    /// bubble-phase handler for every mouse event type **except** `ScrollWheelEvent`:
    ///
    /// ```ignore
    /// window.on_mouse_event(move |_: &EveryMouseEventTypeExceptScroll, phase, window, cx| {
    ///     if phase == DispatchPhase::Bubble && hitbox.should_handle_scroll(window) {
    ///         cx.stop_propagation();
    ///     }
    /// })
    /// ```
    ///
    /// See the documentation of [`Hitbox::is_hovered`] for details of why `ScrollWheelEvent` is
    /// handled differently than other mouse events. If also blocking these scroll events is
    /// desired, then a `cx.stop_propagation()` handler like the one above can be used.
    ///
    /// This has effects beyond event handling - this affects any use of `is_hovered`, such as
    /// hover styles and tooltops. These other behaviors are the main point of this mechanism.
    /// An alternative might be to not affect mouse event handling - but this would allow
    /// inconsistent UI where clicks and moves interact with elements that are not considered to
    /// be hovered.
    BlockMouseExceptScroll,
}

/// An identifier for a tooltip.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq)]
pub struct TooltipId(usize);

impl TooltipId {
    /// Checks if the tooltip is currently hovered.
    pub fn is_hovered(&self, window: &Window) -> bool {
        window
            .tooltip_bounds
            .as_ref()
            .is_some_and(|tooltip_bounds| {
                tooltip_bounds.id == *self
                    && tooltip_bounds.bounds.contains(&window.mouse_position())
            })
    }
}

pub(crate) struct TooltipBounds {
    id: TooltipId,
    bounds: Bounds<Pixels>,
}

#[derive(Clone)]
pub(crate) struct TooltipRequest {
    id: TooltipId,
    tooltip: AnyTooltip,
}

pub(crate) struct DeferredDraw {
    current_view: EntityId,
    priority: usize,
    parent_node: DispatchNodeId,
    element_id_stack: SmallVec<[ElementId; 32]>,
    text_style_stack: Vec<TextStyleRefinement>,
    element: Option<AnyElement>,
    absolute_offset: Point<Pixels>,
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct RetainedSceneSegment {
    pub(crate) bounds: Bounds<ScaledPixels>,
    pub(crate) scene_range: Range<usize>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) entity_id: EntityId,
    pub(crate) dirty: bool,
}

pub(crate) struct Frame {
    pub(crate) focus: Option<FocusId>,
    pub(crate) window_active: bool,
    pub(crate) element_states: FxHashMap<(GlobalElementId, TypeId), ElementStateBox>,
    accessed_element_states: Vec<(GlobalElementId, TypeId)>,
    pub(crate) mouse_listeners: Vec<MouseListener>,
    pub(crate) dispatch_tree: DispatchTree,
    pub(crate) scene: Scene,
    pub(crate) hitboxes: Vec<Hitbox>,
    pub(crate) window_control_hitboxes: Vec<(WindowControlArea, Hitbox)>,
    pub(crate) deferred_draws: Vec<DeferredDraw>,
    pub(crate) input_handlers: Vec<Option<PlatformInputHandler>>,
    pub(crate) tooltip_requests: Vec<Option<TooltipRequest>>,
    pub(crate) cursor_styles: Vec<CursorStyleRequest>,
    pub(crate) retained_scene_segments: Vec<RetainedSceneSegment>,
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) debug_bounds: FxHashMap<String, Bounds<Pixels>>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) next_inspector_instance_ids: FxHashMap<Rc<crate::InspectorElementPath>, usize>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) inspector_hitboxes: FxHashMap<HitboxId, crate::InspectorElementId>,
    pub(crate) tab_stops: TabStopMap,
    idle_clear_frames: u16,
}

const FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER: usize = 4;
const FRAME_MIN_RETAINED_CAPACITY: usize = 32;
const WINDOW_LIGHT_TRIM_IDLE_FRAMES: u16 = 300;
const WINDOW_STRONG_TRIM_IDLE_FRAMES: u16 = 900;
const DIRTY_REGION_FULL_REDRAW_RATIO: f32 = 0.6;

#[derive(Clone, Default)]
pub(crate) struct PrepaintStateIndex {
    hitboxes_index: usize,
    tooltips_index: usize,
    deferred_draws_index: usize,
    dispatch_tree_index: usize,
    accessed_element_states_index: usize,
    line_layout_index: LineLayoutIndex,
}

#[derive(Clone, Default)]
pub(crate) struct PaintIndex {
    scene_index: usize,
    mouse_listeners_index: usize,
    input_handlers_index: usize,
    cursor_styles_index: usize,
    window_control_hitboxes_index: usize,
    accessed_element_states_index: usize,
    tab_handle_index: usize,
    line_layout_index: LineLayoutIndex,
}

impl Frame {
    pub(crate) fn new(dispatch_tree: DispatchTree) -> Self {
        Frame {
            focus: None,
            window_active: false,
            element_states: FxHashMap::default(),
            accessed_element_states: Vec::new(),
            mouse_listeners: Vec::new(),
            dispatch_tree,
            scene: Scene::default(),
            hitboxes: Vec::new(),
            window_control_hitboxes: Vec::new(),
            deferred_draws: Vec::new(),
            input_handlers: Vec::new(),
            tooltip_requests: Vec::new(),
            cursor_styles: Vec::new(),
            retained_scene_segments: Vec::new(),

            #[cfg(any(test, feature = "test-support"))]
            debug_bounds: FxHashMap::default(),

            #[cfg(any(feature = "inspector", debug_assertions))]
            next_inspector_instance_ids: FxHashMap::default(),

            #[cfg(any(feature = "inspector", debug_assertions))]
            inspector_hitboxes: FxHashMap::default(),
            tab_stops: TabStopMap::default(),
            idle_clear_frames: 0,
        }
    }

    pub(crate) fn clear(&mut self) {
        let had_hot_path_content = !self.mouse_listeners.is_empty()
            || !self.hitboxes.is_empty()
            || !self.window_control_hitboxes.is_empty()
            || !self.deferred_draws.is_empty()
            || !self.cursor_styles.is_empty();

        self.element_states.clear();
        self.accessed_element_states.clear();
        self.mouse_listeners.clear();
        self.dispatch_tree.clear();
        self.scene.clear();
        self.input_handlers.clear();
        self.tooltip_requests.clear();
        self.cursor_styles.clear();
        self.retained_scene_segments.clear();
        self.hitboxes.clear();
        self.window_control_hitboxes.clear();
        self.deferred_draws.clear();
        self.tab_stops.clear();
        self.focus = None;

        #[cfg(any(feature = "inspector", debug_assertions))]
        {
            self.next_inspector_instance_ids.clear();
            self.inspector_hitboxes.clear();
        }

        if had_hot_path_content {
            self.idle_clear_frames = 0;
        } else {
            self.idle_clear_frames = self.idle_clear_frames.saturating_add(1);
            if self.idle_clear_frames >= WINDOW_LIGHT_TRIM_IDLE_FRAMES {
                self.trim_retained_capacity();
                if self.idle_clear_frames >= WINDOW_STRONG_TRIM_IDLE_FRAMES {
                    self.idle_clear_frames = 0;
                }
            }
        }
    }

    pub(crate) fn cursor_style(&self, window: &Window) -> Option<CursorStyle> {
        self.cursor_styles
            .iter()
            .rev()
            .fold_while(None, |style, request| match request.hitbox_id {
                None => Done(Some(request.style)),
                Some(hitbox_id) => Continue(
                    style.or_else(|| hitbox_id.is_hovered(window).then_some(request.style)),
                ),
            })
            .into_inner()
    }

    pub(crate) fn hit_test(&self, position: Point<Pixels>) -> HitTest {
        let mut set_hover_hitbox_count = false;
        let mut hit_test = HitTest::default();
        for hitbox in self.hitboxes.iter().rev() {
            let bounds = hitbox.bounds.intersect(&hitbox.content_mask.bounds);
            if bounds.contains(&position) {
                hit_test.ids.push(hitbox.id);
                if !set_hover_hitbox_count
                    && hitbox.behavior == HitboxBehavior::BlockMouseExceptScroll
                {
                    hit_test.hover_hitbox_count = hit_test.ids.len();
                    set_hover_hitbox_count = true;
                }
                if hitbox.behavior == HitboxBehavior::BlockMouse {
                    break;
                }
            }
        }
        if !set_hover_hitbox_count {
            hit_test.hover_hitbox_count = hit_test.ids.len();
        }
        hit_test
    }

    pub(crate) fn focus_path(&self) -> SmallVec<[FocusId; 8]> {
        self.focus
            .map(|focus_id| self.dispatch_tree.focus_path(focus_id))
            .unwrap_or_default()
    }

    pub(crate) fn finish(&mut self, prev_frame: &mut Self) {
        for element_state_key in &self.accessed_element_states {
            if let Some((element_state_key, element_state)) =
                prev_frame.element_states.remove_entry(element_state_key)
            {
                self.element_states.insert(element_state_key, element_state);
            }
        }

        self.scene.finish();
    }

    fn retained_capacity(&self) -> usize {
        self.mouse_listeners.capacity()
            + self.hitboxes.capacity()
            + self.window_control_hitboxes.capacity()
            + self.deferred_draws.capacity()
            + self.cursor_styles.capacity()
            + self.retained_scene_segments.capacity()
    }

    fn trim_retained_capacity(&mut self) {
        trim_frame_vec_capacity(
            &mut self.mouse_listeners,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_frame_vec_capacity(
            &mut self.hitboxes,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_frame_vec_capacity(
            &mut self.window_control_hitboxes,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_frame_vec_capacity(
            &mut self.deferred_draws,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_frame_vec_capacity(
            &mut self.cursor_styles,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_frame_vec_capacity(
            &mut self.retained_scene_segments,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
    }

    fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light => self.trim_retained_capacity(),
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive => {
                self.mouse_listeners.shrink_to(FRAME_MIN_RETAINED_CAPACITY);
                self.hitboxes.shrink_to(FRAME_MIN_RETAINED_CAPACITY);
                self.window_control_hitboxes
                    .shrink_to(FRAME_MIN_RETAINED_CAPACITY);
                self.deferred_draws.shrink_to(FRAME_MIN_RETAINED_CAPACITY);
                self.input_handlers.shrink_to(FRAME_MIN_RETAINED_CAPACITY);
                self.tooltip_requests.shrink_to(FRAME_MIN_RETAINED_CAPACITY);
                self.cursor_styles.shrink_to(FRAME_MIN_RETAINED_CAPACITY);
                self.retained_scene_segments
                    .shrink_to(FRAME_MIN_RETAINED_CAPACITY);
            }
        }
        self.scene.trim_retained_capacity_for_level(level);
    }
}

fn trim_frame_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    if vec.capacity() > floor.saturating_mul(multiplier) {
        vec.shrink_to(floor);
    }
}

/// Holds the state for a specific window.
pub struct Window {
    pub(crate) handle: AnyWindowHandle,
    pub(crate) invalidator: WindowInvalidator,
    pub(crate) removed: bool,
    pub(crate) platform_window: Box<dyn PlatformWindow>,
    display_id: Option<DisplayId>,
    sprite_atlas: Arc<dyn PlatformAtlas>,
    text_system: Arc<WindowTextSystem>,
    default_text_style: TextStyle,
    image_pipeline_config: ImagePipelineConfig,
    trim_memory_on_hidden: bool,
    rem_size: Pixels,
    /// The stack of override values for the window's rem size.
    ///
    /// This is used by `with_rem_size` to allow rendering an element tree with
    /// a given rem size.
    rem_size_override_stack: SmallVec<[Pixels; 8]>,
    pub(crate) viewport_size: Size<Pixels>,
    layout_engine: Option<TaffyLayoutEngine>,
    pub(crate) root: Option<AnyView>,
    pub(crate) element_id_stack: SmallVec<[ElementId; 32]>,
    pub(crate) text_style_stack: Vec<TextStyleRefinement>,
    pub(crate) rendered_entity_stack: Vec<EntityId>,
    pub(crate) element_offset_stack: Vec<Point<Pixels>>,
    pub(crate) element_opacity: f32,
    pub(crate) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(crate) requested_autoscroll: Option<Bounds<Pixels>>,
    pub(crate) image_cache_stack: Vec<AnyImageCache>,
    animated_image_slots: FxHashMap<AnimatedImageSlotKey, usize>,
    pub(crate) rendered_frame: Frame,
    pub(crate) next_frame: Frame,
    render_dirty_region: DirtyRegion,
    animation_dirty_region: DirtyRegion,
    render_present_mode: PartialPresentMode,
    render_trim_policy: RetainedResourceTrimPolicy,
    force_full_redraw: Cell<bool>,
    force_view_cache_refresh: bool,
    idle_render_frames: u16,
    next_hitbox_id: HitboxId,
    pub(crate) next_tooltip_id: TooltipId,
    pub(crate) tooltip_bounds: Option<TooltipBounds>,
    next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>>,
    pub(crate) dirty_views: FxHashSet<EntityId>,
    focus_listeners: SubscriberSet<(), AnyWindowFocusListener>,
    pub(crate) focus_lost_listeners: SubscriberSet<(), AnyObserver>,
    default_prevented: bool,
    mouse_position: Point<Pixels>,
    mouse_hit_test: HitTest,
    modifiers: Modifiers,
    capslock: Capslock,
    scale_factor: f32,
    pub(crate) bounds_observers: SubscriberSet<(), AnyObserver>,
    appearance: WindowAppearance,
    pub(crate) appearance_observers: SubscriberSet<(), AnyObserver>,
    active: Rc<Cell<bool>>,
    hovered: Rc<Cell<bool>>,
    pub(crate) needs_present: Rc<Cell<bool>>,
    pub(crate) last_input_timestamp: Rc<Cell<Instant>>,
    pub(crate) refreshing: bool,
    async_app: AsyncApp,
    frame_request_watchdog: Rc<Cell<FrameRequestWatchdog>>,
    frame_throttle: WindowFrameThrottle,
    draw_deadline: Option<Instant>,
    draw_degraded_this_frame: bool,
    has_completed_rendered_frame: bool,
    critical_draw_depth: usize,
    inactive_animation_frame_pending: Rc<Cell<bool>>,
    last_inactive_animation_frame: Rc<Cell<Option<Instant>>>,
    animation_frame_pending_entities: Rc<RefCell<FxHashSet<EntityId>>>,
    deadline_invalidation_pending: Rc<RefCell<FxHashMap<EntityId, (Instant, u64)>>>,
    deadline_invalidation_generation: Rc<Cell<u64>>,
    pub(crate) activation_observers: SubscriberSet<(), AnyObserver>,
    pub(crate) focus: Option<FocusId>,
    focus_enabled: bool,
    pending_input: Option<PendingInput>,
    pending_modifier: ModifierState,
    pub(crate) pending_input_observers: SubscriberSet<(), AnyObserver>,
    prompt: Option<RenderablePromptHandle>,
    pub(crate) client_inset: Option<Pixels>,
    transparent_caption_height: Option<Pixels>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    inspector: Option<Entity<Inspector>>,
}

#[cfg(test)]
#[derive(Default)]
struct EmptyTestView;

#[cfg(test)]
impl Render for EmptyTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
    }
}

#[cfg(test)]
#[derive(Default)]
struct DeferredTestView;

#[cfg(test)]
impl Render for DeferredTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().child(crate::deferred(crate::div()))
    }
}

#[cfg(test)]
#[derive(Default)]
struct PaintedTestView;

#[cfg(test)]
impl Render for PaintedTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .w(px(100.))
            .h(px(100.))
            .bg(crate::rgb(0xff00ff))
    }
}

#[cfg(test)]
#[derive(Clone)]
struct BudgetExhaustingElement {
    prepaint_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl IntoElement for BudgetExhaustingElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
impl Element for BudgetExhaustingElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), None, _cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.prepaint_count
            .set(self.prepaint_count.get().saturating_add(1));
        window.draw_deadline = Some(Instant::now());
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

#[cfg(test)]
#[derive(Clone)]
struct BudgetExhaustingPaintCountingElement {
    prepaint_count: Rc<Cell<usize>>,
    paint_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl IntoElement for BudgetExhaustingPaintCountingElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
impl Element for BudgetExhaustingPaintCountingElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.prepaint_count
            .set(self.prepaint_count.get().saturating_add(1));
        window.draw_deadline = Some(Instant::now());
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.paint_count
            .set(self.paint_count.get().saturating_add(1));
    }
}

#[cfg(test)]
#[derive(Clone)]
struct CountingElement {
    prepaint_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl IntoElement for CountingElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
impl Element for CountingElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        self.prepaint_count
            .set(self.prepaint_count.get().saturating_add(1));
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

#[cfg(test)]
#[derive(Clone)]
struct PaintBudgetExhaustingElement {
    paint_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl IntoElement for PaintBudgetExhaustingElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
impl Element for PaintBudgetExhaustingElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        self.paint_count
            .set(self.paint_count.get().saturating_add(1));
        window.draw_deadline = Some(Instant::now());
    }
}

#[cfg(test)]
#[derive(Clone)]
struct PaintCountingElement {
    paint_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl IntoElement for PaintCountingElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
impl Element for PaintCountingElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(Style::default(), None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&crate::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
        self.paint_count
            .set(self.paint_count.get().saturating_add(1));
    }
}

#[cfg(test)]
struct DeferredBudgetTestView {
    first_count: Rc<Cell<usize>>,
    second_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl Render for DeferredBudgetTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .child(crate::deferred(BudgetExhaustingElement {
                prepaint_count: self.first_count.clone(),
            }))
            .child(crate::deferred(CountingElement {
                prepaint_count: self.second_count.clone(),
            }))
    }
}

#[cfg(test)]
struct PaintedDeferredBudgetTestView {
    first_count: Rc<Cell<usize>>,
    second_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl Render for PaintedDeferredBudgetTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .w(px(100.))
            .h(px(100.))
            .bg(crate::rgb(0xff00ff))
            .child(crate::deferred(BudgetExhaustingElement {
                prepaint_count: self.first_count.clone(),
            }))
            .child(crate::deferred(CountingElement {
                prepaint_count: self.second_count.clone(),
            }))
    }
}

#[cfg(test)]
struct DivPrepaintBudgetTestView {
    first_count: Rc<Cell<usize>>,
    second_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl Render for DivPrepaintBudgetTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .child(BudgetExhaustingElement {
                prepaint_count: self.first_count.clone(),
            })
            .child(CountingElement {
                prepaint_count: self.second_count.clone(),
            })
    }
}

#[cfg(test)]
struct DivPaintBudgetTestView {
    first_count: Rc<Cell<usize>>,
    second_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl Render for DivPaintBudgetTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .child(PaintBudgetExhaustingElement {
                paint_count: self.first_count.clone(),
            })
            .child(PaintCountingElement {
                paint_count: self.second_count.clone(),
            })
    }
}

struct RootBudgetTestView {
    prepaint_count: Rc<Cell<usize>>,
    paint_count: Rc<Cell<usize>>,
}

#[cfg(test)]
impl Render for RootBudgetTestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        crate::div().child(BudgetExhaustingPaintCountingElement {
            prepaint_count: self.prepaint_count.clone(),
            paint_count: self.paint_count.clone(),
        })
    }
}

#[cfg(test)]
#[derive(Default)]
struct ClickNotifyTestView {
    clicks: usize,
}

#[cfg(test)]
impl Render for ClickNotifyTestView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::div()
            .id("click-notify-target")
            .debug_selector(|| "click-notify-target".to_string())
            .w(px(100.))
            .h(px(100.))
            .on_click(cx.listener(|view, _, _, cx| {
                view.clicks += 1;
                cx.notify();
            }))
    }
}

#[cfg(test)]
impl Window {
    pub(crate) fn test_set_refreshing(&mut self, refreshing: bool) {
        self.refreshing = refreshing;
    }

    pub(crate) fn test_inactive_animation_frame_pending(&self) -> bool {
        self.inactive_animation_frame_pending.get()
    }

    pub(crate) fn test_exhaust_draw_budget(&mut self) {
        self.draw_deadline = Some(Instant::now());
    }

    pub(crate) fn set_transparent_caption_height_for_test(&mut self, height: Option<Pixels>) {
        self.transparent_caption_height = height;
    }

    pub(crate) fn test_deadline_invalidation_pending(&self) -> bool {
        !self.deadline_invalidation_pending.borrow().is_empty()
    }

    pub(crate) fn test_set_active_drag(&mut self, cx: &mut App, cursor_offset: Point<Pixels>) {
        cx.active_drag = Some(AnyDrag {
            value: Arc::new(()),
            view: cx.new(|_| EmptyTestView).into(),
            cursor_offset,
            cursor_style: None,
        });
    }

    pub(crate) fn test_request_animation_frame_for_image(&mut self, entity: EntityId, cx: &App) {
        self.invalidator.set_phase(DrawPhase::Paint);
        self.with_rendered_view(entity, |window| {
            window.request_animation_frame_for_image(cx, ImagePipelineConfig::default().animated)
        });
        self.invalidator.set_phase(DrawPhase::None);
    }
}

#[cfg(test)]
fn assert_request_frame_options_match(
    actual: Option<RequestFrameOptions>,
    expected: RequestFrameOptions,
) {
    let actual = actual.expect("frame request should be recorded");
    assert_eq!(actual.force_render, expected.force_render);
    assert_eq!(actual.require_presentation, expected.require_presentation);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        TestAppContext, WindowOptions, performance_metrics_snapshot, point, px,
        scene::{PaintOperation, Primitive},
        size,
    };

    #[gpui::test]
    fn repeated_refresh_requests_are_coalesced(cx: &mut TestAppContext) {
        let before = performance_metrics_snapshot().coalesced_refresh_count;
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });

        window
            .update(cx, |_, window, _| {
                window.test_set_refreshing(false);
                window.refresh();
                window.refresh();
            })
            .unwrap();

        assert!(performance_metrics_snapshot().coalesced_refresh_count > before);
    }

    #[gpui::test]
    fn repeated_refresh_windows_effects_are_coalesced(cx: &mut TestAppContext) {
        let before = performance_metrics_snapshot().coalesced_refresh_effect_count;

        cx.update(|cx| {
            cx.refresh_windows();
            cx.refresh_windows();
        });

        assert!(performance_metrics_snapshot().coalesced_refresh_effect_count > before);
    }

    #[gpui::test]
    fn windows_are_mapped_before_becoming_visible_in_test_platform(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });

        let is_shown = window
            .update(cx, |_, window, _| {
                window
                    .platform_window
                    .as_test()
                    .expect("test platform window")
                    .is_shown()
            })
            .unwrap();

        assert!(is_shown);
    }

    #[gpui::test]
    fn focused_windows_activate_after_map_in_test_platform(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            let mut options = WindowOptions::default();
            options.focus = true;
            options.show = true;
            cx.open_window(options, |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });

        let is_active = window
            .update(cx, |_, window, _| window.platform_window.is_active())
            .unwrap();

        assert!(is_active);
    }

    #[gpui::test]
    fn test_platform_windows_are_mapped_before_becoming_active(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            let mut options = WindowOptions::default();
            options.focus = true;
            options.show = true;
            cx.open_window(options, |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });

        let shown = window
            .update(cx, |_, window, _| {
                window
                    .platform_window
                    .as_test()
                    .expect("test window")
                    .is_shown()
            })
            .unwrap();

        assert!(shown);
    }

    #[gpui::test]
    fn reuse_paint_retains_window_control_hitboxes(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let bounds = Bounds::new(point(px(0.), px(0.)), size(px(300.), px(64.)));
            let hitbox = Hitbox {
                id: HitboxId(42),
                bounds,
                content_mask: ContentMask { bounds },
                behavior: HitboxBehavior::Normal,
            };
            window
                .rendered_frame
                .window_control_hitboxes
                .push((WindowControlArea::Drag, hitbox.clone()));

            let mut range_end = PaintIndex::default();
            range_end.window_control_hitboxes_index = 1;
            window.reuse_paint(PaintIndex::default()..range_end);

            assert_eq!(window.next_frame.window_control_hitboxes.len(), 1);
            let (area, control_hitbox) = &window.next_frame.window_control_hitboxes[0];
            assert_eq!(*area, WindowControlArea::Drag);
            assert_eq!(control_hitbox.id, hitbox.id);
            assert_eq!(control_hitbox.bounds, hitbox.bounds);
            assert_eq!(control_hitbox.content_mask, hitbox.content_mask);
        });
    }

    #[gpui::test]
    fn reuse_paint_preserves_mouse_listener_slots(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window
                .rendered_frame
                .mouse_listeners
                .push(MouseListener::new::<MouseDownEvent>(Box::new(
                    |_, _, _, _| {},
                )));
            window
                .rendered_frame
                .mouse_listeners
                .push(MouseListener::new::<MouseMoveEvent>(Box::new(
                    |_, _, _, _| {},
                )));

            let mut range_end = PaintIndex::default();
            range_end.mouse_listeners_index = 2;
            window.reuse_paint(PaintIndex::default()..range_end);

            assert_eq!(window.next_frame.mouse_listeners.len(), 2);
            assert!(
                window
                    .rendered_frame
                    .mouse_listeners
                    .iter()
                    .all(|listener| listener.listener.is_none())
            );
            assert!(
                window
                    .next_frame
                    .mouse_listeners
                    .iter()
                    .all(|listener| listener.listener.is_some())
            );
        });
    }

    #[gpui::test]
    fn inactive_image_animation_requests_are_throttled(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });

        window
            .update(cx, |_, window, cx| {
                let entity = cx.entity_id();
                window.test_request_animation_frame_for_image(entity, cx);
                window.test_request_animation_frame_for_image(entity, cx);
                assert!(window.test_inactive_animation_frame_pending());
            })
            .unwrap();
        cx.background_executor.allow_parking();
    }

    #[gpui::test]
    fn active_image_animation_requests_are_throttled(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });

        window
            .update(cx, |_, window, cx| {
                window.active.set(true);
                window.last_inactive_animation_frame.set(None);
                let entity = cx.entity_id();
                let test_window = window.platform_window.as_test().unwrap().clone();
                let baseline = test_window.requested_frame_count();
                window.test_request_animation_frame_for_image(entity, cx);
                window.test_request_animation_frame_for_image(entity, cx);

                assert_eq!(test_window.requested_frame_count(), baseline + 1);
            })
            .unwrap();
    }

    #[gpui::test]
    fn drag_mouse_move_keeps_window_dirty(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            window.invalidator.set_dirty(false);
            window.test_set_active_drag(cx, point(px(4.), px(4.)));
            let event = MouseMoveEvent {
                position: point(px(8.), px(8.)),
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            };
            window.dispatch_mouse_event(&event, cx);
            assert!(window.invalidator.is_dirty());
        });
    }

    #[gpui::test]
    fn pure_window_move_does_not_dirty_window(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            {
                let test_window = window.platform_window.as_test().unwrap();
                test_window.0.lock().bounds.origin = point(px(64.), px(64.));
            }

            window.invalidator.set_dirty(false);
            window.window_origin_changed(cx);

            assert!(!window.invalidator.is_dirty());
        });
    }

    #[gpui::test]
    fn content_bounds_change_still_dirties_window(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            {
                let test_window = window.platform_window.as_test().unwrap();
                test_window.0.lock().bounds.size.width += px(1.);
            }

            window.invalidator.set_dirty(false);
            window.content_bounds_changed(cx);

            assert!(window.invalidator.is_dirty());
        });
    }

    #[gpui::test]
    fn background_pointer_button_does_not_request_frame(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline = test_window.requested_frame_count();
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );

            assert_eq!(test_window.requested_frame_count(), baseline);
        });
    }

    #[gpui::test]
    fn mouse_hit_test_uses_event_position(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();

            {
                let mut lock = test_window.0.lock();
                lock.bounds.size = size(px(120.), px(120.));
            }

            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(12.), px(12.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );

            assert_eq!(window.mouse_hit_test.ids.len(), 0);
        });
    }

    #[gpui::test]
    fn refresh_requests_dirty_frame(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.invalidator.set_phase(DrawPhase::None);
            window.refresh();

            assert_request_frame_options_match(
                test_window.last_request_frame_options(),
                RequestFrameOptions::from_refresh(),
            );
        });
    }

    #[gpui::test]
    fn frame_request_watchdog_recovers_stalled_dirty_frame(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let test_window = cx.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.invalidator.set_phase(DrawPhase::None);
            window.refresh();
            test_window
        });

        assert_eq!(test_window.draw_count(), 0);
        assert_eq!(test_window.timed_out_frame_count(), 0);
        cx.executor().advance_clock(FRAME_REQUEST_WATCHDOG_TIMEOUT);
        cx.run_until_parked();

        assert_eq!(test_window.timed_out_frame_count(), 1);
        assert_eq!(test_window.draw_count(), 1);
        cx.update(|window, _cx| {
            assert!(!window.refreshing);
            assert!(!window.frame_request_watchdog.get().pending);
        });
    }

    #[gpui::test]
    fn frame_request_watchdog_defers_during_native_move(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let test_window = cx.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.invalidator.set_phase(DrawPhase::None);
            test_window.set_native_move_active(true);
            window.refresh();
            test_window
        });

        cx.executor().advance_clock(FRAME_REQUEST_WATCHDOG_TIMEOUT);
        cx.run_until_parked();

        assert_eq!(test_window.timed_out_frame_count(), 0);
        assert_eq!(test_window.draw_count(), 0);
        cx.update(|window, _cx| {
            assert!(window.frame_request_watchdog.get().pending);
        });

        test_window.set_native_move_active(false);
        let request_options = test_window
            .last_request_frame_options()
            .expect("frame request should remain pending during native move");
        test_window.simulate_request_frame(request_options);
        cx.run_until_parked();

        assert_eq!(test_window.timed_out_frame_count(), 0);
        assert_eq!(test_window.draw_count(), 1);
        cx.update(|window, _cx| {
            assert!(!window.frame_request_watchdog.get().pending);
        });
    }

    #[gpui::test]
    fn frame_request_watchdog_ignores_completed_frame_request(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let test_window = cx.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.invalidator.set_phase(DrawPhase::None);
            window.refresh();
            test_window
        });

        test_window.simulate_request_frame(RequestFrameOptions::from_refresh());
        cx.run_until_parked();
        assert_eq!(test_window.draw_count(), 1);
        cx.executor().advance_clock(FRAME_REQUEST_WATCHDOG_TIMEOUT);
        cx.run_until_parked();

        assert_eq!(test_window.timed_out_frame_count(), 0);
        assert_eq!(test_window.draw_count(), 1);
    }

    #[gpui::test]
    fn frame_request_watchdog_ignores_late_completed_frame_request(cx: &mut TestAppContext) {
        let cx = cx.add_empty_window();
        let test_window = cx.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.invalidator.set_phase(DrawPhase::None);
            window.refresh();
            test_window
        });
        let pending_options = test_window
            .last_request_frame_options()
            .expect("frame request should be recorded");

        cx.executor().advance_clock(FRAME_REQUEST_WATCHDOG_TIMEOUT);
        cx.run_until_parked();
        assert_eq!(test_window.draw_count(), 1);

        test_window.simulate_request_frame(pending_options);
        cx.run_until_parked();
        assert_eq!(test_window.draw_count(), 1);
    }

    #[gpui::test]
    fn notify_on_rendered_view_requests_dirty_frame(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| EmptyTestView);
        let (test_window, baseline) = cx.update(|window, _| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline = test_window.requested_frame_count();
            (test_window, baseline)
        });

        view.update(cx, |_, cx| cx.notify());

        assert!(test_window.requested_frame_count() > baseline);
        assert_request_frame_options_match(
            test_window.last_request_frame_options(),
            RequestFrameOptions::from_refresh(),
        );
    }

    #[gpui::test]
    fn click_notify_requests_dirty_frame_without_animation(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| ClickNotifyTestView::default());
        let bounds = cx
            .debug_bounds("click-notify-target")
            .expect("click target should render");
        let (test_window, baseline) = cx.update(|window, _| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline = test_window.requested_frame_count();
            (test_window, baseline)
        });

        cx.simulate_click(bounds.center(), Modifiers::default());

        view.read_with(cx, |view, _| assert_eq!(view.clicks, 1));
        assert!(test_window.requested_frame_count() > baseline);
        assert_request_frame_options_match(
            test_window.last_request_frame_options(),
            RequestFrameOptions::from_refresh(),
        );
    }

    #[gpui::test]
    fn click_notify_recovers_when_platform_redraw_is_lost(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, _| ClickNotifyTestView::default());
        let bounds = cx
            .debug_bounds("click-notify-target")
            .expect("click target should render");
        let test_window = cx.update(|window, _| window.platform_window.as_test().unwrap().clone());
        let baseline_draw_count = test_window.draw_count();

        cx.simulate_click(bounds.center(), Modifiers::default());

        view.read_with(cx, |view, _| assert_eq!(view.clicks, 1));
        assert_eq!(test_window.draw_count(), baseline_draw_count);
        assert_eq!(test_window.timed_out_frame_count(), 0);

        cx.executor().advance_clock(FRAME_REQUEST_WATCHDOG_TIMEOUT);
        cx.run_until_parked();

        assert_eq!(test_window.timed_out_frame_count(), 1);
        assert_eq!(test_window.draw_count(), baseline_draw_count + 1);
    }

    #[gpui::test]
    fn on_next_frame_requests_animation_frame(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.on_next_frame(|_, _| {});

            assert_request_frame_options_match(
                test_window.last_request_frame_options(),
                RequestFrameOptions {
                    require_presentation: true,
                    force_render: false,
                    request_id: 0,
                },
            );
        });
    }

    #[gpui::test]
    fn active_animation_frame_requests_force_render(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window.active.set(true);
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.request_animation_frame();

            assert_request_frame_options_match(
                test_window.last_request_frame_options(),
                RequestFrameOptions {
                    require_presentation: true,
                    force_render: true,
                    request_id: 0,
                },
            );
        });
    }

    #[gpui::test]
    fn active_animation_frame_upgrades_pending_next_frame_request(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window.active.set(true);
            let test_window = window.platform_window.as_test().unwrap().clone();

            window.on_next_frame(|_, _| {});
            window.request_animation_frame();

            assert_request_frame_options_match(
                test_window.last_request_frame_options(),
                RequestFrameOptions {
                    require_presentation: true,
                    force_render: true,
                    request_id: 0,
                },
            );
        });
    }

    #[gpui::test]
    fn repeated_on_next_frame_requests_are_coalesced(cx: &mut TestAppContext) {
        let callbacks_ran = Rc::new(Cell::new(0));
        let window = cx.add_empty_window();
        let test_window =
            window.update(|window, _cx| window.platform_window.as_test().unwrap().clone());
        let baseline = test_window.requested_frame_count();

        window.update(|window, _cx| {
            for _ in 0..3 {
                let callbacks_ran = callbacks_ran.clone();
                window.on_next_frame(move |_, _| {
                    callbacks_ran.set(callbacks_ran.get() + 1);
                });
            }
        });

        assert_eq!(test_window.requested_frame_count(), baseline + 1);
        test_window.simulate_request_frame(RequestFrameOptions {
            require_presentation: true,
            force_render: false,
            request_id: 0,
        });
        cx.run_until_parked();
        assert_eq!(callbacks_ran.get(), 3);
    }

    #[gpui::test]
    fn deadline_invalidation_does_not_request_immediate_frame(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let baseline = test_window.requested_frame_count();

            window.request_invalidation_at(Instant::now() + Duration::from_secs(1), cx);

            assert!(window.test_deadline_invalidation_pending());
            assert_eq!(test_window.requested_frame_count(), baseline);
        });
    }

    #[gpui::test]
    fn deadline_invalidation_notifies_after_deadline(cx: &mut TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, _| EmptyTestView);
        cx.update(|window, cx| {
            window.request_invalidation_at(Instant::now() + Duration::from_millis(50), cx);
        });

        cx.executor().advance_clock(Duration::from_millis(50));
        cx.run_until_parked();

        let test_window = cx.update(|window, _| window.platform_window.as_test().unwrap().clone());
        assert!(!cx.update(|window, _| window.test_deadline_invalidation_pending()));
        let options = test_window
            .last_request_frame_options()
            .expect("deadline invalidation should request a frame");
        assert!(!options.require_presentation);
        assert!(options.force_render);
    }

    #[gpui::test]
    fn passive_mouse_move_does_not_extend_recent_input_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let stale_timestamp = Instant::now() - Duration::from_secs(2);
            window.last_input_timestamp.set(stale_timestamp);

            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(8.), px(8.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );

            assert_eq!(window.last_input_timestamp.get(), stale_timestamp);
        });
    }

    #[gpui::test]
    fn dragging_mouse_move_extends_recent_input_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            let stale_timestamp = Instant::now() - Duration::from_secs(2);
            window.last_input_timestamp.set(stale_timestamp);

            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(8.), px(8.)),
                    pressed_button: Some(MouseButton::Left),
                    modifiers: Modifiers::default(),
                }),
                cx,
            );

            assert!(window.last_input_timestamp.get() > stale_timestamp);
            assert_ne!(
                test_window.last_request_frame_options(),
                Some(RequestFrameOptions {
                    require_presentation: true,
                    force_render: false,
                    request_id: 0,
                })
            );
        });
    }

    #[gpui::test]
    fn background_pointer_button_extends_recent_input_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let stale_timestamp = Instant::now() - Duration::from_secs(2);
            window.last_input_timestamp.set(stale_timestamp);

            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );

            assert!(window.last_input_timestamp.get() > stale_timestamp);
        });
    }

    #[gpui::test]
    fn inert_hitbox_pointer_button_extends_recent_input_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let stale_timestamp = Instant::now() - Duration::from_secs(2);
            window.last_input_timestamp.set(stale_timestamp);

            let hitbox = Hitbox {
                id: window.next_hitbox_id,
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
                content_mask: ContentMask {
                    bounds: Bounds::new(Point::default(), window.viewport_size),
                },
                behavior: HitboxBehavior::Normal,
            };
            window.next_hitbox_id = window.next_hitbox_id.next();
            window.rendered_frame.hitboxes.push(hitbox);

            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );

            assert!(window.last_input_timestamp.get() > stale_timestamp);
        });
    }

    #[gpui::test]
    fn handled_pointer_button_extends_recent_input_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let stale_timestamp = Instant::now() - Duration::from_secs(2);
            window.last_input_timestamp.set(stale_timestamp);

            let hitbox = Hitbox {
                id: window.next_hitbox_id,
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
                content_mask: ContentMask {
                    bounds: Bounds::new(Point::default(), window.viewport_size),
                },
                behavior: HitboxBehavior::Normal,
            };
            window.next_hitbox_id = window.next_hitbox_id.next();
            window.rendered_frame.hitboxes.push(hitbox);
            window
                .rendered_frame
                .mouse_listeners
                .push(MouseListener::new::<MouseDownEvent>(Box::new(
                    move |event, _, _, cx| {
                        if event.is::<MouseDownEvent>() {
                            cx.stop_propagation();
                        }
                    },
                )));

            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );

            assert!(window.last_input_timestamp.get() > stale_timestamp);
        });
    }

    #[gpui::test]
    fn passive_mouse_move_over_empty_area_skips_listener_dispatch(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let listener_calls = Rc::new(Cell::new(0));
            let listener_calls_clone = listener_calls.clone();
            window
                .rendered_frame
                .mouse_listeners
                .push(MouseListener::new::<MouseMoveEvent>(Box::new(
                    move |event, _, _, _| {
                        if event.is::<MouseMoveEvent>() {
                            listener_calls_clone.set(listener_calls_clone.get() + 1);
                        }
                    },
                )));

            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(8.), px(8.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );

            assert_eq!(listener_calls.get(), 0);
        });
    }

    #[gpui::test]
    fn passive_mouse_move_entering_hitbox_still_dispatches(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let listener_calls = Rc::new(Cell::new(0));
            let listener_calls_clone = listener_calls.clone();
            let hitbox = Hitbox {
                id: window.next_hitbox_id,
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
                content_mask: ContentMask {
                    bounds: Bounds::new(Point::default(), window.viewport_size),
                },
                behavior: HitboxBehavior::Normal,
            };
            window.next_hitbox_id = window.next_hitbox_id.next();
            window.rendered_frame.hitboxes.push(hitbox);
            window
                .rendered_frame
                .mouse_listeners
                .push(MouseListener::new::<MouseMoveEvent>(Box::new(
                    move |event, _, _, _| {
                        if event.is::<MouseMoveEvent>() {
                            listener_calls_clone.set(listener_calls_clone.get() + 1);
                        }
                    },
                )));

            window.dispatch_event(
                PlatformInput::MouseMove(MouseMoveEvent {
                    position: point(px(8.), px(8.)),
                    pressed_button: None,
                    modifiers: Modifiers::default(),
                }),
                cx,
            );

            assert_eq!(listener_calls.get(), 2);
        });
    }

    #[gpui::test]
    fn mouse_dispatch_skips_unrelated_listener_types(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let listener_calls = Rc::new(Cell::new(0));
            let listener_calls_clone = listener_calls.clone();
            let hitbox = Hitbox {
                id: window.next_hitbox_id,
                bounds: Bounds::new(point(px(0.), px(0.)), size(px(16.), px(16.))),
                content_mask: ContentMask {
                    bounds: Bounds::new(Point::default(), window.viewport_size),
                },
                behavior: HitboxBehavior::Normal,
            };
            window.next_hitbox_id = window.next_hitbox_id.next();
            window.rendered_frame.hitboxes.push(hitbox);
            window
                .rendered_frame
                .mouse_listeners
                .push(MouseListener::new::<MouseMoveEvent>(Box::new(
                    move |_, _, _, _| {
                        listener_calls_clone.set(listener_calls_clone.get() + 1);
                    },
                )));

            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position: point(px(8.), px(8.)),
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );

            assert_eq!(listener_calls.get(), 0);
        });
    }

    #[gpui::test]
    fn refresh_requests_force_render(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.invalidator.set_phase(DrawPhase::None);
            window.refresh();

            let options = test_window
                .last_request_frame_options()
                .expect("refresh should request a frame");
            assert!(!options.require_presentation);
            assert!(options.force_render);
        });
    }

    #[gpui::test]
    fn present_framebuffer_only_clears_needs_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.needs_present.set(true);

            window.present_framebuffer_only();

            assert!(!window.needs_present.get());
            assert_eq!(test_window.present_framebuffer_only_count(), 1);
        });
    }

    #[gpui::test]
    fn clean_active_window_frame_skips_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window.active.set(true);
            window.needs_present.set(false);
            window.invalidator.set_dirty(false);
            window.complete_frame(FrameCompletion::Normal);
        });
    }

    #[gpui::test]
    fn throttled_dirty_frame_degrades_to_retained_present(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window
                .frame_throttle
                .delay(Instant::now(), Duration::from_secs(1));
            window.invalidator.set_dirty(true);

            window.handle_frame_request(RequestFrameOptions::default(), cx);

            assert_eq!(test_window.draw_count(), 0);
            assert_eq!(test_window.present_framebuffer_only_count(), 1);
            assert!(!window.refreshing);
            assert!(!window.needs_present.get());
        });
    }

    #[gpui::test]
    fn transparent_caption_dirty_frame_does_not_degrade_to_retained_present(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.set_transparent_caption_height_for_test(Some(px(66.0)));
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window
                .frame_throttle
                .delay(Instant::now(), Duration::from_secs(1));
            window.invalidator.set_dirty(true);

            window.handle_frame_request(RequestFrameOptions::default(), cx);

            assert_eq!(test_window.draw_count(), 1);
            assert_eq!(test_window.present_framebuffer_only_count(), 0);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.needs_present.get());
        });
    }

    #[gpui::test]
    fn throttled_dirty_presentation_request_still_draws(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.refreshing = false;
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window
                .frame_throttle
                .delay(Instant::now(), Duration::from_secs(1));
            window.invalidator.set_dirty(true);

            window.handle_frame_request(
                RequestFrameOptions {
                    require_presentation: true,
                    ..RequestFrameOptions::default()
                },
                cx,
            );

            assert_eq!(test_window.draw_count(), 1);
            assert_eq!(test_window.present_framebuffer_only_count(), 0);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.needs_present.get());
        });
    }

    #[gpui::test]
    fn visible_inactive_dirty_refresh_defers_without_drawing_or_requeueing(
        cx: &mut TestAppContext,
    ) {
        let before = performance_metrics_snapshot().inactive_dirty_defer_count;
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.active.set(false);
            window.needs_present.set(false);
            window.refreshing = true;
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window
                .last_input_timestamp
                .set(Instant::now() - Duration::from_secs(2));
            window.invalidator.set_dirty(true);
            let requested_frame_count = test_window.requested_frame_count();

            window.handle_frame_request(RequestFrameOptions::from_refresh(), cx);

            assert_eq!(test_window.draw_count(), 0);
            assert_eq!(test_window.present_framebuffer_only_count(), 0);
            assert_eq!(test_window.requested_frame_count(), requested_frame_count);
            assert!(window.invalidator.is_dirty());
            assert!(!window.refreshing);
        });

        assert!(performance_metrics_snapshot().inactive_dirty_defer_count > before);
    }

    #[gpui::test]
    fn minimized_dirty_refresh_defers_without_drawing_or_requeueing(cx: &mut TestAppContext) {
        let before = performance_metrics_snapshot().inactive_dirty_defer_count;
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.active.set(false);
            window.platform_window.minimize();
            window.needs_present.set(false);
            window.refreshing = true;
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window
                .last_input_timestamp
                .set(Instant::now() - Duration::from_secs(2));
            window.invalidator.set_dirty(true);
            let requested_frame_count = test_window.requested_frame_count();

            window.handle_frame_request(RequestFrameOptions::from_refresh(), cx);

            assert_eq!(test_window.draw_count(), 0);
            assert_eq!(test_window.present_framebuffer_only_count(), 0);
            assert_eq!(test_window.requested_frame_count(), requested_frame_count);
            assert!(window.invalidator.is_dirty());
            assert!(!window.refreshing);
        });

        assert!(performance_metrics_snapshot().inactive_dirty_defer_count > before);
    }

    #[gpui::test]
    fn inactive_dirty_refresh_does_not_request_platform_frame(cx: &mut TestAppContext) {
        let before = performance_metrics_snapshot().inactive_dirty_defer_count;
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.active.set(false);
            window.platform_window.minimize();
            window.needs_present.set(false);
            window.refreshing = false;
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window
                .last_input_timestamp
                .set(Instant::now() - Duration::from_secs(2));
            let requested_frame_count = test_window.requested_frame_count();

            window.refresh();

            assert_eq!(test_window.requested_frame_count(), requested_frame_count);
            assert_eq!(test_window.draw_count(), 0);
            assert_eq!(test_window.present_framebuffer_only_count(), 0);
            assert!(window.invalidator.is_dirty());
            assert!(!window.refreshing);
        });

        assert!(performance_metrics_snapshot().inactive_dirty_defer_count > before);
    }

    #[gpui::test]
    fn recently_interacted_inactive_dirty_refresh_still_draws(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.active.set(false);
            window.needs_present.set(false);
            window.refreshing = true;
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window.last_input_timestamp.set(Instant::now());
            window.invalidator.set_dirty(true);

            window.handle_frame_request(RequestFrameOptions::from_refresh(), cx);

            assert_eq!(test_window.draw_count(), 1);
            assert_eq!(test_window.present_framebuffer_only_count(), 0);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.refreshing);
        });
    }

    #[gpui::test]
    fn active_dirty_refresh_still_draws(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.active.set(true);
            window.needs_present.set(false);
            window.refreshing = true;
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            window.invalidator.set_dirty(true);

            window.handle_frame_request(RequestFrameOptions::from_refresh(), cx);

            assert_eq!(test_window.draw_count(), 1);
            assert!(!window.invalidator.is_dirty());
        });
    }

    #[gpui::test]
    fn inactive_dirty_defer_renders_after_activation(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });
        let test_window = cx
            .update_window(window.into(), |_, window, cx| {
                let test_window = window.platform_window.as_test().unwrap().clone();
                window.active.set(false);
                window.platform_window.minimize();
                window.needs_present.set(false);
                window.refreshing = true;
                window.rendered_frame.scene.clear();
                window
                    .rendered_frame
                    .scene
                    .paint_operations
                    .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
                window
                    .last_input_timestamp
                    .set(Instant::now() - Duration::from_secs(2));
                window.invalidator.set_dirty(true);
                window.handle_frame_request(RequestFrameOptions::from_refresh(), cx);
                test_window
            })
            .unwrap();

        test_window.simulate_active_status_change(true);
        let options = test_window
            .last_request_frame_options()
            .expect("activation should request a frame");
        test_window.simulate_request_frame(options);

        assert_eq!(test_window.draw_count(), 1);
        cx.update_window(window.into(), |_, window, _cx| {
            assert!(!window.invalidator.is_dirty());
        })
        .unwrap();
    }

    #[gpui::test]
    fn activation_dirty_refresh_bypasses_slow_frame_throttle(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        let test_window = window.update(|window, _cx| {
            let test_window = window.platform_window.as_test().unwrap().clone();
            window.active.set(false);
            window.needs_present.set(false);
            window.refreshing = false;
            window
                .frame_throttle
                .delay(Instant::now(), Duration::from_secs(1));
            window.rendered_frame.scene.clear();
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(Primitive::Quad(Quad::default())));
            test_window
        });
        let requested_frame_count = test_window.requested_frame_count();

        test_window.simulate_active_status_change(true);

        assert_eq!(
            test_window.requested_frame_count(),
            requested_frame_count + 1
        );
        assert_request_frame_options_match(
            test_window.last_request_frame_options(),
            RequestFrameOptions::from_refresh(),
        );
        window.update(|window, _cx| {
            assert!(window.active.get());
            assert!(!window.frame_throttle.should_delay(Instant::now()));
        });
    }

    #[gpui::test]
    fn frame_budget_tracks_high_refresh_intervals(cx: &mut TestAppContext) {
        cx.update(|_| {
            let mut throttle = WindowFrameThrottle::default();
            let start = Instant::now();

            throttle.record_frame_request(start);
            throttle.record_frame_request(start + Duration::from_micros(4_166));

            assert!(throttle.frame_budget() < Duration::from_millis(4));
            assert!(throttle.frame_budget() >= MIN_DYNAMIC_FRAME_BUDGET);
        });
    }

    #[gpui::test]
    fn frame_budget_caps_sixty_hz_at_target_generation_budget(cx: &mut TestAppContext) {
        cx.update(|_| {
            let mut throttle = WindowFrameThrottle::default();
            let start = Instant::now();

            throttle.record_frame_request(start);
            throttle.record_frame_request(start + Duration::from_micros(16_666));

            assert_eq!(throttle.frame_budget(), TARGET_FRAME_GENERATION_BUDGET);
        });
    }

    #[gpui::test]
    fn frame_retry_delay_tracks_present_interval(cx: &mut TestAppContext) {
        cx.update(|_| {
            let mut throttle = WindowFrameThrottle::default();
            let start = Instant::now();

            throttle.record_frame_request(start);
            throttle.record_frame_request(start + Duration::from_micros(16_666));

            assert_eq!(throttle.retry_delay(), SLOW_FRAME_REQUEST);
        });
    }

    #[gpui::test]
    fn progressive_frame_retry_delay_is_active_aware(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window.active.set(true);
            assert_eq!(window.progressive_frame_retry_delay(), SLOW_FRAME_REQUEST);

            window.active.set(false);
            assert_eq!(
                window.progressive_frame_retry_delay(),
                BACKGROUND_PROGRESSIVE_FRAME_RETRY
            );

            window.platform_window.minimize();
            assert_eq!(
                window.progressive_frame_retry_delay(),
                MINIMIZED_PROGRESSIVE_FRAME_RETRY
            );
        });
    }

    #[gpui::test]
    fn over_budget_draw_skips_deferred_work_and_keeps_window_dirty(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredTestView)
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, Duration::ZERO).clear();

            assert!(window.draw_degraded_this_frame);
            assert!(window.invalidator.is_dirty());
            assert!(window.dirty_views.contains(&dirty_view));
            assert!(window.next_frame.deferred_draws.is_empty());
        })
        .unwrap();
    }

    #[gpui::test]
    fn over_budget_draw_uses_full_redraw_plan_for_progressive_frame(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredTestView)
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, Duration::ZERO).clear();

            assert!(window.draw_degraded_this_frame);
            assert_eq!(window.render_present_mode, PartialPresentMode::FullRedraw);
            assert!(window.render_dirty_region.is_full());
        })
        .unwrap();
    }

    #[gpui::test]
    fn over_budget_draw_keeps_previous_rendered_frame(cx: &mut TestAppContext) {
        let first_count = Rc::new(Cell::new(0));
        let second_count = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| PaintedDeferredBudgetTestView {
                    first_count: first_count.clone(),
                    second_count: second_count.clone(),
                })
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        let previous_scene_len = cx
            .update_window(window, |_, window, cx| {
                window.draw(cx, SLOW_FRAME_REQUEST).clear();
                window.rendered_frame.scene.len()
            })
            .unwrap();
        assert!(previous_scene_len > 0);

        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, Duration::ZERO).clear();

            assert!(window.draw_degraded_this_frame);
            assert!(window.invalidator.is_dirty());
            assert_eq!(window.rendered_frame.scene.len(), previous_scene_len);
            assert!(window.dirty_views.contains(&dirty_view));
        })
        .unwrap();
    }

    #[gpui::test]
    fn over_budget_root_prepaint_keeps_root_paint_complete(cx: &mut TestAppContext) {
        let prepaint_count = Rc::new(Cell::new(0));
        let paint_count = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| RootBudgetTestView {
                    prepaint_count: prepaint_count.clone(),
                    paint_count: paint_count.clone(),
                })
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, SLOW_FRAME_REQUEST).clear();

            assert!(!window.draw_degraded_this_frame);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.dirty_views.contains(&dirty_view));
        })
        .unwrap();

        assert!(prepaint_count.get() > 0);
        assert!(paint_count.get() > 0);
    }

    #[gpui::test]
    fn over_budget_div_child_prepaint_finishes_root_children(cx: &mut TestAppContext) {
        let first_count = Rc::new(Cell::new(0));
        let second_count = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DivPrepaintBudgetTestView {
                    first_count: first_count.clone(),
                    second_count: second_count.clone(),
                })
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, SLOW_FRAME_REQUEST).clear();

            assert!(!window.draw_degraded_this_frame);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.dirty_views.contains(&dirty_view));
        })
        .unwrap();

        assert!(first_count.get() > 0);
        assert!(second_count.get() > 0);
    }

    #[gpui::test]
    fn over_budget_div_child_paint_finishes_root_children(cx: &mut TestAppContext) {
        let first_count = Rc::new(Cell::new(0));
        let second_count = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DivPaintBudgetTestView {
                    first_count: first_count.clone(),
                    second_count: second_count.clone(),
                })
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        cx.update_window(window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();

        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, SLOW_FRAME_REQUEST).clear();

            assert!(!window.draw_degraded_this_frame);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.dirty_views.contains(&dirty_view));
        })
        .unwrap();

        assert!(first_count.get() > 0);
        assert!(second_count.get() > 0);
    }

    #[gpui::test]
    fn completed_draw_clears_dirty_views(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredTestView)
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, SLOW_FRAME_REQUEST).clear();

            assert!(!window.draw_degraded_this_frame);
            assert!(!window.dirty_views.contains(&dirty_view));
        })
        .unwrap();
    }

    #[gpui::test]
    fn draw_ignores_budget_until_progressive_degradation_is_allowed(cx: &mut TestAppContext) {
        let first_count = Rc::new(Cell::new(0));
        let second_count = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredBudgetTestView {
                    first_count: first_count.clone(),
                    second_count: second_count.clone(),
                })
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);

        first_count.set(0);
        second_count.set(0);
        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.has_completed_rendered_frame = false;
            window.draw_degraded_this_frame = false;
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, Duration::ZERO).clear();

            assert!(!window.draw_degraded_this_frame);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.dirty_views.contains(&dirty_view));
        })
        .unwrap();

        assert!(first_count.get() > 0);
        assert!(second_count.get() > 0);

        first_count.set(0);
        second_count.set(0);
        cx.update_window(window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.set_transparent_caption_height_for_test(Some(px(66.0)));
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.draw(cx, Duration::ZERO).clear();

            assert!(!window.draw_degraded_this_frame);
            assert!(!window.invalidator.is_dirty());
            assert!(!window.dirty_views.contains(&dirty_view));
        })
        .unwrap();

        assert!(first_count.get() > 0);
        assert!(second_count.get() > 0);
    }

    #[gpui::test]
    fn over_budget_draw_in_one_window_does_not_block_another_window(cx: &mut TestAppContext) {
        let slow_first_count = Rc::new(Cell::new(0));
        let slow_second_count = Rc::new(Cell::new(0));
        let slow_window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredBudgetTestView {
                    first_count: slow_first_count.clone(),
                    second_count: slow_second_count.clone(),
                })
            })
            .unwrap()
        });
        let responsive_window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredTestView)
            })
            .unwrap()
        });
        let slow_window = AnyWindowHandle::from(slow_window);
        let responsive_window = AnyWindowHandle::from(responsive_window);

        let responsive_platform_window = cx
            .update_window(responsive_window, |_, window, _| {
                window.platform_window.as_test().unwrap().clone()
            })
            .unwrap();
        let responsive_frame_requests = responsive_platform_window.requested_frame_count();
        let responsive_draws = responsive_platform_window.draw_count();

        cx.update_window(slow_window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();
        let slow_second_count_before_dirty_draw = slow_second_count.get();
        cx.update_window(responsive_window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();

        cx.update_window(slow_window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);

            window.handle_frame_request(RequestFrameOptions::from_refresh(), cx);

            assert!(window.draw_degraded_this_frame);
            assert!(window.invalidator.is_dirty());
            assert!(window.dirty_views.contains(&dirty_view));
        })
        .unwrap();
        assert!(slow_first_count.get() > 0);
        assert_eq!(slow_second_count.get(), slow_second_count_before_dirty_draw);

        cx.update_window(responsive_window, |_, window, _| {
            window.refreshing = false;
            window.invalidator.set_phase(DrawPhase::None);
            window.refresh();
        })
        .unwrap();

        assert!(responsive_platform_window.requested_frame_count() > responsive_frame_requests);
        let request_options = responsive_platform_window
            .last_request_frame_options()
            .expect("responsive window should request its own frame");
        responsive_platform_window.simulate_request_frame(request_options);
        cx.run_until_parked();

        assert_eq!(
            responsive_platform_window.draw_count(),
            responsive_draws + 1
        );
        cx.update_window(responsive_window, |_, window, _| {
            assert!(!window.draw_degraded_this_frame);
            assert!(!window.invalidator.is_dirty());
        })
        .unwrap();
    }

    #[gpui::test]
    fn over_budget_draw_in_one_window_does_not_block_input_and_refresh_in_another_window(
        cx: &mut TestAppContext,
    ) {
        let slow_first_count = Rc::new(Cell::new(0));
        let slow_second_count = Rc::new(Cell::new(0));
        let slow_window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredBudgetTestView {
                    first_count: slow_first_count.clone(),
                    second_count: slow_second_count.clone(),
                })
            })
            .unwrap()
        });
        let responsive_window_handle = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| ClickNotifyTestView::default())
            })
            .unwrap()
        });
        let slow_window = AnyWindowHandle::from(slow_window);
        let responsive_view = responsive_window_handle.root(cx).unwrap();
        let responsive_window = AnyWindowHandle::from(responsive_window_handle);

        let test_window = cx
            .update_window(responsive_window, |_, window, _| {
                window.platform_window.as_test().unwrap().clone()
            })
            .unwrap();
        let baseline_requests = test_window.requested_frame_count();

        cx.update_window(slow_window, |_, window, cx| {
            window.draw(cx, SLOW_FRAME_REQUEST).clear();
        })
        .unwrap();
        let slow_second_count_before_dirty_draw = slow_second_count.get();

        cx.update_window(slow_window, |_, window, cx| {
            let dirty_view = window.root.as_ref().unwrap().entity_id();
            window.dirty_views.insert(dirty_view);
            window.invalidator.set_dirty(true);
            window.handle_frame_request(RequestFrameOptions::from_refresh(), cx);
        })
        .unwrap();
        assert!(slow_first_count.get() > 0);
        assert_eq!(slow_second_count.get(), slow_second_count_before_dirty_draw);

        let bounds = cx
            .update_window(responsive_window, |_, window, _| {
                window
                    .rendered_frame
                    .debug_bounds
                    .get("click-notify-target")
                    .copied()
            })
            .unwrap()
            .expect("click target should render");
        let position = bounds.center();
        cx.update_window(responsive_window, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(MouseDownEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                    first_mouse: false,
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(MouseUpEvent {
                    button: MouseButton::Left,
                    position,
                    modifiers: Modifiers::default(),
                    click_count: 1,
                }),
                cx,
            );
        })
        .unwrap();

        responsive_view.read_with(cx, |view, _| assert_eq!(view.clicks, 1));
        assert!(test_window.requested_frame_count() > baseline_requests);
    }

    #[gpui::test]
    fn over_budget_deferred_prepaint_stops_remaining_deferred_draws(cx: &mut TestAppContext) {
        let first_count = Rc::new(Cell::new(0));
        let second_count = Rc::new(Cell::new(0));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| {
                cx.new(|_| DeferredBudgetTestView {
                    first_count: first_count.clone(),
                    second_count: second_count.clone(),
                })
            })
            .unwrap()
        });
        let window = AnyWindowHandle::from(window);
        let second_count_before_dirty_draw = second_count.get();

        cx.update_window(window, |_, window, cx| {
            window.invalidator.set_dirty(true);
            window.draw(cx, SLOW_FRAME_REQUEST).clear();

            assert!(window.draw_degraded_this_frame);
            assert!(first_count.get() > 0);
            assert_eq!(second_count.get(), second_count_before_dirty_draw);
        })
        .unwrap();
    }

    #[gpui::test]
    fn over_budget_tooltip_prepaint_stops_remaining_tooltips(cx: &mut TestAppContext) {
        let first_count = Rc::new(Cell::new(0usize));
        let second_count = Rc::new(Cell::new(0usize));
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| EmptyTestView))
                .unwrap()
        });
        let window = AnyWindowHandle::from(window);
        let second_count_before_dirty_draw = second_count.get();

        cx.update_window(window, |_, window, cx| {
            let second_count_for_tooltip = second_count.clone();
            window
                .next_frame
                .tooltip_requests
                .push(Some(TooltipRequest {
                    id: TooltipId(1),
                    tooltip: AnyTooltip {
                        view: cx.new(|_| EmptyTestView).into(),
                        mouse_position: point(px(0.), px(0.)),
                        check_visible_and_update: Rc::new(move |_, _, _| {
                            second_count_for_tooltip
                                .set(second_count_for_tooltip.get().saturating_add(1));
                            true
                        }),
                    },
                }));

            let first_count_for_tooltip = first_count.clone();
            window
                .next_frame
                .tooltip_requests
                .push(Some(TooltipRequest {
                    id: TooltipId(2),
                    tooltip: AnyTooltip {
                        view: cx.new(|_| EmptyTestView).into(),
                        mouse_position: point(px(0.), px(0.)),
                        check_visible_and_update: Rc::new(move |_, window, _| {
                            first_count_for_tooltip
                                .set(first_count_for_tooltip.get().saturating_add(1));
                            window.draw_deadline = Some(Instant::now());
                            false
                        }),
                    },
                }));

            window.invalidator.set_dirty(true);
            window.draw(cx, SLOW_FRAME_REQUEST).clear();

            assert!(window.draw_degraded_this_frame);
            assert!(first_count.get() > 0);
            assert_eq!(second_count.get(), second_count_before_dirty_draw);
        })
        .unwrap();
    }

    #[gpui::test]
    fn dirty_region_full_redraw_for_large_area(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            let viewport =
                Bounds::new(Point::default(), window.viewport_size).scale(window.scale_factor);
            let mut region = DirtyRegion::empty();
            region.push(viewport);

            region.coalesce_if_large(viewport, DIRTY_REGION_FULL_REDRAW_RATIO);

            assert!(region.is_full());
        });
    }

    #[gpui::test]
    fn clean_retained_segments_keep_empty_partial_dirty_region(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window
                .next_frame
                .retained_scene_segments
                .push(RetainedSceneSegment {
                    bounds: Bounds::new(
                        Point::default(),
                        size(ScaledPixels(10.0), ScaledPixels(10.0)),
                    ),
                    scene_range: 0..1,
                    paint_range: PaintIndex::default()..PaintIndex::default(),
                    prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
                    entity_id: EntityId::from(1),
                    dirty: false,
                });

            window.prepare_render_plan_for_next_frame(false);

            assert_eq!(window.render_present_mode, PartialPresentMode::Partial);
            assert!(window.render_dirty_region.is_empty());
        });
    }

    #[gpui::test]
    fn transparent_windows_force_full_redraw(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window.set_background_appearance(WindowBackgroundAppearance::Transparent);
            window
                .next_frame
                .retained_scene_segments
                .push(RetainedSceneSegment {
                    bounds: Bounds::new(
                        Point::default(),
                        size(ScaledPixels(10.0), ScaledPixels(10.0)),
                    ),
                    scene_range: 0..1,
                    paint_range: PaintIndex::default()..PaintIndex::default(),
                    prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
                    entity_id: EntityId::from(1),
                    dirty: false,
                });

            window.prepare_render_plan_for_next_frame(false);

            assert_eq!(window.render_present_mode, PartialPresentMode::FullRedraw);
            assert!(window.render_dirty_region.is_full());
        });
    }

    #[gpui::test]
    fn animation_dirty_bounds_keep_partial_redraw_with_clean_retained_segments(
        cx: &mut TestAppContext,
    ) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            window
                .next_frame
                .retained_scene_segments
                .push(RetainedSceneSegment {
                    bounds: Bounds::new(
                        Point::default(),
                        size(ScaledPixels(10.0), ScaledPixels(10.0)),
                    ),
                    scene_range: 0..1,
                    paint_range: PaintIndex::default()..PaintIndex::default(),
                    prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
                    entity_id: EntityId::from(1),
                    dirty: false,
                });

            window.invalidator.set_phase(DrawPhase::Prepaint);
            window
                .mark_animation_dirty(Bounds::new(point(px(20.), px(20.)), size(px(40.), px(40.))));
            window.invalidator.set_phase(DrawPhase::None);
            window.prepare_render_plan_for_next_frame(false);

            assert_eq!(window.render_present_mode, PartialPresentMode::Partial);
            assert!(!window.render_dirty_region.is_empty());
            assert!(!window.render_dirty_region.is_full());
        });
    }

    #[gpui::test]
    fn reuse_paint_remaps_retained_scene_range_to_replayed_range(cx: &mut TestAppContext) {
        let window = cx.add_empty_window();
        window.update(|window, _cx| {
            fn quad_at(x: f32) -> Primitive {
                let bounds = Bounds::new(
                    point(ScaledPixels(x), ScaledPixels(0.0)),
                    size(ScaledPixels(10.0), ScaledPixels(10.0)),
                );
                Primitive::Quad(Quad {
                    bounds,
                    content_mask: ContentMask { bounds },
                    ..Quad::default()
                })
            }

            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(quad_at(0.0)));
            window
                .rendered_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(quad_at(10.0)));
            window
                .rendered_frame
                .retained_scene_segments
                .push(RetainedSceneSegment {
                    bounds: Bounds::new(
                        point(ScaledPixels(10.0), ScaledPixels(0.0)),
                        size(ScaledPixels(10.0), ScaledPixels(10.0)),
                    ),
                    scene_range: 1..2,
                    paint_range: PaintIndex::default()..PaintIndex::default(),
                    prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
                    entity_id: EntityId::from(1),
                    dirty: false,
                });
            window
                .next_frame
                .scene
                .paint_operations
                .push(PaintOperation::Primitive(quad_at(20.0)));
            window.rendered_entity_stack.push(EntityId::from(2));
            window.invalidator.set_phase(DrawPhase::Paint);

            window.reuse_paint(
                PaintIndex {
                    scene_index: 0,
                    ..PaintIndex::default()
                }..PaintIndex {
                    scene_index: 2,
                    ..PaintIndex::default()
                },
            );

            window.invalidator.set_phase(DrawPhase::None);
            window.rendered_entity_stack.pop();

            assert_eq!(window.next_frame.scene.len(), 3);
            assert_eq!(window.next_frame.retained_scene_segments.len(), 1);
            assert_eq!(
                window.next_frame.retained_scene_segments[0].scene_range,
                1..3
            );
            assert_eq!(
                window.next_frame.retained_scene_segments[0].entity_id,
                EntityId::from(2)
            );
        });
    }
}

#[derive(Clone, Debug, Default)]
struct ModifierState {
    modifiers: Modifiers,
    saw_keystroke: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawPhase {
    None,
    Prepaint,
    Paint,
    Focus,
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameRequestLoad {
    dirty: bool,
    pending_present: bool,
    active: bool,
    minimized: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FrameCompletion {
    Normal,
    DeferredInactiveDirty,
}

#[derive(Default, Debug)]
struct PendingInput {
    keystrokes: SmallVec<[Keystroke; 1]>,
    focus: Option<FocusId>,
    timer: Option<Task<()>>,
}

#[derive(Clone, Copy, Default, Debug)]
struct FrameRequestWatchdog {
    generation: u64,
    pending: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct WindowFrameThrottle {
    retry_after: Option<Instant>,
    retry_generation: u64,
    armed_retry_generation: Option<u64>,
    last_frame_request_at: Option<Instant>,
    estimated_frame_interval: Option<Duration>,
}

impl WindowFrameThrottle {
    fn should_delay(self, now: Instant) -> bool {
        self.retry_after
            .is_some_and(|retry_after| now < retry_after)
    }

    fn delay(&mut self, now: Instant, duration: Duration) {
        let retry_after = now + duration;
        if self.retry_after.is_none_or(|current| current < retry_after) {
            self.retry_after = Some(retry_after);
            self.retry_generation = self.retry_generation.saturating_add(1);
            self.armed_retry_generation = None;
        }
    }

    fn clear_if_ready(&mut self, now: Instant) {
        if self
            .retry_after
            .is_some_and(|retry_after| now >= retry_after)
        {
            self.retry_after = None;
            self.armed_retry_generation = None;
        }
    }

    fn arm_retry_timer(&mut self) -> Option<(Instant, u64)> {
        let retry_after = self.retry_after?;
        if self.armed_retry_generation == Some(self.retry_generation) {
            return None;
        }
        self.armed_retry_generation = Some(self.retry_generation);
        Some((retry_after, self.retry_generation))
    }

    fn retry_timer_fired(&mut self, generation: u64, now: Instant) -> bool {
        if self.retry_generation != generation {
            return false;
        }
        self.armed_retry_generation = None;
        self.clear_if_ready(now);
        !self.should_delay(now)
    }

    fn clear_delay(&mut self) {
        self.retry_after = None;
        self.armed_retry_generation = None;
    }

    fn record_frame_request(&mut self, now: Instant) {
        if let Some(previous) = self.last_frame_request_at {
            let interval = now.saturating_duration_since(previous);
            if (Duration::from_millis(1)..=Duration::from_millis(100)).contains(&interval) {
                self.estimated_frame_interval = Some(match self.estimated_frame_interval {
                    Some(current) => average_duration(current, interval),
                    None => interval,
                });
            }
        }
        self.last_frame_request_at = Some(now);
    }

    fn frame_budget(self) -> Duration {
        let budget = self
            .estimated_frame_interval
            .filter(|interval| *interval < HIGH_REFRESH_FRAME_INTERVAL)
            .map(|interval| interval.mul_f32(HIGH_REFRESH_FRAME_BUDGET_HEADROOM))
            .unwrap_or(TARGET_FRAME_GENERATION_BUDGET);
        budget.clamp(MIN_DYNAMIC_FRAME_BUDGET, TARGET_FRAME_GENERATION_BUDGET)
    }

    fn retry_delay(self) -> Duration {
        self.estimated_frame_interval
            .unwrap_or(SLOW_FRAME_REQUEST)
            .clamp(TARGET_FRAME_GENERATION_BUDGET, SLOW_FRAME_REQUEST)
    }
}

fn average_duration(current: Duration, sample: Duration) -> Duration {
    let current_micros = current.as_micros();
    let sample_micros = sample.as_micros();
    let average_micros = (current_micros.saturating_mul(3) + sample_micros) / 4;
    Duration::from_micros(average_micros.min(u128::from(u64::MAX)) as u64)
}

pub(crate) struct ElementStateBox {
    pub(crate) inner: Box<dyn Any>,
    #[cfg(debug_assertions)]
    pub(crate) type_name: &'static str,
}

fn default_bounds(display_id: Option<DisplayId>, cx: &mut App) -> Bounds<Pixels> {
    const DEFAULT_WINDOW_OFFSET: Point<Pixels> = point(px(0.), px(35.));

    // TODO, BUG: if you open a window with the currently active window
    // on the stack, this will erroneously select the 'unwrap_or_else'
    // code path
    cx.active_window()
        .and_then(|w| w.update(cx, |_, window, _| window.bounds()).ok())
        .map(|mut bounds| {
            bounds.origin += DEFAULT_WINDOW_OFFSET;
            bounds
        })
        .unwrap_or_else(|| {
            let display = display_id
                .map(|id| cx.find_display(id))
                .unwrap_or_else(|| cx.primary_display());

            display
                .map(|display| display.default_bounds())
                .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), DEFAULT_WINDOW_SIZE))
        })
}

impl Window {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        options: WindowOptions,
        cx: &mut App,
    ) -> Result<Self> {
        let WindowOptions {
            window_bounds,
            titlebar,
            window_icon,
            focus,
            show,
            kind,
            is_movable,
            is_resizable,
            is_minimizable,
            display_id,
            window_background,
            window_corner_preference,
            app_id,
            window_min_size,
            window_decorations,
            #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
            tabbing_identifier,
        } = options;
        let transparent_caption_height = titlebar.as_ref().and_then(|titlebar| {
            titlebar
                .appears_transparent
                .then_some(titlebar.transparent_caption_height.unwrap_or(px(32.0)))
        });

        let bounds = window_bounds
            .map(|bounds| bounds.get_bounds())
            .unwrap_or_else(|| default_bounds(display_id, cx));
        let mut platform_window = cx.platform.open_window(
            handle,
            WindowParams {
                bounds,
                titlebar,
                window_icon: window_icon.or_else(|| cx.default_window_icon.clone()),
                kind,
                is_movable,
                is_resizable,
                is_minimizable,
                focus,
                show,
                display_id,
                window_background,
                window_min_size,
                window_corner_preference,
                #[cfg(target_os = "macos")]
                tabbing_identifier,
            },
        )?;

        let tab_bar_visible = platform_window.tab_bar_visible();
        SystemWindowTabController::init_visible(cx, tab_bar_visible);
        if let Some(tabs) = platform_window.tabbed_windows() {
            SystemWindowTabController::add_tab(cx, handle.window_id(), tabs);
        }

        let display_id = platform_window.display().map(|display| display.id());
        let sprite_atlas = platform_window.sprite_atlas();
        let mouse_position = platform_window.mouse_position();
        let modifiers = platform_window.modifiers();
        let capslock = platform_window.capslock();
        let content_size = platform_window.content_size();
        let scale_factor = platform_window.scale_factor();
        let appearance = platform_window.appearance();
        #[expect(
            clippy::arc_with_non_send_sync,
            reason = "window internals share text shaping state through Arc on the foreground thread"
        )]
        let text_system = Arc::new(WindowTextSystem::new(
            cx.text_system().clone(),
            Some(cx.background_executor().clone()),
        ));
        let invalidator = WindowInvalidator::new();
        let active = Rc::new(Cell::new(platform_window.is_active()));
        let hovered = Rc::new(Cell::new(platform_window.is_hovered()));
        let needs_present = Rc::new(Cell::new(false));
        let next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>> = Default::default();
        let last_input_timestamp = Rc::new(Cell::new(Instant::now() - Duration::from_secs(2)));
        let async_app = cx.to_async();
        let frame_request_watchdog = Rc::new(Cell::new(FrameRequestWatchdog::default()));
        let inactive_animation_frame_pending = Rc::new(Cell::new(false));
        let animation_frame_pending_entities = Rc::new(RefCell::new(FxHashSet::default()));
        let deadline_invalidation_pending = Rc::new(RefCell::new(FxHashMap::default()));
        let deadline_invalidation_generation = Rc::new(Cell::new(0));

        platform_window
            .request_decorations(window_decorations.unwrap_or(WindowDecorations::Server));
        platform_window.set_background_appearance(window_background);

        if let Some(ref window_open_state) = window_bounds {
            match window_open_state {
                WindowBounds::Fullscreen(_) => platform_window.toggle_fullscreen(),
                WindowBounds::Maximized(_) => platform_window.zoom(),
                WindowBounds::Windowed(_) => {}
            }
        }

        platform_window.on_close(Box::new({
            let window_id = handle.window_id();
            let mut cx = cx.to_async();
            move || {
                let _ = handle.update(&mut cx, |_, window, _| window.remove_window());
                let _ = cx.update(|cx| {
                    SystemWindowTabController::remove_tab(cx, window_id);
                });
            }
        }));
        platform_window.on_request_frame(Box::new({
            let mut cx = cx.to_async();
            let frame_request_watchdog = frame_request_watchdog.clone();
            move |request_frame_options| {
                let should_handle = {
                    let mut watchdog = frame_request_watchdog.get();
                    if request_frame_options.request_id == 0 {
                        if watchdog.pending {
                            watchdog.pending = false;
                        }
                        frame_request_watchdog.set(watchdog);
                        true
                    } else if watchdog.pending
                        && watchdog.generation == request_frame_options.request_id
                    {
                        watchdog.pending = false;
                        frame_request_watchdog.set(watchdog);
                        true
                    } else {
                        false
                    }
                };
                if !should_handle {
                    log::debug!(
                        "gpui stale frame request ignored: window={} request_id={} generation={} require_presentation={} force_render={}",
                        handle.window_id().as_u64(),
                        request_frame_options.request_id,
                        frame_request_watchdog.get().generation,
                        request_frame_options.require_presentation,
                        request_frame_options.force_render
                    );
                    return;
                }

                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    window.handle_frame_request(request_frame_options, cx);
                }));
            }
        }));
        platform_window.on_resize(Box::new({
            let mut cx = cx.to_async();
            move |_, _| {
                let _ = ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.content_bounds_changed(cx)),
                );
            }
        }));
        platform_window.on_moved(Box::new({
            let mut cx = cx.to_async();
            move || {
                let _ = ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.window_origin_changed(cx)),
                );
            }
        }));
        platform_window.on_appearance_changed(Box::new({
            let mut cx = cx.to_async();
            move || {
                let _ = ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.appearance_changed(cx)),
                );
            }
        }));
        platform_window.on_active_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    window.active.set(active);
                    if active {
                        window.last_inactive_animation_frame.set(None);
                        window.inactive_animation_frame_pending.set(false);
                        window.frame_throttle.clear_delay();
                    } else if cx.gpui_memory_policy().trim_on_window_hidden {
                        window.trim_gpui_memory(GpuiMemoryTrimLevel::Light);
                    }
                    window.modifiers = window.platform_window.modifiers();
                    window.capslock = window.platform_window.capslock();
                    window
                        .activation_observers
                        .clone()
                        .retain(&(), |callback| callback(window, cx));

                    window.content_bounds_changed(cx);
                    window.refresh();

                    SystemWindowTabController::update_last_active(cx, window.handle.id);
                }));
            }
        }));
        platform_window.on_hover_status_change(Box::new({
            let mut cx = cx.to_async();
            move |active| {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, _| {
                    window.hovered.set(active);
                    window.refresh();
                }));
            }
        }));
        platform_window.on_input({
            let mut cx = cx.to_async();
            Box::new(move |event| {
                ignore_window_not_found(
                    handle.update(&mut cx, |_, window, cx| window.dispatch_event(event, cx)),
                )
                .unwrap_or(DispatchEventResult::default())
            })
        });
        platform_window.on_hit_test_window_control({
            let mut cx = cx.to_async();
            Box::new(move || {
                ignore_window_not_found(handle.update(&mut cx, |_, window, _cx| {
                    for (area, hitbox) in &window.rendered_frame.window_control_hitboxes {
                        if window.mouse_hit_test.ids.contains(&hitbox.id) {
                            return Some(*area);
                        }
                    }
                    None
                }))
                .unwrap_or(None)
            })
        });
        platform_window.on_move_tab_to_new_window({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::move_tab_to_new_window(cx, handle.window_id());
                }));
            })
        });
        platform_window.on_merge_all_windows({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::merge_all_windows(cx, handle.window_id());
                }));
            })
        });
        platform_window.on_select_next_tab({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::select_next_tab(cx, handle.window_id());
                }));
            })
        });
        platform_window.on_select_previous_tab({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, _window, cx| {
                    SystemWindowTabController::select_previous_tab(cx, handle.window_id())
                }));
            })
        });
        platform_window.on_toggle_tab_bar({
            let mut cx = cx.to_async();
            Box::new(move || {
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    let tab_bar_visible = window.platform_window.tab_bar_visible();
                    SystemWindowTabController::set_visible(cx, tab_bar_visible);
                }));
            })
        });

        if let Some(app_id) = app_id {
            platform_window.set_app_id(&app_id);
        }

        platform_window.map_window().unwrap();

        let client_inset = if matches!(
            platform_window.window_decorations(),
            Decorations::Client { .. }
        ) && is_resizable
        {
            let inset = platform_window.default_client_inset().unwrap_or(px(8.0));
            platform_window.set_client_inset(inset);
            Some(inset)
        } else {
            None
        };

        Ok(Window {
            handle,
            invalidator,
            removed: false,
            platform_window,
            display_id,
            sprite_atlas,
            text_system,
            default_text_style: cx.default_text_style.clone(),
            image_pipeline_config: cx.image_pipeline_config(),
            trim_memory_on_hidden: cx.gpui_memory_policy().trim_on_window_hidden,
            rem_size: px(16.),
            rem_size_override_stack: SmallVec::new(),
            viewport_size: content_size,
            layout_engine: Some(TaffyLayoutEngine::new()),
            root: None,
            element_id_stack: SmallVec::default(),
            text_style_stack: Vec::new(),
            rendered_entity_stack: Vec::new(),
            element_offset_stack: Vec::new(),
            content_mask_stack: Vec::new(),
            element_opacity: 1.0,
            requested_autoscroll: None,
            rendered_frame: Frame::new(DispatchTree::new(cx.keymap.clone(), cx.actions.clone())),
            next_frame: Frame::new(DispatchTree::new(cx.keymap.clone(), cx.actions.clone())),
            render_dirty_region: DirtyRegion::empty(),
            animation_dirty_region: DirtyRegion::empty(),
            render_present_mode: PartialPresentMode::FullRedraw,
            render_trim_policy: RetainedResourceTrimPolicy::None,
            force_full_redraw: Cell::new(true),
            force_view_cache_refresh: true,
            idle_render_frames: 0,
            next_frame_callbacks,
            next_hitbox_id: HitboxId(0),
            next_tooltip_id: TooltipId::default(),
            tooltip_bounds: None,
            dirty_views: FxHashSet::default(),
            focus_listeners: SubscriberSet::new(),
            focus_lost_listeners: SubscriberSet::new(),
            default_prevented: true,
            mouse_position,
            mouse_hit_test: HitTest::default(),
            modifiers,
            capslock,
            scale_factor,
            bounds_observers: SubscriberSet::new(),
            appearance,
            appearance_observers: SubscriberSet::new(),
            active,
            hovered,
            needs_present,
            last_input_timestamp,
            refreshing: false,
            async_app,
            frame_request_watchdog,
            frame_throttle: WindowFrameThrottle::default(),
            draw_deadline: None,
            draw_degraded_this_frame: false,
            has_completed_rendered_frame: false,
            critical_draw_depth: 0,
            inactive_animation_frame_pending,
            last_inactive_animation_frame: Rc::new(Cell::new(None)),
            animation_frame_pending_entities,
            deadline_invalidation_pending,
            deadline_invalidation_generation,
            activation_observers: SubscriberSet::new(),
            focus: None,
            focus_enabled: true,
            pending_input: None,
            pending_modifier: ModifierState::default(),
            pending_input_observers: SubscriberSet::new(),
            prompt: None,
            client_inset,
            transparent_caption_height,
            image_cache_stack: Vec::new(),
            animated_image_slots: FxHashMap::default(),
            #[cfg(any(feature = "inspector", debug_assertions))]
            inspector: None,
        })
    }

    pub(crate) fn new_focus_listener(
        &self,
        value: AnyWindowFocusListener,
    ) -> (Subscription, impl FnOnce() + use<>) {
        self.focus_listeners.insert((), value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DispatchEventResult {
    pub propagate: bool,
    pub default_prevented: bool,
}

/// Indicates which region of the window is visible. Content falling outside of this mask will not be
/// rendered. Currently, only rectangular content masks are supported, but we give the mask its own type
/// to leave room to support more complex shapes in the future.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct ContentMask<P: Clone + Debug + Default + PartialEq> {
    /// The bounds
    pub bounds: Bounds<P>,
}

impl ContentMask<Pixels> {
    /// Scale the content mask's pixel units by the given scaling factor.
    pub fn scale(&self, factor: f32) -> ContentMask<ScaledPixels> {
        ContentMask {
            bounds: self.bounds.scale(factor),
        }
    }

    /// Intersect the content mask with the given content mask.
    pub fn intersect(&self, other: &Self) -> Self {
        let bounds = self.bounds.intersect(&other.bounds);
        ContentMask { bounds }
    }
}

impl Window {
    fn mark_view_dirty(&mut self, view_id: EntityId) {
        // Mark ancestor views as dirty. If already in the `dirty_views` set, then all its ancestors
        // should already be dirty.
        for view_id in self
            .rendered_frame
            .dispatch_tree
            .view_path(view_id)
            .into_iter()
            .rev()
        {
            if !self.dirty_views.insert(view_id) {
                break;
            }
        }
    }

    /// Registers a callback to be invoked when the window appearance changes.
    pub fn observe_window_appearance(
        &self,
        mut callback: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let (subscription, activate) = self.appearance_observers.insert(
            (),
            Box::new(move |window, cx| {
                callback(window, cx);
                true
            }),
        );
        activate();
        subscription
    }

    /// Replaces the root entity of the window with a new one.
    pub fn replace_root<E>(
        &mut self,
        cx: &mut App,
        build_view: impl FnOnce(&mut Window, &mut Context<E>) -> E,
    ) -> Entity<E>
    where
        E: 'static + Render,
    {
        let view = cx.new(|cx| build_view(self, cx));
        self.root = Some(view.clone().into());
        self.refresh();
        view
    }

    /// Returns the root entity of the window, if it has one.
    pub fn root<E>(&self) -> Option<Option<Entity<E>>>
    where
        E: 'static + Render,
    {
        self.root
            .as_ref()
            .map(|view| view.clone().downcast::<E>().ok())
    }

    /// Obtain a handle to the window that belongs to this context.
    pub fn window_handle(&self) -> AnyWindowHandle {
        self.handle
    }

    /// Mark the window as dirty, scheduling it to be redrawn on the next frame.
    pub fn refresh(&mut self) {
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
        self.force_view_cache_refresh = true;
        self.invalidator.set_dirty(true);
        self.request_dirty_frame_if_needed();
    }

    pub(crate) fn request_dirty_frame_if_needed(&mut self) {
        let mut should_request_frame = false;
        if self.invalidator.not_drawing() {
            if self.refreshing {
                record_coalesced_refresh();
                self.ensure_frame_request_watchdog();
                log::debug!(
                    "gpui dirty frame coalesced: window={} dirty={} refreshing={}",
                    self.handle.window_id().as_u64(),
                    self.invalidator.is_dirty(),
                    self.refreshing
                );
            } else if self.should_defer_dirty_frame_request() {
                record_inactive_dirty_defer();
                log::debug!(
                    "gpui dirty frame deferred before platform request: window={} dirty={} active={} minimized={} pending_present={} retained_scene_len={}",
                    self.handle.window_id().as_u64(),
                    self.invalidator.is_dirty(),
                    self.active.get(),
                    self.platform_window.is_minimized(),
                    self.needs_present.get(),
                    self.rendered_frame.scene.len()
                );
            } else if self.frame_throttle.should_delay(Instant::now()) {
                record_coalesced_refresh();
                log::debug!(
                    "gpui dirty frame throttled: window={} dirty={} refreshing={}",
                    self.handle.window_id().as_u64(),
                    self.invalidator.is_dirty(),
                    self.refreshing
                );
                self.schedule_frame_throttle_retry();
            } else {
                self.refreshing = true;
                should_request_frame = true;
            }
        }
        if should_request_frame {
            log::debug!(
                "gpui dirty frame requested: window={} dirty={} refreshing={}",
                self.handle.window_id().as_u64(),
                self.invalidator.is_dirty(),
                self.refreshing
            );
            self.request_platform_frame(RequestFrameOptions::from_refresh());
        }
    }

    pub(crate) fn should_defer_dirty_frame_request(&self) -> bool {
        self.should_defer_dirty_frame_request_at(Instant::now())
    }

    fn should_defer_dirty_frame_request_at(&self, now: Instant) -> bool {
        self.invalidator.is_dirty()
            && !self.active.get()
            && !self.needs_present.get()
            && !self.recently_received_input(now)
            && self.next_frame_callbacks.borrow().is_empty()
            && self.rendered_frame.scene.len() != 0
    }

    fn recently_received_input(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.last_input_timestamp.get())
            <= RECENT_INPUT_DIRTY_FRAME_GRACE
    }

    fn delay_window_frame_requests(&mut self, duration: Duration, _cx: &mut App) {
        let now = Instant::now();
        self.frame_throttle.delay(now, duration);
        log::debug!(
            "gpui progressive frame retry armed: window={} delay={:?} active={} minimized={} dirty={}",
            self.handle.window_id().as_u64(),
            duration,
            self.active.get(),
            self.platform_window.is_minimized(),
            self.invalidator.is_dirty()
        );
        self.schedule_frame_throttle_retry();
    }

    fn progressive_frame_retry_delay(&self) -> Duration {
        if self.platform_window.is_minimized() {
            MINIMIZED_PROGRESSIVE_FRAME_RETRY
        } else if !self.active.get() {
            BACKGROUND_PROGRESSIVE_FRAME_RETRY
        } else {
            self.frame_throttle.retry_delay()
        }
    }

    fn schedule_frame_throttle_retry(&mut self) {
        let Some((retry_after, retry_generation)) = self.frame_throttle.arm_retry_timer() else {
            return;
        };

        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        let handle = self.handle;
        executor
            .spawn(async move {
                cx.background_executor()
                    .timer(retry_after.saturating_duration_since(Instant::now()))
                    .await;
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, _cx| {
                    if !window
                        .frame_throttle
                        .retry_timer_fired(retry_generation, Instant::now())
                    {
                        return;
                    }
                    if window.invalidator.is_dirty() && !window.refreshing {
                        window.refreshing = true;
                        window.request_platform_frame(RequestFrameOptions {
                            require_presentation: true,
                            force_render: true,
                            request_id: 0,
                        });
                    }
                }));
            })
            .detach();
    }

    fn request_platform_frame(&mut self, options: RequestFrameOptions) {
        let options = self.prepare_frame_request(options);
        self.arm_frame_request_watchdog(options);
        self.platform_window.request_frame(options);
    }

    fn ensure_frame_request_watchdog(&self) {
        let watchdog = self.frame_request_watchdog.get();
        if watchdog.pending {
            return;
        }
        let options = RequestFrameOptions {
            require_presentation: false,
            force_render: true,
            request_id: watchdog.generation,
        };
        self.arm_frame_request_watchdog(options);
    }

    fn prepare_frame_request(&self, mut options: RequestFrameOptions) -> RequestFrameOptions {
        let mut watchdog = self.frame_request_watchdog.get();
        if options.request_id == 0 {
            watchdog.generation = watchdog.generation.wrapping_add(1);
            options.request_id = watchdog.generation;
        } else {
            watchdog.generation = watchdog.generation.max(options.request_id);
        }
        self.frame_request_watchdog.set(watchdog);
        options
    }

    fn arm_frame_request_watchdog(&self, options: RequestFrameOptions) {
        if options.force_render || options.require_presentation {
            let mut watchdog = self.frame_request_watchdog.get();
            watchdog.pending = true;
            self.frame_request_watchdog.set(watchdog);
            self.spawn_frame_request_watchdog(options.request_id, options);
        }
    }

    fn handle_frame_request(&mut self, request_frame_options: RequestFrameOptions, cx: &mut App) {
        let frame_request_started_at = Instant::now();
        self.frame_throttle
            .record_frame_request(frame_request_started_at);
        let frame_budget = self.frame_throttle.frame_budget();

        let mut callbacks = self.next_frame_callbacks.take();
        let had_frame_callbacks = !callbacks.is_empty();
        for callback in callbacks.drain(..) {
            callback(self, cx);
        }

        let load = FrameRequestLoad {
            dirty: self.invalidator.is_dirty(),
            pending_present: self.needs_present.get(),
            active: self.active.get(),
            minimized: self.platform_window.is_minimized(),
        };
        let should_defer_inactive_dirty_draw = self.should_defer_inactive_dirty_draw(
            load,
            request_frame_options,
            had_frame_callbacks,
            frame_request_started_at,
        );
        let should_draw =
            !should_defer_inactive_dirty_draw && (load.dirty || request_frame_options.force_render);
        let should_degrade_to_present = should_draw
            && self.should_degrade_dirty_frame_to_retained_present(
                request_frame_options,
                frame_request_started_at,
            );
        let will_submit_visible_frame = should_draw
            || should_degrade_to_present
            || (!should_draw
                && (request_frame_options.require_presentation || load.pending_present));
        let should_present = should_degrade_to_present
            || (!should_draw
                && (request_frame_options.require_presentation || load.pending_present));
        let did_draw = should_draw && !should_degrade_to_present;
        let did_skip = !did_draw && !will_submit_visible_frame;

        record_frame_decision(did_draw, will_submit_visible_frame, did_skip);
        record_window_frame_result(
            self.handle.window_id().as_u64(),
            did_draw,
            will_submit_visible_frame,
            did_skip,
        );
        if log::log_enabled!(log::Level::Debug) {
            log::debug!(
                "gpui frame request: window={} request_id={} dirty={} force_render={} require_presentation={} pending_present={} active={} minimized={} draw={} present={} skip={} defer_inactive_dirty={}",
                self.handle.window_id().as_u64(),
                request_frame_options.request_id,
                load.dirty,
                request_frame_options.force_render,
                request_frame_options.require_presentation,
                load.pending_present,
                load.active,
                load.minimized,
                did_draw,
                will_submit_visible_frame,
                did_skip,
                should_defer_inactive_dirty_draw
            );
        }

        if should_degrade_to_present {
            record_draw_degrade();
            self.present_framebuffer_only();
            self.refreshing = false;
        } else if should_defer_inactive_dirty_draw {
            record_inactive_dirty_defer();
            self.refreshing = false;
            log::debug!(
                "gpui inactive dirty frame deferred: window={} request_id={} dirty={} force_render={} pending_present={} retained_scene_len={}",
                self.handle.window_id().as_u64(),
                request_frame_options.request_id,
                load.dirty,
                request_frame_options.force_render,
                load.pending_present,
                self.rendered_frame.scene.len()
            );
        } else if should_draw {
            let draw_started_at = Instant::now();
            measure("frame duration", || {
                let arena_clear_needed = self.draw(cx, frame_budget);
                if self.needs_present.get() {
                    self.present();
                }
                arena_clear_needed.clear();
            });
            let draw_elapsed = draw_started_at.elapsed();
            let draw_degraded = self.draw_degraded_this_frame;
            if draw_elapsed >= frame_budget {
                record_draw_budget_miss();
            }
            if draw_elapsed >= frame_budget && log::log_enabled!(log::Level::Warn) {
                log::warn!(
                    "gpui frame generation budget hit: window={} elapsed={:?} budget={:?} progressive_degraded={}",
                    self.handle.window_id().as_u64(),
                    draw_elapsed,
                    frame_budget,
                    draw_degraded
                );
            }
            if draw_degraded || draw_elapsed >= frame_budget {
                if !draw_degraded {
                    record_draw_degrade();
                }
                self.delay_window_frame_requests(self.progressive_frame_retry_delay(), cx);
            }
        } else if should_present {
            self.present_framebuffer_only();
        } else if load.active {
            record_retained_frame_skip();
        } else {
            record_inactive_present_skip();
        }

        self.complete_frame(if should_defer_inactive_dirty_draw {
            FrameCompletion::DeferredInactiveDirty
        } else {
            FrameCompletion::Normal
        });
        cx.warm_up_text_system_after_startup_frame();
    }

    fn should_defer_inactive_dirty_draw(
        &self,
        load: FrameRequestLoad,
        options: RequestFrameOptions,
        had_frame_callbacks: bool,
        now: Instant,
    ) -> bool {
        load.dirty
            && options.force_render
            && !options.require_presentation
            && !load.pending_present
            && !load.active
            && !had_frame_callbacks
            && !self.recently_received_input(now)
            && self.rendered_frame.scene.len() != 0
    }

    fn should_degrade_dirty_frame_to_retained_present(
        &self,
        options: RequestFrameOptions,
        now: Instant,
    ) -> bool {
        !options.force_render
            && !options.require_presentation
            && self.transparent_caption_height.is_none()
            && self.platform_window.background_appearance() == WindowBackgroundAppearance::Opaque
            && self.dirty_views.is_empty()
            && self.animation_dirty_region.is_empty()
            && self.frame_throttle.should_delay(now)
            && self.rendered_frame.scene.len() != 0
    }

    pub(crate) fn draw_budget_exhausted(&self) -> bool {
        if !self.allows_progressive_frame_degradation() {
            return false;
        }

        self.draw_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub(crate) fn draw_degraded_this_frame(&self) -> bool {
        self.draw_degraded_this_frame
    }

    pub(crate) fn draw_budget_exhausted_for_optional_work(&self) -> bool {
        self.critical_draw_depth == 0 && self.draw_budget_exhausted()
    }

    fn allows_progressive_frame_degradation(&self) -> bool {
        self.has_completed_rendered_frame
            && self.transparent_caption_height.is_none()
            && self.platform_window.background_appearance() == WindowBackgroundAppearance::Opaque
    }

    pub(crate) fn with_critical_draw<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        self.critical_draw_depth = self.critical_draw_depth.saturating_add(1);
        let result = f(self);
        self.critical_draw_depth = self.critical_draw_depth.saturating_sub(1);
        result
    }

    pub(crate) fn force_view_cache_refresh(&self) -> bool {
        self.force_view_cache_refresh
    }

    pub(crate) fn mark_animation_dirty(&mut self, bounds: Bounds<Pixels>) {
        self.invalidator.debug_assert_paint_or_prepaint();
        let clipped_bounds = bounds.intersect(&self.content_mask().bounds);
        if !clipped_bounds.is_empty() {
            self.animation_dirty_region
                .push(clipped_bounds.scale(self.scale_factor));
        }
    }

    pub(crate) fn degrade_current_draw(&mut self) {
        if !self.allows_progressive_frame_degradation() {
            return;
        }

        if !self.draw_degraded_this_frame {
            record_draw_degrade();
        }
        self.draw_degraded_this_frame = true;
    }

    fn spawn_frame_request_watchdog(&self, generation: u64, options: RequestFrameOptions) {
        let mut cx = self.async_app.clone();
        let executor = cx.foreground_executor().clone();
        let handle = self.handle;
        executor
            .spawn(async move {
                cx.background_executor()
                    .timer(FRAME_REQUEST_WATCHDOG_TIMEOUT)
                    .await;
                let _ = ignore_window_not_found(handle.update(&mut cx, |_, window, cx| {
                    window.recover_stalled_frame_request(generation, options, cx);
                }));
            })
            .detach();
    }

    fn recover_stalled_frame_request(
        &mut self,
        generation: u64,
        options: RequestFrameOptions,
        cx: &mut App,
    ) {
        let should_retry = {
            let mut watchdog = self.frame_request_watchdog.get();
            let pending = watchdog.pending
                && watchdog.generation == generation
                && options.request_id == generation;
            if pending {
                watchdog.pending = false;
            }
            self.frame_request_watchdog.set(watchdog);
            pending
        };
        if !should_retry {
            return;
        }

        if self.platform_window.is_native_move_active() {
            {
                let mut watchdog = self.frame_request_watchdog.get();
                watchdog.pending = true;
                self.frame_request_watchdog.set(watchdog);
            }
            log::debug!(
                "gpui frame request watchdog deferred during native move: window={} request_id={} generation={} timeout_ms={} dirty={} refreshing={} needs_present={}",
                self.handle.window_id().as_u64(),
                options.request_id,
                generation,
                FRAME_REQUEST_WATCHDOG_TIMEOUT.as_millis(),
                self.invalidator.is_dirty(),
                self.refreshing,
                self.needs_present.get()
            );
            return;
        }

        log::warn!(
            "gpui frame request watchdog fired: window={} request_id={} generation={} timeout_ms={} dirty={} refreshing={} needs_present={}",
            self.handle.window_id().as_u64(),
            options.request_id,
            generation,
            FRAME_REQUEST_WATCHDOG_TIMEOUT.as_millis(),
            self.invalidator.is_dirty(),
            self.refreshing,
            self.needs_present.get()
        );
        self.platform_window.frame_request_timed_out(options);
        self.delay_window_frame_requests(STALLED_WINDOW_FRAME_RETRY, cx);
        self.handle_frame_request(options, cx);
    }

    /// Close this window.
    pub fn remove_window(&mut self) {
        self.removed = true;
    }

    /// Obtain the currently focused [`FocusHandle`]. If no elements are focused, returns `None`.
    pub fn focused(&self, cx: &App) -> Option<FocusHandle> {
        self.focus
            .and_then(|id| FocusHandle::for_id(id, &cx.focus_handles))
    }

    /// Move focus to the element associated with the given [`FocusHandle`].
    pub fn focus(&mut self, handle: &FocusHandle) {
        if !self.focus_enabled || self.focus == Some(handle.id) {
            return;
        }

        self.focus = Some(handle.id);
        self.clear_pending_keystrokes();
        self.refresh();
    }

    /// Remove focus from all elements within this context's window.
    pub fn blur(&mut self) {
        if !self.focus_enabled {
            return;
        }

        self.focus = None;
        self.refresh();
    }

    /// Blur the window and don't allow anything in it to be focused again.
    pub fn disable_focus(&mut self) {
        self.blur();
        self.focus_enabled = false;
    }

    /// Move focus to next tab stop.
    pub fn focus_next(&mut self) {
        if !self.focus_enabled {
            return;
        }

        if let Some(handle) = self.rendered_frame.tab_stops.next(self.focus.as_ref()) {
            self.focus(&handle)
        }
    }

    /// Move focus to previous tab stop.
    pub fn focus_prev(&mut self) {
        if !self.focus_enabled {
            return;
        }

        if let Some(handle) = self.rendered_frame.tab_stops.prev(self.focus.as_ref()) {
            self.focus(&handle)
        }
    }

    /// Accessor for the text system.
    pub fn text_system(&self) -> &Arc<WindowTextSystem> {
        &self.text_system
    }

    /// The current text style. Which is composed of all the style refinements provided to `with_text_style`.
    pub fn text_style(&self) -> TextStyle {
        let mut style = self.default_text_style.clone();
        for refinement in &self.text_style_stack {
            style.refine(refinement);
        }
        style
    }

    /// Check if the platform window is maximized
    /// On some platforms (namely Windows) this is different than the bounds being the size of the display
    pub fn is_maximized(&self) -> bool {
        self.platform_window.is_maximized()
    }

    /// Returns whether the platform window is currently minimized.
    pub fn is_minimized(&self) -> bool {
        self.platform_window.is_minimized()
    }

    /// request a certain window decoration (Wayland)
    pub fn request_decorations(&self, decorations: WindowDecorations) {
        self.platform_window.request_decorations(decorations);
    }

    /// Start a window resize operation (Wayland)
    pub fn start_window_resize(&self, edge: ResizeEdge) {
        self.platform_window.start_window_resize(edge);
    }

    /// Return the `WindowBounds` to indicate that how a window should be opened
    /// after it has been closed
    pub fn window_bounds(&self) -> WindowBounds {
        self.platform_window.window_bounds()
    }

    /// Return the `WindowBounds` excluding insets (Wayland and X11)
    pub fn inner_window_bounds(&self) -> WindowBounds {
        self.platform_window.inner_window_bounds()
    }

    /// Dispatch the given action on the currently focused element.
    pub fn dispatch_action(&mut self, action: Box<dyn Action>, cx: &mut App) {
        let focus_id = self.focused(cx).map(|handle| handle.id);

        let window = self.handle;
        cx.defer(move |cx| {
            let _ = ignore_window_not_found(window.update(cx, |_, window, cx| {
                let node_id = window.focus_node_id_in_rendered_frame(focus_id);
                window.dispatch_action_on_node(node_id, action.as_ref(), cx);
            }));
        })
    }

    pub(crate) fn dispatch_keystroke_observers(
        &mut self,
        event: &dyn Any,
        action: Option<Box<dyn Action>>,
        context_stack: Vec<KeyContext>,
        cx: &mut App,
    ) {
        let Some(key_down_event) = event.downcast_ref::<KeyDownEvent>() else {
            return;
        };

        cx.keystroke_observers.clone().retain(&(), move |callback| {
            (callback)(
                &KeystrokeEvent {
                    keystroke: key_down_event.keystroke.clone(),
                    action: action.as_ref().map(|action| action.boxed_clone()),
                    context_stack: context_stack.clone(),
                },
                self,
                cx,
            )
        });
    }

    pub(crate) fn dispatch_keystroke_interceptors(
        &mut self,
        event: &dyn Any,
        context_stack: Vec<KeyContext>,
        cx: &mut App,
    ) {
        let Some(key_down_event) = event.downcast_ref::<KeyDownEvent>() else {
            return;
        };

        cx.keystroke_interceptors
            .clone()
            .retain(&(), move |callback| {
                (callback)(
                    &KeystrokeEvent {
                        keystroke: key_down_event.keystroke.clone(),
                        action: None,
                        context_stack: context_stack.clone(),
                    },
                    self,
                    cx,
                )
            });
    }

    /// Schedules the given function to be run at the end of the current effect cycle, allowing entities
    /// that are currently on the stack to be returned to the app.
    pub fn defer(&self, cx: &mut App, f: impl FnOnce(&mut Window, &mut App) + 'static) {
        let handle = self.handle;
        cx.defer(move |cx| {
            handle.update(cx, |_, window, cx| f(window, cx)).ok();
        });
    }

    /// Subscribe to events emitted by a entity.
    /// The entity to which you're subscribing must implement the [`EventEmitter`] trait.
    /// The callback will be invoked a handle to the emitting entity, the event, and a window context for the current window.
    pub fn observe<T: 'static>(
        &mut self,
        observed: &Entity<T>,
        cx: &mut App,
        mut on_notify: impl FnMut(Entity<T>, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let entity_id = observed.entity_id();
        let observed = observed.downgrade();
        let window_handle = self.handle;
        cx.new_observer(
            entity_id,
            Box::new(move |cx| {
                window_handle
                    .update(cx, |_, window, cx| {
                        if let Some(handle) = observed.upgrade() {
                            on_notify(handle, window, cx);
                            true
                        } else {
                            false
                        }
                    })
                    .unwrap_or(false)
            }),
        )
    }

    /// Subscribe to events emitted by a entity.
    /// The entity to which you're subscribing must implement the [`EventEmitter`] trait.
    /// The callback will be invoked a handle to the emitting entity, the event, and a window context for the current window.
    pub fn subscribe<Emitter, Evt>(
        &mut self,
        entity: &Entity<Emitter>,
        cx: &mut App,
        mut on_event: impl FnMut(Entity<Emitter>, &Evt, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        Emitter: EventEmitter<Evt>,
        Evt: 'static,
    {
        let entity_id = entity.entity_id();
        let handle = entity.downgrade();
        let window_handle = self.handle;
        cx.new_subscription(
            entity_id,
            (
                TypeId::of::<Evt>(),
                Box::new(move |event, cx| {
                    window_handle
                        .update(cx, |_, window, cx| {
                            if let Some(entity) = handle.upgrade() {
                                let event = event.downcast_ref().expect("invalid event type");
                                on_event(entity, event, window, cx);
                                true
                            } else {
                                false
                            }
                        })
                        .unwrap_or(false)
                }),
            ),
        )
    }

    /// Register a callback to be invoked when the given `Entity` is released.
    pub fn observe_release<T>(
        &self,
        entity: &Entity<T>,
        cx: &mut App,
        mut on_release: impl FnOnce(&mut T, &mut Window, &mut App) + 'static,
    ) -> Subscription
    where
        T: 'static,
    {
        let entity_id = entity.entity_id();
        let window_handle = self.handle;
        let (subscription, activate) = cx.release_listeners.insert(
            entity_id,
            Box::new(move |entity, cx| {
                let entity = entity.downcast_mut().expect("invalid entity type");
                let _ = window_handle.update(cx, |_, window, cx| on_release(entity, window, cx));
            }),
        );
        activate();
        subscription
    }

    /// Creates an [`AsyncWindowContext`], which has a static lifetime and can be held across
    /// await points in async code.
    pub fn to_async(&self, cx: &App) -> AsyncWindowContext {
        AsyncWindowContext::new_context(cx.to_async(), self.handle)
    }

    /// Schedule the given closure to be run directly after the current frame is rendered.
    pub fn on_next_frame(&self, callback: impl FnOnce(&mut Window, &mut App) + 'static) {
        let should_request_frame = {
            let mut next_frame_callbacks = self.next_frame_callbacks.borrow_mut();
            let should_request_frame = next_frame_callbacks.is_empty();
            next_frame_callbacks.push(Box::new(callback));
            should_request_frame
        };
        if should_request_frame {
            let options = self.prepare_frame_request(RequestFrameOptions {
                require_presentation: true,
                force_render: false,
                request_id: 0,
            });
            self.arm_frame_request_watchdog(options);
            self.platform_window.request_frame(options);
        }
    }

    /// Schedule a frame to be drawn on the next animation frame.
    ///
    /// This is useful for elements that need to animate continuously, such as a video player or an animated GIF.
    /// It will cause the window to redraw on the next frame, even if no other changes have occurred.
    ///
    /// If called from within a view, it will notify that view on the next frame. Otherwise, it will refresh the entire window.
    pub fn request_animation_frame(&self) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };
        if !self
            .animation_frame_pending_entities
            .borrow_mut()
            .insert(entity)
        {
            record_coalesced_refresh();
            return;
        }

        let pending_entities = self.animation_frame_pending_entities.clone();
        if self.active.get() {
            RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(move |_, cx| {
                pending_entities.borrow_mut().remove(&entity);
                cx.notify(entity);
            }));

            let options = self.prepare_frame_request(RequestFrameOptions {
                require_presentation: true,
                force_render: true,
                request_id: 0,
            });
            self.arm_frame_request_watchdog(options);
            self.platform_window.request_frame(options);
        } else if !self.inactive_animation_frame_pending.replace(true) {
            RefCell::borrow_mut(&self.next_frame_callbacks).push(Box::new(move |window, cx| {
                window.inactive_animation_frame_pending.set(false);
                pending_entities.borrow_mut().remove(&entity);
                cx.notify(entity);
            }));
        } else {
            self.animation_frame_pending_entities
                .borrow_mut()
                .remove(&entity);
        }
    }

    /// Notify the current view at or after the given deadline without requesting
    /// continuous animation frames or an immediate presentation.
    ///
    /// Use this for UI that is otherwise static but has time-based visibility,
    /// such as toast expiry or low-FPS status indicators. Unlike
    /// [`Window::request_animation_frame`], this schedules a single dirty-frame
    /// invalidation and coalesces later requests for the same view. If there is
    /// no currently rendering view, the root view is used.
    pub fn request_invalidation_at(&self, deadline: Instant, cx: &App) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };
        self.request_invalidation_for(entity, deadline, cx);
    }

    /// Notify a specific view at or after the given deadline.
    pub fn request_invalidation_for(&self, entity: EntityId, deadline: Instant, cx: &App) {
        let existing = self
            .deadline_invalidation_pending
            .borrow()
            .get(&entity)
            .copied();
        if existing.is_some_and(|(pending_deadline, _)| pending_deadline <= deadline) {
            return;
        }

        let generation = self.deadline_invalidation_generation.get().wrapping_add(1);
        self.deadline_invalidation_generation.set(generation);
        self.deadline_invalidation_pending
            .borrow_mut()
            .insert(entity, (deadline, generation));

        let pending = self.deadline_invalidation_pending.clone();
        let handle = self.handle;
        let delay = deadline.saturating_duration_since(Instant::now());
        self.spawn(cx, async move |cx| {
            cx.background_executor().timer(delay).await;
            if pending.borrow().get(&entity).copied() != Some((deadline, generation)) {
                return;
            }

            pending.borrow_mut().remove(&entity);
            let _ = ignore_window_not_found(handle.update(cx, |_, _window, cx| {
                cx.notify(entity);
            }));
        })
        .detach();
    }

    pub(crate) fn request_animation_frame_for_image(
        &self,
        cx: &App,
        animation_config: crate::AnimatedImageConfig,
    ) {
        let Some(entity) = self.current_view_or_root() else {
            return;
        };
        let minimum_frame_duration = if self.active.get() {
            animation_config.minimum_frame_duration()
        } else {
            animation_config.inactive_minimum_frame_duration()
        };
        let now = Instant::now();
        let remaining = match self.last_inactive_animation_frame.get() {
            Some(last_frame) => minimum_frame_duration
                .checked_sub(now.saturating_duration_since(last_frame))
                .unwrap_or_default(),
            None => Duration::ZERO,
        };

        if remaining.is_zero() {
            self.last_inactive_animation_frame.set(Some(now));
            self.request_animation_frame();
            return;
        }

        if self.inactive_animation_frame_pending.replace(true) {
            return;
        }

        let pending = self.inactive_animation_frame_pending.clone();
        let last_frame = self.last_inactive_animation_frame.clone();
        let handle = self.handle;
        self.spawn(cx, async move |cx| {
            cx.background_executor().timer(remaining).await;
            pending.set(false);
            last_frame.set(Some(Instant::now()));
            let _ = ignore_window_not_found(handle.update(cx, |_, _window, cx| {
                cx.notify(entity);
            }));
        })
        .detach();
    }

    /// Spawn the future returned by the given closure on the application thread pool.
    /// The closure is provided a handle to the current window and an `AsyncWindowContext` for
    /// use within your future.
    #[track_caller]
    pub fn spawn<AsyncFn, R>(&self, cx: &App, f: AsyncFn) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
    {
        let handle = self.handle;
        cx.spawn(async move |app| {
            let mut async_window_cx = AsyncWindowContext::new_context(app.clone(), handle);
            f(&mut async_window_cx).await
        })
    }

    /// Spawn the future returned by the given closure on the application thread
    /// pool, with the given priority.
    #[track_caller]
    pub fn spawn_with_priority<AsyncFn, R>(
        &self,
        priority: Priority,
        cx: &App,
        f: AsyncFn,
    ) -> Task<R>
    where
        R: 'static,
        AsyncFn: AsyncFnOnce(&mut AsyncWindowContext) -> R + 'static,
    {
        let handle = self.handle;
        cx.spawn_with_priority(priority, async move |app| {
            let mut async_window_cx = AsyncWindowContext::new_context(app.clone(), handle);
            f(&mut async_window_cx).await
        })
    }

    fn window_origin_changed(&mut self, cx: &mut App) {
        let scale_factor = self.platform_window.scale_factor();
        let viewport_size = self.platform_window.content_size();
        let display_id = self.platform_window.display().map(|display| display.id());

        if self.scale_factor == scale_factor && self.display_id == display_id {
            return;
        }

        if self.viewport_size != viewport_size {
            self.content_bounds_changed(cx);
            return;
        }

        self.scale_factor = scale_factor;
        self.display_id = display_id;

        self.refresh();

        self.bounds_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    fn content_bounds_changed(&mut self, cx: &mut App) {
        let scale_factor = self.platform_window.scale_factor();
        let viewport_size = self.platform_window.content_size();
        let display_id = self.platform_window.display().map(|display| display.id());

        let text_rasterization_changed = self.scale_factor != scale_factor
            || self.viewport_size != viewport_size
            || self.display_id != display_id;

        if self.scale_factor == scale_factor
            && self.viewport_size == viewport_size
            && self.display_id == display_id
        {
            return;
        }

        self.scale_factor = scale_factor;
        self.viewport_size = viewport_size;
        self.display_id = display_id;
        if text_rasterization_changed {
            self.text_system.clear_layout_cache();
            self.text_system.clear_raster_cache();
            self.sprite_atlas.clear_glyphs();
        }
        self.force_full_redraw.set(true);

        self.refresh();

        self.bounds_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    /// Returns the bounds of the current window in the global coordinate space, which could span across multiple displays.
    pub fn bounds(&self) -> Bounds<Pixels> {
        self.platform_window.bounds()
    }

    /// Set the content size of the window.
    pub fn resize(&mut self, size: Size<Pixels>) {
        self.platform_window.resize(size);
    }

    /// Returns whether or not the window is currently fullscreen
    pub fn is_fullscreen(&self) -> bool {
        self.platform_window.is_fullscreen()
    }

    pub(crate) fn appearance_changed(&mut self, cx: &mut App) {
        self.appearance = self.platform_window.appearance();
        self.refresh();

        self.appearance_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    /// Returns the appearance of the current window.
    pub fn appearance(&self) -> WindowAppearance {
        self.appearance
    }

    /// Returns the size of the drawable area within the window.
    pub fn viewport_size(&self) -> Size<Pixels> {
        self.viewport_size
    }

    /// Returns whether this window is focused by the operating system (receiving key events).
    pub fn is_window_active(&self) -> bool {
        self.active.get()
    }

    /// Returns whether this window is considered to be the window
    /// that currently owns the mouse cursor.
    /// On mac, this is equivalent to `is_window_active`.
    pub fn is_window_hovered(&self) -> bool {
        if cfg!(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )) {
            self.hovered.get()
        } else {
            self.is_window_active()
        }
    }

    /// Toggle zoom on the window.
    pub fn zoom_window(&self) {
        self.platform_window.zoom();
    }

    /// Opens the native title bar context menu, useful when implementing client side decorations (Wayland and X11)
    pub fn show_window_menu(&self, position: Point<Pixels>) {
        self.platform_window.show_window_menu(position)
    }

    /// Tells the compositor to take control of window movement (Wayland and X11)
    ///
    /// Events may not be received during a move operation.
    pub fn start_window_move(&self) {
        self.platform_window.start_window_move()
    }

    /// When using client side decorations, set this to the width of the invisible decorations (Wayland and X11)
    pub fn set_client_inset(&mut self, inset: Pixels) {
        self.client_inset = Some(inset);
        self.platform_window.set_client_inset(inset);
    }

    /// Returns the client_inset value by [`Self::set_client_inset`].
    pub fn client_inset(&self) -> Option<Pixels> {
        self.client_inset
    }

    /// Returns whether the title bar window controls need to be rendered by the application (Wayland and X11)
    pub fn window_decorations(&self) -> Decorations {
        self.platform_window.window_decorations()
    }

    /// Returns which window controls are currently visible (Wayland)
    pub fn window_controls(&self) -> WindowControls {
        self.platform_window.window_controls()
    }

    /// Updates the window's title at the platform level.
    pub fn set_window_title(&mut self, title: &str) {
        self.platform_window.set_title(title);
    }

    /// Sets the application identifier.
    pub fn set_app_id(&mut self, app_id: &str) {
        self.platform_window.set_app_id(app_id);
    }

    /// Sets the window background appearance.
    pub fn set_background_appearance(&self, background_appearance: WindowBackgroundAppearance) {
        self.platform_window
            .set_background_appearance(background_appearance);
    }

    /// Mark the window as dirty at the platform level.
    pub fn set_window_edited(&mut self, edited: bool) {
        self.platform_window.set_edited(edited);
    }

    /// Determine the display on which the window is visible.
    pub fn display(&self, cx: &App) -> Option<Rc<dyn PlatformDisplay>> {
        cx.platform
            .displays()
            .into_iter()
            .find(|display| Some(display.id()) == self.display_id)
    }

    /// Show the platform character palette.
    pub fn show_character_palette(&self) {
        self.platform_window.show_character_palette();
    }

    /// The scale factor of the display associated with the window. For example, it could
    /// return 2.0 for a "retina" display, indicating that each logical pixel should actually
    /// be rendered as two pixels on screen.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor
    }

    /// The size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn rem_size(&self) -> Pixels {
        self.rem_size_override_stack
            .last()
            .copied()
            .unwrap_or(self.rem_size)
    }

    /// Sets the size of an em for the base font of the application. Adjusting this value allows the
    /// UI to scale, just like zooming a web page.
    pub fn set_rem_size(&mut self, rem_size: impl Into<Pixels>) {
        self.rem_size = rem_size.into();
    }

    pub(crate) fn set_default_text_style(&mut self, text_style: TextStyle) {
        self.default_text_style = text_style;
        self.text_system.clear_layout_cache();
        self.refresh();
    }

    /// Acquire a globally unique identifier for the given ElementId.
    /// Only valid for the duration of the provided closure.
    pub fn with_global_id<R>(
        &mut self,
        element_id: ElementId,
        f: impl FnOnce(&GlobalElementId, &mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id);
        let global_id = GlobalElementId(self.element_id_stack.clone());
        let result = f(&global_id, self);
        self.element_id_stack.pop();
        result
    }

    /// Executes the provided function with the specified rem size.
    ///
    /// This method must only be called as part of element drawing.
    pub fn with_rem_size<F, R>(&mut self, rem_size: Option<impl Into<Pixels>>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        if let Some(rem_size) = rem_size {
            self.rem_size_override_stack.push(rem_size.into());
            let result = f(self);
            self.rem_size_override_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// The line height associated with the current text style.
    pub fn line_height(&self) -> Pixels {
        self.text_style().line_height_in_pixels(self.rem_size())
    }

    /// Call to prevent the default action of an event. Currently only used to prevent
    /// parent elements from becoming focused on mouse down.
    pub fn prevent_default(&mut self) {
        self.default_prevented = true;
    }

    /// Obtain whether default has been prevented for the event currently being dispatched.
    pub fn default_prevented(&self) -> bool {
        self.default_prevented
    }

    /// Determine whether the given action is available along the dispatch path to the currently focused element.
    pub fn is_action_available(&self, action: &dyn Action, cx: &mut App) -> bool {
        let node_id =
            self.focus_node_id_in_rendered_frame(self.focused(cx).map(|handle| handle.id));
        self.rendered_frame
            .dispatch_tree
            .is_action_available(action, node_id)
    }

    /// The position of the mouse relative to the window.
    pub fn mouse_position(&self) -> Point<Pixels> {
        self.mouse_position
    }

    /// The current state of the keyboard's modifiers
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// The current state of the keyboard's capslock
    pub fn capslock(&self) -> Capslock {
        self.capslock
    }

    fn complete_frame(&mut self, completion: FrameCompletion) {
        let was_dirty = self.invalidator.is_dirty();
        if self.invalidator.is_dirty() {
            self.idle_render_frames = 0;
            self.render_trim_policy = RetainedResourceTrimPolicy::None;
            if completion == FrameCompletion::Normal {
                self.request_dirty_frame_if_needed();
            }
        } else {
            self.idle_render_frames = self.idle_render_frames.saturating_add(1);
            self.render_trim_policy = if self.idle_render_frames >= WINDOW_STRONG_TRIM_IDLE_FRAMES {
                RetainedResourceTrimPolicy::Strong
            } else if self.idle_render_frames >= WINDOW_LIGHT_TRIM_IDLE_FRAMES {
                RetainedResourceTrimPolicy::Light
            } else {
                RetainedResourceTrimPolicy::None
            };
        }
        self.platform_window.completed_frame();
        log::debug!(
            "gpui complete_frame: window={} was_dirty={} refreshing={} idle_render_frames={} needs_present={} trim_policy={:?} completion={:?}",
            self.handle.window_id().as_u64(),
            was_dirty,
            self.refreshing,
            self.idle_render_frames,
            self.needs_present.get(),
            self.render_trim_policy,
            completion
        );
    }

    /// Produces a new frame and assigns it to `rendered_frame`. To actually show
    /// the contents of the new [`Scene`], use [`Self::present`].
    #[profiling::function]
    pub fn draw(&mut self, cx: &mut App, frame_budget: Duration) -> ArenaClearNeeded {
        let previous_scene_was_empty = self.rendered_frame.scene.len() == 0;
        let force_full_redraw = self.force_full_redraw.get();
        self.draw_deadline = Some(Instant::now() + frame_budget);
        self.draw_degraded_this_frame = false;
        record_window_layout_recompute(self.handle.window_id().as_u64());
        self.invalidate_entities();
        cx.entities.clear_accessed();
        debug_assert!(self.rendered_entity_stack.is_empty());
        self.invalidator.set_dirty(false);
        self.requested_autoscroll = None;

        // Restore the previously-used input handler.
        let restored_input_handler_index =
            if let Some(input_handler) = self.platform_window.take_input_handler() {
                if let Some((index, slot)) = self
                    .rendered_frame
                    .input_handlers
                    .iter_mut()
                    .enumerate()
                    .rev()
                    .find(|(_, handler)| handler.is_none())
                {
                    *slot = Some(input_handler);
                    Some(index)
                } else {
                    let index = self.rendered_frame.input_handlers.len();
                    self.rendered_frame.input_handlers.push(Some(input_handler));
                    Some(index)
                }
            } else {
                None
            };
        self.draw_roots(cx);
        self.next_frame.window_active = self.active.get();

        if self.draw_degraded_this_frame && !previous_scene_was_empty {
            self.restore_input_handler_after_degraded_draw(restored_input_handler_index);
            self.rendered_frame.element_states.extend(self.next_frame.element_states.drain());
            if let Some(layout_engine) = self.layout_engine.as_mut() {
                let (layout_cache_hits, layout_cache_misses) = layout_engine.layout_cache_metrics();
                let metrics = layout_engine.finish_frame();
                record_layout_frame_metrics(metrics);
                record_layout_cache_metrics(layout_cache_hits, layout_cache_misses);
            }
            self.text_system().finish_frame();
            self.next_frame.clear();
            self.invalidator.set_dirty(true);
            self.refreshing = false;
            self.invalidator.set_phase(DrawPhase::None);
            self.force_full_redraw.set(true);
            self.draw_deadline = None;
            return ArenaClearNeeded;
        }

        // Register requested input handler with the platform window.
        if let Some(input_handler) = self
            .next_frame
            .input_handlers
            .iter_mut()
            .rev()
            .find_map(|handler| handler.take())
        {
            self.platform_window.set_input_handler(input_handler);
        }

        if let Some(layout_engine) = self.layout_engine.as_mut() {
            let (layout_cache_hits, layout_cache_misses) = layout_engine.layout_cache_metrics();
            let metrics = layout_engine.finish_frame();
            record_layout_frame_metrics(metrics);
            record_layout_cache_metrics(layout_cache_hits, layout_cache_misses);
        }
        self.text_system().finish_frame();
        self.next_frame.finish(&mut self.rendered_frame);
        self.prepare_render_plan_for_next_frame(
            previous_scene_was_empty || force_full_redraw || self.draw_degraded_this_frame,
        );
        record_frame_retained_capacity(self.next_frame.retained_capacity());
        record_scene_frame_metrics(self.next_frame.scene.frame_metrics());

        self.invalidator.set_phase(DrawPhase::Focus);
        let previous_focus_path = self.rendered_frame.focus_path();
        let previous_window_active = self.rendered_frame.window_active;
        mem::swap(&mut self.rendered_frame, &mut self.next_frame);
        self.next_frame.clear();
        self.sync_platform_window_control_areas();
        let current_focus_path = self.rendered_frame.focus_path();
        let current_window_active = self.rendered_frame.window_active;

        if previous_focus_path != current_focus_path
            || previous_window_active != current_window_active
        {
            if !previous_focus_path.is_empty() && current_focus_path.is_empty() {
                self.focus_lost_listeners
                    .clone()
                    .retain(&(), |listener| listener(self, cx));
            }

            let event = WindowFocusEvent {
                previous_focus_path: if previous_window_active {
                    previous_focus_path
                } else {
                    Default::default()
                },
                current_focus_path: if current_window_active {
                    current_focus_path
                } else {
                    Default::default()
                },
            };
            self.focus_listeners
                .clone()
                .retain(&(), |listener| listener(&event, self, cx));
        }

        debug_assert!(self.rendered_entity_stack.is_empty());
        self.record_entities_accessed(cx);
        self.reset_cursor_style(cx);
        self.refreshing = false;
        self.invalidator.set_phase(DrawPhase::None);
        self.force_full_redraw.set(false);
        self.force_view_cache_refresh = false;
        self.has_completed_rendered_frame = true;
        self.needs_present.set(true);
        if self.draw_degraded_this_frame {
            self.invalidator.set_dirty(true);
        } else {
            self.dirty_views.clear();
            self.animation_dirty_region = DirtyRegion::empty();
        }
        self.draw_deadline = None;

        ArenaClearNeeded
    }

    fn restore_input_handler_after_degraded_draw(&mut self, restored_index: Option<usize>) {
        let restored_input_handler = restored_index
            .and_then(|index| self.rendered_frame.input_handlers.get_mut(index))
            .and_then(Option::take)
            .or_else(|| {
                self.next_frame
                    .input_handlers
                    .iter_mut()
                    .rev()
                    .find_map(|handler| handler.take())
            });

        if let Some(input_handler) = restored_input_handler {
            self.platform_window.set_input_handler(input_handler);
        }
    }

    fn sync_platform_window_control_areas(&self) {
        let caption_bounds = self.transparent_caption_height.map(|height| {
            Bounds::new(
                Point::default(),
                size(
                    self.viewport_size.width,
                    height.min(self.viewport_size.height),
                ),
            )
        });
        let areas = self
            .rendered_frame
            .hitboxes
            .iter()
            .filter_map(|hitbox| {
                let area = self.rendered_frame.window_control_hitboxes.iter().find_map(
                    |(area, control_hitbox)| (control_hitbox.id == hitbox.id).then_some(*area),
                );

                if area.is_some()
                    || caption_bounds
                        .as_ref()
                        .is_some_and(|caption_bounds| hitbox.bounds.intersects(caption_bounds))
                {
                    Some(crate::WindowControlAreaBounds {
                        area,
                        bounds: hitbox.bounds,
                        blocks_behind: hitbox.behavior == HitboxBehavior::BlockMouse,
                    })
                } else {
                    None
                }
            })
            .collect();

        self.platform_window
            .set_window_control_areas(areas, self.transparent_caption_height);
    }

    fn prepare_render_plan_for_next_frame(&mut self, force_full_redraw: bool) {
        let viewport = Bounds::new(Point::default(), self.viewport_size).scale(self.scale_factor);
        let mut dirty_region = DirtyRegion::empty();

        let requires_full_redraw = force_full_redraw
            || self.next_frame.scene.requires_full_redraw_fallback()
            || self.platform_window.background_appearance() != WindowBackgroundAppearance::Opaque;

        if requires_full_redraw {
            dirty_region.mark_full(viewport);
        } else {
            for segment in &self.next_frame.retained_scene_segments {
                if segment.dirty {
                    dirty_region.push(segment.bounds);
                }
            }
            for rect in self.animation_dirty_region.rects() {
                dirty_region.push(rect.bounds);
            }

            if dirty_region.is_empty() && self.next_frame.retained_scene_segments.is_empty() {
                dirty_region.mark_full(viewport);
            } else if !dirty_region.is_empty() {
                dirty_region.coalesce_if_large(viewport, DIRTY_REGION_FULL_REDRAW_RATIO);
            }
        }

        self.render_present_mode = if dirty_region.is_full()
            || (dirty_region.is_empty() && self.next_frame.retained_scene_segments.is_empty())
        {
            PartialPresentMode::FullRedraw
        } else {
            PartialPresentMode::Partial
        };
        record_dirty_region_metrics(dirty_region.rect_count(), dirty_region.area() as usize);
        self.render_dirty_region = dirty_region;
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
    }

    fn render_plan(&self) -> FrameRenderPlan<'_> {
        FrameRenderPlan {
            scene: &self.rendered_frame.scene,
            dirty_region: &self.render_dirty_region,
            partial_present_mode: self.render_present_mode,
            trim_policy: self.render_trim_policy,
        }
    }

    fn record_entities_accessed(&mut self, cx: &mut App) {
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        let mut entities = mem::take(entities_ref.deref_mut());
        drop(entities_ref);
        let handle = self.handle;
        cx.record_entities_accessed(
            handle,
            // Try moving window invalidator into the Window
            self.invalidator.clone(),
            &entities,
        );
        let mut entities_ref = cx.entities.accessed_entities.borrow_mut();
        mem::swap(&mut entities, entities_ref.deref_mut());
    }

    fn invalidate_entities(&mut self) {
        let mut views = self.invalidator.take_views();
        for entity in views.drain() {
            self.mark_view_dirty(entity);
        }
        self.invalidator.replace_views(views);
    }

    #[profiling::function]
    fn present(&self) {
        self.platform_window.draw(self.render_plan());
        self.needs_present.set(false);
        profiling::finish_frame!();
    }

    fn present_framebuffer_only(&self) {
        self.platform_window
            .present_framebuffer_only(self.render_plan());
        self.needs_present.set(false);
        profiling::finish_frame!();
    }

    fn draw_roots(&mut self, cx: &mut App) {
        self.invalidator.set_phase(DrawPhase::Prepaint);
        self.tooltip_bounds.take();

        let _inspector_width: Pixels = rems(30.0).to_pixels(self.rem_size());
        let root_size = {
            #[cfg(any(feature = "inspector", debug_assertions))]
            {
                self.viewport_size
            }
            #[cfg(not(any(feature = "inspector", debug_assertions)))]
            {
                self.viewport_size
            }
        };

        // Layout all root elements.
        let mut root_element = self.root.as_ref().unwrap().clone().into_any();
        self.with_critical_draw(|window| {
            root_element.prepaint_as_root(Point::default(), root_size.into(), window, cx);
        });

        #[cfg(any(feature = "inspector", debug_assertions))]
        let inspector_element = if self.inspector.is_some() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
            None
        } else {
            self.prepaint_inspector(_inspector_width, cx)
        };

        let mut sorted_deferred_draws =
            (0..self.next_frame.deferred_draws.len()).collect::<SmallVec<[_; 8]>>();
        sorted_deferred_draws.sort_by_key(|ix| self.next_frame.deferred_draws[*ix].priority);
        if !sorted_deferred_draws.is_empty() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
            sorted_deferred_draws.clear();
        } else {
            self.prepaint_deferred_draws(&sorted_deferred_draws, cx);
        }

        let mut prompt_element = None;
        let mut active_drag_element = None;
        let mut tooltip_element = None;
        let has_overlay_work = self.prompt.is_some()
            || cx.active_drag.is_some()
            || !self.next_frame.tooltip_requests.is_empty();
        if has_overlay_work && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else if let Some(prompt) = self.prompt.take() {
            let mut element = prompt.view.any_view().into_any();
            element.prepaint_as_root(Point::default(), root_size.into(), self, cx);
            prompt_element = Some(element);
            self.prompt = Some(prompt);
        } else if let Some(active_drag) = cx.active_drag.take() {
            let mut element = active_drag.view.clone().into_any();
            let offset = self.mouse_position() - active_drag.cursor_offset;
            element.prepaint_as_root(offset, AvailableSpace::min_size(), self, cx);
            active_drag_element = Some(element);
            cx.active_drag = Some(active_drag);
        } else {
            tooltip_element = self.prepaint_tooltip(cx);
        }

        self.mouse_hit_test = self.next_frame.hit_test(self.mouse_position);

        // Now actually paint the elements.
        self.invalidator.set_phase(DrawPhase::Paint);
        self.with_critical_draw(|window| {
            root_element.paint(window, cx);
        });

        #[cfg(any(feature = "inspector", debug_assertions))]
        if inspector_element.is_some() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else {
            self.paint_inspector(inspector_element, cx);
        }

        if !sorted_deferred_draws.is_empty() && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else {
            self.paint_deferred_draws(&sorted_deferred_draws, cx);
        }

        let has_overlay_element = prompt_element.is_some()
            || active_drag_element.is_some()
            || tooltip_element.is_some();
        if has_overlay_element && self.draw_budget_exhausted() {
            self.degrade_current_draw();
        } else if let Some(mut prompt_element) = prompt_element {
            prompt_element.paint(self, cx);
        } else if let Some(mut drag_element) = active_drag_element {
            drag_element.paint(self, cx);
        } else if let Some(mut tooltip_element) = tooltip_element {
            tooltip_element.paint(self, cx);
        }

        #[cfg(any(feature = "inspector", debug_assertions))]
        self.paint_inspector_hitbox(cx);
    }

    fn prepaint_tooltip(&mut self, cx: &mut App) -> Option<AnyElement> {
        // Use indexing instead of iteration to avoid borrowing self for the duration of the loop.
        for tooltip_request_index in (0..self.next_frame.tooltip_requests.len()).rev() {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let Some(Some(tooltip_request)) = self
                .next_frame
                .tooltip_requests
                .get(tooltip_request_index)
                .cloned()
            else {
                log::error!("Unexpectedly absent TooltipRequest");
                continue;
            };
            let mut element = tooltip_request.tooltip.view.clone().into_any();
            let mouse_position = tooltip_request.tooltip.mouse_position;
            let tooltip_size = element.layout_as_root(AvailableSpace::min_size(), self, cx);

            let mut tooltip_bounds =
                Bounds::new(mouse_position + point(px(1.), px(1.)), tooltip_size);
            let window_bounds = Bounds {
                origin: Point::default(),
                size: self.viewport_size(),
            };

            if tooltip_bounds.right() > window_bounds.right() {
                let new_x = mouse_position.x - tooltip_bounds.size.width - px(1.);
                if new_x >= Pixels::ZERO {
                    tooltip_bounds.origin.x = new_x;
                } else {
                    tooltip_bounds.origin.x = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.x - tooltip_bounds.right() - window_bounds.right(),
                    );
                }
            }

            if tooltip_bounds.bottom() > window_bounds.bottom() {
                let new_y = mouse_position.y - tooltip_bounds.size.height - px(1.);
                if new_y >= Pixels::ZERO {
                    tooltip_bounds.origin.y = new_y;
                } else {
                    tooltip_bounds.origin.y = cmp::max(
                        Pixels::ZERO,
                        tooltip_bounds.origin.y - tooltip_bounds.bottom() - window_bounds.bottom(),
                    );
                }
            }

            // It's possible for an element to have an active tooltip while not being painted (e.g.
            // via the `visible_on_hover` method). Since mouse listeners are not active in this
            // case, instead update the tooltip's visibility here.
            let is_visible =
                (tooltip_request.tooltip.check_visible_and_update)(tooltip_bounds, self, cx);
            if !is_visible {
                continue;
            }

            self.with_absolute_element_offset(tooltip_bounds.origin, |window| {
                element.prepaint(window, cx)
            });

            self.tooltip_bounds = Some(TooltipBounds {
                id: tooltip_request.id,
                bounds: tooltip_bounds,
            });
            return Some(element);
        }
        None
    }

    fn prepaint_deferred_draws(&mut self, deferred_draw_indices: &[usize], cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.text_style_stack
                .clone_from(&deferred_draw.text_style_stack);
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let prepaint_start = self.prepaint_index();
            if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(deferred_draw.current_view, |window| {
                    window.with_absolute_element_offset(deferred_draw.absolute_offset, |window| {
                        element.prepaint(window, cx)
                    });
                })
            } else {
                self.reuse_prepaint(deferred_draw.prepaint_range.clone());
            }
            let prepaint_end = self.prepaint_index();
            deferred_draw.prepaint_range = prepaint_start..prepaint_end;
        }
        assert_eq!(
            self.next_frame.deferred_draws.len(),
            0,
            "cannot call defer_draw during deferred drawing"
        );
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
        self.text_style_stack.clear();
    }

    fn paint_deferred_draws(&mut self, deferred_draw_indices: &[usize], cx: &mut App) {
        assert_eq!(self.element_id_stack.len(), 0);

        let mut deferred_draws = mem::take(&mut self.next_frame.deferred_draws);
        for deferred_draw_ix in deferred_draw_indices {
            if self.draw_budget_exhausted() {
                self.degrade_current_draw();
                break;
            }

            let mut deferred_draw = &mut deferred_draws[*deferred_draw_ix];
            self.element_id_stack
                .clone_from(&deferred_draw.element_id_stack);
            self.next_frame
                .dispatch_tree
                .set_active_node(deferred_draw.parent_node);

            let paint_start = self.paint_index();
            if let Some(element) = deferred_draw.element.as_mut() {
                self.with_rendered_view(deferred_draw.current_view, |window| {
                    element.paint(window, cx);
                })
            } else {
                self.reuse_paint(deferred_draw.paint_range.clone());
            }
            let paint_end = self.paint_index();
            deferred_draw.paint_range = paint_start..paint_end;
        }
        self.next_frame.deferred_draws = deferred_draws;
        self.element_id_stack.clear();
    }

    pub(crate) fn prepaint_index(&self) -> PrepaintStateIndex {
        PrepaintStateIndex {
            hitboxes_index: self.next_frame.hitboxes.len(),
            tooltips_index: self.next_frame.tooltip_requests.len(),
            deferred_draws_index: self.next_frame.deferred_draws.len(),
            dispatch_tree_index: self.next_frame.dispatch_tree.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            line_layout_index: self.text_system.layout_index(),
        }
    }

    pub(crate) fn truncate_prepaint_to(&mut self, index: PrepaintStateIndex) {
        self.next_frame.hitboxes.truncate(index.hitboxes_index);
        self.next_frame
            .tooltip_requests
            .truncate(index.tooltips_index);
        self.next_frame
            .deferred_draws
            .truncate(index.deferred_draws_index);
        self.next_frame
            .dispatch_tree
            .truncate(index.dispatch_tree_index);
        self.next_frame
            .accessed_element_states
            .truncate(index.accessed_element_states_index);
        self.text_system.truncate_layouts(index.line_layout_index);
    }

    pub(crate) fn reuse_prepaint(&mut self, range: Range<PrepaintStateIndex>) {
        self.next_frame.hitboxes.extend(
            self.rendered_frame.hitboxes[range.start.hitboxes_index..range.end.hitboxes_index]
                .iter()
                .cloned(),
        );
        self.next_frame.tooltip_requests.extend(
            self.rendered_frame.tooltip_requests
                [range.start.tooltips_index..range.end.tooltips_index]
                .iter_mut()
                .map(|request| request.take()),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (GlobalElementId(id.0.clone()), *type_id)),
        );
        self.text_system
            .reuse_layouts(range.start.line_layout_index..range.end.line_layout_index);

        let reused_subtree = self.next_frame.dispatch_tree.reuse_subtree(
            range.start.dispatch_tree_index..range.end.dispatch_tree_index,
            &mut self.rendered_frame.dispatch_tree,
            self.focus,
        );

        if reused_subtree.contains_focus() {
            self.next_frame.focus = self.focus;
        }

        self.next_frame.deferred_draws.extend(
            self.rendered_frame.deferred_draws
                [range.start.deferred_draws_index..range.end.deferred_draws_index]
                .iter()
                .map(|deferred_draw| DeferredDraw {
                    current_view: deferred_draw.current_view,
                    parent_node: reused_subtree.refresh_node_id(deferred_draw.parent_node),
                    element_id_stack: deferred_draw.element_id_stack.clone(),
                    text_style_stack: deferred_draw.text_style_stack.clone(),
                    priority: deferred_draw.priority,
                    element: None,
                    absolute_offset: deferred_draw.absolute_offset,
                    prepaint_range: deferred_draw.prepaint_range.clone(),
                    paint_range: deferred_draw.paint_range.clone(),
                }),
        );
    }

    pub(crate) fn paint_index(&self) -> PaintIndex {
        PaintIndex {
            scene_index: self.next_frame.scene.len(),
            mouse_listeners_index: self.next_frame.mouse_listeners.len(),
            input_handlers_index: self.next_frame.input_handlers.len(),
            cursor_styles_index: self.next_frame.cursor_styles.len(),
            window_control_hitboxes_index: self.next_frame.window_control_hitboxes.len(),
            accessed_element_states_index: self.next_frame.accessed_element_states.len(),
            tab_handle_index: self.next_frame.tab_stops.paint_index(),
            line_layout_index: self.text_system.layout_index(),
        }
    }

    pub(crate) fn reuse_paint(&mut self, range: Range<PaintIndex>) {
        let retained_paint_start = self.paint_index();
        self.next_frame.cursor_styles.extend(
            self.rendered_frame.cursor_styles
                [range.start.cursor_styles_index..range.end.cursor_styles_index]
                .iter()
                .cloned(),
        );
        self.next_frame.window_control_hitboxes.extend(
            self.rendered_frame.window_control_hitboxes[range.start.window_control_hitboxes_index
                ..range.end.window_control_hitboxes_index]
                .iter()
                .cloned(),
        );
        self.next_frame.input_handlers.extend(
            self.rendered_frame.input_handlers
                [range.start.input_handlers_index..range.end.input_handlers_index]
                .iter_mut()
                .map(|handler| handler.take()),
        );
        self.next_frame.mouse_listeners.extend(
            self.rendered_frame.mouse_listeners
                [range.start.mouse_listeners_index..range.end.mouse_listeners_index]
                .iter_mut()
                .map(|listener| listener.take()),
        );
        self.next_frame.accessed_element_states.extend(
            self.rendered_frame.accessed_element_states[range.start.accessed_element_states_index
                ..range.end.accessed_element_states_index]
                .iter()
                .map(|(id, type_id)| (GlobalElementId(id.0.clone()), *type_id)),
        );
        self.next_frame.tab_stops.replay(
            &self.rendered_frame.tab_stops.insertion_history
                [range.start.tab_handle_index..range.end.tab_handle_index],
        );

        self.text_system.reuse_layouts(
            range.start.line_layout_index.clone()..range.end.line_layout_index.clone(),
        );
        let old_scene_range = range.start.scene_index..range.end.scene_index;
        let new_scene_start = self.next_frame.scene.len();
        self.next_frame
            .scene
            .replay(old_scene_range, &self.rendered_frame.scene);
        let new_scene_range = new_scene_start..self.next_frame.scene.len();
        if let Some(bounds) = self
            .next_frame
            .scene
            .bounds_for_range(new_scene_range.clone())
        {
            let entity_id = self.current_view();
            self.next_frame
                .retained_scene_segments
                .push(RetainedSceneSegment {
                    bounds,
                    scene_range: new_scene_range,
                    paint_range: retained_paint_start..self.paint_index(),
                    prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
                    entity_id,
                    dirty: false,
                });
        }
    }

    /// Push a text style onto the stack, and call a function with that style active.
    /// Use [`Window::text_style`] to get the current, combined text style. This method
    /// should only be called as part of element drawing.
    pub fn with_text_style<F, R>(&mut self, style: Option<TextStyleRefinement>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(style) = style {
            self.text_style_stack.push(style);
            let result = f(self);
            self.text_style_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the cursor style at the platform level. This method should only be called
    /// during the prepaint phase of element drawing.
    pub fn set_cursor_style(&mut self, style: CursorStyle, hitbox: &Hitbox) {
        self.invalidator.debug_assert_paint();
        self.next_frame.cursor_styles.push(CursorStyleRequest {
            hitbox_id: Some(hitbox.id),
            style,
        });
    }

    /// Updates the cursor style for the entire window at the platform level. A cursor
    /// style using this method will have precedence over any cursor style set using
    /// `set_cursor_style`. This method should only be called during the prepaint
    /// phase of element drawing.
    pub fn set_window_cursor_style(&mut self, style: CursorStyle) {
        self.invalidator.debug_assert_paint();
        self.next_frame.cursor_styles.push(CursorStyleRequest {
            hitbox_id: None,
            style,
        })
    }

    /// Sets a tooltip to be rendered for the upcoming frame. This method should only be called
    /// during the paint phase of element drawing.
    pub fn set_tooltip(&mut self, tooltip: AnyTooltip) -> TooltipId {
        self.invalidator.debug_assert_prepaint();
        let id = TooltipId(post_inc(&mut self.next_tooltip_id.0));
        self.next_frame
            .tooltip_requests
            .push(Some(TooltipRequest { id, tooltip }));
        id
    }

    /// Invoke the given function with the given content mask after intersecting it
    /// with the current mask. This method should only be called during element drawing.
    pub fn with_content_mask<R>(
        &mut self,
        mask: Option<ContentMask<Pixels>>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();
        if let Some(mask) = mask {
            let mask = mask.intersect(&self.content_mask());
            self.content_mask_stack.push(mask);
            let result = f(self);
            self.content_mask_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Updates the global element offset relative to the current offset. This is used to implement
    /// scrolling. This method should only be called during the prepaint phase of element drawing.
    pub fn with_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();

        if offset.is_zero() {
            return f(self);
        };

        let abs_offset = self.element_offset() + offset;
        self.with_absolute_element_offset(abs_offset, f)
    }

    /// Updates the global element offset based on the given offset. This is used to implement
    /// drag handles and other manual painting of elements. This method should only be called during
    /// the prepaint phase of element drawing.
    pub fn with_absolute_element_offset<R>(
        &mut self,
        offset: Point<Pixels>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack.push(offset);
        let result = f(self);
        self.element_offset_stack.pop();
        result
    }

    pub(crate) fn with_element_opacity<R>(
        &mut self,
        opacity: Option<f32>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.invalidator.debug_assert_paint_or_prepaint();

        let Some(opacity) = opacity else {
            return f(self);
        };

        let previous_opacity = self.element_opacity;
        self.element_opacity = previous_opacity * opacity;
        let result = f(self);
        self.element_opacity = previous_opacity;
        result
    }

    /// Perform prepaint on child elements in a "retryable" manner, so that any side effects
    /// of prepaints can be discarded before prepainting again. This is used to support autoscroll
    /// where we need to prepaint children to detect the autoscroll bounds, then adjust the
    /// element offset and prepaint again. See [`crate::List`] for an example. This method should only be
    /// called during the prepaint phase of element drawing.
    pub fn transact<T, U>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, U>) -> Result<T, U> {
        self.invalidator.debug_assert_prepaint();
        let index = self.prepaint_index();
        let result = f(self);
        if result.is_err() {
            self.truncate_prepaint_to(index);
        }
        result
    }

    /// When you call this method during [`Element::prepaint`], containing elements will attempt to
    /// scroll to cause the specified bounds to become visible. When they decide to autoscroll, they will call
    /// [`Element::prepaint`] again with a new set of bounds. See [`crate::List`] for an example of an element
    /// that supports this method being called on the elements it contains. This method should only be
    /// called during the prepaint phase of element drawing.
    pub fn request_autoscroll(&mut self, bounds: Bounds<Pixels>) {
        self.invalidator.debug_assert_prepaint();
        self.requested_autoscroll = Some(bounds);
    }

    /// This method can be called from a containing element such as [`crate::List`] to support the autoscroll behavior
    /// described in [`Self::request_autoscroll`].
    pub fn take_autoscroll(&mut self) -> Option<Bounds<Pixels>> {
        self.invalidator.debug_assert_prepaint();
        self.requested_autoscroll.take()
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading this will return None.
    /// Your view will be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time.
    pub fn use_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        let (task, is_first) = cx.fetch_asset::<A>(source);
        task.clone().now_or_never().or_else(|| {
            if is_first {
                let entity_id = self.current_view();
                self.spawn(cx, {
                    let task = task.clone();
                    async move |cx| {
                        task.await;

                        cx.on_next_frame(move |_, cx| {
                            cx.notify(entity_id);
                        });
                    }
                })
                .detach();
            }

            None
        })
    }

    /// Asynchronously load an asset, if the asset hasn't finished loading or doesn't exist this will return None.
    /// Your view will not be re-drawn once the asset has finished loading.
    ///
    /// Note that the multiple calls to this method will only result in one `Asset::load` call at a
    /// time.
    pub fn get_asset<A: Asset>(&mut self, source: &A::Source, cx: &mut App) -> Option<A::Output> {
        let (task, _) = cx.fetch_asset::<A>(source);
        task.now_or_never()
    }
    /// Obtain the current element offset. This method should only be called during the
    /// prepaint phase of element drawing.
    pub fn element_offset(&self) -> Point<Pixels> {
        self.invalidator.debug_assert_prepaint();
        self.element_offset_stack
            .last()
            .copied()
            .unwrap_or_default()
    }

    /// Obtain the current element opacity. This method should only be called during the
    /// prepaint phase of element drawing.
    #[inline]
    pub(crate) fn element_opacity(&self) -> f32 {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.element_opacity
    }

    /// Obtain the current content mask. This method should only be called during element drawing.
    pub fn content_mask(&self) -> ContentMask<Pixels> {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.content_mask_stack
            .last()
            .cloned()
            .unwrap_or_else(|| ContentMask {
                bounds: Bounds {
                    origin: Point::default(),
                    size: self.viewport_size,
                },
            })
    }

    /// Provide elements in the called function with a new namespace in which their identifiers must be unique.
    /// This can be used within a custom element to distinguish multiple sets of child elements.
    pub fn with_element_namespace<R>(
        &mut self,
        element_id: impl Into<ElementId>,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.element_id_stack.push(element_id.into());
        let result = f(self);
        self.element_id_stack.pop();
        result
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames.
    pub fn use_keyed_state<S: 'static>(
        &mut self,
        key: impl Into<ElementId>,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        let current_view = self.current_view();
        self.with_global_id(key.into(), |global_id, window| {
            window.with_element_state(global_id, |state: Option<Entity<S>>, window| {
                if let Some(state) = state {
                    (state.clone(), state)
                } else {
                    let new_state = cx.new(|cx| init(window, cx));
                    cx.observe(&new_state, move |_, cx| {
                        cx.notify(current_view);
                    })
                    .detach();
                    (new_state.clone(), new_state)
                }
            })
        })
    }

    /// Immediately push an element ID onto the stack. Useful for simplifying IDs in lists
    pub fn with_id<R>(&mut self, id: impl Into<ElementId>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.with_global_id(id.into(), |_, window| f(window))
    }

    /// Use a piece of state that exists as long this element is being rendered in consecutive frames, without needing to specify a key
    ///
    /// NOTE: This method uses the location of the caller to generate an ID for this state.
    ///       If this is not sufficient to identify your state (e.g. you're rendering a list item),
    ///       you can provide a custom ElementID using the `use_keyed_state` method.
    #[track_caller]
    pub fn use_state<S: 'static>(
        &mut self,
        cx: &mut App,
        init: impl FnOnce(&mut Self, &mut Context<S>) -> S,
    ) -> Entity<S> {
        self.use_keyed_state(
            ElementId::CodeLocation(*core::panic::Location::caller()),
            cx,
            init,
        )
    }

    /// Updates or initializes state for an element with the given id that lives across multiple
    /// frames. If an element with this ID existed in the rendered frame, its state will be passed
    /// to the given closure. The state returned by the closure will be stored so it can be referenced
    /// when drawing the next frame. This method should only be called as part of element drawing.
    pub fn with_element_state<S, R>(
        &mut self,
        global_id: &GlobalElementId,
        f: impl FnOnce(Option<S>, &mut Self) -> (R, S),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        let key = (GlobalElementId(global_id.0.clone()), TypeId::of::<S>());
        self.next_frame
            .accessed_element_states
            .push((GlobalElementId(key.0.clone()), TypeId::of::<S>()));

        if let Some(any) = self
            .next_frame
            .element_states
            .remove(&key)
            .or_else(|| self.rendered_frame.element_states.remove(&key))
        {
            let ElementStateBox {
                inner,
                #[cfg(debug_assertions)]
                type_name,
            } = any;
            // Using the extra inner option to avoid needing to reallocate a new box.
            let mut state_box = inner
                .downcast::<Option<S>>()
                .map_err(|_| {
                    #[cfg(debug_assertions)]
                    {
                        anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}, actual: {:?}",
                            std::any::type_name::<S>(),
                            type_name
                        )
                    }

                    #[cfg(not(debug_assertions))]
                    {
                        anyhow::anyhow!(
                            "invalid element state type for id, requested {:?}",
                            std::any::type_name::<S>(),
                        )
                    }
                })
                .unwrap();

            let state = state_box.take().expect(
                "reentrant call to with_element_state for the same state type and element id",
            );
            let (result, state) = f(Some(state), self);
            state_box.replace(state);
            self.next_frame.element_states.insert(
                key,
                ElementStateBox {
                    inner: state_box,
                    #[cfg(debug_assertions)]
                    type_name,
                },
            );
            result
        } else {
            let (result, state) = f(None, self);
            self.next_frame.element_states.insert(
                key,
                ElementStateBox {
                    inner: Box::new(Some(state)),
                    #[cfg(debug_assertions)]
                    type_name: std::any::type_name::<S>(),
                },
            );
            result
        }
    }

    /// A variant of `with_element_state` that allows the element's id to be optional. This is a convenience
    /// method for elements where the element id may or may not be assigned. Prefer using `with_element_state`
    /// when the element is guaranteed to have an id.
    ///
    /// The first option means 'no ID provided'
    /// The second option means 'not yet initialized'
    pub fn with_optional_element_state<S, R>(
        &mut self,
        global_id: Option<&GlobalElementId>,
        f: impl FnOnce(Option<Option<S>>, &mut Self) -> (R, Option<S>),
    ) -> R
    where
        S: 'static,
    {
        self.invalidator.debug_assert_paint_or_prepaint();

        if let Some(global_id) = global_id {
            self.with_element_state(global_id, |state, cx| {
                let (result, state) = f(Some(state), cx);
                let state =
                    state.expect("you must return some state when you pass some element id");
                (result, state)
            })
        } else {
            let (result, state) = f(None, self);
            debug_assert!(
                state.is_none(),
                "you must not return an element state when passing None for the global id"
            );
            result
        }
    }

    /// Executes the given closure within the context of a tab group.
    #[inline]
    pub fn with_tab_group<R>(&mut self, index: Option<isize>, f: impl FnOnce(&mut Self) -> R) -> R {
        if let Some(index) = index {
            self.next_frame.tab_stops.begin_group(index);
            let result = f(self);
            self.next_frame.tab_stops.end_group();
            result
        } else {
            f(self)
        }
    }

    /// Defers the drawing of the given element, scheduling it to be painted on top of the currently-drawn tree
    /// at a later time. The `priority` parameter determines the drawing order relative to other deferred elements,
    /// with higher values being drawn on top.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn defer_draw(
        &mut self,
        element: AnyElement,
        absolute_offset: Point<Pixels>,
        priority: usize,
    ) {
        self.invalidator.debug_assert_prepaint();
        let parent_node = self.next_frame.dispatch_tree.active_node_id().unwrap();
        self.next_frame.deferred_draws.push(DeferredDraw {
            current_view: self.current_view(),
            parent_node,
            element_id_stack: self.element_id_stack.clone(),
            text_style_stack: self.text_style_stack.clone(),
            priority,
            element: Some(element),
            absolute_offset,
            prepaint_range: PrepaintStateIndex::default()..PrepaintStateIndex::default(),
            paint_range: PaintIndex::default()..PaintIndex::default(),
        });
    }

    /// Creates a new painting layer for the specified bounds. A "layer" is a batch
    /// of geometry that are non-overlapping and have the same draw order. This is typically used
    /// for performance reasons.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_layer<R>(&mut self, bounds: Bounds<Pixels>, f: impl FnOnce(&mut Self) -> R) -> R {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let clipped_bounds = bounds.intersect(&content_mask.bounds);
        if !clipped_bounds.is_empty() {
            self.next_frame
                .scene
                .push_layer(clipped_bounds.scale(scale_factor));
        }

        let result = f(self);

        if !clipped_bounds.is_empty() {
            self.next_frame.scene.pop_layer();
        }

        result
    }

    /// Paint one or more drop shadows into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_shadows(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        shadows: &[BoxShadow],
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        for shadow in shadows {
            let shadow_bounds = (bounds + shadow.offset).dilate(shadow.spread_radius);
            self.next_frame.scene.insert_primitive(Shadow {
                order: 0,
                blur_radius: shadow.blur_radius.scale(scale_factor),
                bounds: shadow_bounds.scale(scale_factor),
                content_mask: content_mask.scale(scale_factor),
                corner_radii: corner_radii.scale(scale_factor),
                color: shadow.color.opacity(opacity),
            });
        }
    }

    /// Paint one or more quads into the scene for the next frame at the current stacking context.
    /// Quads are colored rectangular regions with an optional background, border, and corner radius.
    /// see [`fill`], [`outline`], and [`quad`] to construct this type.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    ///
    /// Note that the `quad.corner_radii` are allowed to exceed the bounds, creating sharp corners
    /// where the circular arcs meet. This will not display well when combined with dashed borders.
    /// Use `Corners::clamp_radii_for_quad_size` if the radii should fit within the bounds.
    pub fn paint_quad(&mut self, quad: PaintQuad) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        self.next_frame.scene.insert_primitive(Quad {
            order: 0,
            bounds: quad.bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            background: quad.background.opacity(opacity),
            border_color: quad.border_color.opacity(opacity),
            corner_radii: quad.corner_radii.scale(scale_factor),
            border_widths: quad.border_widths.scale(scale_factor),
            border_style: quad.border_style,
        });
    }

    /// Paint the given `Path` into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_path(&mut self, mut path: Path<Pixels>, color: impl Into<Background>) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();
        path.content_mask = content_mask;
        let color: Background = color.into();
        path.color = color.opacity(opacity);
        self.next_frame
            .scene
            .insert_primitive(path.scale(scale_factor));
    }

    /// Paint an underline into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_underline(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &UnderlineStyle,
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let height = if style.wavy {
            style.thickness * 3.
        } else {
            style.thickness
        };
        let bounds = Bounds {
            origin,
            size: size(width, height),
        };
        let content_mask = self.content_mask();
        let element_opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(Underline {
            order: 0,
            pad: 0,
            bounds: bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            color: style.color.unwrap_or_default().opacity(element_opacity),
            thickness: style.thickness.scale(scale_factor),
            wavy: if style.wavy { 1 } else { 0 },
        });
    }

    /// Paint a strikethrough into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_strikethrough(
        &mut self,
        origin: Point<Pixels>,
        width: Pixels,
        style: &StrikethroughStyle,
    ) {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let height = style.thickness;
        let bounds = Bounds {
            origin,
            size: size(width, height),
        };
        let content_mask = self.content_mask();
        let opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(Underline {
            order: 0,
            pad: 0,
            bounds: bounds.scale(scale_factor),
            content_mask: content_mask.scale(scale_factor),
            thickness: style.thickness.scale(scale_factor),
            color: style.color.unwrap_or_default().opacity(opacity),
            wavy: 0,
        });
    }

    /// Paints a monochrome (non-emoji) glyph into the scene for the next frame at the current z-index.
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single glyph that has already been shaped.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_glyph(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
        is_cjk: bool,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let (_, subpixel_variant) = glyph_device_origin(origin, Point::default(), scale_factor);
        let subpixel_rendering = self.should_use_subpixel_rendering(font_id, font_size);
        let dilation = self.text_system().glyph_dilation_for_color(color);

        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant,
            scale_factor,
            is_emoji: false,
            is_cjk,
            subpixel_rendering,
            dilation,
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let Some(tile) = self.sprite_atlas.get_or_insert_glyph(&params, &mut || {
                self.text_system().rasterize_glyph(&params)
            })?
            else {
                log::warn!(
                    "glyph atlas allocation failed: font_id={:?} glyph_id={:?} font_size={:?} cjk={} subpixel={}",
                    font_id,
                    glyph_id,
                    font_size,
                    is_cjk,
                    subpixel_rendering
                );
                return Ok(());
            };
            let (origin, _) = glyph_device_origin(origin, raster_bounds.origin, scale_factor);
            let bounds = Bounds {
                origin,
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.content_mask().scale(scale_factor);
            if subpixel_rendering {
                self.next_frame.scene.insert_primitive(SubpixelSprite {
                    order: 0,
                    pad: 0,
                    bounds,
                    content_mask,
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            } else {
                self.next_frame.scene.insert_primitive(MonochromeSprite {
                    order: 0,
                    pad: MonochromeSpriteSampling::Glyph as u32,
                    bounds,
                    content_mask,
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
        }
        Ok(())
    }

    fn should_use_subpixel_rendering(&self, font_id: FontId, font_size: Pixels) -> bool {
        if self.platform_window.background_appearance() != WindowBackgroundAppearance::Opaque {
            return false;
        }
        if !self.platform_window.is_subpixel_rendering_supported() {
            return false;
        }

        self.text_system()
            .recommended_rendering_mode(font_id, font_size)
            == TextRenderingMode::Subpixel
    }

    /// Paints an emoji glyph into the scene for the next frame at the current z-index.
    ///
    /// The y component of the origin is the baseline of the glyph.
    /// You should generally prefer to use the [`ShapedLine::paint`](crate::ShapedLine::paint) or
    /// [`WrappedLine::paint`](crate::WrappedLine::paint) methods in the [`TextSystem`](crate::TextSystem).
    /// This method is only useful if you need to paint a single emoji that has already been shaped.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_emoji(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            // We don't render emojis with subpixel variants.
            subpixel_variant: Default::default(),
            scale_factor,
            is_emoji: true,
            is_cjk: false,
            subpixel_rendering: false,
            dilation: 0,
        };

        let raster_bounds = self.text_system().raster_bounds(&params)?;
        if !raster_bounds.is_zero() {
            let Some(tile) = self.sprite_atlas.get_or_insert_glyph(&params, &mut || {
                self.text_system().rasterize_glyph(&params)
            })?
            else {
                log::warn!(
                    "emoji atlas allocation failed: font_id={:?} glyph_id={:?} font_size={:?}",
                    font_id,
                    glyph_id,
                    font_size
                );
                return Ok(());
            };

            let bounds = Bounds {
                origin: glyph_origin.map(|px| px.floor()) + raster_bounds.origin.map(Into::into),
                size: tile.bounds.size.map(Into::into),
            };
            let content_mask = self.content_mask().scale(scale_factor);
            let opacity = self.element_opacity();

            self.next_frame.scene.insert_primitive(PolychromeSprite {
                order: 0,
                pad: 0,
                grayscale: false,
                bounds,
                corner_radii: Default::default(),
                content_mask,
                tile,
                opacity,
            });
        }
        Ok(())
    }

    /// Paint a monochrome SVG into the scene for the next frame at the current stacking context.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_svg(
        &mut self,
        bounds: Bounds<Pixels>,
        path: SharedString,
        transformation: TransformationMatrix,
        color: Hsla,
        cx: &App,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();

        let bounds = bounds.scale(scale_factor);
        let svg_bounds =
            svg_paint_bounds_for_requested_bounds(bounds.map(|value| ScaledPixels(value.0)));
        let params = RenderSvgParams {
            path,
            size: svg_raster_size_for_paint_bounds(svg_bounds),
        };

        let Some(tile) =
            self.sprite_atlas
                .get_or_insert_with(&params.clone().into(), &mut || {
                    let Some((size, bytes)) = cx.svg_renderer.render(&params)? else {
                        return Ok(None);
                    };
                    Ok(Some((size, Cow::Owned(bytes))))
                })?
        else {
            return Ok(());
        };
        let content_mask = self.content_mask().scale(scale_factor);

        self.next_frame.scene.insert_primitive(MonochromeSprite {
            order: 0,
            pad: MonochromeSpriteSampling::Linear as u32,
            bounds: svg_bounds,
            content_mask,
            color: color.opacity(element_opacity),
            tile,
            transformation,
        });

        Ok(())
    }

    /// Paint an image into the scene for the next frame at the current z-index.
    /// This method will panic if the frame_index is not valid
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_image(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame_index: usize,
        grayscale: bool,
    ) -> Result<()> {
        let frame = data
            .frame(frame_index)
            .ok_or_else(|| anyhow!("invalid image frame index {frame_index}"))?;
        self.paint_image_frame(bounds, corner_radii, data, frame, grayscale)
    }

    pub(crate) fn paint_image_frame(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        data: Arc<RenderImage>,
        frame: AnimatedFrame,
        grayscale: bool,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let animation_config = self.image_pipeline_config.animated;
        let frame_slot = data.gpu_frame_slot_for_frame(frame.sequence(), animation_config);
        let params = RenderImageParams {
            image_id: data.id,
            frame_slot,
            pixel_format: frame.pixel_format(),
        };
        let animated_slot_key = AnimatedImageSlotKey {
            image_id: data.id,
            frame_slot,
        };
        let update_animated_slot = data.is_animated()
            && self.animated_image_slots.get(&animated_slot_key).copied() != Some(frame.sequence());

        let atlas_key = params.into();
        let mut build = || Ok(Some((frame.size(), Cow::Borrowed(frame.bytes()))));
        let tile = if update_animated_slot {
            self.sprite_atlas
                .get_or_update_with(&atlas_key, &mut build)?
        } else {
            self.sprite_atlas
                .get_or_insert_with(&atlas_key, &mut build)?
        };
        let Some(tile) = tile else {
            log::warn!(
                "gpui image atlas allocation failed; skipping image for this frame and retrying: image_id={:?} frame_slot={:?} size={:?} pixel_format={:?}",
                data.id,
                frame_slot,
                frame.size(),
                frame.pixel_format()
            );
            self.invalidator.set_dirty(true);
            return Ok(());
        };
        if update_animated_slot {
            self.animated_image_slots
                .insert(animated_slot_key, frame.sequence());
        }
        let content_mask = self.content_mask().scale(scale_factor);
        let corner_radii = corner_radii.scale(scale_factor);
        let opacity = self.element_opacity();

        self.next_frame.scene.insert_primitive(PolychromeSprite {
            order: 0,
            pad: 0,
            grayscale,
            bounds: bounds
                .map_origin(|origin| origin.floor())
                .map_size(|size| size.ceil()),
            content_mask,
            corner_radii,
            tile,
            opacity,
        });
        Ok(())
    }

    /// Paint a GPU-backed backdrop blur over content already drawn behind `bounds`.
    ///
    /// Backends that do not yet implement a real blur may draw the optional tint only; the
    /// primitive remains in the scene so diagnostics and future backend work are consistent.
    pub fn paint_backdrop_blur(
        &mut self,
        bounds: Bounds<Pixels>,
        corner_radii: Corners<Pixels>,
        style: BackdropBlurStyle,
    ) {
        use crate::PaintBackdropBlur;

        self.invalidator.debug_assert_paint();

        if style.radius <= Pixels::ZERO && style.tint.is_none() {
            return;
        }

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintBackdropBlur {
            order: 0,
            bounds: bounds
                .map_origin(|origin| origin.floor())
                .map_size(|size| size.ceil()),
            content_mask,
            corner_radii: corner_radii.scale(scale_factor),
            radius: ScaledPixels::from(f32::from(style.radius) * scale_factor),
            downsample: style.downsample.max(1),
            levels: style.levels.clamp(1, 6),
            saturation: style.saturation.max(0.0),
            tint: style.tint,
        });
    }

    /// Paint a GPU-resident 3D mesh into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn paint_gpu_mesh_3d(
        &mut self,
        bounds: Bounds<Pixels>,
        mesh: Arc<GpuMesh3d>,
        camera: GpuMesh3dCamera,
    ) {
        use crate::PaintGpuMesh3d;

        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintGpuMesh3d {
            order: 0,
            bounds,
            content_mask,
            mesh,
            camera,
        });
    }

    /// Paint a surface into the scene for the next frame at the current z-index.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    #[cfg(target_os = "macos")]
    pub fn paint_surface(&mut self, bounds: Bounds<Pixels>, image_buffer: CVPixelBuffer) {
        use crate::{PaintSurface, SurfaceContent};

        self.invalidator.debug_assert_paint();

        let scale_factor = self.scale_factor();
        let bounds = bounds.scale(scale_factor);
        let content_mask = self.content_mask().scale(scale_factor);
        self.next_frame.scene.insert_primitive(PaintSurface {
            order: 0,
            bounds,
            content_mask,
            content: SurfaceContent::CoreVideo(image_buffer),
        });
    }

    /// Removes an image from the sprite atlas.
    pub fn drop_image(&mut self, data: Arc<RenderImage>) -> Result<()> {
        let animation_config = self.image_pipeline_config.animated;
        let frame_slots = if data.is_animated() {
            data.frame_count()
                .min(animation_config.max_gpu_frame_slots.max(1))
        } else {
            data.frame_count()
        };
        for frame_slot in 0..frame_slots {
            let params = RenderImageParams {
                image_id: data.id,
                frame_slot,
                pixel_format: RenderImagePixelFormat::Bgra8,
            };

            self.sprite_atlas.remove(&params.clone().into());
            let params = RenderImageParams {
                image_id: data.id,
                frame_slot,
                pixel_format: RenderImagePixelFormat::Rgba8,
            };
            self.sprite_atlas.remove(&params.into());
        }
        self.animated_image_slots
            .retain(|slot_key, _| slot_key.image_id != data.id);
        record_image_drop(1);

        Ok(())
    }

    /// Hints the platform renderer backing this window to release idle GPUI resources.
    pub fn trim_gpui_memory(&mut self, level: GpuiMemoryTrimLevel) {
        self.text_system.trim_window_caches(level);
        self.rendered_frame.trim_retained_capacity_for_level(level);
        self.next_frame.trim_retained_capacity_for_level(level);
        self.platform_window.trim_gpui_memory(level);
    }

    pub(crate) fn set_gpui_memory_policy(&mut self, policy: GpuiMemoryPolicy) {
        self.trim_memory_on_hidden = policy.trim_on_window_hidden;
    }

    /// Add a node to the layout tree for the current frame. Takes the `Style` of the element for which
    /// layout is being requested, along with the layout ids of any children. This method is called during
    /// calls to the [`Element::request_layout`] trait method and enables any element to participate in layout.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    #[must_use]
    pub fn request_layout(
        &mut self,
        style: Style,
        children: impl IntoIterator<Item = LayoutId>,
        cx: &mut App,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        cx.layout_id_buffer.clear();
        cx.layout_id_buffer.extend(children);
        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();

        self.layout_engine.as_mut().unwrap().request_layout(
            style,
            rem_size,
            scale_factor,
            &cx.layout_id_buffer,
        )
    }

    /// Add a node to the layout tree for the current frame. Instead of taking a `Style` and children,
    /// this variant takes a function that is invoked during layout so you can use arbitrary logic to
    /// determine the element's size. One place this is used internally is when measuring text.
    ///
    /// The given closure is invoked at layout time with the known dimensions and available space and
    /// returns a `Size`.
    ///
    /// This method should only be called as part of the request_layout or prepaint phase of element drawing.
    pub fn request_measured_layout<
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    >(
        &mut self,
        style: Style,
        measure: F,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_measured_layout(style, rem_size, scale_factor, measure)
    }

    pub(crate) fn request_measured_layout_with_fingerprint<
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    >(
        &mut self,
        style: Style,
        fingerprint_seed: u64,
        measure: F,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_measured_layout_with_fingerprint(
                style,
                rem_size,
                scale_factor,
                Some(fingerprint_seed),
                measure,
            )
    }

    pub(crate) fn request_pure_measured_layout_with_fingerprint<
        F: FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>
            + 'static,
    >(
        &mut self,
        style: Style,
        fingerprint_seed: u64,
        measure: F,
    ) -> LayoutId {
        self.invalidator.debug_assert_prepaint();

        let rem_size = self.rem_size();
        let scale_factor = self.scale_factor();
        self.layout_engine
            .as_mut()
            .unwrap()
            .request_pure_measured_layout_with_fingerprint(
                style,
                rem_size,
                scale_factor,
                Some(fingerprint_seed),
                measure,
            )
    }

    /// Compute the layout for the given id within the given available space.
    /// This method is called for its side effect, typically by the framework prior to painting.
    /// After calling it, you can request the bounds of the given layout node id or any descendant.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn compute_layout(
        &mut self,
        layout_id: LayoutId,
        available_space: Size<AvailableSpace>,
        cx: &mut App,
    ) {
        self.invalidator.debug_assert_prepaint();

        let mut layout_engine = self.layout_engine.take().unwrap();
        layout_engine.compute_layout(layout_id, available_space, self, cx);
        self.layout_engine = Some(layout_engine);
    }

    /// Obtain the bounds computed for the given LayoutId relative to the window. This method will usually be invoked by
    /// GPUI itself automatically in order to pass your element its `Bounds` automatically.
    ///
    /// This method should only be called as part of element drawing.
    pub fn layout_bounds(&mut self, layout_id: LayoutId) -> Bounds<Pixels> {
        self.invalidator.debug_assert_prepaint();

        let scale_factor = self.scale_factor();
        let mut bounds = self
            .layout_engine
            .as_mut()
            .unwrap()
            .layout_bounds(layout_id, scale_factor)
            .map(Into::into);
        bounds.origin += self.element_offset();
        bounds
    }

    /// This method should be called during `prepaint`. You can use
    /// the returned [Hitbox] during `paint` or in an event handler
    /// to determine whether the inserted hitbox was the topmost.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn insert_hitbox(&mut self, bounds: Bounds<Pixels>, behavior: HitboxBehavior) -> Hitbox {
        self.invalidator.debug_assert_prepaint();

        let content_mask = self.content_mask();
        let mut id = self.next_hitbox_id;
        self.next_hitbox_id = self.next_hitbox_id.next();
        let hitbox = Hitbox {
            id,
            bounds,
            content_mask,
            behavior,
        };
        self.next_frame.hitboxes.push(hitbox.clone());
        hitbox
    }

    /// Set a hitbox which will act as a control area of the platform window.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn insert_window_control_hitbox(&mut self, area: WindowControlArea, hitbox: Hitbox) {
        self.invalidator.debug_assert_paint();
        self.next_frame.window_control_hitboxes.push((area, hitbox));
    }

    /// Sets the key context for the current element. This context will be used to translate
    /// keybindings into actions.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn set_key_context(&mut self, context: KeyContext) {
        self.invalidator.debug_assert_paint();
        self.next_frame.dispatch_tree.set_key_context(context);
    }

    /// Sets the focus handle for the current element. This handle will be used to manage focus state
    /// and keyboard event dispatch for the element.
    ///
    /// This method should only be called as part of the prepaint phase of element drawing.
    pub fn set_focus_handle(&mut self, focus_handle: &FocusHandle, _: &App) {
        self.invalidator.debug_assert_prepaint();
        if focus_handle.is_focused(self) {
            self.next_frame.focus = Some(focus_handle.id);
        }
        self.next_frame.dispatch_tree.set_focus_id(focus_handle.id);
    }

    /// Sets the view id for the current element, which will be used to manage view caching.
    ///
    /// This method should only be called as part of element prepaint. We plan on removing this
    /// method eventually when we solve some issues that require us to construct editor elements
    /// directly instead of always using editors via views.
    pub fn set_view_id(&mut self, view_id: EntityId) {
        self.invalidator.debug_assert_prepaint();
        self.next_frame.dispatch_tree.set_view_id(view_id);
    }

    /// Get the entity ID for the currently rendering view
    pub fn current_view(&self) -> EntityId {
        self.invalidator.debug_assert_paint_or_prepaint();
        self.rendered_entity_stack.last().copied().unwrap()
    }

    fn current_view_or_root(&self) -> Option<EntityId> {
        self.rendered_entity_stack
            .last()
            .copied()
            .or_else(|| self.root.as_ref().map(AnyView::entity_id))
    }

    pub(crate) fn with_rendered_view<R>(
        &mut self,
        id: EntityId,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.rendered_entity_stack.push(id);
        let should_track_segment = self.invalidator.phase() == DrawPhase::Paint;
        let prepaint_start = should_track_segment.then(|| self.prepaint_index());
        let paint_start = should_track_segment.then(|| self.paint_index());
        let was_dirty = should_track_segment && self.dirty_views.contains(&id);
        let result = f(self);
        if should_track_segment {
            if let (Some(prepaint_start), Some(paint_start)) = (prepaint_start, paint_start) {
                let paint_end = self.paint_index();
                let scene_range = paint_start.scene_index..paint_end.scene_index;
                if scene_range.start != scene_range.end {
                    if let Some(bounds) =
                        self.next_frame.scene.bounds_for_range(scene_range.clone())
                    {
                        self.next_frame
                            .retained_scene_segments
                            .push(RetainedSceneSegment {
                                bounds,
                                scene_range,
                                paint_range: paint_start..paint_end,
                                prepaint_range: prepaint_start..self.prepaint_index(),
                                entity_id: id,
                                dirty: was_dirty,
                            });
                    }
                }
            }
        }
        self.rendered_entity_stack.pop();
        result
    }

    /// Executes the provided function with the specified image cache.
    pub fn with_image_cache<F, R>(&mut self, image_cache: Option<AnyImageCache>, f: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        if let Some(image_cache) = image_cache {
            self.image_cache_stack.push(image_cache);
            let result = f(self);
            self.image_cache_stack.pop();
            result
        } else {
            f(self)
        }
    }

    /// Sets an input handler, such as [`ElementInputHandler`][element_input_handler], which interfaces with the
    /// platform to receive textual input with proper integration with concerns such
    /// as IME interactions. This handler will be active for the upcoming frame until the following frame is
    /// rendered.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    ///
    /// [element_input_handler]: crate::ElementInputHandler
    pub fn handle_input(
        &mut self,
        focus_handle: &FocusHandle,
        input_handler: impl InputHandler,
        cx: &App,
    ) {
        self.invalidator.debug_assert_paint();

        if focus_handle.is_focused(self) {
            let cx = self.to_async(cx);
            self.next_frame
                .input_handlers
                .push(Some(PlatformInputHandler::new(cx, Box::new(input_handler))));
        }
    }

    /// Register a mouse event listener on the window for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_mouse_event<Event: MouseEvent>(
        &mut self,
        mut handler: impl FnMut(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame
            .mouse_listeners
            .push(MouseListener::new::<Event>(Box::new(
                move |event: &dyn Any, phase: DispatchPhase, window: &mut Window, cx: &mut App| {
                    if let Some(event) = event.downcast_ref() {
                        handler(event, phase, window, cx)
                    }
                },
            )));
    }

    /// Register a key event listener on the window for the next frame. The type of event
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a global listener.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_key_event<Event: KeyEvent>(
        &mut self,
        listener: impl Fn(&Event, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame.dispatch_tree.on_key_event(Rc::new(
            move |event: &dyn Any, phase, window: &mut Window, cx: &mut App| {
                if let Some(event) = event.downcast_ref::<Event>() {
                    listener(event, phase, window, cx)
                }
            },
        ));
    }

    /// Register a modifiers changed event listener on the window for the next frame.
    ///
    /// This is a fairly low-level method, so prefer using event handlers on elements unless you have
    /// a specific need to register a global listener.
    ///
    /// This method should only be called as part of the paint phase of element drawing.
    pub fn on_modifiers_changed(
        &mut self,
        listener: impl Fn(&ModifiersChangedEvent, &mut Window, &mut App) + 'static,
    ) {
        self.invalidator.debug_assert_paint();

        self.next_frame.dispatch_tree.on_modifiers_changed(Rc::new(
            move |event: &ModifiersChangedEvent, window: &mut Window, cx: &mut App| {
                listener(event, window, cx)
            },
        ));
    }

    /// Register a listener to be called when the given focus handle or one of its descendants receives focus.
    /// This does not fire if the given focus handle - or one of its descendants - was previously focused.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_in(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) =
            self.new_focus_listener(Box::new(move |event, window, cx| {
                if event.is_focus_in(focus_id) {
                    listener(window, cx);
                }
                true
            }));
        cx.defer(move |_| activate());
        subscription
    }

    /// Register a listener to be called when the given focus handle or one of its descendants loses focus.
    /// Returns a subscription and persists until the subscription is dropped.
    pub fn on_focus_out(
        &mut self,
        handle: &FocusHandle,
        cx: &mut App,
        mut listener: impl FnMut(FocusOutEvent, &mut Window, &mut App) + 'static,
    ) -> Subscription {
        let focus_id = handle.id;
        let (subscription, activate) =
            self.new_focus_listener(Box::new(move |event, window, cx| {
                if let Some(blurred_id) = event.previous_focus_path.last().copied()
                    && event.is_focus_out(focus_id)
                {
                    let event = FocusOutEvent {
                        blurred: WeakFocusHandle {
                            id: blurred_id,
                            handles: Arc::downgrade(&cx.focus_handles),
                        },
                    };
                    listener(event, window, cx)
                }
                true
            }));
        cx.defer(move |_| activate());
        subscription
    }

    fn reset_cursor_style(&self, cx: &mut App) {
        // Set the cursor only if we're the active window.
        if self.is_window_hovered() {
            let style = if matches!(self.window_decorations(), Decorations::Client { .. }) {
                self.client_inset
                    .and_then(|inset| resize_edge_hit_test(self, self.mouse_position(), inset))
                    .map(resize_edge_cursor_style)
                    .or_else(|| self.rendered_frame.cursor_style(self))
                    .unwrap_or(CursorStyle::Arrow)
            } else {
                self.rendered_frame
                    .cursor_style(self)
                    .unwrap_or(CursorStyle::Arrow)
            };
            cx.platform.set_cursor_style(style);
        }
    }

    /// Dispatch a given keystroke as though the user had typed it.
    /// You can create a keystroke with Keystroke::parse("").
    pub fn dispatch_keystroke(&mut self, keystroke: Keystroke, cx: &mut App) -> bool {
        let keystroke = keystroke.with_simulated_ime();
        let result = self.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
            }),
            cx,
        );
        if !result.propagate {
            return true;
        }

        if let Some(input) = keystroke.key_char
            && let Some(mut input_handler) = self.platform_window.take_input_handler()
        {
            input_handler.dispatch_input(&input, self, cx);
            self.platform_window.set_input_handler(input_handler);
            return true;
        }

        false
    }

    /// Return a key binding string for an action, to display in the UI. Uses the highest precedence
    /// binding for the action (last binding added to the keymap).
    pub fn keystroke_text_for(&self, action: &dyn Action) -> String {
        self.highest_precedence_binding_for_action(action)
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| action.name().to_string())
    }

    /// Dispatch a mouse or keyboard event on the window.
    #[profiling::function]
    pub fn dispatch_event(&mut self, event: PlatformInput, cx: &mut App) -> DispatchEventResult {
        let event_started_at = Instant::now();
        let event_name = platform_input_name(&event);
        if event.unconditionally_extends_recent_input_present() {
            self.last_input_timestamp.set(Instant::now());
        }
        // Handlers may set this to false by calling `stop_propagation`.
        cx.propagate_event = true;
        // Handlers may set this to true by calling `prevent_default`.
        self.default_prevented = false;

        let event = match event {
            // Track the mouse position with our own state, since accessing the platform
            // API for the mouse position can only occur on the main thread.
            PlatformInput::MouseMove(mouse_move) => {
                self.mouse_position = mouse_move.position;
                self.modifiers = mouse_move.modifiers;
                PlatformInput::MouseMove(mouse_move)
            }
            PlatformInput::MouseDown(mouse_down) => {
                self.mouse_position = mouse_down.position;
                self.modifiers = mouse_down.modifiers;
                PlatformInput::MouseDown(mouse_down)
            }
            PlatformInput::MouseUp(mouse_up) => {
                self.mouse_position = mouse_up.position;
                self.modifiers = mouse_up.modifiers;
                PlatformInput::MouseUp(mouse_up)
            }
            PlatformInput::MouseExited(mouse_exited) => {
                self.modifiers = mouse_exited.modifiers;
                PlatformInput::MouseExited(mouse_exited)
            }
            PlatformInput::ModifiersChanged(modifiers_changed) => {
                self.modifiers = modifiers_changed.modifiers;
                self.capslock = modifiers_changed.capslock;
                PlatformInput::ModifiersChanged(modifiers_changed)
            }
            PlatformInput::ScrollWheel(scroll_wheel) => {
                self.mouse_position = scroll_wheel.position;
                self.modifiers = scroll_wheel.modifiers;
                PlatformInput::ScrollWheel(scroll_wheel)
            }
            // Translate dragging and dropping of external files from the operating system
            // to internal drag and drop events.
            PlatformInput::FileDrop(file_drop) => match file_drop {
                FileDropEvent::Entered { position, paths } => {
                    self.mouse_position = position;
                    if cx.active_drag.is_none() {
                        cx.active_drag = Some(AnyDrag {
                            value: Arc::new(paths.clone()),
                            view: cx.new(|_| paths).into(),
                            cursor_offset: position,
                            cursor_style: None,
                        });
                    }
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    })
                }
                FileDropEvent::Pending { position } => {
                    self.mouse_position = position;
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position,
                        pressed_button: Some(MouseButton::Left),
                        modifiers: Modifiers::default(),
                    })
                }
                FileDropEvent::Submit { position } => {
                    cx.activate(true);
                    self.mouse_position = position;
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    })
                }
                FileDropEvent::Exited => {
                    cx.active_drag.take();
                    PlatformInput::FileDrop(FileDropEvent::Exited)
                }
            },
            PlatformInput::KeyDown(_) | PlatformInput::KeyUp(_) => event,
        };

        if let Some(any_mouse_event) = event.mouse_event() {
            self.dispatch_mouse_event(any_mouse_event, cx);
        } else if let Some(any_key_event) = event.keyboard_event() {
            self.dispatch_key_event(any_key_event, cx);
        }

        let event_elapsed = event_started_at.elapsed();
        log_timed_gpui_event("gpui dispatch_event", event_elapsed, || {
            format!(
                "event={} propagate={} default_prevented={}",
                event_name, cx.propagate_event, self.default_prevented
            )
        });

        DispatchEventResult {
            propagate: cx.propagate_event,
            default_prevented: self.default_prevented,
        }
    }

    fn dispatch_mouse_event(&mut self, event: &dyn Any, cx: &mut App) {
        let mouse_position = event
            .downcast_ref::<MouseMoveEvent>()
            .map(|event| event.position)
            .or_else(|| {
                event
                    .downcast_ref::<MouseDownEvent>()
                    .map(|event| event.position)
            })
            .or_else(|| {
                event
                    .downcast_ref::<MouseUpEvent>()
                    .map(|event| event.position)
            })
            .unwrap_or_else(|| self.mouse_position());
        let hit_test = self.rendered_frame.hit_test(mouse_position);
        let hit_test_unchanged = hit_test == self.mouse_hit_test;
        if hit_test != self.mouse_hit_test {
            self.mouse_hit_test = hit_test;
            self.reset_cursor_style(cx);
        }

        let client_resize_edge = if matches!(self.window_decorations(), Decorations::Client { .. })
        {
            self.client_inset
                .and_then(|inset| resize_edge_hit_test(self, mouse_position, inset))
        } else {
            None
        };

        if let Some(edge) = client_resize_edge {
            if event.is::<MouseMoveEvent>() {
                self.reset_cursor_style(cx);
            } else if event
                .downcast_ref::<MouseDownEvent>()
                .is_some_and(|mouse_down| mouse_down.button == MouseButton::Left)
            {
                self.start_window_resize(edge);
                cx.propagate_event = false;
                self.default_prevented = true;
                return;
            }
        }

        #[cfg(any(feature = "inspector", debug_assertions))]
        if self.is_inspector_picking(cx) {
            self.handle_inspector_mouse_event(event, cx);
            // When inspector is picking, all other mouse handling is skipped.
            return;
        }

        if event
            .downcast_ref::<MouseMoveEvent>()
            .is_some_and(|event| event.pressed_button.is_none())
            && hit_test_unchanged
            && self.mouse_hit_test.ids.is_empty()
            && !cx.has_active_drag()
        {
            self.reset_cursor_style(cx);
            record_skipped_pointer_frame();
            if log::log_enabled!(log::Level::Debug) {
                log::debug!(
                    "gpui mouse move skipped: hit_test_unchanged={} active_drag={} ids={}",
                    hit_test_unchanged,
                    cx.has_active_drag(),
                    self.mouse_hit_test.ids.len()
                );
            }
            return;
        }

        let mut mouse_listeners = mem::take(&mut self.rendered_frame.mouse_listeners);
        let event_type = event.type_id();

        // Capture phase, events bubble from back to front. Handlers for this phase are used for
        // special purposes, such as detecting events outside of a given Bounds.
        for listener in &mut mouse_listeners {
            if !listener.handles(event_type) {
                continue;
            }
            let Some(listener) = listener.listener_mut() else {
                continue;
            };
            listener(event, DispatchPhase::Capture, self, cx);
            if !cx.propagate_event {
                break;
            }
        }

        // Bubble phase, where most normal handlers do their work.
        if cx.propagate_event {
            for listener in mouse_listeners.iter_mut().rev() {
                if !listener.handles(event_type) {
                    continue;
                }
                let Some(listener) = listener.listener_mut() else {
                    continue;
                };
                listener(event, DispatchPhase::Bubble, self, cx);
                if !cx.propagate_event {
                    break;
                }
            }
        }

        self.rendered_frame.mouse_listeners = mouse_listeners;

        if cx.has_active_drag() {
            if event.is::<MouseMoveEvent>() {
                // If this was a mouse move event, redraw the window so that the
                // active drag can follow the mouse cursor.
                self.refresh();
            } else if event.is::<MouseUpEvent>() {
                // If this was a mouse up event, cancel the active drag and redraw
                // the window.
                cx.active_drag = None;
                self.refresh();
            }
        }
    }

    fn dispatch_key_event(&mut self, event: &dyn Any, cx: &mut App) {
        if self.invalidator.is_dirty() {
            self.draw(cx, TARGET_FRAME_GENERATION_BUDGET).clear();
        }

        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        let mut keystroke: Option<Keystroke> = None;

        if let Some(event) = event.downcast_ref::<ModifiersChangedEvent>() {
            if event.modifiers.number_of_modifiers() == 0
                && self.pending_modifier.modifiers.number_of_modifiers() == 1
                && !self.pending_modifier.saw_keystroke
            {
                let key = match self.pending_modifier.modifiers {
                    modifiers if modifiers.shift => Some("shift"),
                    modifiers if modifiers.control => Some("control"),
                    modifiers if modifiers.alt => Some("alt"),
                    modifiers if modifiers.platform => Some("platform"),
                    modifiers if modifiers.function => Some("function"),
                    _ => None,
                };
                if let Some(key) = key {
                    keystroke = Some(Keystroke {
                        key: key.to_string(),
                        key_char: None,
                        modifiers: Modifiers::default(),
                    });
                }
            }

            if self.pending_modifier.modifiers.number_of_modifiers() == 0
                && event.modifiers.number_of_modifiers() == 1
            {
                self.pending_modifier.saw_keystroke = false
            }
            self.pending_modifier.modifiers = event.modifiers
        } else if let Some(key_down_event) = event.downcast_ref::<KeyDownEvent>() {
            self.pending_modifier.saw_keystroke = true;
            keystroke = Some(key_down_event.keystroke.clone());
        }

        let Some(keystroke) = keystroke else {
            self.finish_dispatch_key_event(event, dispatch_path, self.context_stack(), cx);
            return;
        };

        cx.propagate_event = true;
        self.dispatch_keystroke_interceptors(event, self.context_stack(), cx);
        if !cx.propagate_event {
            self.finish_dispatch_key_event(event, dispatch_path, self.context_stack(), cx);
            return;
        }

        let mut currently_pending = self.pending_input.take().unwrap_or_default();
        if currently_pending.focus.is_some() && currently_pending.focus != self.focus {
            currently_pending = PendingInput::default();
        }

        let match_result = self.rendered_frame.dispatch_tree.dispatch_key(
            currently_pending.keystrokes,
            keystroke,
            &dispatch_path,
        );

        if !match_result.to_replay.is_empty() {
            self.replay_pending_input(match_result.to_replay, cx);
            cx.propagate_event = true;
        }

        if !match_result.pending.is_empty() {
            currently_pending.keystrokes = match_result.pending;
            currently_pending.focus = self.focus;
            currently_pending.timer = Some(self.spawn(cx, async move |cx| {
                cx.background_executor.timer(Duration::from_secs(1)).await;
                let _ = ignore_window_not_found(cx.update(move |window, cx| {
                    let Some(currently_pending) = window
                        .pending_input
                        .take()
                        .filter(|pending| pending.focus == window.focus)
                    else {
                        return;
                    };

                    let node_id = window.focus_node_id_in_rendered_frame(window.focus);
                    let dispatch_path = window.rendered_frame.dispatch_tree.dispatch_path(node_id);

                    let to_replay = window
                        .rendered_frame
                        .dispatch_tree
                        .flush_dispatch(currently_pending.keystrokes, &dispatch_path);

                    window.pending_input_changed(cx);
                    window.replay_pending_input(to_replay, cx)
                }));
            }));
            self.pending_input = Some(currently_pending);
            self.pending_input_changed(cx);
            cx.propagate_event = false;
            return;
        }

        for binding in match_result.bindings {
            self.dispatch_action_on_node(node_id, binding.action.as_ref(), cx);
            if !cx.propagate_event {
                self.dispatch_keystroke_observers(
                    event,
                    Some(binding.action),
                    match_result.context_stack,
                    cx,
                );
                self.pending_input_changed(cx);
                return;
            }
        }

        self.finish_dispatch_key_event(event, dispatch_path, match_result.context_stack, cx);
        self.pending_input_changed(cx);
    }

    fn finish_dispatch_key_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: SmallVec<[DispatchNodeId; 32]>,
        context_stack: Vec<KeyContext>,
        cx: &mut App,
    ) {
        self.dispatch_key_down_up_event(event, &dispatch_path, cx);
        if !cx.propagate_event {
            return;
        }

        self.dispatch_modifiers_changed_event(event, &dispatch_path, cx);
        if !cx.propagate_event {
            return;
        }

        self.dispatch_keystroke_observers(event, None, context_stack, cx);
    }

    fn pending_input_changed(&mut self, cx: &mut App) {
        self.pending_input_observers
            .clone()
            .retain(&(), |callback| callback(self, cx));
    }

    fn dispatch_key_down_up_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
        cx: &mut App,
    ) {
        // Capture phase
        for node_id in dispatch_path {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);

            for key_listener in node.key_listeners.clone() {
                key_listener(event, DispatchPhase::Capture, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }

        // Bubble phase
        for node_id in dispatch_path.iter().rev() {
            // Handle low level key events
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for key_listener in node.key_listeners.clone() {
                key_listener(event, DispatchPhase::Bubble, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }
    }

    fn dispatch_modifiers_changed_event(
        &mut self,
        event: &dyn Any,
        dispatch_path: &SmallVec<[DispatchNodeId; 32]>,
        cx: &mut App,
    ) {
        let Some(event) = event.downcast_ref::<ModifiersChangedEvent>() else {
            return;
        };
        for node_id in dispatch_path.iter().rev() {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for listener in node.modifiers_changed_listeners.clone() {
                listener(event, self, cx);
                if !cx.propagate_event {
                    return;
                }
            }
        }
    }

    /// Determine whether a potential multi-stroke key binding is in progress on this window.
    pub fn has_pending_keystrokes(&self) -> bool {
        self.pending_input.is_some()
    }

    pub(crate) fn clear_pending_keystrokes(&mut self) {
        self.pending_input.take();
    }

    /// Returns the currently pending input keystrokes that might result in a multi-stroke key binding.
    pub fn pending_input_keystrokes(&self) -> Option<&[Keystroke]> {
        self.pending_input
            .as_ref()
            .map(|pending_input| pending_input.keystrokes.as_slice())
    }

    fn replay_pending_input(&mut self, replays: SmallVec<[Replay; 1]>, cx: &mut App) {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        'replay: for replay in replays {
            let event = KeyDownEvent {
                keystroke: replay.keystroke.clone(),
                is_held: false,
            };

            cx.propagate_event = true;
            for binding in replay.bindings {
                self.dispatch_action_on_node(node_id, binding.action.as_ref(), cx);
                if !cx.propagate_event {
                    self.dispatch_keystroke_observers(
                        &event,
                        Some(binding.action),
                        Vec::default(),
                        cx,
                    );
                    continue 'replay;
                }
            }

            self.dispatch_key_down_up_event(&event, &dispatch_path, cx);
            if !cx.propagate_event {
                continue 'replay;
            }
            if let Some(input) = replay.keystroke.key_char.as_ref().cloned()
                && let Some(mut input_handler) = self.platform_window.take_input_handler()
            {
                input_handler.dispatch_input(&input, self, cx);
                self.platform_window.set_input_handler(input_handler)
            }
        }
    }

    fn focus_node_id_in_rendered_frame(&self, focus_id: Option<FocusId>) -> DispatchNodeId {
        focus_id
            .and_then(|focus_id| {
                self.rendered_frame
                    .dispatch_tree
                    .focusable_node_id(focus_id)
            })
            .unwrap_or_else(|| self.rendered_frame.dispatch_tree.root_node_id())
    }

    fn dispatch_action_on_node(
        &mut self,
        node_id: DispatchNodeId,
        action: &dyn Action,
        cx: &mut App,
    ) {
        let dispatch_path = self.rendered_frame.dispatch_tree.dispatch_path(node_id);

        // Capture phase for global actions.
        cx.propagate_event = true;
        if let Some(mut global_listeners) = cx
            .global_action_listeners
            .remove(&action.as_any().type_id())
        {
            for listener in &global_listeners {
                listener(action.as_any(), DispatchPhase::Capture, cx);
                if !cx.propagate_event {
                    break;
                }
            }

            global_listeners.extend(
                cx.global_action_listeners
                    .remove(&action.as_any().type_id())
                    .unwrap_or_default(),
            );

            cx.global_action_listeners
                .insert(action.as_any().type_id(), global_listeners);
        }

        if !cx.propagate_event {
            return;
        }

        // Capture phase for window actions.
        for node_id in &dispatch_path {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for DispatchActionListener {
                action_type,
                listener,
            } in node.action_listeners.clone()
            {
                let any_action = action.as_any();
                if action_type == any_action.type_id() {
                    listener(any_action, DispatchPhase::Capture, self, cx);

                    if !cx.propagate_event {
                        return;
                    }
                }
            }
        }

        // Bubble phase for window actions.
        for node_id in dispatch_path.iter().rev() {
            let node = self.rendered_frame.dispatch_tree.node(*node_id);
            for DispatchActionListener {
                action_type,
                listener,
            } in node.action_listeners.clone()
            {
                let any_action = action.as_any();
                if action_type == any_action.type_id() {
                    cx.propagate_event = false; // Actions stop propagation by default during the bubble phase
                    listener(any_action, DispatchPhase::Bubble, self, cx);

                    if !cx.propagate_event {
                        return;
                    }
                }
            }
        }

        // Bubble phase for global actions.
        if let Some(mut global_listeners) = cx
            .global_action_listeners
            .remove(&action.as_any().type_id())
        {
            for listener in global_listeners.iter().rev() {
                cx.propagate_event = false; // Actions stop propagation by default during the bubble phase

                listener(action.as_any(), DispatchPhase::Bubble, cx);
                if !cx.propagate_event {
                    break;
                }
            }

            global_listeners.extend(
                cx.global_action_listeners
                    .remove(&action.as_any().type_id())
                    .unwrap_or_default(),
            );

            cx.global_action_listeners
                .insert(action.as_any().type_id(), global_listeners);
        }
    }

    /// Register the given handler to be invoked whenever the global of the given type
    /// is updated.
    pub fn observe_global<G: Global>(
        &mut self,
        cx: &mut App,
        f: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Subscription {
        let window_handle = self.handle;
        let (subscription, activate) = cx.global_observers.insert(
            TypeId::of::<G>(),
            Box::new(move |cx| {
                window_handle
                    .update(cx, |_, window, cx| f(window, cx))
                    .is_ok()
            }),
        );
        cx.defer(move |_| activate());
        subscription
    }

    /// Focus the current window and bring it to the foreground at the platform level.
    pub fn activate_window(&self) {
        self.platform_window.activate();
    }

    /// Minimize the current window at the platform level.
    pub fn minimize_window(&self) {
        self.platform_window.minimize();
    }

    /// Maximize the current window at the platform level.
    pub fn maximize_window(&self) {
        self.platform_window.maximize();
    }

    /// Restore the current window from maximized or minimized state at the platform level.
    pub fn restore_window(&self) {
        self.platform_window.restore();
    }

    /// Show the current window at the platform level.
    pub fn show_window(&self) {
        self.platform_window.show();
    }

    /// Hide the current window at the platform level.
    pub fn hide_window(&self) {
        self.platform_window.hide_window();
        if self.trim_memory_on_hidden {
            self.text_system
                .trim_window_caches(GpuiMemoryTrimLevel::Moderate);
            self.platform_window
                .trim_gpui_memory(GpuiMemoryTrimLevel::Moderate);
        }
    }

    /// Toggle full screen status on the current window at the platform level.
    pub fn toggle_fullscreen(&self) {
        self.platform_window.toggle_fullscreen();
    }

    /// Updates the IME panel position suggestions for languages like japanese, chinese.
    pub fn invalidate_character_coordinates(&self) {
        self.on_next_frame(|window, cx| {
            if let Some(mut input_handler) = window.platform_window.take_input_handler() {
                if let Some(bounds) = input_handler.selected_bounds(window, cx) {
                    window.platform_window.update_ime_position(bounds);
                }
                window.platform_window.set_input_handler(input_handler);
            }
        });
    }

    /// Present a platform dialog.
    /// The provided message will be presented, along with buttons for each answer.
    /// When a button is clicked, the returned Receiver will receive the index of the clicked button.
    pub fn prompt<T>(
        &mut self,
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[T],
        cx: &mut App,
    ) -> oneshot::Receiver<usize>
    where
        T: Clone + Into<PromptButton>,
    {
        let prompt_builder = cx.prompt_builder.take();
        let Some(prompt_builder) = prompt_builder else {
            unreachable!("Re-entrant window prompting is not supported by GPUI");
        };

        let answers = answers
            .iter()
            .map(|answer| answer.clone().into())
            .collect::<Vec<_>>();

        let receiver = match &prompt_builder {
            PromptBuilder::Default => self
                .platform_window
                .prompt(level, message, detail, &answers)
                .unwrap_or_else(|| {
                    self.build_custom_prompt(&prompt_builder, level, message, detail, &answers, cx)
                }),
            PromptBuilder::Custom(_) => {
                self.build_custom_prompt(&prompt_builder, level, message, detail, &answers, cx)
            }
        };

        cx.prompt_builder = Some(prompt_builder);

        receiver
    }

    fn build_custom_prompt(
        &mut self,
        prompt_builder: &PromptBuilder,
        level: PromptLevel,
        message: &str,
        detail: Option<&str>,
        answers: &[PromptButton],
        cx: &mut App,
    ) -> oneshot::Receiver<usize> {
        let (sender, receiver) = oneshot::channel();
        let handle = PromptHandle::new(sender);
        let handle = (prompt_builder)(level, message, detail, answers, handle, self, cx);
        self.prompt = Some(handle);
        receiver
    }

    /// Returns the current context stack.
    pub fn context_stack(&self) -> Vec<KeyContext> {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree
            .dispatch_path(node_id)
            .iter()
            .filter_map(move |&node_id| dispatch_tree.node(node_id).context.clone())
            .collect()
    }

    /// Returns all available actions for the focused element.
    pub fn available_actions(&self, cx: &App) -> Vec<Box<dyn Action>> {
        let node_id = self.focus_node_id_in_rendered_frame(self.focus);
        let mut actions = self.rendered_frame.dispatch_tree.available_actions(node_id);
        for action_type in cx.global_action_listeners.keys() {
            if let Err(ix) = actions.binary_search_by_key(action_type, |a| a.as_any().type_id()) {
                let action = cx.actions.build_action_type(action_type).ok();
                if let Some(action) = action {
                    actions.insert(ix, action);
                }
            }
        }
        actions
    }

    /// Returns key bindings that invoke an action on the currently focused element. Bindings are
    /// returned in the order they were added. For display, the last binding should take precedence.
    pub fn bindings_for_action(&self, action: &dyn Action) -> Vec<KeyBinding> {
        self.rendered_frame
            .dispatch_tree
            .bindings_for_action(action, &self.rendered_frame.dispatch_tree.context_stack)
    }

    /// Returns the highest precedence key binding that invokes an action on the currently focused
    /// element. This is more efficient than getting the last result of `bindings_for_action`.
    pub fn highest_precedence_binding_for_action(&self, action: &dyn Action) -> Option<KeyBinding> {
        self.rendered_frame
            .dispatch_tree
            .highest_precedence_binding_for_action(
                action,
                &self.rendered_frame.dispatch_tree.context_stack,
            )
    }

    /// Returns the key bindings for an action in a context.
    pub fn bindings_for_action_in_context(
        &self,
        action: &dyn Action,
        context: KeyContext,
    ) -> Vec<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree.bindings_for_action(action, &[context])
    }

    /// Returns the highest precedence key binding for an action in a context. This is more
    /// efficient than getting the last result of `bindings_for_action_in_context`.
    pub fn highest_precedence_binding_for_action_in_context(
        &self,
        action: &dyn Action,
        context: KeyContext,
    ) -> Option<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        dispatch_tree.highest_precedence_binding_for_action(action, &[context])
    }

    /// Returns any bindings that would invoke an action on the given focus handle if it were
    /// focused. Bindings are returned in the order they were added. For display, the last binding
    /// should take precedence.
    pub fn bindings_for_action_in(
        &self,
        action: &dyn Action,
        focus_handle: &FocusHandle,
    ) -> Vec<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let Some(context_stack) = self.context_stack_for_focus_handle(focus_handle) else {
            return vec![];
        };
        dispatch_tree.bindings_for_action(action, &context_stack)
    }

    /// Returns the highest precedence key binding that would invoke an action on the given focus
    /// handle if it were focused. This is more efficient than getting the last result of
    /// `bindings_for_action_in`.
    pub fn highest_precedence_binding_for_action_in(
        &self,
        action: &dyn Action,
        focus_handle: &FocusHandle,
    ) -> Option<KeyBinding> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let context_stack = self.context_stack_for_focus_handle(focus_handle)?;
        dispatch_tree.highest_precedence_binding_for_action(action, &context_stack)
    }

    fn context_stack_for_focus_handle(
        &self,
        focus_handle: &FocusHandle,
    ) -> Option<Vec<KeyContext>> {
        let dispatch_tree = &self.rendered_frame.dispatch_tree;
        let node_id = dispatch_tree.focusable_node_id(focus_handle.id)?;
        let context_stack: Vec<_> = dispatch_tree
            .dispatch_path(node_id)
            .into_iter()
            .filter_map(|node_id| dispatch_tree.node(node_id).context.clone())
            .collect();
        Some(context_stack)
    }

    /// Returns a generic event listener that invokes the given listener with the view and context associated with the given view handle.
    pub fn listener_for<V: Render, E>(
        &self,
        view: &Entity<V>,
        f: impl Fn(&mut V, &E, &mut Window, &mut Context<V>) + 'static,
    ) -> impl Fn(&E, &mut Window, &mut App) + 'static {
        let view = view.downgrade();
        move |e: &E, window: &mut Window, cx: &mut App| {
            view.update(cx, |view, cx| f(view, e, window, cx)).ok();
        }
    }

    /// Returns a generic handler that invokes the given handler with the view and context associated with the given view handle.
    pub fn handler_for<E: 'static, Callback: Fn(&mut E, &mut Window, &mut Context<E>) + 'static>(
        &self,
        entity: &Entity<E>,
        f: Callback,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let entity = entity.downgrade();
        move |window: &mut Window, cx: &mut App| {
            entity.update(cx, |entity, cx| f(entity, window, cx)).ok();
        }
    }

    /// Register a callback that can interrupt the closing of the current window based the returned boolean.
    /// If the callback returns false, the window won't be closed.
    pub fn on_window_should_close(
        &self,
        cx: &App,
        f: impl Fn(&mut Window, &mut App) -> bool + 'static,
    ) {
        let mut cx = self.to_async(cx);
        self.platform_window.on_should_close(Box::new(move || {
            cx.update(|window, cx| f(window, cx)).unwrap_or(true)
        }))
    }

    /// Register an action listener on the window for the next frame. The type of action
    /// is determined by the first parameter of the given listener. When the next frame is rendered
    /// the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using action handlers on elements unless you have
    /// a specific need to register a global listener.
    pub fn on_action(
        &mut self,
        action_type: TypeId,
        listener: impl Fn(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        self.next_frame
            .dispatch_tree
            .on_action(action_type, Rc::new(listener));
    }

    /// Register an action listener on the window for the next frame if the condition is true.
    /// The type of action is determined by the first parameter of the given listener.
    /// When the next frame is rendered the listener will be cleared.
    ///
    /// This is a fairly low-level method, so prefer using action handlers on elements unless you have
    /// a specific need to register a global listener.
    pub fn on_action_when(
        &mut self,
        condition: bool,
        action_type: TypeId,
        listener: impl Fn(&dyn Any, DispatchPhase, &mut Window, &mut App) + 'static,
    ) {
        if condition {
            self.next_frame
                .dispatch_tree
                .on_action(action_type, Rc::new(listener));
        }
    }

    /// Read information about the GPU backing this window.
    /// Currently returns None on Mac and Windows.
    pub fn gpu_specs(&self) -> Option<GpuSpecs> {
        self.platform_window.gpu_specs()
    }

    /// Performs the platform titlebar double-click action.
    ///
    /// On macOS this follows the user's system titlebar preference. Other platforms toggle
    /// maximized/restored state.
    pub fn titlebar_double_click(&self) {
        self.platform_window.titlebar_double_click();
    }

    /// Gets the window's title at the platform level.
    /// This is macOS specific.
    pub fn window_title(&self) -> String {
        self.platform_window.get_title()
    }

    /// Returns a list of all tabbed windows and their titles.
    /// This is macOS specific.
    pub fn tabbed_windows(&self) -> Option<Vec<SystemWindowTab>> {
        self.platform_window.tabbed_windows()
    }

    /// Returns the tab bar visibility.
    /// This is macOS specific.
    pub fn tab_bar_visible(&self) -> bool {
        self.platform_window.tab_bar_visible()
    }

    /// Merges all open windows into a single tabbed window.
    /// This is macOS specific.
    pub fn merge_all_windows(&self) {
        self.platform_window.merge_all_windows()
    }

    /// Moves the tab to a new containing window.
    /// This is macOS specific.
    pub fn move_tab_to_new_window(&self) {
        self.platform_window.move_tab_to_new_window()
    }

    /// Shows or hides the window tab overview.
    /// This is macOS specific.
    pub fn toggle_window_tab_overview(&self) {
        self.platform_window.toggle_window_tab_overview()
    }

    /// Sets the tabbing identifier for the window.
    /// This is macOS specific.
    pub fn set_tabbing_identifier(&self, tabbing_identifier: Option<String>) {
        self.platform_window
            .set_tabbing_identifier(tabbing_identifier)
    }

    /// Toggles the inspector mode on this window.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn toggle_inspector(&mut self, cx: &mut App) {
        self.inspector = match self.inspector {
            None => Some(cx.new(|_| Inspector::new())),
            Some(_) => None,
        };
        self.refresh();
    }

    /// Returns true if the window is in inspector mode.
    pub fn is_inspector_picking(&self, _cx: &App) -> bool {
        #[cfg(any(feature = "inspector", debug_assertions))]
        {
            if let Some(inspector) = &self.inspector {
                return inspector.read(_cx).is_picking();
            }
        }
        false
    }

    /// Executes the provided function with mutable access to an inspector state.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn with_inspector_state<T: 'static, R>(
        &mut self,
        _inspector_id: Option<&crate::InspectorElementId>,
        cx: &mut App,
        f: impl FnOnce(&mut Option<T>, &mut Self) -> R,
    ) -> R {
        if let Some(inspector_id) = _inspector_id
            && let Some(inspector) = &self.inspector
        {
            let inspector = inspector.clone();
            let active_element_id = inspector.read(cx).active_element_id();
            if Some(inspector_id) == active_element_id {
                return inspector.update(cx, |inspector, _cx| {
                    inspector.with_active_element_state(self, f)
                });
            }
        }
        f(&mut None, self)
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(crate) fn build_inspector_element_id(
        &mut self,
        path: crate::InspectorElementPath,
    ) -> crate::InspectorElementId {
        self.invalidator.debug_assert_paint_or_prepaint();
        let path = Rc::new(path);
        let next_instance_id = self
            .next_frame
            .next_inspector_instance_ids
            .entry(path.clone())
            .or_insert(0);
        let instance_id = *next_instance_id;
        *next_instance_id += 1;
        crate::InspectorElementId { path, instance_id }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn prepaint_inspector(&mut self, inspector_width: Pixels, cx: &mut App) -> Option<AnyElement> {
        if let Some(inspector) = self.inspector.take() {
            let mut inspector_element = AnyView::from(inspector.clone()).into_any_element();
            inspector_element.prepaint_as_root(
                point(self.viewport_size.width - inspector_width, px(0.0)),
                size(inspector_width, self.viewport_size.height).into(),
                self,
                cx,
            );
            self.inspector = Some(inspector);
            Some(inspector_element)
        } else {
            None
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn paint_inspector(&mut self, mut inspector_element: Option<AnyElement>, cx: &mut App) {
        if let Some(mut inspector_element) = inspector_element {
            inspector_element.paint(self, cx);
        };
    }

    /// Registers a hitbox that can be used for inspector picking mode, allowing users to select and
    /// inspect UI elements by clicking on them.
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub fn insert_inspector_hitbox(
        &mut self,
        hitbox_id: HitboxId,
        inspector_id: Option<&crate::InspectorElementId>,
        cx: &App,
    ) {
        self.invalidator.debug_assert_paint_or_prepaint();
        if !self.is_inspector_picking(cx) {
            return;
        }
        if let Some(inspector_id) = inspector_id {
            self.next_frame
                .inspector_hitboxes
                .insert(hitbox_id, inspector_id.clone());
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn paint_inspector_hitbox(&mut self, cx: &App) {
        if let Some(inspector) = self.inspector.as_ref() {
            let inspector = inspector.read(cx);
            if let Some((hitbox_id, _)) = self.hovered_inspector_hitbox(inspector, &self.next_frame)
                && let Some(hitbox) = self
                    .next_frame
                    .hitboxes
                    .iter()
                    .find(|hitbox| hitbox.id == hitbox_id)
            {
                self.paint_quad(crate::fill(hitbox.bounds, crate::rgba(0x61afef4d)));
            }
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn handle_inspector_mouse_event(&mut self, event: &dyn Any, cx: &mut App) {
        let Some(inspector) = self.inspector.clone() else {
            return;
        };
        if event.downcast_ref::<MouseMoveEvent>().is_some() {
            inspector.update(cx, |inspector, _cx| {
                if let Some((_, inspector_id)) =
                    self.hovered_inspector_hitbox(inspector, &self.rendered_frame)
                {
                    inspector.hover(inspector_id, self);
                }
            });
        } else if event.downcast_ref::<crate::MouseDownEvent>().is_some() {
            inspector.update(cx, |inspector, _cx| {
                if let Some((_, inspector_id)) =
                    self.hovered_inspector_hitbox(inspector, &self.rendered_frame)
                {
                    inspector.select(inspector_id, self);
                }
            });
        } else if let Some(event) = event.downcast_ref::<crate::ScrollWheelEvent>() {
            // This should be kept in sync with SCROLL_LINES in x11 platform.
            const SCROLL_LINES: f32 = 3.0;
            const SCROLL_PIXELS_PER_LAYER: f32 = 36.0;
            let delta_y = event
                .delta
                .pixel_delta(px(SCROLL_PIXELS_PER_LAYER / SCROLL_LINES))
                .y;
            if let Some(inspector) = self.inspector.clone() {
                inspector.update(cx, |inspector, _cx| {
                    if let Some(depth) = inspector.pick_depth.as_mut() {
                        *depth += f32::from(delta_y) / SCROLL_PIXELS_PER_LAYER;
                        let max_depth = self.mouse_hit_test.ids.len() as f32 - 0.5;
                        if *depth < 0.0 {
                            *depth = 0.0;
                        } else if *depth > max_depth {
                            *depth = max_depth;
                        }
                        if let Some((_, inspector_id)) =
                            self.hovered_inspector_hitbox(inspector, &self.rendered_frame)
                        {
                            inspector.set_active_element_id(inspector_id, self);
                        }
                    }
                });
            }
        }
    }

    #[cfg(any(feature = "inspector", debug_assertions))]
    fn hovered_inspector_hitbox(
        &self,
        inspector: &Inspector,
        frame: &Frame,
    ) -> Option<(HitboxId, crate::InspectorElementId)> {
        if let Some(pick_depth) = inspector.pick_depth {
            let depth = (pick_depth as i64).try_into().unwrap_or(0);
            let max_skipped = self.mouse_hit_test.ids.len().saturating_sub(1);
            let skip_count = (depth as usize).min(max_skipped);
            for hitbox_id in self.mouse_hit_test.ids.iter().skip(skip_count) {
                if let Some(inspector_id) = frame.inspector_hitboxes.get(hitbox_id) {
                    return Some((*hitbox_id, inspector_id.clone()));
                }
            }
        }
        None
    }

    /// For testing: set the current modifier keys state.
    /// This does not generate any events.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_modifiers(&mut self, modifiers: Modifiers) {
        self.modifiers = modifiers;
    }
}

// #[derive(Clone, Copy, Eq, PartialEq, Hash)]
slotmap::new_key_type! {
    /// A unique identifier for a window.
    pub struct WindowId;
}

impl WindowId {
    /// Converts this window ID to a `u64`.
    pub fn as_u64(&self) -> u64 {
        self.0.as_ffi()
    }
}

impl From<u64> for WindowId {
    fn from(value: u64) -> Self {
        WindowId(slotmap::KeyData::from_ffi(value))
    }
}

/// A handle to a window with a specific root view type.
/// Note that this does not keep the window alive on its own.
#[derive(Deref, DerefMut)]
pub struct WindowHandle<V> {
    #[deref]
    #[deref_mut]
    pub(crate) any_handle: AnyWindowHandle,
    state_type: PhantomData<fn(V) -> V>,
}

impl<V> Debug for WindowHandle<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowHandle")
            .field("any_handle", &self.any_handle.id.as_u64())
            .finish()
    }
}

impl<V: 'static + Render> WindowHandle<V> {
    /// Creates a new handle from a window ID.
    /// This does not check if the root type of the window is `V`.
    pub fn new(id: WindowId) -> Self {
        WindowHandle {
            any_handle: AnyWindowHandle {
                id,
                state_type: TypeId::of::<V>(),
            },
            state_type: PhantomData,
        }
    }

    /// Get the root view out of this window.
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    #[cfg(any(test, feature = "test-support"))]
    pub fn root<C>(&self, cx: &mut C) -> Result<Entity<V>>
    where
        C: AppContext,
    {
        crate::Flatten::flatten(cx.update_window(self.any_handle, |root_view, _, _| {
            root_view
                .downcast::<V>()
                .map_err(|_| anyhow!("the type of the window's root view has changed"))
        }))
    }

    /// Updates the root view of this window.
    ///
    /// This will fail if the window has been closed or if the root view's type does not match
    pub fn update<C, R>(
        &self,
        cx: &mut C,
        update: impl FnOnce(&mut V, &mut Window, &mut Context<V>) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        cx.update_window(self.any_handle, |root_view, window, cx| {
            let view = root_view
                .downcast::<V>()
                .map_err(|_| anyhow!("the type of the window's root view has changed"))?;

            Ok(view.update(cx, |view, cx| update(view, window, cx)))
        })?
    }

    /// Read the root view out of this window.
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    pub fn read<'a>(&self, cx: &'a App) -> Result<&'a V> {
        let x = cx
            .windows
            .get(self.id)
            .and_then(|window| {
                window
                    .as_deref()
                    .and_then(|window| window.root.clone())
                    .map(|root_view| root_view.downcast::<V>())
            })
            .context("window not found")?
            .map_err(|_| anyhow!("the type of the window's root view has changed"))?;

        Ok(x.read(cx))
    }

    /// Read the root view out of this window, with a callback
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    pub fn read_with<C, R>(&self, cx: &C, read_with: impl FnOnce(&V, &App) -> R) -> Result<R>
    where
        C: AppContext,
    {
        cx.read_window(self, |root_view, cx| read_with(root_view.read(cx), cx))
    }

    /// Read the root view pointer off of this window.
    ///
    /// This will fail if the window is closed or if the root view's type does not match `V`.
    pub fn entity<C>(&self, cx: &C) -> Result<Entity<V>>
    where
        C: AppContext,
    {
        cx.read_window(self, |root_view, _cx| root_view)
    }

    /// Check if this window is 'active'.
    ///
    /// Will return `None` if the window is closed or currently
    /// borrowed.
    pub fn is_active(&self, cx: &mut App) -> Option<bool> {
        cx.update_window(self.any_handle, |_, window, _| window.is_window_active())
            .ok()
    }
}

impl<V> Copy for WindowHandle<V> {}

impl<V> Clone for WindowHandle<V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<V> PartialEq for WindowHandle<V> {
    fn eq(&self, other: &Self) -> bool {
        self.any_handle == other.any_handle
    }
}

impl<V> Eq for WindowHandle<V> {}

impl<V> Hash for WindowHandle<V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.any_handle.hash(state);
    }
}

impl<V: 'static> From<WindowHandle<V>> for AnyWindowHandle {
    fn from(val: WindowHandle<V>) -> Self {
        val.any_handle
    }
}

/// A handle to a window with any root view type, which can be downcast to a window with a specific root view type.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct AnyWindowHandle {
    pub(crate) id: WindowId,
    state_type: TypeId,
}

impl AnyWindowHandle {
    /// Get the ID of this window.
    pub fn window_id(&self) -> WindowId {
        self.id
    }

    /// Attempt to convert this handle to a window handle with a specific root view type.
    /// If the types do not match, this will return `None`.
    pub fn downcast<T: 'static>(&self) -> Option<WindowHandle<T>> {
        if TypeId::of::<T>() == self.state_type {
            Some(WindowHandle {
                any_handle: *self,
                state_type: PhantomData,
            })
        } else {
            None
        }
    }

    /// Updates the state of the root view of this window.
    ///
    /// This will fail if the window has been closed.
    pub fn update<C, R>(
        self,
        cx: &mut C,
        update: impl FnOnce(AnyView, &mut Window, &mut App) -> R,
    ) -> Result<R>
    where
        C: AppContext,
    {
        cx.update_window(self, update)
    }

    /// Read the state of the root view of this window.
    ///
    /// This will fail if the window has been closed.
    pub fn read<T, C, R>(self, cx: &C, read: impl FnOnce(Entity<T>, &App) -> R) -> Result<R>
    where
        C: AppContext,
        T: 'static,
    {
        let view = self
            .downcast::<T>()
            .context("the type of the window's root view has changed")?;

        cx.read_window(&view, read)
    }
}

impl HasWindowHandle for Window {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, HandleError> {
        self.platform_window.window_handle()
    }
}

impl HasDisplayHandle for Window {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, HandleError> {
        self.platform_window.display_handle()
    }
}

/// An identifier for an [`Element`].
///
/// Can be constructed with a string, a number, or both, as well
/// as other internal representations.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum ElementId {
    /// The ID of a View element
    View(EntityId),
    /// An integer ID.
    Integer(u64),
    /// A string based ID.
    Name(SharedString),
    /// A UUID.
    Uuid(Uuid),
    /// An ID that's equated with a focus handle.
    FocusHandle(FocusId),
    /// A combination of a name and an integer.
    NamedInteger(SharedString, u64),
    /// A path.
    Path(Arc<std::path::Path>),
    /// A code location.
    CodeLocation(core::panic::Location<'static>),
    /// A labeled child of an element.
    NamedChild(Box<ElementId>, SharedString),
}

impl ElementId {
    /// Constructs an `ElementId::NamedInteger` from a name and `usize`.
    pub fn named_usize(name: impl Into<SharedString>, integer: usize) -> ElementId {
        Self::NamedInteger(name.into(), integer as u64)
    }
}

impl Display for ElementId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElementId::View(entity_id) => write!(f, "view-{}", entity_id)?,
            ElementId::Integer(ix) => write!(f, "{}", ix)?,
            ElementId::Name(name) => write!(f, "{}", name)?,
            ElementId::FocusHandle(_) => write!(f, "FocusHandle")?,
            ElementId::NamedInteger(s, i) => write!(f, "{}-{}", s, i)?,
            ElementId::Uuid(uuid) => write!(f, "{}", uuid)?,
            ElementId::Path(path) => write!(f, "{}", path.display())?,
            ElementId::CodeLocation(location) => write!(f, "{}", location)?,
            ElementId::NamedChild(id, name) => write!(f, "{}-{}", id, name)?,
        }

        Ok(())
    }
}

impl TryInto<SharedString> for ElementId {
    type Error = anyhow::Error;

    fn try_into(self) -> anyhow::Result<SharedString> {
        if let ElementId::Name(name) = self {
            Ok(name)
        } else {
            anyhow::bail!("element id is not string")
        }
    }
}

impl From<usize> for ElementId {
    fn from(id: usize) -> Self {
        ElementId::Integer(id as u64)
    }
}

impl From<i32> for ElementId {
    fn from(id: i32) -> Self {
        Self::Integer(id as u64)
    }
}

impl From<SharedString> for ElementId {
    fn from(name: SharedString) -> Self {
        ElementId::Name(name)
    }
}

impl From<Arc<std::path::Path>> for ElementId {
    fn from(path: Arc<std::path::Path>) -> Self {
        ElementId::Path(path)
    }
}

impl From<&'static str> for ElementId {
    fn from(name: &'static str) -> Self {
        ElementId::Name(name.into())
    }
}

impl<'a> From<&'a FocusHandle> for ElementId {
    fn from(handle: &'a FocusHandle) -> Self {
        ElementId::FocusHandle(handle.id)
    }
}

impl From<(&'static str, EntityId)> for ElementId {
    fn from((name, id): (&'static str, EntityId)) -> Self {
        ElementId::NamedInteger(name.into(), id.as_u64())
    }
}

impl From<(&'static str, usize)> for ElementId {
    fn from((name, id): (&'static str, usize)) -> Self {
        ElementId::NamedInteger(name.into(), id as u64)
    }
}

impl From<(SharedString, usize)> for ElementId {
    fn from((name, id): (SharedString, usize)) -> Self {
        ElementId::NamedInteger(name, id as u64)
    }
}

impl From<(&'static str, u64)> for ElementId {
    fn from((name, id): (&'static str, u64)) -> Self {
        ElementId::NamedInteger(name.into(), id)
    }
}

impl From<Uuid> for ElementId {
    fn from(value: Uuid) -> Self {
        Self::Uuid(value)
    }
}

impl From<(&'static str, u32)> for ElementId {
    fn from((name, id): (&'static str, u32)) -> Self {
        ElementId::NamedInteger(name.into(), id.into())
    }
}

impl<T: Into<SharedString>> From<(ElementId, T)> for ElementId {
    fn from((id, name): (ElementId, T)) -> Self {
        ElementId::NamedChild(Box::new(id), name.into())
    }
}

impl From<&'static core::panic::Location<'static>> for ElementId {
    fn from(location: &'static core::panic::Location<'static>) -> Self {
        ElementId::CodeLocation(*location)
    }
}

/// A rectangle to be rendered in the window at the given position and size.
/// Passed as an argument [`Window::paint_quad`].
#[derive(Clone)]
pub struct PaintQuad {
    /// The bounds of the quad within the window.
    pub bounds: Bounds<Pixels>,
    /// The radii of the quad's corners.
    pub corner_radii: Corners<Pixels>,
    /// The background color of the quad.
    pub background: Background,
    /// The widths of the quad's borders.
    pub border_widths: Edges<Pixels>,
    /// The color of the quad's borders.
    pub border_color: Hsla,
    /// The style of the quad's borders.
    pub border_style: BorderStyle,
}

impl PaintQuad {
    /// Sets the corner radii of the quad.
    pub fn corner_radii(self, corner_radii: impl Into<Corners<Pixels>>) -> Self {
        PaintQuad {
            corner_radii: corner_radii.into(),
            ..self
        }
    }

    /// Sets the border widths of the quad.
    pub fn border_widths(self, border_widths: impl Into<Edges<Pixels>>) -> Self {
        PaintQuad {
            border_widths: border_widths.into(),
            ..self
        }
    }

    /// Sets the border color of the quad.
    pub fn border_color(self, border_color: impl Into<Hsla>) -> Self {
        PaintQuad {
            border_color: border_color.into(),
            ..self
        }
    }

    /// Sets the background color of the quad.
    pub fn background(self, background: impl Into<Background>) -> Self {
        PaintQuad {
            background: background.into(),
            ..self
        }
    }
}

/// Creates a quad with the given parameters.
pub fn quad(
    bounds: Bounds<Pixels>,
    corner_radii: impl Into<Corners<Pixels>>,
    background: impl Into<Background>,
    border_widths: impl Into<Edges<Pixels>>,
    border_color: impl Into<Hsla>,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad {
        bounds,
        corner_radii: corner_radii.into(),
        background: background.into(),
        border_widths: border_widths.into(),
        border_color: border_color.into(),
        border_style,
    }
}

/// Creates a filled quad with the given bounds and background color.
pub fn fill(bounds: impl Into<Bounds<Pixels>>, background: impl Into<Background>) -> PaintQuad {
    PaintQuad {
        bounds: bounds.into(),
        corner_radii: (0.).into(),
        background: background.into(),
        border_widths: (0.).into(),
        border_color: transparent_black(),
        border_style: BorderStyle::default(),
    }
}

/// Creates a rectangle outline with the given bounds, border color, and a 1px border width
pub fn outline(
    bounds: impl Into<Bounds<Pixels>>,
    border_color: impl Into<Hsla>,
    border_style: BorderStyle,
) -> PaintQuad {
    PaintQuad {
        bounds: bounds.into(),
        corner_radii: (0.).into(),
        background: transparent_black().into(),
        border_widths: (1.).into(),
        border_color: border_color.into(),
        border_style,
    }
}
