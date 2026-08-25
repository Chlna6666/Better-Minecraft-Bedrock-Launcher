#![expect(unsafe_code, reason = "native dialogs call Win32 and COM interfaces")]

use anyhow::Context;
use windows::{
    Win32::{
        System::LibraryLoader::GetProcAddress,
        UI::{Controls::*, WindowsAndMessaging::*},
    },
    core::{BOOL, HRESULT, HSTRING},
};

use super::with_dll_library;

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

    let response = with_dll_library(windows::core::s!("comctl32.dll"), |library| {
        let Some(proc) =
            (unsafe { GetProcAddress(library, windows::core::s!("TaskDialogIndirect")) })
        else {
            anyhow::bail!("TaskDialogIndirect entry point is not available");
        };
        let task_dialog: TaskDialogIndirectFn = unsafe { std::mem::transmute(proc) };
        let mut button = 0_i32;
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

    match response {
        Ok(button) => Some(button),
        Err(error) => {
            log::warn!("TaskDialogIndirect unavailable, falling back to MessageBoxW: {error:#}");
            let response = unsafe {
                MessageBoxW(
                    Some(config.hwndParent),
                    &HSTRING::from(fallback_content),
                    &HSTRING::from(fallback_title),
                    MB_OKCANCEL | MB_ICONINFORMATION | MB_SYSTEMMODAL,
                )
            };
            match response {
                IDOK => Some(IDOK.0),
                IDCANCEL => Some(IDCANCEL.0),
                _ => None,
            }
        }
    }
}
