use crate::window::RetainedElementIdentity;
use crate::{
    App, AvailableSpace, Bounds, DispatchNodeId, ElementId, InspectorElementId, LayoutId,
    Pixels, Size, Window,
};
use derive_more::Deref;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
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
    ids: SmallVec<[ElementId; 32]>,
    hashes: SmallVec<[u64; 32]>,
}

impl ElementIdStack {
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
    }

    /// The hash of the whole path on the stack.
    pub(crate) fn hash(&self) -> u64 {
        self.hashes
            .last()
            .copied()
            .unwrap_or(EMPTY_PATH_HASH)
    }

    pub(crate) fn as_slice(&self) -> &[ElementId] {
        &self.ids
    }

    pub(crate) fn global_id(&self) -> GlobalElementId {
        GlobalElementId::with_hash(&self.ids, self.hash())
    }
}

impl std::ops::Deref for ElementIdStack {
    type Target = [ElementId];

    fn deref(&self) -> &Self::Target {
        &self.ids
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
/// The global ids handed out this frame and the last, by the hash of their
/// path.
///
/// An element whose path is the one it had on the frame before, which is
/// nearly every element, is given the id it had then, rather than a copy of
/// the whole element id stack to be dropped id by id when the frame is done.
#[derive(Default)]
pub(crate) struct GlobalIdCache {
    previous: collections::FxHashMap<u64, GlobalElementId>,
    current: collections::FxHashMap<u64, GlobalElementId>,
}

impl GlobalIdCache {
    /// The global id of `path`: one handed out already if there is one, a
    /// new one otherwise.
    pub(crate) fn get(&mut self, path: &ElementIdStack) -> GlobalElementId {
        let hash = path.hash();
        if let Some(id) = self.current.get(&hash)
            && id.0.as_ref() == path.as_slice()
        {
            return id.clone();
        }
        let id = match self.previous.get(&hash) {
            Some(id) if id.0.as_ref() == path.as_slice() => id.clone(),
            _ => path.global_id(),
        };
        self.current.insert(hash, id.clone());
        id
    }

    /// Keeps this frame's ids for the next one to find, and forgets the
    /// previous frame's.
    pub(crate) fn finish_frame(&mut self) {
        mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
    }

    /// Keep the committed cache when BMCBL abandons a speculative draw.
    pub(crate) fn abort_frame(&mut self) {
        self.current.clear();
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

    fn set_retained_source_location(
        &mut self,
        source: &'static core::panic::Location<'static>,
        ordinal: Option<u32>,
    );

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
    retained_source_location: Option<&'static core::panic::Location<'static>>,
    retained_source_ordinal: Option<u32>,
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
        retained_segment: ElementId,
        inspector_id: Option<InspectorElementId>,
        request_layout: RequestLayoutState,
    },
    LayoutComputed {
        layout_id: LayoutId,
        global_id: Option<GlobalElementId>,
        retained_segment: ElementId,
        inspector_id: Option<InspectorElementId>,
        available_space: Size<AvailableSpace>,
        request_layout: RequestLayoutState,
    },
    Prepaint {
        node_id: DispatchNodeId,
        global_id: Option<GlobalElementId>,
        retained_segment: ElementId,
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

struct RetainedElementMount {
    global_id: Option<GlobalElementId>,
    retained_segment: ElementId,
    inspector_id: Option<InspectorElementId>,
}

#[inline(never)]
fn begin_retained_element_mount(
    element_id: Option<ElementId>,
    element_source_location: Option<&'static core::panic::Location<'static>>,
    retained_source_location: Option<&'static core::panic::Location<'static>>,
    retained_source_ordinal: Option<u32>,
    element_type: TypeId,
    window: &mut Window,
) -> RetainedElementMount {
    let _work = crate::window::element_work::begin_named(
        "layout_retained_mount",
        crate::window::element_work::ElementWorkPhase::Layout,
    );
    let retained_identity = if let Some(element_id) = element_id.clone() {
        RetainedElementIdentity::Explicit(element_id)
    } else if retained_source_location.is_some() || element_source_location.is_some() {
        RetainedElementIdentity::Auto {
            mount: retained_source_location.copied(),
            source: element_source_location.copied(),
            element_type,
            ordinal: retained_source_ordinal,
        }
    } else {
        RetainedElementIdentity::Positional
    };
    let (retained_segment, _, _) =
        window.begin_retained_element(retained_identity);
    let global_id = element_id.map(|element_id| prepare_element_id(element_id, window));

    let inspector_id;
    #[cfg(any(feature = "inspector", debug_assertions))]
    {
        inspector_id = if window.inspector_enabled() {
            element_source_location.map(|source| prepare_inspector_id(source, window))
        } else {
            None
        };
    }
    #[cfg(not(any(feature = "inspector", debug_assertions)))]
    {
        inspector_id = None;
    }

    RetainedElementMount {
        global_id,
        retained_segment,
        inspector_id,
    }
}

#[inline(never)]
fn run_element_prepaint(
    element_id: Option<ElementId>,
    global_id: Option<&GlobalElementId>,
    retained_segment: &ElementId,
    window: &mut Window,
    callback: &mut dyn FnMut(&mut Window),
) -> DispatchNodeId {
    if let Some(element_id) = element_id {
        window.element_id_stack.push(element_id);
        debug_assert_eq!(
            global_id
                .expect("element id must have a corresponding global id")
                .0
                .as_ref(),
            window.element_id_stack.as_slice()
        );
    }

    let node_id = window.next_frame.dispatch_tree.push_node();
    window.with_retained_element_segment(retained_segment, |window| callback(window));
    window.next_frame.dispatch_tree.pop_node();

    if global_id.is_some() {
        window.element_id_stack.pop();
    }

    node_id
}

#[inline(never)]
fn run_element_paint(
    element_id: Option<ElementId>,
    global_id: Option<&GlobalElementId>,
    node_id: DispatchNodeId,
    retained_segment: &ElementId,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
    callback: &mut dyn FnMut(&mut Window, &mut App),
) {
    if let Some(element_id) = element_id {
        window.element_id_stack.push(element_id);
        debug_assert_eq!(
            global_id
                .expect("element id must have a corresponding global id")
                .0
                .as_ref(),
            window.element_id_stack.as_slice()
        );
    }

    window.record_debug_element_paint(bounds, cx);
    window.next_frame.dispatch_tree.set_active_node(node_id);
    window.with_retained_element_segment(retained_segment, |window| callback(window, cx));
}

#[inline(never)]
fn finish_element_paint(global_id: Option<&GlobalElementId>, window: &mut Window) {
    if global_id.is_some() {
        window.element_id_stack.pop();
    }
}

#[inline(never)]
fn compute_element_root_layout(
    layout_id: LayoutId,
    available_space: Size<AvailableSpace>,
    previous_available_space: Option<Size<AvailableSpace>>,
    window: &mut Window,
    cx: &mut App,
) {
    if previous_available_space != Some(available_space) {
        window.compute_layout(layout_id, available_space, cx);
    }
}

/// A wrapper around an implementer of [`Element`] that allows it to be drawn in a window.
impl<E: Element> Drawable<E> {
    pub(crate) fn new(element: E) -> Self {
        Drawable {
            element,
            retained_source_location: None,
            retained_source_ordinal: None,
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
                let element_source_location = self.element.source_location();
                let mount = begin_retained_element_mount(
                    self.element.id(),
                    element_source_location,
                    self.retained_source_location,
                    self.retained_source_ordinal,
                    TypeId::of::<E>(),
                    window,
                );

                let (layout_id, request_layout) = self.element.request_layout(
                    mount.global_id.as_ref(),
                    mount.inspector_id.as_ref(),
                    window,
                    cx,
                );
                if mount.global_id.is_some() {
                    window.element_id_stack.pop();
                }
                window.end_retained_element();

                self.phase = ElementDrawPhase::RequestLayout {
                    layout_id,
                    global_id: mount.global_id,
                    retained_segment: mount.retained_segment,
                    inspector_id: mount.inspector_id,
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
                retained_segment,
                inspector_id,
                mut request_layout,
            }
            | ElementDrawPhase::LayoutComputed {
                layout_id,
                global_id,
                retained_segment,
                inspector_id,
                mut request_layout,
                ..
            } => {
                let bounds = window.layout_bounds(layout_id);
                let mut prepaint = None;
                let element_type = std::any::type_name::<E>();
                let prepaint_element_id = self.element.id();
                let element = &mut self.element;
                let node_id = run_element_prepaint(
                    prepaint_element_id,
                    global_id.as_ref(),
                    &retained_segment,
                    window,
                    &mut |window| {
                        prepaint = Some(crate::window::element_work::measure_named(
                            element_type,
                            crate::window::element_work::ElementWorkPhase::Prepaint,
                            window,
                            cx,
                            |window, cx| {
                                element.prepaint(
                                    global_id.as_ref(),
                                    inspector_id.as_ref(),
                                    bounds,
                                    &mut request_layout,
                                    window,
                                    cx,
                                )
                            },
                        ));
                    },
                );
                let prepaint =
                    prepaint.expect("element prepaint callback must execute exactly once");

                self.phase = ElementDrawPhase::Prepaint {
                    node_id,
                    global_id,
                    retained_segment,
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
                retained_segment,
                inspector_id,
                bounds,
                mut request_layout,
                mut prepaint,
            } => {
                let paint_element_type = std::any::type_name::<E>();
                let paint_element_id = self.element.id();
                let element = &mut self.element;
                run_element_paint(
                    paint_element_id,
                    global_id.as_ref(),
                    node_id,
                    &retained_segment,
                    bounds,
                    window,
                    cx,
                    &mut |window, cx| {
                        crate::window::element_work::measure_named(
                            paint_element_type,
                            crate::window::element_work::ElementWorkPhase::Paint,
                            window,
                            cx,
                            |window, cx| {
                                element.paint(
                                    global_id.as_ref(),
                                    inspector_id.as_ref(),
                                    bounds,
                                    &mut request_layout,
                                    &mut prepaint,
                                    window,
                                    cx,
                                );
                            },
                        );
                    },
                );
                finish_element_paint(global_id.as_ref(), window);

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
            retained_segment,
            inspector_id,
            previous_available_space,
            request_layout,
        ) = match mem::take(&mut self.phase) {
            ElementDrawPhase::RequestLayout {
                layout_id,
                global_id,
                retained_segment,
                inspector_id,
                request_layout,
            } => (
                layout_id,
                global_id,
                retained_segment,
                inspector_id,
                None,
                request_layout,
            ),
            ElementDrawPhase::LayoutComputed {
                layout_id,
                global_id,
                retained_segment,
                inspector_id,
                available_space: previous_available_space,
                request_layout,
            } => (
                layout_id,
                global_id,
                retained_segment,
                inspector_id,
                Some(previous_available_space),
                request_layout,
            ),
            _ => panic!("cannot measure after painting"),
        };

        compute_element_root_layout(
            layout_id,
            available_space,
            previous_available_space,
            window,
            cx,
        );
        self.phase = ElementDrawPhase::LayoutComputed {
            layout_id,
            global_id,
            retained_segment,
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

    fn set_retained_source_location(
        &mut self,
        source: &'static core::panic::Location<'static>,
        ordinal: Option<u32>,
    ) {
        self.retained_source_location = Some(source);
        self.retained_source_ordinal = ordinal;
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
    fn global_id_cache_rejects_colliding_paths() {
        let mut stack = ElementIdStack::default();
        stack.push(ElementId::from("row"));
        let mut cache = GlobalIdCache::default();
        let forged = GlobalElementId(Arc::from([ElementId::from("cell")]), stack.hash());
        cache.current.insert(stack.hash(), forged.clone());
        assert_eq!(cache.get(&stack).0.as_ref(), stack.as_slice());
        cache.finish_frame();
        cache.previous.insert(stack.hash(), forged);
        assert_eq!(cache.get(&stack).0.as_ref(), stack.as_slice());
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
