//! Win32 platform implementation for GPUI.

#![cfg(target_os = "windows")]

#[doc(hidden)]
pub mod dialogs;
#[doc(hidden)]
pub mod vsync;

mod driver_messages;
mod library;

pub use driver_messages::service_driver_messages;

mod appearance;
mod clipboard;
mod coordinates;
mod credentials;
mod destination_list;
mod direct_write;
mod dispatcher;
mod display;
mod display_events;
mod input;
mod keyboard;
mod implementation;
mod window;
mod winit_support;

pub(crate) use appearance::*;
pub(crate) use clipboard::*;
pub(crate) use coordinates::*;
pub(crate) use credentials::*;
pub(crate) use destination_list::*;
pub(crate) use direct_write::*;
pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use input::*;
pub(crate) use keyboard::{WindowsKeyboardLayout, WindowsKeyboardMapper};
pub(crate) use implementation::{WindowCreationInfo, WindowsUserEvent};
pub use implementation::WindowsPlatform;
pub(crate) use window::*;
