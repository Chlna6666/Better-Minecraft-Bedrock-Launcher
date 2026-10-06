//! Caching of the compiled renderer core across windows.

use super::super::*;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// Renderer cores created on this thread, keyed by device and surface color format.
    static CORES: RefCell<HashMap<(DeviceKey, Format), RendererCore>> = RefCell::new(HashMap::new());
}

/// Returns the renderer core for `key` and `format`, creating it on first use.
///
/// The core carries the compiled shader modules and render pipelines, so a second window
/// rendering through the same device with the same color format reuses them instead of paying
/// for another shader compile and driver pipeline build. Cores are cached per thread for the
/// same reason devices are: a core holds handles owned by a device that is not `Send`. A core
/// therefore never outlives the device whose handles it holds.
///
/// # Errors
///
/// Returns the creation error when no core exists for this key and format yet and creating one
/// fails.
pub(in crate::platform::nova) fn shared_renderer_core(
    key: DeviceKey,
    format: Format,
    create: impl FnOnce() -> Result<RendererCore>,
) -> Result<RendererCore> {
    CORES.with(|cores| {
        let mut cores = cores.borrow_mut();
        let lookup = (key, format);
        if let Some(existing) = cores.get(&lookup) {
            log::info!("reusing shared nova renderer core: format={format:?}");
            return Ok(*existing);
        }

        let core = create()?;
        log::info!("created shared nova renderer core: format={format:?}");
        cores.insert(lookup, core);
        Ok(core)
    })
}
