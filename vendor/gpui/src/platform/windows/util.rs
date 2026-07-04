use ::util::ResultExt;
use anyhow::Context;
use windows::{
    UI::{
        Color,
        ViewManagement::{UIColorType, UISettings},
    },
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        System::LibraryLoader::{GetProcAddress, LoadLibraryA},
        UI::{Controls::*, WindowsAndMessaging::*},
    },
    core::{BOOL, HRESULT, HSTRING, PCSTR},
};

use crate::*;

pub(crate) fn windows_credentials_target_name(url: &str) -> String {
    format!("zed:url={}", url)
}

#[inline]
pub(crate) fn logical_point(x: f32, y: f32, scale_factor: f32) -> Point<Pixels> {
    Point {
        x: px(x / scale_factor),
        y: px(y / scale_factor),
    }
}

// https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/apply-windows-themes
#[inline]
pub(crate) fn system_appearance() -> Result<WindowAppearance> {
    let ui_settings = UISettings::new()?;
    let foreground_color = ui_settings.GetColorValue(UIColorType::Foreground)?;
    // If the foreground is light, then is_color_light will evaluate to true,
    // meaning Dark mode is enabled.
    if is_color_light(&foreground_color) {
        Ok(WindowAppearance::Dark)
    } else {
        Ok(WindowAppearance::Light)
    }
}

#[inline(always)]
fn is_color_light(color: &Color) -> bool {
    ((5 * color.G as u32) + (2 * color.R as u32) + color.B as u32) > (8 * 128)
}

pub(crate) fn show_error(title: &str, content: String) {
    let _ = unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(content),
            &HSTRING::from(title),
            MB_ICONERROR | MB_SYSTEMMODAL,
        )
    };
}

pub(crate) fn show_task_dialog_or_message_box(
    config: &TASKDIALOGCONFIG,
    fallback_title: &str,
    fallback_content: &str,
) -> Option<i32> {
    type TaskDialogIndirectFn = unsafe extern "system" fn(
        *const TASKDIALOGCONFIG,
        *mut i32,
        *mut i32,
        *mut BOOL,
    ) -> HRESULT;

    let dialog_response = with_dll_library(windows::core::s!("comctl32.dll"), |library| {
        let proc = unsafe { GetProcAddress(library, windows::core::s!("TaskDialogIndirect")) };
        let Some(proc) = proc else {
            anyhow::bail!("TaskDialogIndirect entry point is not available");
        };
        // SAFETY: The symbol name is fixed and we only call it with the documented signature.
        let task_dialog: TaskDialogIndirectFn = unsafe { std::mem::transmute(proc) };
        let mut button = 0_i32;
        // SAFETY: `config` points to a fully initialized TASKDIALOGCONFIG for the duration
        // of the call, and the out-pointers are valid stack locals.
        unsafe {
            task_dialog(
                config as *const _,
                &mut button,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        }
        .ok()
        .context("TaskDialogIndirect returned an error")?;
        Ok(button)
    });

    match dialog_response {
        Ok(button) => Some(button),
        Err(error) => {
            log::warn!("TaskDialogIndirect unavailable, falling back to MessageBoxW: {error:#}");
            let message_box_response = unsafe {
                MessageBoxW(
                    Some(config.hwndParent),
                    &HSTRING::from(fallback_content),
                    &HSTRING::from(fallback_title),
                    MB_OKCANCEL | MB_ICONINFORMATION | MB_SYSTEMMODAL,
                )
            };
            let button = match message_box_response {
                IDOK => IDOK.0,
                IDCANCEL => IDCANCEL.0,
                _ => 0,
            };
            (button != 0).then_some(button)
        }
    }
}

pub(crate) fn with_dll_library<R, F>(dll_name: PCSTR, f: F) -> Result<R>
where
    F: FnOnce(HMODULE) -> Result<R>,
{
    let library = unsafe {
        LoadLibraryA(dll_name).with_context(|| format!("Loading dll: {}", dll_name.display()))?
    };
    let library_call = f(library);
    unsafe {
        FreeLibrary(library)
            .with_context(|| format!("Freeing dll: {}", dll_name.display()))
            .log_err();
    }
    library_call
}
