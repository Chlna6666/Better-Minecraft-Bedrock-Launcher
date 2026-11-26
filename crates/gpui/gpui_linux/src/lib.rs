//! Linux and FreeBSD desktop services used by GPUI.

#![cfg(any(target_os = "linux", target_os = "freebsd"))]

#[cfg(feature = "portal-settings")]
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
