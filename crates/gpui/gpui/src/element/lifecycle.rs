use crate::{
    App, AvailableSpace, Bounds, DispatchNodeId, ElementId, InspectorElementId, LayoutId,
    Pixels, Size, Window,
};
use derive_more::Deref;
use smallvec::SmallVec;
use std::{
    any::Any,
    fmt::{self, Display},
    hash::{Hash, Hasher},
    mem,
    sync::Arc,
};

use super::Element;

/// A globally unique identifier for an element, used to track state across frames.
#[derive(Clone, Deref, Debug)]
pub struct GlobalElementId(
    #[deref] pub(crate) Arc<[ElementId]>,
    u64,
);

// Incremental path hashing and stack from longbridge/gpui-fast (1100a214532bdeac0282acd1c5c6d2062d8ab1a7).
// Copyright 2022 - 2025 Zed Industries, Inc.
// Copyright 2026 Longbridge.
// SPDX-License-Identifier: Apache-2.0
pub(crate) const EMPTY_PATH_HASH: u64 = 0;

/// The hash of the path `prefix` + `id`, given the hash of `prefix`.
pub(crate) fn extend_path_hash(prefix: u64, id: &ElementId) -> u64 {
    let mut hasher = collections::FxHasher::with_seed(prefix as usize);
    id.hash(&mut hasher);
    hasher.finish()
}

#[derive(Clone, Default)]
pub(crate) struct ElementIdStack {
    // A suspended traversal already owns an immutable, hashed prefix.
    prefix: Option<GlobalElementId>,
    ids: SmallVec<[ElementId; 32]>,
    hashes: SmallVec<[u64; 32]>,
}

impl ElementIdStack {
    pub(crate) fn from_prefix(prefix: GlobalElementId) -> Self {
        Self { prefix: Some(prefix), ..Self::default() }
    }

    pub(crate) fn restore_prefix(&mut self, prefix: Option<GlobalElementId>) {
        debug_assert!(self.is_empty());
        self.prefix = prefix;
    }

    pub(crate) fn take_prefix(&mut self) -> Option<GlobalElementId> {
        debug_assert!(self.ids.is_empty() && self.hashes.is_empty());
        self.ids.clear();
        self.hashes.clear();
        self.prefix.take()
    }

    fn prefix_slice(&self) -> &[ElementId] {
        self.prefix.as_ref().map_or(&[], |prefix| prefix.0.as_ref())
    }

    pub(crate) fn len(&self) -> usize {
        self.prefix_slice().len() + self.ids.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.ids.is_empty() && self.prefix_slice().is_empty()
    }

    fn iter(&self) -> impl Iterator<Item = &ElementId> {
        self.prefix_slice().iter().chain(self.ids.iter())
    }

    pub(crate) fn matches(&self, path: &[ElementId]) -> bool {
        let prefix = self.prefix_slice();
        path.len() == prefix.len() + self.ids.len()
            && &path[..prefix.len()] == prefix
            && &path[prefix.len()..] == self.ids.as_slice()
    }

    pub(crate) fn push(&mut self, id: ElementId) {
        self.hashes.push(extend_path_hash(self.hash(), &id));
        self.ids.push(id);
    }

    pub(crate) fn pop(&mut self) -> Option<ElementId> {
        self.hashes.pop();
        self.ids.pop()
    }

    pub(crate) fn clear(&mut self) {
        self.ids.clear();
        self.hashes.clear();
        self.prefix = None;
    }

    /// The hash of the whole path on the stack.
    pub(crate) fn hash(&self) -> u64 {
        self.hashes
            .last()
            .copied()
            .unwrap_or_else(|| self.prefix.as_ref().map_or(EMPTY_PATH_HASH, |prefix| prefix.1))
    }

    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[ElementId] {
        assert!(self.prefix.is_none());
        &self.ids
    }

    pub(crate) fn global_id(&self) -> GlobalElementId {
        if let Some(prefix) = &self.prefix {
            if self.ids.is_empty() {
                return prefix.clone();
            }
            let path: Arc<[ElementId]> = self.iter().cloned().collect();
            return GlobalElementId(path, self.hash());
        }
        GlobalElementId::with_hash(&self.ids, self.hash())
    }
}

impl GlobalElementId {
    /// Builds an identifier for an arbitrary element path.
    #[inline]
    pub(crate) fn from_path(path: &[ElementId]) -> Self {
        Self::with_hash(path, path.iter().fold(EMPTY_PATH_HASH, extend_path_hash))
    }

    /// Uses the prefix hash maintained by the element ID stack.
    #[inline]
    fn with_hash(path: &[ElementId], hash: u64) -> Self {
        debug_assert_eq!(hash, path.iter().fold(EMPTY_PATH_HASH, extend_path_hash));
        Self(Arc::from(path), hash)
    }
}

// GlobalIdCache from longbridge/gpui-fast (1100a214532bdeac0282acd1c5c6d2062d8ab1a7), adapted to the
// tuple representation and BMCBL's speculative frame rollback.
// Copyright 2022 - 2025 Zed Industries, Inc.
// Copyright 2026 Longbridge.
// SPDX-License-Identifier: Apache-2.0
/// One persistent identity table. The path hash is already the map key;
/// only the immutable path allocation and its access generation are stored.
struct GlobalIdEntry {
    path: Arc<[ElementId]>,
    stamp: u64,
}

/// The high bit marks an identity first inserted by the candidate frame.
/// It is meaningful only while the low bits equal the current epoch.
const CANDIDATE_ID: u64 = 1 << 63;

pub(crate) struct GlobalIdCache {
    entries: collections::FxHashMap<u64, GlobalIdEntry>,
    epoch: u64,
    used: usize,
    last_full_count: usize,
}

impl Default for GlobalIdCache {
    fn default() -> Self {
        Self { entries: collections::FxHashMap::default(), epoch: 1, used: 0, last_full_count: 0 }
    }
}

impl GlobalIdCache {
    pub(crate) fn get(&mut self, path: &ElementIdStack) -> GlobalElementId {
        let hash = path.hash();
        if let Some(entry) = self.entries.get_mut(&hash) {
            if !path.matches(&entry.path) {
                // A hash is not identity. A rare collision uses an
                // uncached path without replacing the committed owner.
                return path.global_id();
            }
            if entry.stamp & !CANDIDATE_ID != self.epoch {
                entry.stamp = self.epoch;
                self.used += 1;
            }
            return GlobalElementId(Arc::clone(&entry.path), hash);
        }
        let id = path.global_id();
        self.entries.insert(hash, GlobalIdEntry {
            path: Arc::clone(&id.0),
            stamp: self.epoch | CANDIDATE_ID,
        });
        self.used += 1;
        id
    }

    pub(crate) fn finish_frame(&mut self) {
        debug_assert!(self.used <= self.entries.len());
        if self.used != self.entries.len() {
            let epoch = self.epoch;
            self.entries.retain(|_, entry| entry.stamp & !CANDIDATE_ID == epoch);
            let target = self.entries.len().max(32);
            if self.entries.capacity() > target.saturating_mul(4) {
                self.entries.shrink_to(target);
            }
        }
        self.last_full_count = self.entries.len();
        self.advance_epoch();
    }

    /// A detached boundary does not visit unrelated application paths.
    /// Preserve their immutable Arc identities until a normal traversal
    /// can sweep them, with a bounded working set between full frames.
    #[allow(dead_code)]
    pub(crate) fn can_finish_partial_frame(&self) -> bool {
        self.entries.len() <= self.last_full_count.max(32).saturating_mul(2)
    }

    #[allow(dead_code)]
    pub(crate) fn finish_partial_frame(&mut self) {
        debug_assert!(self.can_finish_partial_frame());
        self.advance_epoch();
    }

    fn advance_epoch(&mut self) {
        self.used = 0;
        self.epoch = self.epoch.checked_add(1)
            .filter(|epoch| *epoch < CANDIDATE_ID).expect("global id epoch exhausted");
    }

    /// Roll back candidate-only paths without dropping committed paths
    /// that were not reached by the abandoned traversal.
    pub(crate) fn abort_frame(&mut self) {
        if self.used == 0 { return; }
        let epoch = self.epoch;
        self.entries.retain(|_, entry| {
            if entry.stamp == (epoch | CANDIDATE_ID) { return false; }
            if entry.stamp == epoch { entry.stamp = epoch - 1; }
            true
        });
        self.used = 0;
    }
}

impl Default for GlobalElementId {
    fn default() -> Self {
        Self::from_path(&[])
    }
}

impl PartialEq for GlobalElementId {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.1 == other.1 && (Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0)
    }
}

impl Eq for GlobalElementId {}

impl Hash for GlobalElementId {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Retained identity participates in several frame and layout maps.
        // Hash the path once per construction, not once per lookup. Equality
        // still checks every segment when allocations differ, including on
        // a hash collision.
        state.write_u64(self.1);
    }
}

impl Display for GlobalElementId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, element_id) in self.0.iter().enumerate() {
            if i > 0 {
                write!(f, ".")?;
            }
            write!(f, "{}", element_id)?;
        }
        Ok(())
    }
}

pub(super) trait ElementObject {
    fn inner_element(&mut self) -> &mut dyn Any;

    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId;

    fn prepaint(&mut self, window: &mut Window, cx: &mut App);

    fn paint(&mut self, window: &mut Window, cx: &mut App);

    fn layout_as_root(
        &mut self,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels>;
}

/// A wrapper around an implementer of [`Element`] that allows it to be drawn in a window.
pub struct Drawable<E: Element> {
    /// The drawn element.
    pub element: E,
    phase: ElementDrawPhase<E::RequestLayoutState, E::PrepaintState>,
    #[cfg(any(test, feature = "test-support"))]
    painted_state: Option<(E::RequestLayoutState, E::PrepaintState)>,
    #[cfg(any(test, feature = "test-support"))]
    capture_paint_state: bool,
}

#[derive(Default)]
enum ElementDrawPhase<RequestLayoutState, PrepaintState> {
    #[default]
    Start,
    RequestLayout {
        layout_id: LayoutId,
        global_id: Option<GlobalElementId>,
        fast_layout_key: u64,
        inspector_id: Option<InspectorElementId>,
        request_layout: RequestLayoutState,
    },
    LayoutComputed {
        layout_id: LayoutId,
        global_id: Option<GlobalElementId>,
        fast_layout_key: u64,
        inspector_id: Option<InspectorElementId>,
        available_space: Size<AvailableSpace>,
        request_layout: RequestLayoutState,
    },
    Prepaint {
        node_id: DispatchNodeId,
        global_id: Option<GlobalElementId>,
        fast_layout_key: u64,
        inspector_id: Option<InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: RequestLayoutState,
        prepaint: PrepaintState,
    },
    Painted,
}

#[inline(never)]
fn prepare_element_id(element_id: ElementId, window: &mut Window) -> GlobalElementId {
    window.element_id_stack.push(element_id);
    window.global_ids.borrow_mut().get(&window.element_id_stack)
}

#[cfg(any(feature = "inspector", debug_assertions))]
#[inline(never)]
fn prepare_inspector_id(
    source: &'static core::panic::Location<'static>,
    window: &mut Window,
) -> InspectorElementId {
    let path = crate::InspectorElementPath {
        global_id: window.global_ids.borrow_mut().get(&window.element_id_stack),
        source_location: source,
    };
    window.build_inspector_element_id(path)
}

/// A wrapper around an implementer of [`Element`] that allows it to be drawn in a window.
impl<E: Element> Drawable<E> {
    pub(crate) fn new(element: E) -> Self {
        Drawable {
            element,
            phase: ElementDrawPhase::Start,
            #[cfg(any(test, feature = "test-support"))]
            painted_state: None,
            #[cfg(any(test, feature = "test-support"))]
            capture_paint_state: false,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn paint_for_test(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> (E::RequestLayoutState, E::PrepaintState) {
        self.capture_paint_state = true;
        self.paint(window, cx);
        self.painted_state
            .take()
            .expect("test element must paint rather than replay")
    }

    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        match mem::take(&mut self.phase) {
            ElementDrawPhase::Start => {
                let element_id = self.element.id();
                let fast_layout_key =
                    crate::fast::layout_key::push_layout_key(window, element_id.as_ref());
                let global_id = element_id.map(|id| prepare_element_id(id, window));

                #[cfg(any(feature = "inspector", debug_assertions))]
                let inspector_id = if window.inspector_enabled() {
                    self.element.source_location().map(|source| prepare_inspector_id(source, window))
                } else {
                    None
                };
                #[cfg(not(any(feature = "inspector", debug_assertions)))]
                let inspector_id = None;

                let (layout_id, request_layout) = self.element.request_layout(
                    global_id.as_ref(),
                    inspector_id.as_ref(),
                    window,
                    cx,
                );
                if global_id.is_some() {
                    window.element_id_stack.pop();
                }
                crate::fast::layout_key::pop_layout_key(window);

                self.phase = ElementDrawPhase::RequestLayout {
                    layout_id,
                    global_id,
                    fast_layout_key,
                    inspector_id,
                    request_layout,
                };
                layout_id
            }
            _ => panic!("must call request_layout only once"),
        }
    }

    pub(crate) fn prepaint(&mut self, window: &mut Window, cx: &mut App) {
        match mem::take(&mut self.phase) {
            ElementDrawPhase::RequestLayout {
                layout_id,
                global_id,
                fast_layout_key,
                inspector_id,
                mut request_layout,
            }
            | ElementDrawPhase::LayoutComputed {
                layout_id,
                global_id,
                fast_layout_key,
                inspector_id,
                mut request_layout,
                ..
            } => {
                let bounds = window.layout_bounds(layout_id);
                if let Some(element_id) = self.element.id() {
                    window.element_id_stack.push(element_id);
                    debug_assert!(window.element_id_stack.matches(
                        global_id.as_ref().expect("element id must have a corresponding global id").0.as_ref()
                    ));
                }
                let node_id = window.next_frame.dispatch_tree.push_node();
                let scope = crate::fast::layout_key::enter_prepaint_scope(window, fast_layout_key);
                let prepaint = {
                    let _work = crate::window::element_work::begin_named(
                        std::any::type_name::<E>(),
                        crate::window::element_work::ElementWorkPhase::Prepaint,
                    );
                    self.element.prepaint(
                        global_id.as_ref(), inspector_id.as_ref(), bounds,
                        &mut request_layout, window, cx,
                    )
                };
                crate::fast::layout_key::exit_prepaint_scope(window, scope);
                window.next_frame.dispatch_tree.pop_node();
                if global_id.is_some() {
                    window.element_id_stack.pop();
                }

                self.phase = ElementDrawPhase::Prepaint {
                    node_id,
                    global_id,
                    fast_layout_key,
                    inspector_id,
                    bounds,
                    request_layout,
                    prepaint,
                };
            }
            _ => panic!("must call request_layout before prepaint"),
        }
    }

    pub(crate) fn paint(&mut self, window: &mut Window, cx: &mut App) {
        match mem::take(&mut self.phase) {
            ElementDrawPhase::Prepaint {
                node_id,
                global_id,
                fast_layout_key,
                inspector_id,
                bounds,
                mut request_layout,
                mut prepaint,
            } => {
                if let Some(element_id) = self.element.id() {
                    window.element_id_stack.push(element_id);
                    debug_assert!(window.element_id_stack.matches(
                        global_id.as_ref().expect("element id must have a corresponding global id").0.as_ref()
                    ));
                }
                window.record_debug_element_paint(bounds, cx);
                window.next_frame.dispatch_tree.set_active_node(node_id);
                let previous = window.paint_layout_key.replace(fast_layout_key);
                {
                    let _work = crate::window::element_work::begin_named(
                        std::any::type_name::<E>(),
                        crate::window::element_work::ElementWorkPhase::Paint,
                    );
                    self.element.paint(
                        global_id.as_ref(), inspector_id.as_ref(), bounds,
                        &mut request_layout, &mut prepaint, window, cx,
                    );
                }
                window.paint_layout_key = previous;
                if global_id.is_some() {
                    window.element_id_stack.pop();
                }

                self.phase = ElementDrawPhase::Painted;
                #[cfg(any(test, feature = "test-support"))]
                {
                    if self.capture_paint_state {
                        self.painted_state = Some((request_layout, prepaint));
                    }
                }
            }
            _ => panic!("must call prepaint before paint"),
        }
    }

    pub(crate) fn layout_as_root(
        &mut self,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        if matches!(&self.phase, ElementDrawPhase::Start) {
            self.request_layout(window, cx);
        }

        let (
            layout_id,
            global_id,
            fast_layout_key,
            inspector_id,
            previous_available_space,
            request_layout,
        ) = match mem::take(&mut self.phase) {
            ElementDrawPhase::RequestLayout {
                layout_id,
                global_id,
                fast_layout_key,
                inspector_id,
                request_layout,
            } => (
                layout_id,
                global_id,
                fast_layout_key,
                inspector_id,
                None,
                request_layout,
            ),
            ElementDrawPhase::LayoutComputed {
                layout_id,
                global_id,
                fast_layout_key,
                inspector_id,
                available_space: previous_available_space,
                request_layout,
            } => (
                layout_id,
                global_id,
                fast_layout_key,
                inspector_id,
                Some(previous_available_space),
                request_layout,
            ),
            _ => panic!("cannot measure after painting"),
        };

        if previous_available_space != Some(available_space) {
            window.compute_layout(layout_id, available_space, cx);
        }
        self.phase = ElementDrawPhase::LayoutComputed {
            layout_id,
            global_id,
            fast_layout_key,
            inspector_id,
            available_space,
            request_layout,
        };

        window.layout_bounds(layout_id).size
    }
}

impl<E> ElementObject for Drawable<E>
where
    E: Element,
    E::RequestLayoutState: 'static,
{
    fn inner_element(&mut self) -> &mut dyn Any {
        &mut self.element
    }

    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        Drawable::request_layout(self, window, cx)
    }

    fn prepaint(&mut self, window: &mut Window, cx: &mut App) {
        Drawable::prepaint(self, window, cx)
    }

    fn paint(&mut self, window: &mut Window, cx: &mut App) {
        Drawable::paint(self, window, cx)
    }

    fn layout_as_root(
        &mut self,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Size<Pixels> {
        Drawable::layout_as_root(self, available_space, window, cx)
    }
}

#[cfg(test)]
mod global_element_id_hash_tests {
    use super::*;

    #[test]
    fn incremental_stack_matches_arbitrary_path_and_restores_prefixes() {
        let mut stack = ElementIdStack::default();
        assert_eq!(stack.global_id(), GlobalElementId::default());
        stack.push(ElementId::from("root"));
        let root = stack.global_id();
        stack.push(ElementId::from(42u64));
        let child = stack.global_id();
        assert_eq!(child, GlobalElementId::from_path(stack.as_slice()));
        assert_eq!(child, stack.global_id());
        let mut deferred = stack.clone();
        deferred.pop();
        assert_eq!(deferred.global_id(), root);
        deferred.push(ElementId::from(43u64));
        assert_ne!(deferred.global_id(), child);
        assert_eq!(stack.global_id(), child);
        deferred.clear();
        assert_eq!(deferred.global_id(), GlobalElementId::default());
    }

    #[test]
    fn equal_paths_remain_interchangeable_without_interning() {
        let path = [ElementId::from("root"), ElementId::from(42u64)];
        let first = GlobalElementId::from_path(&path);
        let second = GlobalElementId::from_path(&path);
        assert!(!Arc::ptr_eq(&first.0, &second.0));
        assert_eq!(first, second);
        let mut map = collections::FxHashMap::default();
        map.insert(first, 7);
        assert_eq!(map.get(&second), Some(&7));
        assert_eq!(GlobalElementId::default(), GlobalElementId::from_path(&[]));
    }

    #[test]
    fn global_id_cache_reuses_two_frames_and_releases_stale_paths() {
        let mut stack = ElementIdStack::default();
        stack.push(ElementId::from("root"));
        stack.push(ElementId::from("row"));
        let mut cache = GlobalIdCache::default();
        let first = cache.get(&stack);
        assert!(Arc::ptr_eq(&first.0, &cache.get(&stack).0));
        cache.finish_frame();
        assert!(Arc::ptr_eq(&first.0, &cache.get(&stack).0));
        cache.finish_frame();
        cache.finish_frame();
        let later = cache.get(&stack);
        assert_eq!(first, later);
        assert!(!Arc::ptr_eq(&first.0, &later.0));
    }

    #[test]
    fn global_id_cache_aborts_without_losing_committed_ids() {
        let mut stack = ElementIdStack::default();
        stack.push(ElementId::from("committed"));
        let mut cache = GlobalIdCache::default();
        let first = cache.get(&stack);
        cache.finish_frame();
        let mut transient = stack.clone();
        transient.push(ElementId::from("aborted"));
        let abandoned = cache.get(&transient);
        let weak = Arc::downgrade(&abandoned.0);
        drop(abandoned);
        cache.abort_frame();
        assert!(weak.upgrade().is_none());
        assert!(Arc::ptr_eq(&first.0, &cache.get(&stack).0));
    }

    #[test]
    fn global_id_cache_repeated_abort_and_partial_commit_preserve_only_live_paths() {
        let mut a = ElementIdStack::default();
        a.push(ElementId::from("a"));
        let mut b = ElementIdStack::default();
        b.push(ElementId::from("b"));
        let mut c = ElementIdStack::default();
        c.push(ElementId::from("c"));
        let mut cache = GlobalIdCache::default();
        let committed_a = cache.get(&a);
        let committed_b = cache.get(&b);
        cache.finish_frame();
        for _ in 0..3 {
            assert!(Arc::ptr_eq(&committed_a.0, &cache.get(&a).0));
            let abandoned = cache.get(&c);
            let weak = Arc::downgrade(&abandoned.0);
            drop(abandoned);
            cache.abort_frame();
            assert!(weak.upgrade().is_none());
            assert_eq!(cache.entries.len(), 2);
        }
        assert!(Arc::ptr_eq(&committed_b.0, &cache.get(&b).0));
        cache.finish_frame();
        assert_eq!(cache.entries.len(), 1);
        assert!(!Arc::ptr_eq(&committed_a.0, &cache.get(&a).0));
    }

    #[test]
    fn partial_global_id_commit_preserves_unvisited_paths_and_abort_removes_only_new_paths() {
        let path = |name: &'static str| {
            let mut stack = ElementIdStack::default();
            stack.push(ElementId::from(name));
            stack
        };
        let a = path("a");
        let b = path("b");
        let c = path("c");
        let d = path("d");
        let mut cache = GlobalIdCache::default();
        let old = cache.get(&a);
        cache.get(&b);
        cache.finish_frame();
        let fresh = cache.get(&c);
        assert!(cache.can_finish_partial_frame());
        cache.finish_partial_frame();
        let abandoned = cache.get(&d);
        let weak = Arc::downgrade(&abandoned.0);
        drop(abandoned);
        cache.abort_frame();
        assert!(weak.upgrade().is_none());
        assert_eq!(cache.entries.len(), 3);
        assert!(Arc::ptr_eq(&old.0, &cache.get(&a).0));
        assert!(Arc::ptr_eq(&fresh.0, &cache.get(&c).0));
        cache.finish_frame();
        assert_eq!(cache.entries.len(), 2);
    }

    #[test]
    fn global_id_cache_rejects_colliding_paths() {
        let mut stack = ElementIdStack::default();
        stack.push(ElementId::from("row"));
        let mut cache = GlobalIdCache::default();
        let forged = GlobalElementId(Arc::from([ElementId::from("cell")]), stack.hash());
        cache.entries.insert(stack.hash(), GlobalIdEntry {
            path: Arc::clone(&forged.0), stamp: cache.epoch | CANDIDATE_ID,
        });
        cache.used = 1;
        assert_eq!(cache.get(&stack).0.as_ref(), stack.as_slice());
        cache.finish_frame();
        assert_eq!(cache.get(&stack).0.as_ref(), stack.as_slice());
        assert!(Arc::ptr_eq(&cache.entries[&stack.hash()].path, &forged.0));
    }

    #[test]
    fn distinct_paths_do_not_share_an_allocation() {
        let first = GlobalElementId::from_path(&[ElementId::from("root"), ElementId::from(1u64)]);
        let second = GlobalElementId::from_path(&[ElementId::from("root"), ElementId::from(2u64)]);
        assert!(!Arc::ptr_eq(&first.0, &second.0));
        assert_ne!(first, second);
    }

    #[test]
    fn cached_hash_does_not_replace_structural_equality() {
        let first = GlobalElementId::from_path(&[ElementId::from("first")]);
        let mut second = GlobalElementId::from_path(&[ElementId::from("second")]);
        second.1 = first.1;
        assert_ne!(first, second);
    }
}
