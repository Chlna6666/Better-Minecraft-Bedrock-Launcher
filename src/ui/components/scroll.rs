use crate::ui::theme::colors::ThemeColors;
use gpui::{
    AnyElement, App, Bounds, DispatchPhase, Div, ElementId, Entity, Hsla, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    ParentElement, Pixels, RenderOnce, ScrollHandle, Stateful, StatefulInteractiveElement, Styled,
    Window, canvas, div, fill, point, px, size,
};
use std::panic::Location;
use std::rc::Rc;

type ScrollCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// A vertical scrollbar backed by the same handle as its scroll container.
#[derive(IntoElement)]
pub(crate) struct Scrollbar {
    id: ElementId,
    handle: ScrollHandle,
    colors: ThemeColors,
    on_scroll: Option<ScrollCallback>,
}

#[derive(Default)]
struct ScrollbarState {
    geometry: Option<ScrollbarGeometry>,
    drag_offset: Option<Pixels>,
}

#[derive(Clone, Copy)]
struct ScrollbarGeometry {
    track: Bounds<Pixels>,
    thumb: Bounds<Pixels>,
    max_scroll: Pixels,
}

impl ScrollbarGeometry {
    fn new(
        bounds: Bounds<Pixels>,
        viewport: Pixels,
        max_scroll: Pixels,
        offset: Pixels,
    ) -> Option<Self> {
        if viewport <= px(0.) || max_scroll <= px(0.) || bounds.size.height <= px(0.) {
            return None;
        }
        let thumb_height = (bounds.size.height * (viewport / (viewport + max_scroll)))
            .max(px(24.))
            .min(bounds.size.height);
        let travel = bounds.size.height - thumb_height;
        let progress = (-offset / max_scroll).clamp(0., 1.);
        Some(Self {
            track: bounds,
            thumb: Bounds::new(
                point(bounds.left() + px(3.), bounds.top() + travel * progress),
                size((bounds.size.width - px(6.)).max(px(0.)), thumb_height),
            ),
            max_scroll,
        })
    }

    fn offset_at(self, pointer_y: Pixels, drag_offset: Pixels) -> Pixels {
        let travel = self.track.size.height - self.thumb.size.height;
        if travel <= px(0.) {
            return px(0.);
        }
        let progress = ((pointer_y - self.track.top() - drag_offset) / travel).clamp(0., 1.);
        -self.max_scroll * progress
    }
}

impl Scrollbar {
    pub(crate) fn new(
        id: impl Into<ElementId>,
        handle: &ScrollHandle,
        colors: &ThemeColors,
    ) -> Self {
        Self {
            id: id.into(),
            handle: handle.clone(),
            colors: *colors,
            on_scroll: None,
        }
    }

    pub(crate) fn on_scroll(mut self, callback: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_scroll = Some(Rc::new(callback));
        self
    }

    fn set_offset(&self, offset: Pixels, window: &mut Window, cx: &mut App) {
        let previous = self.handle.offset();
        if previous.y == offset {
            return;
        }
        self.handle.set_offset(point(previous.x, offset));
        window.refresh();
        if let Some(on_scroll) = &self.on_scroll {
            on_scroll(window, cx);
        }
    }

    fn start_drag(
        &self,
        event: &MouseDownEvent,
        state: &Entity<ScrollbarState>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(geometry) = state.read(cx).geometry else {
            return;
        };
        let drag_offset = if geometry.thumb.contains(&event.position) {
            event.position.y - geometry.thumb.top()
        } else {
            geometry.thumb.size.height / 2.
        };
        state.update(cx, |state, _| state.drag_offset = Some(drag_offset));
        self.set_offset(
            geometry.offset_at(event.position.y, drag_offset),
            window,
            cx,
        );
        cx.stop_propagation();
    }

    fn bind_drag(self: &Rc<Self>, state: &Entity<ScrollbarState>, window: &mut Window) {
        let scrollbar = self.clone();
        let drag_state = state.clone();
        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let state = drag_state.read(cx);
            let (Some(geometry), Some(drag_offset)) = (state.geometry, state.drag_offset) else {
                return;
            };
            if !event.dragging() {
                drag_state.update(cx, |state, _| state.drag_offset = None);
                return;
            }
            scrollbar.set_offset(
                geometry.offset_at(event.position.y, drag_offset),
                window,
                cx,
            );
            cx.stop_propagation();
        });
        let drag_state = state.clone();
        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
            if phase == DispatchPhase::Capture
                && event.button == MouseButton::Left
                && drag_state.read(cx).drag_offset.is_some()
            {
                drag_state.update(cx, |state, _| state.drag_offset = None);
                cx.stop_propagation();
            }
        });
    }

    fn paint(&self, geometry: ScrollbarGeometry, window: &mut Window) {
        let track = Bounds::new(
            point(geometry.track.left() + px(5.), geometry.track.top()),
            size(px(2.), geometry.track.size.height),
        );
        window.paint_quad(fill(
            track,
            Hsla {
                a: 0.35,
                ..self.colors.border
            },
        ));
        window.paint_quad(PaintQuad {
            corner_radii: px(3.).into(),
            ..fill(geometry.thumb, self.colors.text_muted)
        });
    }

    fn track(self: &Rc<Self>, state: &Entity<ScrollbarState>) -> AnyElement {
        let measure_scrollbar = self.clone();
        let measure_state = state.clone();
        let paint_scrollbar = self.clone();
        let paint_state = state.clone();
        canvas(
            move |bounds, _, cx| {
                let handle = &measure_scrollbar.handle;
                let geometry = ScrollbarGeometry::new(
                    bounds,
                    handle.bounds().size.height,
                    handle.max_offset().height,
                    handle.offset().y,
                );
                measure_state.update(cx, |state, _| {
                    state.geometry = geometry;
                    if geometry.is_none() {
                        state.drag_offset = None;
                    }
                });
                geometry
            },
            move |_, geometry, window, _| {
                if let Some(geometry) = geometry {
                    paint_scrollbar.paint(geometry, window);
                    paint_scrollbar.bind_drag(&paint_state, window);
                }
            },
        )
        .size_full()
        .into_any_element()
    }
}

impl RenderOnce for Scrollbar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(self.id.clone(), cx, |_, _| ScrollbarState::default());
        let scrollbar = Rc::new(self);
        let track = scrollbar.track(&state);
        let wheel_scrollbar = scrollbar.clone();
        div()
            .id(scrollbar.id.clone())
            .flex_none()
            .w(px(12.))
            .h_full()
            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                scrollbar.start_drag(event, &state, window, cx);
            })
            .on_scroll_wheel(move |event, window, cx| {
                let offset = wheel_scrollbar.handle.offset().y
                    + event.delta.pixel_delta(window.line_height()).y;
                let max_scroll = wheel_scrollbar.handle.max_offset().height;
                wheel_scrollbar.set_offset(offset.clamp(-max_scroll, px(0.)), window, cx);
                cx.stop_propagation();
            })
            .child(track)
    }
}

pub trait ScrollableElement {
    type Output;

    fn overflow_y_scrollbar(self) -> Self::Output;

    fn overflow_x_scrollbar(self) -> Self::Output;
}

impl ScrollableElement for Div {
    type Output = Stateful<Div>;

    #[track_caller]
    fn overflow_y_scrollbar(self) -> Self::Output {
        self.id(ElementId::CodeLocation(*Location::caller()))
            .overflow_y_scroll()
    }

    #[track_caller]
    fn overflow_x_scrollbar(self) -> Self::Output {
        self.id(ElementId::CodeLocation(*Location::caller()))
            .overflow_x_scroll()
    }
}

impl ScrollableElement for Stateful<Div> {
    type Output = Self;

    fn overflow_y_scrollbar(self) -> Self::Output {
        self.overflow_y_scroll()
    }

    fn overflow_x_scrollbar(self) -> Self::Output {
        self.overflow_x_scroll()
    }
}

#[cfg(test)]
mod tests {
    use super::ScrollbarGeometry;
    use gpui::{Bounds, Pixels, point, px, size};

    fn track() -> Bounds<Pixels> {
        Bounds::new(point(px(0.), px(20.)), size(px(12.), px(200.)))
    }

    #[test]
    fn scrollbar_hides_when_content_fits_or_layout_is_empty() {
        assert!(ScrollbarGeometry::new(track(), px(200.), px(0.), px(0.)).is_none());
        assert!(ScrollbarGeometry::new(track(), px(0.), px(100.), px(0.)).is_none());
    }

    #[test]
    fn thumb_tracks_scroll_extent_and_clamps_pointer_outside_track() {
        let geometry = ScrollbarGeometry::new(track(), px(200.), px(600.), px(-300.)).unwrap();
        assert_eq!(geometry.thumb.size.height, px(50.));
        assert_eq!(geometry.thumb.top(), px(95.));
        assert_eq!(geometry.offset_at(px(120.), px(25.)), px(-300.));
        assert_eq!(geometry.offset_at(px(-100.), px(25.)), px(0.));
        assert_eq!(geometry.offset_at(px(400.), px(25.)), px(-600.));
    }

    #[test]
    fn minimum_thumb_still_reaches_both_ends() {
        let geometry = ScrollbarGeometry::new(track(), px(200.), px(20000.), px(-20000.)).unwrap();
        assert_eq!(geometry.thumb.size.height, px(24.));
        assert_eq!(geometry.thumb.bottom(), track().bottom());
        assert_eq!(
            geometry.offset_at(geometry.thumb.top(), px(0.)),
            px(-20000.)
        );
    }
}
