//! Roll back raw GPU allocations when a multi-resource constructor fails.
use gpui::ExtensionDevice;

type Cleanup = Box<dyn FnOnce(&mut dyn ExtensionDevice)>;

#[derive(Default)]
pub(super) struct Construction {
    cleanup: Vec<Cleanup>,
}

impl Construction {
    pub(super) fn record<T: Copy + 'static>(
        &mut self,
        value: T,
        destroy: impl FnOnce(&mut dyn ExtensionDevice, T) + 'static,
    ) -> T {
        self.cleanup
            .push(Box::new(move |device| destroy(device, value)));
        value
    }

    pub(super) fn finish<T>(
        self,
        device: &mut dyn ExtensionDevice,
        result: gpui::Result<T>,
    ) -> gpui::Result<T> {
        if result.is_err() {
            for cleanup in self.cleanup.into_iter().rev() {
                cleanup(device);
            }
        }
        result
    }
}
