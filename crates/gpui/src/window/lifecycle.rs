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
    /// listeners are invoked from the root of the tree downward toward the focused element. This phase
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
        if self
            .invalidator
            .invalidate_retained_path(view_id, global_id, descendants_dirty)
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

struct WindowInvalidatorInner {
    pub dirty: bool,
    pub draw_phase: DrawPhase,
    pub dirty_views: FxHashSet<EntityId>,
    pub dirty_frame_diagnostics: Rc<RefCell<DirtyFrameDiagnostics>>,
    /// True only while every queued invalidation supplies a stable retained target, or the frame
    /// is a replay-only window overlay. This mode is shared by interactions, animations, focus,
    /// and any future element-local invalidation source.
    pub pending_targeted_replay: bool,
    /// Stable dirty retained paths and whether each target invalidates all descendants.
    pub pending_targeted_elements: FxHashMap<GlobalElementId, bool>,
    /// Snapshot consumed by the frame currently being generated.
    pub active_targeted_replay: bool,
    pub active_targeted_elements: FxHashMap<GlobalElementId, bool>,
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
                active_targeted_replay: false,
                active_targeted_elements: FxHashMap::default(),
            })),
        }
    }

    pub(in crate::window) fn set_dirty_frame_diagnostics(
        &self,
        dirty_frame_diagnostics: Rc<RefCell<DirtyFrameDiagnostics>>,
    ) {
        self.inner.borrow_mut().dirty_frame_diagnostics = dirty_frame_diagnostics;
    }

    pub fn invalidate_view(&self, entity: EntityId, cx: &mut App) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.pending_targeted_replay = false;
        inner.pending_targeted_elements.clear();
        inner
            .dirty_frame_diagnostics
            .borrow_mut()
            .record_notify_invalidation(entity);
        inner.dirty_views.insert(entity);
        if inner.draw_phase == DrawPhase::None {
            inner.dirty = true;
            cx.push_effect(Effect::Notify { emitter: entity });
            true
        } else {
            false
        }
    }

    /// Marks a view dirty because one stable retained element path changed.
    ///
    /// No application-level `Notify` effect is emitted: the caller already owns the reason for
    /// invalidation (interaction state, animation sampling, focus, window overlay, and so on). The
    /// retained path is carried into the next frame so unrelated siblings remain replayable.
    pub(in crate::window) fn invalidate_retained_path(
        &self,
        entity: EntityId,
        global_id: Option<&GlobalElementId>,
        descendants_dirty: bool,
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
            inner.pending_targeted_replay = global_id.is_some();
            inner.pending_targeted_elements.clear();
        } else if inner.pending_targeted_replay && global_id.is_none() {
            inner.pending_targeted_replay = false;
            inner.pending_targeted_elements.clear();
        }

        if inner.pending_targeted_replay
            && let Some(global_id) = global_id
        {
            inner
                .pending_targeted_elements
                .entry(global_id.clone())
                .and_modify(|existing_scope| *existing_scope |= descendants_dirty)
                .or_insert(descendants_dirty);
        }
        inner.dirty = true;
        true
    }

    pub fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    /// Marks a generic frame dirty. This is a conservative invalidation and disables targeted
    /// retained replay because the caller has not supplied a trustworthy retained path.
    pub fn set_dirty(&self, is_dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        if is_dirty {
            inner.pending_targeted_replay = false;
            inner.pending_targeted_elements.clear();
            inner.active_targeted_replay = false;
            inner.active_targeted_elements.clear();
        } else {
            inner.active_targeted_replay = inner.pending_targeted_replay;
            inner.active_targeted_elements = mem::take(&mut inner.pending_targeted_elements);
            inner.pending_targeted_replay = false;
        }
        inner.dirty = is_dirty;
    }

    /// Schedules a frame whose only window-level change is outside the retained application tree.
    ///
    /// An empty target set means every stable retained element is eligible for replay. If a
    /// targeted invalidation is queued before this frame starts, its path is merged into the same
    /// replay frame. A pre-existing generic dirty request is never upgraded to targeted replay.
    pub(in crate::window) fn set_replay_only_dirty(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.draw_phase != DrawPhase::None {
            inner.dirty = true;
            return;
        }

        if !inner.dirty {
            inner.pending_targeted_replay = true;
            inner.pending_targeted_elements.clear();
        }
        inner.dirty = true;
    }

    pub(in crate::window) fn active_targeted_replay(&self) -> bool {
        self.inner.borrow().active_targeted_replay
    }

    /// Returns whether this stable retained path must execute normally in the active targeted
    /// frame. Structural ancestors execute so traversal can reach a dirty target. Descendants run
    /// only when that target explicitly carries subtree-context damage.
    pub(in crate::window) fn retained_path_is_dirty(&self, global_id: &GlobalElementId) -> bool {
        let inner = self.inner.borrow();
        if !inner.active_targeted_replay {
            return true;
        }
        inner.active_targeted_elements.iter().any(
            |(dirty, descendants_dirty)| {
                retained_path_requires_repaint(global_id, dirty, *descendants_dirty)
            },
        )
    }

    /// Returns true when `global_id` executes only because it is a structural ancestor of one or
    /// more dirty retained targets, while its own pixels and subtree context remain unchanged.
    ///
    /// The ancestor still routes traversal to the changed child, but its own stable
    /// background/shadow/border primitives can be replayed. A direct hit on this path, or an
    /// ancestor invalidation whose scope damages descendants, disables self-scene reuse.
    pub(in crate::window) fn retained_path_is_descendant_only(
        &self,
        global_id: &GlobalElementId,
    ) -> bool {
        let inner = self.inner.borrow();
        if !inner.active_targeted_replay {
            return false;
        }

        let mut has_dirty_descendant = false;
        for (dirty, descendants_dirty) in &inner.active_targeted_elements {
            if global_id == dirty
                || (*descendants_dirty && global_element_path_is_prefix(dirty, global_id))
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
    descendants_dirty: bool,
) -> bool {
    global_element_path_is_prefix(candidate, dirty)
        || descendants_dirty && global_element_path_is_prefix(dirty, candidate)
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

        assert!(retained_path_requires_repaint(&ancestor, &dirty, false));
        assert!(retained_path_requires_repaint(&dirty, &dirty, false));
        assert!(!retained_path_requires_repaint(&descendant, &dirty, false));
        assert!(!retained_path_requires_repaint(&sibling, &dirty, false));
    }

    #[test]
    fn subtree_target_invalidates_descendants_but_not_siblings() {
        let ancestor = path(&[0]);
        let dirty = path(&[0, 1]);
        let descendant = path(&[0, 1, 2]);
        let sibling = path(&[0, 3]);

        assert!(retained_path_requires_repaint(&ancestor, &dirty, true));
        assert!(retained_path_requires_repaint(&dirty, &dirty, true));
        assert!(retained_path_requires_repaint(&descendant, &dirty, true));
        assert!(!retained_path_requires_repaint(&sibling, &dirty, true));
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
}

pub(crate) type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;

pub(crate) type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;
