use crate::{Div, Styled, div, relative};

/// Build a vertical flex container.
pub fn v_stack() -> Div {
    div().flex().flex_col()
}

/// Build a horizontal flex container.
pub fn h_stack() -> Div {
    div().flex().flex_row()
}

/// Build a container centered on both axes.
pub fn center() -> Div {
    div().flex().items_center().justify_center()
}

/// Build an absolutely positioned fill container.
pub fn absolute_fill() -> Div {
    div().absolute().inset_0().size_full()
}

/// Build a relatively sized fill container.
pub fn relative_fill() -> Div {
    div().relative().w(relative(1.0)).h(relative(1.0))
}
