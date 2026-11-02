//! Per-view attachments, material shadows and frame encoding.
use super::super::{SceneView, resources::RendererResources, scene::SceneResources};
use super::renderer::pass;
use super::{
    ScreenEffects,
    light_tiles::LightTiles,
    targets::{Target, Targets, uniform},
};
use crate::{Camera, Mat4, Scene, Vec3};
use anyhow::Context as _;
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext, RendererExtensionPass};
use std::sync::Arc;
use std::time::Instant;
pub(super) struct EffectScene {
    pub(super) last_seen: Instant,
    pub(super) capture: Option<SceneView>,
    pub(super) targets_valid: bool,
    pub(super) queued_capture: bool,
    pub(super) shadow_valid: bool,
    pub(super) queued_shadow: bool,
    pub(super) scene: SceneResources,
    pub(super) targets: Targets,
    pub(super) frame: BufferId,
    pub(super) composite_frame: BufferId,
    pub(super) tiles: LightTiles,
    pub(super) groups: Option<[BindGroupId; 2]>,
    pub(super) group_key: Option<(BufferId, Option<crate::TextureAssetId>)>,
    pub(super) shadow: Target,
    pub(super) shadow_color: Target,
    pub(super) shadow_key: Option<[i32; 6]>,
    pub(super) shadow_input: Option<SceneView>,
    pub(super) shadow_scene: Option<Arc<Scene>>,
    pub(super) shadow_transform: Mat4,
    pub(super) shadow_matrix: Mat4,
    pub(super) tile_transform: Mat4,
    pub(super) tile_camera: Option<Camera>,
    pub(super) tile_scene: Option<Arc<Scene>>,
    pub(super) retired: Vec<RetiredTargets>,
    pub(super) submitted_retirements: usize,
}
pub(super) struct RetiredTargets {
    pub(super) targets: Targets,
    pub(super) tiles: LightTiles,
    pub(super) groups: Option<[BindGroupId; 2]>,
}
impl RetiredTargets {
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(groups) = self.groups {
            for group in groups {
                device.destroy_bind_group(group)?;
            }
        }
        self.tiles.destroy(device)?;
        self.targets.destroy(device)
    }
}
impl EffectScene {
    pub(super) fn capture_geometry(
        &mut self,
        device: &mut dyn ExtensionDevice,
        resources: [&mut RendererResources; 3],
        view: &SceneView,
        local: &RendererExtensionContext,
        passes: &mut Vec<RendererExtensionPass>,
    ) -> gpui::Result<()> {
        let capture_changed = !self.targets_valid
            || self
                .capture
                .as_ref()
                .is_none_or(|old| !super::renderer::same_capture(old, view));
        let mut shading = view.voxel_shading;
        shading.shadow_strength = 0.;
        let capture = view.with_voxel_shader().with_voxel_shading(shading)?;
        if capture_changed {
            self.targets_valid = false;
            self.scene
                .prepare(device, resources[0], &capture, &local)
                .context("screen scene preparation")?;
        }
        if capture_changed {
            let mut opaque = Vec::new();
            self.scene.append_steps(resources[0], true, &mut opaque)?;
            let mut surface = Vec::new();
            self.scene.append_steps(resources[1], true, &mut surface)?;
            let mut albedo = Vec::new();
            self.scene.append_steps(resources[2], true, &mut albedo)?;
            passes.push(pass(
                local.render_pass(),
                self.targets.color.view,
                self.targets.depth.view,
                opaque,
                true,
            ));
            passes.push(pass(
                local.render_pass(),
                self.targets.surface.view,
                self.targets.surface_depth.view,
                surface,
                true,
            ));
            passes.push(pass(
                local.render_pass(),
                self.targets.albedo.view,
                self.targets.surface_depth.view,
                albedo,
                false,
            ));
            self.capture = Some(view.clone());
            self.queued_capture = true;
        }
        Ok(())
    }

    pub(super) fn new(
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        extent: Extent2d,
    ) -> gpui::Result<Self> {
        let mut cleanup = super::construction::Construction::default();
        let result = (|| {
            let targets = Targets::create(device, extent)?;
            for target in [
                &targets.color,
                &targets.depth,
                &targets.surface,
                &targets.surface_depth,
                &targets.albedo,
                &targets.resolved,
            ] {
                target.record(&mut cleanup);
            }
            let frame = cleanup.record(uniform(device, backend)?, |d, id| {
                let _ = d.destroy_buffer(id);
            });
            let composite_frame = cleanup.record(uniform(device, backend)?, |d, id| {
                let _ = d.destroy_buffer(id);
            });
            let tiles = LightTiles::new(device, backend, extent)?;
            cleanup.record((tiles.buffer, tiles.shaders), |d, (buffer, shaders)| {
                for shader in shaders {
                    let _ = d.destroy_shader_module(shader);
                }
                let _ = d.destroy_buffer(buffer);
            });
            let shadow = Target::create(
                device,
                Extent2d::new(1024, 1024)?,
                Format::Depth32Float,
                true,
            )?;
            shadow.record(&mut cleanup);
            let shadow_color = Target::create(
                device,
                Extent2d::new(1024, 1024)?,
                Format::Rgba16Float,
                false,
            )?;
            shadow_color.record(&mut cleanup);
            let scene = SceneResources::new(device, backend)?;
            Ok(Self {
                last_seen: Instant::now(),
                capture: None,
                targets_valid: false,
                queued_capture: false,
                shadow_valid: false,
                queued_shadow: false,
                scene,
                targets,
                frame,
                composite_frame,
                tiles,
                groups: None,
                group_key: None,
                shadow,
                shadow_color,
                shadow_key: None,
                shadow_input: None,
                shadow_scene: None,
                shadow_transform: Mat4::IDENTITY,
                shadow_matrix: Mat4::IDENTITY,
                tile_transform: Mat4::IDENTITY,
                tile_camera: None,
                tile_scene: None,
                retired: Vec::new(),
                submitted_retirements: 0,
            })
        })();
        cleanup.finish(device, result)
    }
    pub(super) fn resize(
        &mut self,
        device: &mut dyn ExtensionDevice,
        backend: BackendKind,
        extent: Extent2d,
    ) -> gpui::Result<()> {
        if self.targets.extent == extent {
            return Ok(());
        }
        self.targets_valid = false;
        let targets = Targets::create(device, extent)?;
        let tiles = match LightTiles::new(device, backend, extent) {
            Ok(tiles) => tiles,
            Err(error) => {
                let _ = targets.destroy(device);
                return Err(error);
            }
        };
        self.retired.push(RetiredTargets {
            targets: std::mem::replace(&mut self.targets, targets),
            tiles: std::mem::replace(&mut self.tiles, tiles),
            groups: self.groups.take(),
        });
        self.tile_camera = None;
        self.group_key = None;
        Ok(())
    }
    pub(super) fn shadow_pass(
        &mut self,
        device: &mut dyn ExtensionDevice,
        renderer: &RendererResources,
        view: &SceneView,
        local: &RendererExtensionContext,
        passes: &mut Vec<RendererExtensionPass>,
    ) -> gpui::Result<()> {
        if view.voxel_shading.shadow_strength <= 0. && self.shadow_valid {
            return Ok(());
        }
        let sun = view.voxel_shading.sun_direction;
        let eye = view.camera.eye;
        let key = [
            (eye.x / 4.).floor() as i32,
            (eye.y / 4.).floor() as i32,
            (eye.z / 4.).floor() as i32,
            (sun.x * 256.).round() as i32,
            (sun.y * 256.).round() as i32,
            (sun.z * 256.).round() as i32,
        ];
        let changed = !self.shadow_valid
            || self.shadow_key != Some(key)
            || self
                .shadow_scene
                .as_ref()
                .is_none_or(|old| !Arc::ptr_eq(old, &view.scene))
            || self.shadow_transform != view.scene_transform
            || self.shadow_input.as_ref().is_none_or(|old| {
                (old.voxel_shading.shadow_strength > 0.)
                    != (view.voxel_shading.shadow_strength > 0.)
                    || !Arc::ptr_eq(&old.textures, &view.textures)
                    || old.sampling != view.sampling
                    || old.anisotropy_enabled != view.anisotropy_enabled
                    || old.blend_edge_feather != view.blend_edge_feather
                    || old.culling_bounds != view.culling_bounds
                    || !Arc::ptr_eq(&old.animation, &view.animation)
                    || old.animation_time != view.animation_time
            });
        if !changed {
            return Ok(());
        }
        // Any subsequent error must retry this projection, not reuse the old map
        // with the new cache key.
        self.shadow_valid = false;
        self.shadow_key = Some(key);
        self.shadow_scene = Some(view.scene.clone());
        self.shadow_transform = view.scene_transform;
        self.shadow_input = Some(view.clone());
        let center = Vec3::new(key[0] as f32 * 4., key[1] as f32 * 4., key[2] as f32 * 4.);
        let direction = Vec3::new(key[3] as f32, key[4] as f32, key[5] as f32)
            .normalized()
            .ok_or_else(|| anyhow::anyhow!("invalid sun"))?;
        let camera = Camera::orthographic(
            center + direction * 192.,
            center,
            if direction.y.abs() > 0.99 {
                Vec3::Z
            } else {
                Vec3::Y
            },
            192.,
            0.1,
            384.,
        );
        self.shadow_matrix = camera.projection_matrix(1.)? * camera.view()?;
        let shadow_context = local.offscreen(
            local.render_pass(),
            Format::Rgba16Float,
            Extent2d::new(1024, 1024)?,
        );
        let commands = if view.voxel_shading.shadow_strength > 0. {
            self.scene
                .shadow_steps(device, renderer, camera, &shadow_context)?
        } else {
            Vec::new()
        };
        passes.push(pass(
            local.render_pass(),
            self.shadow_color.view,
            self.shadow.view,
            commands,
            true,
        ));
        self.queued_shadow = true;
        Ok(())
    }
    pub(super) fn write_frames(
        &self,
        device: &mut dyn ExtensionDevice,
        view: &SceneView,
        settings: ScreenEffects,
        local: &RendererExtensionContext,
        host: &RendererExtensionContext,
        time: f32,
        light_count: u32,
    ) -> gpui::Result<()> {
        for (buffer, context) in [(self.frame, local), (self.composite_frame, host)] {
            let matrix = view.camera.projection_matrix(
                self.targets.extent.width() as f32 / self.targets.extent.height() as f32,
            )? * view.camera.view()?;
            let mut bytes = Vec::with_capacity(400);
            for matrix in [
                matrix,
                matrix
                    .inverse()
                    .ok_or_else(|| anyhow::anyhow!("singular projection"))?,
                self.shadow_matrix,
            ] {
                for column in matrix.columns() {
                    for value in column {
                        bytes.extend(value.to_le_bytes());
                    }
                }
            }
            let shading = view.voxel_shading;
            let bounds = context.bounds();
            let extent = context.viewport();
            for values in [
                [view.camera.eye.x, view.camera.eye.y, view.camera.eye.z, 1.],
                [
                    shading.sun_direction.x,
                    shading.sun_direction.y,
                    shading.sun_direction.z,
                    shading.sky_brightness,
                ],
                [
                    shading.sun_color[0],
                    shading.sun_color[1],
                    shading.sun_color[2],
                    shading.sun_intensity,
                ],
                [32., 8., 16., time],
                [
                    f32::from(settings.reflections),
                    f32::from(settings.water),
                    f32::from(settings.volumetric_fog),
                    f32::from(settings.clouds && settings.cloud_texture.is_some()),
                ],
                [
                    settings.exposure,
                    settings.fog_density,
                    settings.cloud_height,
                    f32::from(settings.underwater),
                ],
                [
                    settings.world_origin.x,
                    settings.world_origin.y,
                    settings.world_origin.z,
                    0.,
                ],
                [
                    f64::from(bounds.origin.x) as f32,
                    f64::from(bounds.origin.y) as f32,
                    f64::from(bounds.size.width) as f32,
                    f64::from(bounds.size.height) as f32,
                ],
                [
                    extent.width() as f32,
                    extent.height() as f32,
                    shading.shadow_strength,
                    0.00005,
                ],
            ] {
                for value in values {
                    bytes.extend(value.to_le_bytes());
                }
            }
            for value in [light_count, self.tiles.size[0], self.tiles.size[1], 64] {
                bytes.extend(value.to_le_bytes());
            }
            for values in [
                [0.12f32, 0.045, 0.018, 0.],
                [16., 0.0002, 3072., 1.],
                settings
                    .background_color
                    .map_or([shading.sky_brightness, 0., 0., 0.], |color| {
                        [-1., color[0], color[1], color[2]]
                    }),
            ] {
                for value in values {
                    bytes.extend(value.to_le_bytes());
                }
            }
            device.write_buffer(buffer, 0, &bytes)?;
        }
        Ok(())
    }
    pub(super) fn destroy(mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(groups) = self.groups {
            for group in groups {
                device.destroy_bind_group(group)?;
            }
        }
        for retired in self.retired {
            retired.destroy(device)?;
        }
        self.scene.destroy(device)?;
        self.tiles.destroy(device)?;
        self.targets.destroy(device)?;
        self.shadow.destroy(device)?;
        self.shadow_color.destroy(device)?;
        device.destroy_buffer(self.frame)?;
        device.destroy_buffer(self.composite_frame)?;
        Ok(())
    }
}
