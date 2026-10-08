//! Owns the WGL pixel-format DC independently of application windows.
use gfx_core::{Error, Result};
use raw_window_handle::{RawWindowHandle, Win32WindowHandle};
use std::num::NonZeroIsize;
use windows::{
    Win32::{
        Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CS_OWNDC, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClassInfoW,
            RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW, WS_POPUP,
        },
    },
    core::w,
};

pub(crate) struct Bootstrap(HWND, HINSTANCE);

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: Windows calls this trampoline with its live window and message parameters.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

impl Bootstrap {
    pub(crate) fn new() -> Result<Self> {
        // SAFETY: class data is stable and the hidden window is created/destroyed on
        // the graphics owner. CS_OWNDC keeps the config DC valid for the device lifetime.
        unsafe {
            let instance = HINSTANCE(GetModuleHandleW(None).map_err(crate::device::native)?.0);
            let class = w!("NovaOpenGlBootstrap");
            let mut existing = WNDCLASSW::default();
            if GetClassInfoW(Some(instance), class, &mut existing).is_err() {
                let descriptor = WNDCLASSW {
                    style: CS_OWNDC,
                    lpfnWndProc: Some(window_proc),
                    hInstance: instance,
                    lpszClassName: class,
                    ..Default::default()
                };
                if RegisterClassW(&descriptor) == 0 {
                    return Err(crate::device::native(windows::core::Error::from_thread()));
                }
            }
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                class,
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )
            .map_err(crate::device::native)?;
            Ok(Self(hwnd, instance))
        }
    }

    pub(crate) fn window_handle(&self) -> Result<RawWindowHandle> {
        let hwnd = NonZeroIsize::new(self.0.0 as isize)
            .ok_or_else(|| Error::Backend("invalid WGL bootstrap HWND".into()))?;
        let mut handle = Win32WindowHandle::new(hwnd);
        handle.hinstance = NonZeroIsize::new(self.1.0 as isize);
        Ok(RawWindowHandle::Win32(handle))
    }
}

impl Drop for Bootstrap {
    fn drop(&mut self) {
        // SAFETY: owned HWND, released after all device GL objects and surfaces.
        if let Err(error) = unsafe { DestroyWindow(self.0) } {
            log::warn!("destroying OpenGL bootstrap window failed: {error}");
        }
    }
}
