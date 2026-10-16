// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

mod animation_bounds;
mod batch;
mod bounds_tree;
mod composite;
mod display_list;
pub(crate) mod geometry;
mod prepared;
mod operations;
mod primitive;
mod quad_stream;
#[cfg(test)]
mod tests;

use crate::primitives::*;

pub(crate) use batch::*;
pub(crate) use bounds_tree::BoundsTree;
pub(crate) use display_list::*;
pub(crate) use prepared::*;
pub(crate) use operations::{Operations, OperationRange};
pub(crate) use primitive::*;
pub(crate) use quad_stream::{QuadReplay, QuadStream};
