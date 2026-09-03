use super::state::ElementVisualTransform;
use super::*;
use std::borrow::Borrow;

pub(crate) struct DeferredDraw {
    pub(super) current_view: EntityId,
    pub(super) priority: usize,
    pub(super) parent_node: DispatchNodeId,
    pub(super) element_id_stack: SmallVec<[ElementId; 32]>,
    pub(super) retained_element_id_stack: SmallVec<[ElementId; 32]>,
    pub(super) text_style_stack: Vec<TextStyleRefinement>,
    pub(super) element_visual_transform: ElementVisualTransform,
    pub(super) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(super) visual_content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(super) element: Option<AnyElement>,
    pub(super) absolute_offset: Point<Pixels>,
    pub(super) prepaint_range: Range<PrepaintStateIndex>,
    pub(super) paint_range: Range<PaintIndex>,
}

/// Source ranges required to rebase one retained deferred subtree into the next frame.
#[derive(Clone)]
pub(crate) struct DeferredRetainedReplay {
    pub(super) prepaint_range: Range<PrepaintStateIndex>,
    pub(super) paint_range: Range<PaintIndex>,
    pub(super) metadata_range: Range<usize>,
}

/// Reconciliation metadata kept in a parallel vector indexed exactly like `Frame::deferred_draws`.
///
/// Keeping this outside [`DeferredDraw`] avoids growing the hot deferred draw descriptor used by
/// normal non-replayed overlays. Slots are materialized lazily before retained extension/deferred
/// processing, so ordinary `defer_draw` calls do not perform extra initialization work.
#[derive(Clone, Default)]
pub(crate) struct DeferredRetainedMetadata {
    pub(super) metadata_range: Range<usize>,
    pub(super) replay_source: Option<DeferredRetainedReplay>,
}

#[derive(Clone)]
#[allow(dead_code)]
pub(crate) struct RetainedSceneSegment {
    pub(crate) bounds: Bounds<ScaledPixels>,
    pub(crate) scene_range: Range<usize>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) entity_id: EntityId,
}

/// Structural identity used exclusively by retained rendering reconciliation.
///
/// Keeping this distinct from [`GlobalElementId`] prevents positional anonymous-element slots
/// from accidentally becoming state-bearing application IDs while still allowing borrowed lookup
/// by the structural path representation already produced by the lifecycle traversal.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub(crate) struct ReconcileKey(GlobalElementId);

impl From<GlobalElementId> for ReconcileKey {
    fn from(value: GlobalElementId) -> Self {
        Self(value)
    }
}

impl Borrow<GlobalElementId> for ReconcileKey {
    fn borrow(&self) -> &GlobalElementId {
        &self.0
    }
}

/// Retained lifecycle ranges for one stable rendering identity path.
///
/// `metadata_range` points into `Frame::retained_element_order`. Entries are emitted in post-order,
/// so one subtree occupies one contiguous span ending in its own [`ReconcileKey`]. This lets a
/// replayed subtree carry all descendant reconciliation metadata into the next frame without a
/// full-map prefix scan.
#[derive(Clone)]
pub(crate) struct RetainedElementRange {
    pub(crate) bounds: Bounds<Pixels>,
    pub(crate) prepaint_range: Range<PrepaintStateIndex>,
    pub(crate) paint_range: Range<PaintIndex>,
    pub(crate) metadata_range: Range<usize>,
}

pub(crate) struct Frame {
    pub(crate) focus: Option<FocusId>,
    pub(crate) window_active: bool,
    pub(crate) element_states: FxHashMap<(GlobalElementId, TypeId), ElementStateBox>,
    pub(super) accessed_element_states: Vec<(GlobalElementId, TypeId)>,
    pub(crate) mouse_listeners: Vec<MouseListener>,
    pub(crate) dispatch_tree: DispatchTree,
    pub(crate) scene: Scene,
    pub(crate) hitboxes: Vec<Hitbox>,
    pub(crate) window_control_hitboxes: Vec<(WindowControlArea, Hitbox)>,
    pub(crate) deferred_draws: Vec<DeferredDraw>,
    pub(crate) deferred_retained_metadata: Vec<DeferredRetainedMetadata>,
    pub(crate) input_handlers: Vec<Option<PlatformInputHandler>>,
    pub(crate) tooltip_requests: Vec<Option<TooltipRequest>>,
    pub(crate) cursor_styles: Vec<CursorStyleRequest>,
    pub(crate) retained_scene_segments: Vec<RetainedSceneSegment>,
    pub(crate) retained_element_ranges: FxHashMap<ReconcileKey, RetainedElementRange>,
    pub(crate) retained_element_order: Vec<ReconcileKey>,
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
pub(super) const WINDOW_LIGHT_TRIM_IDLE_FRAMES: u16 = 300;
pub(super) const WINDOW_STRONG_TRIM_IDLE_FRAMES: u16 = 900;
pub(super) const DIRTY_REGION_FULL_REDRAW_RATIO: f32 = 0.6;

#[derive(Clone, Default)]
pub(crate) struct PrepaintStateIndex {
    pub(super) hitboxes_index: usize,
    pub(super) tooltips_index: usize,
    pub(super) deferred_draws_index: usize,
    pub(super) dispatch_tree_index: usize,
    pub(super) accessed_element_states_index: usize,
    pub(super) line_layout_index: LineLayoutIndex,
}

impl PrepaintStateIndex {
    pub(crate) fn rebased_from(&self, source: &Self, target: &Self) -> Option<Self> {
        Some(Self {
            hitboxes_index: rebase_index(self.hitboxes_index, source.hitboxes_index, target.hitboxes_index)?,
            tooltips_index: rebase_index(self.tooltips_index, source.tooltips_index, target.tooltips_index)?,
            deferred_draws_index: rebase_index(
                self.deferred_draws_index,
                source.deferred_draws_index,
                target.deferred_draws_index,
            )?,
            dispatch_tree_index: rebase_index(
                self.dispatch_tree_index,
                source.dispatch_tree_index,
                target.dispatch_tree_index,
            )?,
            accessed_element_states_index: rebase_index(
                self.accessed_element_states_index,
                source.accessed_element_states_index,
                target.accessed_element_states_index,
            )?,
            line_layout_index: self
                .line_layout_index
                .rebased_from(&source.line_layout_index, &target.line_layout_index)?,
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct PaintIndex {
    pub(super) scene_index: usize,
    pub(super) mouse_listeners_index: usize,
    pub(super) input_handlers_index: usize,
    pub(super) cursor_styles_index: usize,
    pub(super) window_control_hitboxes_index: usize,
    pub(super) accessed_element_states_index: usize,
    pub(super) tab_handle_index: usize,
    pub(super) line_layout_index: LineLayoutIndex,
}

impl PaintIndex {
    pub(crate) fn rebased_from(&self, source: &Self, target: &Self) -> Option<Self> {
        Some(Self {
            scene_index: rebase_index(self.scene_index, source.scene_index, target.scene_index)?,
            mouse_listeners_index: rebase_index(
                self.mouse_listeners_index,
                source.mouse_listeners_index,
                target.mouse_listeners_index,
            )?,
            input_handlers_index: rebase_index(
                self.input_handlers_index,
                source.input_handlers_index,
                target.input_handlers_index,
            )?,
            cursor_styles_index: rebase_index(
                self.cursor_styles_index,
                source.cursor_styles_index,
                target.cursor_styles_index,
            )?,
            window_control_hitboxes_index: rebase_index(
                self.window_control_hitboxes_index,
                source.window_control_hitboxes_index,
                target.window_control_hitboxes_index,
            )?,
            accessed_element_states_index: rebase_index(
                self.accessed_element_states_index,
                source.accessed_element_states_index,
                target.accessed_element_states_index,
            )?,
            tab_handle_index: rebase_index(
                self.tab_handle_index,
                source.tab_handle_index,
                target.tab_handle_index,
            )?,
            line_layout_index: self
                .line_layout_index
                .rebased_from(&source.line_layout_index, &target.line_layout_index)?,
        })
    }
}

fn rebase_index(value: usize, source: usize, target: usize) -> Option<usize> {
    target.checked_add(value.checked_sub(source)?)
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
            deferred_retained_metadata: Vec::new(),
            input_handlers: Vec::new(),
            tooltip_requests: Vec::new(),
            cursor_styles: Vec::new(),
            retained_scene_segments: Vec::new(),
            retained_element_ranges: FxHashMap::default(),
            retained_element_order: Vec::new(),

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
        self.retained_element_ranges.clear();
        self.retained_element_order.clear();
        self.hitboxes.clear();
        self.window_control_hitboxes.clear();
        self.deferred_draws.clear();
        self.deferred_retained_metadata.clear();
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

        self.scene.finish_retaining_revision(&prev_frame.scene);
    }

    pub(super) fn retained_capacity(&self) -> usize {
        self.element_states.capacity()
            + self.accessed_element_states.capacity()
            + self.dispatch_tree.retained_capacity()
            + self.mouse_listeners.capacity()
            + self.hitboxes.capacity()
            + self.window_control_hitboxes.capacity()
            + self.deferred_draws.capacity()
            + self.deferred_retained_metadata.capacity()
            + self.input_handlers.capacity()
            + self.tooltip_requests.capacity()
            + self.cursor_styles.capacity()
            + self.retained_scene_segments.capacity()
            + self.retained_element_ranges.capacity()
            + self.retained_element_order.capacity()
            + self.debug_container_capacity()
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
            &mut self.deferred_retained_metadata,
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
        trim_frame_vec_capacity(
            &mut self.retained_element_order,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        if self.retained_element_ranges.capacity()
            > FRAME_MIN_RETAINED_CAPACITY.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.retained_element_ranges
                .shrink_to(FRAME_MIN_RETAINED_CAPACITY.max(self.retained_element_ranges.len()));
        }
    }

    pub(super) fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
        match level {
            GpuiMemoryTrimLevel::Light => self.trim_retained_capacity(),
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive => {
                let floor = if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
                    0
                } else {
                    FRAME_MIN_RETAINED_CAPACITY
                };
                self.element_states
                    .shrink_to(floor.max(self.element_states.len()));
                self.accessed_element_states
                    .shrink_to(floor.max(self.accessed_element_states.len()));
                self.dispatch_tree
                    .trim_retained_capacity(matches!(level, GpuiMemoryTrimLevel::Aggressive));
                self.mouse_listeners.shrink_to(floor);
                self.hitboxes.shrink_to(floor);
                self.window_control_hitboxes.shrink_to(floor);
                self.deferred_draws.shrink_to(floor);
                self.deferred_retained_metadata.shrink_to(floor);
                self.input_handlers.shrink_to(floor);
                self.tooltip_requests.shrink_to(floor);
                self.cursor_styles.shrink_to(floor);
                self.retained_scene_segments.shrink_to(floor);
                self.retained_element_ranges
                    .shrink_to(floor.max(self.retained_element_ranges.len()));
                self.retained_element_order.shrink_to(floor);
                #[cfg(any(test, feature = "test-support"))]
                self.debug_bounds
                    .shrink_to(floor.max(self.debug_bounds.len()));
                #[cfg(any(feature = "inspector", debug_assertions))]
                {
                    self.next_inspector_instance_ids
                        .shrink_to(floor.max(self.next_inspector_instance_ids.len()));
                    self.inspector_hitboxes
                        .shrink_to(floor.max(self.inspector_hitboxes.len()));
                }
            }
        }
    }

    fn debug_container_capacity(&self) -> usize {
        let capacity = 0;
        #[cfg(any(test, feature = "test-support"))]
        let capacity = capacity + self.debug_bounds.capacity();
        #[cfg(any(feature = "inspector", debug_assertions))]
        let capacity = capacity
            + self.next_inspector_instance_ids.capacity()
            + self.inspector_hitboxes.capacity();
        capacity
    }

    pub(super) fn release_image_element_bitmaps(&mut self) {
        let image_state_type = TypeId::of::<crate::ImageElementState>();
        for ((_, type_id), state) in &mut self.element_states {
            if *type_id != image_state_type {
                continue;
            }
            let Some(state) = state
                .inner
                .downcast_mut::<Option<crate::ImageElementState>>()
                .and_then(Option::as_mut)
            else {
                continue;
            };
            state.current_image = None;
            state.current_frame = None;
        }
    }
}

fn trim_frame_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    if vec.capacity() > floor.saturating_mul(multiplier) {
        vec.shrink_to(floor);
    }
}
