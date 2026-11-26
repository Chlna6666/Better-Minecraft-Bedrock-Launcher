#![expect(
    unsafe_code,
    reason = "dynamic Win32 library lookup returns raw procedure addresses"
)]

use anyhow::Context;
use windows::{
    Win32::Foundation::{FreeLibrary, HMODULE},
    Win32::System::LibraryLoader::LoadLibraryA,
    core::PCSTR,
};

pub(crate) fn with_dll_library<R, F>(dll_name: PCSTR, call: F) -> anyhow::Result<R>
where
    F: FnOnce(HMODULE) -> anyhow::Result<R>,
{
    // SAFETY: The caller provides a nul-terminated DLL name used for this call only.
    let library = unsafe {
        LoadLibraryA(dll_name).with_context(|| format!("Loading dll: {}", dll_name.display()))?
    };
    let result = call(library);
    // SAFETY: This handle is owned by the successful LoadLibraryA above.
    if let Err(error) = unsafe { FreeLibrary(library) } {
        log::error!("FreeLibrary failed: {error}");
    }
    result
}
