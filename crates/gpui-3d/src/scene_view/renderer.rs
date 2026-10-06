use super::{SceneView, SceneViewId, resources::RendererResources, scene::SceneResources};
use anyhow::{anyhow, bail};
use gfx_core::{
    BackendKind, ExtensionDevice, MemoryTrimLevel, RenderPassId, RenderStepDescriptor,
};
use gpui::{RendererExtension, RendererExtensionContext, RendererExtensionRenderer};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const SCENE_MEMORY_TRIM_IDLE: Duration = Duration::from_secs(1);

fn should_trim_scene(level: MemoryTrimLevel, last_used_at: Instant, now: Instant) -> bool {
    match level {
        MemoryTrimLevel::Light => false,
        MemoryTrimLevel::Moderate => {
            now.saturating_duration_since(last_used_at) >= SCENE_MEMORY_TRIM_IDLE
        }
        MemoryTrimLevel::Aggressive => true,
    }
}
pub(super) struct Renderer {
    backend: BackendKind,
    render_pass: RenderPassId,
    color_format: gfx_core::Format,
    target_extent: gfx_core::Extent2d,
    resources: RendererResources,
    scenes: HashMap<SceneViewId, SceneResources>,
    frame_time: Option<Instant>,
    frame_index: u64,
}

impl Renderer {
    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
    ) -> gpui::Result<Self> {
        let resources = RendererResources::new(device, &context)?;
        Ok(Self {
            backend: context.backend_kind(),
            render_pass: context.render_pass(),
            color_format: context.color_format(),
            target_extent: context.viewport(),
            resources,
            scenes: HashMap::new(),
            frame_time: None,
            frame_index: 0,
        })
    }

    fn sync_pipeline(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        if self.backend != context.backend_kind() {
            bail!("GPUI 3D renderer backend changed during a window lifetime")
        }
        let next_target_extent = context.viewport();
        if self.render_pass == context.render_pass()
            && self.color_format == context.color_format()
            && self.target_extent == next_target_extent
        {
            return Ok(());
        }
        self.resources.replace_pipelines(device, context)?;
        self.render_pass = context.render_pass();
        self.color_format = context.color_format();
        self.target_extent = next_target_extent;
        Ok(())
    }

    fn begin_frame(&mut self, frame_time: Instant) {
        if self.frame_time != Some(frame_time) {
            self.frame_time = Some(frame_time);
            self.frame_index = self.frame_index.wrapping_add(1);
        }
    }

    fn retire_idle_scenes(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let stale = self
            .scenes
            .iter()
            .filter_map(|(id, scene)| {
                (self.frame_index.saturating_sub(scene.last_used_frame) > 120).then_some(*id)
            })
            .collect::<Vec<_>>();
        self.destroy_scenes(device, stale)
    }

    fn trim_scene_cache(
        &mut self,
        device: &mut dyn ExtensionDevice,
        level: MemoryTrimLevel,
    ) -> gpui::Result<()> {
        match level {
            MemoryTrimLevel::Light => Ok(()),
            MemoryTrimLevel::Moderate => {
                let now = Instant::now();
                let stale = self
                    .scenes
                    .iter()
                    .filter_map(|(id, scene)| {
                        should_trim_scene(level, scene.last_used_at, now).then_some(*id)
                    })
                    .collect();
                self.destroy_scenes(device, stale)
            }
            MemoryTrimLevel::Aggressive => {
                let stale = self.scenes.keys().copied().collect();
                self.destroy_scenes(device, stale)
            }
        }
    }

    fn destroy_scenes(
        &mut self,
        device: &mut dyn ExtensionDevice,
        ids: Vec<SceneViewId>,
    ) -> gpui::Result<()> {
        for id in ids {
            if let Some(mut scene) = self.scenes.remove(&id) {
                scene.destroy(device)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{SCENE_MEMORY_TRIM_IDLE, should_trim_scene};
    use gfx_core::MemoryTrimLevel;
    use std::time::{Duration, Instant};

    #[test]
    fn memory_trim_preserves_recent_scenes_until_aggressive_trim() {
        let now = Instant::now();
        let recent = now - SCENE_MEMORY_TRIM_IDLE + Duration::from_millis(1);
        let idle = now - SCENE_MEMORY_TRIM_IDLE;

        assert!(!should_trim_scene(MemoryTrimLevel::Light, idle, now));
        assert!(!should_trim_scene(
            MemoryTrimLevel::Moderate,
            recent,
            now
        ));
        assert!(should_trim_scene(MemoryTrimLevel::Moderate, idle, now));
        assert!(should_trim_scene(
            MemoryTrimLevel::Aggressive,
            recent,
            now
        ));
    }
}

impl RendererExtensionRenderer for Renderer {
    fn render(
        &mut self,
        extension: &dyn RendererExtension,
        device: &mut dyn ExtensionDevice,
        context: RendererExtensionContext,
        steps: &mut Vec<RenderStepDescriptor>,
    ) -> gpui::Result<()> {
        let scene_view = extension
            .downcast_ref::<SceneView>()
            .ok_or_else(|| anyhow!("GPUI 3D renderer received an unexpected extension type"))?;
        self.sync_pipeline(device, &context)?;
        self.begin_frame(context.frame_time());
        self.retire_idle_scenes(device)?;
        if !self.scenes.contains_key(&scene_view.id) {
            self.scenes
                .insert(scene_view.id, SceneResources::new(device)?);
        }
        let scene = self
            .scenes
            .get_mut(&scene_view.id)
            .ok_or_else(|| anyhow!("GPUI 3D scene-view state was not initialized"))?;
        scene.last_used_frame = self.frame_index;
        scene.last_used_at = Instant::now();
        scene.prepare(device, &mut self.resources, scene_view, &context)?;
        scene.append_steps(&self.resources, steps)?;
        Ok(())
    }

    fn trim_memory(
        &mut self,
        device: &mut dyn ExtensionDevice,
        level: MemoryTrimLevel,
    ) -> gpui::Result<()> {
        self.trim_scene_cache(device, level)
    }

    fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let scenes = self.scenes.keys().copied().collect();
        self.destroy_scenes(device, scenes)?;
        self.resources.destroy(device)
    }
}
