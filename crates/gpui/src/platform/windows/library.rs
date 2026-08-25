#![expect(
    unsafe_code,
    reason = "dynamic Win32 library lookup returns raw procedure addresses"
)]

use ::util::ResultExt;
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
    let library = unsafe {
        LoadLibraryA(dll_name).with_context(|| format!("Loading dll: {}", dll_name.display()))?
    };
    let result = call(library);
    unsafe {
        FreeLibrary(library)
            .with_context(|| format!("Freeing dll: {}", dll_name.display()))
            .log_err();
    }
    result
}
