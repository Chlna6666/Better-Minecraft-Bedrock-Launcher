use crate::{
    App, Bounds, Context, ElementId, FluentBuilder, InspectorElementId, LayoutId, Pixels, Window,
};
use std::panic;

use super::{AnyElement, GlobalElementId};

/// Describes how the generic retained reconciler may treat an [`Element`] boundary.
///
/// Most elements are safe to replay as part of an ancestor retained subtree. Elements that own
/// frame-local cache metadata must execute their own lifecycle every frame so they can rebase that
/// metadata before reusing their internal subtree. `NonReplayable` is reserved for elements whose
/// lifecycle must always execute and which must also prevent an ancestor from skipping across them.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedReplayCapability {
    Normal,
    OwnsFrameLocalCacheBoundary,
    NonReplayable,
}

impl RetainedReplayCapability {
    #[inline]
    pub(crate) const fn allows_outer_replay(self) -> bool {
        matches!(self, Self::Normal)
    }
}

/// Implemented by types that participate in laying out and painting the contents of a window.
/// Elements form a tree and are laid out according to web-based layout rules, as implemented by Taffy.
/// You can create custom elements by implementing this trait, see the module-level documentation
/// for more details.
pub trait Element: 'static + IntoElement {
    /// The type of state returned from [`Element::request_layout`]. A mutable reference to this state is subsequently
    /// provided to [`Element::prepaint`] and [`Element::paint`].
    type RequestLayoutState: 'static;

    /// The type of state returned from [`Element::prepaint`]. A mutable reference to this state is subsequently
    /// provided to [`Element::paint`].
    type PrepaintState: 'static;

    /// Retained-replay contract for this element type.
    ///
    /// Elements that persist absolute frame-local indices, GPU target slots, or similar metadata in
    /// element state must use [`RetainedReplayCapability::OwnsFrameLocalCacheBoundary`] so the outer
    /// reconciler cannot skip the lifecycle that rebases that metadata.
    #[doc(hidden)]
    const RETAINED_REPLAY_CAPABILITY: RetainedReplayCapability = RetainedReplayCapability::Normal;

    /// If this element has a unique identifier, return it here. This is used to track elements across frames, and
    /// will cause a GlobalElementId to be passed to the request_layout, prepaint, and paint methods.
    ///
    /// The global id can in turn be used to access state that's connected to an element with the same id across
    /// frames. This id must be unique among children of the first containing element with an id.
    fn id(&self) -> Option<ElementId>;

    /// Source location where this element was constructed, used to disambiguate elements in the
    /// inspector and navigate to their source code.
    fn source_location(&self) -> Option<&'static panic::Location<'static>>;

    /// Before an element can be painted, we need to know where it's going to be and how big it is.
    /// Use this method to request a layout from Taffy and initialize the element's state.
    fn request_layout(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState);

    /// After laying out an element, we need to commit its bounds to the current frame for hitbox
    /// purposes. The state argument is the same state that was returned from [`Element::request_layout()`].
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState;

    /// Once layout has been completed, this method will be called to paint the element to the screen.
    /// The state argument is the same state that was returned from [`Element::request_layout()`].
    fn paint(
        &mut self,
        id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    );

    /// Convert this element into a dynamically-typed [`AnyElement`].
    ///
    /// The erasure call site is retained separately from the element's inspector source location.
    /// This gives reconciliation a stable parent-facing identity even when reusable helpers construct
    /// the same concrete element type from one internal source line.
    #[track_caller]
    fn into_any(self) -> AnyElement {
        AnyElement::new(self).with_retained_auto_mount(panic::Location::caller())
    }
}

/// Implemented by any type that can be converted into an element.
pub trait IntoElement: Sized {
    /// The specific type of element into which the implementing type is converted.
    /// Useful for converting other types into elements automatically, like Strings
    type Element: Element;

    /// Convert self into a type that implements [`Element`].
    fn into_element(self) -> Self::Element;

    /// Convert self into a dynamically-typed [`AnyElement`].
    #[track_caller]
    fn into_any_element(self) -> AnyElement {
        self.into_element().into_any()
    }
}

impl<T: IntoElement> FluentBuilder for T {}

/// An object that can be drawn to the screen. This is the trait that distinguishes "views" from
/// other entities. Views are `Entity`'s which `impl Render` and drawn to the screen.
pub trait Render: 'static + Sized {
    /// Render this view into an element tree.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement;
}

/// You can derive [`IntoElement`] on any type that implements this trait.
/// It is used to construct reusable `components` out of plain data. Think of
/// components as a recipe for a certain pattern of elements. RenderOnce allows
/// you to invoke this pattern, without breaking the fluent builder pattern.
pub trait RenderOnce: 'static {
    /// Render this component or element. Note that this method takes ownership of self, as compared
    /// to [`Render::render`] which takes a mutable reference.
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement;
}
