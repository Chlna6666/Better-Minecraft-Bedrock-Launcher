use super::state::ElementVisualTransform;
use super::*;

mod paint;
mod state_access;
mod retained_scene_index;
mod layers;
pub(crate) use layers::{LayerFrame, SceneLayers};
pub(super) use layers::{StaticPaintOrder, next_layer_id, static_scroll_raster_operation};
pub(crate) use paint::{Paint, PaintGeometry};
use retained_scene_index::Entry;

pub(crate) struct DeferredDraw {
    pub(super) current_view: EntityId,
    pub(super) priority: usize,
    pub(super) parent_node: DispatchNodeId,
    pub(super) element_prefix: Option<GlobalElementId>,
    pub(super) text_style_stack: crate::fast::text_style::TextStyleStack,
    pub(super) element_visual_transform: ElementVisualTransform,
    pub(super) content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(super) visual_content_mask_stack: Vec<ContentMask<Pixels>>,
    pub(super) element: Option<AnyElement>,
    pub(super) absolute_offset: Point<Pixels>,
    pub(super) prepaint_range: Range<PrepaintStateIndex>,
    pub(super) paint_range: Range<PaintIndex>,
}

#[derive(Clone)]
pub(crate) struct RetainedSceneSegment {
    pub(crate) bounds: Bounds<ScaledPixels>,
    pub(crate) scene_range: Range<usize>,
    pub(crate) entity_id: EntityId,
    next_for_entity: Option<std::num::NonZeroUsize>,
}

impl RetainedSceneSegment {
    pub(crate) fn new(entity_id: EntityId, bounds: Bounds<ScaledPixels>, scene_range: Range<usize>) -> Self {
        Self { entity_id, bounds, scene_range, next_for_entity: None }
    }
}

/// Inherited state that changes either lifecycle side effects or the pixels produced by an
/// otherwise identical retained element. Targeted reconciliation tracks the changed element path;
/// this key lets unaffected descendants/siblings prove that their logical clip, visual clip, text
/// inheritance and transforms are still identical before replaying old prepaint/paint ranges.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RetainedPaintContext {
    /// Scene primitives are already in device pixels; a DPI transition cannot replay them.
    pub(crate) device_scale_factor: f32,
    pub(crate) opacity: f32,
    pub(crate) scale: f32,
    pub(crate) translation: Point<Pixels>,
    pub(crate) element_offset: Point<Pixels>,
    pub(crate) text_raster_scale: f32,
    pub(crate) content_mask: ContentMask<Pixels>,
    pub(crate) visual_content_mask: ContentMask<Pixels>,
    pub(crate) text_style: TextStyle,
    pub(crate) rem_size: Pixels,
}

/// A scroll container's contiguous input records in the committed frame.
/// Ranges close inside-out and are ordered by end, like scene captures.
pub(super) struct LayerInput {
    pub(super) hitboxes: Range<usize>,
    viewport: Bounds<Pixels>,
    /// The original scroll placement is retained only if no descendant
    /// introduced a moving clip or replayed input of unknown provenance.
    geometry: Option<ScrollInputGeometry>,
}

#[derive(Clone, Copy)]
struct ScrollInputGeometry {
    offset: Point<Pixels>,
    visual_scale: f32,
    viewport: ContentMask<Pixels>,
}

pub(crate) struct Frame {
    pub(crate) focus: Option<FocusId>,
    pub(crate) window_active: bool,
    pub(crate) element_states: FxHashMap<(GlobalElementId, TypeId), ElementStateBox>,
    // Tombstones cancel speculative layout-state replay without shifting frame ranges.
    pub(super) accessed_element_states: state_access::StateAccessLog,
    pub(crate) mouse_listeners: Vec<MouseListener>,
    pub(super) mounted_scroll_sources: Vec<super::scroll_dependencies::MountedScrollSource>,
    /// Whether the completed frame contains an application listener that needs every MouseMove
    /// sample, even when the hit-test result did not change. Computed once when the frame finishes
    /// so high-report-rate mice do not scan the full listener array for every hardware event.
    pub(crate) has_continuous_mouse_move_listener: bool,
    pub(crate) dispatch_tree: DispatchTree,
    pub(crate) painter: crate::scene::Painter,
    paint: Option<Arc<Paint>>,
    pub(crate) retained_paint: Arc<crate::window::retained::RetainedPaint>,
    pub(crate) hitboxes: Vec<Hitbox>,
    pub(super) layer_inputs: Vec<LayerInput>,
    pub(super) scroll_rows: super::scroll_rows::ScrollRows,
    pub(super) scroll_views: super::scroll_views::ScrollViews,
    pub(crate) input_clip_epoch: u64,
    pub(crate) window_control_hitboxes: Vec<(WindowControlArea, Hitbox)>,
    pub(crate) deferred_draws: Vec<DeferredDraw>,
    pub(crate) input_handlers: Vec<Option<PlatformInputHandler>>,
    /// Non-cloneable IME owners moved during speculative Paint replay.
    /// Records exist only for non-empty source slots.
    pub(crate) replayed_input_handlers: Vec<(usize, usize)>,
    pub(crate) tooltip_requests: Vec<Option<TooltipRequest>>,
    pub(crate) cursor_styles: Vec<CursorStyleRequest>,
    pub(crate) retained_scene_segments: Vec<RetainedSceneSegment>,
    pub(super) retained_scene_index: FxHashMap<EntityId, Entry>,
    /// Native key lookup; ambiguous keys use usize::MAX and cannot be admitted.
    /// The actual records remain in prepaint order.
    pub(crate) view_retentions: FxHashMap<u64, usize>,
    pub(crate) view_retention_records: Vec<super::view_retention::ViewRecordSlot>,
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) debug_bounds: FxHashMap<String, Bounds<Pixels>>,
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) debug_bounds_records: Vec<(String, Bounds<Pixels>)>,
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

#[derive(Clone, Default, PartialEq)]
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
            hitboxes_index: rebase_index(
                self.hitboxes_index,
                source.hitboxes_index,
                target.hitboxes_index,
            )?,
            tooltips_index: rebase_index(
                self.tooltips_index,
                source.tooltips_index,
                target.tooltips_index,
            )?,
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

#[derive(Clone, Default, PartialEq)]
pub(crate) struct PaintIndex {
    pub(super) scene_index: usize,
    #[cfg(any(test, feature = "test-support"))]
    pub(super) debug_bounds_index: usize,
    pub(super) mouse_listeners_index: usize,
    pub(super) input_handlers_index: usize,
    pub(super) cursor_styles_index: usize,
    pub(super) window_control_hitboxes_index: usize,
    pub(super) accessed_element_states_index: usize,
    pub(super) tab_handle_index: usize,
    pub(super) line_layout_index: LineLayoutIndex,
}

impl PaintIndex {
    #[cfg(test)]
    pub(crate) fn scene_index(&self) -> usize {
        self.scene_index
    }

    pub(crate) fn rebased_from(&self, source: &Self, target: &Self) -> Option<Self> {
        Some(Self {
            scene_index: rebase_index(self.scene_index, source.scene_index, target.scene_index)?,
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds_index: rebase_index(
                self.debug_bounds_index,
                source.debug_bounds_index,
                target.debug_bounds_index,
            )?,
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
    /// Borrows the sole Paint payload for this frame's current lifecycle phase.
    pub(crate) fn scene(&self) -> &Scene {
        self.paint
            .as_ref()
            .map_or(&self.painter.scene, |paint| &paint.scene)
    }

    /// Borrow a completed publication without allocating or cloning its Arc.
    pub(super) fn published_paint_ref(&self) -> Option<&Paint> {
        self.paint.as_deref()
    }

    /// Shares only a completed payload; mutable construction state never crosses this boundary.
    pub(crate) fn published_paint(&self) -> Arc<Paint> {
        match &self.paint {
            Some(paint) => Arc::clone(paint),
            None => {
                assert!(
                    self.painter.len() == 0,
                    "cannot publish an unfinished drawing"
                );
                Arc::new(Paint::new(
                    Scene::default(),
                    Arc::clone(&self.retained_paint),
                    PaintGeometry::default(),
                ))
            }
        }
    }

    /// Move finalized drawing storage into its sole publication, then share that publication.
    pub(crate) fn publish(
        &mut self, geometry: PaintGeometry, previous: Option<&Self>, window_opaque: bool,
    ) -> Arc<Paint> {
        assert!(self.paint.is_none(), "Paint may only be published once");
        let mut paint = Paint::new(
            self.painter.take_scene(),
            Arc::clone(&self.retained_paint),
            geometry,
        );
        paint.layers = SceneLayers::capture_against(
            &paint.scene,
            previous.and_then(|frame| frame.paint.as_ref()).map(|paint| (&paint.scene, &paint.layers)),
            window_opaque,
        );
        let paint = Arc::new(paint);
        self.paint = Some(Arc::clone(&paint));
        paint
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn record_debug_bounds(&mut self, selector: String, bounds: Bounds<Pixels>) {
        self.debug_bounds.insert(selector.clone(), bounds);
        self.debug_bounds_records.push((selector, bounds));
    }

    pub(crate) fn new(dispatch_tree: DispatchTree) -> Self {
        Frame {
            focus: None,
            window_active: false,
            element_states: FxHashMap::default(),
            accessed_element_states: state_access::StateAccessLog::default(),
            mouse_listeners: Vec::new(),
            mounted_scroll_sources: Vec::new(),
            has_continuous_mouse_move_listener: false,
            dispatch_tree,
            painter: crate::scene::Painter::default(),
            paint: None,
            retained_paint: Arc::default(),
            hitboxes: Vec::new(),
            layer_inputs: Vec::new(),
            scroll_rows: super::scroll_rows::ScrollRows::default(),
            scroll_views: super::scroll_views::ScrollViews::default(),
            input_clip_epoch: 0,
            window_control_hitboxes: Vec::new(),
            deferred_draws: Vec::new(),
            input_handlers: Vec::new(),
            replayed_input_handlers: Vec::new(),
            tooltip_requests: Vec::new(),
            cursor_styles: Vec::new(),
            retained_scene_segments: Vec::new(),
            retained_scene_index: FxHashMap::default(),
            view_retentions: FxHashMap::default(),
            view_retention_records: Vec::new(),

            #[cfg(any(test, feature = "test-support"))]
            debug_bounds: FxHashMap::default(),
            #[cfg(any(test, feature = "test-support"))]
            debug_bounds_records: Vec::new(),

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
        self.mounted_scroll_sources.clear();
        self.has_continuous_mouse_move_listener = false;
        self.dispatch_tree.clear();
        if let Some(paint) = self.paint.take() {
            self.painter.scene = Arc::try_unwrap(paint)
                .ok()
                .map(|paint| paint.scene)
                .unwrap_or_default();
        }
        self.painter.clear();
        if let Some(retained) = Arc::get_mut(&mut self.retained_paint) {
            retained.clear();
        } else {
            self.retained_paint = Arc::default();
        }
        self.input_handlers.clear();
        self.replayed_input_handlers.clear();
        self.tooltip_requests.clear();
        self.cursor_styles.clear();
        self.retained_scene_segments.clear();
        self.retained_scene_index.clear();
        self.view_retentions.clear();
        self.view_retention_records.clear();
        self.hitboxes.clear();
        self.layer_inputs.clear();
        self.scroll_rows.clear();
        self.scroll_views.clear();
        self.input_clip_epoch = 0;
        self.window_control_hitboxes.clear();
        self.deferred_draws.clear();
        self.tab_stops.clear();
        self.focus = None;

        #[cfg(any(test, feature = "test-support"))]
        {
            self.debug_bounds.clear();
            self.debug_bounds_records.clear();
        }

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
                Some(hitbox_id) => Continue(style.or_else(|| {
                    hitbox_id
                        .is_hovered_ignoring_last_input(window)
                        .then_some(request.style)
                })),
            })
            .into_inner()
    }

    /// Admit only a range whose actual input masks prove containment.
    /// Custom prepaint code and partially replayed ranges must not cause
    /// false-negative hit tests. Small ranges do not justify an index entry.
    pub(crate) fn record_layer_input(&mut self, start: usize, viewport: Bounds<Pixels>) {
        let end = self.hitboxes.len();
        if end.saturating_sub(start) < 8 {
            return;
        }
        let bottom_right = viewport.bottom_right();
        if self.hitboxes[start..end].iter().all(|hitbox| {
            let clip = hitbox.content_mask.bounds;
            let end = clip.bottom_right();
            clip.origin.x >= viewport.origin.x
                && clip.origin.y >= viewport.origin.y
                && end.x <= bottom_right.x
                && end.y <= bottom_right.y
        }) {
            self.layer_inputs.push(LayerInput {
                hitboxes: start..end,
                viewport,
                geometry: None,
            });
        }
    }

    /// Record a fixed-viewport scroll input span. The epoch deliberately
    /// rejects nested clip scopes, even if their current intersection equals
    /// the viewport: that equality may stop holding when content moves.
    pub(crate) fn record_scroll_layer_input(
        &mut self,
        start: usize,
        viewport: ContentMask<Pixels>,
        offset: Point<Pixels>,
        visual_scale: f32,
        clip_epoch: u64,
    ) {
        let before = self.layer_inputs.len();
        self.record_layer_input(start, viewport.bounds);
        if self.layer_inputs.len() != before
            && self.input_clip_epoch == clip_epoch
            && visual_scale.is_finite()
            && visual_scale > 0.0
            && self.hitboxes[start..].iter().all(|hitbox| hitbox.content_mask == viewport)
        {
            self.layer_inputs.last_mut().expect("recorded input").geometry =
                Some(ScrollInputGeometry { offset, visual_scale, viewport });
        }
    }

    /// Carry only complete input groups. A partial splice may still replay
    /// individual hitboxes, but cannot claim the enclosing viewport.
    pub(super) fn carry_layer_inputs(&mut self, previous: &Self, source: Range<usize>) {
        let target = self.hitboxes.len();
        let first = previous.layer_inputs.partition_point(|layer| layer.hitboxes.end <= source.start);
        self.layer_inputs.extend(previous.layer_inputs[first..].iter()
            .take_while(|layer| layer.hitboxes.end <= source.end)
            .filter(|layer| layer.hitboxes.start >= source.start)
            .map(|layer| LayerInput {
                hitboxes: target + (layer.hitboxes.start - source.start)
                    ..target + (layer.hitboxes.end - source.start),
                viewport: layer.viewport,
                geometry: layer.geometry,
            }));
    }

    /// Copy committed input without mutating its source. Nonzero placement
    /// is admitted only for one complete fixed-clip group. The caller must
    /// also prove scene coverage and refresh coordinate-capturing listeners
    /// before dispatching non-wheel input; this routine only moves geometry.
    pub(super) fn carry_input_span(
        &mut self,
        previous: &Self,
        source: Range<usize>,
        scroll_offset: Option<Point<Pixels>>,
    ) -> bool {
        let Some(hitboxes) = previous.hitboxes.get(source.clone()) else {
            return false;
        };
        if let Some(offset) = scroll_offset {
            let first = previous.layer_inputs.partition_point(|layer| layer.hitboxes.end < source.end);
            let mut matching = previous.layer_inputs[first..].iter()
                .take_while(|layer| layer.hitboxes.end == source.end)
                .filter(|layer| layer.hitboxes == source);
            let Some(layer) = matching.next() else { return false; };
            if matching.next().is_some() {
                return false;
            }
            let Some(geometry) = layer.geometry else { return false; };
            let delta = (offset - geometry.offset) * geometry.visual_scale;
            if !delta.x.0.is_finite() || !delta.y.0.is_finite() {
                return false;
            }
            if delta != Point::default() {
                let start = self.hitboxes.len();
                self.hitboxes.extend(hitboxes.iter().map(|hitbox| Hitbox {
                    id: hitbox.id,
                    bounds: Bounds::new(hitbox.bounds.origin + delta, hitbox.bounds.size),
                    content_mask: geometry.viewport,
                    behavior: hitbox.behavior,
                }));
                self.layer_inputs.push(LayerInput {
                    hitboxes: start..self.hitboxes.len(),
                    viewport: layer.viewport,
                    geometry: Some(ScrollInputGeometry { offset, ..geometry }),
                });
                self.input_clip_epoch = self.input_clip_epoch.wrapping_add(1);
                return true;
            }
        }
        // Normal retained prepaint uses the exact existing coordinates.
        // Its parent cannot infer raw mask provenance from a replayed span.
        if !hitboxes.is_empty() {
            self.input_clip_epoch = self.input_clip_epoch.wrapping_add(1);
        }
        self.carry_layer_inputs(previous, source);
        self.hitboxes.extend_from_slice(hitboxes);
        true
    }

    pub(crate) fn hit_test(&self, position: Point<Pixels>) -> HitTest {
        let mut set_hover_hitbox_count = false;
        let mut hit_test = HitTest::default();
        let mut cursor = self.hitboxes.len();
        let mut layers = self.layer_inputs.iter().rev().peekable();
        'hitboxes: while cursor > 0 {
            // Traverse ordinary spans in one pass, and skip a whole clipped
            // scroll range when the pointer cannot reach any of its children.
            let mut start = 0;
            while let Some(layer) = layers.peek() {
                if layer.hitboxes.end > cursor {
                    layers.next();
                } else if layer.hitboxes.end == cursor {
                    let layer = layers.next().expect("peeked layer");
                    if !layer.viewport.contains(&position) {
                        cursor = layer.hitboxes.start;
                        continue 'hitboxes;
                    }
                } else {
                    start = layer.hitboxes.end;
                    break;
                }
            }
            for hitbox in self.hitboxes[start..cursor].iter().rev() {
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
                        break 'hitboxes;
                    }
                }
            }
            cursor = start;
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

    /// Validates retained Fragment structure against this frame's immutable Paint generation.
    /// Structure is recorded with the authoritative operation stream.
    pub(crate) fn range_is_replayable(&self, range: Range<usize>) -> bool {
        crate::paint::retention::range_is_replayable(self.scene(), range)
    }

    pub(crate) fn finish(&mut self, prev_frame: &mut Self) {
        debug_assert!(
            self.retained_scene_index
                .values()
                .map(Entry::len)
                .sum::<usize>()
                == self.retained_scene_segments.len()
        );
        self.accessed_element_states.freeze();
        for element_state_key in self.accessed_element_states.keys(0..self.accessed_element_states.len()) {
            if prev_frame.element_states.is_empty() {
                break;
            }
            if let Some((element_state_key, element_state)) =
                prev_frame.element_states.remove_entry(element_state_key)
            {
                // Snapshot-based progressive frames keep the committed
                // element state in prev_frame during construction. Fresh
                // candidate state must win on successful publication.
                self.element_states
                    .entry(element_state_key)
                    .or_insert(element_state);
            }
        }

        // A complete Frame is committed atomically, so speculative IME
        // transfers no longer need an abort journal after publication.
        self.replayed_input_handlers.clear();
        self.has_continuous_mouse_move_listener = self
            .mouse_listeners
            .iter()
            .any(|listener| listener.handles(TypeId::of::<MouseMoveEvent>(), true));
        let same_paint = self.painter.finish_against(prev_frame.scene());
        let retention = Arc::get_mut(&mut self.retained_paint)
            .expect("unpublished paint retention");
        let previous_scene = prev_frame.scene();
        if previous_scene.revision == 0 {
            retention.finish(&mut self.painter.scene);
        } else {
            retention.finish_against(
                &mut self.painter.scene,
                previous_scene,
                &prev_frame.retained_paint,
                same_paint,
            );
        }
    }

    /// Abort a candidate generation without dropping IME handlers still
    /// owned by the last committed tree. The platform may already own the
    /// active handler; only nonempty speculative slots are restored.
    /// Keep committed snapshots, but migrate non-cloneable states moved by
    /// older Element implementations back from an aborted candidate.
    /// Never overwrite an intact committed element snapshot with an
    /// uncommitted range generation.
    pub(crate) fn rollback_element_states(&mut self, candidate: &mut Self) {
        for (key, state) in candidate.element_states.drain() {
            self.element_states.entry(key).or_insert(state);
        }
    }

    pub(crate) fn rollback_replayed_input_handlers(&mut self, candidate: &mut Self) -> usize {
        rollback_replayed_option_transfers(
            &mut self.input_handlers,
            &mut candidate.input_handlers,
            &mut candidate.replayed_input_handlers,
        )
    }

    pub(super) fn retained_capacity(&self) -> usize {
        self.element_states.capacity()
            + self.painter.construction_capacity()
            + self.retained_paint.retained_capacity()
            + self.accessed_element_states.capacity()
            + self.dispatch_tree.retained_capacity()
            + self.mouse_listeners.capacity()
            + self.mounted_scroll_sources.capacity()
            + self.hitboxes.capacity()
            + self.layer_inputs.capacity()
            + self.window_control_hitboxes.capacity()
            + self.deferred_draws.capacity()
            + self.input_handlers.capacity()
            + self.tooltip_requests.capacity()
            + self.cursor_styles.capacity()
            + self.retained_scene_segments.capacity()
            + self.retained_scene_index.capacity()
            + self.view_retentions.capacity()
            + self.view_retention_records.capacity()
            + self.tab_stops.retained_capacity()
            + self.debug_container_capacity()
    }

    fn trim_retained_capacity(&mut self) {
        let state_target = FRAME_MIN_RETAINED_CAPACITY.max(self.element_states.len());
        if self.element_states.capacity()
            > state_target.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.element_states.shrink_to(state_target);
        }
        self.accessed_element_states.trim(false);
        self.dispatch_tree.trim_retained_capacity(false);
        trim_frame_vec_capacity(
            &mut self.mouse_listeners,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_frame_vec_capacity(
            &mut self.mounted_scroll_sources,
            FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        trim_frame_vec_capacity(
            &mut self.layer_inputs,
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
        let input_handler_target = FRAME_MIN_RETAINED_CAPACITY.max(self.input_handlers.len());
        trim_frame_vec_capacity(
            &mut self.input_handlers,
            input_handler_target,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER,
        );
        let tooltip_request_target = FRAME_MIN_RETAINED_CAPACITY.max(self.tooltip_requests.len());
        trim_frame_vec_capacity(
            &mut self.tooltip_requests,
            tooltip_request_target,
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
        let index_target = FRAME_MIN_RETAINED_CAPACITY.max(self.retained_scene_index.len());
        if self.retained_scene_index.capacity()
            > index_target.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.retained_scene_index.shrink_to(index_target);
        }
        let view_target = FRAME_MIN_RETAINED_CAPACITY.max(self.view_retentions.len());
        if self.view_retentions.capacity() > view_target.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER) {
            self.view_retentions.shrink_to(view_target);
        }
        trim_frame_vec_capacity(&mut self.view_retention_records, FRAME_MIN_RETAINED_CAPACITY,
            FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER);
        self.tab_stops.trim_retained_capacity(false);
    }

    /// Clears this scratch frame and adapts its retained capacity to the frame that just won the
    /// swap. Stable large pages keep their hot allocations; a large-to-small page transition
    /// returns only capacity that is far above the new working set.
    pub(crate) fn clear_for_reuse(&mut self, current: &Self) {
        self.clear();
        self.scroll_rows.trim_for_reuse(&current.scroll_rows);
        self.painter
            .trim_for_reuse(&current.painter, current.scene());
        if let Some(retained) = Arc::get_mut(&mut self.retained_paint) {
            retained.trim_for_reuse(&current.retained_paint);
        }

        macro_rules! trim_vec_against {
            ($field:ident) => {{
                let target = FRAME_MIN_RETAINED_CAPACITY.max(current.$field.len());
                if self.$field.capacity()
                    > target.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER)
                {
                    self.$field.shrink_to(target);
                }
            }};
        }

        let state_target = FRAME_MIN_RETAINED_CAPACITY.max(current.element_states.len());
        if self.element_states.capacity()
            > state_target.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.element_states.shrink_to(state_target);
        }
        self.accessed_element_states.trim_for_reuse(&current.accessed_element_states);
        self.dispatch_tree.trim_for_reuse(&current.dispatch_tree);
        trim_vec_against!(mouse_listeners);
        trim_vec_against!(mounted_scroll_sources);
        trim_vec_against!(hitboxes);
        trim_vec_against!(layer_inputs);
        trim_vec_against!(window_control_hitboxes);
        trim_vec_against!(deferred_draws);
        trim_vec_against!(input_handlers);
        trim_vec_against!(tooltip_requests);
        trim_vec_against!(cursor_styles);
        trim_vec_against!(retained_scene_segments);
        let index_target = FRAME_MIN_RETAINED_CAPACITY.max(current.retained_scene_index.len());
        if self.retained_scene_index.capacity()
            > index_target.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.retained_scene_index.shrink_to(index_target);
        }

        trim_vec_against!(view_retention_records);
        let view_target = FRAME_MIN_RETAINED_CAPACITY.max(current.view_retentions.len());
        if self.view_retentions.capacity()
            > view_target.saturating_mul(FRAME_IDLE_TRIM_WATERMARK_MULTIPLIER)
        {
            self.view_retentions.shrink_to(view_target);
        }
        self.tab_stops.trim_for_reuse(&current.tab_stops);
    }

    /// Lower bound on retained metadata backing storage; excludes hash control
    /// bytes, Arc/Rc allocations, and allocator overhead. This is not RSS.
    pub(super) fn retained_metadata_inline_bytes(&self) -> usize {
        self.view_retentions.capacity()
            .saturating_mul(std::mem::size_of::<(u64, usize)>())
            .saturating_add(self.view_retention_records.capacity()
                .saturating_mul(std::mem::size_of::<super::view_retention::ViewRecordSlot>()))

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
                self.accessed_element_states.trim(true);
                self.dispatch_tree
                    .trim_retained_capacity(matches!(level, GpuiMemoryTrimLevel::Aggressive));
                self.mouse_listeners.shrink_to(floor);
                self.mounted_scroll_sources.shrink_to(floor);
                self.hitboxes.shrink_to(floor);
                self.layer_inputs.shrink_to(floor);
                self.window_control_hitboxes.shrink_to(floor);
                self.deferred_draws.shrink_to(floor);
                self.input_handlers.shrink_to(floor);
                self.tooltip_requests.shrink_to(floor);
                self.cursor_styles.shrink_to(floor);
                self.retained_scene_segments.shrink_to(floor);
                self.retained_scene_index.shrink_to(floor.max(self.retained_scene_index.len()));
                self.view_retentions.shrink_to(floor.max(self.view_retentions.len()));
                self.view_retention_records.shrink_to(floor.max(self.view_retention_records.len()));
                self.tab_stops
                    .trim_retained_capacity(matches!(level, GpuiMemoryTrimLevel::Aggressive));
                #[cfg(any(test, feature = "test-support"))]
                {
                    self.debug_bounds
                        .shrink_to(floor.max(self.debug_bounds.len()));
                    self.debug_bounds_records
                        .shrink_to(floor.max(self.debug_bounds_records.len()));
                }
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
        let capacity =
            capacity + self.debug_bounds.capacity() + self.debug_bounds_records.capacity();
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

fn rollback_replayed_option_transfers<T>(
    committed: &mut [Option<T>],
    candidate: &mut [Option<T>],
    transfers: &mut Vec<(usize, usize)>,
) -> usize {
    let mut count = 0;
    for (from, to) in transfers.drain(..).rev() {
        let (Some(source), Some(destination)) = (committed.get_mut(from), candidate.get_mut(to))
        else {
            debug_assert!(false, "retained input replay transfer index out of bounds");
            continue;
        };
        // An in-flight IME owner may have returned to the platform already.
        // Only restore still-owned candidate values.
        if source.is_none() {
            *source = destination.take();
            count += usize::from(source.is_some());
        }
    }
    count
}

fn trim_frame_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    if vec.capacity() > floor.saturating_mul(multiplier) {
        vec.shrink_to(floor);
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[test]
    fn aborted_candidate_restores_noncloneable_input_handler_ownership() {
        let mut committed = vec![Some(String::from("IME")), None, Some(String::from("Text"))];
        let mut speculative = vec![committed[0].take(), committed[2].take()];
        let mut transfers = vec![(0, 0), (2, 1)];
        assert_eq!(
            rollback_replayed_option_transfers(&mut committed, &mut speculative, &mut transfers),
            2
        );
        assert_eq!(
            committed,
            vec![Some(String::from("IME")), None, Some(String::from("Text"))]
        );
        assert_eq!(speculative, vec![None, None]);
        assert!(transfers.is_empty());
    }

    #[test]
    fn rollback_does_not_take_a_fresh_input_handler_without_a_replay_record() {
        let mut committed = vec![None::<u64>];
        let mut candidate = vec![Some(17)];
        let mut transfers = Vec::new();
        assert_eq!(
            rollback_replayed_option_transfers(&mut committed, &mut candidate, &mut transfers),
            0
        );
        assert_eq!(candidate, vec![Some(17)]);
    }

    fn test_cached_slot(value: u64) -> ElementStateBox {
        ElementStateBox {
            inner: Box::new(Some(value)),
            #[cfg(debug_assertions)]
            type_name: "Option<u64>",
        }
    }

    fn test_cached_value(slot: &ElementStateBox) -> Option<u64> {
        slot.inner.downcast_ref::<Option<u64>>().copied().flatten()
    }

    #[test]
    fn abort_keeps_existing_committed_snapshot_and_discards_candidate() {
        let key = (GlobalElementId::default(), TypeId::of::<u64>());
        let mut old = Frame::new(DispatchTree::new(Default::default(), Default::default()));
        let mut candidate = Frame::new(DispatchTree::new(Default::default(), Default::default()));
        old.element_states.insert(key.clone(), test_cached_slot(10));
        candidate
            .element_states
            .insert(key.clone(), test_cached_slot(20));
        old.rollback_element_states(&mut candidate);
        assert_eq!(test_cached_value(&old.element_states[&key]), Some(10));
        assert!(candidate.element_states.is_empty());
    }

    #[test]
    fn successful_frame_prefers_candidate_state_over_committed_snapshot() {
        let key = (GlobalElementId::default(), TypeId::of::<u64>());
        let mut old = Frame::new(DispatchTree::new(Default::default(), Default::default()));
        let mut candidate = Frame::new(DispatchTree::new(Default::default(), Default::default()));
        old.element_states.insert(key.clone(), test_cached_slot(10));
        candidate
            .element_states
            .insert(key.clone(), test_cached_slot(20));
        candidate.accessed_element_states.push(Some(key.clone()));
        candidate.finish(&mut old);
        assert_eq!(test_cached_value(&candidate.element_states[&key]), Some(20));
        assert!(!old.element_states.contains_key(&key));
    }

    #[test]
    fn published_paint_moves_without_copying_and_next_build_is_independent() {
        let mut frame = Frame::new(DispatchTree::new(Default::default(), Default::default()));
        let mut previous = Frame::new(DispatchTree::new(Default::default(), Default::default()));
        let bounds = crate::bounds(
            point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(16.), ScaledPixels(16.)),
        );
        let quad = Quad {
            bounds,
            content_mask: ContentMask::new(bounds),
            ..Default::default()
        };
        frame.painter.insert_primitive(quad);
        let original = &frame.painter.paint_operations[0] as *const _;
        frame.finish(&mut previous);
        let paint = frame.publish(PaintGeometry::default(), None, true);
        let operation = &paint.scene.paint_operations[0] as *const _;
        assert_eq!(original, operation, "publication moves existing payload");
        {
            let mut presentation = crate::window::state::PresentationState::default();
            presentation.publish_pending(Arc::clone(&paint), []);
            assert!(presentation.activate_pending());
            let packet = PresentationPacket::new(
                presentation.active_paint().expect("active Paint"),
                [],
                [],
                Instant::now(),
                DirtyRegion::empty(),
                crate::BackdropBlurDamagePlan::default(),
                PartialPresentMode::FullRedraw,
            );
            assert!(
                Arc::ptr_eq(&paint, &packet.paint),
                "queue and renderer consume the original publication"
            );
        }
        assert!(frame.painter.paint_operations.iter().next().is_none());
        frame.clear();
        frame.painter.insert_primitive(quad);
        assert_eq!(paint.scene.len(), 1);
        assert_eq!(&paint.scene.paint_operations[0] as *const _, operation);
        assert_eq!(Arc::strong_count(&paint), 1);
    }
}
