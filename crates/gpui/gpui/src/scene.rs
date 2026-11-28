// todo("windows"): remove
#![cfg_attr(windows, allow(dead_code))]

mod animation_bounds;
mod batch;
mod bounds_tree;
#[cfg(test)]
mod composite;
mod display_list;
pub(crate) mod geometry;
mod operations;
mod operation_page;
mod page_compaction;
mod painter;
mod prepared;
mod primitive;
mod primitive_stream;
#[cfg(test)]
mod tests;

use crate::primitives::*;

pub(crate) use batch::*;
pub(crate) use bounds_tree::BoundsTree;
pub(crate) use display_list::*;
pub(crate) use operations::{OperationRange, OperationRun, Operations};
pub(crate) use painter::{CapturedViewRange, Painter};
pub(crate) use prepared::*;
pub(crate) use primitive::*;
pub(crate) use primitive_stream::{PayloadIdentity, PrimitiveStream, QuadReplay};
