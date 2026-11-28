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
/// Additive owner-scoped dependency adoption. Retired row reads remain
/// conservative until the ordinary traversal reconciles the owner.
pub(super) struct BoundaryRowReads {
    owner: EntityId,
    merged: crate::RenderDependencies,
}

struct CommittedReads {
    reads: crate::RenderDependencies,
    own_reads: crate::OwnerOwnReads,
    state_validation: crate::StateReadValidation,
    route: Option<EntityId>,
    keep: bool,
}

/// An identity index into the existing committed owner records. Only
/// larger owner sets pay the indexing cost; no dependency payload is copied.
#[derive(Default)]
struct OwnerStateRouting {
    index: Vec<(usize, Option<EntityId>)>,
    cursor: Option<crate::StateMutationCursor>,
    pending: SmallVec<[Option<EntityId>; 4]>,
}

impl OwnerStateRouting {
    const SPARSE_THRESHOLD: usize = 16;

    fn reset(&mut self) {
        self.index.clear();
        self.cursor = None;
        self.pending.clear();
    }

    fn trim(&mut self, owners: usize, eager: bool) {
        if eager {
            self.reset();
            self.index.shrink_to_fit();
            self.pending.shrink_to_fit();
        } else if self.index.is_empty() && self.index.capacity() > owners.max(16).saturating_mul(4) {
            self.index.shrink_to(owners.max(16));
        }
    }
}

fn poll_owner_states(entry: &mut CommittedReads) -> bool {
    if entry.reads.states.is_empty() {
        return false;
    }
    let mut has_change = false;
    let mut dead = SmallVec::<[usize; 4]>::new();
    entry.state_validation.poll(entry.reads.states.iter().enumerate(), |index, status| {
        match status {
            crate::StateReadStatus::Changed => has_change = true,
            crate::StateReadStatus::Dead => dead.push(index),
            crate::StateReadStatus::Unchanged => {}
        }
    });
    if !dead.is_empty() {
        let mut dead = dead.into_iter().peekable();
        entry.reads.states = entry.reads.states.iter().enumerate()
            .filter_map(|(index, read)| {
                if dead.peek() == Some(&index) {
                    dead.next();
                    None
                } else {
                    Some(read.clone())
                }
            })
            .collect();
        entry.state_validation = Default::default();
    }
    has_change
}

fn report_state_owner(route: Option<EntityId>, changed: &mut SmallVec<[EntityId; 4]>, unscoped: &mut bool) {
    match route {
        Some(owner) if !changed.contains(&owner) => changed.push(owner),
        Some(_) => {}
        None => *unscoped = true,
    }
}

// Bounded history of actual application notifications. Row routing borrows
// this history instead of recording another stream or subscribing per row.
#[derive(Clone, Copy)]
pub(super) enum RowNotification {
    Entity(EntityId),
    Global(TypeId, bool),
}

impl RowNotification {
    fn affects(self, reads: &crate::RenderDependencies) -> bool {
        self.matches(&reads.entities, &reads.globals)
    }

    fn matches(self, entities: &[EntityId], globals: &[(TypeId, Option<bool>)]) -> bool {
        match self {
            Self::Entity(entity) => entities.binary_search(&entity).is_ok(),
            Self::Global(global, exists) => {
                globals.binary_search(&(global, None)).is_ok()
                    || globals.binary_search(&(global, Some(!exists))).is_ok()
            }
        }
    }
}

impl CommittedReads {
    fn row_only_notification(&self, owner: EntityId, event: RowNotification) -> bool {
        self.route == Some(owner)
            && !matches!(event, RowNotification::Entity(entity) if entity == owner)
            && event.affects(&self.reads)
            && !event.matches(&self.own_reads.entities, &self.own_reads.globals)
    }
}

#[derive(Default)]
struct RowNotifications {
    sequence: u64,
    events: std::collections::VecDeque<RowNotification>,
}

impl RowNotifications {
    const CAPACITY: usize = 128;

    fn push(&mut self, event: RowNotification) {
        if self.events.len() == Self::CAPACITY {
            self.events.pop_front();
        }
        self.events.push_back(event);
        self.sequence = self.sequence.wrapping_add(1);
    }

    fn visit_since(&self, since: u64, mut visit: impl FnMut(u64, RowNotification)) -> Option<u64> {
        let count = self.sequence.wrapping_sub(since);
        if count > self.events.len() as u64 {
            return None;
        }
        let start = self.events.len() - count as usize;
        for (offset, &event) in self.events.iter().skip(start).enumerate() {
            visit(since.wrapping_add(offset as u64 + 1), event);
        }
        Some(self.sequence)
    }

    fn unchanged(&self, since: u64, reads: &crate::RenderDependencies) -> bool {
        if reads.entities.is_empty() && reads.globals.is_empty() {
            return true;
        }
        let count = self.sequence.wrapping_sub(since);
        if count > self.events.len() as u64 {
            // A lagging row cannot prove that no relevant event was lost.
            return false;
        }
        !self.events.iter().rev().take(count as usize).any(|event| event.affects(reads))
    }
}

// Requests are accumulated until begin_draw_cycle consumes them. In
// particular a wheel event must not turn a pending model/overlay refresh
// into a placement-only frame.
#[derive(Default, Clone, Copy)]
struct FrameWork(u8);

impl FrameWork {
    const PLACEMENT: u8 = 1;
    const CONTENT: u8 = 2;
    const OVERLAY: u8 = 4;

    fn insert(&mut self, work: u8) { self.0 |= work; }
    fn contains(self, work: u8) -> bool { self.0 & work != 0 }
    fn placement_only(self) -> bool { self.0 == Self::PLACEMENT }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PlacementEpoch {
    pub(super) content: u64,
    placement: u64,
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
    owner_state_routing: OwnerStateRouting,
    row_notifications: RowNotifications,
    pending_row_only: FxHashSet<EntityId>,
    active_row_only: FxHashSet<EntityId>,
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
    // Placement requests stay distinct from content owners until the
    // conservative traversal consumes them. Usually one native scroller.
    pending_placement_views: SmallVec<[EntityId; 4]>,
    scroll_content_revision: u64,
    scroll_placement_revision: u64,
    pending_work: FrameWork,
    drawing_placement: Option<PlacementEpoch>,
    drawing_scoped_content: Option<PlacementEpoch>,
}

impl WindowInvalidatorInner {
    fn mark_dependency_dirty(&mut self, owner: EntityId, event: Option<RowNotification>) {
        let row_only = event.is_some_and(|event| self.view_reads.get(&Some(owner))
            .is_some_and(|entry| entry.row_only_notification(owner, event)));
        // Every cause for this owner must be row-only.
        if row_only {
            if !self.dirty_views.contains(&owner) {
                self.pending_row_only.insert(owner);
            }
        } else {
            self.pending_row_only.remove(&owner);
        }
        self.dirty_views.insert(owner);
    }
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
                owner_state_routing: OwnerStateRouting::default(),
                row_notifications: RowNotifications::default(),
                pending_row_only: FxHashSet::default(),
                active_row_only: FxHashSet::default(),
                render_dependencies: FxHashMap::default(),
                global_read_dependencies: FxHashMap::default(),
                dirty_frame_diagnostics: Rc::new(RefCell::new(DirtyFrameDiagnostics::default())),
                pending_view_scoped: false,
                pending_layout_animation_frames: FxHashSet::default(),
                pending_layout_animation_deadlines: FxHashMap::default(),
                layout_animation_deadline_generation: 0,
                active_view_scoped: false,
                pending_placement_views: SmallVec::new(),
                scroll_content_revision: 0,
                scroll_placement_revision: 0,
                pending_work: FrameWork(FrameWork::CONTENT),
                drawing_placement: None,
                drawing_scoped_content: None,
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
    /// their committed reverse routes. Apply scoped edge deltas in place;
    /// rebuild only when removing an unresolved masking edge.
    /// Prepare a union without modifying the committed reverse indexes.
    /// Native state reads require a separate snapshot transaction.
    pub(super) fn prepare_boundary_row_reads(
        &self, owner: EntityId, reads: &crate::RenderDependencies,
    ) -> Option<BoundaryRowReads> {
        if !reads.states.is_empty() { return None; }
        let inner = self.inner.borrow();
        let mut merged = reads.clone();
        if let Some(old) = inner.view_reads.get(&Some(owner)) {
            if old.route != Some(owner) { return None; }
            merged.merge_replayed(&old.reads);
            // Bound conservative growth until a full traversal removes
            // retired edges. Already-known large owner sets stay valid.
            if merged.entities.len() > old.reads.entities.len().max(4096)
                || merged.globals.len() > old.reads.globals.len().max(256)
            { return None; }
        } else if merged.entities.len() > 4096 || merged.globals.len() > 256 {
            return None;
        }
        Some(BoundaryRowReads { owner, merged })
    }

    /// No callbacks or fallible admission after the placement epoch is
    /// consumed. Preserve own_reads: these additions came only from rows.
    pub(super) fn commit_boundary_row_reads(&self, prepared: BoundaryRowReads) {
        let mut inner = self.inner.borrow_mut();
        let inner = &mut *inner;
        let route = Some(prepared.owner);
        let entry = inner.view_reads.entry(route).or_insert_with(|| CommittedReads {
            reads: Default::default(), own_reads: Default::default(),
            state_validation: Default::default(), route, keep: true,
        });
        debug_assert_eq!(entry.route, route);
        update_dependency_routes(&mut inner.render_dependencies,
            &entry.reads.entities, route, &prepared.merged.entities, route);
        update_dependency_routes(&mut inner.global_read_dependencies,
            &entry.reads.globals, route, &prepared.merged.globals, route);
        entry.reads = prepared.merged;
        entry.keep = true;
    }

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
        let mut rebuild_routes = false;
        let mut state_snapshots_changed = false;
        let entity_routes = &mut inner.render_dependencies;
        let global_routes = &mut inner.global_read_dependencies;
        for (owner, entry) in &mut inner.view_reads {
            let route = owner.filter(|owner| has_route(*owner));
            changed |= route != entry.route;
            entry.keep = match *owner {
                Some(owner) => replayed.contains(&owner) && route.is_some(),
                None => !replayed.is_empty(),
            };
            // Owners not replayed may be rebuilt or retired. Keep their
            // old route until that decision, avoiding a transient unresolved
            // edge that would force a rebuild on an ordinary retirement.
            if entry.keep && route != entry.route {
                rebuild_routes |= update_dependency_routes(entity_routes, &entry.reads.entities, entry.route, &entry.reads.entities, route);
                rebuild_routes |= update_dependency_routes(global_routes, &entry.reads.globals, entry.route, &entry.reads.globals, route);
                entry.route = route;
            }
        }

        log.finish(|owner, mut reads, mut own_reads| {
            if let Some(entry) = inner.view_reads.get_mut(&owner) {
                if entry.keep {
                    reads.merge_replayed(&entry.reads);
                    own_reads.merge_replayed(&entry.own_reads);
                }
                let route = owner.filter(|owner| has_route(*owner));
                if route != entry.route || !entry.reads.same_routes(&reads) {
                    changed = true;
                    rebuild_routes |= update_dependency_routes(entity_routes, &entry.reads.entities, entry.route, &reads.entities, route);
                    rebuild_routes |= update_dependency_routes(global_routes, &entry.reads.globals, entry.route, &reads.globals, route);
                    entry.route = route;
                }
                if !Rc::ptr_eq(&entry.reads.states, &reads.states) {
                    if entry.reads.states == reads.states {
                        // Identical observations keep their physical snapshot
                        // and the previous routing/validation proof.
                        reads.states = entry.reads.states.clone();
                    } else {
                        state_snapshots_changed = true;
                        entry.state_validation = Default::default();
                    }
                }
                entry.reads = reads;
                entry.own_reads = own_reads;
                entry.keep = true;
            } else {
                let route = owner.filter(|owner| has_route(*owner));
                changed |= !reads.entities.is_empty() || !reads.globals.is_empty();
                state_snapshots_changed |= !reads.states.is_empty();
                rebuild_routes |= update_dependency_routes(entity_routes, &[], route, &reads.entities, route);
                rebuild_routes |= update_dependency_routes(global_routes, &[], route, &reads.globals, route);
                inner.view_reads.insert(owner, CommittedReads {
                    reads,
                    own_reads,
                    state_validation: Default::default(),
                    route,
                    keep: true,
                });
            }
        });

        inner.view_reads.retain(|_, entry| {
            if !entry.keep {
                changed |= !entry.reads.entities.is_empty() || !entry.reads.globals.is_empty();
                state_snapshots_changed |= !entry.reads.states.is_empty();
                rebuild_routes |= update_dependency_routes(entity_routes, &entry.reads.entities, entry.route, &[], entry.route);
                rebuild_routes |= update_dependency_routes(global_routes, &entry.reads.globals, entry.route, &[], entry.route);
                return false;
            }
            let reads = &mut entry.reads;
            if !reads.entities.iter().all(|entity| is_live(*entity)) {
                let live: Rc<[EntityId]> = reads.entities.iter().copied().filter(|entity| is_live(*entity)).collect();
                rebuild_routes |= update_dependency_routes(entity_routes, &reads.entities, entry.route, &live, entry.route);
                reads.entities = live;
                changed = true;
            }
            true
        });

        if state_snapshots_changed {
            inner.owner_state_routing.reset();
        }
        // An unresolved read masks all scoped readers in the compact
        // Option index. Only removing such a read needs full reconstruction.
        if rebuild_routes {
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
        let inner = &mut *inner;
        let routing = &mut inner.owner_state_routing;
        let owners = &mut inner.view_reads;
        let mut unscoped = false;
        if owners.len() < OwnerStateRouting::SPARSE_THRESHOLD {
            routing.reset();
            for entry in owners.values_mut() {
                if poll_owner_states(entry) {
                    report_state_owner(entry.route, changed, &mut unscoped);
                }
            }
            return unscoped;
        }

        if let Some(cursor) = routing.cursor {
            let index = &routing.index;
            let pending = &mut routing.pending;
            if let Some(next) = cursor.visit_since(owners.len() / 2, |id| {
                let start = index.partition_point(|&(identity, _)| identity < id);
                let end = index.partition_point(|&(identity, _)| identity <= id);
                pending.extend(index[start..end].iter().map(|&(_, owner)| owner));
            }) {
                routing.cursor = Some(next);
                pending.sort_unstable();
                pending.dedup();
                pending.retain(|owner| {
                    let Some(entry) = owners.get_mut(owner) else { return false; };
                    if poll_owner_states(entry) {
                        report_state_owner(entry.route, changed, &mut unscoped);
                        true
                    } else {
                        false
                    }
                });
                return unscoped;
            }
        }

        // Initial publication and journal overflow use the authoritative
        // snapshots. Rebuild the identity table here to retire dead edges.
        routing.index.clear();
        routing.pending.clear();
        for (&owner, entry) in owners.iter_mut() {
            if poll_owner_states(entry) {
                report_state_owner(entry.route, changed, &mut unscoped);
                routing.pending.push(owner);
            }
            routing.index.extend(entry.reads.states.iter().map(|read| (read.id(), owner)));
        }
        routing.index.sort_unstable();
        routing.cursor = Some(crate::StateMutationCursor::current());
        unscoped
    }

    /// Global notifications are already dispatched by the App effect queue.
    /// Never synthesize an Entity Notify for the dependent views.
    pub(crate) fn invalidate_global_dependency(&self, global: TypeId, exists: bool) {
        let mut inner = self.inner.borrow_mut();
        inner.row_notifications.push(RowNotification::Global(global, exists));
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
        for &owner in &owners {
            inner.mark_dependency_dirty(owner, Some(RowNotification::Global(global, exists)));
        }
        inner.pending_work.insert(FrameWork::CONTENT);
        inner.scroll_content_revision = inner.scroll_content_revision.wrapping_add(1);
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
        self.invalidate_views(owners, false)
    }

    pub(super) fn invalidate_scroll_views(
        &self,
        owners: impl IntoIterator<Item = EntityId>,
    ) -> bool {
        self.invalidate_views(owners, true)
    }

    fn invalidate_views(&self, owners: impl IntoIterator<Item = EntityId>, scroll: bool) -> bool {
        let mut owners = owners.into_iter().peekable();
        if owners.peek().is_none() {
            return false;
        }
        let mut inner = self.inner.borrow_mut();
        if scroll {
            if !inner.pending_work.contains(FrameWork::PLACEMENT) {
                inner.scroll_placement_revision = inner.scroll_placement_revision.wrapping_add(1);
            }
            inner.pending_work.insert(FrameWork::PLACEMENT);
            for owner in owners {
                if !inner.pending_placement_views.contains(&owner) {
                    inner.pending_placement_views.push(owner);
                }
            }
        } else {
            inner.pending_work.insert(FrameWork::CONTENT);
            inner.scroll_content_revision = inner.scroll_content_revision.wrapping_add(1);
            for owner in owners {
                inner.mark_dependency_dirty(owner, None);
            }
        }
        inner.dirty_to_present_started_at.get_or_insert_with(Instant::now);
        if inner.draw_phase != DrawPhase::None {
            return false;
        }
        if !inner.dirty {
            inner.pending_view_scoped = true;
        }
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
            inner.row_notifications.push(RowNotification::Entity(entity));
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
        for &owner in &owners {
            let event = (resolve_dependencies && !unknown).then_some(RowNotification::Entity(entity));
            inner.mark_dependency_dirty(owner, event);
        }
        inner.pending_work.insert(FrameWork::CONTENT);
        inner.scroll_content_revision = inner.scroll_content_revision.wrapping_add(1);
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
            inner.pending_row_only.clear();
            inner.active_row_only.clear();
            inner.pending_work.insert(FrameWork::CONTENT);
            inner.scroll_content_revision = inner.scroll_content_revision.wrapping_add(1);
            inner.drawing_placement = None;
            inner.drawing_scoped_content = None;
        } else {
            inner.active_view_scoped = inner.pending_view_scoped;
            inner.pending_view_scoped = false;
            let inner = &mut *inner;
            mem::swap(&mut inner.pending_row_only, &mut inner.active_row_only);
            inner.pending_row_only.clear();
            if !inner.active_view_scoped {
                inner.active_row_only.clear();
            }
            let work = std::mem::take(&mut inner.pending_work);
            let epoch = PlacementEpoch {
                content: inner.scroll_content_revision,
                placement: inner.scroll_placement_revision,
            };
            inner.drawing_placement =
                (inner.active_view_scoped && work.placement_only()).then_some(epoch);
            inner.drawing_scoped_content = (inner.active_view_scoped
                && work.contains(FrameWork::CONTENT)
                && !work.contains(FrameWork::OVERLAY)).then_some(epoch);
        }
        inner.dirty = is_dirty;
    }

    /// Peek without consuming the ordinary traversal's invalidation set.
    /// A failed direct transaction leaves every pending request intact.
    pub(super) fn pending_placement(&self) -> Option<(PlacementEpoch, SmallVec<[EntityId; 4]>)> {
        let inner = self.inner.borrow();
        if !inner.dirty || inner.draw_phase != DrawPhase::None || !inner.pending_view_scoped
            || !inner.pending_work.placement_only() || !inner.dirty_views.is_empty()
            || inner.pending_placement_views.is_empty()
        {
            return None;
        }
        Some((PlacementEpoch { content: inner.scroll_content_revision,
            placement: inner.scroll_placement_revision }, inner.pending_placement_views.clone()))
    }

    pub(super) fn commit_placement(&self, epoch: PlacementEpoch, owners: &[EntityId]) -> bool {
        if !self.pending_placement().is_some_and(|(pending, expected)| {
            pending == epoch && expected.as_slice() == owners
        }) {
            return false;
        }
        let mut inner = self.inner.borrow_mut();
        inner.pending_placement_views.clear();
        inner.pending_work = FrameWork::default();
        inner.pending_view_scoped = false;
        inner.active_view_scoped = false;
        inner.drawing_placement = None;
        inner.drawing_scoped_content = None;
        inner.dirty = false;
        true
    }

    pub(super) fn scroll_content_revision(&self) -> (u64, bool) {
        let inner = self.inner.borrow();
        let current = PlacementEpoch {
            content: inner.scroll_content_revision,
            placement: inner.scroll_placement_revision,
        };
        (current.content, inner.drawing_placement == Some(current))
    }

    /// Capture the notification boundary before recording row dependencies.
    pub(super) fn row_only_dirty(&self, owner: EntityId) -> bool {
        self.inner.borrow().active_row_only.contains(&owner)
    }

    pub(super) fn row_notification_epoch(&self) -> u64 {
        self.inner.borrow().row_notifications.sequence
    }

    pub(super) fn row_dependencies_unchanged(&self, since: u64, reads: &crate::RenderDependencies) -> bool {
        self.inner.borrow().row_notifications.unchanged(since, reads)
    }

    pub(super) fn visit_row_notifications(
        &self,
        since: u64,
        visit: impl FnMut(u64, RowNotification),
    ) -> Option<u64> {
        self.inner.borrow().row_notifications.visit_since(since, visit)
    }

    /// A scoped content draw can retain rows from a provably clean owner.
    /// The epoch also revokes admission if new work arrives during preparation.
    /// The boolean distinguishes the existing placement-only proof from the
    /// stricter owner-path proof required for a content draw.
    pub(super) fn row_replay_revision(&self) -> Option<(u64, bool)> {
        let inner = self.inner.borrow();
        let current = PlacementEpoch {
            content: inner.scroll_content_revision,
            placement: inner.scroll_placement_revision,
        };
        if inner.drawing_placement == Some(current) {
            Some((current.content, true))
        } else if inner.drawing_scoped_content == Some(current) {
            Some((current.content, false))
        } else {
            None
        }
    }

    /// Window-owned overlay work does not invalidate an application view.
    pub(in crate::window) fn set_replay_only_dirty(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.pending_work.insert(FrameWork::OVERLAY);
        // An overlay change during preparation revokes placement admission
        // without invalidating the content generation itself.
        inner.drawing_placement = None;
        inner.drawing_scoped_content = None;
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
        let mut inner = self.inner.borrow_mut();
        let mut views = mem::take(&mut inner.dirty_views);
        // Until direct placement publication is admitted, retain the full
        // existing invalidation semantics: a scroll owner is traversed.
        let inner = &mut *inner;
        for owner in inner.pending_placement_views.drain(..) {
            inner.pending_row_only.remove(&owner);
            views.insert(owner);
        }
        views
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
        trim_collection!(pending_row_only);
        trim_collection!(active_row_only);
        if eager {
            inner.pending_placement_views.shrink_to_fit();
        }
        trim_collection!(view_reads);
        let owner_count = inner.view_reads.len();
        inner.owner_state_routing.trim(owner_count, eager);
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

/// Remove a scoped edge in place. An unresolved edge hides the scoped
/// reader set, so its removal requests the existing full-rebuild fallback.
fn remove_dependency_owner<K: Copy + Eq + std::hash::Hash>(
    index: &mut FxHashMap<K, Option<SmallVec<[EntityId; 2]>>>,
    key: K,
    owner: Option<EntityId>,
) -> bool {
    let Some(owner) = owner else { return true; };
    if let Some(Some(owners)) = index.get_mut(&key) {
        if let Some(position) = owners.iter().position(|&reader| reader == owner) {
            owners.swap_remove(position);
        }
        if owners.is_empty() {
            index.remove(&key);
        }
    }
    false
}

/// Apply the set difference of two sorted unique snapshots, leaving common
/// reverse entries and their allocations untouched.
fn update_dependency_routes<K: Copy + Ord + std::hash::Hash>(
    index: &mut FxHashMap<K, Option<SmallVec<[EntityId; 2]>>>,
    old: &[K],
    old_route: Option<EntityId>,
    new: &[K],
    new_route: Option<EntityId>,
) -> bool {
    if old_route == new_route && std::ptr::eq(old, new) {
        return false;
    }
    let mut rebuild = false;
    if old_route != new_route {
        for &key in old {
            rebuild |= remove_dependency_owner(index, key, old_route);
        }
        for &key in new {
            insert_dependency_owner(index, key, new_route);
        }
        return rebuild;
    }
    let (mut i, mut j) = (0, 0);
    while i < old.len() && j < new.len() {
        match old[i].cmp(&new[j]) {
            std::cmp::Ordering::Less => {
                rebuild |= remove_dependency_owner(index, old[i], old_route);
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                insert_dependency_owner(index, new[j], new_route);
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
        }
    }
    for &key in &old[i..] {
        rebuild |= remove_dependency_owner(index, key, old_route);
    }
    for &key in &new[j..] {
        insert_dependency_owner(index, key, new_route);
    }
    rebuild
}

#[cfg(test)]
mod view_invalidation_tests {
    use super::*;

    #[test]
    fn placement_commit_consumes_only_the_exact_pending_transaction() {
        let invalidator = WindowInvalidator::new();
        invalidator.set_dirty(false);
        let owner = EntityId::from(1);
        invalidator.invalidate_scroll_views([owner]);
        let (epoch, expected_owner) = invalidator.pending_placement().unwrap();
        assert_eq!(expected_owner.as_slice(), &[owner]);
        assert!(!invalidator.commit_placement(epoch, &[EntityId::from(2)]));
        assert_eq!(invalidator.pending_placement(), Some((epoch, expected_owner)));
        invalidator.set_replay_only_dirty();
        assert!(!invalidator.commit_placement(epoch, &[owner]));
        assert!(invalidator.is_dirty());
        assert_eq!(invalidator.take_views(), FxHashSet::from_iter([owner]));
        invalidator.set_dirty(false);
        invalidator.invalidate_scroll_views([owner]);
        let (next, _) = invalidator.pending_placement().unwrap();
        assert_ne!(epoch, next);
        assert!(invalidator.commit_placement(next, &[owner]));
        assert!(!invalidator.is_dirty());
        assert!(invalidator.take_views().is_empty());
    }

    #[test]
    fn multi_placement_commit_requires_the_complete_owner_set() {
        let invalidator = WindowInvalidator::new();
        invalidator.set_dirty(false);
        let first = EntityId::from(1);
        let second = EntityId::from(2);
        invalidator.invalidate_scroll_views([first]);
        let (epoch, old) = invalidator.pending_placement().unwrap();
        invalidator.invalidate_scroll_views([second]);
        assert!(!invalidator.commit_placement(epoch, &old));
        let (current, owners) = invalidator.pending_placement().unwrap();
        assert_eq!(owners.as_slice(), &[first, second]);
        assert!(invalidator.commit_placement(current, &owners));
        assert!(!invalidator.is_dirty());
    }

    #[test]
    fn placement_requests_coalesce_without_advancing_content_generation() {
        let invalidator = WindowInvalidator::new();
        invalidator.set_dirty(false);
        let first = EntityId::from(1);
        let second = EntityId::from(2);
        let content = invalidator.scroll_content_revision().0;
        invalidator.invalidate_scroll_views([first]);
        invalidator.invalidate_scroll_views([first, second]);
        {
            let inner = invalidator.inner.borrow();
            assert_eq!(inner.scroll_placement_revision, 1);
            assert_eq!(inner.pending_placement_views.as_slice(), &[first, second]);
            assert!(inner.dirty_views.is_empty());
        }
        assert_eq!(invalidator.take_views(), FxHashSet::from_iter([first, second]));
        invalidator.set_dirty(false);
        assert_eq!(invalidator.scroll_content_revision(), (content, true));
        invalidator.invalidate_scroll_views([first]);
        assert_eq!(invalidator.scroll_content_revision(), (content, false));
        assert_eq!(invalidator.take_views(), FxHashSet::from_iter([first]));
        invalidator.set_dirty(false);
        assert_eq!(invalidator.scroll_content_revision(), (content, true));
        assert_eq!(invalidator.inner.borrow().scroll_placement_revision, 2);
    }

    #[test]
    fn mixed_scroll_content_and_overlay_requests_never_admit_placement_only() {
        let owner = EntityId::from(1);
        for scroll_first in [true, false] {
            for overlay in [true, false] {
                let invalidator = WindowInvalidator::new();
                invalidator.set_dirty(false);
                let content = invalidator.scroll_content_revision().0;
                let other = || {
                    if overlay { invalidator.set_replay_only_dirty(); }
                    else { invalidator.invalidate_owned_view(owner); }
                };
                if scroll_first { invalidator.invalidate_scroll_views([owner]); other(); }
                else { other(); invalidator.invalidate_scroll_views([owner]); }
                assert_eq!(invalidator.take_views(), FxHashSet::from_iter([owner]));
                invalidator.set_dirty(false);
                assert_eq!(invalidator.scroll_content_revision(), (content + u64::from(!overlay), false));
            }
        }
    }

    #[test]
    fn content_change_during_placement_preparation_revokes_admission() {
        let invalidator = WindowInvalidator::new();
        let owner = EntityId::from(1);
        invalidator.set_dirty(false);
        invalidator.invalidate_scroll_views([owner]);
        invalidator.take_views();
        invalidator.set_dirty(false);
        assert!(invalidator.scroll_content_revision().1);
        invalidator.set_phase(DrawPhase::Prepaint);
        assert!(!invalidator.invalidate_owned_view(owner));
        assert!(!invalidator.scroll_content_revision().1);
        invalidator.set_phase(DrawPhase::None);
        assert_eq!(invalidator.take_views(), FxHashSet::from_iter([owner]));
    }

    #[test]
    fn scoped_row_epoch_is_revoked_by_new_content_or_overlay_work() {
        let owner = EntityId::from(1);
        for overlay in [false, true] {
            let invalidator = WindowInvalidator::new();
            invalidator.set_dirty(false);
            invalidator.invalidate_owned_view(owner);
            invalidator.take_views();
            invalidator.set_dirty(false);
            assert_eq!(invalidator.row_replay_revision(),
                Some((invalidator.scroll_content_revision().0, false)));
            assert!(!invalidator.scroll_content_revision().1);
            invalidator.set_phase(DrawPhase::Prepaint);
            if overlay {
                invalidator.set_replay_only_dirty();
            } else {
                invalidator.invalidate_owned_view(owner);
            }
            assert!(invalidator.row_replay_revision().is_none());
        }
        let invalidator = WindowInvalidator::new();
        invalidator.set_dirty(true);
        invalidator.invalidate_owned_view(owner);
        invalidator.set_dirty(false);
        assert!(invalidator.row_replay_revision().is_none());
    }

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
    fn boundary_row_reads_preserve_other_owners_and_own_read_provenance() {
        let invalidator = WindowInvalidator::new();
        let owner = EntityId::from(1);
        let other = EntityId::from(2);
        let original = EntityId::from(3);
        let entering = EntityId::from(4);
        let global = TypeId::of::<u32>();
        let mut log = crate::AppDependencies::default();
        log.entity(Some(owner), original);
        log.entity(Some(other), original);
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(),
            &FxHashSet::default(), |_| true, |_| true);
        let reads = crate::RenderDependencies {
            entities: Rc::from([entering]),
            globals: Rc::from([(global, None)]),
            ..Default::default()
        };
        let prepared = invalidator.prepare_boundary_row_reads(owner, &reads).unwrap();
        assert!(!invalidator.inner.borrow().render_dependencies.contains_key(&entering));
        invalidator.commit_boundary_row_reads(prepared);
        let prepared = invalidator.prepare_boundary_row_reads(owner, &reads).unwrap();
        invalidator.commit_boundary_row_reads(prepared);
        {
            let inner = invalidator.inner.borrow();
            let entry = &inner.view_reads[&Some(owner)];
            assert_eq!(entry.own_reads.entities.as_ref(), &[original]);
            assert_eq!(inner.render_dependencies[&entering].as_ref().unwrap().as_slice(), &[owner]);
            assert!(inner.render_dependencies[&original].as_ref().unwrap().contains(&other));
            assert_eq!(inner.global_read_dependencies[&(global, None)].as_ref().unwrap().as_slice(), &[owner]);
        }
        let mut inner = invalidator.inner.borrow_mut();
        inner.mark_dependency_dirty(owner, Some(RowNotification::Entity(entering)));
        assert!(inner.pending_row_only.contains(&owner));
    }

    #[test]
    fn global_row_only_provenance_preserves_presence_semantics() {
        let invalidator = WindowInvalidator::new();
        let owner = EntityId::from(1);
        let global = TypeId::of::<u32>();
        let mut log = crate::AppDependencies::default();
        log.set_owner(Some(owner));
        log.note_global_read((global, Some(true)));
        let start = log.dependency_read_index();
        log.note_global_read((global, None));
        log.row_reads_since(start);
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);
        invalidator.set_dirty(false);
        invalidator.invalidate_global_dependency(global, true);
        assert!(invalidator.inner.borrow().pending_row_only.contains(&owner));
        invalidator.invalidate_global_dependency(global, false);
        assert!(!invalidator.inner.borrow().pending_row_only.contains(&owner));
    }

    #[test]
    fn row_only_provenance_requires_all_causes_and_excludes_self_notify() {
        let invalidator = WindowInvalidator::new();
        let owner = EntityId::from(1);
        let model = EntityId::from(2);
        let own = EntityId::from(3);
        let mut log = crate::AppDependencies::default();
        log.set_owner(Some(owner));
        log.note_access(own);
        let start = log.dependency_read_index();
        log.note_access(model);
        log.note_access(owner);
        log.row_reads_since(start);
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);
        let mut inner = invalidator.inner.borrow_mut();
        inner.mark_dependency_dirty(owner, Some(RowNotification::Entity(model)));
        assert!(inner.pending_row_only.contains(&owner));
        inner.mark_dependency_dirty(owner, Some(RowNotification::Entity(own)));
        assert!(!inner.pending_row_only.contains(&owner));
        inner.mark_dependency_dirty(owner, Some(RowNotification::Entity(model)));
        assert!(!inner.pending_row_only.contains(&owner));
        inner.dirty_views.clear();
        inner.mark_dependency_dirty(owner, Some(RowNotification::Entity(owner)));
        assert!(!inner.pending_row_only.contains(&owner));
        inner.dirty_views.clear();
        inner.mark_dependency_dirty(owner, Some(RowNotification::Entity(model)));
        assert!(inner.pending_row_only.contains(&owner));
        inner.pending_view_scoped = true;
        drop(inner);
        invalidator.take_views();
        invalidator.set_dirty(false);
        assert!(invalidator.row_only_dirty(owner));
        invalidator.set_dirty(true);
        assert!(!invalidator.row_only_dirty(owner));
    }

    #[test]
    fn notification_cursor_visits_only_new_events_and_rejects_overflow() {
        let model = EntityId::from(1);
        let global = TypeId::of::<u32>();
        let mut notifications = RowNotifications::default();
        notifications.sequence = u64::MAX - 1;
        let start = notifications.sequence;
        notifications.push(RowNotification::Entity(model));
        notifications.push(RowNotification::Global(global, true));
        let mut visited = Vec::new();
        let cursor = notifications.visit_since(start, |seq, _| visited.push(seq)).unwrap();
        assert_eq!(visited, [u64::MAX, 0]);
        assert_eq!(cursor, 0);
        notifications.push(RowNotification::Entity(model));
        visited.clear();
        assert_eq!(notifications.visit_since(cursor, |seq, _| visited.push(seq)), Some(1));
        assert_eq!(visited, [1]);
        for _ in 0..RowNotifications::CAPACITY {
            notifications.push(RowNotification::Entity(model));
        }
        assert!(notifications.visit_since(cursor, |_, _| unreachable!()).is_none());
    }

    #[test]
    fn row_notifications_match_entity_global_and_existence_reads() {
        let model = EntityId::from(1);
        let unrelated = EntityId::from(2);
        let global = TypeId::of::<u32>();
        let reads = crate::RenderDependencies {
            entities: Rc::from([model]),
            globals: Rc::from([(global, Some(true))]),
            ..Default::default()
        };
        let mut notifications = RowNotifications::default();
        let start = notifications.sequence;
        notifications.push(RowNotification::Entity(unrelated));
        notifications.push(RowNotification::Global(global, true));
        assert!(notifications.unchanged(start, &reads));
        notifications.push(RowNotification::Global(global, false));
        assert!(!notifications.unchanged(start, &reads));
        let start = notifications.sequence;
        notifications.push(RowNotification::Entity(model));
        assert!(!notifications.unchanged(start, &reads));
        let start = notifications.sequence;
        for _ in 0..=RowNotifications::CAPACITY {
            notifications.push(RowNotification::Entity(unrelated));
        }
        assert!(!notifications.unchanged(start, &reads));
        assert!(notifications.unchanged(start, &Default::default()));
        let value_reads = crate::RenderDependencies {
            globals: Rc::from([(global, None)]),
            ..Default::default()
        };
        let start = notifications.sequence;
        notifications.push(RowNotification::Global(global, true));
        assert!(!notifications.unchanged(start, &value_reads));
    }

    #[test]
    fn scoped_route_delta_preserves_unrelated_and_common_edges() {
        let first = EntityId::from(1);
        let second = EntityId::from(2);
        let mut index = FxHashMap::default();
        assert!(!update_dependency_routes(&mut index, &[], Some(first), &[1, 2, 3], Some(first)));
        assert!(!update_dependency_routes(&mut index, &[], Some(second), &[2, 4], Some(second)));
        assert!(!update_dependency_routes(&mut index, &[1, 2, 3], Some(first), &[2, 3, 5], Some(first)));
        assert!(!index.contains_key(&1));
        assert_eq!(index[&2].as_ref().unwrap().as_slice(), &[first, second]);
        assert_eq!(index[&4].as_ref().unwrap().as_slice(), &[second]);
        assert_eq!(index[&5].as_ref().unwrap().as_slice(), &[first]);
        assert!(!update_dependency_routes(&mut index, &[], None, &[2], None));
        assert_eq!(index[&2], None);
        assert!(update_dependency_routes(&mut index, &[2], None, &[], None));
    }

    #[test]
    fn sparse_owner_poll_preserves_pending_and_rebuilds_after_snapshot_changes() {
        let invalidator = WindowInvalidator::new();
        let mut states: Vec<_> = (0..32).map(|_| Some(crate::StateVersion::default())).collect();
        let owners: Vec<_> = (1..=32_u64).map(EntityId::from).collect();
        let mut log = crate::AppDependencies::default();
        let scope = log.begin_state_reads();
        for (owner, state) in owners.iter().zip(&states) {
            log.set_owner(Some(*owner));
            state.as_ref().unwrap().read();
        }
        log.set_owner(None);
        states[7].as_ref().unwrap().read();
        drop(scope);
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);
        let mut changed = SmallVec::new();
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert!(invalidator.inner.borrow().owner_state_routing.cursor.is_some());
        states[7].as_ref().unwrap().bump();
        assert!(invalidator.changed_state_owners(&mut changed));
        assert_eq!(changed.as_slice(), &[owners[7]]);
        changed.clear();
        assert!(invalidator.changed_state_owners(&mut changed));
        assert_eq!(changed.as_slice(), &[owners[7]]);
        drop(states[7].take());
        changed.clear();
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert!(changed.is_empty());
        // Overflow still sees every affected owner.
        for _ in 0..300 {
            states[9].as_ref().unwrap().bump();
        }
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert_eq!(changed.as_slice(), &[owners[9]]);
        // Publish a replacement snapshot and retire all other owners.
        let scope = log.begin_state_reads();
        log.set_owner(Some(owners[9]));
        states[9].as_ref().unwrap().read();
        drop(scope);
        invalidator.commit_frame_dependencies(&mut log, &mut FxHashSet::default(), &FxHashSet::default(), |_| true, |_| true);
        changed.clear();
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert!(changed.is_empty());
        states[9].as_ref().unwrap().bump();
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert_eq!(changed.as_slice(), &[owners[9]]);
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
    fn owner_state_validation_rechecks_replaced_snapshots_and_preserves_routes() {
        let invalidator = WindowInvalidator::new();
        let content = crate::StateVersion::default();
        let placement = crate::StateVersion::placement();
        let owner = EntityId::from(1);
        let mut log = crate::AppDependencies::default();
        let mut publish = |state: &crate::StateVersion| {
            let recording = log.begin_state_reads();
            log.set_owner(Some(owner));
            state.read();
            drop(recording);
            invalidator.commit_frame_dependencies(
                &mut log, &mut FxHashSet::default(), &FxHashSet::default(),
                |_| true, |_| true,
            );
        };
        publish(&content);
        let mut changed = SmallVec::new();
        assert!(!invalidator.changed_state_owners(&mut changed));
        placement.bump();
        assert!(!invalidator.changed_state_owners(&mut changed));
        assert!(changed.is_empty());
        // Replace a previously certified snapshot without bumping content.
        publish(&placement);
        placement.bump();
        assert!(!invalidator.changed_state_owners(&mut changed));
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
            let epoch = invalidator.row_notification_epoch();
            let reads = crate::RenderDependencies {
                entities: Rc::from([model]),
                ..Default::default()
            };
            assert!(invalidator.row_dependencies_unchanged(epoch, &reads));
            assert!(invalidator.invalidate_dependency(model, cx));
            assert!(!invalidator.row_dependencies_unchanged(epoch, &reads));
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
