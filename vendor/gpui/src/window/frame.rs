use super::state::ElementVisualTransform;
use super::*;

pub(crate) struct DeferredDraw {
    pub(super) current_view: EntityId,
    pub(super) priority: usize,
    pub(super) parent_node: DispatchNodeId,
    pub(super) element_id_stack: SmallVec<[ElementId; 32]>,
    pub(super) text_style_stack: Vec<TextStyleRefinement>,
    pub(super) element_visual_transform: ElementVisualTransform,
    pub(super) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(super) visual_content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(super) element: Option<AnyElement>,
    pub(super) absolute_offset: Point<Pixels>,
    pub(super) prepaint_range: Range<PrepaintStateIndex>,
    pub(super) paint_range: Range<PaintIndex>,
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
    pub(super) accessed_element_states: Vec<(GlobalElementId, TypeId)>,
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

    pub(super) fn retained_capacity(&self) -> usize {
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

    pub(super) fn trim_retained_capacity_for_level(&mut self, level: GpuiMemoryTrimLevel) {
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
    }
}

fn trim_frame_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    if vec.capacity() > floor.saturating_mul(multiplier) {
        vec.shrink_to(floor);
    }
}
