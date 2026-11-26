//! Native Windows services used by GPUI.

#![cfg(target_os = "windows")]

#[doc(hidden)]
pub mod dialogs;
#[doc(hidden)]
pub mod vsync;

mod driver_messages;
mod library;

pub use driver_messages::service_driver_messages;
