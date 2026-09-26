use crate::{
    App, AvailableSpace, Bounds, DispatchNodeId, ElementId, InspectorElementId, LayoutId, PaintIndex,
    Pixels, PrepaintStateIndex, SharedString, Size, TextLayout, TextStyle, Window,
    WrappedLineLayout,
};
use crate::window::{
    RetainedElementIdentity, debug_visualization::ViewCacheDebugStatus,
};
use derive_more::Deref;
use smallvec::SmallVec;
use std::{
    any::{Any, TypeId},
    cell::Cell,
    fmt::{self, Display},
    mem,
    ops::Range,
    rc::Rc,
    sync::Arc,
};

use super::{DivPrepaint, Element};

/// A globally unique identifier for an element, used to track state across frames.
#[derive(Clone, Deref, Default, Debug, Eq, PartialEq, Hash)]
pub struct GlobalElementId(pub(crate) Arc<[ElementId]>);

impl GlobalElementId {
    #[inline]
    pub(crate) fn from_path(path: &[ElementId]) -> Self {
        Self(Arc::from(path))
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

/// Exact output identity for side-effect-free plain text elements on a generic dirty frame.
///
/// The shaped/wrapped line layout Arcs come directly from the text-system cache. Pointer equality
/// therefore proves that the previous scene references the exact same glyph geometry and wrap
/// boundaries; a cache miss conservatively causes repaint rather than relying on a hash collision.
#[derive(Clone, Debug)]
pub(crate) struct RetainedPlainTextKey {
    pub(crate) text: SharedString,
    pub(crate) text_style: TextStyle,
    pub(crate) rem_size: Pixels,
    pub(crate) line_layouts: SmallVec<[Arc<WrappedLineLayout>; 1]>,
}

impl PartialEq for RetainedPlainTextKey {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
            && self.text_style == other.text_style
            && self.rem_size == other.rem_size
            && self.line_layouts.len() == other.line_layouts.len()
            && self
                .line_layouts
                .iter()
                .zip(&other.line_layouts)
                .all(|(left, right)| Arc::ptr_eq(left, right))
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
        retained_id: GlobalElementId,
        retained_identity_ambiguity: SmallVec<[Rc<Cell<bool>>; 4]>,
        inspector_id: Option<InspectorElementId>,
        request_layout: RequestLayoutState,
    },
    LayoutComputed {
        layout_id: LayoutId,
        global_id: Option<GlobalElementId>,
        retained_segment: ElementId,
        retained_id: GlobalElementId,
        retained_identity_ambiguity: SmallVec<[Rc<Cell<bool>>; 4]>,
        inspector_id: Option<InspectorElementId>,
        available_space: Size<AvailableSpace>,
        request_layout: RequestLayoutState,
    },
    Prepaint {
        node_id: DispatchNodeId,
        global_id: Option<GlobalElementId>,
        retained_segment: ElementId,
        retained_id: GlobalElementId,
        retained_identity_ambiguity: SmallVec<[Rc<Cell<bool>>; 4]>,
        inspector_id: Option<InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout_id: LayoutId,
        layout_fingerprint: Option<u64>,
        request_layout: RequestLayoutState,
        prepaint: PrepaintState,
        prepaint_range: Range<PrepaintStateIndex>,
        plain_text_key: Option<RetainedPlainTextKey>,
    },
    Retained {
        bounds: Bounds<Pixels>,
        source_prepaint_range: Range<PrepaintStateIndex>,
        source_paint_range: Range<PaintIndex>,
        source_metadata_range: Range<usize>,
        prepaint_range: Range<PrepaintStateIndex>,
    },
    Painted,
}

#[inline(never)]
fn prepare_element_id(element_id: ElementId, window: &mut Window) -> GlobalElementId {
    window.element_id_stack.push(element_id);
    GlobalElementId::from_path(&window.element_id_stack)
}

#[cfg(any(feature = "inspector", debug_assertions))]
#[inline(never)]
fn prepare_inspector_id(
    source: &'static core::panic::Location<'static>,
    window: &mut Window,
) -> InspectorElementId {
    let path = crate::InspectorElementPath {
        global_id: GlobalElementId::from_path(&window.element_id_stack),
        source_location: source,
    };
    window.build_inspector_element_id(path)
}

#[inline(never)]
fn retained_identity_is_stable(ambiguity: &[Rc<Cell<bool>>]) -> bool {
    ambiguity.iter().all(|flag| !flag.get())
}

#[inline(never)]
fn retained_plain_text_semantics(
    element: &dyn Any,
    window: &Window,
) -> Option<(SharedString, TextStyle, Pixels)> {
    let text = if let Some(text) = element.downcast_ref::<SharedString>() {
        text.clone()
    } else if let Some(text) = element.downcast_ref::<&'static str>() {
        SharedString::from(*text)
    } else {
        return None;
    };
    Some((text, window.text_style(), window.rem_size()))
}

#[inline(never)]
fn retained_plain_text_key(
    element: &dyn Any,
    request_layout: &dyn Any,
    window: &Window,
) -> Option<RetainedPlainTextKey> {
    let text = if let Some(text) = element.downcast_ref::<SharedString>() {
        text.clone()
    } else if let Some(text) = element.downcast_ref::<&'static str>() {
        SharedString::from(*text)
    } else {
        return None;
    };

    let text_layout = request_layout.downcast_ref::<TextLayout>()?;
    Some(RetainedPlainTextKey {
        text,
        text_style: window.text_style(),
        rem_size: window.rem_size(),
        line_layouts: text_layout.retained_line_layouts(),
    })
}

struct RetainedElementMount {
    global_id: Option<GlobalElementId>,
    retained_segment: ElementId,
    retained_id: GlobalElementId,
    retained_identity_ambiguity: SmallVec<[Rc<Cell<bool>>; 4]>,
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
    let (retained_segment, retained_id, retained_identity_ambiguity) =
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
        retained_id,
        retained_identity_ambiguity,
        inspector_id,
    }
}

#[inline(never)]
fn finish_retained_element_layout(
    mount: &RetainedElementMount,
    layout_id: LayoutId,
    request_layout: &dyn Any,
    plain_text_semantics: Option<(SharedString, TextStyle, Pixels)>,
    window: &mut Window,
) {
    window.register_retained_layout_semantics(
        &mount.retained_id,
        &mount.retained_segment,
        layout_id,
        request_layout,
        plain_text_semantics,
        mount.retained_identity_ambiguity.clone(),
    );

    if mount.global_id.is_some() {
        window.element_id_stack.pop();
    }
    window.end_retained_element();
}

#[inline(never)]
fn try_reuse_retained_element(
    outer_replay_safe: bool,
    identity_stable: bool,
    retained_segment: &ElementId,
    retained_id: &GlobalElementId,
    bounds: Bounds<Pixels>,
    layout_id: LayoutId,
    layout_fingerprint: Option<u64>,
    plain_text_key: Option<&RetainedPlainTextKey>,
    window: &mut Window,
) -> Option<(
    Range<PrepaintStateIndex>,
    Range<PaintIndex>,
    Range<usize>,
    Range<PrepaintStateIndex>,
)> {
    if !outer_replay_safe || (!identity_stable && plain_text_key.is_none()) {
        return None;
    }

    let retained = window.with_retained_element_segment(retained_segment, |window| {
        window.reusable_retained_element(
            retained_id,
            bounds,
            layout_id,
            layout_fingerprint,
            plain_text_key,
        )
    })?;

    let source_prepaint_range = retained.prepaint_range.clone();
    let prepaint_start = window.prepaint_index();
    if !window.reuse_prepaint(source_prepaint_range.clone()) {
        return None;
    }
    let prepaint_end = window.prepaint_index();

    Some((
        source_prepaint_range,
        retained.paint_range,
        retained.metadata_range,
        prepaint_start..prepaint_end,
    ))
}

#[inline(never)]
fn run_element_prepaint(
    element_id: Option<ElementId>,
    global_id: Option<&GlobalElementId>,
    retained_segment: &ElementId,
    window: &mut Window,
    callback: &mut dyn FnMut(&mut Window),
) -> (DispatchNodeId, Range<PrepaintStateIndex>) {
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

    let prepaint_start = window.prepaint_index();
    let node_id = window.next_frame.dispatch_tree.push_node();
    window.with_retained_element_segment(retained_segment, |window| callback(window));
    window.next_frame.dispatch_tree.pop_node();
    let prepaint_end = window.prepaint_index();

    if global_id.is_some() {
        window.element_id_stack.pop();
    }

    (node_id, prepaint_start..prepaint_end)
}

struct RetainedPaintRun {
    metadata_start: usize,
    unstable_identity_start: usize,
    paint_range: Range<PaintIndex>,
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
) -> RetainedPaintRun {
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

    let metadata_start = window.retained_element_metadata_len();
    let unstable_identity_start = window.next_frame.retained_unstable_identity_count;
    let paint_start = window.paint_index();
    window.record_debug_element_paint(bounds, cx);
    window.next_frame.dispatch_tree.set_active_node(node_id);
    window.with_retained_element_segment(retained_segment, |window| callback(window, cx));
    let paint_end = window.paint_index();

    RetainedPaintRun {
        metadata_start,
        unstable_identity_start,
        paint_range: paint_start..paint_end,
    }
}

#[inline(never)]
fn finish_element_paint(global_id: Option<&GlobalElementId>, window: &mut Window) {
    if global_id.is_some() {
        window.element_id_stack.pop();
    }
}

#[inline(never)]
fn retained_div_self_scene(prepaint: &dyn Any) -> Option<super::RetainedDivSelfScene> {
    prepaint
        .downcast_ref::<DivPrepaint>()
        .and_then(DivPrepaint::retained_self_scene)
}

#[inline(never)]
fn record_retained_painted_element(
    retained_id: GlobalElementId,
    bounds: Bounds<Pixels>,
    layout_id: LayoutId,
    layout_fingerprint: Option<u64>,
    prepaint_range: Range<PrepaintStateIndex>,
    paint_range: Range<PaintIndex>,
    metadata_start: usize,
    div_self_scene: Option<super::RetainedDivSelfScene>,
    plain_text_key: Option<RetainedPlainTextKey>,
    identity_stable: bool,
    outer_replay_safe: bool,
    unstable_identity_start: usize,
    window: &mut Window,
) {
    let subtree_stable = identity_stable
        && outer_replay_safe
        && window.next_frame.retained_unstable_identity_count == unstable_identity_start;
    window.record_retained_element_range(
        retained_id,
        bounds,
        layout_id,
        layout_fingerprint,
        prepaint_range,
        paint_range,
        metadata_start,
        div_self_scene,
        plain_text_key,
        identity_stable,
        subtree_stable,
    );

    // Reuse the existing O(1) subtree-stability propagation counter for explicit replay barriers
    // as well as ambiguous identities. Ancestors snapshot this counter before painting children,
    // so a frame-local cache boundary prevents an unrelated ancestor from replaying across it while
    // the boundary's own internal cache remains usable.
    if !outer_replay_safe {
        window.next_frame.retained_unstable_identity_count = window
            .next_frame
            .retained_unstable_identity_count
            .saturating_add(1);
    }
}

#[inline(never)]
fn paint_retained_element(
    bounds: Bounds<Pixels>,
    source_prepaint_range: Range<PrepaintStateIndex>,
    source_paint_range: Range<PaintIndex>,
    source_metadata_range: Range<usize>,
    prepaint_range: Range<PrepaintStateIndex>,
    window: &mut Window,
    cx: &mut App,
) {
    let paint_start = window.paint_index();
    if window.reuse_paint(source_paint_range.clone()) {
        let paint_end = window.paint_index();
        let paint_range = paint_start..paint_end;
        if window.replay_retained_element_metadata(
            &source_prepaint_range,
            &source_paint_range,
            &source_metadata_range,
            &prepaint_range,
            &paint_range,
        ) {
            window.record_debug_view_cache_status(bounds, ViewCacheDebugStatus::Hit, cx);
        } else {
            window.degrade_current_draw();
        }
    } else {
        window.degrade_current_draw();
    }
}

#[inline(never)]
fn compute_element_root_layout(
    layout_id: LayoutId,
    available_space: Size<AvailableSpace>,
    previous_available_space: Option<Size<AvailableSpace>>,
    global_id: Option<&GlobalElementId>,
    window: &mut Window,
    cx: &mut App,
) {
    if previous_available_space != Some(available_space) {
        window.compute_layout_with_diagnostic_id(layout_id, available_space, global_id, cx);
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
                let plain_text_semantics =
                    retained_plain_text_semantics(&self.element as &dyn Any, window);
                finish_retained_element_layout(
                    &mount,
                    layout_id,
                    &request_layout as &dyn Any,
                    plain_text_semantics,
                    window,
                );

                self.phase = ElementDrawPhase::RequestLayout {
                    layout_id,
                    global_id: mount.global_id,
                    retained_segment: mount.retained_segment,
                    retained_id: mount.retained_id,
                    retained_identity_ambiguity: mount.retained_identity_ambiguity,
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
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                mut request_layout,
            }
            | ElementDrawPhase::LayoutComputed {
                layout_id,
                global_id,
                retained_segment,
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                mut request_layout,
                ..
            } => {
                let bounds = window.layout_bounds(layout_id);
                let layout_fingerprint = window.retained_layout_fingerprint(layout_id);
                let identity_stable =
                    retained_identity_is_stable(&retained_identity_ambiguity);
                // ReconcileSubtree proof needs exact shaped text output. Non-text elements exit
                // this helper after two cheap type checks; safe Divs use semantic generations.
                let mut plain_text_key =
                    retained_plain_text_key(
                        &self.element as &dyn Any,
                        &request_layout as &dyn Any,
                        window,
                    );
                if let Some((
                    source_prepaint_range,
                    source_paint_range,
                    source_metadata_range,
                    prepaint_range,
                )) = try_reuse_retained_element(
                    E::RETAINED_REPLAY_CAPABILITY.allows_outer_replay(),
                    identity_stable,
                    &retained_segment,
                    &retained_id,
                    bounds,
                    layout_id,
                    layout_fingerprint,
                    plain_text_key.as_ref(),
                    window,
                ) {
                    self.phase = ElementDrawPhase::Retained {
                        bounds,
                        source_prepaint_range,
                        source_paint_range,
                        source_metadata_range,
                        prepaint_range,
                    };
                    return;
                }

                let mut prepaint = None;
                let (node_id, prepaint_range) = run_element_prepaint(
                    self.element.id(),
                    global_id.as_ref(),
                    &retained_segment,
                    window,
                    &mut |window| {
                        prepaint = Some(self.element.prepaint(
                            global_id.as_ref(),
                            inspector_id.as_ref(),
                            bounds,
                            &mut request_layout,
                            window,
                            cx,
                        ));
                    },
                );
                let prepaint =
                    prepaint.expect("element prepaint callback must execute exactly once");

                self.phase = ElementDrawPhase::Prepaint {
                    node_id,
                    global_id,
                    retained_segment,
                    retained_id,
                    retained_identity_ambiguity,
                    inspector_id,
                    bounds,
                    layout_id,
                    layout_fingerprint,
                    request_layout,
                    prepaint,
                    prepaint_range,
                    plain_text_key,
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
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                bounds,
                layout_id,
                layout_fingerprint,
                mut request_layout,
                mut prepaint,
                prepaint_range,
                plain_text_key,
            } => {
                let paint_run = run_element_paint(
                    self.element.id(),
                    global_id.as_ref(),
                    node_id,
                    &retained_segment,
                    bounds,
                    window,
                    cx,
                    &mut |window, cx| {
                        self.element.paint(
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
                let div_self_scene = retained_div_self_scene(&prepaint as &dyn Any);
                let identity_stable =
                    retained_identity_is_stable(&retained_identity_ambiguity);
                record_retained_painted_element(
                    retained_id,
                    bounds,
                    layout_id,
                    layout_fingerprint,
                    prepaint_range,
                    paint_run.paint_range,
                    paint_run.metadata_start,
                    div_self_scene,
                    plain_text_key,
                    identity_stable,
                    E::RETAINED_REPLAY_CAPABILITY.allows_outer_replay(),
                    paint_run.unstable_identity_start,
                    window,
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
            ElementDrawPhase::Retained {
                bounds,
                source_prepaint_range,
                source_paint_range,
                source_metadata_range,
                prepaint_range,
            } => {
                paint_retained_element(
                    bounds,
                    source_prepaint_range,
                    source_paint_range,
                    source_metadata_range,
                    prepaint_range,
                    window,
                    cx,
                );
                self.phase = ElementDrawPhase::Painted;
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
            retained_id,
            retained_identity_ambiguity,
            inspector_id,
            previous_available_space,
            request_layout,
        ) = match mem::take(&mut self.phase) {
            ElementDrawPhase::RequestLayout {
                layout_id,
                global_id,
                retained_segment,
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                request_layout,
            } => (
                layout_id,
                global_id,
                retained_segment,
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                None,
                request_layout,
            ),
            ElementDrawPhase::LayoutComputed {
                layout_id,
                global_id,
                retained_segment,
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                available_space: previous_available_space,
                request_layout,
            } => (
                layout_id,
                global_id,
                retained_segment,
                retained_id,
                retained_identity_ambiguity,
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
            global_id.as_ref(),
            window,
            cx,
        );
        self.phase = ElementDrawPhase::LayoutComputed {
            layout_id,
            global_id,
            retained_segment,
            retained_id,
            retained_identity_ambiguity,
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
