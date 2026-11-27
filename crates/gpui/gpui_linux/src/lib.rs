//! Wayland, X11 and headless platform providers for GPUI.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

#[cfg(any(feature = "wayland", feature = "x11"))]
pub mod xdg_desktop_portal;

/// Guesses the desktop session without connecting to the compositor.
pub fn guess_compositor() -> &'static str {
    if std::env::var_os("ZED_HEADLESS").is_some() {
        return "Headless";
    }

    if cfg!(feature = "wayland")
        && std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty())
    {
        return "Wayland";
    }
    if cfg!(feature = "x11")
        && std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty())
    {
        return "X11";
    }
    "Headless"
}

mod dispatcher;
mod headless;
mod keyboard;
mod implementation;
#[cfg(feature = "wayland")]
mod wayland;
#[cfg(feature = "x11")]
mod x11;

pub(crate) use dispatcher::*;
pub(crate) use headless::*;
pub(crate) use keyboard::*;
pub(crate) use implementation::*;
#[cfg(feature = "wayland")]
pub(crate) use wayland::*;
#[cfg(feature = "x11")]
pub(crate) use x11::*;

/// Constructs the active Linux desktop provider, or the headless event loop.
pub fn current_platform(headless: bool, options: gpui::RendererOptions) -> std::rc::Rc<dyn gpui::Platform> {
    use std::rc::Rc;
    #[cfg(feature = "x11")]
    use anyhow::Context as _;

    if headless {
        return Rc::new(HeadlessClient::new());
    }
    match guess_compositor() {
        #[cfg(feature = "wayland")]
        "Wayland" => Rc::new(WaylandClient::new(options)),
        #[cfg(feature = "x11")]
        "X11" => Rc::new(X11Client::new(options).context("Failed to initialize X11 client").unwrap()),
        _ => Rc::new(HeadlessClient::new()),
    }
}
