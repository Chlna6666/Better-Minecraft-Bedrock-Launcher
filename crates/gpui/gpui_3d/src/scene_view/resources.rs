mod factory;
use super::{
    ALBEDO_BINDING, DRAW_BINDING, FRAME_BINDING, INSTANCE_BINDING, LIGHT_BINDING,
    METALLIC_ROUGHNESS_BINDING, NORMAL_MAP_BINDING, OCCLUSION_BINDING, SAMPLER_BINDING,
    TANGENT_BINDING, VERTEX_BINDING, VISIBLE_INSTANCE_BINDING,
};
use crate::TextureSampling;
use anyhow::anyhow;
use gfx_core::{
    AddressMode, BackendKind, BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindGroupLayoutId,
    BindingType, BlendMode, CompareFunction, DepthState, Extent2d, FilterMode, Format,
    MemoryLocation, PipelineLayoutDescriptor, PipelineLayoutId, PrimitiveTopology,
    RenderPipelineDescriptor, RenderPipelineId, SamplerDescriptor, SamplerId, ShaderBinary,
    ShaderModuleDescriptor, ShaderModuleId, ShaderStage, ShaderStages, TextureDescriptor,
    TextureDimension, TextureId, TextureUsage, TextureViewDescriptor, TextureViewId,
};
use gpui::ExtensionDevice;
use gpui::RendererExtensionContext;

#[path = "../sampled_table_capacity.rs"]
mod sampled_table_capacity;

pub(super) struct RendererResources {
    pass: PassKind,
    owns_layout: bool,
    pub(super) cull_shader: Option<ShaderModuleId>,
    pub(super) singleton_cull_shader: Option<ShaderModuleId>,
    bind_group_layout: Option<BindGroupLayoutId>,
    pipeline_layout: Option<PipelineLayoutId>,
    vertex_shader: Option<ShaderModuleId>,
    voxel_vertex_shader: Option<ShaderModuleId>,
    generated_vertex_shader: Option<ShaderModuleId>,
    fragment_shader: Option<ShaderModuleId>,
    voxel_fragment_shader: Option<ShaderModuleId>,
    pipelines: [Option<RenderPipelineId>; 18],
    // A handle generation independent of per-view geometry revisions.
    pub(super) pipeline_revision: u64,
    sampled: Option<SampledPipelines>,
    /// A previous native scene PSO may still be referenced by an in-flight
    /// independent producer queue. Defer destruction until GPU fence proof.
    awaiting_pipeline_gpu: Vec<RenderPipelineId>,
    ready_pipeline_release: Vec<RenderPipelineId>,
    fallback_texture: Option<TextureId>,
    fallback_texture_view: Option<TextureViewId>,
    samplers: [Option<SamplerId>; 6],
}

struct SampledPipelines {
    capacity: u32,
    owns_layout: bool,
    geometry_layout: Option<BindGroupLayoutId>,
    layout: Option<BindGroupLayoutId>,
    pipeline_layout: Option<PipelineLayoutId>,
    fragment: Option<ShaderModuleId>,
    voxel_fragment: Option<ShaderModuleId>,
    pipelines: [Option<RenderPipelineId>; 18],
}

impl SampledPipelines {
    fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for pipeline in &mut self.pipelines {
            if let Some(id) = *pipeline {
                device.destroy_render_pipeline(id)?;
                *pipeline = None;
            }
        }
        if let Some(id) = self.fragment { device.destroy_shader_module(id)?; self.fragment = None; }
        if let Some(id) = self.voxel_fragment { device.destroy_shader_module(id)?; self.voxel_fragment = None; }
        if self.owns_layout {
            if let Some(id) = self.pipeline_layout { device.destroy_pipeline_layout(id)?; self.pipeline_layout = None; }
            if let Some(id) = self.layout { device.destroy_bind_group_layout(id)?; self.layout = None; }
            if let Some(id) = self.geometry_layout { device.destroy_bind_group_layout(id)?; self.geometry_layout = None; }
        }
        Ok(())
    }
}

/// Bounded cache of complete sampled pipeline families, one per compiled
/// capacity tier. Old layouts remain valid for queued commands and scene
/// descriptor generations without forcing GPU idle. Descriptor snapshots
/// themselves retire by fence; only reusable PSOs/layouts stay cached.
/// The cache must be destroyed before the render passes and shared shaders.
#[derive(Default)]
pub(super) struct SampledTierCache {
    maximum: Option<u32>,
    observed_frame: Option<std::time::Instant>,
    candidate: Option<(u32, std::time::Instant)>,
    last_attempt: Option<std::time::Instant>,
    retained: Vec<SampledFamily>,
    active_readers: FamilyReaders,
}

#[derive(Default)]
struct FamilyReaders {
    queued: bool,
    untracked: bool,
    fences: std::collections::HashSet<gfx_core::SubmissionId>,
}

impl FamilyReaders {
    fn submitted(&mut self, submission: Option<gfx_core::SubmissionId>) {
        if !std::mem::take(&mut self.queued) { return; }
        if let Some(id) = submission { self.fences.insert(id); }
        else { self.untracked = true; }
    }
    fn accepted(&mut self, id: gfx_core::SubmissionId) {
        if std::mem::take(&mut self.untracked) { self.fences.insert(id); }
    }
    fn ready(&self) -> bool { !self.queued && !self.untracked && self.fences.is_empty() }
}

struct SampledFamily {
    capacity: u32,
    contexts: Vec<(gfx_core::RenderPassId, Format)>,
    passes: Vec<SampledPipelines>,
    readers: FamilyReaders,
    retiring: bool,
}

impl SampledTierCache {
    /// One working-set observation per GPU owner frame, not per SceneView.
    pub(super) fn begin_frame(&mut self, now: std::time::Instant) -> bool {
        if self.observed_frame == Some(now) { return false; }
        self.observed_frame = Some(now);
        true
    }

    /// Select the smallest feasible shrink tier. Stable higher slot indices
    /// may block an aggressive shrink but still permit an intermediate one.
    pub(super) fn feasible_target(current: u32, target: u32, mut feasible: impl FnMut(u32) -> bool) -> Option<u32> {
        if target >= current { return feasible(target).then_some(target); }
        sampled_table_capacity::SAMPLED_TABLE_TIERS.into_iter()
            .filter(|&tier| tier >= target && tier < current)
            .find(|&tier| feasible(tier))
    }

    /// Grow when the working set exceeds the current capacity; shrink only
    /// after ten seconds below 25% utilization, keeping 2x live headroom.
    /// Failed native allocations are throttled without a CPU/GPU wait.
    pub(super) fn suggest(&mut self, current: u32, demand: usize, now: std::time::Instant) -> Option<u32> {
        let maximum = *self.maximum.get_or_insert(current);
        let Some(target) = sampled_capacity_target(current, maximum, demand) else {
            self.candidate = None;
            return None;
        };
        let since = match self.candidate {
            Some((previous, since)) if previous == target => since,
            _ => { self.candidate = Some((target, now)); now }
        };
        if target < current && now.saturating_duration_since(since) < std::time::Duration::from_secs(10) {
            return None;
        }
        if self.last_attempt.is_some_and(|last| now.saturating_duration_since(last) < std::time::Duration::from_secs(2)) {
            return None;
        }
        self.last_attempt = Some(now);
        Some(target)
    }

    /// Stage every related native pass before changing any active pipeline.
    /// A failed child leaves the complete old family active.
    pub(super) fn switch(
        &mut self, device: &mut dyn ExtensionDevice,
        renderers: &mut [&mut RendererResources], contexts: &[RendererExtensionContext],
        capacity: u32,
    ) -> gpui::Result<bool> {
        let Some(current) = renderers.first().and_then(|renderer| renderer.sampled_table_capacity()) else { return Ok(false); };
        if current == capacity || renderers.len() != contexts.len()
            || renderers.iter().any(|renderer| renderer.sampled_table_capacity() != Some(current))
            || !sampled_table_capacity::SAMPLED_TABLE_TIERS.contains(&capacity)
            || capacity > self.maximum.unwrap_or(current)
        { return Ok(false); }
        let context_keys = contexts.iter().map(|context| (context.render_pass(), context.color_format())).collect::<Vec<_>>();
        let (next, next_readers) = if let Some(index) = self.retained.iter().position(|family| !family.retiring && family.capacity == capacity && family.contexts == context_keys) {
            // A cached PSO must never be used with a different render pass.
            // Leave the current family intact on incompatible context changes.
            let family = self.retained.swap_remove(index);
            (family.passes, family.readers)
        } else {
            let mut staged = Vec::with_capacity(renderers.len());
            for (index, (renderer, context)) in renderers.iter().zip(contexts).enumerate() {
                let inherited = if index == 0 { None } else { staged.first() };
                match renderer.build_sampled_tier(device, context, capacity, inherited) {
                    Ok(Some(next)) => staged.push(next),
                    result => {
                        for mut pass in staged.into_iter().rev() { pass.destroy(device)?; }
                        return result.map(|_| false);
                    }
                }
            }
            (staged, FamilyReaders::default())
        };
        let mut previous = Vec::with_capacity(renderers.len());
        for (renderer, next) in renderers.iter_mut().zip(next) {
            previous.push(renderer.sampled.replace(next).expect("validated sampled family"));
            renderer.pipeline_revision = renderer.pipeline_revision.wrapping_add(1);
        }
        self.retained.push(SampledFamily {
            capacity: current, contexts: context_keys, passes: previous,
            readers: std::mem::replace(&mut self.active_readers, next_readers),
            retiring: false,
        });
        self.candidate = None;
        Ok(true)
    }

    pub(super) fn queue_use(&mut self) { self.active_readers.queued = true; }

    pub(super) fn submitted(&mut self, id: Option<gfx_core::SubmissionId>) {
        self.active_readers.submitted(id);
        for family in &mut self.retained { family.readers.submitted(id); }
    }

    pub(super) fn abandon(&mut self, may_have_submitted: bool) {
        if may_have_submitted { self.submitted(None); }
        else {
            self.active_readers.queued = false;
            for family in &mut self.retained { family.readers.queued = false; }
        }
    }

    pub(super) fn accepted(&mut self, id: gfx_core::SubmissionId) {
        self.active_readers.accepted(id);
        for family in &mut self.retained { family.readers.accepted(id); }
    }

    pub(super) fn completed(&mut self, id: gfx_core::SubmissionId) {
        self.active_readers.fences.remove(&id);
        for family in &mut self.retained { family.readers.fences.remove(&id); }
    }

    pub(super) fn idle(&mut self) {
        self.active_readers.fences.clear();
        self.active_readers.untracked = false;
        for family in &mut self.retained {
            family.readers.fences.clear();
            family.readers.untracked = false;
        }
    }

    pub(super) fn needs_completion(&self) -> bool {
        self.retained.iter().any(|family| !family.readers.ready())
    }

    /// Release old families only when no scene can publish an old table
    /// and every actual GPU reader has completed. The Vulkan backend
    /// independently defers destruction of native objects.
    pub(super) fn release_completed(&mut self, device: &mut dyn ExtensionDevice,
        mut referenced: impl FnMut(BindGroupLayoutId) -> bool) -> gpui::Result<()> {
        let mut index = 0;
        while index < self.retained.len() {
            let family = &mut self.retained[index];
            let layout = family.passes.first().and_then(|pass| pass.layout);
            if !family.readers.ready() || layout.is_some_and(&mut referenced) {
                index += 1;
                continue;
            }
            family.retiring = true;
            for pass in family.passes.iter_mut().rev() { pass.destroy(device)?; }
            self.retained.swap_remove(index);
        }
        Ok(())
    }

    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for family in &mut self.retained {
            for pass in family.passes.iter_mut().rev() { pass.destroy(device)?; }
        }
        self.retained.clear();
        Ok(())
    }
}

fn sampled_capacity_target(current: u32, maximum: u32, demand: usize) -> Option<u32> {
    let demand = demand.max(1);
    if demand <= current as usize && demand > current as usize / 4 { return None; }
    let headroom = demand.saturating_mul(2);
    let target = sampled_table_capacity::SAMPLED_TABLE_TIERS.into_iter()
        .filter(|&tier| tier <= maximum)
        .find(|&tier| tier as usize >= headroom)
        .unwrap_or(maximum);
    if target == current || (demand <= current as usize && target > current) { None } else { Some(target) }
}

#[cfg(test)]
mod sampled_capacity_policy_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn family_reader_proof_is_generation_local() {
        let a = gfx_core::SubmissionId::from_parts(1, 1);
        let b = gfx_core::SubmissionId::from_parts(2, 1);
        let mut old = FamilyReaders::default();
        old.queued = true;
        old.submitted(Some(a));
        let mut next = FamilyReaders::default();
        next.queued = true;
        next.submitted(None);
        next.accepted(b);
        assert!(!old.ready());
        assert!(!next.ready());
        old.fences.remove(&b);
        assert!(!old.ready());
        old.fences.remove(&a);
        assert!(old.ready());
        assert!(!next.ready());
    }

    #[test]
    fn sampled_capacity_policy_hysteresis_and_growth() {
        let mut policy = SampledTierCache::default();
        let now = Instant::now();
        assert_eq!(policy.suggest(1024, 24, now), None);
        assert_eq!(policy.suggest(1024, 24, now + Duration::from_secs(9)), None);
        assert_eq!(policy.suggest(1024, 24, now + Duration::from_secs(10)), Some(64));
        assert_eq!(policy.suggest(64, 70, now + Duration::from_secs(12)), Some(256));
        assert_eq!(sampled_capacity_target(256, 1024, 160), None);
        assert_eq!(sampled_capacity_target(1024, 1024, 4096), None);
        assert_eq!(SampledTierCache::feasible_target(1024, 64, |tier| tier >= 256), Some(256));
        assert_eq!(SampledTierCache::feasible_target(1024, 64, |tier| tier >= 1024), None);
        assert!(policy.begin_frame(now));
        assert!(!policy.begin_frame(now));
    }
}

impl RendererResources {
    pub(super) fn sampled_table_capacity(&self) -> Option<u32> {
        self.sampled.as_ref().map(|sampled| sampled.capacity)
    }

    pub(super) fn sampled_table_layout(&self) -> Option<BindGroupLayoutId> {
        self.sampled.as_ref().and_then(|sampled| sampled.layout)
    }

    pub(super) fn draw_bind_group_layout(&self, sampled: bool) -> gpui::Result<BindGroupLayoutId> {
        if sampled {
            self.sampled.as_ref().and_then(|sampled| sampled.geometry_layout)
                .ok_or_else(|| anyhow!("sampled geometry layout unavailable"))
        } else { self.bind_group_layout() }
    }

    pub(in crate::scene_view) fn disable_sampled(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(mut sampled) = self.sampled.take() { sampled.destroy(device)?; }
        Ok(())
    }

    /// Every related render pass shares one immutable geometry/table layout.
    /// A prototype without a sampled pipeline forces its children to Legacy.
    pub(in crate::scene_view) fn try_enable_sampled(
        &mut self, device: &mut dyn ExtensionDevice, context: &RendererExtensionContext,
        max_capacity: u32, prototype: Option<&Self>,
    ) -> gpui::Result<()> {
        if context.backend_kind() != BackendKind::Vulkan
            || std::env::var_os("GPUI_3D_BINDLESS").is_none_or(|value| value != "1")
        {
            return Ok(());
        }
        let inherited = prototype.and_then(|prototype| prototype.sampled.as_ref());
        if prototype.is_some() && inherited.is_none() { return Ok(()); }
        debug_assert!(self.sampled.is_none());
        if let Some(inherited) = inherited {
            // A child cannot choose a different shader ABI or layout.
            self.try_sampled_tier(device, context, inherited.capacity, Some(inherited))?;
        } else {
            let mut maximum = max_capacity;
            let device_limit = device.sampled_texture_table_capacity();
            while let Some(capacity) = sampled_table_capacity::sampled_table_capacity(device_limit, maximum) {
                if self.try_sampled_tier(device, context, capacity, None)? {
                    break;
                }
                maximum = capacity - 1;
            }
        }
        Ok(())
    }

    fn try_sampled_tier(
        &mut self, device: &mut dyn ExtensionDevice, context: &RendererExtensionContext,
        capacity: u32, inherited: Option<&SampledPipelines>,
    ) -> gpui::Result<bool> {
        let next = self.build_sampled_tier(device, context, capacity, inherited)?;
        if let Some(next) = next {
            self.sampled = Some(next);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn build_sampled_tier(
        &self, device: &mut dyn ExtensionDevice, context: &RendererExtensionContext,
        capacity: u32, inherited: Option<&SampledPipelines>,
    ) -> gpui::Result<Option<SampledPipelines>> {
        let mut sampled = SampledPipelines {
            capacity, owns_layout: inherited.is_none(),
            geometry_layout: None, layout: None, pipeline_layout: None, fragment: None, voxel_fragment: None,
            pipelines: [None; 18],
        };
        let result: gpui::Result<()> = (|| {
            if let Some(inherited) = inherited {
                sampled.layout = inherited.layout;
                sampled.geometry_layout = inherited.geometry_layout;
                sampled.pipeline_layout = inherited.pipeline_layout;
            } else {
                let layout = device.create_sampled_texture_table_layout(capacity)?;
                sampled.layout = Some(layout);
                let geometry = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                    label: Some("gpui_3d sampled geometry layout".into()),
                    entries: factory::scene_layout_entries(true),
                })?;
                sampled.geometry_layout = Some(geometry);
                sampled.pipeline_layout = Some(device.create_pipeline_layout(&PipelineLayoutDescriptor {
                    label: Some("gpui_3d sampled scene pipeline layout".into()),
                    bind_group_layouts: vec![geometry, layout],
                })?);
            }
            let [fragment, voxel_fragment] = self.pass.entries();
            sampled.fragment = Some(device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui_3d sampled material fragment".into()),
                binary: embedded_shader(context, ShaderStage::Fragment, &format!("{fragment}_bindless_{capacity}"))?,
            })?);
            sampled.voxel_fragment = Some(device.create_shader_module(&ShaderModuleDescriptor {
                label: Some("gpui_3d sampled voxel fragment".into()),
                binary: embedded_shader(context, ShaderStage::Fragment, &format!("{voxel_fragment}_bindless_{capacity}"))?,
            })?);
            // Every slot in the fixed array, including unused slots, must
            // reference a valid initialized sampled image.
            if self.sampled.is_none() {
                let fallback = self.fallback_texture.ok_or_else(|| anyhow!("missing fallback texture"))?;
                device.write_texture_batch(&[gfx_core::TextureWrite {
                    descriptor: gfx_core::TextureWriteDescriptor {
                        texture: fallback, mip_level: 0,
                        layout: gfx_core::TextureDataLayout::new(0, 4, 1)?,
                        origin: gfx_core::Origin2d { x: 0, y: 0 },
                        size: Extent2d::new(1, 1)?,
                    },
                    data: &[255, 255, 255, 255],
                }])?;
            }
            let pipelines = self.create_pipeline_set(device, context, Some(&sampled))?;
            sampled.pipelines = pipelines;
            Ok(())
        })();
        match result {
            Ok(()) => Ok(Some(sampled)),
            Err(error) => {
                sampled.destroy(device)?;
                eprintln!("gpui_3d bindless pass tier {capacity} unavailable: {error:#}");
                Ok(None)
            }
        }
    }

    pub(super) fn rejects_material(&self, material: &crate::Material) -> bool {
        material.alpha_mode == crate::AlphaMode::Blend
            && (self.pass == PassKind::Shadow
                || (self.pass == PassKind::Hdr && material.surface != crate::SurfaceKind::Standard))
    }
    #[cfg(test)]
    pub(super) fn test_pipeline(
        layout: BindGroupLayoutId,
        pipeline: RenderPipelineId,
        fallback: TextureViewId,
    ) -> Self {
        Self {
            pass: PassKind::Color,
            owns_layout: false,
            cull_shader: None,
            singleton_cull_shader: None,
            bind_group_layout: Some(layout),
            pipeline_layout: None,
            vertex_shader: None,
            voxel_vertex_shader: None,
            generated_vertex_shader: None,
            fragment_shader: None,
            voxel_fragment_shader: None,
            pipelines: [Some(pipeline); 18],
            pipeline_revision: 0,
            sampled: None,
            awaiting_pipeline_gpu: Vec::new(),
            ready_pipeline_release: Vec::new(),
            fallback_texture: None,
            fallback_texture_view: Some(fallback),
            samplers: [None; 6],
        }
    }
    pub(super) fn bind_group_layout(&self) -> gpui::Result<BindGroupLayoutId> {
        self.bind_group_layout
            .ok_or_else(|| anyhow!("GPUI 3D resource-set layout is unavailable"))
    }

    pub(super) fn fallback_texture_view(&self) -> gpui::Result<TextureViewId> {
        self.fallback_texture_view
            .ok_or_else(|| anyhow!("GPUI 3D fallback texture view is unavailable"))
    }

    /// Returns a shared sampler for the requested texel filtering and anisotropy.
    ///
    /// Atlas images request nearest-texel filtering so neighbouring skin or map regions cannot
    /// bleed into each other; pixel-art scenes use nearest magnification with linear
    /// minification; other images keep linear filtering.
    pub(super) fn sampler(
        &mut self,
        device: &mut dyn ExtensionDevice,
        sampling: TextureSampling,
        anisotropy_enabled: bool,
    ) -> gpui::Result<SamplerId> {
        let sampler_index = sampler_slot(sampling) + 3 * usize::from(anisotropy_enabled);
        if let Some(sampler) = self.samplers[sampler_index] {
            return Ok(sampler);
        }
        let (mag_filter, min_filter) = sampler_filters(sampling);
        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("gpui_3d material sampler".into()),
            mag_filter,
            min_filter,
            mipmap_filter: FilterMode::Linear,
            anisotropic: anisotropy_enabled,
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            ..Default::default()
        })?;
        self.samplers[sampler_index] = Some(sampler);
        Ok(sampler)
    }

    pub(super) fn replace_pipelines(
        &mut self,
        device: &mut dyn ExtensionDevice,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        let next = self.create_pipeline_set(device, context, None)?;
        let sampled_next = if let Some(sampled) = self.sampled.as_ref() {
            match self.create_pipeline_set(device, context, Some(sampled)) {
                Ok(pipelines) => Some(pipelines),
                Err(error) => {
                    for pipeline in next.into_iter().flatten() {
                        device.destroy_render_pipeline(pipeline)?;
                    }
                    return Err(error);
                }
            }
        } else { None };
        // Replace the two complete PSO generations together.
        self.awaiting_pipeline_gpu.extend(
            std::mem::replace(&mut self.pipelines, next).into_iter().flatten()
        );
        if let (Some(sampled), Some(next)) = (self.sampled.as_mut(), sampled_next) {
            self.awaiting_pipeline_gpu.extend(
                std::mem::replace(&mut sampled.pipelines, next).into_iter().flatten()
            );
        }
        self.pipeline_revision = self.pipeline_revision.wrapping_add(1);
        Ok(())
    }

    fn create_pipeline_set(
        &self, device: &mut dyn ExtensionDevice, context: &RendererExtensionContext,
        sampled: Option<&SampledPipelines>,
    ) -> gpui::Result<[Option<RenderPipelineId>; 18]> {
        let mut next = [None; 18];
        for (index, slot) in next.iter_mut().enumerate() {
            if index >= 12 && self.cull_shader.is_none() {
                continue;
            }
            let mode = if (index / 3) % 2 == 0 {
                PipelineMode::Opaque
            } else {
                PipelineMode::Blend
            };
            let face = match index % 3 {
                0 => None,
                1 => Some(gfx_core::FrontFace::Ccw),
                _ => Some(gfx_core::FrontFace::Cw),
            };
            match create_pipeline(device, self, context, mode, index / 6, face, sampled) {
                Ok(pipeline) => *slot = Some(pipeline),
                Err(error) => {
                    for pipeline in next.into_iter().flatten() {
                        device.destroy_render_pipeline(pipeline)?;
                    }
                    return Err(error);
                }
            }
        }
        Ok(next)
    }

    pub(super) fn has_retired_pipelines(&self) -> bool {
        !self.awaiting_pipeline_gpu.is_empty()
    }

    pub(super) fn gpu_work_completed(&mut self) {
        self.ready_pipeline_release.append(&mut self.awaiting_pipeline_gpu);
    }

    pub(super) fn release_completed(
        &mut self, device: &mut dyn ExtensionDevice,
    ) -> gpui::Result<()> {
        while let Some(&pipeline) = self.ready_pipeline_release.last() {
            device.destroy_render_pipeline(pipeline)?;
            self.ready_pipeline_release.pop();
        }
        Ok(())
    }

    pub(super) fn pipeline(
        &self,
        voxel: bool,
        blended: bool,
        front_face: Option<gfx_core::FrontFace>,
    ) -> gpui::Result<RenderPipelineId> {
        self.pipeline_variant(usize::from(voxel), blended, front_face, false)
    }
    pub(super) fn generated_pipeline(
        &self,
        blended: bool,
        front_face: Option<gfx_core::FrontFace>,
    ) -> gpui::Result<RenderPipelineId> {
        self.pipeline_variant(2, blended, front_face, false)
    }
    pub(super) fn sampled_pipeline(
        &self, variant: usize, blended: bool, front_face: Option<gfx_core::FrontFace>,
    ) -> gpui::Result<RenderPipelineId> {
        self.pipeline_variant(variant, blended, front_face, true)
    }

    fn pipeline_variant(
        &self,
        variant: usize,
        blended: bool,
        front_face: Option<gfx_core::FrontFace>,
        sampled: bool,
    ) -> gpui::Result<RenderPipelineId> {
        let face = match front_face {
            None => 0,
            Some(gfx_core::FrontFace::Ccw) => 1,
            Some(gfx_core::FrontFace::Cw) => 2,
        };
        let pipelines = if sampled {
            &self.sampled.as_ref().ok_or_else(|| anyhow!("sampled pipeline unavailable"))?.pipelines
        } else { &self.pipelines };
        pipelines[variant * 6 + usize::from(blended) * 3 + face]
            .ok_or_else(|| anyhow!("GPUI 3D draw pipeline is unavailable"))
    }
    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(shader) = self.cull_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.singleton_cull_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        for pipeline in &mut self.pipelines {
            if let Some(pipeline) = pipeline.take() {
                device.destroy_render_pipeline(pipeline)?;
            }
        }
        for pipeline in self.awaiting_pipeline_gpu.drain(..)
            .chain(self.ready_pipeline_release.drain(..))
        {
            device.destroy_render_pipeline(pipeline)?;
        }
        if let Some(mut sampled) = self.sampled.take() {
            sampled.destroy(device)?;
        }
        if let Some(shader) = self.generated_vertex_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.voxel_fragment_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.fragment_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.vertex_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(shader) = self.voxel_vertex_shader.take() {
            device.destroy_shader_module(shader)?;
        }
        if let Some(view) = self.fallback_texture_view.take() {
            device.destroy_texture_view(view)?;
        }
        if let Some(texture) = self.fallback_texture.take() {
            device.destroy_texture(texture)?;
        }
        for sampler in &mut self.samplers {
            if let Some(sampler) = sampler.take() {
                device.destroy_sampler(sampler)?;
            }
        }
        if self.owns_layout
            && let Some(layout) = self.pipeline_layout.take()
        {
            device.destroy_pipeline_layout(layout)?;
        }
        if self.owns_layout
            && let Some(layout) = self.bind_group_layout.take()
        {
            device.destroy_bind_group_layout(layout)?;
        }
        Ok(())
    }
}

/// Stable slot index for one material sampling mode inside the sampler cache.
const fn sampler_slot(sampling: TextureSampling) -> usize {
    match sampling {
        TextureSampling::Linear => 0,
        TextureSampling::Nearest => 1,
        TextureSampling::PixelArt => 2,
    }
}

/// Magnification and minification filters for one material sampling mode.
const fn sampler_filters(sampling: TextureSampling) -> (FilterMode, FilterMode) {
    match sampling {
        TextureSampling::Linear => (FilterMode::Linear, FilterMode::Linear),
        TextureSampling::Nearest => (FilterMode::Nearest, FilterMode::Nearest),
        // Nearest magnification keeps pixel-art texels sharp up close; linear minification with
        // mip blending avoids distant shimmer and moire without re-blurring magnified texels.
        TextureSampling::PixelArt => (FilterMode::Nearest, FilterMode::Linear),
    }
}

/// Resolves the build-generated scene-view shader for the active backend.
///
/// `build.rs` compiles every entry point for every backend the platform supports, so
/// this is a lookup rather than a shader compile while a scene view is being created.
/// The arms follow the platform, not this crate's Cargo features, because the host
/// application decides which backend renders.
pub(super) fn embedded_shader(
    context: &RendererExtensionContext,
    stage: ShaderStage,
    entry_point: &str,
) -> gpui::Result<ShaderBinary> {
    let artifact = match context.backend_kind() {
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        BackendKind::OpenGl => super::artifacts::scene_view_opengl_shader(entry_point),
        #[cfg(target_os = "windows")]
        BackendKind::Dx11 => super::artifacts::scene_view_dx11_shader(entry_point),
        #[cfg(target_os = "windows")]
        BackendKind::Dx12 => super::artifacts::scene_view_dx12_shader(entry_point),
        #[cfg(target_os = "macos")]
        BackendKind::Metal => super::artifacts::scene_view_metal_shader(entry_point),
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        BackendKind::Vulkan => super::artifacts::scene_view_vulkan_shader(entry_point),
        _ => None,
    };

    let artifact = artifact.ok_or_else(|| {
        anyhow!(
            "gpui_3d scene-view shader `{entry_point}` is not embedded for backend {:?}",
            context.backend_kind()
        )
    })?;

    Ok(artifact.to_binary(stage, entry_point)?)
}

#[derive(Clone, Copy)]
enum PipelineMode {
    Opaque,
    Blend,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PassKind {
    Color,
    Hdr,
    AttributesMrt,
    Shadow,
    // Offline MRT-vs-single-target regression references. Shader entries
    // already exist but these passes are never created by production code.
    #[cfg(test)]
    Surface,
    #[cfg(test)]
    Albedo,
}
impl PassKind {
    fn entries(self) -> [&'static str; 2] {
        match self {
            Self::Color => ["fs_main", "fs_voxel"],
            Self::Hdr => ["fs_hdr", "fs_voxel_capture"],
            Self::AttributesMrt => ["fs_scene_attributes_mrt", "fs_voxel_attributes_mrt"],
            Self::Shadow => ["fs_scene_shadow", "fs_voxel_shadow"],
            #[cfg(test)]
            Self::Surface => ["fs_scene_surface", "fs_voxel_surface"],
            #[cfg(test)]
            Self::Albedo => ["fs_scene_albedo", "fs_voxel_albedo"],
        }
    }
    fn is_attribute(self) -> bool {
        matches!(self, Self::AttributesMrt | Self::Shadow)
            || {
                #[cfg(test)]
                { matches!(self, Self::Surface | Self::Albedo) }
                #[cfg(not(test))]
                { false }
            }
    }
}

fn create_pipeline(
    device: &mut dyn ExtensionDevice,
    resources: &RendererResources,
    context: &RendererExtensionContext,
    mode: PipelineMode,
    variant: usize,
    front_face: Option<gfx_core::FrontFace>,
    sampled: Option<&SampledPipelines>,
) -> gpui::Result<RenderPipelineId> {
    let voxel = variant != 0;
    let fragment = if let Some(sampled) = sampled {
        if voxel { sampled.voxel_fragment } else { sampled.fragment }
    } else if voxel { resources.voxel_fragment_shader } else { resources.fragment_shader };
    let entry = resources.pass.entries()[usize::from(voxel)];
    let fragment_entry = if let Some(sampled) = sampled { format!("{entry}_bindless_{}", sampled.capacity) } else { entry.into() };
    let (label, blend, depth_write) = match (mode, voxel) {
        (PipelineMode::Opaque, false) => ("gpui_3d opaque pipeline", false, true),
        (PipelineMode::Blend, false) => ("gpui_3d blended pipeline", true, false),
        (PipelineMode::Opaque, true) => ("gpui_3d voxel opaque pipeline", false, true),
        (PipelineMode::Blend, true) => ("gpui_3d voxel blended pipeline", true, false),
    };
    Ok(device.create_render_pipeline(
        &RenderPipelineDescriptor {
            label: Some(label.into()),
            vertex_shader: if variant == 2 {
                resources.generated_vertex_shader
            } else if voxel {
                resources.voxel_vertex_shader
            } else {
                resources.vertex_shader
            }
            .ok_or_else(|| anyhow!("GPUI 3D vertex shader is unavailable"))?,
            vertex_entry_point: if variant == 2 {
                "vs_generated_voxel"
            } else if voxel {
                if resources.cull_shader.is_some() {
                    "vs_voxel_gpu"
                } else {
                    "vs_voxel"
                }
            } else {
                "vs_main"
            }
            .into(),
            fragment_shader: fragment.ok_or_else(|| anyhow!("GPUI 3D fragment shader is unavailable"))?,
            fragment_entry_point: fragment_entry,
            vertex_buffers: Vec::new(),
            render_pass: context.render_pass(),
            pipeline_layout: sampled.map_or(resources.pipeline_layout, |sampled| sampled.pipeline_layout),
            color_targets: if resources.pass == PassKind::AttributesMrt {
                vec![
                    gfx_core::ColorTargetState::new(context.color_format(), BlendMode::Replace),
                    gfx_core::ColorTargetState::new(context.color_format(), BlendMode::Replace),
                ]
            } else {
                vec![gfx_core::ColorTargetState::new(
                    context.color_format(),
                    if blend && !resources.pass.is_attribute() {
                        BlendMode::PremultipliedAlpha
                    } else {
                        BlendMode::Replace
                    },
                )]
            },
            primitive_topology: PrimitiveTopology::TriangleList,
            raster: gfx_core::RasterState {
                front_face: front_face.unwrap_or_default(),
                cull_mode: front_face.map(|_| gfx_core::Face::Back),
            },
            depth_state: Some(DepthState {
                compare: CompareFunction::LessEqual,
                write_enabled: depth_write || resources.pass.is_attribute(),

                stencil: None,
            }),
        },
        context.viewport(),
    )?)
}

#[cfg(test)]
mod retired_pipeline_tests {
    use super::*;
    #[test]
    fn replaced_scene_pipeline_generations_wait_for_gpu_completion() {
        let pipeline = RenderPipelineId::from_parts(3, 1);
        let layout = BindGroupLayoutId::from_parts(1, 1);
        let view = TextureViewId::from_parts(1, 1);
        let mut resources = RendererResources::test_pipeline(layout, pipeline, view);
        assert!(!resources.has_retired_pipelines());
        // No device required: handles are never freed in this ownership test.
        resources.awaiting_pipeline_gpu.push(RenderPipelineId::from_parts(4, 1));
        assert!(resources.has_retired_pipelines());
        resources.gpu_work_completed();
        assert!(!resources.has_retired_pipelines());
        assert_eq!(resources.ready_pipeline_release, vec![RenderPipelineId::from_parts(4, 1)]);
    }
}

#[cfg(test)]
#[test]
fn sampled_table_tiers_respect_device_and_application_limits() {
    use sampled_table_capacity::sampled_table_capacity as select;
    assert_eq!(select(68, u32::MAX), None);
    assert_eq!(select(69, u32::MAX), Some(64));
    assert_eq!(select(260, u32::MAX), Some(64));
    assert_eq!(select(261, u32::MAX), Some(256));
    assert_eq!(select(1029, u32::MAX), Some(1024));
    assert_eq!(select(8192, u32::MAX), Some(4096));
    assert_eq!(select(8192, 64), Some(64));
}
