//! Renderer instances belong to one GPU owner; scene nodes only supply immutable inputs.

use super::*;
use crate::ExtensionDevice;
use std::{any::TypeId, collections::hash_map::Entry};

#[derive(Default)]
pub(super) struct RendererRegistry {
    renderers: FxHashMap<TypeId, Box<dyn crate::RendererExtensionRenderer>>,
}

impl RendererRegistry {
    pub(super) fn needs_gpu_completion(&self) -> bool {
        self.renderers
            .values()
            .any(|renderer| renderer.needs_gpu_completion())
    }

    pub(super) fn gpu_work_completed(&mut self) {
        for renderer in self.renderers.values_mut() {
            renderer.gpu_work_completed();
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.renderers.is_empty()
    }

    pub(super) fn has_inactive(&self, active: &[TypeId]) -> bool {
        self.renderers
            .keys()
            .any(|type_id| !active.contains(type_id))
    }

    pub(super) fn prepare(
        &mut self,
        input: &dyn crate::RendererExtension,
        device: &mut dyn ExtensionDevice,
        context: crate::RendererExtensionContext,
        steps: &mut Vec<RenderCommand>,
    ) -> Result<()> {
        let renderer = match self.renderers.entry(input.renderer_type()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(input.create_renderer(device, context.clone())?),
        };
        renderer.render(input, device, context, steps)
    }

    pub(super) fn retain(&mut self, active: &[TypeId], device: &mut dyn ExtensionDevice) {
        self.renderers.retain(|type_id, renderer| {
            if active.contains(type_id) {
                return true;
            }
            if let Err(error) = renderer.destroy(device) {
                log::debug!("failed to destroy inactive GPUI renderer extension: {error}");
            }
            false
        });
    }

    pub(super) fn trim(
        &mut self,
        device: &mut dyn ExtensionDevice,
        level: gfx_core::MemoryTrimLevel,
    ) {
        for renderer in self.renderers.values_mut() {
            if let Err(error) = renderer.trim_memory(device, level) {
                log::debug!("failed to trim GPUI renderer extension resources: {error}");
            }
        }
    }

    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) {
        self.retain(&[], device);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PendingRenderer {
        pending: bool,
    }

    impl crate::RendererExtensionRenderer for PendingRenderer {
        fn needs_gpu_completion(&self) -> bool {
            self.pending
        }
        fn gpu_work_completed(&mut self) {
            self.pending = false;
        }
        fn render(
            &mut self,
            _extension: &dyn crate::RendererExtension,
            _device: &mut dyn ExtensionDevice,
            _context: crate::RendererExtensionContext,
            _steps: &mut Vec<RenderCommand>,
        ) -> crate::Result<()> {
            Ok(())
        }
        fn destroy(&mut self, _device: &mut dyn ExtensionDevice) -> crate::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn completion_notification_retires_pending_extension_work() {
        let mut registry = RendererRegistry::default();
        registry.renderers.insert(
            TypeId::of::<PendingRenderer>(),
            Box::new(PendingRenderer { pending: true }),
        );
        assert!(registry.needs_gpu_completion());
        registry.gpu_work_completed();
        assert!(!registry.needs_gpu_completion());
    }
}
