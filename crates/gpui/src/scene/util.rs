use std::ops::Range;

use super::Quad;
use crate::IsZero;

pub(super) fn trim_vec_capacity<T>(vec: &mut Vec<T>, floor: usize, multiplier: usize) {
    if vec.capacity() > floor.saturating_mul(multiplier) {
        vec.shrink_to(floor);
    }
}

pub(super) fn slice_range<T>(whole: &[T], part: &[T]) -> Range<usize> {
    let start =
        part.as_ptr().addr().saturating_sub(whole.as_ptr().addr()) / std::mem::size_of::<T>();
    start..start.saturating_add(part.len())
}

pub(super) fn is_solid_quad(quad: &Quad) -> bool {
    quad.background.tag == crate::BackgroundTag::Solid
        && !quad.border_widths.any(|width| !width.is_zero())
        && quad.corner_radii.is_zero()
}
