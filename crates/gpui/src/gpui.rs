#![doc = include_str!("../README.md")]
#![deny(missing_docs)]
#![allow(clippy::type_complexity)] // Not useful, GPUI makes heavy use of callbacks
#![allow(clippy::collapsible_else_if)] // False positives in platform specific code
#![allow(unused_mut)] // False positives in platform specific code

extern crate self as gpui;

#[macro_use]
mod input;
mod app;

mod animation;
mod assets;
mod diagnostics;
mod element;
mod foundation;
mod geometry;
mod layout;
mod platform;
mod render_pipeline;
mod scene;
mod style;
mod text_system;
mod window;

#[cfg(doc)]
pub use app::ownership_and_data_flow as _ownership_and_data_flow;

#[cfg(any(test, feature = "test-support"))]
pub use app::test_support as test;
pub use foundation::prelude;

/// Do not touch, here be dragons for use by gpui_macros and such.
#[doc(hidden)]
pub mod private {
    pub use anyhow;
    pub use inventory;
    pub use schemars;
    pub use serde;
    pub use serde_json;
}

mod seal {
    /// A mechanism for restricting implementations of a trait to only those in GPUI.
    /// See: <https://predr.ag/blog/definitive-guide-to-sealed-traits-in-rust/>
    pub trait Sealed {}
}

pub use animation::*;
pub use anyhow::Result;
pub use app::*;
pub use assets::*;
pub use ctor::ctor;
pub use diagnostics::*;
pub use element::*;
pub use foundation::*;
pub use geometry::*;
pub use gpui_macros::{AppContext, IntoElement, Render, VisualContext, register_action, test};
pub use http_client;
pub use image;
pub use input::*;
pub use layout::{
    AvailableSpace, LayoutId, absolute_fill, center, h_stack, relative_fill, v_stack,
};
pub use platform::*;
pub use refineable::*;
pub use render_pipeline::*;
pub use scene::*;
pub use smol::Timer;
pub use style::*;
#[cfg(any(test, feature = "test-support"))]
pub use test::*;
pub use text_system::*;
pub use window::*;

pub(crate) use layout::TaffyLayoutEngine;
