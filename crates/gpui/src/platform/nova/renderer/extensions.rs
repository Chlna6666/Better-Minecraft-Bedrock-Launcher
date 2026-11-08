//! Renderer instances belong to one GPU owner; scene nodes only supply immutable inputs.

use super::*;
use crate::ExtensionDevice;
use std::{any::TypeId, collections::hash_map::Entry};

#[derive(Default)]
pub(super) struct RendererRegistry {
    renderers: FxHashMap<TypeId, Box<dyn crate::RendererExtensionRenderer>>,
    passes: Vec<crate::RendererExtensionPass>,
    /// Only renderer extensions that actually contributed producer passes
    /// to this frame may receive a submission acknowledgement. An unrelated
    /// extension must never mark another renderer's failed/old submission
    /// as published.
    submitted_types: Vec<TypeId>,
    /// True only when every producer in the current frame explicitly opts
    /// into a native secondary graphics queue. No role mixing across one
    /// shared render-to-textures submission.
    independent_producers_only: bool,
}

impl RendererRegistry {
    pub(super) fn render_passes_submitted(&mut self) {
        for type_id in self.submitted_types.drain(..) {
            if let Some(renderer) = self.renderers.get_mut(&type_id) {
                renderer.render_passes_submitted();
            }
        }
    }
    pub(super) fn independent_producers_only(&self) -> bool {
        !self.passes.is_empty() && self.independent_producers_only
    }
    pub(super) fn begin_frame(&mut self) {
        self.passes.clear();
        self.independent_producers_only = true;
        // A previous submit may have failed after draining producer passes.
        // Do not carry its pending acknowledgements to an unrelated frame.
        self.submitted_types.clear();
    }
    pub(super) fn passes(&self) -> &[crate::RendererExtensionPass] {
        &self.passes
    }
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
        let type_id = input.renderer_type();
        let before = self.passes.len();
        let renderer = match self.renderers.entry(type_id) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(input.create_renderer(device, context.clone())?),
        };
        renderer.render(input, device, context, steps)?;
        renderer.drain_render_passes(&mut self.passes);
        if self.passes.len() > before {
            self.independent_producers_only &= renderer.prefers_independent_graphics_queue();
            if !self.submitted_types.contains(&type_id) {
                self.submitted_types.push(type_id);
            }
        }
        Ok(())
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
    fn independent_queue_requires_all_offscreen_producers_to_opt_in() {
        let mut registry = RendererRegistry::default();
        registry.begin_frame();
        assert!(!registry.independent_producers_only());
        registry.passes.push(crate::RendererExtensionPass {
            target: TextureViewId::from_parts(1, 1),
            gpu_work_tag: None,
            resolve_target: None,
            render_pass: RenderPassId::from_parts(1, 1),
            commands: Vec::new().into(),
            color_load_op: LoadOp::Load,
            depth_attachment: None,
        });
        registry.independent_producers_only = true;
        assert!(registry.independent_producers_only());
        registry.independent_producers_only = false;
        assert!(!registry.independent_producers_only());
        registry.begin_frame();
        assert!(!registry.independent_producers_only());
    }

    #[test]
    fn submission_acknowledgements_are_scoped_to_current_producers() {
        use std::sync::{atomic::{AtomicUsize, Ordering}, Arc};
        struct Submitted(Arc<AtomicUsize>);
        impl crate::RendererExtensionRenderer for Submitted {
            fn render_passes_submitted(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
            fn render(
                &mut self, _: &dyn crate::RendererExtension,
                _: &mut dyn ExtensionDevice,
                _: crate::RendererExtensionContext,
                _: &mut Vec<RenderCommand>,
            ) -> crate::Result<()> {
                Ok(())
            }
            fn destroy(&mut self, _: &mut dyn ExtensionDevice) -> crate::Result<()> {
                Ok(())
            }
        }
        let mut registry = RendererRegistry::default();
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let first_id = TypeId::of::<PendingRenderer>();
        let second_id = TypeId::of::<Submitted>();
        registry.renderers.insert(first_id, Box::new(Submitted(first.clone())));
        registry.renderers.insert(second_id, Box::new(Submitted(second.clone())));
        registry.submitted_types.push(first_id);
        registry.render_passes_submitted();
        assert_eq!(first.load(Ordering::Relaxed), 1);
        assert_eq!(second.load(Ordering::Relaxed), 0);
        registry.render_passes_submitted();
        assert_eq!(first.load(Ordering::Relaxed), 1);
        registry.submitted_types.push(second_id);
        registry.begin_frame();
        registry.render_passes_submitted();
        assert_eq!(second.load(Ordering::Relaxed), 0);
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
