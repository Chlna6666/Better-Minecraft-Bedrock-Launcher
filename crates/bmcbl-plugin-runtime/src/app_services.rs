use crate::PluginError;
use crate::bindings::bmcbl::plugin::host_app::Host;
use crate::execution::PluginState;

impl Host for PluginState {
    fn invoke(&mut self, service: String, arguments: Vec<u8>) -> Result<String, PluginError> {
        self.require_render_safe("BMCBL service invocation")?;
        if arguments.len() > 4 * 1024 * 1024 {
            return Err(PluginError::invalid_input("service arguments exceed 4 MiB"));
        }
        self.context.invoke_service(&service, arguments)
    }

    fn take(&mut self, id: String) -> Result<Option<Vec<u8>>, PluginError> {
        self.require_render_safe("take BMCBL service result")?;
        self.context.take_service(&id)
    }

    fn cancel(&mut self, id: String) -> Result<(), PluginError> {
        self.require_render_safe("cancel BMCBL service")?;
        self.context.cancel_service(&id)
    }
}
