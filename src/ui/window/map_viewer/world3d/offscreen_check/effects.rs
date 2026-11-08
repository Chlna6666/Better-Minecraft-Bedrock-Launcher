//! Exercise the production SceneView multipass renderer against a read-only real world.
use super::*;
use gfx_core::*;
use gpui::{RendererExtension, RendererExtensionContext};
use gpui_3d::{Aabb, SceneView, ScreenEffects, VoxelIllumination, VoxelShading};

mod frames;
use frames::{Attachment, frame, inspect_passes};

fn capture<D: Device + TextureTransferDevice>(
    mut device: D,
    world: SnapshotWorld,
    spawn: [f64; 3],
) {
    let (mut scene, textures) = scene_from_world(&world);
    let light_count = world
        .lights
        .values()
        .map(|lights| lights.len())
        .sum::<usize>();
    for (&(x, z), lights) in &world.lights {
        for light in lights.iter() {
            scene
                .insert(
                    None,
                    Node::new()
                        .with_light(light.light)
                        .with_transform(Transform {
                            translation: Vec3::new((x * 16) as f32, 0., (z * 16) as f32),
                            ..Transform::IDENTITY
                        }),
                )
                .unwrap();
        }
    }
    let camera = snapshot_camera_for(&world, spawn);
    let size = Extent2d::new(1280, 800).unwrap();
    let color = Attachment::new(&mut device, size, Format::Rgba8Unorm);
    let depth = Attachment::new(&mut device, size, Format::Depth32Float);
    let pass = PipelineDevice::create_render_pass(
        &mut device,
        &RenderPassDescriptor {
            label: None,
            color_attachments: vec![ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            }],
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,
                stencil_format: None,
            }),

            sample_count: 1,
            resolve_target: None,

        },
    )
    .unwrap();
    let fixed =
        RendererExtensionContext::for_target(D::BACKEND_KIND, pass, Format::Rgba8Unorm, size);
    let context = || fixed.clone();
    let vibrant = std::env::var("BMCBL_WORLD3D_TEST_LIGHTING").as_deref() == Ok("vibrant");
    let settings = ScreenEffects {
        cloud_texture: world.cloud.as_ref().map(|cloud| cloud.id()),
        water_texture: world.water[0].as_ref().map(|v| gpui_3d::FlipbookTexture {
            texture: v.asset.id(),
            seconds_per_frame: v.seconds_per_frame,
        }),
        water_flow_texture: world.water[1].as_ref().map(|v| gpui_3d::FlipbookTexture {
            texture: v.asset.id(),
            seconds_per_frame: v.seconds_per_frame,
        }),
        ..ScreenEffects::default()
    };
    let view = SceneView::new(Arc::new(scene), camera)
        .with_textures(textures.into_values())
        .unwrap()
        .with_voxel_shader()
        .with_voxel_shading(VoxelShading {
            illumination: if vibrant {
                VoxelIllumination::Sunlight
            } else {
                VoxelIllumination::Unlit
            },
            sun_direction: Vec3::new(0.4, 0.8, 0.3).normalized().unwrap(),
            sun_intensity: 1.,
            shadow_strength: if vibrant { 0.9 } else { 0. },
            ..VoxelShading::default()
        })
        .unwrap()
        .with_screen_effects(vibrant.then_some(settings))
        .unwrap();
    let mut renderer = view.create_renderer(&mut device, context()).unwrap();
    frame(
        &mut device,
        renderer.as_mut(),
        &view,
        context(),
        &color,
        &depth,
    );
    let image = TextureTransferDevice::read_texture(&mut device, color.texture).unwrap();
    assert!(image.bytes.chunks_exact(4).all(|pixel| pixel[3] == 255));
    if let Ok(Some(memory)) = gpui::process_memory_snapshot() {
        eprintln!(
            "world3d prepared_memory private_bytes={} working_set_bytes={}",
            memory.private_bytes, memory.working_set_bytes
        );
    }
    let output = env_path("BMCBL_WORLD3D_TEST_SNAPSHOT")
        .unwrap_or_else(|| PathBuf::from("target/world3d-effects.png"));
    image::RgbaImage::from_raw(size.width(), size.height(), image.bytes)
        .unwrap()
        .save(&output)
        .unwrap();
    eprintln!(
        "world3d effects adapter={} size={}x{} scale={} face_capacity={} source_count={} occluder_proxies={}",
        device.adapter_name(),
        size.width(),
        size.height(),
        if vibrant {
            settings.resolution_scale
        } else {
            1.
        },
        world.meshes.values().map(|mesh| mesh.faces).sum::<usize>(),
        light_count,
        world
            .meshes
            .values()
            .map(|mesh| mesh.occluders.len())
            .sum::<usize>()
    );
    let profiles = if vibrant {
        vec![
            ("full", Some(settings)),
            (
                "without_atmosphere",
                Some(ScreenEffects {
                    clouds: false,
                    volumetric_fog: false,
                    ..settings
                }),
            ),
        ]
    } else {
        vec![("night_vision", None)]
    };
    for (profile, effects) in profiles {
        let view = view.with_screen_effects(effects).unwrap();
        frame(
            &mut device,
            renderer.as_mut(),
            &view,
            context(),
            &color,
            &depth,
        );
        let pixels = TextureTransferDevice::read_texture(&mut device, color.texture).unwrap();
        if profile == "without_atmosphere" {
            image::RgbaImage::from_raw(size.width(), size.height(), pixels.bytes)
                .unwrap()
                .save(output.with_file_name("world3d-effects-without-atmosphere.png"))
                .unwrap();
        }
        for moving in [false, true] {
            let mut cpu = Vec::new();
            let mut submit = Vec::new();
            let mut records = Vec::new();
            let mut waits = Vec::new();
            for index in 0..40 {
                let offset = if moving {
                    Vec3::new(
                        (index as f32 * 0.17).sin() * 4.,
                        0.,
                        (index as f32 * 0.17).cos() * 4.,
                    )
                } else {
                    Vec3::ZERO
                };
                let sample = view.with_camera(Camera {
                    eye: camera.eye + offset,
                    ..camera
                });
                let (encode, recorded, waited) = frame(
                    &mut device,
                    renderer.as_mut(),
                    &sample,
                    context(),
                    &color,
                    &depth,
                );
                cpu.push(encode);
                submit.push(recorded + waited);
                records.push(recorded);
                waits.push(waited);
            }
            cpu.sort_by(f64::total_cmp);
            submit.sort_by(f64::total_cmp);
            records.sort_by(f64::total_cmp);
            waits.sort_by(f64::total_cmp);
            eprintln!(
                "world3d effects {:?}: profile={profile} moving={moving} cpu_encode_p50_ms={:.3} cpu_encode_p95_ms={:.3} submit_wait_p50_ms={:.3} submit_wait_p95_ms={:.3} sections={} face_capacity={} lights={} clouds={} eye={:?} target={:?} -> {}",
                D::BACKEND_KIND,
                cpu[19],
                cpu[37],
                submit[19],
                submit[37],
                world.meshes.len(),
                world.meshes.values().map(|mesh| mesh.faces).sum::<usize>(),
                light_count,
                world.cloud.is_some(),
                camera.eye,
                camera.target,
                output.display()
            );
            eprintln!(
                "world3d native profile={profile} moving={moving} record_p50_ms={:.3} record_p95_ms={:.3} fence_wait_p50_ms={:.3} fence_wait_p95_ms={:.3}",
                records[19], records[37], waits[19], waits[37]
            );
        }
    }
    inspect_passes(
        &mut device,
        renderer.as_mut(),
        &view.with_camera(Camera {
            eye: camera.eye + Vec3::new(17., 0., 17.),
            ..camera
        }),
        context(),
        &color,
        &depth,
    );
    renderer.destroy(&mut device).unwrap();
    color.destroy(&mut device);
    depth.destroy(&mut device);
    PipelineDevice::destroy_render_pass(&mut device, pass).unwrap();
}

#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD, BMCBL_WORLD3D_TEST_LIGHTING=vibrant, resource pack and GPU"]
fn render_real_world_effects() {
    assert_eq!(
        std::env::var("BMCBL_WORLD3D_TEST_LIGHTING").as_deref(),
        Ok("vibrant")
    );
    render_world();
}

#[test]
#[ignore = "requires a read-only Bedrock world, resource pack and native GPU"]
fn render_real_world_default() {
    assert!(std::env::var("BMCBL_WORLD3D_TEST_LIGHTING").as_deref() != Ok("vibrant"));
    render_world();
}

fn percentile(samples: &mut [f64], fraction: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    let index = ((samples.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1));
    samples[index]
}

/// End-to-end streaming benchmark: real chunk service → ChunkScene registry →
/// SceneView publish → native record, while the camera moves horizontally and
/// Sections keep arriving. This mirrors the editor's per-frame pipeline and
/// reports the same phase split its HUD prints (poll/registry/encode/record/wait).
#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD, a native GPU; profiles chunk-load → render under camera motion"]
fn profile_streaming_move() {
    let Some(world_path) = env_path("BMCBL_WORLD3D_TEST_WORLD") else {
        eprintln!("skip: BMCBL_WORLD3D_TEST_WORLD is not set");
        return;
    };
    let package_roots = env_path("BMCBL_WORLD3D_TEST_PACKAGE")
        .into_iter()
        .collect::<Vec<_>>();
    let spawn = read_world_spawn(&world_path);
    let radius = env_i32("BMCBL_WORLD3D_TEST_RADIUS").unwrap_or(16).clamp(1, 64);
    let total_frames = env_i32("BMCBL_WORLD3D_TEST_SECONDS")
        .map(|seconds| seconds * 60)
        .unwrap_or(600)
        .clamp(60, 7200) as usize;
    // World blocks per presented frame along +X. 8 ≈ a fast fly-over.
    let speed = env_f32("BMCBL_WORLD3D_TEST_SPEED").unwrap_or(8.).clamp(0.5, 128.);
    let dimension = env_dimension();
    let service = spawn_chunk_service(ChunkServiceConfig {
        world_path: world_path.clone(),
        world_name: "world3d-stream".to_string(),
        dimension,
        package_roots,
        view_radius: radius,
    })
    .expect("start 3D chunk worker");
    service.send(ChunkServiceCommand::SetLighting(
        super::super::rendering::LightingMode::NightVision,
    ));
    let mut state = StreamState::new(service, dimension, radius);
    let mut position = [spawn[0], spawn[1] + 48., spawn[2]];
    state.send_view(position);

    // Warmup: stream the local neighbourhood before opening the renderer so
    // the measured loop starts from a representative scene.
    let warmup_deadline = Instant::now() + Duration::from_secs(180);
    let warmup_target = ((((2 * radius + 1) * (2 * radius + 1)) / 4).max(9)) as usize;
    while Instant::now() < warmup_deadline && state.meshes.len() < warmup_target {
        state.poll();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!state.meshes.is_empty(), "streaming warmup produced no Sections");
    eprintln!(
        "world3d stream warmup sections={} textures={} lights={}",
        state.meshes.len(),
        state.textures.len(),
        state.lights.len(),
    );

    let descriptor = DeviceDescriptor {
        power_preference: PowerPreference::HighPerformance,
        ..DeviceDescriptor::default()
    };
    match std::env::var("BMCBL_WORLD3D_TEST_BACKEND").as_deref() {
        Ok("dx11") => streaming_move(
            gfx_dx11::Dx11Device::new(&descriptor).unwrap(), &mut state,
            &mut position, total_frames, speed,
        ),
        Ok("vulkan") => streaming_move(
            gfx_vulkan::VulkanDevice::new(&descriptor).unwrap(), &mut state,
            &mut position, total_frames, speed,
        ),
        _ => streaming_move(
            gfx_dx12::Dx12Device::new(&descriptor).unwrap(), &mut state,
            &mut position, total_frames, speed,
        ),
    }
    state.service.shutdown();
}

/// Mutable editor-equivalent streaming state shared by the warmup and the
/// measured loop.
struct StreamState {
    service: ChunkServiceHandle,
    dimension: Dimension,
    radius: i32,
    generation: u64,
    meshes: BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
    lights: BTreeMap<ChunkPos, Arc<[super::super::types::World3dLocalLight]>>,
    textures: HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
    cloud: Option<Arc<gpui_3d::TextureAsset>>,
    water: [Option<super::super::types::World3dWaterTexture>; 2],
    dirty_sections: BTreeSet<(ChunkPos, i32)>,
    dirty_lights: BTreeSet<ChunkPos>,
}

impl StreamState {
    fn new(service: ChunkServiceHandle, dimension: Dimension, radius: i32) -> Self {
        Self {
            service,
            dimension,
            radius,
            generation: 1,
            meshes: BTreeMap::new(),
            lights: BTreeMap::new(),
            textures: HashMap::new(),
            cloud: None,
            water: [None, None],
            dirty_sections: BTreeSet::new(),
            dirty_lights: BTreeSet::new(),
        }
    }

    fn send_view(&mut self, position: [f64; 3]) {
        self.generation += 1;
        self.service.send(ChunkServiceCommand::SetView {
            generation: self.generation,
            dimension: self.dimension,
            camera: position,
            forward_xz: [1.0, 0.0],
            forward_y: -0.2,
            radius: self.radius,
            view_distance: 256.0,
        });
    }

    /// The editor's budgeted event drain: at most 16 mesh events or 2 ms.
    fn poll(&mut self) {
        const MAX_MESH_EVENTS_PER_POLL: usize = 16;
        let started = Instant::now();
        let mut added = 0usize;
        while let Some(event) = self.service.try_recv() {
            match event {
                ChunkServiceEvent::Ready(info) => {
                    for texture in info.water_textures.iter().flatten() {
                        self.textures.insert(texture.asset.id(), texture.asset.clone());
                    }
                    self.water = info.water_textures;
                    if let Some(texture) = info.cloud_texture {
                        self.textures.insert(texture.id(), texture.clone());
                        self.cloud = Some(texture);
                    }
                }
                ChunkServiceEvent::Chunk(payload) => {
                    for texture in payload.mesh.textures.iter() {
                        self.textures.insert(texture.id(), texture.clone());
                    }
                    let key = (payload.mesh.pos, payload.mesh.section_y);
                    self.meshes.insert(key, payload.mesh);
                    self.dirty_sections.insert(key);
                    added += 1;
                }
                ChunkServiceEvent::Completed(completions) => {
                    for completion in completions {
                        if completion.pos.dimension != self.dimension {
                            continue;
                        }
                        if completion.lights.is_empty() {
                            self.lights.remove(&completion.pos);
                        } else {
                            self.lights.insert(completion.pos, completion.lights);
                        }
                        self.dirty_lights.insert(completion.pos);
                        for mesh in completion.retained_meshes {
                            let key = (mesh.pos, mesh.section_y);
                            if self.meshes.contains_key(&key) {
                                continue;
                            }
                            for texture in mesh.textures.iter() {
                                self.textures.insert(texture.id(), texture.clone());
                            }
                            self.meshes.insert(key, mesh);
                        }
                        let stale = self
                            .meshes
                            .range((completion.pos, i32::MIN)..=(completion.pos, i32::MAX))
                            .filter_map(|(key, _)| {
                                (!completion.sections.contains(&key.1)).then_some(*key)
                            })
                            .collect::<Vec<_>>();
                        for key in stale {
                            self.meshes.remove(&key);
                            self.dirty_sections.insert(key);
                        }
                    }
                }
                ChunkServiceEvent::Unloaded { pos, .. } => {
                    if pos.dimension != self.dimension {
                        continue;
                    }
                    self.lights.remove(&pos);
                    self.dirty_lights.insert(pos);
                    let stale = self
                        .meshes
                        .range((pos, i32::MIN)..=(pos, i32::MAX))
                        .map(|(key, _)| *key)
                        .collect::<Vec<_>>();
                    for key in stale {
                        self.meshes.remove(&key);
                        self.dirty_sections.insert(key);
                    }
                }
                ChunkServiceEvent::Fatal { error } => panic!("chunk worker failed: {error}"),
                _ => {}
            }
            if added >= MAX_MESH_EVENTS_PER_POLL
                || started.elapsed() >= Duration::from_millis(2)
            {
                break;
            }
        }
    }
}

/// The measured streaming loop. One iteration = one editor frame: advance the
/// camera, drain worker events within budget, rebuild the Scene through the
/// ChunkScene registry when Sections changed, publish the SceneView and render.
fn streaming_move<D: Device + TextureTransferDevice>(
    mut device: D,
    state: &mut StreamState,
    position: &mut [f64; 3],
    total_frames: usize,
    speed: f32,
) {
    // Build the initial Scene through the registry exactly like the editor.
    let mut registry = super::super::editor::scene_registry::ChunkScene::default();
    let origin = |position: [f64; 3]| {
        [
            (position[0] / 512.0).round() as i32 * 512,
            0,
            (position[2] / 512.0).round() as i32 * 512,
        ]
    };
    let mut current_origin = origin(*position);
    let mut scene = registry
        .update(None, &state.meshes, None, current_origin)
        .expect("initial registry build");
    registry
        .update_lights(&mut scene, &state.lights, None, current_origin)
        .expect("initial light build");
    state.dirty_sections.clear();
    state.dirty_lights.clear();
    let mut scene = std::sync::Arc::new(scene);
    let camera_at = |position: [f64; 3]| {
        Camera::perspective(
            Vec3::new(position[0] as f32, position[1] as f32, position[2] as f32),
            Vec3::new(
                position[0] as f32 + 24.,
                position[1] as f32 - 28.,
                position[2] as f32,
            ),
            Vec3::Y,
            70_f32.to_radians(),
            0.05,
            512.0,
        )
    };
    let cull_radius = f64::from(state.radius.max(1));
    let culling_bounds = move |position: [f64; 3], origin: [i32; 3]| {
        let radius = cull_radius;
        let x = (position[0] / 16.0).floor();
        let z = (position[2] / 16.0).floor();
        Aabb {
            min: Vec3::new(
                ((x - radius) * 16.0 - f64::from(origin[0])) as f32,
                -f32::MAX,
                ((z - radius) * 16.0 - f64::from(origin[2])) as f32,
            ),
            max: Vec3::new(
                ((x + radius + 1.0) * 16.0 - f64::from(origin[0])) as f32,
                f32::MAX,
                ((z + radius + 1.0) * 16.0 - f64::from(origin[2])) as f32,
            ),
        }
    };
    let configured = SceneView::new(scene.clone(), camera_at(*position))
        .with_voxel_shader()
        .with_culling_bounds(Some(culling_bounds(*position, current_origin)))
        .map(|view| view.with_continuous_presentation())
        .expect("streaming scene view config");
    let mut view = configured
        .with_textures(state.textures.values().cloned())
        .expect("streaming texture table");
    let size = Extent2d::new(1280, 800).unwrap();
    let color = Attachment::new(&mut device, size, Format::Rgba8Unorm);
    let depth = Attachment::new(&mut device, size, Format::Depth32Float);
    let pass = PipelineDevice::create_render_pass(
        &mut device,
        &RenderPassDescriptor {
            label: None,
            color_attachments: vec![ColorAttachmentDescriptor {
                format: Format::Rgba8Unorm,
            }],
            depth_attachment: Some(DepthAttachmentDescriptor {
                format: Format::Depth32Float,
                stencil_format: None,
            }),

            sample_count: 1,
            resolve_target: None,

        },
    )
    .unwrap();
    let context = RendererExtensionContext::for_target(
        D::BACKEND_KIND, pass, Format::Rgba8Unorm, size,
    );
    let mut renderer = view.create_renderer(&mut device, context.clone()).unwrap();

    let mut poll_ms = Vec::new();
    let mut registry_ms = Vec::new();
    let mut encode_ms = Vec::new();
    let mut record_ms = Vec::new();
    let mut wait_ms = Vec::new();
    let mut private_mib_samples = Vec::<u64>::new();
    let mut publishes = 0usize;
    let mut last_chunk = i32::MIN;
    let mut published_texture_count = state.textures.len();
    for frame_index in 0..total_frames {
        // One editor frame: move, poll, rebuild, render.
        position[0] += f64::from(speed);
        let chunk_x = (position[0] / 16.0).floor() as i32;
        if last_chunk != chunk_x {
            last_chunk = chunk_x;
            state.send_view(*position);
        }
        let started = Instant::now();
        state.poll();
        poll_ms.push(started.elapsed().as_secs_f64() * 1000.);

        let started = Instant::now();
        let next_origin = origin(*position);
        let scene_dirty =
            !state.dirty_sections.is_empty() || !state.dirty_lights.is_empty();
        if scene_dirty || next_origin != current_origin {
            let changed = std::mem::take(&mut state.dirty_sections);
            let changed_lights = std::mem::take(&mut state.dirty_lights);
            let mut next = registry
                .update(
                    Some(scene.as_ref()), &state.meshes, Some(&changed), next_origin,
                )
                .expect("streaming registry update");
            registry
                .update_lights(&mut next, &state.lights, Some(&changed_lights), next_origin)
                .expect("streaming light update");
            scene = std::sync::Arc::new(next);
            current_origin = next_origin;
            view = view
                .with_scene(scene.clone())
                .with_camera(camera_at(*position))
                .with_culling_bounds(Some(culling_bounds(*position, current_origin)))
                .expect("streaming culling bounds");
            if state.textures.len() != published_texture_count {
                published_texture_count = state.textures.len();
                view = view
                    .with_textures(state.textures.values().cloned())
                    .expect("streaming texture refresh");
            }
            publishes += 1;
        } else {
            view = view.with_camera(camera_at(*position));
        }
        registry_ms.push(started.elapsed().as_secs_f64() * 1000.);

        let (encode, record, wait) = frame(
            &mut device, renderer.as_mut(), &view, context.clone(),
            &color, &depth,
        );
        encode_ms.push(encode);
        record_ms.push(record);
        wait_ms.push(wait);
        if let Some(memory) = gpui::process_memory_snapshot().ok().flatten() {
            private_mib_samples.push(memory.private_bytes / 1048576);
        }

        if (frame_index + 1) % 120 == 0 {
            let sections = state.meshes.len();
            let faces = state.meshes.values().map(|mesh| mesh.faces).sum::<usize>();
            let memory = gpui::process_memory_snapshot().ok().flatten();
            eprintln!(
                "world3d stream frame={} sections={} faces={} publishes={} frame_ms={:.2} (poll {:.2} registry {:.2} encode {:.2} record {:.2} wait {:.2}) private_mib={}",
                frame_index + 1,
                sections,
                faces,
                publishes,
                poll_ms.last().unwrap_or(&0.) + registry_ms.last().unwrap_or(&0.)
                    + encode + record + wait,
                poll_ms.last().unwrap_or(&0.),
                registry_ms.last().unwrap_or(&0.),
                encode, record, wait,
                memory.map_or(0, |memory| memory.private_bytes / 1048576),
            );
        }
    }
    let faces = state.meshes.values().map(|mesh| mesh.faces).sum::<usize>();
    // Frame totals per index must be zipped before the percentile sorts below.
    let frame_total = poll_ms
        .iter()
        .zip(&registry_ms)
        .zip(&encode_ms)
        .zip(&record_ms)
        .zip(&wait_ms)
        .map(|((((a, b), c), d), e)| a + b + c + d + e)
        .collect::<Vec<_>>();
    eprintln!(
        "world3d stream summary sections={} faces={} publishes={} poll_p50={:.2} poll_p95={:.2} registry_p50={:.2} registry_p95={:.2} encode_p50={:.2} encode_p95={:.2} record_p50={:.2} record_p95={:.2} wait_p50={:.2} wait_p95={:.2} frame_p50={:.2} frame_p95={:.2}",
        state.meshes.len(),
        faces,
        publishes,
        percentile(&mut poll_ms, 0.5),
        percentile(&mut poll_ms, 0.95),
        percentile(&mut registry_ms, 0.5),
        percentile(&mut registry_ms, 0.95),
        percentile(&mut encode_ms, 0.5),
        percentile(&mut encode_ms, 0.95),
        percentile(&mut record_ms, 0.5),
        percentile(&mut record_ms, 0.95),
        percentile(&mut wait_ms, 0.5),
        percentile(&mut wait_ms, 0.95),
        percentile(&mut { frame_total.clone() }, 0.5),
        percentile(&mut { frame_total.clone() }, 0.95),
    );
    let frame_p99 = percentile(&mut { frame_total.clone() }, 0.99);
    let frame_max = frame_total.iter().copied().fold(0.0f64, f64::max);
    let peak_private_mib = private_mib_samples.iter().copied().max().unwrap_or(0);
    // Separate the cold-start publication from the steady streaming frames.
    // The standing goal is *stable* 120fps, so a p99 failure must say whether
    // it comes from the first presented frame or from ongoing streaming.
    let first_frame_ms = frame_total.first().copied().unwrap_or(0.0);
    let steady: Vec<f64> = frame_total.iter().skip(1).copied().collect();
    let steady_p99 = if steady.is_empty() {
        0.0
    } else {
        percentile(&mut { steady.clone() }, 0.99)
    };
    eprintln!(
        "world3d stream gate frame_p99={:.2} frame_max={:.2} first_frame_ms={:.2} steady_p99={:.2} peak_private_mib={} budget_ms=8.30 memory_budget_mib=3072",
        frame_p99, frame_max, first_frame_ms, steady_p99, peak_private_mib,
    );
    // Opt-in acceptance gate: the standing goal is *stable* 120fps, so the
    // worst frames and peak residency are the acceptance criteria, not p50.
    if std::env::var("BMCBL_WORLD3D_TEST_ENFORCE_GATE").as_deref() == Ok("1") {
        assert!(
            frame_p99 <= 8.30,
            "stable-120fps gate failed: frame p99 {:.2}ms exceeds the 8.3ms budget",
            frame_p99
        );
        assert!(
            peak_private_mib <= 3072,
            "memory gate failed: peak private {} MiB exceeds 3GB",
            peak_private_mib
        );
    }
    if let Ok(Some(memory)) = gpui::process_memory_snapshot() {
        eprintln!(
            "world3d stream memory private_mib={} working_set_mib={}",
            memory.private_bytes / 1048576,
            memory.working_set_bytes / 1048576,
        );
    }
    renderer.destroy(&mut device).unwrap();
    color.destroy(&mut device);
    depth.destroy(&mut device);
    PipelineDevice::destroy_render_pass(&mut device, pass).unwrap();
}

fn render_world() {
    let world = env_path("BMCBL_WORLD3D_TEST_WORLD").expect("read-only world root");
    let roots = env_path("BMCBL_WORLD3D_TEST_PACKAGE").into_iter().collect();
    let spawn = read_world_spawn(&world);
    let radius = env_i32("BMCBL_WORLD3D_TEST_RADIUS")
        .unwrap_or(3)
        .clamp(1, 64);
    let (world, mut service) = collect_world(
        &world,
        roots,
        spawn,
        radius,
        ((2 * radius + 1).pow(2) / 2).max(9) as usize,
    );
    service.shutdown();
    assert!(!world.meshes.is_empty());
    let descriptor = DeviceDescriptor {
        power_preference: PowerPreference::HighPerformance,
        ..DeviceDescriptor::default()
    };
    match std::env::var("BMCBL_WORLD3D_TEST_BACKEND").as_deref() {
        Ok("dx11") => capture(
            gfx_dx11::Dx11Device::new(&descriptor).unwrap(),
            world,
            spawn,
        ),
        Ok("vulkan") => capture(
            gfx_vulkan::VulkanDevice::new(&descriptor).unwrap(),
            world,
            spawn,
        ),
        _ => capture(
            gfx_dx12::Dx12Device::new(&descriptor).unwrap(),
            world,
            spawn,
        ),
    }
}
