//! Pixel damage and filter-source proofs derived from immutable Paint data.
//!
//! These planners borrow the authoritative Scene; they retain no second paint stream,
//! resource residency state, or animation clock. Callers supply resolved frame samples.
mod animation_damage;
mod damage;
mod filter_damage;
mod geometry;
pub(crate) mod retention;

pub(crate) use animation_damage::*;
pub(crate) use damage::*;
pub(crate) use filter_damage::*;
pub(crate) use geometry::*;

use crate::scene::blur_influence_radius;
use crate::scene::geometry::is_solid_quad;
use crate::{
    BlurCapture, Bounds, DrawOrder, PaintBackdropBlur, PaintOperation, Primitive, ScaledPixels,
    Scene, SceneAnimationId, SceneAnimationValue, TransitionProperty,
};
use smallvec::SmallVec;
use std::ops::Range;

#[cfg(test)]
mod tests;
