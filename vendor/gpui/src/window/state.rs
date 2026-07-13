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
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct DirtyFrameDiagnostics {
    pub(super) refreshes: usize,
    pub(super) view_dirty: usize,
    pub(super) notify_invalidations: usize,
    pub(super) first_view_dirty_entity: Option<EntityId>,
    pub(super) first_notify_entity: Option<EntityId>,
}

impl DirtyFrameDiagnostics {
    pub(super) fn record_refresh(&mut self) {
        self.refreshes = self.refreshes.saturating_add(1);
    }

    pub(super) fn record_view_dirty(&mut self, entity_id: EntityId) {
        self.view_dirty = self.view_dirty.saturating_add(1);
        self.first_view_dirty_entity.get_or_insert(entity_id);
    }

    pub(super) fn record_notify_invalidation(&mut self, entity_id: EntityId) {
        self.notify_invalidations = self.notify_invalidations.saturating_add(1);
        self.first_notify_entity.get_or_insert(entity_id);
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
    pub(super) pixel_format: RenderImagePixelFormat,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DispatchEventResult {
    pub propagate: bool,
    pub default_prevented: bool,
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
    pub(crate) text_style_stack: Vec<TextStyleRefinement>,
    pub(crate) rendered_entity_stack: Vec<EntityId>,
    pub(crate) element_offset_stack: Vec<Point<Pixels>>,
    pub(crate) element_opacity: f32,
    pub(crate) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(crate) requested_autoscroll: Option<Bounds<Pixels>>,
    pub(crate) image_cache_stack: Vec<AnyImageCache>,
    pub(super) animated_image_slots: FxHashMap<AnimatedImageSlotKey, usize>,
    pub(super) image_paint_tile_cache: FxHashMap<ImagePaintTileCacheKey, AtlasTile>,
    pub(crate) rendered_frame: Frame,
    pub(crate) next_frame: Frame,
    pub(super) render_dirty_region: DirtyRegion,
    pub(super) animation_dirty_region: DirtyRegion,
    pub(super) render_present_mode: PartialPresentMode,
    pub(super) render_trim_policy: RetainedResourceTrimPolicy,
    pub(super) force_full_redraw: Cell<bool>,
    pub(super) force_view_cache_refresh: bool,
    pub(super) idle_render_frames: u16,
    pub(super) next_hitbox_id: HitboxId,
    pub(crate) next_tooltip_id: TooltipId,
    pub(crate) tooltip_bounds: Option<TooltipBounds>,
    pub(super) next_frame_callbacks: Rc<RefCell<Vec<FrameCallback>>>,
    pub(crate) dirty_views: FxHashSet<EntityId>,
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
    pub(super) async_app: AsyncApp,
    pub(super) frame_watchdog: Rc<Cell<FrameWatchdog>>,
    pub(super) platform_frame_watchdog_task: Option<Task<()>>,
    pub(super) frame_throttle: WindowFrameThrottle,
    pub(super) draw_deadline: Option<Instant>,
    pub(super) draw_was_degraded: bool,
    pub(super) recovering_degraded_draw: bool,
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
    pub(super) image_animation_deadline_pending: Rc<RefCell<FxHashMap<EntityId, (Instant, u64)>>>,
    pub(super) deadline_invalidation_pending: Rc<RefCell<FxHashMap<EntityId, (Instant, u64)>>>,
    pub(super) deadline_invalidation_generation: Rc<Cell<u64>>,
    pub(super) image_animation_deadline_generation: Rc<Cell<u64>>,
    pub(crate) activation_observers: SubscriberSet<(), AnyObserver>,
    pub(crate) focus: Option<FocusId>,
    pub(super) focus_enabled: bool,
    pub(super) pending_input: Option<PendingInput>,
    pub(super) pending_modifier: ModifierState,
    pub(crate) pending_input_observers: SubscriberSet<(), AnyObserver>,
    pub(super) prompt: Option<RenderablePromptHandle>,
    pub(crate) client_inset: Option<Pixels>,
    pub(super) window_control_drag_gesture: TitlebarGestureState,
    pub(super) transparent_caption_enabled: bool,
    pub(super) transparent_caption_height: Option<Pixels>,
    pub(super) observed_caption_height: Option<Pixels>,
    #[cfg(any(feature = "inspector", debug_assertions))]
    pub(super) inspector: Option<Entity<Inspector>>,
}

impl Window {
    pub(crate) fn record_list_measured_items(&mut self, count: usize) {
        self.pending_list_measured_items = self.pending_list_measured_items.saturating_add(count);
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
