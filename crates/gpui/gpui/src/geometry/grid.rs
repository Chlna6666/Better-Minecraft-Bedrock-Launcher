use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::ops::Range;
use taffy::prelude::{TaffyGridLine, TaffyGridSpan};

/// A location in a grid layout.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, JsonSchema, Default)]
pub struct GridLocation {
    /// The rows this item uses within the grid.
    pub row: Range<GridPlacement>,
    /// The columns this item uses within the grid.
    pub column: Range<GridPlacement>,
}

/// The placement of an item within a grid layout's column or row.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, JsonSchema, Default)]
pub enum GridPlacement {
    /// The grid line index to place this item.
    Line(i16),
    /// The number of grid lines to span.
    Span(u16),
    /// Automatically determine the placement, equivalent to Span(1)
    #[default]
    Auto,
}

impl From<GridPlacement> for taffy::GridPlacement {
    fn from(placement: GridPlacement) -> Self {
        match placement {
            GridPlacement::Line(index) => taffy::GridPlacement::from_line_index(index),
            GridPlacement::Span(span) => taffy::GridPlacement::from_span(span),
            GridPlacement::Auto => taffy::GridPlacement::Auto,
        }
    }
}
