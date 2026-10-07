//! Renderer instances belong to one GPU owner; scene nodes only supply immutable inputs.

use super::*;
use std::{any::TypeId, collections::hash_map::Entry};

#[derive(Default)]
pub(super) struct RendererRegistry {
    renderers: FxHashMap<TypeId, Box<dyn crate::RendererExtensionRenderer>>,
}

impl RendererRegistry {
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
        device: &mut dyn gfx_core::ExtensionDevice,
        context: crate::RendererExtensionContext,
        steps: &mut Vec<RenderStepDescriptor>,
    ) -> Result<()> {
        let renderer = match self.renderers.entry(input.renderer_type()) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(input.create_renderer(device, context.clone())?),
        };
        renderer.render(input, device, context, steps)
    }

    pub(super) fn retain(&mut self, active: &[TypeId], device: &mut dyn gfx_core::ExtensionDevice) {
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
        device: &mut dyn gfx_core::ExtensionDevice,
        level: gfx_core::MemoryTrimLevel,
    ) {
        for renderer in self.renderers.values_mut() {
            if let Err(error) = renderer.trim_memory(device, level) {
                log::debug!("failed to trim GPUI renderer extension resources: {error}");
            }
        }
    }

    pub(super) fn destroy(&mut self, device: &mut dyn gfx_core::ExtensionDevice) {
        self.retain(&[], device);
    }
}
