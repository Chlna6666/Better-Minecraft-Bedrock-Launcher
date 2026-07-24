use crate::{
    AbsoluteLength, DefiniteLength, Edges, LayoutStyle, Length, Pixels, Point, Size, Style,
    record_layout_conversion,
};
use std::{fmt::Debug, ops::Range};
use taffy::geometry::{Point as TaffyPoint, Rect as TaffyRect, Size as TaffySize};

pub(super) trait ToTaffy<Output> {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Output;
}

impl From<crate::AlignItems> for taffy::style::AlignItems {
    fn from(value: crate::AlignItems) -> Self {
        match value {
            crate::AlignItems::Start => Self::START,
            crate::AlignItems::End => Self::END,
            crate::AlignItems::FlexStart => Self::FLEX_START,
            crate::AlignItems::FlexEnd => Self::FLEX_END,
            crate::AlignItems::Center => Self::CENTER,
            crate::AlignItems::Baseline => Self::BASELINE,
            crate::AlignItems::Stretch => Self::STRETCH,
        }
    }
}

impl From<crate::AlignContent> for taffy::style::AlignContent {
    fn from(value: crate::AlignContent) -> Self {
        match value {
            crate::AlignContent::Start => Self::START,
            crate::AlignContent::End => Self::END,
            crate::AlignContent::FlexStart => Self::FLEX_START,
            crate::AlignContent::FlexEnd => Self::FLEX_END,
            crate::AlignContent::Center => Self::CENTER,
            crate::AlignContent::Stretch => Self::STRETCH,
            crate::AlignContent::SpaceBetween => Self::SPACE_BETWEEN,
            crate::AlignContent::SpaceEvenly => Self::SPACE_EVENLY,
            crate::AlignContent::SpaceAround => Self::SPACE_AROUND,
        }
    }
}

impl From<crate::Display> for taffy::style::Display {
    fn from(value: crate::Display) -> Self {
        match value {
            crate::Display::Block => Self::Block,
            crate::Display::Flex => Self::Flex,
            crate::Display::Grid => Self::Grid,
            crate::Display::None => Self::None,
        }
    }
}

impl From<crate::FlexWrap> for taffy::style::FlexWrap {
    fn from(value: crate::FlexWrap) -> Self {
        match value {
            crate::FlexWrap::NoWrap => Self::NoWrap,
            crate::FlexWrap::Wrap => Self::Wrap,
            crate::FlexWrap::WrapReverse => Self::WrapReverse,
        }
    }
}

impl From<crate::FlexDirection> for taffy::style::FlexDirection {
    fn from(value: crate::FlexDirection) -> Self {
        match value {
            crate::FlexDirection::Row => Self::Row,
            crate::FlexDirection::Column => Self::Column,
            crate::FlexDirection::RowReverse => Self::RowReverse,
            crate::FlexDirection::ColumnReverse => Self::ColumnReverse,
        }
    }
}

impl From<crate::Overflow> for taffy::style::Overflow {
    fn from(value: crate::Overflow) -> Self {
        match value {
            crate::Overflow::Visible => Self::Visible,
            crate::Overflow::Clip => Self::Clip,
            crate::Overflow::Hidden => Self::Hidden,
            crate::Overflow::Scroll => Self::Scroll,
        }
    }
}

impl From<crate::Position> for taffy::style::Position {
    fn from(value: crate::Position) -> Self {
        match value {
            crate::Position::Relative => Self::Relative,
            crate::Position::Absolute => Self::Absolute,
        }
    }
}

impl ToTaffy<taffy::style::Style> for Style {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Style {
        LayoutStyle::from(self).to_taffy(rem_size, scale_factor)
    }
}

impl ToTaffy<taffy::style::Style> for LayoutStyle {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Style {
        record_layout_conversion(1);
        use taffy::style_helpers::{fr, length, minmax, repeat};

        fn to_grid_line(
            placement: &Range<crate::GridPlacement>,
        ) -> taffy::Line<taffy::GridPlacement> {
            taffy::Line {
                start: placement.start.into(),
                end: placement.end.into(),
            }
        }

        fn to_grid_repeat<T: taffy::style::CheapCloneStr>(
            unit: &Option<u16>,
        ) -> Vec<taffy::GridTemplateComponent<T>> {
            // grid-template-columns: repeat(<number>, minmax(0, 1fr));
            unit.map(|count| vec![repeat(count, vec![minmax(length(0.0), fr(1.0))])])
                .unwrap_or_default()
        }

        let has_grid =
            self.grid_rows.is_some() || self.grid_cols.is_some() || self.grid_location.is_some();

        if !has_grid {
            return taffy::style::Style {
                display: self.display.into(),
                overflow: self.overflow.into(),
                scrollbar_width: self.scrollbar_width.to_taffy(rem_size, scale_factor),
                position: self.position.into(),
                inset: self.inset.to_taffy(rem_size, scale_factor),
                size: self.size.to_taffy(rem_size, scale_factor),
                min_size: self.min_size.to_taffy(rem_size, scale_factor),
                max_size: self.max_size.to_taffy(rem_size, scale_factor),
                aspect_ratio: self.aspect_ratio,
                margin: self.margin.to_taffy(rem_size, scale_factor),
                padding: self.padding.to_taffy(rem_size, scale_factor),
                border: self.border_widths.to_taffy(rem_size, scale_factor),
                align_items: self.align_items.map(|x| x.into()),
                align_self: self.align_self.map(|x| x.into()),
                align_content: self.align_content.map(|x| x.into()),
                justify_content: self.justify_content.map(|x| x.into()),
                gap: self.gap.to_taffy(rem_size, scale_factor),
                flex_direction: self.flex_direction.into(),
                flex_wrap: self.flex_wrap.into(),
                flex_basis: self.flex_basis.to_taffy(rem_size, scale_factor),
                flex_grow: self.flex_grow,
                flex_shrink: self.flex_shrink,
                ..Default::default()
            };
        }

        taffy::style::Style {
            display: self.display.into(),
            overflow: self.overflow.into(),
            scrollbar_width: self.scrollbar_width.to_taffy(rem_size, scale_factor),
            position: self.position.into(),
            inset: self.inset.to_taffy(rem_size, scale_factor),
            size: self.size.to_taffy(rem_size, scale_factor),
            min_size: self.min_size.to_taffy(rem_size, scale_factor),
            max_size: self.max_size.to_taffy(rem_size, scale_factor),
            aspect_ratio: self.aspect_ratio,
            margin: self.margin.to_taffy(rem_size, scale_factor),
            padding: self.padding.to_taffy(rem_size, scale_factor),
            border: self.border_widths.to_taffy(rem_size, scale_factor),
            align_items: self.align_items.map(|x| x.into()),
            align_self: self.align_self.map(|x| x.into()),
            align_content: self.align_content.map(|x| x.into()),
            justify_content: self.justify_content.map(|x| x.into()),
            gap: self.gap.to_taffy(rem_size, scale_factor),
            flex_direction: self.flex_direction.into(),
            flex_wrap: self.flex_wrap.into(),
            flex_basis: self.flex_basis.to_taffy(rem_size, scale_factor),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            grid_template_rows: to_grid_repeat(&self.grid_rows),
            grid_template_columns: to_grid_repeat(&self.grid_cols),
            grid_row: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.row))
                .unwrap_or_default(),
            grid_column: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.column))
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl ToTaffy<f32> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> f32 {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                pixels * scale_factor
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                pixels * scale_factor
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for Length {
    fn to_taffy(
        &self,
        rem_size: Pixels,
        scale_factor: f32,
    ) -> taffy::prelude::LengthPercentageAuto {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::LengthPercentageAuto::auto(),
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::prelude::Dimension {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::Dimension::auto(),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentage::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentageAuto {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentageAuto::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Dimension {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::Dimension::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    taffy::style::Dimension::length((*rems * rem_size * scale_factor).into())
                }
            },
            DefiniteLength::Fraction(fraction) => taffy::style::Dimension::percent(*fraction),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
        }
    }
}

impl<T, T2> From<TaffyPoint<T>> for Point<T2>
where
    T: Into<T2>,
    T2: Clone + Debug + Default + PartialEq,
{
    fn from(point: TaffyPoint<T>) -> Point<T2> {
        Point {
            x: point.x.into(),
            y: point.y.into(),
        }
    }
}

impl<T, T2> From<Point<T>> for TaffyPoint<T2>
where
    T: Into<T2> + Clone + Debug + Default + PartialEq,
{
    fn from(point: Point<T>) -> Self {
        TaffyPoint {
            x: point.x.into(),
            y: point.y.into(),
        }
    }
}

impl<T, U> ToTaffy<TaffySize<U>> for Size<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffySize<U> {
        TaffySize {
            width: self.width.to_taffy(rem_size, scale_factor),
            height: self.height.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> ToTaffy<TaffyRect<U>> for Edges<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffyRect<U> {
        TaffyRect {
            top: self.top.to_taffy(rem_size, scale_factor),
            right: self.right.to_taffy(rem_size, scale_factor),
            bottom: self.bottom.to_taffy(rem_size, scale_factor),
            left: self.left.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> From<TaffySize<T>> for Size<U>
where
    T: Into<U>,
    U: Clone + Debug + Default + PartialEq,
{
    fn from(taffy_size: TaffySize<T>) -> Self {
        Size {
            width: taffy_size.width.into(),
            height: taffy_size.height.into(),
        }
    }
}

impl<T, U> From<Size<T>> for TaffySize<U>
where
    T: Into<U> + Clone + Debug + Default + PartialEq,
{
    fn from(size: Size<T>) -> Self {
        TaffySize {
            width: size.width.into(),
            height: size.height.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToTaffy;
    use crate::{LayoutStyle, Style, performance_metrics_snapshot, px};

    #[test]
    fn style_to_taffy_records_single_conversion() {
        let before = performance_metrics_snapshot().layout_conversion_count;
        let _ = Style::default().to_taffy(px(16.), 1.0);
        let after = performance_metrics_snapshot().layout_conversion_count;
        assert_eq!(after, before + 1);
    }

    #[test]
    fn layout_style_without_grid_uses_empty_grid_fields() {
        let style = LayoutStyle::from(&Style::default());
        let taffy_style = style.to_taffy(px(16.), 1.0);

        assert!(taffy_style.grid_template_rows.is_empty());
        assert!(taffy_style.grid_template_columns.is_empty());
        assert_eq!(taffy_style.grid_row, Default::default());
        assert_eq!(taffy_style.grid_column, Default::default());
    }
}
