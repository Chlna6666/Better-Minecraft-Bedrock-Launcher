use super::{
    DRAW_SLOT_STRIDE, FRAME_PARAMS_STRIDE, INSTANCE_STRIDE, LIGHT_STRIDE, SceneView,
    gpu::{
        DrawBindingResources, MeshResources, TextureResources, create_draw_resource_set,
        create_mesh, create_storage_buffer, create_texture, destroy_mesh, destroy_texture,
        encode_draws, encode_frame_params, encode_instances, encode_lights, grow_capacity,
    },
    projection::FrameProjection,
    resources::RendererResources,
};
use crate::{
    AlphaMode, AnimationScratch, Camera, Mat4, MeshId, PreparedScene, Scene, ShadingModel,
    TextureAssetId,
};
use anyhow::{Context as _, anyhow, bail};
use gfx_core::{
    BackendKind, BufferId, DrawIndexedStepDescriptor, ExtensionDevice, IndexBufferBinding,
    IndexFormat, MemoryLocation, RenderStepDescriptor, ResourceSetId, ResourceSetLayoutId,
    SamplerId, resource_set_list,
};
use gpui::RendererExtensionContext;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

pub(super) struct SceneResources {
    scene: Option<Arc<Scene>>,
    texture_table: Option<Arc<HashMap<TextureAssetId, Arc<crate::TextureAsset>>>>,
    animation: Option<Arc<[crate::TransformTrack]>>,
    animation_time: Option<Duration>,
    animation_scratch: AnimationScratch,
    camera: Option<Camera>,
    scene_transform: Option<Mat4>,
    projection: Option<FrameProjection>,
    prepared: PreparedScene,
    batches: Vec<DrawBatchRange>,
    draw_buffer: BufferId,
    draw_capacity: usize,
    instance_buffer: BufferId,
    instance_capacity: usize,
    light_buffer: BufferId,
    light_capacity: usize,
    frame_buffer: BufferId,
    frame_state: Option<FrameState>,
    sampling: crate::TextureSampling,
    meshes: HashMap<MeshId, MeshResources>,
    textures: HashMap<TextureAssetId, TextureResources>,
    draw_resources: Vec<DrawResources>,
    sampler: Option<SamplerId>,
    // Resource-cache age only; frame-visible output samples the extension frame time.
    pub(super) last_used_at: Instant,
    pub(super) last_used_frame: u64,
}

/// Values that only affect the frame-wide uniform buffer, not the prepared draw list.
#[derive(Clone, Copy, PartialEq)]
struct FrameState {
    camera: Camera,
    projection: FrameProjection,
}

#[derive(Clone, Copy)]
struct DrawBatchRange {
    first_instance: usize,
    instance_count: usize,
}

struct DrawResources {
    draw_index: usize,
    instance_count: usize,
    mesh_id: MeshId,
    generation: u64,
    albedo_id: Option<TextureAssetId>,
    normal_id: Option<TextureAssetId>,
    occlusion_id: Option<TextureAssetId>,
    resource_set: ResourceSetId,
}

impl SceneResources {
    pub(super) fn new(device: &mut dyn ExtensionDevice) -> gpui::Result<Self> {
        let draw_capacity = 1;
        let light_capacity = 1;
        let draw_buffer = create_storage_buffer(
            device,
            "gpui-3d draw data",
            draw_capacity * DRAW_SLOT_STRIDE,
            MemoryLocation::CpuToGpu,
        )?;
        let light_buffer = match create_storage_buffer(
            device,
            "gpui-3d lights",
            light_capacity * LIGHT_STRIDE as usize,
            MemoryLocation::CpuToGpu,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(draw_buffer)?;
                return Err(error);
            }
        };
        let instance_capacity = 1;
        let instance_buffer = match create_storage_buffer(
            device,
            "gpui-3d instance data",
            instance_capacity * INSTANCE_STRIDE as usize,
            MemoryLocation::CpuToGpu,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(light_buffer)?;
                device.destroy_buffer(draw_buffer)?;
                return Err(error);
            }
        };
        let frame_buffer = match create_storage_buffer(
            device,
            "gpui-3d frame params",
            FRAME_PARAMS_STRIDE,
            MemoryLocation::CpuToGpu,
        ) {
            Ok(buffer) => buffer,
            Err(error) => {
                device.destroy_buffer(instance_buffer)?;
                device.destroy_buffer(light_buffer)?;
                device.destroy_buffer(draw_buffer)?;
                return Err(error);
            }
        };
        Ok(Self {
            scene: None,
            texture_table: None,
            animation: None,
            animation_time: None,
            animation_scratch: AnimationScratch::new(),
            camera: None,
            scene_transform: None,
            projection: None,
            prepared: PreparedScene::default(),
            batches: Vec::new(),
            draw_buffer,
            draw_capacity,
            instance_buffer,
            instance_capacity,
            light_buffer,
            light_capacity,
            frame_buffer,
            frame_state: None,
            sampling: crate::TextureSampling::Linear,
            meshes: HashMap::new(),
            textures: HashMap::new(),
            draw_resources: Vec::new(),
            sampler: None,
            last_used_at: Instant::now(),
            last_used_frame: 0,
        })
    }

    pub(super) fn prepare(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &mut RendererResources,
        scene_view: &SceneView,
        context: &RendererExtensionContext,
    ) -> gpui::Result<()> {
        let projection = FrameProjection::new(scene_view, context)?;
        let sampler = renderer.sampler(device, scene_view.sampling, scene_view.anisotropy_enabled)?;
        let assets_changed =
            scene_assets_changed(self.scene.as_ref(), self.texture_table.as_ref(), scene_view);
        let draw_state_changed = assets_changed
            || self.scene_transform != Some(scene_view.scene_transform)
            || self
                .animation
                .as_ref()
                .is_none_or(|animation| !Arc::ptr_eq(animation, &scene_view.animation))
            || self.animation_time != Some(scene_view.animation_time);
        // A camera or viewport change only re-culls, re-sorts, and rewrites the frame parameters.
        // It must not re-walk the scene graph or rebuild the per-draw material records.
        let camera_changed = self.camera != Some(scene_view.camera)
            || self.projection.map(FrameProjection::aspect) != Some(projection.aspect());
        let frame_changed = camera_changed
            || self.frame_state.is_none_or(|state| {
                state.camera != scene_view.camera || state.projection != projection
            });
        let sampler_changed = self.sampler != Some(sampler);
        if !draw_state_changed && !frame_changed && !sampler_changed {
            return Ok(());
        }
        if draw_state_changed {
            update_prepared(
                &mut self.prepared,
                &mut self.animation_scratch,
                scene_view,
                projection.aspect(),
            )?;
            self.batches.clear();
            self.batches
                .extend(self.prepared.draw_batches().map(|batch| DrawBatchRange {
                    first_instance: batch.first_instance(),
                    instance_count: batch.draws().len(),
                }));
            validate_textures(&self.prepared, scene_view)?;
            self.ensure_textures(device, scene_view)?;
            self.ensure_meshes(device, context.backend_kind())?;
        } else if camera_changed
            && self
                .prepared
                .update_camera(scene_view.camera, projection.aspect())?
        {
            self.batches.clear();
            self.batches
                .extend(self.prepared.draw_batches().map(|batch| DrawBatchRange {
                    first_instance: batch.first_instance(),
                    instance_count: batch.draws().len(),
                }));
        }
        if frame_changed {
            self.sync_frame_params(device, projection, scene_view.camera)?;
            self.frame_state = Some(FrameState {
                camera: scene_view.camera,
                projection,
            });
        }
        if draw_state_changed {
            let resource_set_layout = renderer.resource_set_layout()?;
            self.sync_buffers(
                device,
                renderer,
                resource_set_layout,
                projection,
                scene_view.camera,
                sampler,
                sampler_changed,
            )?;
        } else {
            self.sync_draw_resources(
                device,
                renderer,
                renderer.resource_set_layout()?,
                sampler,
                sampler_changed,
            )?;
        }
        if assets_changed {
            // Frustum culling is transient during camera motion; keep those GPU resources resident.
            self.retire_textures(device)?;
            self.retire_meshes(device)?;
        }
        self.scene = Some(scene_view.scene.clone());
        self.texture_table = Some(scene_view.textures.clone());
        self.animation = Some(scene_view.animation.clone());
        self.animation_time = Some(scene_view.animation_time);
        self.camera = Some(scene_view.camera);
        self.scene_transform = Some(scene_view.scene_transform);
        self.projection = Some(projection);
        self.sampler = Some(sampler);
        Ok(())
    }

    /// Uploads the camera, viewport, and extension-bounds values shared by every draw.
    fn sync_frame_params(
        &mut self,
        device: &mut dyn ExtensionDevice,
        projection: FrameProjection,
        camera: Camera,
    ) -> gpui::Result<()> {
        let bytes = encode_frame_params(
            self.prepared.view_projection,
            camera,
            projection.bounds(),
            projection.target_extent(),
            projection.blend_edge_feather(),
        );
        device.write_buffer(self.frame_buffer, 0, &bytes)?;
        Ok(())
    }

    fn sync_buffers(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        resource_set_layout: ResourceSetLayoutId,
        projection: FrameProjection,
        camera: Camera,
        sampler: SamplerId,
        sampler_changed: bool,
    ) -> gpui::Result<()> {
        let draw_count = self.prepared.draws.len().max(1);
        let instance_count = draw_count;
        let light_count = self.prepared.lights.len().max(1);
        let next_draw_capacity = grow_capacity(self.draw_capacity, draw_count)?;
        let next_instance_capacity = grow_capacity(self.instance_capacity, instance_count)?;
        let next_light_capacity = grow_capacity(self.light_capacity, light_count)?;
        let replace_draw = next_draw_capacity != self.draw_capacity;
        let replace_instances = next_instance_capacity != self.instance_capacity;
        let replace_lights = next_light_capacity != self.light_capacity;
        let draw_bytes = encode_draws(&self.prepared)?;
        let instance_bytes = encode_instances(&self.prepared)?;
        let light_bytes = encode_lights(&self.prepared.lights);
        let new_draw = if replace_draw {
            Some(create_storage_buffer(
                device,
                "gpui-3d draw data",
                next_draw_capacity * DRAW_SLOT_STRIDE,
                MemoryLocation::CpuToGpu,
            )?)
        } else {
            None
        };
        let new_lights = if replace_lights {
            match create_storage_buffer(
                device,
                "gpui-3d lights",
                next_light_capacity * LIGHT_STRIDE as usize,
                MemoryLocation::CpuToGpu,
            ) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    if let Some(buffer) = new_draw {
                        device.destroy_buffer(buffer)?;
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let new_instances = if replace_instances {
            match create_storage_buffer(
                device,
                "gpui-3d instance data",
                next_instance_capacity * INSTANCE_STRIDE as usize,
                MemoryLocation::CpuToGpu,
            ) {
                Ok(buffer) => Some(buffer),
                Err(error) => {
                    if let Some(buffer) = new_lights {
                        device.destroy_buffer(buffer)?;
                    }
                    if let Some(buffer) = new_draw {
                        device.destroy_buffer(buffer)?;
                    }
                    return Err(error);
                }
            }
        } else {
            None
        };
        let draw_buffer = new_draw.unwrap_or(self.draw_buffer);
        let instance_buffer = new_instances.unwrap_or(self.instance_buffer);
        let light_buffer = new_lights.unwrap_or(self.light_buffer);
        let write_result = device
            .write_buffer(draw_buffer, 0, &draw_bytes)
            .and_then(|()| device.write_buffer(instance_buffer, 0, &instance_bytes))
            .and_then(|()| device.write_buffer(light_buffer, 0, &light_bytes));
        if let Err(error) = write_result {
            if let Some(buffer) = new_draw {
                device.destroy_buffer(buffer)?;
            }
            if let Some(buffer) = new_instances {
                device.destroy_buffer(buffer)?;
            }
            if let Some(buffer) = new_lights {
                device.destroy_buffer(buffer)?;
            }
            return Err(error.into());
        }
        let old_draw = self.draw_buffer;
        let old_draw_capacity = self.draw_capacity;
        let old_instances = self.instance_buffer;
        let old_instance_capacity = self.instance_capacity;
        let old_lights = self.light_buffer;
        let old_light_capacity = self.light_capacity;
        self.draw_buffer = draw_buffer;
        self.draw_capacity = next_draw_capacity;
        self.instance_buffer = instance_buffer;
        self.instance_capacity = next_instance_capacity;
        self.light_buffer = light_buffer;
        self.light_capacity = next_light_capacity;
        if let Err(error) = self.sync_draw_resources(
            device,
            renderer,
            resource_set_layout,
            sampler,
            sampler_changed || replace_draw || replace_instances || replace_lights,
        ) {
            self.draw_buffer = old_draw;
            self.draw_capacity = old_draw_capacity;
            self.instance_buffer = old_instances;
            self.instance_capacity = old_instance_capacity;
            self.light_buffer = old_lights;
            self.light_capacity = old_light_capacity;
            if replace_draw {
                device.destroy_buffer(draw_buffer)?;
            }
            if replace_lights {
                device.destroy_buffer(light_buffer)?;
            }
            if replace_instances {
                device.destroy_buffer(instance_buffer)?;
            }
            return Err(error);
        }
        if replace_draw {
            device.destroy_buffer(old_draw)?;
        }
        if replace_lights {
            device.destroy_buffer(old_lights)?;
        }
        if replace_instances {
            device.destroy_buffer(old_instances)?;
        }
        Ok(())
    }

    fn ensure_textures(
        &mut self,
        device: &mut dyn ExtensionDevice,
        scene_view: &SceneView,
    ) -> gpui::Result<()> {
        for draw in &self.prepared.draws {
            for id in [
                draw.material.albedo_texture,
                draw.material.normal_texture,
                draw.material.occlusion_texture,
            ]
            .into_iter()
            .flatten()
            {
                if self.textures.contains_key(&id) {
                    continue;
                }
                let asset = scene_view
                    .texture(id)
                    .ok_or_else(|| anyhow!("GPUI 3D scene view has no texture asset {}", id.0))?;
                let resources = create_texture(device, asset)?;
                self.textures.insert(id, resources);
            }
        }
        Ok(())
    }

    fn retire_textures(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let active = self
            .prepared
            .draws
            .iter()
            .flat_map(|draw| {
                [
                    draw.material.albedo_texture,
                    draw.material.normal_texture,
                    draw.material.occlusion_texture,
                ]
            })
            .flatten()
            .collect::<HashSet<_>>();
        let stale = self
            .textures
            .keys()
            .filter(|id| !active.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in stale {
            if let Some(texture) = self.textures.remove(&id) {
                destroy_texture(device, texture)?;
            }
        }
        Ok(())
    }

    fn ensure_meshes(
        &mut self,
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
    ) -> gpui::Result<()> {
        for draw in &self.prepared.draws {
            let id = draw.mesh.id();
            let generation = draw.mesh.generation();
            if self
                .meshes
                .get(&id)
                .is_some_and(|mesh| mesh.generation == generation)
            {
                continue;
            }
            if let Some(old) = self.meshes.remove(&id) {
                destroy_mesh(device, old)?;
            }
            let mesh = create_mesh(device, &draw.mesh, backend)?;
            self.meshes.insert(id, mesh);
        }
        Ok(())
    }

    fn retire_meshes(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let active = self
            .prepared
            .draws
            .iter()
            .map(|draw| draw.mesh.id())
            .collect::<HashSet<_>>();
        let stale = self
            .meshes
            .keys()
            .filter(|id| !active.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in stale {
            if let Some(mesh) = self.meshes.remove(&id) {
                destroy_mesh(device, mesh)?;
            }
        }
        Ok(())
    }

    fn sync_draw_resources(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        resource_set_layout: ResourceSetLayoutId,
        sampler: SamplerId,
        force: bool,
    ) -> gpui::Result<()> {
        let mut next = Vec::with_capacity(self.batches.len());
        let mut created = Vec::new();
        let instance_count = self.prepared.draws.len();
        for (batch_index, batch) in self.batches.iter().enumerate() {
            let index = batch.first_instance;
            let draw = self
                .prepared
                .draws
                .get(index)
                .ok_or_else(|| anyhow!("GPUI 3D draw batch has an invalid first draw"))?;
            let mesh_id = draw.mesh.id();
            let generation = draw.mesh.generation();
            let albedo_id = draw.material.albedo_texture;
            let normal_id = draw.material.normal_texture;
            let occlusion_id = draw.material.occlusion_texture;
            if !force
                && let Some(previous) = self.draw_resources.get(batch_index)
                && previous.draw_index == index
                && previous.instance_count == instance_count
                && previous.mesh_id == mesh_id
                && previous.generation == generation
                && previous.albedo_id == albedo_id
                && previous.normal_id == normal_id
                && previous.occlusion_id == occlusion_id
            {
                next.push(DrawResources {
                    draw_index: index,
                    instance_count,
                    mesh_id,
                    generation,
                    albedo_id,
                    normal_id,
                    occlusion_id,
                    resource_set: previous.resource_set,
                });
                continue;
            }
            let mesh = self
                .meshes
                .get(&mesh_id)
                .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
            let albedo_view = if let Some(id) = albedo_id {
                self.textures
                    .get(&id)
                    .map(|texture| texture.view)
                    .ok_or_else(|| anyhow!("GPUI 3D texture {} was not uploaded", id.0))?
            } else {
                renderer.fallback_texture_view()?
            };
            let normal_texture_view = if let Some(id) = normal_id {
                self.textures
                    .get(&id)
                    .map(|texture| texture.view)
                    .ok_or_else(|| anyhow!("GPUI 3D texture {} was not uploaded", id.0))?
            } else {
                renderer.fallback_texture_view()?
            };
            let occlusion_view = if let Some(id) = occlusion_id {
                self.textures
                    .get(&id)
                    .map(|texture| texture.view)
                    .ok_or_else(|| anyhow!("GPUI 3D texture {} was not uploaded", id.0))?
            } else {
                renderer.fallback_texture_view()?
            };
            match create_draw_resource_set(
                device,
                DrawBindingResources {
                    resource_set_layout,
                    mesh,
                    draw_buffer: self.draw_buffer,
                    draw_index: index,
                    instance_buffer: self.instance_buffer,
                    instance_count,
                    light_buffer: self.light_buffer,
                    light_capacity: self.light_capacity,
                    albedo_view,
                    normal_texture_view,
                    occlusion_view,
                    material_sampler: sampler,
                    frame_buffer: self.frame_buffer,
                },
            ) {
                Ok(resource_set) => {
                    created.push(resource_set);
                    next.push(DrawResources {
                        draw_index: index,
                        instance_count,
                        mesh_id,
                        generation,
                        albedo_id,
                        normal_id,
                        occlusion_id,
                        resource_set,
                    });
                }
                Err(error) => {
                    for resource_set in created {
                        device.destroy_resource_set(resource_set)?;
                    }
                    return Err(error);
                }
            }
        }
        let old = std::mem::replace(&mut self.draw_resources, next);
        for previous in old {
            if !self
                .draw_resources
                .iter()
                .any(|current| current.resource_set == previous.resource_set)
            {
                device.destroy_resource_set(previous.resource_set)?;
            }
        }
        Ok(())
    }

    pub(super) fn append_steps(
        &self,
        renderer: &RendererResources,
        steps: &mut Vec<RenderStepDescriptor>,
    ) -> gpui::Result<()> {
        for (batch_index, batch) in self.batches.iter().enumerate() {
            let index = batch.first_instance;
            let draw = self
                .prepared
                .draws
                .get(index)
                .ok_or_else(|| anyhow!("GPUI 3D draw batch has an invalid first draw"))?;
            let mesh = self
                .meshes
                .get(&draw.mesh.id())
                .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
            let draw_resources = self
                .draw_resources
                .get(batch_index)
                .ok_or_else(|| anyhow!("GPUI 3D draw resources were not initialized"))?;
            let pipeline = if draw.material.alpha_mode == AlphaMode::Blend {
                renderer
                    .blended_pipeline
                    .ok_or_else(|| anyhow!("GPUI 3D blended pipeline is unavailable"))?
            } else {
                renderer
                    .opaque_pipeline
                    .ok_or_else(|| anyhow!("GPUI 3D opaque pipeline is unavailable"))?
            };
            steps.push(RenderStepDescriptor::DrawIndexed(
                DrawIndexedStepDescriptor {
                    pipeline,
                    resource_sets: resource_set_list([draw_resources.resource_set]),
                    index_buffer: IndexBufferBinding {
                        buffer: mesh.indices,
                        format: IndexFormat::Uint32,
                        offset: 0,
                    },
                    index_count: draw.part.index_count(),
                    first_index: draw.part.first_index(),
                    base_vertex: 0,
                    instance_count: u32::try_from(batch.instance_count)
                        .context("GPUI 3D instance count exceeds u32")?,
                    first_instance: u32::try_from(batch.first_instance)
                        .context("GPUI 3D first instance exceeds u32")?,
                    scissor: None,
                },
            ));
        }
        Ok(())
    }

    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for draw in self.draw_resources.drain(..) {
            device.destroy_resource_set(draw.resource_set)?;
        }
        for (_, mesh) in self.meshes.drain() {
            destroy_mesh(device, mesh)?;
        }
        for (_, texture) in self.textures.drain() {
            destroy_texture(device, texture)?;
        }
        device.destroy_buffer(self.draw_buffer)?;
        device.destroy_buffer(self.instance_buffer)?;
        device.destroy_buffer(self.light_buffer)?;
        device.destroy_buffer(self.frame_buffer)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{scene_assets_changed, update_prepared, validate_textures};
    use crate::{
        AnimationScratch, Camera, Material, Mesh, Node, PreparedScene, Scene, SceneView,
        TextureAsset, TextureAssetId, TransformTrack, Vec3, Vec3Track,
    };
    use std::{sync::Arc, time::Duration};

    #[test]
    fn camera_changes_keep_scene_assets_reusable() {
        let scene_view = SceneView::new(
            Arc::new(Scene::new()),
            Camera::perspective(
                Vec3::new(0.0, 0.0, 3.0),
                Vec3::ZERO,
                Vec3::Y,
                1.0,
                0.1,
                10.0,
            ),
        );
        let rotated = scene_view.with_camera(Camera {
            eye: Vec3::new(3.0, 0.0, 0.0),
            ..scene_view.camera
        });

        assert!(!scene_assets_changed(
            Some(&scene_view.scene),
            Some(&scene_view.textures),
            &rotated,
        ));
    }

    #[test]
    fn scene_view_preparation_samples_animation_at_snapshot_time() {
        let mut scene = Scene::new();
        let node = scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([
                crate::Keyframe::new(Duration::ZERO, Vec3::ZERO),
                crate::Keyframe::new(Duration::from_secs(1), Vec3::new(2.0, 0.0, 0.0)),
            ])
            .unwrap(),
        );
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_animation([track], Duration::from_millis(500))
            .unwrap();
        let mut prepared = crate::PreparedScene::default();
        let mut animation_scratch = AnimationScratch::new();

        update_prepared(&mut prepared, &mut animation_scratch, &scene_view, 1.0).unwrap();

        assert_eq!(prepared.draws.len(), 1);
        assert!((prepared.draws[0].world.transform_point(Vec3::ZERO).x - 1.0).abs() < 1e-5);

        update_prepared(
            &mut prepared,
            &mut animation_scratch,
            &scene_view.with_animation_time(Duration::from_secs(1)),
            1.0,
        )
        .unwrap();

        assert!((prepared.draws[0].world.transform_point(Vec3::ZERO).x - 2.0).abs() < 1e-5);
    }

    #[test]
    fn scene_view_rejects_missing_occlusion_texture_assets() {
        let mut material = Material::new();
        material.occlusion_texture = Some(TextureAssetId(19));
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera);
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(error.to_string().contains("texture asset 19"));
    }

    #[test]
    fn scene_view_rejects_normal_maps_without_mesh_tangents() {
        let texture = Arc::new(
            TextureAsset::linear_rgba8(1, 1, Arc::<[u8]>::from([128, 128, 255, 255])).unwrap(),
        );
        let mut material = Material::new();
        material.normal_texture = Some(texture.id());
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_textures([texture])
            .unwrap();
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("require generated or supplied mesh tangents")
        );
    }

    #[test]
    fn scene_view_rejects_srgb_normal_texture_assets() {
        let texture = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([128; 4])).unwrap());
        let mut material = Material::new();
        material.normal_texture = Some(texture.id());
        let mesh = Mesh::cube().generate_tangents().unwrap().into_parts().0;
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(mesh))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_textures([texture])
            .unwrap();
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(error.to_string().contains("must use linear color space"));
    }

    #[test]
    fn scene_view_rejects_srgb_occlusion_texture_assets() {
        let texture = Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([128; 4])).unwrap());
        let mut material = Material::new();
        material.occlusion_texture = Some(texture.id());
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_materials([Arc::new(material)]),
            )
            .unwrap();
        let camera = Camera::perspective(
            Vec3::new(0.0, 0.0, 3.0),
            Vec3::ZERO,
            Vec3::Y,
            1.0,
            0.1,
            10.0,
        );
        let scene_view = SceneView::new(Arc::new(scene), camera)
            .with_textures([texture])
            .unwrap();
        let prepared = PreparedScene::new(scene_view.scene(), camera, 1.0).unwrap();

        let error = validate_textures(&prepared, &scene_view).unwrap_err();

        assert!(error.to_string().contains("must use linear color space"));
    }
}

fn scene_assets_changed(
    scene: Option<&Arc<Scene>>,
    texture_table: Option<&Arc<HashMap<TextureAssetId, Arc<crate::TextureAsset>>>>,
    scene_view: &SceneView,
) -> bool {
    scene.is_none_or(|scene| !Arc::ptr_eq(scene, &scene_view.scene))
        || texture_table.is_none_or(|textures| !Arc::ptr_eq(textures, &scene_view.textures))
}

fn validate_textures(prepared: &PreparedScene, scene_view: &SceneView) -> anyhow::Result<()> {
    for draw in &prepared.draws {
        if draw.material.shading_model == ShadingModel::MetallicRoughness
            && draw.material.normal_texture.is_some()
            && draw
                .mesh
                .tangents()
                .is_none_or(|tangents| tangents.len() != draw.mesh.vertices().len())
        {
            bail!("GPUI 3D normal maps require generated or supplied mesh tangents")
        }
        for texture_id in [
            draw.material.albedo_texture,
            draw.material.normal_texture,
            draw.material.occlusion_texture,
        ]
        .into_iter()
        .flatten()
        {
            let asset = scene_view.texture(texture_id).ok_or_else(|| {
                anyhow!("GPUI 3D scene view has no texture asset {}", texture_id.0)
            })?;
            if draw.material.occlusion_texture == Some(texture_id)
                && asset.color_space() != crate::TextureColorSpace::Linear
            {
                bail!(
                    "GPUI 3D occlusion texture {} must use linear color space",
                    texture_id.0
                )
            }
            if draw.material.normal_texture == Some(texture_id)
                && asset.color_space() != crate::TextureColorSpace::Linear
            {
                bail!(
                    "GPUI 3D normal texture {} must use linear color space",
                    texture_id.0
                )
            }
        }
    }
    Ok(())
}

fn update_prepared(
    prepared: &mut PreparedScene,
    animation_scratch: &mut AnimationScratch,
    scene_view: &SceneView,
    aspect: f32,
) -> anyhow::Result<()> {
    if scene_view.animation.is_empty() {
        prepared
            .update_transformed(
                &scene_view.scene,
                scene_view.camera,
                aspect,
                scene_view.scene_transform,
            )
            .context("preparing GPUI 3D scene")?;
    } else {
        let evaluated = scene_view
            .scene
            .evaluate_with(
                &scene_view.animation,
                scene_view.animation_time,
                animation_scratch,
            )
            .context("evaluating GPUI 3D scene animation")?;
        prepared
            .update_transformed_evaluated(
                &evaluated,
                scene_view.camera,
                aspect,
                scene_view.scene_transform,
            )
            .context("preparing animated GPUI 3D scene")?;
    }
    Ok(())
}
