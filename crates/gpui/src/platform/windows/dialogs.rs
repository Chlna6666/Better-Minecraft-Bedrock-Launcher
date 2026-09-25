#![expect(unsafe_code, reason = "native dialogs call Win32 and COM interfaces")]

use std::{sync::{Arc, Mutex}, thread};

use anyhow::{Context, Result};
use futures::channel::oneshot;
use windows::{
    Win32::{
        System::{
            Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize},
            LibraryLoader::GetProcAddress,
        },
        UI::{Controls::*, WindowsAndMessaging::*},
    },
    core::{BOOL, HRESULT, HSTRING},
};

use super::with_dll_library;

struct StaApartment;

impl StaApartment {
    fn enter() -> Result<Self> {
        // SAFETY: This runs on a freshly created worker thread before any dialog COM object is
        // created. The matching CoUninitialize executes on the same thread in Drop.
        unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for StaApartment {
    fn drop(&mut self) {
        // SAFETY: Paired with the successful CoInitializeEx call on this same worker thread.
        unsafe { CoUninitialize() };
    }
}

/// Runs one synchronous native dialog on a dedicated COM STA thread.
///
/// File pickers and modal Win32 dialogs can remain open for seconds or minutes while the user
/// decides. Executing their native modal loops from GPUI's foreground executor turns that whole
/// interval into one giant foreground task poll and blocks input/frame scheduling. The returned
/// receiver preserves the existing asynchronous platform API while the foreground thread remains
/// free to pump windows and frames.
pub(crate) fn spawn_sta_dialog<T>(
    thread_name: &'static str,
    dialog: impl FnOnce() -> Result<T> + Send + 'static,
) -> oneshot::Receiver<Result<T>>
where
    T: Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    let sender = Arc::new(Mutex::new(Some(sender)));
    let worker_sender = sender.clone();

    let spawn_result = thread::Builder::new()
        .name(thread_name.to_string())
        .spawn(move || {
            let result = (|| {
                let _apartment = StaApartment::enter()?;
                dialog()
            })();

            if let Some(sender) = worker_sender
                .lock()
                .expect("Windows dialog result sender lock poisoned")
                .take()
            {
                let _ = sender.send(result);
            }
        });

    if let Err(error) = spawn_result
        && let Some(sender) = sender
            .lock()
            .expect("Windows dialog result sender lock poisoned")
            .take()
    {
        let _ = sender.send(Err(error.into()));
    }

    receiver
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
