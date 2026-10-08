#![cfg_attr(
    target_os = "windows",
    expect(unsafe_code, reason = "native drawable queries borrow the window HWND")
)]

use super::*;

#[cfg(target_os = "windows")]
pub(super) fn native_windows_hwnd<W>(window: &W) -> Option<isize>
where
    W: ::winit::raw_window_handle::HasWindowHandle + ?Sized,
{
    use ::winit::raw_window_handle::RawWindowHandle;

    let raw_window_handle = window.window_handle().ok()?.as_raw();
    let RawWindowHandle::Win32(handle) = raw_window_handle else {
        return None;
    };
    Some(handle.hwnd.get())
}

#[cfg(target_os = "windows")]
fn native_windows_drawable_size<W>(window: &W) -> Option<Size<DevicePixels>>
where
    W: ::winit::raw_window_handle::HasWindowHandle + ?Sized,
{
    use windows::Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::GetClientRect,
    };

    let hwnd = HWND(native_windows_hwnd(window)? as *mut _);
    let mut client_rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut client_rect).ok()? };
    let width = client_rect.right.saturating_sub(client_rect.left);
    let height = client_rect.bottom.saturating_sub(client_rect.top);
    if width <= 0 || height <= 0 {
        return None;
    }

    Some(Size {
        width: DevicePixels(width),
        height: DevicePixels(height),
    })
}

pub(super) fn resolve_initial_drawable_size<W>(
    window: &W,
    requested: Size<DevicePixels>,
) -> Size<DevicePixels>
where
    W: ::winit::raw_window_handle::HasWindowHandle + ?Sized,
{
    #[cfg(target_os = "windows")]
    if let Some(native) = native_windows_drawable_size(window) {
        if native != requested {
            log::debug!(
                "Nova renderer initial drawable size corrected from requested={}x{} to native-client={}x{}",
                requested.width.0,
                requested.height.0,
                native.width.0,
                native.height.0,
            );
        }
        return native;
    }

    #[cfg(not(target_os = "windows"))]
    let _ = window;

    requested
}
