use crate::{
    AbsoluteLength, BorderStyle, Corners, CornersRefinement, CursorStyle, DefiniteLength, Edges,
    EdgesRefinement, GridLocation, Hsla, Length, Point, PointRefinement, Size, SizeRefinement,
    TextStyleRefinement, TransitionStyle,
};
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Fill, paint::BoxShadow, paint::Visibility};

/// The normalized point within an element used as the origin for visual transforms.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TransformOrigin {
    /// Horizontal position, where `0.0` is the left edge and `1.0` is the right edge.
    pub x: f32,
    /// Vertical position, where `0.0` is the top edge and `1.0` is the bottom edge.
    pub y: f32,
}

impl TransformOrigin {
    /// Creates a normalized transform origin.
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// The center of an element's final layout bounds.
    pub const CENTER: Self = Self::new(0.5, 0.5);

    pub(crate) fn resolve(
        self,
        bounds: crate::Bounds<crate::Pixels>,
    ) -> crate::Point<crate::Pixels> {
        let normalize = |value: f32| {
            if value.is_finite() {
                value.clamp(0.0, 1.0)
            } else {
                0.5
            }
        };
        crate::point(
            bounds.origin.x + bounds.size.width * normalize(self.x),
            bounds.origin.y + bounds.size.height * normalize(self.y),
        )
    }
}

#[cfg(test)]
mod transform_origin_tests {
    use super::*;
    use crate::{LayoutStyle, bounds, point, px, size};

    #[test]
    fn transform_origin_resolves_from_final_layout_bounds() {
        let bounds = bounds(point(px(10.0), px(20.0)), size(px(200.0), px(100.0)));

        assert_eq!(
            TransformOrigin::CENTER.resolve(bounds),
            point(px(110.0), px(70.0))
        );
    }

    #[test]
    fn transform_origin_clamps_invalid_normalized_coordinates() {
        let bounds = bounds(point(px(10.0), px(20.0)), size(px(200.0), px(100.0)));

        assert_eq!(
            TransformOrigin::new(-1.0, f32::NAN).resolve(bounds),
            point(px(10.0), px(70.0))
        );
    }

    #[test]
    fn scale_does_not_change_layout_style() {
        let scaled = Style {
            scale: 0.5,
            ..Style::default()
        };

        assert_eq!(
            LayoutStyle::from(&scaled),
            LayoutStyle::from(&Style::default())
        );
    }
}

impl Default for TransformOrigin {
    fn default() -> Self {
        Self::CENTER
    }
}

/// Used to control how child nodes are aligned.
/// For Flexbox it controls alignment in the cross axis
/// For Grid it controls alignment in the block axis
///
/// [MDN](https://developer.mozilla.org/en-US/docs/Web/CSS/align-items)
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub enum AlignItems {
    /// Items are packed toward the start of the axis
    Start,
    /// Items are packed toward the end of the axis
    End,
    /// Items are packed towards the flex-relative start of the axis.
    FlexStart,
    /// Items are packed towards the flex-relative end of the axis.
    FlexEnd,
    /// Items are packed along the center of the cross axis
    Center,
    /// Items are aligned such as their baselines align
    Baseline,
    /// Stretch to fill the container
    Stretch,
}

/// Used to control how child nodes are aligned.
pub type JustifyItems = AlignItems;
/// Used to control how the specified nodes is aligned.
pub type AlignSelf = AlignItems;
/// Used to control how the specified nodes is aligned.
pub type JustifySelf = AlignItems;

/// Sets the distribution of space between and around content items.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize, JsonSchema)]
pub enum AlignContent {
    /// Items are packed toward the start of the axis
    Start,
    /// Items are packed toward the end of the axis
    End,
    /// Items are packed towards the flex-relative start of the axis.
    FlexStart,
    /// Items are packed towards the flex-relative end of the axis.
    FlexEnd,
    /// Items are centered around the middle of the axis
    Center,
    /// Items are stretched to fill the container
    Stretch,
    /// The first and last items are aligned flush with the edges of the container.
    SpaceBetween,
    /// The gap between the first and last items is the same as the gap between items.
    SpaceEvenly,
    /// The gap between the first and last items is half the gap between items.
    SpaceAround,
}

/// Sets the distribution of space between and around content items.
pub type JustifyContent = AlignContent;

/// Sets the layout used for the children of this node.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub enum Display {
    /// The children will follow the block layout algorithm
    Block,
    /// The children will follow the flexbox layout algorithm
    #[default]
    Flex,
    /// The children will follow the CSS Grid layout algorithm
    Grid,
    /// The children will not be laid out, and will follow absolute positioning
    None,
}

/// Controls whether flex items are forced onto one line or can wrap onto multiple lines.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub enum FlexWrap {
    /// Items will not wrap and stay on a single line
    #[default]
    NoWrap,
    /// Items will wrap according to this item's [`FlexDirection`]
    Wrap,
    /// Items will wrap in the opposite direction to this item's [`FlexDirection`]
    WrapReverse,
}

/// The direction of the flexbox layout main axis.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub enum FlexDirection {
    /// Defines +x as the main axis
    #[default]
    Row,
    /// Defines +y as the main axis
    Column,
    /// Defines -x as the main axis
    RowReverse,
    /// Defines -y as the main axis
    ColumnReverse,
}

/// How children overflowing their container should affect layout.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub enum Overflow {
    /// The automatic minimum size should be based on the size of its content.
    #[default]
    Visible,
    /// Overflow should not contribute to the scroll region of its parent.
    Clip,
    /// The automatic minimum size should be `0`.
    Hidden,
    /// The automatic minimum size should be `0` and reserve space for a scrollbar.
    Scroll,
}

/// The positioning strategy for this item.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub enum Position {
    /// The offset is computed relative to the final position given by the layout algorithm.
    #[default]
    Relative,
    /// The offset is computed relative to this item's closest positioned ancestor.
    Absolute,
}

/// The CSS styling that can be applied to an element via the `Styled` trait
#[derive(Clone, Refineable, Debug)]
#[refineable(Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Style {
    /// What layout strategy should be used?
    pub display: Display,

    /// Should the element be painted on screen?
    pub visibility: Visibility,

    // Overflow properties
    /// How children overflowing their container should affect layout
    #[refineable]
    pub overflow: Point<Overflow>,
    /// How much space (in points) should be reserved for the scrollbars of `Overflow::Scroll` and `Overflow::Auto` nodes.
    pub scrollbar_width: AbsoluteLength,
    /// Whether both x and y axis should be scrollable at the same time.
    pub allow_concurrent_scroll: bool,
    /// Whether scrolling should be restricted to the axis indicated by the mouse wheel.
    ///
    /// This means that:
    /// - The mouse wheel alone will only ever scroll the Y axis.
    /// - Holding `Shift` and using the mouse wheel will scroll the X axis.
    ///
    /// ## Motivation
    ///
    /// On the web when scrolling with the mouse wheel, scrolling up and down will always scroll the Y axis, even when
    /// the mouse is over a horizontally-scrollable element.
    ///
    /// The only way to scroll horizontally is to hold down `Shift` while scrolling, which then changes the scroll axis
    /// to the X axis.
    ///
    /// Currently, GPUI operates differently from the web in that it will scroll an element in either the X or Y axis
    /// when scrolling with just the mouse wheel. This causes problems when scrolling in a vertical list that contains
    /// horizontally-scrollable elements, as when you get to the horizontally-scrollable elements the scroll will be
    /// hijacked.
    ///
    /// Ideally we would match the web's behavior and not have a need for this, but right now we're adding this opt-in
    /// style property to limit the potential blast radius.
    pub restrict_scroll_to_axis: bool,

    // Position properties
    /// What should the `position` value of this struct use as a base offset?
    pub position: Position,
    /// How should the position of this element be tweaked relative to the layout defined?
    #[refineable]
    pub inset: Edges<Length>,

    // Size properties
    /// Sets the initial size of the item
    #[refineable]
    pub size: Size<Length>,
    /// Controls the minimum size of the item
    #[refineable]
    pub min_size: Size<Length>,
    /// Controls the maximum size of the item
    #[refineable]
    pub max_size: Size<Length>,
    /// Sets the preferred aspect ratio for the item. The ratio is calculated as width divided by height.
    pub aspect_ratio: Option<f32>,

    /// Makes this node a transparent percentage containing block for its single in-flow child.
    ///
    /// On an auto-sized axis, a percentage-sized child normally creates an indefinite-size
    /// dependency (the parent depends on the child while the child depends on the parent).
    /// When enabled, GPUI promotes the child's percentage to this wrapper and rewrites the
    /// child to 100% on that axis. This is intended for paint-only wrappers used by animations,
    /// opacity, transforms, and modal layers.
    pub percentage_passthrough: bool,

    // Spacing Properties
    /// How large should the margin be on each side?
    #[refineable]
    pub margin: Edges<Length>,
    /// How large should the padding be on each side?
    #[refineable]
    pub padding: Edges<DefiniteLength>,
    /// How large should the border be on each side?
    #[refineable]
    pub border_widths: Edges<AbsoluteLength>,

    // Alignment properties
    /// How this node's children aligned in the cross/block axis?
    pub align_items: Option<AlignItems>,
    /// How this node should be aligned in the cross/block axis. Falls back to the parents [`AlignItems`] if not set
    pub align_self: Option<AlignSelf>,
    /// How should content contained within this item be aligned in the cross/block axis
    pub align_content: Option<AlignContent>,
    /// How should contained within this item be aligned in the main/inline axis
    pub justify_content: Option<JustifyContent>,
    /// How large should the gaps between items in a flex container be?
    #[refineable]
    pub gap: Size<DefiniteLength>,

    // Flexbox properties
    /// Which direction does the main axis flow in?
    pub flex_direction: FlexDirection,
    /// Should elements wrap, or stay in a single line?
    pub flex_wrap: FlexWrap,
    /// Sets the initial main axis size of the item
    pub flex_basis: Length,
    /// The relative rate at which this item grows when it is expanding to fill space, 0.0 is the default value, and this value must be positive.
    pub flex_grow: f32,
    /// The relative rate at which this item shrinks when it is contracting to fit into space, 1.0 is the default value, and this value must be positive.
    pub flex_shrink: f32,

    /// The fill color of this element
    pub background: Option<Fill>,

    /// The border color of this element
    pub border_color: Option<Hsla>,

    /// The border style of this element
    pub border_style: BorderStyle,

    /// The radius of the corners of this element
    #[refineable]
    pub corner_radii: Corners<AbsoluteLength>,

    /// Box shadow of the element
    pub box_shadow: Vec<BoxShadow>,

    /// The text style of this element
    pub text: TextStyleRefinement,

    /// The mouse cursor style shown when the mouse pointer is over an element.
    pub mouse_cursor: Option<CursorStyle>,

    /// The opacity of this element
    pub opacity: Option<f32>,

    /// Paint-time scale applied to this element and its descendants without changing layout.
    pub scale: f32,

    /// Origin for paint-time transforms, resolved from the element's final layout bounds.
    pub transform_origin: TransformOrigin,

    /// Transition metadata for state-change animations.
    pub transition: Option<TransitionStyle>,

    /// The grid columns of this element
    /// Equivalent to the Tailwind `grid-cols-<number>`
    pub grid_cols: Option<u16>,

    /// The row span of this element
    /// Equivalent to the Tailwind `grid-rows-<number>`
    pub grid_rows: Option<u16>,

    /// The grid location of this element
    pub grid_location: Option<GridLocation>,

    /// Whether to draw a red debugging outline around this element
    #[cfg(debug_assertions)]
    pub debug: bool,

    /// Whether to draw a red debugging outline around this element and all of its conforming children
    #[cfg(debug_assertions)]
    pub debug_below: bool,
}

impl Default for Style {
    fn default() -> Self {
        Style {
            display: Display::Block,
            visibility: Visibility::Visible,
            overflow: Point {
                x: Overflow::Visible,
                y: Overflow::Visible,
            },
            allow_concurrent_scroll: false,
            restrict_scroll_to_axis: false,
            scrollbar_width: AbsoluteLength::default(),
            position: Position::Relative,
            inset: Edges::auto(),
            margin: Edges::<Length>::zero(),
            padding: Edges::<DefiniteLength>::zero(),
            border_widths: Edges::<AbsoluteLength>::zero(),
            size: Size::auto(),
            min_size: Size::auto(),
            max_size: Size::auto(),
            aspect_ratio: None,
            percentage_passthrough: false,
            gap: Size::default(),
            // Alignment
            align_items: None,
            align_self: None,
            align_content: None,
            justify_content: None,
            // Flexbox
            flex_direction: FlexDirection::Row,
            flex_wrap: FlexWrap::NoWrap,
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: Length::Auto,
            background: None,
            border_color: None,
            border_style: BorderStyle::default(),
            corner_radii: Corners::default(),
            box_shadow: Default::default(),
            text: TextStyleRefinement::default(),
            mouse_cursor: None,
            opacity: None,
            scale: 1.0,
            transform_origin: TransformOrigin::default(),
            transition: None,
            grid_rows: None,
            grid_cols: None,
            grid_location: None,

            #[cfg(debug_assertions)]
            debug: false,
            #[cfg(debug_assertions)]
            debug_below: false,
        }
    }
}
