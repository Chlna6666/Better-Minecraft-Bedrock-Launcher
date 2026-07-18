mod clipboard;
mod destination_list;
mod direct_write;
mod dispatcher;
mod display;
mod input;
mod keyboard;
mod platform;
mod util;
mod window;

pub(crate) use clipboard::*;
pub(crate) use destination_list::*;
pub(crate) use direct_write::*;
pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use input::*;
pub(crate) use keyboard::{WindowsKeyboardLayout, WindowsKeyboardMapper};
pub(crate) use platform::{WindowCreationInfo, WindowsPlatform, WindowsUserEvent};
pub(crate) use util::*;
pub(crate) use window::*;

#[cfg(feature = "screen-capture")]
pub(crate) type PlatformScreenCaptureFrame = scap::frame::Frame;
#[cfg(not(feature = "screen-capture"))]
pub(crate) type PlatformScreenCaptureFrame = ();
