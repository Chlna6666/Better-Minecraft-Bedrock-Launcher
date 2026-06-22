use gpui::{Div, ElementId, InteractiveElement, Stateful, StatefulInteractiveElement};
use std::panic::Location;

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
