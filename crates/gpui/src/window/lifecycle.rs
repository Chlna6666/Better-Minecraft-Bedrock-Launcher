use super::*;

pub(crate) fn ignore_window_not_found<T>(result: Result<T>) -> Option<T> {
    match result {
        Ok(value) => Some(value),
        Err(error) if error.to_string() == "window not found" => None,
        Err(error) => Err::<T, _>(error).log_err(),
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
    /// listeners are invoked from the root of the element tree downward toward the focused element. This phase
    /// is used for special purposes, such as clearing the "pressed" state for click events. If
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

/// Dependency scope carried by a targeted retained invalidation.
///
/// `ElementOnly` is for paint-local changes whose descendants are provably unaffected.
/// `ReconcileSubtree` means descendants must be visited far enough to prove whether they can reuse
/// previous work, but they are not intrinsically dirty. `InvalidateSubtree` is the conservative
/// barrier used when inherited context or opaque runtime state makes descendant reuse unsafe.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum RetainedInvalidationScope {
    #[default]
    ElementOnly,
    ReconcileSubtree,
    InvalidateSubtree,
}

impl RetainedInvalidationScope {
    #[inline]
    fn from_descendants_dirty(descendants_dirty: bool) -> Self {
        if descendants_dirty {
            Self::InvalidateSubtree
        } else {
            Self::ElementOnly
        }
    }

    #[inline]
    fn merged(self, other: Self) -> Self {
        self.max(other)
    }
}

impl Window {
    /// Invalidates an interactive element without turning an element-local state change into a
    /// general application notification or a forced refresh of every cached view.
    ///
    /// This conservative entry point keeps the changed element's descendants dirty. Call
    /// [`Self::notify_interactive_region_scoped`] when the caller knows the interaction only
    /// changes the element's own paint and descendants can retain their previous ranges.
    pub(crate) fn notify_interactive_region(
        &mut self,
        view_id: EntityId,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        cx: &mut App,
    ) {
        self.notify_interactive_region_scoped(view_id, global_id, bounds, true, cx);
    }

    /// Invalidates one stable interactive path with an explicit descendant-damage scope.
    ///
    /// `descendants_dirty == false` means only the changed element and its structural ancestors
    /// must execute normally. Descendants remain eligible for retained prepaint/paint replay. This
    /// is correct for state styles that only change the element's own pixels (for example a
    /// background or shadow). Set it to `true` whenever the interaction can change layout,
    /// inherited text, clipping, opacity, transforms, or another subtree paint context.
    pub(crate) fn notify_interactive_region_scoped(
        &mut self,
        view_id: EntityId,
        global_id: Option<&GlobalElementId>,
        bounds: Bounds<Pixels>,
        descendants_dirty: bool,
        _cx: &mut App,
    ) {
        if !bounds.is_empty() {
            self.animation_dirty_region.push(bounds.scale(self.scale_factor));
        }
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
        if self.invalidator.invalidate_retained_path_with_scope(
            view_id,
            global_id,
            RetainedInvalidationScope::from_descendants_dirty(descendants_dirty),
        ) {
            self.schedule_dirty_frame();
        }
    }

    /// Requests another draw for a window-owned overlay without invalidating application views.
    ///
    /// The root element tree may be traversed to assemble the frame, but unchanged retained
    /// subtrees remain replayable. This is used for tooltips and drag previews, whose lifecycle is
    /// owned by the window rather than by an entity notification.
    pub(crate) fn redraw_without_view_cache_refresh(&mut self) {
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
        self.invalidator.set_replay_only_dirty();
        self.schedule_dirty_frame();
    }
}

struct WindowInvalidatorInner {
    pub dirty: bool,
    pub draw_phase: DrawPhase,
    pub dirty_views: FxHashSet<EntityId>,
    pub dirty_frame_diagnostics: Rc<RefCell<DirtyFrameDiagnostics>>,
    /// True while the queued frame still has enough provenance to make retained replay decisions
    /// per view. Exact element targets and generic-dirty views may coexist in the same frame.
    pub pending_targeted_replay: bool,
    /// Stable dirty retained paths and the dependency scope each target carries.
    pub pending_targeted_elements: FxHashMap<GlobalElementId, RetainedInvalidationScope>,
    /// Views that received a generic application invalidation while selective replay remained
    /// available for unrelated views. These views and their descendants stay conservative.
    pub pending_generic_dirty_views: FxHashSet<EntityId>,
    /// Layout-animation frame tickets that have already been armed for an exact retained target.
    /// Keeping this per target prevents repeated samples from stacking stale next-frame callbacks.
    pub pending_layout_animation_frames: FxHashSet<(EntityId, GlobalElementId)>,
    /// Delayed layout-animation tickets keyed by the exact retained target. The earliest deadline
    /// wins; generation changes make superseded timers exit without touching retained state.
    pub pending_layout_animation_deadlines:
        FxHashMap<(EntityId, GlobalElementId), (Instant, u64)>,
    pub layout_animation_deadline_generation: u64,
    /// Snapshot consumed by the frame currently being generated.
    pub active_targeted_replay: bool,
    pub active_targeted_elements: FxHashMap<GlobalElementId, RetainedInvalidationScope>,
    pub active_generic_dirty_views: FxHashSet<EntityId>,
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
                dirty_frame_diagnostics: Rc::new(RefCell::new(DirtyFrameDiagnostics::default())),
                pending_targeted_replay: false,
                pending_targeted_elements: FxHashMap::default(),
                pending_generic_dirty_views: FxHashSet::default(),
                pending_layout_animation_frames: FxHashSet::default(),
                pending_layout_animation_deadlines: FxHashMap::default(),
                layout_animation_deadline_generation: 0,
                active_targeted_replay: false,
                active_targeted_elements: FxHashMap::default(),
                active_generic_dirty_views: FxHashSet::default(),
            })),
        }
    }

    pub(in crate::window) fn set_dirty_frame_diagnostics(
        &self,
        dirty_frame_diagnostics: Rc<RefCell<DirtyFrameDiagnostics>>,
    ) {
        self.inner.borrow_mut().dirty_frame_diagnostics = dirty_frame_diagnostics;
    }

    /// Arms one immediate layout-animation ticket for an exact retained target.
    ///
    /// Repeated requests for the same target before its callback runs are coalesced. An immediate
    /// sample also supersedes any delayed ticket for that target, so an old timer cannot enqueue a
    /// second stale frame after fresh state has already requested the next VSync.
    pub(in crate::window) fn arm_layout_animation_frame(
        &self,
        entity: EntityId,
        retained_id: &GlobalElementId,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let key = (entity, retained_id.clone());
        inner.pending_layout_animation_deadlines.remove(&key);
        inner.pending_layout_animation_frames.insert(key)
    }

    /// Consumes the immediate ticket owned by one layout-animation callback.
    pub(in crate::window) fn take_layout_animation_frame(
        &self,
        entity: EntityId,
        retained_id: &GlobalElementId,
    ) -> bool {
        self.inner
            .borrow_mut()
            .pending_layout_animation_frames
            .remove(&(entity, retained_id.clone()))
    }

    /// Arms or tightens a delayed layout-animation sample for one exact retained target.
    ///
    /// Returns the generation assigned to a newly armed timer. `None` means an immediate ticket is
    /// already pending or an earlier/equal deadline already covers the requested sample.
    pub(in crate::window) fn arm_layout_animation_deadline(
        &self,
        entity: EntityId,
        retained_id: &GlobalElementId,
        deadline: Instant,
    ) -> Option<u64> {
        let mut inner = self.inner.borrow_mut();
        let key = (entity, retained_id.clone());
        if inner.pending_layout_animation_frames.contains(&key)
            || inner
                .pending_layout_animation_deadlines
                .get(&key)
                .is_some_and(|(pending_deadline, _)| *pending_deadline <= deadline)
        {
            return None;
        }

        inner.layout_animation_deadline_generation =
            inner.layout_animation_deadline_generation.wrapping_add(1);
        let generation = inner.layout_animation_deadline_generation;
        inner
            .pending_layout_animation_deadlines
            .insert(key, (deadline, generation));
        Some(generation)
    }

    /// Consumes a delayed ticket only if this timer still owns the target/deadline generation.
    pub(in crate::window) fn take_layout_animation_deadline(
        &self,
        entity: EntityId,
        retained_id: &GlobalElementId,
        deadline: Instant,
        generation: u64,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let key = (entity, retained_id.clone());
        if inner.pending_layout_animation_deadlines.get(&key).copied()
            != Some((deadline, generation))
        {
            return false;
        }
        inner.pending_layout_animation_deadlines.remove(&key);
        true
    }

    pub fn invalidate_view(&self, entity: EntityId, cx: &mut App) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner
            .dirty_frame_diagnostics
            .borrow_mut()
            .record_notify_invalidation(entity);
        inner.dirty_views.insert(entity);
        if inner.draw_phase == DrawPhase::None {
            if !inner.dirty {
                inner.pending_targeted_replay = true;
                inner.pending_targeted_elements.clear();
                inner.pending_generic_dirty_views.clear();
            }
            if inner.pending_targeted_replay {
                inner.pending_generic_dirty_views.insert(entity);
            }
            inner.dirty = true;
            cx.push_effect(Effect::Notify { emitter: entity });
            true
        } else {
            false
        }
    }

    /// Compatibility entry point for callers that only know whether descendants are dirty.
    pub(in crate::window) fn invalidate_retained_path(
        &self,
        entity: EntityId,
        global_id: Option<&GlobalElementId>,
        descendants_dirty: bool,
    ) -> bool {
        self.invalidate_retained_path_with_scope(
            entity,
            global_id,
            RetainedInvalidationScope::from_descendants_dirty(descendants_dirty),
        )
    }

    /// Marks a view dirty because one stable retained element path changed, preserving the exact
    /// dependency scope required below that path.
    ///
    /// No application-level `Notify` effect is emitted: the caller already owns the reason for
    /// invalidation (interaction state, animation sampling, focus, window overlay, and so on). The
    /// retained path is carried into the next frame so unrelated siblings remain replayable.
    pub(in crate::window) fn invalidate_retained_path_with_scope(
        &self,
        entity: EntityId,
        global_id: Option<&GlobalElementId>,
        scope: RetainedInvalidationScope,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner
            .dirty_frame_diagnostics
            .borrow_mut()
            .record_notify_invalidation(entity);
        inner.dirty_views.insert(entity);

        if inner.draw_phase != DrawPhase::None {
            return false;
        }

        if !inner.dirty {
            inner.pending_targeted_replay = true;
            inner.pending_targeted_elements.clear();
            inner.pending_generic_dirty_views.clear();
        }

        if inner.pending_targeted_replay {
            if let Some(global_id) = global_id {
                inner
                    .pending_targeted_elements
                    .entry(global_id.clone())
                    .and_modify(|existing_scope| *existing_scope = existing_scope.merged(scope))
                    .or_insert(scope);
            } else {
                inner.pending_generic_dirty_views.insert(entity);
            }
        }
        inner.dirty = true;
        true
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    /// Marks a generic frame dirty. This is a conservative invalidation and disables selective
    /// retained replay because the caller has not supplied any view- or element-level provenance.
    pub fn set_dirty(&self, is_dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        if is_dirty {
            inner.pending_targeted_replay = false;
            inner.pending_targeted_elements.clear();
            inner.pending_generic_dirty_views.clear();
            inner.active_targeted_replay = false;
            inner.active_targeted_elements.clear();
            inner.active_generic_dirty_views.clear();
        } else {
            inner.active_targeted_replay = inner.pending_targeted_replay;
            inner.active_targeted_elements = mem::take(&mut inner.pending_targeted_elements);
            inner.active_generic_dirty_views = mem::take(&mut inner.pending_generic_dirty_views);
            inner.pending_targeted_replay = false;
            if !inner.active_targeted_replay {
                inner.active_targeted_elements.clear();
                inner.active_generic_dirty_views.clear();
            }
        }
        inner.dirty = is_dirty;
    }

    /// Schedules a frame whose only window-level change is outside the retained application tree.
    ///
    /// An empty target set means every stable retained element is eligible for replay. If a
    /// targeted or per-view generic invalidation is queued before this frame starts, its provenance
    /// is merged into the same selective replay frame. A pre-existing global dirty request is never
    /// upgraded to selective replay.
    pub(in crate::window) fn set_replay_only_dirty(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.draw_phase != DrawPhase::None {
            inner.dirty = true;
            return;
        }

        if !inner.dirty {
            inner.pending_targeted_replay = true;
            inner.pending_targeted_elements.clear();
            inner.pending_generic_dirty_views.clear();
        }
        inner.dirty = true;
    }

    pub(in crate::window) fn active_targeted_replay(&self) -> bool {
        self.inner.borrow().active_targeted_replay
    }

    pub(in crate::window) fn active_generic_view_is_dirty(&self, entity: EntityId) -> bool {
        self.inner
            .borrow()
            .active_generic_dirty_views
            .contains(&entity)
    }

    /// Returns whether this stable retained path is intrinsically dirty in the active targeted
    /// frame. Structural ancestors execute so traversal can reach the target. `ReconcileSubtree`
    /// descendants are intentionally not reported dirty here: they use the separate reconciliation
    /// query below so the renderer can require proof before replay rather than repainting blindly.
    pub(in crate::window) fn retained_path_is_dirty(&self, global_id: &GlobalElementId) -> bool {
        let inner = self.inner.borrow();
        if !inner.active_targeted_replay {
            return true;
        }
        inner
            .active_targeted_elements
            .iter()
            .any(|(dirty, scope)| retained_path_requires_repaint(global_id, dirty, *scope))
    }

    /// Returns true when `global_id` lies below a `ReconcileSubtree` target.
    ///
    /// Such an element is not known dirty, but an ancestor cannot hide it by replaying an old
    /// subtree solely because its own bounds stayed fixed. Callers may still reuse the element when
    /// they possess a semantic proof for the current frame (for example exact plain-text output).
    pub(in crate::window) fn retained_path_requires_reconciliation(
        &self,
        global_id: &GlobalElementId,
    ) -> bool {
        let inner = self.inner.borrow();
        if !inner.active_targeted_replay {
            return false;
        }
        inner.active_targeted_elements.iter().any(|(dirty, scope)| {
            *scope == RetainedInvalidationScope::ReconcileSubtree
                && global_element_path_is_strict_prefix(dirty, global_id)
        })
    }

    /// Returns true when `global_id` executes only because it is a structural ancestor of one or
    /// more dirty retained targets, while its own pixels and subtree context remain unchanged.
    ///
    /// The ancestor still routes traversal to the changed child, but its own stable
    /// background/shadow/border primitives can be replayed. A direct hit on this path, or an
    /// ancestor invalidation whose scope damages/reconciles descendants, disables self-scene reuse.
    pub(in crate::window) fn retained_path_is_descendant_only(
        &self,
        global_id: &GlobalElementId,
    ) -> bool {
        let inner = self.inner.borrow();
        if !inner.active_targeted_replay {
            return false;
        }

        let mut has_dirty_descendant = false;
        for (dirty, scope) in &inner.active_targeted_elements {
            if global_id == dirty
                || (*scope != RetainedInvalidationScope::ElementOnly
                    && global_element_path_is_prefix(dirty, global_id))
            {
                return false;
            }
            if global_element_path_is_strict_prefix(global_id, dirty) {
                has_dirty_descendant = true;
            }
        }
        has_dirty_descendant
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

fn retained_path_requires_repaint(
    candidate: &GlobalElementId,
    dirty: &GlobalElementId,
    scope: RetainedInvalidationScope,
) -> bool {
    global_element_path_is_prefix(candidate, dirty)
        || scope == RetainedInvalidationScope::InvalidateSubtree
            && global_element_path_is_prefix(dirty, candidate)
}

fn global_element_path_is_prefix(prefix: &GlobalElementId, path: &GlobalElementId) -> bool {
    prefix.0.len() <= path.0.len()
        && prefix
            .0
            .iter()
            .zip(path.0.iter())
            .all(|(prefix, path)| prefix == path)
}

fn global_element_path_is_strict_prefix(prefix: &GlobalElementId, path: &GlobalElementId) -> bool {
    prefix.0.len() < path.0.len() && global_element_path_is_prefix(prefix, path)
}

#[cfg(test)]
mod retained_dirty_scope_tests {
    use super::*;

    fn path(parts: &[u32]) -> GlobalElementId {
        GlobalElementId(parts.iter().copied().map(ElementId::InstanceSlot).collect())
    }

    #[test]
    fn element_only_target_keeps_descendants_replayable() {
        let ancestor = path(&[0]);
        let dirty = path(&[0, 1]);
        let descendant = path(&[0, 1, 2]);
        let sibling = path(&[0, 3]);

        assert!(retained_path_requires_repaint(
            &ancestor,
            &dirty,
            RetainedInvalidationScope::ElementOnly
        ));
        assert!(retained_path_requires_repaint(
            &dirty,
            &dirty,
            RetainedInvalidationScope::ElementOnly
        ));
        assert!(!retained_path_requires_repaint(
            &descendant,
            &dirty,
            RetainedInvalidationScope::ElementOnly
        ));
        assert!(!retained_path_requires_repaint(
            &sibling,
            &dirty,
            RetainedInvalidationScope::ElementOnly
        ));
    }

    #[test]
    fn subtree_target_invalidates_descendants_but_not_siblings() {
        let ancestor = path(&[0]);
        let dirty = path(&[0, 1]);
        let descendant = path(&[0, 1, 2]);
        let sibling = path(&[0, 3]);

        assert!(retained_path_requires_repaint(
            &ancestor,
            &dirty,
            RetainedInvalidationScope::InvalidateSubtree
        ));
        assert!(retained_path_requires_repaint(
            &dirty,
            &dirty,
            RetainedInvalidationScope::InvalidateSubtree
        ));
        assert!(retained_path_requires_repaint(
            &descendant,
            &dirty,
            RetainedInvalidationScope::InvalidateSubtree
        ));
        assert!(!retained_path_requires_repaint(
            &sibling,
            &dirty,
            RetainedInvalidationScope::InvalidateSubtree
        ));
    }

    #[test]
    fn reconcile_target_visits_descendants_without_marking_them_dirty() {
        let invalidator = WindowInvalidator::new();
        let dirty = path(&[0, 1]);
        let descendant = path(&[0, 1, 2]);
        let sibling = path(&[0, 3]);

        invalidator.set_dirty(false);
        assert!(invalidator.invalidate_retained_path_with_scope(
            EntityId::from_u64(1),
            Some(&dirty),
            RetainedInvalidationScope::ReconcileSubtree,
        ));
        invalidator.set_dirty(false);

        assert!(invalidator.retained_path_is_dirty(&dirty));
        assert!(!invalidator.retained_path_is_dirty(&descendant));
        assert!(invalidator.retained_path_requires_reconciliation(&descendant));
        assert!(!invalidator.retained_path_is_dirty(&sibling));
        assert!(!invalidator.retained_path_requires_reconciliation(&sibling));
    }

    #[test]
    fn stronger_scope_wins_when_targets_merge() {
        let invalidator = WindowInvalidator::new();
        let dirty = path(&[0, 1]);
        let descendant = path(&[0, 1, 2]);

        invalidator.set_dirty(false);
        assert!(invalidator.invalidate_retained_path_with_scope(
            EntityId::from_u64(1),
            Some(&dirty),
            RetainedInvalidationScope::ReconcileSubtree,
        ));
        assert!(invalidator.invalidate_retained_path_with_scope(
            EntityId::from_u64(1),
            Some(&dirty),
            RetainedInvalidationScope::InvalidateSubtree,
        ));
        invalidator.set_dirty(false);

        assert!(invalidator.retained_path_is_dirty(&descendant));
        assert!(!invalidator.retained_path_requires_reconciliation(&descendant));
    }

    #[test]
    fn ancestor_can_reuse_own_scene_for_element_only_child_damage() {
        let invalidator = WindowInvalidator::new();
        let ancestor = path(&[0]);
        let dirty = path(&[0, 1]);

        invalidator.set_dirty(false);
        assert!(invalidator.invalidate_retained_path(EntityId::from_u64(1), Some(&dirty), false));
        invalidator.set_dirty(false);

        assert!(invalidator.retained_path_is_descendant_only(&ancestor));
        assert!(!invalidator.retained_path_is_descendant_only(&dirty));
    }

    #[test]
    fn subtree_damage_disables_descendant_self_scene_reuse() {
        let invalidator = WindowInvalidator::new();
        let dirty = path(&[0]);
        let descendant = path(&[0, 1]);

        invalidator.set_dirty(false);
        assert!(invalidator.invalidate_retained_path(EntityId::from_u64(1), Some(&dirty), true));
        invalidator.set_dirty(false);

        assert!(!invalidator.retained_path_is_descendant_only(&dirty));
        assert!(!invalidator.retained_path_is_descendant_only(&descendant));
    }

    #[test]
    fn replay_only_dirty_keeps_all_retained_paths_replayable() {
        let invalidator = WindowInvalidator::new();
        invalidator.set_dirty(false);
        invalidator.set_replay_only_dirty();
        invalidator.set_dirty(false);

        assert!(invalidator.active_targeted_replay());
        assert!(!invalidator.retained_path_is_dirty(&path(&[0])));
        assert!(!invalidator.retained_path_is_dirty(&path(&[0, 1, 2])));
    }

    #[test]
    fn generic_view_invalidation_preserves_unrelated_targeted_replay() {
        let invalidator = WindowInvalidator::new();
        let targeted_view = EntityId::from_u64(1);
        let generic_view = EntityId::from_u64(2);
        let dirty = path(&[0, 1]);

        invalidator.set_dirty(false);
        assert!(invalidator.invalidate_retained_path_with_scope(
            targeted_view,
            Some(&dirty),
            RetainedInvalidationScope::ReconcileSubtree,
        ));
        assert!(invalidator.invalidate_retained_path_with_scope(
            generic_view,
            None,
            RetainedInvalidationScope::InvalidateSubtree,
        ));
        invalidator.set_dirty(false);

        assert!(invalidator.active_targeted_replay());
        assert!(!invalidator.active_generic_view_is_dirty(targeted_view));
        assert!(invalidator.active_generic_view_is_dirty(generic_view));
        assert!(invalidator.retained_path_is_dirty(&dirty));
    }

    #[test]
    fn generic_view_invalidation_wins_over_same_view_target() {
        let invalidator = WindowInvalidator::new();
        let view = EntityId::from_u64(1);
        let dirty = path(&[0, 1]);

        invalidator.set_dirty(false);
        assert!(invalidator.invalidate_retained_path_with_scope(
            view,
            Some(&dirty),
            RetainedInvalidationScope::ElementOnly,
        ));
        assert!(invalidator.invalidate_retained_path_with_scope(
            view,
            None,
            RetainedInvalidationScope::InvalidateSubtree,
        ));
        invalidator.set_dirty(false);

        assert!(invalidator.active_targeted_replay());
        assert!(invalidator.active_generic_view_is_dirty(view));
    }

    #[test]
    fn layout_animation_frame_ticket_is_per_target() {
        let invalidator = WindowInvalidator::new();
        let entity = EntityId::from_u64(1);
        let first = path(&[0, 1]);
        let second = path(&[0, 2]);

        assert!(invalidator.arm_layout_animation_frame(entity, &first));
        assert!(!invalidator.arm_layout_animation_frame(entity, &first));
        assert!(invalidator.arm_layout_animation_frame(entity, &second));
        assert!(invalidator.take_layout_animation_frame(entity, &first));
        assert!(!invalidator.take_layout_animation_frame(entity, &first));
        assert!(invalidator.take_layout_animation_frame(entity, &second));
    }

    #[test]
    fn immediate_layout_animation_ticket_supersedes_delayed_ticket() {
        let invalidator = WindowInvalidator::new();
        let entity = EntityId::from_u64(1);
        let target = path(&[0, 1]);
        let deadline = Instant::now() + Duration::from_millis(8);
        let generation = invalidator
            .arm_layout_animation_deadline(entity, &target, deadline)
            .expect("first delayed ticket");

        assert!(invalidator.arm_layout_animation_frame(entity, &target));
        assert!(!invalidator.take_layout_animation_deadline(
            entity,
            &target,
            deadline,
            generation,
        ));
    }

    #[test]
    fn earlier_layout_animation_deadline_replaces_later_generation() {
        let invalidator = WindowInvalidator::new();
        let entity = EntityId::from_u64(1);
        let target = path(&[0, 1]);
        let now = Instant::now();
        let later = now + Duration::from_millis(12);
        let earlier = now + Duration::from_millis(4);
        let old_generation = invalidator
            .arm_layout_animation_deadline(entity, &target, later)
            .expect("initial delayed ticket");
        let new_generation = invalidator
            .arm_layout_animation_deadline(entity, &target, earlier)
            .expect("earlier delayed ticket");

        assert_ne!(old_generation, new_generation);
        assert!(!invalidator.take_layout_animation_deadline(
            entity,
            &target,
            later,
            old_generation,
        ));
        assert!(invalidator.take_layout_animation_deadline(
            entity,
            &target,
            earlier,
            new_generation,
        ));
    }
}

pub(crate) type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;

pub(crate) type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;
