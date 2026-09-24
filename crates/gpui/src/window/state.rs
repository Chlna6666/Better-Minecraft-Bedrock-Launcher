use super::*;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct LayoutCacheFrameMetrics {
    pub(super) hits: usize,
    pub(super) misses: usize,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FrameGenerationStats {
    pub(super) layout: LayoutFrameMetrics,
    pub(super) layout_cache: LayoutCacheFrameMetrics,
    pub(super) text_layout: LineLayoutFrameMetrics,
    pub(super) scene: SceneFrameMetrics,
    pub(super) frame_retained_capacity: usize,
    pub(super) list_measured_items: usize,
    pub(super) deadline_remaining_at_prepaint_start_us: Option<i64>,
    pub(super) deadline_remaining_at_layout_start_us: Option<i64>,
    pub(super) deadline_remaining_at_paint_start_us: Option<i64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DirtyFrameDiagnostics {
    pub(super) refreshes: usize,
    pub(super) view_dirty: usize,
    pub(super) direct_dirty_views: usize,
    pub(super) traversal_ancestor_views: usize,
    pub(super) selective_splice_attempts: usize,
    pub(super) selective_splice_hits: usize,
    pub(super) rendered_views: usize,
    pub(super) rendered_view_types: [(&'static str, usize); 8],
    pub(super) rendered_view_type_count: usize,
    pub(super) rendered_view_type_overflow: usize,
    pub(super) notify_invalidations: usize,
    pub(super) frame_request_reasons: u16,
    pub(super) first_frame_request: Option<FrameRequestProvenance>,
    pub(super) first_view_dirty_entity: Option<EntityId>,
    pub(super) first_rendered_entity: Option<EntityId>,
    pub(super) first_notify_entity: Option<EntityId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ViewDirtyScope {
    /// This view received an invalidation and must update its own output.
    Direct,
    /// This view is on the route to a dirty descendant.
    TraversalAncestor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FrameRequestReason {
    Input,
    StateNotify,
    LayoutAnimation,
    PresentationAnimation,
    ProgressiveWork,
    ImageReady,
    Timer,
    Recovery,
    ExplicitRedraw,
}

impl FrameRequestReason {
    pub(super) const fn bit(self) -> u16 {
        1 << (self as u8)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FrameRequestProvenance {
    pub(super) reason: FrameRequestReason,
    pub(super) source_file: &'static str,
    pub(super) source_line: u32,
}

impl DirtyFrameDiagnostics {
    pub(super) fn record_refresh(&mut self) {
        self.refreshes = self.refreshes.saturating_add(1);
    }

    pub(super) fn record_view_dirty(&mut self, entity_id: EntityId) {
        self.view_dirty = self.view_dirty.saturating_add(1);
        self.first_view_dirty_entity.get_or_insert(entity_id);
    }

    pub(super) fn record_dirty_scopes(&mut self, direct: usize, ancestors: usize) {
        self.direct_dirty_views = direct;
        self.traversal_ancestor_views = ancestors;
    }

    pub(super) fn record_selective_splice_attempt(&mut self) {
        self.selective_splice_attempts = self.selective_splice_attempts.saturating_add(1);
    }

    pub(super) fn record_selective_splice_hit(&mut self) {
        self.selective_splice_hits = self.selective_splice_hits.saturating_add(1);
    }

    pub(super) fn record_rendered_view(&mut self, entity_id: EntityId, type_name: &'static str) {
        self.rendered_views = self.rendered_views.saturating_add(1);
        self.first_rendered_entity.get_or_insert(entity_id);
        if let Some((_, count)) = self.rendered_view_types[..self.rendered_view_type_count]
            .iter_mut()
            .find(|(name, _)| *name == type_name)
        {
            *count = count.saturating_add(1);
        } else if self.rendered_view_type_count < self.rendered_view_types.len() {
            self.rendered_view_types[self.rendered_view_type_count] = (type_name, 1);
            self.rendered_view_type_count += 1;
        } else {
            self.rendered_view_type_overflow = self.rendered_view_type_overflow.saturating_add(1);
        }
    }

    pub(super) fn record_notify_invalidation(&mut self, entity_id: EntityId) {
        self.notify_invalidations = self.notify_invalidations.saturating_add(1);
        self.first_notify_entity.get_or_insert(entity_id);
    }

    #[track_caller]
    pub(super) fn record_frame_request_reason(&mut self, reason: FrameRequestReason) {
        let location = std::panic::Location::caller();
        self.record_frame_request_reason_at(reason, location.file(), location.line());
    }

    pub(super) fn record_frame_request_reason_at(
        &mut self,
        reason: FrameRequestReason,
        source_file: &'static str,
        source_line: u32,
    ) {
        self.frame_request_reasons |= reason.bit();
        self.first_frame_request
            .get_or_insert(FrameRequestProvenance {
                reason,
                source_file,
                source_line,
            });
    }

    pub(super) fn is_interactive_or_animating(&self) -> bool {
        let mask = FrameRequestReason::Input.bit()
            | FrameRequestReason::LayoutAnimation.bit()
            | FrameRequestReason::PresentationAnimation.bit();
        (self.frame_request_reasons & mask) != 0
    }

    pub(super) fn requires_fresh_progressive_views(&self) -> bool {
        let mask = FrameRequestReason::Input.bit() | FrameRequestReason::ImageReady.bit();
        (self.frame_request_reasons & mask) != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct AnimatedImageSlotKey {
    pub(super) image_id: crate::ImageId,
    pub(super) frame_slot: usize,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct ImagePaintTileCacheKey {
    pub(super) image_id: crate::ImageId,
    pub(super) frame_slot: usize,
    pub(super) frame_sequence: usize,
    pub(super) pixel_format: ImagePixelFormat,
}

#[derive(Clone, Debug, Default)]
pub(super) struct ModifierState {
    pub(super) modifiers: Modifiers,
    pub(super) saw_keystroke: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DrawPhase {
    None,
    Prepaint,
    Paint,
    Focus,
}

#[derive(Default, Debug)]
pub(super) struct PendingInput {
    pub(super) keystrokes: SmallVec<[Keystroke; 1]>,
    pub(super) focus: Option<FocusId>,
    pub(super) timer: Option<Task<()>>,
}

pub(crate) struct ElementStateBox {
    pub(crate) inner: Box<dyn Any>,
    #[cfg(debug_assertions)]
    pub(crate) type_name: &'static str,
}

/// Persistent lookup from a focus handle to the retained rendering identity that owns it.
///
/// Unlike `GlobalElementId`, this identity also exists for anonymous elements through internal
/// `InstanceSlot` path segments. Keeping the lookup on the window lets a focus transition target
/// the old and new repaint boundaries even when either subtree was replayed in an intervening frame.
#[derive(Clone, Debug)]
pub(crate) struct FocusRetainedTarget {
    pub(crate) view_id: EntityId,
    pub(crate) retained_id: GlobalElementId,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ElementVisualTransform {
    pub(crate) scale: f32,
    pub(crate) translation: Point<Pixels>,
}

impl ElementVisualTransform {
    pub(crate) fn identity() -> Self {
        Self {
            scale: 1.0,
            translation: Point::default(),
        }
    }

    pub(crate) fn then_scale(self, scale: f32, origin: Point<Pixels>) -> Self {
        let translation = origin * (1.0 - scale);
        Self {
            scale: self.scale * scale,
            translation: self.translation + translation * self.scale,
        }
    }

    pub(crate) fn transform_point(self, point: Point<Pixels>) -> Point<Pixels> {
        point * self.scale + self.translation
    }

    pub(crate) fn transform_bounds(self, bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        Bounds {
            origin: self.transform_point(bounds.origin),
            size: bounds.size.map(|value| value * self.scale),
        }
    }

    pub(crate) fn transform_mask(self, mask: &ContentMask<Pixels>) -> ContentMask<Pixels> {
        ContentMask {
            bounds: self.transform_bounds(mask.bounds),
            corner_bounds: self.transform_bounds(mask.corner_bounds),
            corner_radii: mask.corner_radii.map(|value| *value * self.scale),
        }
    }
}

#[cfg(test)]
mod visual_transform_tests {
    use super::*;
    use crate::{bounds, point, px, size};

    #[test]
    fn nested_scales_compose_around_responsive_origins() {
        let transform = ElementVisualTransform::identity()
            .then_scale(0.5, point(px(100.0), px(50.0)))
            .then_scale(0.8, point(px(40.0), px(20.0)));

        assert_eq!(transform.scale, 0.4);
        assert_eq!(
            transform.transform_bounds(bounds(point(px(0.0), px(0.0)), size(px(200.0), px(100.0)))),
            bounds(point(px(54.0), px(27.0)), size(px(80.0), px(40.0)))
        );
    }

    #[test]
    fn rendered_view_counts_group_by_type_without_allocating() {
        let mut diagnostics = DirtyFrameDiagnostics::default();
        diagnostics.record_rendered_view(EntityId::from(1), "MainWindowView");
        diagnostics.record_rendered_view(EntityId::from(2), "ChildView");
        diagnostics.record_rendered_view(EntityId::from(1), "MainWindowView");

        assert_eq!(diagnostics.rendered_views, 3);
        assert_eq!(diagnostics.rendered_view_type_count, 2);
        assert_eq!(diagnostics.rendered_view_types[0], ("MainWindowView", 2));
        assert_eq!(diagnostics.rendered_view_types[1], ("ChildView", 1));
        assert_eq!(diagnostics.rendered_view_type_overflow, 0);
    }

    #[test]
    fn frame_request_reasons_coalesce_and_keep_first_provenance() {
        let mut diagnostics = DirtyFrameDiagnostics::default();
        diagnostics.record_frame_request_reason_at(FrameRequestReason::Input, "input.rs", 7);
        diagnostics.record_frame_request_reason_at(FrameRequestReason::Timer, "timer.rs", 9);

        assert_eq!(
            diagnostics.frame_request_reasons,
            FrameRequestReason::Input.bit() | FrameRequestReason::Timer.bit()
        );
        assert_eq!(
            diagnostics.first_frame_request,
            Some(FrameRequestProvenance {
                reason: FrameRequestReason::Input,
                source_file: "input.rs",
                source_line: 7,
            })
        );
    }

    #[test]
    fn input_and_image_ready_require_fresh_progressive_views() {
        for reason in [FrameRequestReason::Input, FrameRequestReason::ImageReady] {
            let mut diagnostics = DirtyFrameDiagnostics::default();
            diagnostics.record_frame_request_reason_at(reason, "latency.rs", 1);
            assert!(diagnostics.requires_fresh_progressive_views());
        }

        for reason in [
            FrameRequestReason::StateNotify,
            FrameRequestReason::ProgressiveWork,
            FrameRequestReason::Timer,
        ] {
            let mut diagnostics = DirtyFrameDiagnostics::default();
            diagnostics.record_frame_request_reason_at(reason, "background.rs", 1);
            assert!(!diagnostics.requires_fresh_progressive_views());
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DispatchEventResult {
    pub propagate: bool,
    pub default_prevented: bool,
}

/// Bounds accumulation state for one view being painted.
///
/// `scanned_until` is the scene index up to which this view's paint operations have already
/// been folded into `bounds` (either scanned directly or merged from a completed child view).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ViewBoundsFrame {
    pub(crate) scanned_until: usize,
    pub(crate) bounds: Option<Bounds<ScaledPixels>>,
}

/// Inherited Window state captured at one cached view boundary.
///
/// A selective ancestor traversal restores this snapshot before rebuilding the dirty child so
/// skipping intermediate Render calls cannot change text inheritance, clipping, transforms,
/// retained identity namespaces, element offsets, or image-cache ownership.
#[derive(Clone)]
pub(crate) struct CachedViewTraversalContext {
    element_id_stack: SmallVec<[ElementId; 32]>,
    retained_element_id_stack: SmallVec<[ElementId; 32]>,
    retained_child_slot_stack: SmallVec<[u32; 32]>,
    text_style_stack: Vec<TextStyleRefinement>,
    rem_size_override_stack: SmallVec<[Pixels; 8]>,
    element_offset_stack: Vec<Point<Pixels>>,
    element_opacity: f32,
    scene_animation: Option<(crate::SceneAnimationId, crate::TransitionProperty)>,
    scene_text_raster_scale: f32,
    element_visual_transform: ElementVisualTransform,
    content_mask_stack: Vec<ContentMask<Pixels>>,
    visual_content_mask_stack: Vec<ContentMask<Pixels>>,
    image_cache_stack: Vec<AnyImageCache>,
}

/// Holds the state for a specific window.
pub struct Window {
    pub(crate) handle: AnyWindowHandle,
    pub(crate) invalidator: WindowInvalidator,
    pub(crate) removed: bool,
    pub(crate) platform_window: Box<dyn PlatformWindow>,
    pub(super) display_id: Option<DisplayId>,
    pub(super) sprite_atlas: Arc<dyn PlatformAtlas>,
    pub(super) text_system: Arc<WindowTextSystem>,
    pub(super) text_rendering_mode: std::rc::Rc<std::cell::Cell<crate::TextRenderingMode>>,
    pub(super) default_text_style: TextStyle,
    pub(super) image_pipeline_config: ImagePipelineConfig,
    pub(super) trim_memory_on_hidden: bool,
    pub(super) rem_size: Pixels,
    /// The stack of override values for the window's rem size.
    ///
    /// This is used by `with_rem_size` to allow rendering an element tree with
    /// a given rem size.
    pub(super) rem_size_override_stack: SmallVec<[Pixels; 8]>,
    pub(crate) viewport_size: Size<Pixels>,
    pub(super) layout_engine: Option<TaffyLayoutEngine>,
    pub(crate) root: Option<AnyView>,
    pub(crate) element_id_stack: SmallVec<[ElementId; 32]>,
    /// Retained rendering identity path. This intentionally differs from `element_id_stack`:
    /// anonymous elements receive internal `InstanceSlot` segments here but still receive no
    /// `GlobalElementId` for state storage.
    pub(crate) retained_element_id_stack: SmallVec<[ElementId; 32]>,
    /// Parent-local positional counters used only while `request_layout` constructs retained IDs.
    pub(crate) retained_child_slot_stack: SmallVec<[u32; 32]>,
    pub(crate) text_style_stack: Vec<TextStyleRefinement>,
    pub(crate) rendered_entity_stack: Vec<EntityId>,
    /// Views whose rendered output has read [`Window::viewport_size`]. A content resize dirties
    /// only these views; cached siblings whose bounds and dependencies stay unchanged remain
    /// eligible for retained replay.
    pub(crate) viewport_dependent_views: RefCell<FxHashSet<EntityId>>,
    /// Per-view bounds accumulation frames for the paint phase, parallel to the painted views
    /// in `rendered_entity_stack`. Child views fold their already computed bounds into their
    /// parent's frame so each scene operation is scanned exactly once regardless of nesting
    /// depth when computing retained scene segment bounds.
    pub(crate) view_bounds_stack: Vec<ViewBoundsFrame>,
    pub(crate) element_offset_stack: Vec<Point<Pixels>>,
    pub(crate) element_opacity: f32,
    pub(crate) scene_animation: Option<(crate::SceneAnimationId, crate::TransitionProperty)>,
    /// Additional glyph raster scale reserved for renderer-owned visual transforms.
    ///
    /// The scene still paints at its stable layout size; the larger atlas tile is sampled down
    /// until the GPU animation reaches its largest declared scale.
    pub(crate) scene_text_raster_scale: f32,
    pub(crate) element_visual_transform: ElementVisualTransform,
    pub(crate) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(crate) visual_content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(crate) requested_autoscroll: Option<Bounds<Pixels>>,
    pub(crate) image_cache_stack: Vec<AnyImageCache>,
    pub(super) animated_image_slots: FxHashMap<AnimatedImageSlotKey, usize>,
    pub(super) image_paint_tile_cache: FxHashMap<ImagePaintTileCacheKey, AtlasTile>,
    /// Reused liveness set for static GPU image tiles. Entries are derived from the current and
    /// previous committed retained scenes, not from CPU paint calls, so retained replay remains
    /// authoritative.
    pub(super) image_paint_live_tiles_scratch: FxHashSet<(crate::AtlasTextureId, u32)>,
    pub(crate) rendered_frame: Frame,
    pub(crate) next_frame: Frame,
    pub(super) render_dirty_region: DirtyRegion,
    pub(super) animation_dirty_region: DirtyRegion,
    pub(super) render_present_mode: PartialPresentMode,
    pub(super) render_trim_policy: RetainedResourceTrimPolicy,
    pub(super) backdrop_blur_damage_plan: BackdropBlurDamagePlan,
    pub(super) force_full_redraw: Cell<bool>,
    pub(super) force_view_cache_refresh: bool,
    pub(super) idle_render_frames: u16,
    pub(super) next_hitbox_id: HitboxId,
    pub(crate) next_tooltip_id: TooltipId,
    pub(crate) tooltip_bounds: Option<TooltipBounds>,
    pub(super) next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>>,
    pub(crate) dirty_views: FxHashSet<EntityId>,
    pub(crate) direct_dirty_views: FxHashSet<EntityId>,
    pub(super) focus_listeners: SubscriberSet<(), AnyWindowFocusListener>,
    pub(crate) focus_lost_listeners: SubscriberSet<(), AnyObserver>,
    pub(super) default_prevented: bool,
    pub(super) mouse_position: Point<Pixels>,
    pub(super) mouse_hit_test: HitTest,
    pub(super) modifiers: Modifiers,
    pub(super) capslock: Capslock,
    pub(super) scale_factor: f32,
    pub(crate) bounds_observers: SubscriberSet<(), AnyObserver>,
    pub(super) appearance: WindowAppearance,
    pub(crate) appearance_observers: SubscriberSet<(), AnyObserver>,
    pub(super) active: Rc<Cell<bool>>,
    pub(super) hovered: Rc<Cell<bool>>,
    pub(crate) needs_present: Rc<Cell<bool>>,
    pub(crate) last_input_timestamp: Rc<Cell<Instant>>,
    pub(super) animation_time: Cell<Instant>,
    pub(crate) refreshing: bool,
    pub(super) dirty_frame_scheduled: bool,
    pub(super) dirty_frame_throttle_pending: bool,
    pub(super) dirty_frame_deferred_pending: bool,
    /// Optional per-window override for visible inactive dirty redraw pacing.
    ///
    /// Ordinary windows keep the framework default. Tool and diagnostic windows may opt into a
    /// tighter interval without changing the process-wide inactive-window policy.
    pub(super) inactive_dirty_frame_retry_interval: Option<Duration>,
    /// Allows a visible inactive window to publish ordinary dirty redraws without first entering
    /// the background defer/retry path. Minimized windows still use the normal deferred policy.
    pub(super) inactive_dirty_redraw_enabled: bool,
    pub(super) async_app: AsyncApp,
    pub(super) frame_watchdog: Rc<Cell<FrameWatchdog>>,
    pub(super) platform_frame_watchdog_task: RefCell<Option<Task<()>>>,
    /// Pending delayed memory trim scheduled when the window loses focus; dropped (and thereby
    /// cancelled) when the window becomes active again before the delay elapses.
    pub(super) deactivation_trim_task: Option<Task<()>>,
    pub(super) frame_throttle: WindowFrameThrottle,
    pub(super) draw_deadline: Option<Instant>,
    pub(super) draw_was_degraded: bool,
    pub(super) recovering_degraded_draw: bool,
    pub(super) degraded_draw_count: u64,
    pub(super) recovery_full_redraw_count: u64,
    pub(super) last_generation_stats: FrameGenerationStats,
    pub(super) dirty_frame_diagnostics: Rc<RefCell<DirtyFrameDiagnostics>>,
    pub(super) pending_list_measured_items: usize,
    pub(super) has_completed_rendered_frame: bool,
    pub(super) critical_draw_depth: usize,
    pub(super) inactive_animation_frame_pending: Rc<Cell<bool>>,
    pub(super) last_inactive_animation_frame: Rc<Cell<Option<Instant>>>,
    pub(super) animation_frame_pending_entities: Rc<RefCell<FxHashSet<EntityId>>>,
    pub(super) animation_engine: Rc<RefCell<AnimationEngine>>,
    pub(super) animation_engine_frame_driver: Cell<Option<AnimationDriver>>,
    pub(super) animation_engine_frame_deadline:
        Rc<Cell<Option<(Instant, u64, AnimationDriver)>>>,
    pub(super) animation_engine_frame_deadline_generation: Rc<Cell<u64>>,
    /// Explicit opt-in for visible NOACTIVATE/panel windows whose retained scene animations must
    /// keep presenting while the OS does not consider the window active. Minimized windows still
    /// stop animation work regardless of this flag.
    pub(super) inactive_animation_engine_enabled: bool,
    pub(super) next_scene_animation_id: Cell<u32>,
    pub(super) image_animation_deadline_pending: Rc<RefCell<FxHashMap<EntityId, (Instant, u64)>>>,
    pub(super) deadline_invalidation_pending: Rc<RefCell<FxHashMap<EntityId, (Instant, u64)>>>,
    pub(super) deadline_invalidation_generation: Rc<Cell<u64>>,
    pub(super) image_animation_deadline_generation: Rc<Cell<u64>>,
    pub(crate) activation_observers: SubscriberSet<(), AnyObserver>,
    pub(crate) focus: Option<FocusId>,
    /// Persistent focus-to-retained-boundary lookup used to avoid `focus()/blur() -> refresh()`.
    pub(crate) focus_retained_targets: FxHashMap<FocusId, FocusRetainedTarget>,
    pub(super) focus_enabled: bool,
    pub(super) pending_input: Option<PendingInput>,
    pub(super) pending_modifier: ModifierState,
    pub(crate) pending_input_observers: SubscriberSet<(), AnyObserver>,
    pub(super) prompt: Option<RenderablePromptHandle>,
    pub(crate) client_inset: Option<Pixels>,
    pub(super) window_control_drag_gesture: TitlebarGesture,
    pub(super) transparent_caption_enabled: bool,
    pub(super) transparent_caption_height: Option<Pixels>,
    pub(super) observed_caption_height: Option<Pixels>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(super) inspector: Option<Entity<Inspector>>,
}

impl Window {
    pub(crate) fn capture_cached_view_traversal_context(&self) -> CachedViewTraversalContext {
        CachedViewTraversalContext {
            element_id_stack: self.element_id_stack.clone(),
            retained_element_id_stack: self.retained_element_id_stack.clone(),
            retained_child_slot_stack: self.retained_child_slot_stack.clone(),
            text_style_stack: self.text_style_stack.clone(),
            rem_size_override_stack: self.rem_size_override_stack.clone(),
            element_offset_stack: self.element_offset_stack.clone(),
            element_opacity: self.element_opacity,
            scene_animation: self.scene_animation,
            scene_text_raster_scale: self.scene_text_raster_scale,
            element_visual_transform: self.element_visual_transform,
            content_mask_stack: self.content_mask_stack.clone(),
            visual_content_mask_stack: self.visual_content_mask_stack.clone(),
            image_cache_stack: self.image_cache_stack.clone(),
        }
    }

    pub(crate) fn with_cached_view_traversal_context<R>(
        &mut self,
        context: &CachedViewTraversalContext,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let previous = self.capture_cached_view_traversal_context();

        self.element_id_stack.clone_from(&context.element_id_stack);
        self.retained_element_id_stack
            .clone_from(&context.retained_element_id_stack);
        self.retained_child_slot_stack
            .clone_from(&context.retained_child_slot_stack);
        self.text_style_stack.clone_from(&context.text_style_stack);
        self.rem_size_override_stack
            .clone_from(&context.rem_size_override_stack);
        self.element_offset_stack
            .clone_from(&context.element_offset_stack);
        self.element_opacity = context.element_opacity;
        self.scene_animation = context.scene_animation;
        self.scene_text_raster_scale = context.scene_text_raster_scale;
        self.element_visual_transform = context.element_visual_transform;
        self.content_mask_stack.clone_from(&context.content_mask_stack);
        self.visual_content_mask_stack
            .clone_from(&context.visual_content_mask_stack);
        self.image_cache_stack.clone_from(&context.image_cache_stack);

        let result = f(self);

        self.element_id_stack = previous.element_id_stack;
        self.retained_element_id_stack = previous.retained_element_id_stack;
        self.retained_child_slot_stack = previous.retained_child_slot_stack;
        self.text_style_stack = previous.text_style_stack;
        self.rem_size_override_stack = previous.rem_size_override_stack;
        self.element_offset_stack = previous.element_offset_stack;
        self.element_opacity = previous.element_opacity;
        self.scene_animation = previous.scene_animation;
        self.scene_text_raster_scale = previous.scene_text_raster_scale;
        self.element_visual_transform = previous.element_visual_transform;
        self.content_mask_stack = previous.content_mask_stack;
        self.visual_content_mask_stack = previous.visual_content_mask_stack;
        self.image_cache_stack = previous.image_cache_stack;

        result
    }

    pub(crate) fn record_list_measured_items(&mut self, count: usize) {
        self.pending_list_measured_items = self.pending_list_measured_items.saturating_add(count);
    }

    pub(crate) fn recovering_degraded_draw(&self) -> bool {
        self.recovering_degraded_draw
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

    /// Close this window.
    pub fn remove_window(&mut self) {
        self.removed = true;
    }
}
