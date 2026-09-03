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
    /// is used for special purposes such as clearing the "pressed" state for click events. If
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
            .invalidate_interactive_view(view_id, global_id, descendants_dirty)
        {
            self.schedule_dirty_frame();
        }
    }

    /// Requests another draw without forcing every [`AnyView`] cache entry to miss.
    ///
    /// This is intended for window-owned overlays such as tooltips and drag previews whose state
    /// lives outside the normal entity render tree.
    pub(crate) fn redraw_without_view_cache_refresh(&mut self) {
        self.idle_render_frames = 0;
        self.render_trim_policy = RetainedResourceTrimPolicy::None;
        self.invalidator.set_dirty(true);
        self.schedule_dirty_frame();
    }
}

struct WindowInvalidatorInner {
    pub dirty: bool,
    pub draw_phase: DrawPhase,
    pub dirty_views: FxHashSet<EntityId>,
    pub dirty_frame_diagnostics: Rc<RefCell<DirtyFrameDiagnostics>>,
    /// True only while all invalidations queued for the next frame are element-local interactions.
    pub pending_interaction_only: bool,
    /// Stable dirty paths and whether each path invalidates all of its descendants.
    pub pending_interactive_elements: FxHashMap<GlobalElementId, bool>,
    /// Snapshot consumed by the frame currently being generated.
    pub active_interaction_only: bool,
    pub active_interactive_elements: FxHashMap<GlobalElementId, bool>,
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
                pending_interaction_only: false,
                pending_interactive_elements: FxHashMap::default(),
                active_interaction_only: false,
                active_interactive_elements: FxHashMap::default(),
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
        inner.pending_interaction_only = false;
        inner.pending_interactive_elements.clear();
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

    /// Marks a view dirty because one stable element path changed its interaction state.
    /// No application-level `Notify` effect is emitted because hover/pressed state is owned by
    /// GPUI's element state rather than by the entity itself.
    pub(in crate::window) fn invalidate_interactive_view(
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
            inner.pending_interaction_only = global_id.is_some();
            inner.pending_interactive_elements.clear();
        } else if inner.pending_interaction_only && global_id.is_none() {
            inner.pending_interaction_only = false;
            inner.pending_interactive_elements.clear();
        }

        if inner.pending_interaction_only
            && let Some(global_id) = global_id
        {
            inner
                .pending_interactive_elements
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

    pub fn set_dirty(&self, is_dirty: bool) {
        let mut inner = self.inner.borrow_mut();
        if is_dirty {
            // A generic dirty request cannot safely use the interaction-only replay fast path.
            inner.pending_interaction_only = false;
            inner.pending_interactive_elements.clear();
            inner.active_interaction_only = false;
            inner.active_interactive_elements.clear();
        } else {
            inner.active_interaction_only = inner.pending_interaction_only;
            inner.active_interactive_elements =
                mem::take(&mut inner.pending_interactive_elements);
            inner.pending_interaction_only = false;
        }
        inner.dirty = is_dirty;
    }

    pub(in crate::window) fn active_interaction_only(&self) -> bool {
        self.inner.borrow().active_interaction_only
    }

    /// Returns whether this stable retained path must execute normally in the active interaction
    /// frame. Structural ancestors always execute so the traversal can reach a changed element.
    /// Descendants execute only when the invalidation explicitly carries subtree context damage.
    pub(in crate::window) fn interactive_path_is_dirty(&self, global_id: &GlobalElementId) -> bool {
        let inner = self.inner.borrow();
        if !inner.active_interaction_only {
            return true;
        }
        inner.active_interactive_elements.iter().any(
            |(dirty, descendants_dirty)| {
                interaction_path_requires_repaint(global_id, dirty, *descendants_dirty)
            },
        )
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

fn interaction_path_requires_repaint(
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

#[cfg(test)]
mod interaction_dirty_scope_tests {
    use super::*;

    fn path(parts: &[u32]) -> GlobalElementId {
        GlobalElementId(parts.iter().copied().map(ElementId::InstanceSlot).collect())
    }

    #[test]
    fn element_only_interaction_keeps_descendants_replayable() {
        let ancestor = path(&[0]);
        let dirty = path(&[0, 1]);
        let descendant = path(&[0, 1, 2]);
        let sibling = path(&[0, 3]);

        assert!(interaction_path_requires_repaint(&ancestor, &dirty, false));
        assert!(interaction_path_requires_repaint(&dirty, &dirty, false));
        assert!(!interaction_path_requires_repaint(&descendant, &dirty, false));
        assert!(!interaction_path_requires_repaint(&sibling, &dirty, false));
    }

    #[test]
    fn subtree_interaction_invalidates_descendants_but_not_siblings() {
        let ancestor = path(&[0]);
        let dirty = path(&[0, 1]);
        let descendant = path(&[0, 1, 2]);
        let sibling = path(&[0, 3]);

        assert!(interaction_path_requires_repaint(&ancestor, &dirty, true));
        assert!(interaction_path_requires_repaint(&dirty, &dirty, true));
        assert!(interaction_path_requires_repaint(&descendant, &dirty, true));
        assert!(!interaction_path_requires_repaint(&sibling, &dirty, true));
    }
}

pub(crate) type AnyObserver = Box<dyn FnMut(&mut Window, &mut App) -> bool + 'static>;

pub(crate) type FrameCallback = Box<dyn FnOnce(&mut Window, &mut App)>;
