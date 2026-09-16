use crate::{
    AnyElement, App, Bounds, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, SceneAnimationId, TextStyleRefinement, TransitionProperty, Window,
};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy)]
struct DeferredSceneAnimationContext {
    animation_id: SceneAnimationId,
    property: TransitionProperty,
    text_raster_scale: f32,
}

#[derive(Clone)]
struct DeferredInheritedContext {
    text_style_stack: Vec<TextStyleRefinement>,
    rem_size: Pixels,
    element_opacity: f32,
    scene_animation: Option<DeferredSceneAnimationContext>,
}

struct DeferredContextElement {
    child: Option<AnyElement>,
    context: Rc<RefCell<Option<DeferredInheritedContext>>>,
}

fn with_deferred_inherited_context<R>(
    context: &Rc<RefCell<Option<DeferredInheritedContext>>>,
    window: &mut Window,
    f: impl FnOnce(&mut Window) -> R,
) -> R {
    let Some(context) = context.borrow().clone() else {
        return f(window);
    };

    // Deferred children leave their original ancestor stack before prepaint/paint. Restore the
    // inherited values that are not part of Window::defer_draw's frame descriptor so text paint,
    // rem-dependent styling, cumulative opacity, and direct renderer-owned animation bindings
    // remain identical to inline traversal.
    let previous_text_style_stack =
        std::mem::replace(&mut window.text_style_stack, context.text_style_stack);
    let previous_element_opacity =
        std::mem::replace(&mut window.element_opacity, context.element_opacity);
    let result = window.with_rem_size(Some(context.rem_size), |window| {
        if let Some(scene_animation) = context.scene_animation {
            window.with_scene_animation(
                scene_animation.animation_id,
                scene_animation.property,
                scene_animation.text_raster_scale,
                f,
            )
        } else {
            f(window)
        }
    });
    window.element_opacity = previous_element_opacity;
    window.text_style_stack = previous_text_style_stack;
    result
}

impl IntoElement for DeferredContextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for DeferredContextElement {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut child = self
            .child
            .take()
            .expect("deferred context child should only be laid out once");
        let layout_id = child.request_layout(window, cx);
        (layout_id, child)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        with_deferred_inherited_context(&self.context, window, |window| {
            child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        child: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        with_deferred_inherited_context(&self.context, window, |window| {
            child.paint(window, cx)
        });
    }
}

/// Builds a `Deferred` element, which delays the layout and paint of its child.
pub fn deferred(child: impl IntoElement) -> Deferred {
    let context = Rc::new(RefCell::new(None));
    let child = DeferredContextElement {
        child: Some(child.into_any_element()),
        context: context.clone(),
    }
    .into_any_element();

    Deferred {
        child: Some(child),
        context,
        priority: 0,
    }
}

/// An element which delays the painting of its child until after all of
/// its ancestors, while keeping its layout as part of the current element tree.
pub struct Deferred {
    child: Option<AnyElement>,
    context: Rc<RefCell<Option<DeferredInheritedContext>>>,
    priority: usize,
}

impl Deferred {
    /// Sets the `priority` value of the `deferred` element, which
    /// determines the drawing order relative to other deferred elements,
    /// with higher values being drawn on top.
    pub fn with_priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }
}

impl Element for Deferred {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<crate::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let layout_id = self.child.as_mut().unwrap().request_layout(window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        *self.context.borrow_mut() = Some(DeferredInheritedContext {
            text_style_stack: window.text_style_stack.clone(),
            rem_size: window.rem_size(),
            element_opacity: window.element_opacity(),
            // Renderer-owned animation ownership is established during paint, after the deferred
            // child has already been registered. `Deferred::paint` fills this slot while it is
            // still traversed under the original ancestor animation context.
            scene_animation: None,
        });

        let child = self.child.take().unwrap();
        let element_offset = window.element_offset();
        window.defer_draw(child, element_offset, self.priority)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let scene_animation = window.scene_animation.map(|(animation_id, property)| {
            DeferredSceneAnimationContext {
                animation_id,
                property,
                text_raster_scale: window.scene_text_raster_scale(),
            }
        });
        if let Some(context) = self.context.borrow_mut().as_mut() {
            context.scene_animation = scene_animation;
        }
    }
}

impl IntoElement for Deferred {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Deferred {
    /// Sets a priority for the element. A higher priority conceptually means painting the element
    /// on top of deferred draws with a lower priority (i.e. closer to the viewer).
    pub fn priority(mut self, priority: usize) -> Self {
        self.priority = priority;
        self
    }
}
