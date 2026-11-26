use super::state::FrameRequestReason;
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

impl Window {
    /// Clears in-flight dirty-to-present timing, optionally starting a fresh epoch for dirty state.
    ///
    /// Visibility changes and OS sleep/wake use this boundary so time spent off-screen or suspended
    /// never appears as foreground UI latency.
    pub(crate) fn reset_dirty_to_present_timing(&mut self, restart_if_dirty: bool) {
        self.active_dirty_to_present_started_at = None;
        self.invalidator
            .reset_dirty_to_present_epoch(restart_if_dirty);
    }

    /// Invalidate the owning view and its previous visual damage region.
    pub(crate) fn notify_interactive_region(
        &mut self,
        view_id: EntityId,
        bounds: Bounds<Pixels>,
        _cx: &mut App,
    ) {
        self.invalidate_interactive_region(view_id, bounds, true);
    }

    /// An animation callback already running inside a platform frame does
    /// not need to enqueue another frame to commit its UI changes.
    pub(crate) fn notify_frame_interactive_region(
        &mut self,
        view_id: EntityId,
        bounds: Bounds<Pixels>,
        _cx: &mut App,
    ) {
        self.record_frame_request_reason(FrameRequestReason::PresentationAnimation);
        self.invalidate_interactive_region(view_id, bounds, false);
    }

    fn invalidate_interactive_region(
        &mut self,
        view_id: EntityId,
        bounds: Bounds<Pixels>,
        schedule_frame: bool,
    ) {
        if !bounds.is_empty() {
            self.animation_dirty_region
                .push(bounds.scale(self.scale_factor));
        }
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
        if self.invalidator.invalidate_owned_view(view_id) && schedule_frame
        {
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

/// One owner record, with its last resolved dispatch route. `keep` is
/// scratch for publication, not an additional owner index.
struct CommittedReads {
    reads: crate::RenderDependencies,
    route: Option<EntityId>,
    keep: bool,
}

struct WindowInvalidatorInner {
    pub dirty: bool,
    /// First pending dirty edge that has not yet been assigned to a draw cycle.
    pub dirty_to_present_started_at: Option<Instant>,
    pub draw_phase: DrawPhase,
    pub dirty_views: FxHashSet<EntityId>,
    // Last successfully committed Render read owners. An unknown owner
    // requires the conservative window-wide fallback.
    view_reads: FxHashMap<Option<EntityId>, CommittedReads>,
    render_dependencies: FxHashMap<EntityId, Option<SmallVec<[EntityId; 2]>>>,
    global_read_dependencies: crate::GlobalReadDependencies,
    pub dirty_frame_diagnostics: Rc<RefCell<DirtyFrameDiagnostics>>,
    // Only distinguish view-scoped work from an unclassified window refresh.
    pending_view_scoped: bool,
    /// Layout-animation frame tickets that have already been armed for an exact retained target.
    /// Keeping this per target prevents repeated samples from stacking stale next-frame callbacks.
    pub pending_layout_animation_frames: FxHashSet<(EntityId, u64)>,
    /// Delayed layout-animation tickets keyed by the exact retained target. The earliest deadline
    /// wins; generation changes make superseded timers exit without touching retained state.
    pub pending_layout_animation_deadlines: FxHashMap<(EntityId, u64), (Instant, u64)>,
    pub layout_animation_deadline_generation: u64,
    active_view_scoped: bool,
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
                dirty_to_present_started_at: Some(Instant::now()),
                draw_phase: DrawPhase::None,
                dirty_views: FxHashSet::default(),
                view_reads: FxHashMap::default(),
                render_dependencies: FxHashMap::default(),
                global_read_dependencies: FxHashMap::default(),
                dirty_frame_diagnostics: Rc::new(RefCell::new(DirtyFrameDiagnostics::default())),
                pending_view_scoped: false,
                pending_layout_animation_frames: FxHashSet::default(),
                pending_layout_animation_deadlines: FxHashMap::default(),
                layout_animation_deadline_generation: 0,
                active_view_scoped: false,
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
        layout_key: u64,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let key = (entity, layout_key);
        inner.pending_layout_animation_deadlines.remove(&key);
        inner.pending_layout_animation_frames.insert(key)
    }

    /// Consume the pending sample for this native element key.
    pub(in crate::window) fn take_layout_animation_frame(
        &self,
        entity: EntityId,
        layout_key: u64,
    ) -> bool {
        self.inner.borrow_mut().pending_layout_animation_frames.remove(&(entity, layout_key))
    }

    /// Arms or tightens a delayed layout-animation sample for one exact retained target.
    ///
    /// Returns the generation assigned to a newly armed timer. `None` means an immediate ticket is
    /// already pending or an earlier/equal deadline already covers the requested sample.
    pub(in crate::window) fn arm_layout_animation_deadline(
        &self,
        entity: EntityId,
        layout_key: u64,
        deadline: Instant,
    ) -> Option<u64> {
        let mut inner = self.inner.borrow_mut();
        let key = (entity, layout_key);
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
        layout_key: u64,
        deadline: Instant,
        generation: u64,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let key = (entity, layout_key);
        if inner.pending_layout_animation_deadlines.get(&key).copied()
            != Some((deadline, generation))
        {
            return false;
        }
        inner.pending_layout_animation_deadlines.remove(&key);
        true
    }

    #[track_caller]
    pub fn invalidate_view(&self, entity: EntityId, cx: &mut App) -> bool {
        self.invalidate_entity(entity, cx, false)
    }

    /// Publish the current read log and the owners whose lifecycle was replayed.
    /// Like Fast's tracked-entity reconciliation, unchanged read edges keep
    /// their committed reverse routes. Rebuild only after a real change.
    pub(in crate::window) fn commit_frame_dependencies(
        &self,
        log: &mut crate::AppDependencies,
        entities: &mut FxHashSet<EntityId>,
        replayed: &FxHashSet<EntityId>,
        mut is_live: impl FnMut(EntityId) -> bool,
        mut has_route: impl FnMut(EntityId) -> bool,
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;
        let mut changed = false;
        for (owner, entry) in &mut inner.view_reads {
            let route = owner.filter(|owner| has_route(*owner));
            changed |= route != entry.route;
            entry.route = route;
            entry.keep = match *owner {
                Some(owner) => replayed.contains(&owner) && route.is_some(),
                None => !replayed.is_empty(),
            };
        }

        log.finish(|owner, mut reads| {
            if let Some(entry) = inner.view_reads.get_mut(&owner) {
                if entry.keep {
                    reads.merge_replayed(&entry.reads);
                }
                changed |= !entry.reads.same_routes(&reads);
                entry.reads = reads;
                entry.keep = true;
            } else {
                changed |= !reads.entities.is_empty() || !reads.globals.is_empty();
                inner.view_reads.insert(owner, CommittedReads {
                    reads,
                    route: owner.filter(|owner| has_route(*owner)),
                    keep: true,
                });
            }
        });

        inner.view_reads.retain(|_, entry| {
            if !entry.keep {
                changed |= !entry.reads.entities.is_empty() || !entry.reads.globals.is_empty();
                return false;
            }
            let reads = &mut entry.reads;
            if !reads.entities.iter().all(|entity| is_live(*entity)) {
                reads.entities = reads.entities.iter().copied().filter(|entity| is_live(*entity)).collect();
                changed = true;
            }
            true
        });

        if changed {
            inner.render_dependencies.clear();
            inner.global_read_dependencies.clear();
            for entry in inner.view_reads.values() {
                for &entity in entry.reads.entities.iter() {
                    insert_dependency_owner(&mut inner.render_dependencies, entity, entry.route);
                }
                for &key in entry.reads.globals.iter() {
                    insert_dependency_owner(&mut inner.global_read_dependencies, key, entry.route);
                }
            }
        }
        entities.extend(inner.render_dependencies.keys().copied());
        changed
    }

    /// Poll the committed state reads once. Like entity and global reads,
    /// a changed read without a resolved route requires a full traversal.
    /// Returns whether scoped invalidation is insufficient.
    pub(in crate::window) fn changed_state_owners(&self, changed: &mut SmallVec<[EntityId; 4]>) -> bool {
        let mut inner = self.inner.borrow_mut();
        let mut unscoped = false;
        for entry in inner.view_reads.values_mut() {
            let mut has_change = false;
            let mut has_dead = false;
            for read in entry.reads.states.iter() {
                match read.status() {
                    crate::StateReadStatus::Changed => has_change = true,
                    crate::StateReadStatus::Dead => has_dead = true,
                    crate::StateReadStatus::Unchanged => {}
                }
            }
            if has_change {
                match entry.route {
                    Some(owner) if !changed.contains(&owner) => changed.push(owner),
                    Some(_) => {}
                    None => unscoped = true,
                }
            }
            if has_dead {
                entry.reads.states = entry.reads.states.iter()
                    .filter(|read| read.status() != crate::StateReadStatus::Dead)
                    .cloned()
                    .collect();
            }
        }
        unscoped
    }

    /// Global notifications are already dispatched by the App effect queue.
    /// Never synthesize an Entity Notify for the dependent views.
    pub(crate) fn invalidate_global_dependency(&self, global: TypeId, exists: bool) {
        let mut inner = self.inner.borrow_mut();
        let mut owners = SmallVec::<[EntityId; 4]>::new();
        // A value read always depends on a write. Existence reads depend
        // only on a change from the observed boolean (including absence).
        for key in [(global, None), (global, Some(!exists))] {
            match inner.global_read_dependencies.get(&key) {
                Some(Some(readers)) => {
                    for &reader in readers {
                        if !owners.contains(&reader) {
                            owners.push(reader);
                        }
                    }
                }
                Some(None) => {
                    drop(inner);
                    self.set_dirty(true);
                    return;
                }
                None => {}
            }
        }
        if owners.is_empty() {
            return;
        }
        inner.dirty_to_present_started_at.get_or_insert_with(Instant::now);
        inner.dirty_views.extend(owners.iter().copied());
        if !inner.dirty {
            inner.pending_view_scoped = true;
        }
        inner.dirty = true;
    }

    /// Window-owned dependencies do not emit application-level Entity Notify.
    pub(in crate::window) fn invalidate_dependent_views(
        &self,
        owners: impl IntoIterator<Item = EntityId>,
    ) -> bool {
        let mut owners = owners.into_iter().peekable();
        if owners.peek().is_none() {
            return false;
        }
        let mut inner = self.inner.borrow_mut();
        inner.dirty_to_present_started_at.get_or_insert_with(Instant::now);
        if inner.draw_phase != DrawPhase::None {
            inner.dirty_views.extend(owners);
            return false;
        }
        if !inner.dirty {
            inner.pending_view_scoped = true;
        }
        inner.dirty_views.extend(owners);
        inner.dirty = true;
        true
    }

    /// Only application notifications resolve read dependencies. Direct
    /// window invalidations still name the actual view to invalidate.
    #[track_caller]
    pub(crate) fn invalidate_dependency(&self, entity: EntityId, cx: &mut App) -> bool {
        self.invalidate_entity(entity, cx, true)
    }

    #[track_caller]
    fn invalidate_entity(&self, entity: EntityId, cx: &mut App, resolve_dependencies: bool) -> bool {
        let mut inner = self.inner.borrow_mut();
        let (owners, unknown) = if resolve_dependencies {
            match inner.render_dependencies.get(&entity) {
                Some(Some(owners)) => (owners.clone(), false),
                Some(None) => (smallvec::smallvec![entity], true),
                None => (smallvec::smallvec![entity], false),
            }
        } else {
            (smallvec::smallvec![entity], false)
        };
        let location = std::panic::Location::caller();
        let mut diagnostics = inner.dirty_frame_diagnostics.borrow_mut();
        diagnostics.record_frame_request_reason_at(
            FrameRequestReason::StateNotify,
            location.file(),
            location.line(),
        );
        diagnostics.record_notify_invalidation(entity);
        drop(diagnostics);
        inner
            .dirty_to_present_started_at
            .get_or_insert_with(Instant::now);
        inner.dirty_views.extend(owners.iter().copied());
        if inner.draw_phase == DrawPhase::None {
            if unknown {
                inner.pending_view_scoped = false;
            } else if !inner.dirty {
                inner.pending_view_scoped = true;
            }

            inner.dirty = true;
            // Retained targeting changes only rendering provenance. Context::notify observers keep
            // the same semantics and are still dispatched by the normal Notify effect.
            cx.push_effect(Effect::Notify { emitter: entity });
            true
        } else {
            false
        }
    }

    /// Invalidate the owning view without dispatching an application Notify.
    pub(in crate::window) fn invalidate_owned_view(&self, entity: EntityId) -> bool {
        self.inner.borrow().dirty_frame_diagnostics.borrow_mut()
            .record_notify_invalidation(entity);
        self.invalidate_dependent_views(std::iter::once(entity))
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    /// An unclassified refresh invalidates all retained views.
    pub fn set_dirty(&self, is_dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        if is_dirty {
            inner.dirty_to_present_started_at.get_or_insert_with(Instant::now);
            inner.pending_view_scoped = false;
            inner.active_view_scoped = false;
        } else {
            inner.active_view_scoped = inner.pending_view_scoped;
            inner.pending_view_scoped = false;
        }
        inner.dirty = is_dirty;
    }

    /// Window-owned overlay work does not invalidate an application view.
    pub(in crate::window) fn set_replay_only_dirty(&self) {
        let mut inner = self.inner.borrow_mut();
        inner
            .dirty_to_present_started_at
            .get_or_insert_with(Instant::now);
        if inner.draw_phase != DrawPhase::None {
            inner.dirty = true;
            return;
        }

        if !inner.dirty {
            inner.pending_view_scoped = true;
        }
        inner.dirty = true;
    }

    pub(in crate::window) fn take_dirty_to_present_start(&self) -> Option<Instant> {
        self.inner.borrow_mut().dirty_to_present_started_at.take()
    }

    pub(in crate::window) fn reset_dirty_to_present_epoch(&self, restart_if_dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.dirty_to_present_started_at = if restart_if_dirty && inner.dirty {
            Some(Instant::now())
        } else {
            None
        };
    }

    pub(in crate::window) fn active_view_scoped(&self) -> bool {
        self.inner.borrow().active_view_scoped
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

    pub(in crate::window) fn trim_retained_capacity(&self, level: GpuiMemoryTrimLevel) {
        let mut inner = self.inner.borrow_mut();
        let floor = if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            0
        } else {
            16
        };
        let eager = !matches!(level, GpuiMemoryTrimLevel::Light);

        macro_rules! trim_collection {
            ($field:ident) => {{
                let target = floor.max(inner.$field.len());
                if eager || inner.$field.capacity() > target.saturating_mul(4) {
                    inner.$field.shrink_to(target);
                }
            }};
        }

        trim_collection!(dirty_views);
        trim_collection!(view_reads);
        trim_collection!(render_dependencies);
        trim_collection!(global_read_dependencies);
        trim_collection!(pending_layout_animation_frames);
        trim_collection!(pending_layout_animation_deadlines);
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

fn insert_dependency_owner<K: Eq + std::hash::Hash>(
    index: &mut FxHashMap<K, Option<SmallVec<[EntityId; 2]>>>,
    key: K,
    owner: Option<EntityId>,
) {
    match owner {
        None => { index.insert(key, None); }
        Some(owner) => {
            if let Some(owners) = index.entry(key).or_insert_with(|| Some(SmallVec::new())) {
                owners.push(owner);
            }
        }
    }
}

#[cfg(test)]
mod view_invalidation_tests {
    use super::*;

    #[test]
    fn owned_view_invalidation_coalesces_by_entity() {
        let invalidator = WindowInvalidator::new();
        let first = EntityId::from(1);
        let second = EntityId::from(2);
        invalidator.set_dirty(false);
        assert!(invalidator.invalidate_owned_view(first));
        assert!(invalidator.invalidate_owned_view(first));
        assert!(invalidator.invalidate_owned_view(second));
        assert_eq!(invalidator.take_views(), FxHashSet::from_iter([first, second]));
        invalidator.set_dirty(false);
        assert!(invalidator.active_view_scoped());
    }

    #[test]
    fn unclassified_refresh_cannot_be_narrowed_by_owned_view_invalidation() {
        let invalidator = WindowInvalidator::new();
        invalidator.set_dirty(true);
        invalidator.invalidate_owned_view(EntityId::from(1));
        invalidator.set_dirty(false);
        assert!(!invalidator.active_view_scoped());
    }

    #[test]
    fn replay_carries_only_reused_owners_and_replaces_rebuilt_reads() {
        let invalidator = WindowInvalidator::new();
        let model = EntityId::from(11);
        let replayed = EntityId::from(12);
        let rebuilt = EntityId::from(13);
        let removed = EntityId::from(14);
        let global = TypeId::of::<u32>();
        let mut entities = FxHashSet::default();
        let mut log = crate::AppDependencies::default();
        for owner in [replayed, rebuilt, removed] {
            log.entity(Some(owner), model);
            log.global(Some(owner), (global, None));
        }
        invalidator.commit_frame_dependencies(&mut log, &mut entities, &FxHashSet::default(), |_| true, |_| true);
        entities.clear();
        let new_model = EntityId::from(15);
        log.entity(Some(rebuilt), new_model);
        invalidator.commit_frame_dependencies(&mut log, &mut entities, &FxHashSet::from_iter([replayed]), |_| true, |owner| owner != removed);
        let inner = invalidator.inner.borrow();
        assert_eq!(inner.render_dependencies[&model].as_ref().unwrap().as_slice(), &[replayed]);
        assert_eq!(inner.render_dependencies[&new_model].as_ref().unwrap().as_slice(), &[rebuilt]);
        assert_eq!(inner.global_read_dependencies[&(global, None)].as_ref().unwrap().as_slice(), &[replayed]);
        assert!(!inner.view_reads.contains_key(&Some(removed)));
    }

    #[test]
    fn identical_rebuilt_reads_keep_reverse_routes_and_route_loss_invalidates() {
        let invalidator = WindowInvalidator::new();
        let owner = EntityId::from(1);
        let model = EntityId::from(2);
        let global = TypeId::of::<u32>();
        let mut log = crate::AppDependencies::default();
        let mut entities = FxHashSet::default();
        for frame in 0..3 {
            log.entity(Some(owner), model);
            log.global(Some(owner), (global, None));
            entities.clear();
            let changed = invalidator.commit_frame_dependencies(
                &mut log, &mut entities, &FxHashSet::default(), |_| true, |_| frame != 2,
            );
            assert_eq!(changed, frame != 1);
            assert_eq!(entities, FxHashSet::from_iter([model]));
        }
        assert_eq!(invalidator.inner.borrow().render_dependencies[&model], None);
        assert_eq!(invalidator.inner.borrow().global_read_dependencies[&(global, None)], None);
        assert!(invalidator.commit_frame_dependencies(
            &mut log, &mut entities, &FxHashSet::default(), |_| true, |_| false,
        ));
        assert!(invalidator.inner.borrow().render_dependencies.is_empty());
    }

    #[test]
    fn state_reads_invalidate_resolved_routes_and_fall_back_for_unscoped_reads() {
        let invalidator = WindowInvalidator::new();
        let scoped = crate::StateVersion::default();
        let unscoped = crate::StateVersion::default();
        let orphaned = crate::StateVersion::default();
        let owner = EntityId::from(1);
        let orphan = EntityId::from(2);
        let mut log = crate::AppDependencies::default();
        let recording = log.begin_state_reads();
        unscoped.read();
        log.set_owner(Some(owner));
        scoped.read();
        log.set_owner(Some(orphan));
        orphaned.read();
        drop(recording);
        invalidator.commit_frame_dependencies(
            &mut log, &mut FxHashSet::default(), &FxHashSet::default(),
            |_| true, |id| id != orphan,
        );

        let mut changed = SmallVec::new();
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert!(changed.is_empty());
        scoped.bump();
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert_eq!(changed.as_slice(), &[owner]);

        orphaned.bump();
        assert!(invalidator.changed_state_owners(&mut changed));
        drop(orphaned);
        assert!(!invalidator.changed_state_owners(&mut changed));
        unscoped.bump();
        assert!(invalidator.changed_state_owners(&mut changed));
        assert_eq!(changed.as_slice(), &[owner]);
    }

    #[test]
    fn global_value_changes_do_not_invalidate_existence_only_readers() {
        let invalidator = WindowInvalidator::new();
        let global = TypeId::of::<u32>();
        let value_reader = EntityId::from(1);
        let present_reader = EntityId::from(2);
        let absent_reader = EntityId::from(3);
        let mut log = crate::AppDependencies::default();
        log.global(Some(value_reader), (global, None));
        log.global(Some(present_reader), (global, Some(true)));
        log.global(Some(absent_reader), (global, Some(false)));
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);

        invalidator.set_dirty(false);
        invalidator.invalidate_global_dependency(TypeId::of::<u64>(), true);
        assert!(!invalidator.is_dirty());
        invalidator.invalidate_global_dependency(global, true);
        assert_eq!(invalidator.take_views(), FxHashSet::from_iter([value_reader, absent_reader]));
        invalidator.set_dirty(false);
        assert!(invalidator.active_view_scoped());
        invalidator.invalidate_global_dependency(global, false);
        assert_eq!(invalidator.take_views(), FxHashSet::from_iter([value_reader, present_reader]));
    }

    #[test]
    fn unscoped_reads_dominate_until_complete_traversal() {
        let invalidator = WindowInvalidator::new();
        let owner = EntityId::from(1);
        let model = EntityId::from(2);
        let global = TypeId::of::<u32>();
        let mut log = crate::AppDependencies::default();
        log.entity(None, model);
        log.global(None, (global, None));
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);
        log.entity(Some(owner), model);
        log.global(Some(owner), (global, None));
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::from_iter([owner]), |_| true, |_| true);
        assert_eq!(invalidator.inner.borrow().render_dependencies[&model], None);
        assert_eq!(invalidator.inner.borrow().global_read_dependencies[&(global, None)], None);
        log.entity(Some(owner), model);
        log.global(Some(owner), (global, None));
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);
        assert_eq!(invalidator.inner.borrow().render_dependencies[&model].as_ref().unwrap().as_slice(), &[owner]);
    }

    #[gpui::test]
    fn render_dependency_notification_targets_all_readers_but_not_unrelated_views(
        cx: &mut crate::TestAppContext,
    ) {
        cx.update(|cx| {
            let invalidator = WindowInvalidator::new();
            let model = EntityId::from(101);
            let first = EntityId::from(102);
            let second = EntityId::from(103);
            let mut log = crate::AppDependencies::default();
            log.entity(Some(first), model);
            log.entity(Some(second), model);
            invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);
            invalidator.set_dirty(false);
            assert!(invalidator.invalidate_dependency(model, cx));
            assert_eq!(invalidator.take_views(), FxHashSet::from_iter([first, second]));
        });
    }

    #[test]
    fn layout_animation_frame_ticket_is_per_target() {
        let invalidator = WindowInvalidator::new();
        let entity = EntityId::from(1);
        let first = 1;
        let second = 2;
        assert!(invalidator.arm_layout_animation_frame(entity, first));
        assert!(!invalidator.arm_layout_animation_frame(entity, first));
        assert!(invalidator.arm_layout_animation_frame(entity, second));
        assert!(invalidator.take_layout_animation_frame(entity, first));
        assert!(!invalidator.take_layout_animation_frame(entity, first));
        assert!(invalidator.take_layout_animation_frame(entity, second));
    }

    #[test]
    fn immediate_layout_animation_ticket_supersedes_delayed_ticket() {
        let invalidator = WindowInvalidator::new();
        let entity = EntityId::from(1);
        let target = 1;
        let deadline = Instant::now() + Duration::from_millis(8);
        let generation = invalidator
            .arm_layout_animation_deadline(entity, target, deadline)
            .expect("first delayed ticket");

        assert!(invalidator.arm_layout_animation_frame(entity, target));
        assert!(
            !invalidator.take_layout_animation_deadline(entity, target, deadline, generation,)
        );
    }

    #[test]
    fn earlier_layout_animation_deadline_replaces_later_generation() {
        let invalidator = WindowInvalidator::new();
        let entity = EntityId::from(1);
        let target = 1;
        let now = Instant::now();
        let later = now + Duration::from_millis(12);
        let earlier = now + Duration::from_millis(4);
        let old_generation = invalidator
            .arm_layout_animation_deadline(entity, target, later)
            .expect("initial delayed ticket");
        let new_generation = invalidator
            .arm_layout_animation_deadline(entity, target, earlier)
            .expect("earlier delayed ticket");

        assert_ne!(old_generation, new_generation);
        assert!(!invalidator.take_layout_animation_deadline(
            entity,
            target,
            later,
            old_generation,
        ));
        assert!(invalidator.take_layout_animation_deadline(
            entity,
            target,
            earlier,
            new_generation,
        ));
    }
}

pub(crate) type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;

pub(crate) type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;
