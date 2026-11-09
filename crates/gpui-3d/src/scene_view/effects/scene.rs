//! Per-view attachments, material shadows and frame encoding.
use super::super::{SceneView, resources::RendererResources, scene::SceneResources};
use super::renderer::pass;
use super::{
    ScreenEffects, SHADOW_CASCADE_COUNT,
    light_tiles::LightTiles,
    targets::{Target, Targets, uniform},
};
use crate::{Camera, Mat4, Scene, Vec3};
use anyhow::Context as _;
use gfx_core::*;
use gpui::{ExtensionDevice, RendererExtensionContext, RendererExtensionPass};
use std::sync::Arc;
use std::time::Instant;

/// Stable, rotationally independent three-level shadow clipmap. Each level
/// has a dedicated GPU depth texture and culling output to avoid overwriting
/// another cascade's indirect arguments before native submission.
pub(super) const SHADOW_MAP_SIZE: u32 = 1024;
pub(super) const SHADOW_CASCADE_WIDTHS: [f32; SHADOW_CASCADE_COUNT] = [128.0, 320.0, 640.0];
pub(super) const SHADOW_CASCADE_DEPTHS: [f32; SHADOW_CASCADE_COUNT] = [384.0, 768.0, 1280.0];
/// Near/mid/far receiver radius and crossfade half-width.
pub(super) const SHADOW_CASCADE_SPLITS: [f32; 4] = [52.0, 140.0, 296.0, 8.0];
const SHADOW_SCROLL_TEXELS: f32 = 32.0;
const SHADOW_DEPTH_SCROLL_WORLD: f32 = 16.0;

/// Conservatively include casters upstream along sunlight, even when outside
/// main-camera frustum. Shadows can only be received in this radial partition.
fn shadow_caster_bounds(center: Vec3, sun: Vec3, cascade: usize) -> crate::Aabb {
    // The caster volume must be derived from the *snapped* light camera, not
    // the live eye. Otherwise its contents could change while the cascade key
    // stays cached. Inflate for any main-eye motion inside the projection tile.
    let texel = SHADOW_CASCADE_WIDTHS[cascade] / SHADOW_MAP_SIZE as f32;
    let hysteresis = texel * SHADOW_SCROLL_TEXELS * 0.5
        + SHADOW_DEPTH_SCROLL_WORLD * 0.5 + 1.0;
    let radius = SHADOW_CASCADE_SPLITS[cascade]
        + SHADOW_CASCADE_SPLITS[3] + hysteresis;
    let padding = Vec3::new(radius, radius, radius);
    let upstream = sun * SHADOW_CASCADE_DEPTHS[cascade];
    crate::Aabb {
        min: center.min(center + upstream) - padding,
        max: center.max(center + upstream) + padding,
    }
}

/// Stable light-space texel snapping: movement inside a tile does not change
/// this projection. Quantized light direction and whole-texel movement avoid
/// temporal shimmer without any CPU readback or GPU queue synchronization.
fn stable_shadow_camera(
    eye: Vec3,
    sun: Vec3,
    cascade: usize,
) -> gpui::Result<([i32; 6], Camera)> {
    let sun = sun.normalized().ok_or_else(|| anyhow::anyhow!("invalid sun"))?;
    let width = SHADOW_CASCADE_WIDTHS[cascade];
    let scroll_world = width / SHADOW_MAP_SIZE as f32 * SHADOW_SCROLL_TEXELS;
    let quant = [
        (sun.x * 4096.0).round() as i32,
        (sun.y * 4096.0).round() as i32,
        (sun.z * 4096.0).round() as i32,
    ];
    let direction = Vec3::new(quant[0] as f32, quant[1] as f32, quant[2] as f32)
        .normalized().ok_or_else(|| anyhow::anyhow!("invalid shadow light axis"))?;
    // Keep the light-space basis continuous through high noon. Switching
    // world-Y to world-Z when the sun crosses y=0.99 rotated the entire
    // shadow map in one frame. Project a fixed world-X anchor instead;
    // fallback only for the degenerate sun-along-X configuration.
    let anchor = if direction.x.abs() > 0.99 { Vec3::Z } else { Vec3::X };
    let right = (anchor - direction * anchor.dot(direction)).normalized()
        .ok_or_else(|| anyhow::anyhow!("invalid shadow right"))?;
    let vertical = direction.cross(right).normalized()
        .ok_or_else(|| anyhow::anyhow!("invalid shadow up"))?;
    let up = vertical;
    let key = [
        (eye.dot(right) / scroll_world).round() as i32,
        (eye.dot(vertical) / scroll_world).round() as i32,
        (eye.dot(direction) / SHADOW_DEPTH_SCROLL_WORLD).round() as i32,
        quant[0], quant[1], quant[2],
    ];
    let center = right * (key[0] as f32 * scroll_world)
        + vertical * (key[1] as f32 * scroll_world)
        + direction * (key[2] as f32 * SHADOW_DEPTH_SCROLL_WORLD);
    let depth = SHADOW_CASCADE_DEPTHS[cascade];
    Ok((key, Camera::orthographic(
        center + direction * depth,
        center,
        up,
        width,
        0.1,
        depth * 2.0,
    )))
}

/// Per-cascade shadow-cache invalidation using the existing Scene revision
/// lineage. Unlike whole-scene invalidation, a changed Section outside this
/// cascade's sun-extruded caster region cannot affect its depth attachment.
/// This visits only changed root nodes, never every voxel or every DrawRecord.
/// Unknown/structural revisions and animated hierarchies still conservatively
/// invalidate because their derived world bounds cannot be proven local.
fn shadow_caster_scene_changed(
    previous: Option<&Arc<Scene>>,
    current: &Arc<Scene>,
    transform: Mat4,
    caster_region: crate::Aabb,
) -> bool {
    let Some(previous) = previous else { return true };
    if Arc::ptr_eq(previous, current)
        || (previous.instance_id() == current.instance_id()
            && previous.version() == current.version())
    {
        return false;
    }
    let Some(changed) = current.changed_nodes(previous) else {
        return true;
    };
    for id in changed.iter().copied() {
        // Both prior and new bounds are necessary: removing a caster must
        // invalidate the old cascade even if its new position is elsewhere.
        for snapshot in [&**previous, &**current] {
            let Some(node) = snapshot.node(id) else { continue };
            if !node.visible { continue; }
            if snapshot.parent(id).is_some()
                || snapshot.children(id).is_some_and(|children| !children.is_empty())
                || !node.lod.is_empty()
            {
                return true;
            }
            let Some(mesh) = node.mesh.as_ref() else { continue };
            let Some(bounds) = mesh.bounds() else { continue };
            let bounds = bounds.transformed(transform * node.transform.matrix());
            if !bounds.min.is_finite() || !bounds.max.is_finite() {
                return true;
            }
            if bounds.min.x <= caster_region.max.x
                && bounds.max.x >= caster_region.min.x
                && bounds.min.y <= caster_region.max.y
                && bounds.max.y >= caster_region.min.y
                && bounds.min.z <= caster_region.max.z
                && bounds.max.z >= caster_region.min.z
            {
                return true;
            }
        }
    }
    false
}

pub(super) struct EffectScene {
    pub(super) last_seen: Instant,
    pub(super) capture: Option<SceneView>,
    pub(super) targets_valid: bool,
    pub(super) queued_capture: bool,
    pub(super) shadow_valid: [bool; SHADOW_CASCADE_COUNT],
    pub(super) queued_shadow: [bool; SHADOW_CASCADE_COUNT],
    pub(super) scene: SceneResources,
    pub(super) targets: Targets,
    pub(super) msaa_sample_count: u32,
    pub(super) frame: BufferId,
    pub(super) composite_frame: BufferId,
    pub(super) tiles: LightTiles,
    pub(super) groups: Option<[BindGroupId; 2]>,
    pub(super) group_key: Option<(BufferId, [Option<crate::TextureAssetId>; 3])>,
    pub(super) shadow: [Target; SHADOW_CASCADE_COUNT],
    pub(super) shadow_color: [Target; SHADOW_CASCADE_COUNT],
    pub(super) shadow_key: [Option<[i32; 6]>; SHADOW_CASCADE_COUNT],
    pub(super) shadow_input: Option<SceneView>,
    pub(super) shadow_scene: Option<Arc<Scene>>,
    pub(super) shadow_transform: Mat4,
    pub(super) shadow_matrix: [Mat4; SHADOW_CASCADE_COUNT],
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
        resources: [&mut RendererResources; 2],
        view: &SceneView,
        local: &RendererExtensionContext,
        attributes_pass: RenderPassId,
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
            let mut attributes = Vec::new();
            self.scene.append_steps(resources[1], true, &mut attributes)?;
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
            let surface = msaa.map_or(self.targets.surface.view, |m| m.surface.view);
            let albedo = msaa.map_or(self.targets.albedo.view, |m| m.albedo.view);
            let mut attributes_plan = pass(
                attributes_pass,
                surface,
                msaa.map_or(self.targets.surface_depth.view, |m| m.surface_depth.view),
                msaa.map(|_| self.targets.surface.view),
                attributes,
                true,
            );
            attributes_plan.color_attachments = Arc::from([surface, albedo]);
            passes.push(attributes_plan);
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
            let mut shadows = Vec::with_capacity(SHADOW_CASCADE_COUNT);
            let mut shadow_colors = Vec::with_capacity(SHADOW_CASCADE_COUNT);
            for _ in 0..SHADOW_CASCADE_COUNT {
                let shadow = Target::create(
                    device, Extent2d::new(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE)?,
                    Format::Depth32Float, true, 1,
                )?;
                shadow.record(&mut cleanup);
                shadows.push(shadow);
                // This unused color attachment exists for native backends that
                // do not support a depth-only material RenderPass.
                let color = Target::create(
                    device, Extent2d::new(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE)?,
                    Format::R8Unorm, false, 1,
                )?;
                color.record(&mut cleanup);
                shadow_colors.push(color);
            }
            let shadow: [Target; SHADOW_CASCADE_COUNT] = shadows
                .try_into().map_err(|_| anyhow::anyhow!("missing shadow cascades"))?;
            let shadow_color: [Target; SHADOW_CASCADE_COUNT] = shadow_colors
                .try_into().map_err(|_| anyhow::anyhow!("missing shadow colors"))?;
            let scene = SceneResources::new(device, backend)?;
            Ok(Self {
                last_seen: Instant::now(),
                capture: None,
                targets_valid: false,
                queued_capture: false,
                shadow_valid: [false; SHADOW_CASCADE_COUNT],
                queued_shadow: [false; SHADOW_CASCADE_COUNT],
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
                shadow_key: [None; SHADOW_CASCADE_COUNT],
                shadow_input: None,
                shadow_scene: None,
                shadow_transform: Mat4::IDENTITY,
                shadow_matrix: [Mat4::IDENTITY; SHADOW_CASCADE_COUNT],
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
        if view.voxel_shading.shadow_strength <= 0. {
            // Shading is disabled, so neither a map clear nor a GPU cull is
            // necessary. Retain the old maps for a subsequent re-enable.
            return Ok(());
        }
        let Some(sun) = view.voxel_shading.sun_direction.normalized() else {
            self.shadow_valid.fill(false);
            return Err(anyhow::anyhow!("invalid sun"));
        };
        let caster_state_dirty =
            self.shadow_transform != view.scene_transform
            || self.shadow_input.as_ref().is_none_or(|old| {
                !Arc::ptr_eq(&old.textures, &view.textures)
                    || old.sampling != view.sampling
                    || old.anisotropy_enabled != view.anisotropy_enabled
                    || old.blend_edge_feather != view.blend_edge_feather
                    || !Arc::ptr_eq(&old.animation, &view.animation)
                    || (!view.animation.is_empty()
                        && old.animation_time != view.animation_time)
            });
        // Persistent CSM: rebuild only a cascade whose texel-snapped camera
        // crossed an integer tile, or whose actual caster geometry changed.
        // Rotation of the main viewer never invalidates any shadow cascade.
        for cascade in 0..SHADOW_CASCADE_COUNT {
            let (key, camera) = match stable_shadow_camera(view.camera.eye, sun, cascade) {
                Ok(camera) => camera,
                Err(error) => {
                    self.shadow_valid.fill(false);
                    return Err(error);
                }
            };
            let caster_bounds = shadow_caster_bounds(camera.target, sun, cascade);
            if self.shadow_valid[cascade]
                && self.shadow_key[cascade] == Some(key)
                && !caster_state_dirty
                && !shadow_caster_scene_changed(
                    self.shadow_scene.as_ref(), &view.scene,
                    view.scene_transform, caster_bounds,
                )
            {
                continue;
            }
            // Transactional failure: keep this map invalid until the backend
            // has actually submitted the full compute+draw command sequence.
            self.shadow_valid[cascade] = false;
            self.shadow_key[cascade] = Some(key);
            self.shadow_matrix[cascade] = camera.projection_matrix(1.)? * camera.view()?;
            let context = local.offscreen(
                shadow_pass, Format::R8Unorm,
                Extent2d::new(SHADOW_MAP_SIZE, SHADOW_MAP_SIZE)?,
            );
            let commands = self.scene.shadow_steps(
                device, renderer, camera, cascade, caster_bounds, &context,
            )?;
            passes.push(pass(
                shadow_pass,
                self.shadow_color[cascade].view,
                self.shadow[cascade].view,
                None,
                commands,
                true,
            ));
            self.queued_shadow[cascade] = true;
        }
        self.shadow_scene = Some(view.scene.clone());
        self.shadow_transform = view.scene_transform;
        self.shadow_input = Some(view.clone());
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
                self.shadow_matrix[0],
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
                [4., 0.0002, 16., 1.],
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
            // Existing 416-byte effect ABI is followed by two additional
            // light matrices and one 16-byte vec4 of cascade split distances.
            for matrix in &self.shadow_matrix[1..] {
                for column in matrix.columns() {
                    for value in column {
                        bytes.extend(value.to_le_bytes());
                    }
                }
            }
            for value in SHADOW_CASCADE_SPLITS {
                bytes.extend(value.to_le_bytes());
            }
            debug_assert_eq!(bytes.len(), super::FRAME_BYTES as usize);
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
        for cascade in self.shadow {
            cascade.destroy(device)?;
        }
        for color in self.shadow_color {
            color.destroy(device)?;
        }
        device.destroy_buffer(self.frame)?;
        device.destroy_buffer(self.composite_frame)?;
        Ok(())
    }
}

#[cfg(test)]
mod stable_shadow_tests {
    use super::*;

    #[test]
    fn shadow_caster_cache_reuses_empty_fork_without_gpu_redraw() {
        let old = Arc::new(Scene::new());
        let same = Arc::new(old.fork());
        let bounds = crate::Aabb::new(
            Vec3::new(-10.0,-10.0,-10.0), Vec3::new(10.0,10.0,10.0),
        ).unwrap();
        assert!(!shadow_caster_scene_changed(Some(&old), &old, Mat4::IDENTITY, bounds));
        assert!(!shadow_caster_scene_changed(Some(&old), &same, Mat4::IDENTITY, bounds));
    }

    #[test]
    fn cascade_only_invalidates_for_changed_caster_within_its_region() {
        let previous = Arc::new(Scene::new());
        let mut changed = previous.fork();
        changed.insert(
            None,
            crate::Node::new().with_mesh(Arc::new(crate::Mesh::cube())),
        ).unwrap();
        let changed = Arc::new(changed);
        let near = crate::Aabb::new(
            Vec3::new(-12.0,-12.0,-12.0), Vec3::new(12.0,12.0,12.0),
        ).unwrap();
        let far = crate::Aabb::new(
            Vec3::new(400.0,400.0,400.0), Vec3::new(500.0,500.0,500.0),
        ).unwrap();
        assert!(shadow_caster_scene_changed(
            Some(&previous), &changed, Mat4::IDENTITY, near,
        ));
        assert!(!shadow_caster_scene_changed(
            Some(&previous), &changed, Mat4::IDENTITY, far,
        ));
        assert!(shadow_caster_scene_changed(None, &changed, Mat4::IDENTITY, far));
    }

    #[test]
    fn removed_caster_invalidates_its_old_cascade() {
        let mut baseline = Scene::new();
        let id = baseline.insert(
            None, crate::Node::new().with_mesh(Arc::new(crate::Mesh::cube())),
        ).unwrap();
        let baseline = Arc::new(baseline);
        let mut changed = baseline.fork();
        changed.remove(id).unwrap();
        let changed = Arc::new(changed);
        let near = crate::Aabb::new(
            Vec3::new(-12.0,-12.0,-12.0), Vec3::new(12.0,12.0,12.0),
        ).unwrap();
        assert!(shadow_caster_scene_changed(
            Some(&baseline), &changed, Mat4::IDENTITY, near,
        ));
    }

    #[test]
    fn all_cascades_use_texel_snapped_camera_projections() {
        let sun = Vec3::new(0.45, 0.81, 0.37);
        let eye = Vec3::new(17.0, 80.0, -12.0);
        for cascade in 0..SHADOW_CASCADE_COUNT {
            let (key, camera) = stable_shadow_camera(eye, sun, cascade).unwrap();
            let (moved, _) = stable_shadow_camera(
                eye + Vec3::new(0.0001, 0.0001, -0.0001), sun, cascade,
            ).unwrap();
            assert_eq!(key, moved, "cascade {cascade} must not jitter on subpixel motion");
            let light = (camera.eye - camera.target).normalized().unwrap();
            let right = camera.up.cross(light).normalized().unwrap();
            let vertical = light.cross(right).normalized().unwrap();
            let texel = SHADOW_CASCADE_WIDTHS[cascade] / SHADOW_MAP_SIZE as f32;
            for axis in [right, vertical] {
                let coordinate = camera.target.dot(axis) / texel;
                assert!((coordinate - coordinate.round()).abs() < 0.01);
            }
        }
    }

    #[test]
    fn each_cascade_caches_camera_motion_within_its_shadow_tile() {
        let sun = Vec3::new(0.45, 0.81, 0.37);
        let eye = Vec3::new(16.0, 72.0, -8.0);
        for cascade in 0..SHADOW_CASCADE_COUNT {
            let (key, first) = stable_shadow_camera(eye, sun, cascade).unwrap();
            for dx in [0.0001, 0.001, 0.01] {
                let (next_key, next) =
                    stable_shadow_camera(eye + Vec3::X * dx, sun, cascade).unwrap();
                assert_eq!(key, next_key);
                assert_eq!(first, next);
            }
        }
    }

    #[test]
    fn light_space_scrolling_moves_in_whole_texels() {
        let sun = Vec3::new(0.45, 0.81, 0.37);
        let eye = Vec3::new(0.0, 64.0, 0.0);
        for cascade in 0..SHADOW_CASCADE_COUNT {
            let (key, camera) = stable_shadow_camera(eye, sun, cascade).unwrap();
            let right = camera.up.cross((camera.eye - camera.target).normalized().unwrap())
                .normalized().unwrap();
            let texel = SHADOW_CASCADE_WIDTHS[cascade] / SHADOW_MAP_SIZE as f32;
            let (next_key, next) = stable_shadow_camera(
                eye + right * (texel * SHADOW_SCROLL_TEXELS * 1.5), sun, cascade,
            ).unwrap();
            assert_ne!(key, next_key);
            let displacement = (next.target - camera.target).dot(right) / texel;
            assert!((displacement - displacement.round()).abs() < 0.01);
        }
    }

    #[test]
    fn near_zenith_sun_motion_does_not_flip_shadow_projection_basis() {
        let eye = Vec3::new(0.0, 80.0, 0.0);
        let a = Vec3::new(0.1, 0.986, 0.07).normalized().unwrap();
        let b = Vec3::new(0.1, 0.996, 0.07).normalized().unwrap();
        let (_, camera_a) = stable_shadow_camera(eye, a, 0).unwrap();
        let (_, camera_b) = stable_shadow_camera(eye, b, 0).unwrap();
        assert!(camera_a.up.dot(camera_b.up) > 0.99,
            "sun passing near zenith must not rotate the entire shadow grid");
    }

    #[test]
    fn cascade_splits_fit_inside_their_projection_widths() {
        for cascade in 0..SHADOW_CASCADE_COUNT {
            assert!(
                SHADOW_CASCADE_SPLITS[cascade] + SHADOW_CASCADE_SPLITS[3]
                    < SHADOW_CASCADE_WIDTHS[cascade] * 0.5,
                "cascade {cascade} must fit its receiver radius and fade margin",
            );
        }
        assert!(SHADOW_CASCADE_WIDTHS[0] / (SHADOW_MAP_SIZE as f32) < 0.22);
        assert!(SHADOW_CASCADE_WIDTHS[2] > 320.0);
    }

    #[test]
    fn sun_extruded_caster_bounds_include_offscreen_upstream_casters() {
        let sun = Vec3::new(0.45, 0.81, 0.37).normalized().unwrap();
        let eye = Vec3::new(0.0, 64.0, 0.0);
        let bounds = shadow_caster_bounds(eye, sun, 0);
        let upstream = eye + sun * 128.0;
        assert!(upstream.x >= bounds.min.x && upstream.x <= bounds.max.x);
        assert!(upstream.y >= bounds.min.y && upstream.y <= bounds.max.y);
        assert!(upstream.z >= bounds.min.z && upstream.z <= bounds.max.z);
        let behind = eye - sun * 256.0;
        assert!(behind.y < bounds.min.y);
    }
}
