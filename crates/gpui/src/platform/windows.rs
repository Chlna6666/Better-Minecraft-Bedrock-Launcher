mod appearance;
mod clipboard;
mod coordinates;
mod credentials;
mod destination_list;
mod dialogs;
mod direct_write;
mod dispatcher;
mod display;
mod input;
mod keyboard;
mod library;
mod platform;
mod vsync;
mod window;

pub(crate) use appearance::*;
pub(crate) use clipboard::*;
pub(crate) use coordinates::*;
pub(crate) use credentials::*;
pub(crate) use destination_list::*;
pub(crate) use dialogs::*;
pub(crate) use direct_write::*;
pub(crate) use dispatcher::*;
pub(crate) use display::*;
pub(crate) use input::*;
pub(crate) use keyboard::{WindowsKeyboardLayout, WindowsKeyboardMapper};
pub(crate) use library::*;
pub(crate) use platform::{WindowCreationInfo, WindowsPlatform, WindowsUserEvent};
pub(crate) use window::*;

#[allow(unsafe_code)]
pub(super) fn service_driver_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        MSG, PM_NOREMOVE, PM_QS_SENDMESSAGE, PeekMessageW, WM_NULL,
    };
    // DXGI presentation and COM release can synchronously send to the HWND
    // thread. Keep those sends moving while retaining the destruction barrier;
    // posted input and application callbacks are not consumed here.
    let mut message = MSG::default();
    // SAFETY: The output is stack-owned. PeekMessage itself dispatches sends;
    // its BOOL only reports queued-message availability, not success/failure.
    let _ = unsafe {
        PeekMessageW(
            &mut message,
            None,
            WM_NULL,
            WM_NULL,
            PM_NOREMOVE | PM_QS_SENDMESSAGE,
        )
    };
}
