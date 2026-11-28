use super::state::ElementVisualTransform;
use super::*;

mod paint;
mod hitboxes;
pub(super) use hitboxes::HitboxStore;
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
    pub(crate) hitboxes: HitboxStore,
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

/// Transient old-coordinate edit plan for one physical Frame record stream.
/// All destinations are calculated before any record ownership is changed.
/// Not stored in Frame: consumers adopt the resulting pages and indices.
pub(crate) struct RecordSplicePlan {
    old_len: usize,
    replacement_len: usize,
    new_len: usize,
    edits: Vec<RecordSpliceEdit>,
}

struct RecordSpliceEdit {
    old: Range<usize>,
    replacement: Range<usize>,
    target: Range<usize>,
}

pub(crate) enum SpliceSource {
    Retained,
    Replacement,
}

#[allow(dead_code)] // Frame publication will consume the retained index mapping.
impl RecordSplicePlan {
    pub(crate) fn new(
        old_len: usize,
        replacement_len: usize,
        edits: &[(Range<usize>, Range<usize>)],
    ) -> Option<Self> {
        let mut cursor = 0;
        let mut target_end = 0usize;
        let mut validated = Vec::with_capacity(edits.len());
        for (old, replacement) in edits {
            if old.start < cursor || old.start > old.end || old.end > old_len
                || replacement.start > replacement.end || replacement.end > replacement_len
            {
                return None;
            }
            if old.is_empty() && replacement.is_empty() { continue; }
            let target_start = target_end.checked_add(old.start - cursor)?;
            target_end = target_start.checked_add(replacement.len())?;
            validated.push(RecordSpliceEdit {
                old: old.clone(),
                replacement: replacement.clone(),
                target: target_start..target_end,
            });
            cursor = old.end;
        }
        let new_len = target_end.checked_add(old_len - cursor)?;
        Some(Self { old_len, replacement_len, new_len, edits: validated })
    }

    pub(crate) fn matches_sources(&self, old_len: usize, replacement_len: usize) -> bool {
        self.old_len == old_len && self.replacement_len == replacement_len
    }

    pub(crate) fn len(&self) -> usize { self.new_len }

    /// A nonempty retained interval may touch an edit boundary but must
    /// not cross an insertion or own any retired records. The start is
    /// right-affine and the end left-affine. Empty checkpoints require
    /// explicit ownership and are deliberately not inferred here.
    pub(crate) fn retained_range(&self, range: Range<usize>) -> Option<Range<usize>> {
        if range.start >= range.end || range.end > self.old_len { return None; }
        let next = self.edits.partition_point(|edit| edit.old.end <= range.start);
        if self.edits.get(next).is_some_and(|edit| edit.old.start < range.end) {
            return None;
        }
        let (old_base, target_base) = next.checked_sub(1).map_or((0, 0), |index| {
            let previous = &self.edits[index];
            (previous.old.end, previous.target.end)
        });
        let start = target_base.checked_add(range.start.checked_sub(old_base)?)?;
        Some(start..start.checked_add(range.len())?)
    }

    /// Empty spans can be relocated only at unambiguous boundaries.
    /// A nonempty insertion at this checkpoint needs explicit before/after
    /// ownership, and a checkpoint inside retired records is invalid.
    pub(crate) fn retained_span(&self, range: Range<usize>) -> Option<Range<usize>> {
        if !range.is_empty() || range.start > range.end {
            return self.retained_range(range);
        }
        let index = range.start;
        if index > self.old_len { return None; }
        let first = self.edits.partition_point(|edit| edit.old.end < index);
        let mut mapped = None;
        for edit in self.edits[first..].iter().take_while(|edit| edit.old.start <= index) {
            if edit.old.is_empty() { return None; }
            let target = if index == edit.old.start {
                edit.target.start
            } else if index == edit.old.end {
                edit.target.end
            } else {
                return None;
            };
            if mapped.is_some_and(|previous| previous != target) { return None; }
            mapped = Some(target);
        }
        let index = match mapped {
            Some(target) => target,
            None => {
                let (old_base, target_base) = first.checked_sub(1).map_or((0, 0), |i| {
                    let edit = &self.edits[i];
                    (edit.old.end, edit.target.end)
                });
                target_base.checked_add(index.checked_sub(old_base)?)?
            }
        };
        Some(index..index)
    }

    /// A unique owner cannot be replayed twice or moved backwards through
    /// an already consumed replacement stream. Unlike shared page edits,
    /// these edits must also have disjoint, monotonic replacement ranges.
    fn can_move_owners(&self, old_len: usize, replacement_len: usize) -> bool {
        if !self.matches_sources(old_len, replacement_len) { return false; }
        let mut end = 0;
        for edit in &self.edits {
            if edit.replacement.is_empty() { continue; }
            if edit.replacement.start < end { return false; }
            end = edit.replacement.end;
        }
        true
    }

    /// Consume two unique record streams only after admission. Reserve all
    /// destination storage before taking either source. Retirement occurs
    /// when the remaining source iterators are dropped. No Clone bound.
    fn move_owners<T>(&self, old: &mut Vec<T>, replacement: &mut Vec<T>) -> Option<Vec<T>> {
        if !self.can_move_owners(old.len(), replacement.len()) { return None; }
        let mut result = Vec::new();
        result.try_reserve_exact(self.new_len).ok()?;
        let mut old = std::mem::take(old).into_iter();
        let mut replacement = std::mem::take(replacement).into_iter();
        let (mut old_index, mut new_index) = (0, 0);
        self.visit_ranges(|source, range| {
            let (records, index) = match source {
                SpliceSource::Retained => (&mut old, &mut old_index),
                SpliceSource::Replacement => (&mut replacement, &mut new_index),
            };
            let skip = range.start - *index;
            result.extend(records.by_ref().skip(skip).take(range.len()));
            *index = range.end;
        });
        debug_assert_eq!(result.len(), self.new_len);
        Some(result)
    }

    pub(crate) fn visit_ranges(&self, mut visit: impl FnMut(SpliceSource, Range<usize>)) {
        let mut cursor = 0;
        for edit in &self.edits {
            if cursor < edit.old.start {
                visit(SpliceSource::Retained, cursor..edit.old.start);
            }
            if !edit.replacement.is_empty() {
                visit(SpliceSource::Replacement, edit.replacement.clone());
            }
            cursor = edit.old.end;
        }
        if cursor < self.old_len {
            visit(SpliceSource::Retained, cursor..self.old_len);
        }
    }
}

fn rebase_index(value: usize, source: usize, target: usize) -> Option<usize> {
    target.checked_add(value.checked_sub(source)?)
}

impl Window {
    /// Samples known root Scene and Hitbox backing allocations in the committed
    /// and build frames, deduplicating physical storage across both.
    /// Intended for diagnostics, not the per-frame hot path.
    pub fn frame_storage_snapshot(&self) -> crate::diagnostics::frame_storage::FrameStorageSnapshot {
        let mut accounting = crate::diagnostics::frame_storage::Accounting::default();
        for frame in [&self.rendered_frame, &self.next_frame] {
            if let Some(paint) = frame.published_paint_ref() {
                paint.scene.account_storage(&mut accounting);
            }
            frame.painter.scene.account_storage(&mut accounting);
            frame.hitboxes.account_storage(&mut accounting);
        }
        accounting.finish()
    }
}

/// Index projection for the four currently supported boundary streams.
/// Callers must separately certify that the other record streams, including
/// text layout logs, have not changed before adopting these mapped ranges.
#[allow(dead_code)]
pub(super) struct BoundaryIndexSplices<'a> {
    pub(super) scene: &'a RecordSplicePlan,
    pub(super) hitboxes: &'a RecordSplicePlan,
    pub(super) states: &'a RecordSplicePlan,
    pub(super) dispatch: &'a RecordSplicePlan,
}

#[allow(dead_code)]
impl BoundaryIndexSplices<'_> {
    pub(super) fn retained_prepaint(
        &self, source: &Range<PrepaintStateIndex>,
    ) -> Option<Range<PrepaintStateIndex>> {
        let mut result = source.clone();
        let hitboxes = self.hitboxes.retained_span(source.start.hitboxes_index..source.end.hitboxes_index)?;
        let dispatch = self.dispatch.retained_span(source.start.dispatch_tree_index..source.end.dispatch_tree_index)?;
        let states = self.states.retained_span(source.start.accessed_element_states_index..source.end.accessed_element_states_index)?;
        result.start.hitboxes_index = hitboxes.start;
        result.end.hitboxes_index = hitboxes.end;
        result.start.dispatch_tree_index = dispatch.start;
        result.end.dispatch_tree_index = dispatch.end;
        result.start.accessed_element_states_index = states.start;
        result.end.accessed_element_states_index = states.end;
        Some(result)
    }

    pub(super) fn retained_paint(&self, source: &Range<PaintIndex>) -> Option<Range<PaintIndex>> {
        let mut result = source.clone();
        let scene = self.scene.retained_span(source.start.scene_index..source.end.scene_index)?;
        let states = self.states.retained_span(source.start.accessed_element_states_index..source.end.accessed_element_states_index)?;
        result.start.scene_index = scene.start;
        result.end.scene_index = scene.end;
        result.start.accessed_element_states_index = states.start;
        result.end.accessed_element_states_index = states.end;
        Some(result)
    }
}

/// One unpublished boundary candidate. All four native stores must agree
/// before any ownership can move out of the committed Frame. Other CPU
/// records (including noncloneable callbacks) still need admission and
/// relocation before this can become a published Paint.
#[allow(dead_code)]
pub(super) struct BoundaryRecordCandidate {
    pub(super) scene: Scene,
    pub(super) hitboxes: HitboxStore,
    pub(super) state_access: state_access::StateAccessLog,
    pub(super) dispatch: DispatchTree,
}

impl Frame {
    #[allow(dead_code)]
    pub(super) fn prepare_boundary_records(
        &self,
        replacement: &Self,
        scene_edits: &[(Range<usize>, Range<usize>)],
        hitboxes: &RecordSplicePlan,
        states: &RecordSplicePlan,
        dispatch: &RecordSplicePlan,
        replacement_parent: DispatchNodeId,
    ) -> Option<BoundaryRecordCandidate> {
        // Validate the structural and route constraints first. The three
        // other candidates own only new descriptors and shared payloads.
        let dispatch = self.dispatch_tree.with_spliced_records(
            &replacement.dispatch_tree, dispatch, replacement_parent,
        )?;
        let hitboxes = self.hitboxes.with_spliced_records(&replacement.hitboxes, hitboxes)?;
        let state_access = self.accessed_element_states.with_spliced_records(
            &replacement.accessed_element_states, states,
        )?;
        let scene = self.scene().with_spliced_row_markers(
            &replacement.scene().paint_operations, scene_edits,
        )?;
        Some(BoundaryRecordCandidate { scene, hitboxes, state_access, dispatch })
    }

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

    /// Commit the already validated placement without swapping the Frame.
    /// All non-Paint CPU owners stay resident, including noncloneable
    /// listener/state records and the existing layout ordering tree.
    pub(super) fn commit_scroll_placements(
        &mut self, paint: Arc<Paint>,
        placements: &[(Range<usize>, Point<Pixels>)],
    ) {
        // Admission already normalized nonempty, disjoint input spans.
        // Invalidate each enclosing proof once, even when it covers many
        // rows or a batch of independently scrolling views.
        debug_assert!(placements.iter().all(|(range, _)| !range.is_empty()));
        debug_assert!(placements.windows(2).all(|pair| pair[0].0.end <= pair[1].0.start));
        for (hitboxes, delta) in placements {
            self.hitboxes.translate_committed(hitboxes.clone(), *delta);
        }
        if !placements.is_empty() {
            for layer in &mut self.layer_inputs {
                if layer.geometry.is_none() { continue; }
                let first = placements.partition_point(|(range, _)| range.end <= layer.hitboxes.start);
                if placements.get(first).is_some_and(|(range, _)| range.start < layer.hitboxes.end) {
                    // The fixed clip remains valid, but the enclosing
                    // offset proof no longer describes every child.
                    layer.geometry = None;
                }
            }
            self.input_clip_epoch = self.input_clip_epoch.wrapping_add(1);
        }
        self.retained_paint = Arc::clone(&paint.retained_paint);
        self.paint = Some(paint);
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
            accessed_element_states: state_access::StateAccessLog::default(),
            mouse_listeners: Vec::new(),
            mounted_scroll_sources: Vec::new(),
            has_continuous_mouse_move_listener: false,
            dispatch_tree,
            painter: crate::scene::Painter::for_frame(),
            paint: None,
            retained_paint: Arc::default(),
            hitboxes: HitboxStore::default(),
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
        if self.hitboxes.slices(start..end).all(|(slice, _)| slice.iter().all(|hitbox| {
            let clip = hitbox.content_mask.bounds;
            let end = clip.bottom_right();
            clip.origin.x >= viewport.origin.x
                && clip.origin.y >= viewport.origin.y
                && end.x <= bottom_right.x
                && end.y <= bottom_right.y
        })) {
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
            && self.hitboxes.slices(start..self.hitboxes.len()).all(|(slice, _)|
                slice.iter().all(|hitbox| hitbox.content_mask == viewport))
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
        if !previous.hitboxes.contains(&source) { return false; }
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
                self.hitboxes.share(&previous.hitboxes, source.clone(), delta);
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
        if !source.is_empty() {
            self.input_clip_epoch = self.input_clip_epoch.wrapping_add(1);
        }
        self.carry_layer_inputs(previous, source.clone());
        self.hitboxes.share(&previous.hitboxes, source, Point::default());
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
            for (slice, translation) in self.hitboxes
                .candidate_slices(start..cursor, position, Point::default()).rev()
            {
                for hitbox in slice.iter().rev() {
                let bounds = translation.bounds(hitbox.bounds).intersect(&hitbox.content_mask.bounds);
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
        self.dispatch_tree.freeze();
        self.hitboxes.freeze();
        // A complete Frame is committed atomically, so speculative IME
        // transfers no longer need an abort journal after publication.
        self.replayed_input_handlers.clear();
        self.has_continuous_mouse_move_listener = self
            .mouse_listeners
            .iter()
            .any(|listener| listener.handles(TypeId::of::<MouseMoveEvent>(), true));
        let (same_paint, placement_only) = self.painter.finish_against_with_storage(prev_frame.scene());
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
                placement_only,
            );
        }
    }

    /// Restore IME handlers transferred from the still-committed frame.
    pub(crate) fn rollback_replayed_input_handlers(&mut self, candidate: &mut Self) -> usize {
        rollback_replayed_option_transfers(
            &mut self.input_handlers,
            &mut candidate.input_handlers,
            &mut candidate.replayed_input_handlers,
        )
    }

    pub(super) fn retained_capacity(&self) -> usize {
        self.painter.construction_capacity()
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
        self.hitboxes.trim(false, FRAME_MIN_RETAINED_CAPACITY);
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

        self.accessed_element_states.trim_for_reuse(&current.accessed_element_states);
        self.dispatch_tree.trim_for_reuse(&current.dispatch_tree);
        trim_vec_against!(mouse_listeners);
        trim_vec_against!(mounted_scroll_sources);
        self.hitboxes.trim_for_reuse(&current.hitboxes);
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
                self.accessed_element_states.trim(true);
                self.dispatch_tree
                    .trim_retained_capacity(matches!(level, GpuiMemoryTrimLevel::Aggressive));
                self.mouse_listeners.shrink_to(floor);
                self.mounted_scroll_sources.shrink_to(floor);
                self.hitboxes.trim(true, floor);
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
    fn boundary_candidate_rebases_dispatch_and_preserves_committed_frame() {
        let keymap = Rc::new(RefCell::new(crate::Keymap::default()));
        let actions = Rc::new(crate::ActionRegistry::default());
        let mut old = Frame::new(DispatchTree::new(keymap.clone(), actions.clone()));
        let mut replacement = Frame::new(DispatchTree::new(keymap, actions));
        let root = old.dispatch_tree.push_node();
        old.dispatch_tree.push_node(); // retired sibling
        old.dispatch_tree.pop_node();
        let kept = old.dispatch_tree.push_node();
        let owner = EntityId::from(71_u64);
        old.dispatch_tree.set_view_id(owner);
        let child = old.dispatch_tree.push_node();
        old.dispatch_tree.pop_node();
        old.dispatch_tree.pop_node();
        old.dispatch_tree.pop_node();
        old.dispatch_tree.freeze();

        replacement.dispatch_tree.push_node();
        replacement.dispatch_tree.push_node();
        replacement.dispatch_tree.pop_node();
        replacement.dispatch_tree.pop_node();
        replacement.dispatch_tree.freeze();

        let bounds = Bounds::new(point(ScaledPixels(0.), ScaledPixels(0.)),
            size(ScaledPixels(16.), ScaledPixels(16.)));
        old.painter.insert_primitive(Quad {
            bounds, content_mask: ContentMask::new(bounds), ..Default::default()
        });
        old.painter.finish();
        replacement.painter.finish();
        let empty = RecordSplicePlan::new(0, 0, &[]).unwrap();
        let plan = RecordSplicePlan::new(4, 2, &[(1..2, 0..2)]).unwrap();
        let next = old.prepare_boundary_records(
            &replacement, &[], &empty, &empty, &plan, root,
        ).expect("independent child replacement");
        assert_eq!(next.dispatch.len(), 5);
        assert_eq!(next.dispatch.dispatch_path(DispatchNodeId(2)).as_slice(),
            &[root, DispatchNodeId(1), DispatchNodeId(2)]);
        assert_eq!(next.dispatch.dispatch_path(DispatchNodeId(4)).as_slice(),
            &[root, DispatchNodeId(3), DispatchNodeId(4)]);
        assert!(next.dispatch.has_view_route(owner));
        assert_eq!(next.hitboxes.len(), 0);
        assert_eq!(next.state_access.len(), 0);
        assert_eq!(next.scene.len(), old.scene().len());
        assert!(std::ptr::eq(&next.scene.quads[0], &old.scene().quads[0]));

        // A retained child cannot refer to a retired parent. The old
        // record stream and routes remain untouched after rejection.
        let orphan = RecordSplicePlan::new(4, 2, &[(2..3, 0..0)]).unwrap();
        assert!(old.prepare_boundary_records(
            &replacement, &[], &empty, &empty, &orphan, root,
        ).is_none());
        assert_eq!(old.dispatch_tree.len(), 4);
        assert_eq!(old.dispatch_tree.dispatch_path(child).as_slice(), &[root, kept, child]);
        assert!(old.dispatch_tree.has_view_route(owner));

        replacement.dispatch_tree.set_active_node(DispatchNodeId(0));
        replacement.dispatch_tree.set_view_id(owner);
        replacement.dispatch_tree.pop_node();
        replacement.dispatch_tree.freeze();
        assert!(old.prepare_boundary_records(
            &replacement, &[], &empty, &empty, &plan, root,
        ).is_none());
        assert!(old.dispatch_tree.has_view_route(owner));
    }

    #[test]
    fn boundary_indices_relocate_each_stream_without_guessing_empty_ownership() {
        let scene = RecordSplicePlan::new(10, 2, &[(2..3, 0..2)]).unwrap();
        let hitboxes = RecordSplicePlan::new(10, 0, &[(2..4, 0..0)]).unwrap();
        let states = RecordSplicePlan::new(10, 3, &[(1..1, 0..3)]).unwrap();
        let dispatch = RecordSplicePlan::new(10, 1, &[(2..5, 0..1)]).unwrap();
        let maps = BoundaryIndexSplices {
            scene: &scene, hitboxes: &hitboxes, states: &states, dispatch: &dispatch,
        };
        let pre = PrepaintStateIndex {
            hitboxes_index: 5, dispatch_tree_index: 6, accessed_element_states_index: 7,
            tooltips_index: 2, ..Default::default()
        }..PrepaintStateIndex {
            hitboxes_index: 7, dispatch_tree_index: 8, accessed_element_states_index: 9,
            tooltips_index: 3, ..Default::default()
        };
        let moved = maps.retained_prepaint(&pre).unwrap();
        assert_eq!(moved.start.hitboxes_index..moved.end.hitboxes_index, 3..5);
        assert_eq!(moved.start.dispatch_tree_index..moved.end.dispatch_tree_index, 4..6);
        assert_eq!(moved.start.accessed_element_states_index..moved.end.accessed_element_states_index, 10..12);
        assert_eq!(moved.start.tooltips_index..moved.end.tooltips_index, 2..3);
        let paint = PaintIndex { scene_index: 4, accessed_element_states_index: 5, ..Default::default() }
            ..PaintIndex { scene_index: 8, accessed_element_states_index: 9, ..Default::default() };
        let moved = maps.retained_paint(&paint).unwrap();
        assert_eq!(moved.start.scene_index..moved.end.scene_index, 5..9);
        assert_eq!(moved.start.accessed_element_states_index..moved.end.accessed_element_states_index, 8..12);
        assert_eq!(states.retained_span(1..1), None);
        assert_eq!(hitboxes.retained_span(3..3), None);
        assert_eq!(hitboxes.retained_span(4..4), Some(2..2));
        assert_eq!(scene.retained_span(2..2), Some(2..2));
        assert_eq!(scene.retained_span(3..3), Some(4..4));
    }

    #[test]
    fn unique_record_splice_moves_noncloneable_owners_once_and_preserves_order() {
        struct Owner(usize, Rc<Cell<usize>>);
        impl Drop for Owner {
            fn drop(&mut self) { self.1.set(self.1.get() + 1); }
        }
        let drops = Rc::new(Cell::new(0));
        let mut old = (0..6).map(|i| Owner(i, drops.clone())).collect::<Vec<_>>();
        let mut new = (10..14).map(|i| Owner(i, drops.clone())).collect::<Vec<_>>();
        let invalid = RecordSplicePlan::new(6, 4, &[(1..2, 0..2), (3..4, 1..3)]).unwrap();
        assert!(invalid.move_owners(&mut old, &mut new).is_none());
        assert_eq!(old.len(), 6);
        assert_eq!(new.len(), 4);
        assert_eq!(drops.get(), 0);
        let plan = RecordSplicePlan::new(6, 4, &[(1..3, 0..1), (5..5, 2..4)]).unwrap();
        let result = plan.move_owners(&mut old, &mut new).unwrap();
        assert_eq!(result.iter().map(|owner| owner.0).collect::<Vec<_>>(), vec![0, 10, 3, 4, 12, 13, 5]);
        assert!(old.is_empty() && new.is_empty());
        assert_eq!(drops.get(), 3);
        drop(result);
        assert_eq!(drops.get(), 10);
    }

    #[test]
    fn record_splice_plan_maps_retained_intervals_without_crossing_edits() {
        let plan = RecordSplicePlan::new(12, 6, &[
            (2..2, 0..2), (4..7, 2..3), (9..10, 3..6),
        ]).unwrap();
        assert_eq!(plan.len(), 14);
        assert_eq!(plan.retained_range(0..2), Some(0..2));
        assert_eq!(plan.retained_range(2..4), Some(4..6));
        assert_eq!(plan.retained_range(7..9), Some(7..9));
        assert_eq!(plan.retained_range(10..12), Some(12..14));
        assert_eq!(plan.retained_range(1..3), None);
        assert_eq!(plan.retained_range(3..5), None);
        assert_eq!(plan.retained_range(5..6), None);
        assert_eq!(plan.retained_range(2..2), None);
        assert_eq!(plan.retained_range(12..13), None);
        assert!(RecordSplicePlan::new(5, 3, &[(0..3, 0..1), (2..4, 1..2)]).is_none());
        assert!(RecordSplicePlan::new(5, 3, &[(0..1, 0..4)]).is_none());
        assert!(RecordSplicePlan::new(usize::MAX, 1, &[(0..0, 0..1)]).is_none());
    }

    #[test]
    fn record_splice_plan_matches_all_small_retained_singletons() {
        for a in 0..=5 {
            for b in a..=5 {
                for c in b..=5 {
                    for d in c..=5 {
                        for n in 0..=2 {
                            for m in 0..=2 {
                                let plan = RecordSplicePlan::new(5, 2, &[
                                    (a..b, 0..n), (c..d, 0..m),
                                ]).unwrap();
                                let mut result = Vec::new();
                                plan.visit_ranges(|source, range| {
                                    result.extend(range.map(|i| match source {
                                        SpliceSource::Retained => Some(i),
                                        SpliceSource::Replacement => None,
                                    }));
                                });
                                assert_eq!(result.len(), plan.len());
                                for i in 0..5 {
                                    let expected = result.iter().position(|record| *record == Some(i))
                                        .map(|index| index..index + 1);
                                    assert_eq!(plan.retained_range(i..i + 1), expected);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

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
        let original = frame.painter.paint_operations.storage_identity(0);
        frame.finish(&mut previous);
        let paint = frame.publish(PaintGeometry::default(), None, true);
        let operation = paint.scene.paint_operations.storage_identity(0);
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
        assert!(frame.painter.paint_operations.views().next().is_none());
        frame.clear();
        frame.painter.insert_primitive(quad);
        assert_eq!(paint.scene.len(), 1);
        assert_eq!(paint.scene.paint_operations.storage_identity(0), operation);
        assert_eq!(Arc::strong_count(&paint), 1);
    }
}
