use crate::{
    App, AvailableSpace, Bounds, DispatchNodeId, ElementId, InspectorElementId, LayoutId, PaintIndex,
    Pixels, PrepaintStateIndex, SharedString, Size, TextLayout, TextStyle, Window,
    WrappedLineLayout,
};
use crate::window::{
    RetainedElementIdentity, debug_visualization::ViewCacheDebugStatus,
};
use derive_more::{Deref, DerefMut};
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
#[derive(Clone, Deref, DerefMut, Default, Debug, Eq, PartialEq, Hash)]
pub struct GlobalElementId(pub(crate) SmallVec<[ElementId; 32]>);

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

fn retained_identity_is_stable(ambiguity: &[Rc<Cell<bool>>]) -> bool {
    ambiguity.iter().all(|flag| !flag.get())
}

fn retained_plain_text_key<E: Element>(
    element: &E,
    request_layout: &E::RequestLayoutState,
    window: &Window,
) -> Option<RetainedPlainTextKey> {
    let element = element as &dyn Any;
    let text = if let Some(text) = element.downcast_ref::<SharedString>() {
        text.clone()
    } else if let Some(text) = element.downcast_ref::<&'static str>() {
        SharedString::from(*text)
    } else {
        return None;
    };

    let text_layout = (request_layout as &dyn Any).downcast_ref::<TextLayout>()?;
    Some(RetainedPlainTextKey {
        text,
        text_style: window.text_style(),
        rem_size: window.rem_size(),
        line_layouts: text_layout.retained_line_layouts(),
    })
}

/// A wrapper around an implementer of [`Element`] that allows it to be drawn in a window.
impl<E: Element> Drawable<E> {
    pub(crate) fn new(element: E) -> Self {
        Drawable {
            element,
            retained_source_location: None,
            retained_source_ordinal: None,
            phase: ElementDrawPhase::Start,
        }
    }

    fn request_layout(&mut self, window: &mut Window, cx: &mut App) -> LayoutId {
        match mem::take(&mut self.phase) {
            ElementDrawPhase::Start => {
                let element_id = self.element.id();
                let element_source_location = self.element.source_location();
                let mount = self.retained_source_location.copied();
                let source = element_source_location.copied();
                let retained_identity = if let Some(element_id) = element_id.clone() {
                    RetainedElementIdentity::Explicit(element_id)
                } else if mount.is_some() || source.is_some() {
                    RetainedElementIdentity::Auto {
                        mount,
                        source,
                        element_type: TypeId::of::<E>(),
                        ordinal: self.retained_source_ordinal,
                    }
                } else {
                    RetainedElementIdentity::Positional
                };
                let (retained_segment, retained_id, retained_identity_ambiguity) =
                    window.begin_retained_element(retained_identity);
                let global_id = element_id.map(|element_id| {
                    window.element_id_stack.push(element_id);
                    GlobalElementId(window.element_id_stack.clone())
                });

                let inspector_id;
                #[cfg(any(feature = "inspector", debug_assertions))]
                {
                    inspector_id = element_source_location.map(|source| {
                        let path = crate::InspectorElementPath {
                            global_id: GlobalElementId(window.element_id_stack.clone()),
                            source_location: source,
                        };
                        window.build_inspector_element_id(path)
                    });
                }
                #[cfg(not(any(feature = "inspector", debug_assertions)))]
                {
                    inspector_id = None;
                }

                let (layout_id, request_layout) = self.element.request_layout(
                    global_id.as_ref(),
                    inspector_id.as_ref(),
                    window,
                    cx,
                );

                if global_id.is_some() {
                    window.element_id_stack.pop();
                }
                window.end_retained_element();

                self.phase = ElementDrawPhase::RequestLayout {
                    layout_id,
                    global_id,
                    retained_segment,
                    retained_id,
                    retained_identity_ambiguity,
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
                let identity_stable =
                    retained_identity_is_stable(&retained_identity_ambiguity);
                let targeted_replay = window.retained_replay_is_targeted();
                let mut plain_text_key = (!targeted_replay || !identity_stable)
                    .then(|| retained_plain_text_key(&self.element, &request_layout, window))
                    .flatten();
                let may_reconcile = E::RETAINED_REPLAY_CAPABILITY.allows_outer_replay()
                    && (identity_stable || plain_text_key.is_some());

                let retained = may_reconcile
                    .then(|| {
                        window.with_retained_element_segment(&retained_segment, |window| {
                            window.reusable_retained_element(
                                &retained_id,
                                bounds,
                                plain_text_key.as_ref(),
                            )
                        })
                    })
                    .flatten();
                if let Some(retained) = retained {
                    let source_prepaint_range = retained.prepaint_range.clone();
                    let prepaint_start = window.prepaint_index();
                    if window.reuse_prepaint(source_prepaint_range.clone()) {
                        let prepaint_end = window.prepaint_index();
                        self.phase = ElementDrawPhase::Retained {
                            bounds,
                            source_prepaint_range,
                            source_paint_range: retained.paint_range,
                            source_metadata_range: retained.metadata_range,
                            prepaint_range: prepaint_start..prepaint_end,
                        };
                        return;
                    }
                }

                if plain_text_key.is_none() {
                    plain_text_key = retained_plain_text_key(&self.element, &request_layout, window);
                }

                if let Some(element_id) = self.element.id() {
                    window.element_id_stack.push(element_id);
                    debug_assert_eq!(global_id.as_ref().unwrap().0, window.element_id_stack);
                }

                let prepaint_start = window.prepaint_index();
                let node_id = window.next_frame.dispatch_tree.push_node();
                let prepaint = window.with_retained_element_segment(&retained_segment, |window| {
                    self.element.prepaint(
                        global_id.as_ref(),
                        inspector_id.as_ref(),
                        bounds,
                        &mut request_layout,
                        window,
                        cx,
                    )
                });
                window.next_frame.dispatch_tree.pop_node();
                let prepaint_end = window.prepaint_index();

                if global_id.is_some() {
                    window.element_id_stack.pop();
                }

                self.phase = ElementDrawPhase::Prepaint {
                    node_id,
                    global_id,
                    retained_segment,
                    retained_id,
                    retained_identity_ambiguity,
                    inspector_id,
                    bounds,
                    request_layout,
                    prepaint,
                    prepaint_range: prepaint_start..prepaint_end,
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
                mut request_layout,
                mut prepaint,
                prepaint_range,
                plain_text_key,
            } => {
                if let Some(element_id) = self.element.id() {
                    window.element_id_stack.push(element_id);
                    debug_assert_eq!(global_id.as_ref().unwrap().0, window.element_id_stack);
                }

                let metadata_start = window.retained_element_metadata_len();
                let unstable_identity_start = window.next_frame.retained_unstable_identity_count;
                let paint_start = window.paint_index();
                window.record_debug_element_paint(bounds, cx);
                window.next_frame.dispatch_tree.set_active_node(node_id);
                window.with_retained_element_segment(&retained_segment, |window| {
                    self.element.paint(
                        global_id.as_ref(),
                        inspector_id.as_ref(),
                        bounds,
                        &mut request_layout,
                        &mut prepaint,
                        window,
                        cx,
                    );
                });
                let paint_end = window.paint_index();
                let div_self_scene = (&prepaint as &dyn Any)
                    .downcast_ref::<DivPrepaint>()
                    .and_then(DivPrepaint::retained_self_scene);

                let identity_stable =
                    retained_identity_is_stable(&retained_identity_ambiguity);
                let outer_replay_safe = E::RETAINED_REPLAY_CAPABILITY.allows_outer_replay();
                let subtree_stable = identity_stable
                    && outer_replay_safe
                    && window.next_frame.retained_unstable_identity_count
                        == unstable_identity_start;
                window.record_retained_element_range(
                    retained_id,
                    bounds,
                    prepaint_range,
                    paint_start..paint_end,
                    metadata_start,
                    div_self_scene,
                    plain_text_key,
                    identity_stable,
                    subtree_stable,
                );

                // Reuse the existing O(1) subtree-stability propagation counter for explicit replay
                // barriers as well as ambiguous identities. Ancestors snapshot this counter before
                // painting children, so a frame-local cache boundary prevents an unrelated ancestor
                // from replaying across it while the boundary's own internal cache remains usable.
                if !outer_replay_safe {
                    window.next_frame.retained_unstable_identity_count = window
                        .next_frame
                        .retained_unstable_identity_count
                        .saturating_add(1);
                }

                if global_id.is_some() {
                    window.element_id_stack.pop();
                }

                self.phase = ElementDrawPhase::Painted;
            }
            ElementDrawPhase::Retained {
                bounds,
                source_prepaint_range,
                source_paint_range,
                source_metadata_range,
                prepaint_range,
            } => {
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
                        window.record_debug_view_cache_status(
                            bounds,
                            ViewCacheDebugStatus::Hit,
                            cx,
                        );
                    } else {
                        window.degrade_current_draw();
                    }
                } else {
                    window.degrade_current_draw();
                }
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

        let layout_id = match mem::take(&mut self.phase) {
            ElementDrawPhase::RequestLayout {
                layout_id,
                global_id,
                retained_segment,
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                request_layout,
            } => {
                window.compute_layout(layout_id, available_space, cx);
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
                layout_id
            }
            ElementDrawPhase::LayoutComputed {
                layout_id,
                global_id,
                retained_segment,
                retained_id,
                retained_identity_ambiguity,
                inspector_id,
                available_space: prev_available_space,
                request_layout,
            } => {
                if available_space != prev_available_space {
                    window.compute_layout(layout_id, available_space, cx);
                }
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
                layout_id
            }
            _ => panic!("cannot measure after painting"),
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
