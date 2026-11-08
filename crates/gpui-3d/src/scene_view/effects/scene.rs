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

/// Shared capture dimensions. The enlarged shadow footprint covers more
/// terrain without changing the old shadow texel density appreciably.
pub(super) const SHADOW_MAP_SIZE: u32 = 1536;
pub(super) const SHADOW_WORLD_HEIGHT: f32 = 320.0;
const SHADOW_TEXEL_WORLD: f32 = SHADOW_WORLD_HEIGHT / SHADOW_MAP_SIZE as f32;

/// Snap in light-space, rather than snapping world X/Y/Z to four-block steps.
/// Reprojecting at world-grid transitions caused the whole shadow map to jump.
fn stable_shadow_camera(eye: Vec3, sun: Vec3) -> gpui::Result<([i32; 6], Camera)> {
    let sun = sun.normalized().ok_or_else(|| anyhow::anyhow!("invalid sun"))?;
    let quant = [
        (sun.x * 4096.0).round() as i32,
        (sun.y * 4096.0).round() as i32,
        (sun.z * 4096.0).round() as i32,
    ];
    let direction = Vec3::new(quant[0] as f32, quant[1] as f32, quant[2] as f32)
        .normalized().ok_or_else(|| anyhow::anyhow!("invalid shadow light axis"))?;
    let up = if direction.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let right = up.cross(direction).normalized()
        .ok_or_else(|| anyhow::anyhow!("invalid shadow right"))?;
    let vertical = direction.cross(right).normalized()
        .ok_or_else(|| anyhow::anyhow!("invalid shadow up"))?;
    let key = [
        (eye.dot(right) / SHADOW_TEXEL_WORLD).round() as i32,
        (eye.dot(vertical) / SHADOW_TEXEL_WORLD).round() as i32,
        (eye.dot(direction) / 4.0).round() as i32,
        quant[0], quant[1], quant[2],
    ];
    let center = right * (key[0] as f32 * SHADOW_TEXEL_WORLD)
        + vertical * (key[1] as f32 * SHADOW_TEXEL_WORLD)
        + direction * (key[2] as f32 * 4.0);
    Ok((key, Camera::orthographic(
        center + direction * SHADOW_WORLD_HEIGHT,
        center,
        up,
        SHADOW_WORLD_HEIGHT,
        0.1,
        SHADOW_WORLD_HEIGHT * 2.0,
    )))
}

pub(super) struct EffectScene {
    pub(super) last_seen: Instant,
    pub(super) capture: Option<SceneView>,
    pub(super) targets_valid: bool,
    pub(super) queued_capture: bool,
    pub(super) shadow_valid: bool,
    pub(super) queued_shadow: bool,
    pub(super) scene: SceneResources,
    pub(super) targets: Targets,
    pub(super) msaa_sample_count: u32,
    pub(super) frame: BufferId,
    pub(super) composite_frame: BufferId,
    pub(super) tiles: LightTiles,
    pub(super) groups: Option<[BindGroupId; 2]>,
    pub(super) group_key: Option<(BufferId, [Option<crate::TextureAssetId>; 3])>,
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
            let msaa = self.targets.msaa.as_ref();
            // When MSAA is enabled the geometry renders into multisampled
            // color+depth copies and resolves into the single-sample views that
            // downstream compositing samples. `None` keeps the single-sample path.
            passes.push(pass(
                local.render_pass(),
                msaa.map_or(self.targets.color.view, |m| m.color.view),
                msaa.map_or(self.targets.depth.view, |m| m.depth.view),
                msaa.map(|_| self.targets.color.view),
                opaque,
                true,
            ));
            passes.push(pass(
                local.render_pass(),
                msaa.map_or(self.targets.surface.view, |m| m.surface.view),
                msaa.map_or(self.targets.surface_depth.view, |m| m.surface_depth.view),
                msaa.map(|_| self.targets.surface.view),
                surface,
                true,
            ));
            passes.push(pass(
                local.render_pass(),
                msaa.map_or(self.targets.albedo.view, |m| m.albedo.view),
                msaa.map_or(self.targets.surface_depth.view, |m| m.surface_depth.view),
                msaa.map(|_| self.targets.albedo.view),
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
        msaa_sample_count: u32,
    ) -> gpui::Result<Self> {
        let mut cleanup = super::construction::Construction::default();
        let result = (|| {
            let targets = Targets::create(device, extent, msaa_sample_count)?;
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
                Extent2d::new(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE)?,
                Format::Depth32Float,
                true,
                1,
            )?;
            shadow.record(&mut cleanup);
            let shadow_color = Target::create(
                device,
                Extent2d::new(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE)?,
                Format::Rgba16Float,
                false,
                1,
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
                msaa_sample_count,
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
        let targets = Targets::create(device, extent, self.msaa_sample_count)?;
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
        // The shadow map stays single-sampled even when the scene passes are multisampled.
        shadow_pass: RenderPassId,
        passes: &mut Vec<RendererExtensionPass>,
    ) -> gpui::Result<()> {
        if view.voxel_shading.shadow_strength <= 0. && self.shadow_valid {
            return Ok(());
        }
        let (key, camera) = stable_shadow_camera(
            view.camera.eye,
            view.voxel_shading.sun_direction,
        )?;
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
         self.shadow_matrix = camera.projection_matrix(1.)? * camera.view()?;
        let shadow_context =
            local.offscreen(shadow_pass, Format::Rgba16Float, Extent2d::new(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE)?);
        let commands = if view.voxel_shading.shadow_strength > 0. {
            self.scene
                .shadow_steps(device, renderer, camera, &shadow_context)?
        } else {
            Vec::new()
        };
        passes.push(pass(
            shadow_pass,
            self.shadow_color.view,
            self.shadow.view,
            None,
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
            let mut bytes = Vec::with_capacity(super::FRAME_BYTES as usize);
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
                {
                    let animation = |strip: Option<super::FlipbookTexture>| {
                        strip
                            .and_then(|strip| {
                                view.textures.get(&strip.texture).map(|asset| {
                                    [
                                        asset.height() as f32 / asset.width() as f32,
                                        strip.seconds_per_frame,
                                    ]
                                })
                            })
                            .unwrap_or([0., 1.])
                    };
                    let [still, flow] = settings.water_flipbooks().map(animation);
                    [still[0], flow[0], still[1], flow[1]]
                },
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

#[cfg(test)]
mod stable_shadow_tests {
    use super::*;
    #[test]
    fn projection_stays_in_same_light_texel_under_subpixel_motion() {
        let sun = Vec3::new(0.45, 0.81, 0.37);
        let (key, camera) = stable_shadow_camera(Vec3::new(17.0, 80.0, -12.0), sun).unwrap();
        let (moved, _) = stable_shadow_camera(Vec3::new(17.001, 80.0, -12.0), sun).unwrap();
        assert_eq!(key, moved);
        let light = (camera.eye - camera.target).normalized().unwrap();
        let right = camera.up.cross(light).normalized().unwrap();
        let vertical = light.cross(right).normalized().unwrap();
        for axis in [right, vertical] {
            let normalized = camera.target.dot(axis) / SHADOW_TEXEL_WORLD;
            assert!((normalized - normalized.round()).abs() < 0.005);
        }
    }

    #[test]
    fn enlarged_shadow_map_keeps_previous_world_texel_density() {
        assert!(SHADOW_WORLD_HEIGHT / SHADOW_MAP_SIZE as f32 < 0.22);
    }
}
