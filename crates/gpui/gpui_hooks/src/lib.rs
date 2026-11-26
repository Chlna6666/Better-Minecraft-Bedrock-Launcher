#![doc = include_str!("../README.md")]

extern crate self as gpui_hooks;

mod element;

pub mod hooks;

pub use element::{HookedElement, HookedRender, execute_hooked_render};
pub use gpui_hooks_macros::{hook_element, hook_render};
