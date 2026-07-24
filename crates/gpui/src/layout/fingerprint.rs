use crate::{Edges, Point, Size};
use collections::FxHasher;
use std::{
    hash::{Hash, Hasher},
    ops::Range,
};

pub(super) fn hash_grid_location(location: &crate::GridLocation) -> u64 {
    let mut hasher = FxHasher::default();
    hash_grid_placement_range(&location.row, &mut hasher);
    hash_grid_placement_range(&location.column, &mut hasher);
    hasher.finish()
}

pub(super) fn hash_grid_placement_range(
    range: &Range<crate::GridPlacement>,
    hasher: &mut FxHasher,
) {
    hash_grid_placement(range.start, hasher);
    hash_grid_placement(range.end, hasher);
}

pub(super) fn hash_display(display: crate::Display, hasher: &mut FxHasher) {
    std::mem::discriminant(&display).hash(hasher);
}

pub(super) fn hash_position(position: crate::Position, hasher: &mut FxHasher) {
    std::mem::discriminant(&position).hash(hasher);
}

pub(super) fn hash_overflow_point(point: &Point<crate::Overflow>, hasher: &mut FxHasher) {
    hash_overflow_value(point.x, hasher);
    hash_overflow_value(point.y, hasher);
}

pub(super) fn hash_absolute_length(length: crate::AbsoluteLength, hasher: &mut FxHasher) {
    match length {
        crate::AbsoluteLength::Pixels(pixels) => {
            0u8.hash(hasher);
            pixels.0.to_bits().hash(hasher);
        }
        crate::AbsoluteLength::Rems(rems) => {
            1u8.hash(hasher);
            rems.0.to_bits().hash(hasher);
        }
    }
}

pub(super) fn hash_length(length: crate::Length, hasher: &mut FxHasher) {
    match length {
        crate::Length::Definite(definite) => {
            0u8.hash(hasher);
            hash_definite_length(definite, hasher);
        }
        crate::Length::Auto => {
            1u8.hash(hasher);
        }
    }
}

pub(super) fn hash_overflow_value(overflow: crate::Overflow, hasher: &mut FxHasher) {
    std::mem::discriminant(&overflow).hash(hasher);
}

pub(super) fn hash_definite_length(length: crate::DefiniteLength, hasher: &mut FxHasher) {
    match length {
        crate::DefiniteLength::Absolute(absolute) => {
            0u8.hash(hasher);
            hash_absolute_length(absolute, hasher);
        }
        crate::DefiniteLength::Fraction(fraction) => {
            1u8.hash(hasher);
            fraction.to_bits().hash(hasher);
        }
    }
}

pub(super) fn hash_size_length(size: &Size<crate::Length>, hasher: &mut FxHasher) {
    hash_length(size.width, hasher);
    hash_length(size.height, hasher);
}

pub(super) fn hash_size_definite_length(size: &Size<crate::DefiniteLength>, hasher: &mut FxHasher) {
    hash_definite_length(size.width, hasher);
    hash_definite_length(size.height, hasher);
}

pub(super) fn hash_edges_length(edges: &Edges<crate::Length>, hasher: &mut FxHasher) {
    hash_length(edges.top, hasher);
    hash_length(edges.right, hasher);
    hash_length(edges.bottom, hasher);
    hash_length(edges.left, hasher);
}

pub(super) fn hash_edges_definite_length(
    edges: &Edges<crate::DefiniteLength>,
    hasher: &mut FxHasher,
) {
    hash_definite_length(edges.top, hasher);
    hash_definite_length(edges.right, hasher);
    hash_definite_length(edges.bottom, hasher);
    hash_definite_length(edges.left, hasher);
}

pub(super) fn hash_edges_absolute_length(
    edges: &Edges<crate::AbsoluteLength>,
    hasher: &mut FxHasher,
) {
    hash_absolute_length(edges.top, hasher);
    hash_absolute_length(edges.right, hasher);
    hash_absolute_length(edges.bottom, hasher);
    hash_absolute_length(edges.left, hasher);
}

pub(super) fn hash_grid_placement(placement: crate::GridPlacement, hasher: &mut FxHasher) {
    match placement {
        crate::GridPlacement::Line(index) => {
            0u8.hash(hasher);
            index.hash(hasher);
        }
        crate::GridPlacement::Span(span) => {
            1u8.hash(hasher);
            span.hash(hasher);
        }
        crate::GridPlacement::Auto => {
            2u8.hash(hasher);
        }
    }
}

pub(super) fn hash_flex_direction(direction: crate::FlexDirection, hasher: &mut FxHasher) {
    std::mem::discriminant(&direction).hash(hasher);
}

pub(super) fn hash_flex_wrap(wrap: crate::FlexWrap, hasher: &mut FxHasher) {
    std::mem::discriminant(&wrap).hash(hasher);
}

pub(super) fn hash_optional_align_items(value: Option<crate::AlignItems>, hasher: &mut FxHasher) {
    match value {
        Some(value) => {
            1u8.hash(hasher);
            std::mem::discriminant(&value).hash(hasher);
        }
        None => {
            0u8.hash(hasher);
        }
    }
}

pub(super) fn hash_optional_align_content(
    value: Option<crate::AlignContent>,
    hasher: &mut FxHasher,
) {
    match value {
        Some(value) => {
            1u8.hash(hasher);
            std::mem::discriminant(&value).hash(hasher);
        }
        None => {
            0u8.hash(hasher);
        }
    }
}
