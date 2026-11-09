//! Material-aware shadow draws share the scene's resident geometry and draw tables.
use super::*;
use crate::{VoxelIllumination, VoxelShading};
use gpui::RendererExtensionContext;

pub(super) struct LightPass {
    frame: BufferId,
    culling: CullResources,
    groups: HashMap<(DrawResourceKey, BufferId, BufferId, BufferId), BindGroupId>,
}
impl LightPass {
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for group in self.groups.into_values() {
            device.destroy_bind_group(group)?;
        }
        self.culling.destroy(device)?;
        device.destroy_buffer(self.frame)?;
        Ok(())
    }
}
impl SceneResources {
    pub(in crate::scene_view) fn light_resources(&self) -> (BufferId, u64, u32) {
        (
            self.light_buffer,
            self.light_capacity as u64 * LIGHT_STRIDE as u64,
            self.prepared.lights.len() as u32,
        )
    }
    pub(in crate::scene_view) fn shadow_steps(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        camera: Camera,
        cascade: usize,
        caster_bounds: Aabb,
        context: &RendererExtensionContext,
    ) -> gpui::Result<Vec<RenderCommand>> {
        let matrix = camera.projection_matrix(1.)? * camera.view()?;
        let mut bytes =
            encode_frame_params(matrix, camera, context.bounds(), context.viewport(), 0., 0);
        for (dst, value) in bytes[144..192].chunks_exact_mut(4).zip(
            VoxelShading {
                illumination: VoxelIllumination::Unlit,
                ..VoxelShading::default()
            }
            .encoded(),
        ) {
            dst.copy_from_slice(&value.to_le_bytes());
        }
        bytes[268..272].copy_from_slice(&2f32.to_le_bytes());
        if self.light_pass[cascade].is_none() {
            let frame = device.create_buffer(&BufferDescriptor {
                label: Some("gpui-3d shadow frame".into()),
                size: FRAME_PARAMS_STRIDE as u64,
                usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
                memory_location: gfx_core::MemoryLocation::GpuOnly,
            })?;
            let culling = match CullResources::create(
                device,
                &self.prepared.draws,
                &self.batches,
                self.backend,
                Some(&self.meshes),
            ) {
                Ok(culling) => culling,
                Err(error) => {
                    let _ = device.destroy_buffer(frame);
                    return Err(error);
                }
            };
            self.light_pass[cascade] = Some(LightPass {
                frame,
                culling,
                groups: HashMap::new(),
            });
        }
        let mut pass = self.light_pass[cascade]
            .take().ok_or_else(|| anyhow!("shadow cascade pass missing"))?;
        let result = (|| {
            device.write_buffer(pass.frame, 0, &bytes)?;
            pass.culling.update(
                device,
                &self.prepared.draws,
                &self.batches,
                Some(&self.meshes),
            )?;
            pass.culling.dispatch_shadow_optimized(
                device,
                renderer
                    .cull_shader
                    .ok_or_else(|| anyhow!("shadow culling requires compute"))?,
                renderer.singleton_cull_shader,
                pass.frame,
                // Full AABB versus receiver volume extruded along the sun:
                // preserves offscreen casters and rejects work that cannot
                // influence this cascade. Light frustum is then tested on GPU.
                caster_bounds,
                self.generated_arguments,
            )?;
            let groups = self.shadow_bindings(device, renderer, &mut pass)?;
            let mut commands = Vec::new();
            self.append_pass_steps(
                renderer,
                true,
                &mut commands,
                Some((&groups, pass.culling.arguments)),
            )?;
            Ok(commands)
        })();
        self.light_pass[cascade] = Some(pass);
        result
    }
    fn shadow_bindings(
        &self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        pass: &mut LightPass,
    ) -> gpui::Result<Vec<BindGroupId>> {
        // Shadow DrawIndirect explicitly excludes alpha-blended materials.
        // Build and cache BindGroups only for actual depth casters; transparent
        // water/glass batches can never use their binding in append_pass_steps.
        // Retain a positional slot so the existing stable batch indexing stays
        // unchanged, without an additional CPU remapping layer.
        let mut groups = Vec::<Option<BindGroupId>>::with_capacity(self.batches.len());
        let mut active = HashSet::new();
        let fallback = renderer.fallback_texture_view()?;
        for (index, batch) in self.batches.iter().enumerate() {
            let draw = &self.prepared.draws[batch.first_instance];
            if draw.material.alpha_mode == AlphaMode::Blend {
                groups.push(None);
                continue;
            }
            let mesh = &self.meshes[&mesh_key(draw, true)];
            let mut key = self.draw_resource_keys[index];
            key.visible_instances = Some(pass.culling.visible_instances);
            key.shadow = None;
            let key = (
                key,
                self.draw_buffer,
                self.instance_buffer,
                self.draw_slot_buffer,
            );
            active.insert(key);
            let group = if let Some(group) = pass.groups.get(&key) {
                *group
            } else {
                let view = |id: Option<TextureAssetId>| {
                    id.and_then(|id| self.textures.get(&id))
                        .map_or(fallback, |texture| texture.view)
                };
                let group = create_draw_resource_set(
                    device,
                    DrawBindingResources {
                        shadow: None,
                        bind_group_layout: renderer.bind_group_layout()?,
                        mesh,
                        draw_buffer: self.draw_buffer,
                        draw_capacity: self.draw_capacity,
                        instance_buffer: self.instance_buffer,
                        instance_count: self.instance_capacity,
                        light_buffer: self.light_buffer,
                        light_capacity: self.light_capacity,
                        albedo_view: view(draw.material.albedo_texture),
                        normal_texture_view: view(draw.material.normal_texture),
                        occlusion_view: view(draw.material.occlusion_texture),
                        metallic_roughness_view: view(draw.material.metallic_roughness_texture),
                        material_sampler: self
                            .sampler
                            .ok_or_else(|| anyhow!("scene sampler missing"))?,
                        frame_buffer: pass.frame,
                        visible_instances: Some((
                            pass.culling.visible_instances,
                            pass.culling.instance_capacity as u64 * 4,
                        )),
                        draw_slots: self.draw_slot_buffer,
                        draw_slot_capacity: self.draw_slot_capacity,
                    },
                )?;
                pass.groups.insert(key, group);
                group
            };
            groups.push(Some(group));
        }
        let stale = pass
            .groups
            .keys()
            .filter(|key| !active.contains(key))
            .copied()
            .collect::<Vec<_>>();
        for key in stale {
            if let Some(group) = pass.groups.remove(&key) {
                device.destroy_bind_group(group)?;
            }
        }
        // The skipped blend positions are never read: shadow draw encoding
        // excludes them before indexing this vector. Fill with a caster group
        // if available to preserve the fixed RenderCommand batch indexing.
        // All-blended scenes correctly return an empty binding list because
        // no shadow draw can dereference it.
        let Some(first_caster) = groups.iter().flatten().copied().next() else {
            return Ok(Vec::new());
        };
        Ok(groups.into_iter().map(|group| group.unwrap_or(first_caster)).collect())
    }
}
