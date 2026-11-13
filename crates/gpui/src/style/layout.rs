use crate::{AbsoluteLength, DefiniteLength, Edges, GridLocation, Length, Point, Size};

use super::{
    AlignContent, AlignItems, AlignSelf, Display, FlexDirection, FlexWrap, JustifyContent,
    Overflow, Position,
};

/// A layout-only subset of [`super::Style`].
///
/// This type is currently a structural placeholder for the later vNext
/// performance work. The active layout fingerprint implementation still hashes
/// directly from `Style`.
#[derive(Clone, Debug, PartialEq)]
pub struct LayoutStyle {
    /// Layout display strategy.
    pub display: Display,
    /// Overflow behavior for each axis.
    pub overflow: Point<Overflow>,
    /// Reserved scrollbar width for scrollable layout nodes.
    pub scrollbar_width: AbsoluteLength,
    /// Whether both axes may scroll concurrently.
    pub allow_concurrent_scroll: bool,
    /// Whether wheel scrolling is constrained to the indicated axis.
    pub restrict_scroll_to_axis: bool,
    /// Layout positioning mode.
    pub position: Position,
    /// Layout inset values.
    pub inset: Edges<Length>,
    /// Preferred size.
    pub size: Size<Length>,
    /// Minimum size.
    pub min_size: Size<Length>,
    /// Maximum size.
    pub max_size: Size<Length>,
    /// Preferred aspect ratio.
    pub aspect_ratio: Option<f32>,
    /// Whether a single child's percentage size is forwarded through this auto-sized wrapper.
    pub percentage_passthrough: bool,
    /// Margin values.
    pub margin: Edges<Length>,
    /// Padding values.
    pub padding: Edges<DefiniteLength>,
    /// Border width values.
    pub border_widths: Edges<AbsoluteLength>,
    /// Cross-axis item alignment.
    pub align_items: Option<AlignItems>,
    /// Per-item cross-axis alignment override.
    pub align_self: Option<AlignSelf>,
    /// Multi-line content alignment.
    pub align_content: Option<AlignContent>,
    /// Main-axis content alignment.
    pub justify_content: Option<JustifyContent>,
    /// Inter-item gap values.
    pub gap: Size<DefiniteLength>,
    /// Flex main axis direction.
    pub flex_direction: FlexDirection,
    /// Flex wrapping behavior.
    pub flex_wrap: FlexWrap,
    /// Flex basis.
    pub flex_basis: Length,
    /// Flex grow factor.
    pub flex_grow: f32,
    /// Flex shrink factor.
    pub flex_shrink: f32,
    /// Grid row repeat count.
    pub grid_rows: Option<u16>,
    /// Grid column repeat count.
    pub grid_cols: Option<u16>,
    /// Grid placement for the node.
    pub grid_location: Option<GridLocation>,
}

impl From<&super::Style> for LayoutStyle {
    fn from(style: &super::Style) -> Self {
        Self {
            display: style.display,
            overflow: style.overflow,
            scrollbar_width: style.scrollbar_width,
            allow_concurrent_scroll: style.allow_concurrent_scroll,
            restrict_scroll_to_axis: style.restrict_scroll_to_axis,
            position: style.position,
            inset: style.inset,
            size: style.size,
            min_size: style.min_size,
            max_size: style.max_size,
            aspect_ratio: style.aspect_ratio,
            percentage_passthrough: style.percentage_passthrough,
            margin: style.margin,
            padding: style.padding,
            border_widths: style.border_widths,
            align_items: style.align_items,
            align_self: style.align_self,
            align_content: style.align_content,
            justify_content: style.justify_content,
            gap: style.gap,
            flex_direction: style.flex_direction,
            flex_wrap: style.flex_wrap,
            flex_basis: style.flex_basis,
            flex_grow: style.flex_grow,
            flex_shrink: style.flex_shrink,
            grid_rows: style.grid_rows,
            grid_cols: style.grid_cols,
            grid_location: style.grid_location.clone(),
        }
    }
}
