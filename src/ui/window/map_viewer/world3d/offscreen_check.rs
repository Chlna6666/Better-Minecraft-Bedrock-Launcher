//! Ignored local-world verification for the streaming 3D renderer.
//!
//! These tests intentionally require a real Bedrock world and (for the snapshot) a native
//! GPU. They never run in CI. Run locally with:
//!
//!   $env:BMCBL_WORLD3D_TEST_WORLD='<world root>'
//!   $env:BMCBL_WORLD3D_TEST_PACKAGE='<versions/26.40>'
//!   cargo test -p bmcbl --lib world3d::offscreen_check -- --ignored --nocapture
//!
//! The snapshot test writes an image to `target/world3d-snapshot.png`.

use super::chunk_service::{ChunkServiceConfig, ChunkServiceHandle, spawn_chunk_service};
use super::types::{ChunkServiceCommand, ChunkServiceEvent, World3dChunkMesh, World3dChunkProbe};
use ::bedrock_world::block::BlockState;
use ::bedrock_world::level::read_level_dat_document;
use ::bedrock_world::surface::{
    BiomeDataRequirement, ChunkData, ChunkDataRequest, ChunkLoadOptions, ChunkLoadPriority,
    WorldPipelineOptions, WorldThreadingOptions,
};
use ::bedrock_world::world::{OpenOptions, World};
use ::bedrock_world::{ChunkPos, Dimension, NbtTag, SubChunkDecodeMode};
use gpui_3d::{
    Camera, Node, OffscreenRenderer, RenderOptions, Scene, ShadowOptions, Transform, Vec3,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct SnapshotWorld {
    meshes: BTreeMap<(i32, i32, i32), Arc<World3dChunkMesh>>,
    textures: HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
    probes: BTreeMap<(i32, i32), Arc<World3dChunkProbe>>,
    completed_columns: BTreeSet<(i32, i32)>,
    missing_columns: usize,
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

fn env_i32(name: &str) -> Option<i32> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn env_f32(name: &str) -> Option<f32> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn env_coords(name: &str) -> Option<[f64; 3]> {
    let raw = std::env::var(name).ok()?;
    let values = raw
        .split(',')
        .map(str::trim)
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    (values.len() == 3).then(|| [values[0], values[1], values[2]])
}

/// Resolves the snapshot camera from BMCBL_WORLD3D_TEST_EYE/TARGET/FOV/FAR, or derives an
/// automatic overview from the loaded chunks.
fn env_dimension() -> Dimension {
    match std::env::var("BMCBL_WORLD3D_TEST_DIMENSION")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "nether" => Dimension::Nether,
        "end" => Dimension::End,
        _ => Dimension::Overworld,
    }
}

fn snapshot_camera_for(world: &SnapshotWorld, spawn: [f64; 3]) -> Camera {
    let (Some(eye), Some(target)) = (
        env_coords("BMCBL_WORLD3D_TEST_EYE"),
        env_coords("BMCBL_WORLD3D_TEST_TARGET"),
    ) else {
        return snapshot_camera(world, spawn);
    };
    let fov = env_f32("BMCBL_WORLD3D_TEST_FOV")
        .unwrap_or(70.0)
        .clamp(10.0, 120.0);
    let far = env_f32("BMCBL_WORLD3D_TEST_FAR").unwrap_or(512.0).max(16.0);
    Camera::perspective(
        Vec3::new(eye[0] as f32, eye[1] as f32, eye[2] as f32),
        Vec3::new(target[0] as f32, target[1] as f32, target[2] as f32),
        Vec3::Y,
        fov.to_radians(),
        0.05,
        far,
    )
}

fn read_world_spawn(world_path: &std::path::Path) -> [f64; 3] {
    if let (Some(x), Some(y), Some(z)) = (
        std::env::var("BMCBL_WORLD3D_TEST_X")
            .ok()
            .and_then(|v| v.parse::<i32>().ok()),
        std::env::var("BMCBL_WORLD3D_TEST_Y")
            .ok()
            .and_then(|v| v.parse::<i32>().ok()),
        std::env::var("BMCBL_WORLD3D_TEST_Z")
            .ok()
            .and_then(|v| v.parse::<i32>().ok()),
    ) {
        return [f64::from(x) + 0.5, f64::from(y) + 1.6, f64::from(z) + 0.5];
    }
    let document = read_level_dat_document(&world_path.join("level.dat"))
        .expect("readable level.dat for the configured world");
    let NbtTag::Compound(root) = &document.root else {
        panic!("level.dat root is not a compound");
    };
    let read = |key: &str| match root.get(key) {
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        _ => None,
    };
    let x = read("SpawnX").expect("level.dat SpawnX");
    let y = read("SpawnY").expect("level.dat SpawnY");
    let z = read("SpawnZ").expect("level.dat SpawnZ");
    [f64::from(x) + 0.5, f64::from(y) + 1.6, f64::from(z) + 0.5]
}

fn collect_world(
    world_path: &std::path::Path,
    package_roots: Vec<PathBuf>,
    spawn: [f64; 3],
    radius: i32,
    minimum_chunks: usize,
) -> (SnapshotWorld, ChunkServiceHandle) {
    collect_world_with_timeout(
        world_path,
        package_roots,
        spawn,
        radius,
        minimum_chunks,
        Duration::from_secs(240),
    )
}

#[allow(clippy::too_many_arguments)]
fn collect_world_with_timeout(
    world_path: &std::path::Path,
    package_roots: Vec<PathBuf>,
    spawn: [f64; 3],
    radius: i32,
    minimum_chunks: usize,
    timeout: Duration,
) -> (SnapshotWorld, ChunkServiceHandle) {
    let dimension = env_dimension();
    let service = spawn_chunk_service(ChunkServiceConfig {
        world_path: world_path.to_path_buf(),
        world_name: "world3d-test".to_string(),
        dimension,
        package_roots,
        view_radius: radius,
    })
    .expect("start 3D chunk worker");
    let generation = 1;
    service.send(ChunkServiceCommand::SetView {
        generation,
        dimension,
        camera: spawn,
        forward_xz: [0.0, -1.0],
        forward_y: 0.0,
        radius,
        view_distance: 256.0,
    });
    let mut result = SnapshotWorld {
        meshes: BTreeMap::new(),
        textures: HashMap::new(),
        probes: BTreeMap::new(),
        completed_columns: BTreeSet::new(),
        missing_columns: 0,
    };
    let deadline = Instant::now() + timeout;
    let mut last_report = Instant::now();
    while Instant::now() < deadline {
        while let Some(event) = service.try_recv() {
            match event {
                ChunkServiceEvent::Chunk(payload) => {
                    for texture in payload.mesh.textures.iter() {
                        result.textures.insert(texture.id(), texture.clone());
                    }
                    result.meshes.insert(
                        (
                            payload.mesh.pos.x,
                            payload.mesh.pos.z,
                            payload.mesh.section_y,
                        ),
                        payload.mesh,
                    );
                    result
                        .probes
                        .insert((payload.probe.pos.x, payload.probe.pos.z), payload.probe);
                }
                ChunkServiceEvent::Completed(completions) => {
                    for completion in completions {
                        for mesh in completion.retained_meshes {
                            let key = (mesh.pos.x, mesh.pos.z, mesh.section_y);
                            if let std::collections::btree_map::Entry::Vacant(entry) =
                                result.meshes.entry(key)
                            {
                                for texture in mesh.textures.iter() {
                                    result.textures.insert(texture.id(), texture.clone());
                                }
                                entry.insert(mesh);
                            }
                        }
                        result
                            .completed_columns
                            .insert((completion.pos.x, completion.pos.z));
                        let stale = result
                            .meshes
                            .range(
                                (completion.pos.x, completion.pos.z, i32::MIN)
                                    ..=(completion.pos.x, completion.pos.z, i32::MAX),
                            )
                            .filter_map(|(key, _)| {
                                (!completion.sections.contains(&key.2)).then_some(*key)
                            })
                            .collect::<Vec<_>>();
                        for key in stale {
                            result.meshes.remove(&key);
                        }
                        result
                            .probes
                            .insert((completion.pos.x, completion.pos.z), completion.probe);
                    }
                }
                ChunkServiceEvent::Fatal { error } => panic!("chunk worker failed: {error}"),
                ChunkServiceEvent::Status(status) => {
                    result.missing_columns = status.missing_chunks;
                    assert!(
                        status.last_error.as_deref().unwrap_or("").is_empty()
                            || status.loaded_chunks > 0,
                        "chunk worker error before producing chunks: {:?}",
                        status.last_error
                    );
                }
                _ => {}
            }
        }
        if result.probes.len() >= minimum_chunks {
            // Drain a little longer so the visible radius is mostly complete.
            std::thread::sleep(Duration::from_millis(400));
            break;
        }
        if last_report.elapsed() >= Duration::from_secs(5) {
            last_report = Instant::now();
            eprintln!(
                "world3d test: {} chunks, {} textures, {} probes",
                result.meshes.len(),
                result.textures.len(),
                result.probes.len()
            );
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    (result, service)
}

fn scene_from_world(
    world: &SnapshotWorld,
) -> (
    Scene,
    HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
) {
    let mut scene = Scene::new();
    for mesh in world.meshes.values() {
        let translation = Vec3::new(
            mesh.origin[0] as f32,
            mesh.origin[1] as f32,
            mesh.origin[2] as f32,
        );
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(mesh.mesh.clone())
                    .with_materials(mesh.materials.iter().cloned())
                    .with_transform(Transform {
                        translation,
                        ..Transform::IDENTITY
                    }),
            )
            .expect("valid chunk scene node");
    }
    let textures = world
        .textures
        .iter()
        .map(|(id, texture)| (*id, texture.clone()))
        .collect();
    (scene, textures)
}

fn surface_y_at(world: &SnapshotWorld, x: i32, z: i32) -> Option<i32> {
    let probe = world.probes.get(&(x.div_euclid(16), z.div_euclid(16)))?;
    let (min_y, max_y) = probe.pos.y_range(::bedrock_world::ChunkVersion::New);
    (min_y..=max_y)
        .rev()
        .find(|y| probe.block_at(x, *y, z).is_some())
}

fn snapshot_camera(world: &SnapshotWorld, spawn: [f64; 3]) -> Camera {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_z = f32::INFINITY;
    let mut max_z = f32::NEG_INFINITY;
    for mesh in world.meshes.values() {
        min_x = min_x.min((mesh.pos.x * 16) as f32);
        max_x = max_x.max((mesh.pos.x * 16 + 16) as f32);
        min_z = min_z.min((mesh.pos.z * 16) as f32);
        max_z = max_z.max((mesh.pos.z * 16 + 16) as f32);
    }
    let center_x = (min_x + max_x) * 0.5;
    let center_z = (min_z + max_z) * 0.5;
    let span = (max_x - min_x).max(max_z - min_z).max(32.0);
    let surface =
        surface_y_at(world, spawn[0].floor() as i32, spawn[2].floor() as i32).unwrap_or(72);
    let target = Vec3::new(center_x, surface as f32 + 2.0, center_z);
    let eye = Vec3::new(
        center_x + span * 0.26,
        surface as f32 + span * 0.20,
        center_z + span * 0.26,
    );
    Camera::perspective(
        eye,
        target,
        Vec3::Y,
        55.0_f32.to_radians(),
        0.05,
        (span * 10.0).max(512.0),
    )
}

#[cfg(windows)]
fn create_device() -> (&'static str, Box<dyn DeviceBundle>) {
    let descriptor = gfx_core::DeviceDescriptor {
        application_name: "BMCBL 3D world snapshot".to_string(),
        power_preference: gfx_core::PowerPreference::HighPerformance,
        ..gfx_core::DeviceDescriptor::default()
    };
    if gfx_dx11::Dx11Device::new(&descriptor).is_ok() {
        return ("dx11", Box::new(Dx11Bundle));
    }
    if gfx_dx12::Dx12Device::new(&descriptor).is_ok() {
        return ("dx12", Box::new(Dx12Bundle));
    }
    if gfx_vulkan::VulkanDevice::new(&descriptor).is_ok() {
        return ("vulkan", Box::new(VulkanBundle));
    }
    panic!("no native GPU backend could be created");
}

trait DeviceBundle {
    fn render(
        &mut self,
        scene: &Scene,
        textures: &HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> gpui_3d::CapturedImage;
}

#[cfg(windows)]
struct Dx11Bundle;
#[cfg(windows)]
impl DeviceBundle for Dx11Bundle {
    fn render(
        &mut self,
        scene: &Scene,
        textures: &HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> gpui_3d::CapturedImage {
        let mut device = gfx_dx11::Dx11Device::new(&gfx_core::DeviceDescriptor {
            application_name: "BMCBL 3D world snapshot".to_string(),
            ..gfx_core::DeviceDescriptor::default()
        })
        .expect("DX11 device");
        render_scene(&mut device, scene, textures, camera, width, height)
    }
}

#[cfg(windows)]
struct Dx12Bundle;
#[cfg(windows)]
impl DeviceBundle for Dx12Bundle {
    fn render(
        &mut self,
        scene: &Scene,
        textures: &HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> gpui_3d::CapturedImage {
        let mut device = gfx_dx12::Dx12Device::new(&gfx_core::DeviceDescriptor {
            application_name: "BMCBL 3D world snapshot".to_string(),
            ..gfx_core::DeviceDescriptor::default()
        })
        .expect("DX12 device");
        render_scene(&mut device, scene, textures, camera, width, height)
    }
}

#[cfg(windows)]
struct VulkanBundle;
#[cfg(windows)]
impl DeviceBundle for VulkanBundle {
    fn render(
        &mut self,
        scene: &Scene,
        textures: &HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
        camera: Camera,
        width: u32,
        height: u32,
    ) -> gpui_3d::CapturedImage {
        let mut device = gfx_vulkan::VulkanDevice::new(&gfx_core::DeviceDescriptor {
            application_name: "BMCBL 3D world snapshot".to_string(),
            ..gfx_core::DeviceDescriptor::default()
        })
        .expect("Vulkan device");
        render_scene(&mut device, scene, textures, camera, width, height)
    }
}

fn render_scene<D>(
    device: &mut D,
    scene: &Scene,
    textures: &HashMap<gpui_3d::TextureAssetId, Arc<gpui_3d::TextureAsset>>,
    camera: Camera,
    width: u32,
    height: u32,
) -> gpui_3d::CapturedImage
where
    D: gfx_core::Device + gfx_core::TextureTransferDevice + ?Sized,
{
    let mut renderer = OffscreenRenderer::new(device, width, height).expect("offscreen renderer");
    let mut options = RenderOptions {
        shadows: ShadowOptions {
            enabled: false,
            ..ShadowOptions::default()
        },
        // Keep the snapshot display-referred like the in-app scene view: no tone mapping, no
        // FXAA blur and no exposure curve.
        clear_color: [0.38, 0.58, 0.86, 1.0],
        post_process: false,
        tonemap: gpui_3d::Tonemap::None,
        antialias: gpui_3d::AntialiasMode::None,
        material_sampling: gpui_3d::TextureSampling::PixelArt,
        ..RenderOptions::default()
    };
    options.bloom = gpui_3d::BloomOptions {
        enabled: false,
        ..gpui_3d::BloomOptions::default()
    };
    let image = renderer
        .render_to_image(device, scene, textures, camera, &options)
        .expect("render world snapshot");
    renderer
        .destroy(device)
        .expect("destroy offscreen renderer");
    image
}

#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD pointing at a real Bedrock world"]
fn stream_real_world_produces_textured_meshes() {
    let Some(world_path) = env_path("BMCBL_WORLD3D_TEST_WORLD") else {
        eprintln!("skip: BMCBL_WORLD3D_TEST_WORLD is not set");
        return;
    };
    let package_roots = env_path("BMCBL_WORLD3D_TEST_PACKAGE")
        .into_iter()
        .collect::<Vec<_>>();
    let spawn = read_world_spawn(&world_path);
    let (world, mut service) = collect_world(&world_path, package_roots, spawn, 2, 9);
    service.shutdown();
    assert!(!world.meshes.is_empty(), "no chunk meshes were produced");
    let sections = world
        .meshes
        .values()
        .map(|mesh| mesh.mesh.parts().len())
        .sum::<usize>();
    assert!(
        sections >= world.meshes.len(),
        "chunk section parts are missing"
    );
    let faces = world.meshes.values().map(|mesh| mesh.faces).sum::<usize>();
    let missing = world
        .meshes
        .values()
        .map(|mesh| mesh.missing_texture_faces)
        .sum::<usize>();
    let materials = world
        .meshes
        .values()
        .map(|mesh| mesh.materials.len())
        .sum::<usize>();
    eprintln!(
        "world3d streaming: {} chunks, {} section parts, {} faces, {} materials, {} textures, {} missing-texture faces, spawn {:?}",
        world.meshes.len(),
        sections,
        faces,
        materials,
        world.textures.len(),
        missing,
        spawn
    );
    assert!(faces > 0, "no faces were produced");
    assert!(
        !world.textures.is_empty(),
        "vanilla resource-pack textures were not resolved"
    );
    assert!(
        missing * 4 < faces,
        "too many faces without a resolved texture: {missing}/{faces}"
    );
    assert!(!world.probes.is_empty(), "no chunk probes were produced");
}

#[cfg(windows)]
#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD and a native GPU"]
fn render_real_world_snapshot() {
    let Some(world_path) = env_path("BMCBL_WORLD3D_TEST_WORLD") else {
        eprintln!("skip: BMCBL_WORLD3D_TEST_WORLD is not set");
        return;
    };
    let package_roots = env_path("BMCBL_WORLD3D_TEST_PACKAGE")
        .into_iter()
        .collect::<Vec<_>>();
    let spawn = read_world_spawn(&world_path);
    let radius = env_i32("BMCBL_WORLD3D_TEST_RADIUS")
        .unwrap_or(3)
        .clamp(1, 8);
    let side = usize::try_from(2 * radius + 1).unwrap_or(1);
    let minimum_chunks = (side * side / 2).max(9);
    let (world, mut service) =
        collect_world(&world_path, package_roots, spawn, radius, minimum_chunks);
    service.shutdown();
    assert!(!world.meshes.is_empty(), "no chunk meshes were produced");
    let (scene, textures) = scene_from_world(&world);
    let camera = snapshot_camera_for(&world, spawn);
    let (backend, mut bundle) = create_device();
    let started = Instant::now();
    let image = bundle.render(&scene, &textures, camera, 1600, 1000);
    let elapsed = started.elapsed();
    let output = env_path("BMCBL_WORLD3D_TEST_SNAPSHOT")
        .unwrap_or_else(|| PathBuf::from("target/world3d-snapshot.png"));
    if let Some(parent) = output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    image::RgbaImage::from_raw(image.width(), image.height(), image.rgba().to_vec())
        .expect("RGBA frame")
        .save(&output)
        .expect("write snapshot PNG");
    eprintln!(
        "world3d snapshot: backend={backend} chunks={} textures={} size={}x{} render_ms={} -> {}",
        world.meshes.len(),
        textures.len(),
        image.width(),
        image.height(),
        elapsed.as_millis(),
        output.display()
    );
}
fn audit_chunk_load_options() -> ChunkLoadOptions {
    ChunkLoadOptions {
        data_request: ChunkDataRequest::new()
            .full_3d_indices()
            .biome(BiomeDataRequirement::All)
            .block_entities(),
        subchunk_decode: SubChunkDecodeMode::FullIndices,
        threading: WorldThreadingOptions::Single,
        pipeline: WorldPipelineOptions::default(),
        cancel: None,
        progress: None,
        priority: ChunkLoadPriority::DistanceFrom {
            chunk_x: 0,
            chunk_z: 0,
        },
        storage_cache_policy: Default::default(),
    }
}

fn audit_block_state_at<'a>(data: &'a ChunkData, x: i32, y: i32, z: i32) -> Option<&'a BlockState> {
    let sub_y = i8::try_from(y.div_euclid(16)).ok()?;
    let subchunk = data.subchunks.get(&sub_y)?;
    subchunk.block_state_at(
        x.rem_euclid(16) as u8,
        y.rem_euclid(16) as u8,
        z.rem_euclid(16) as u8,
    )
}

fn audit_interesting_name(name: &str) -> bool {
    [
        "redstone",
        "repeater",
        "comparator",
        "piston",
        "observer",
        "torch",
        "ladder",
        "vine",
        "grass",
        "fern",
        "flower",
        "sapling",
        "bush",
        "roots",
        "kelp",
        "seagrass",
        "pane",
        "fence",
        "fence_gate",
        "door",
        "trapdoor",
        "stairs",
        "slab",
        "wall",
        "pane",
        "bars",
        "button",
        "pressure_plate",
        "rail",
        "wall",
        "stairs",
        "slab",
        "trapdoor",
        "door",
        "rail",
        "carpet",
        "candle",
        "lantern",
        "button",
        "lever",
        "sign",
        "banner",
        "pot",
        "bed",
        "frame",
        "chain",
        "cobweb",
        "anvil",
        "stone",
        "cobblestone",
        "smooth_stone",
        "deepslate",
        "chest",
        "barrel",
        "scaffolding",
        "snow",
        "water",
        "lava",
    ]
    .iter()
    .any(|needle| name.contains(needle))
}

#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD pointing at a real Bedrock world"]
fn audit_real_world_blocks() {
    let Some(world_path) = env_path("BMCBL_WORLD3D_TEST_WORLD") else {
        eprintln!("skip: BMCBL_WORLD3D_TEST_WORLD is not set");
        return;
    };
    let center = read_world_spawn(&world_path);
    let center_x = center[0].floor() as i32;
    let center_z = center[2].floor() as i32;
    let radius = env_i32("BMCBL_WORLD3D_TEST_AUDIT_RADIUS")
        .unwrap_or(2)
        .clamp(1, 5);
    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = World::open(&world_path, options).expect("open world");
    let base_chunk_x = center_x.div_euclid(16);
    let base_chunk_z = center_z.div_euclid(16);
    let mut names = std::collections::BTreeMap::<String, usize>::new();
    let mut samples = std::collections::BTreeMap::<String, Vec<String>>::new();
    let mut frames = Vec::<String>::new();
    let mut loaded_chunks = 0usize;
    for dx in -radius..=radius {
        for dz in -radius..=radius {
            let pos = ChunkPos {
                x: base_chunk_x + dx,
                z: base_chunk_z + dz,
                dimension: env_dimension(),
            };
            let Ok(data) = world.query_chunk_data(pos, audit_chunk_load_options()) else {
                continue;
            };
            if !data.is_loaded {
                continue;
            }
            loaded_chunks += 1;
            for entity in &data.block_entities {
                let Some(position) = entity.position else {
                    continue;
                };
                let state = audit_block_state_at(&data, position[0], position[1], position[2]);
                let is_frame_block = state.is_some_and(|state| {
                    matches!(
                        state.name.as_str(),
                        "minecraft:frame" | "minecraft:glow_frame"
                    )
                });
                let is_frame_entity = entity
                    .id
                    .as_deref()
                    .is_some_and(|id| id.to_ascii_lowercase().contains("frame"));
                if !is_frame_block && !is_frame_entity {
                    continue;
                }
                let mut neighbors = Vec::new();
                for offset in [
                    [0, 1, 0],
                    [0, -1, 0],
                    [0, 0, -1],
                    [0, 0, 1],
                    [-1, 0, 0],
                    [1, 0, 0],
                ] {
                    let name = audit_block_state_at(
                        &data,
                        position[0] + offset[0],
                        position[1] + offset[1],
                        position[2] + offset[2],
                    )
                    .map_or("?", |state| {
                        state.name.strip_prefix("minecraft:").unwrap_or(&state.name)
                    });
                    neighbors.push(name.to_string());
                }
                let nbt = format!("{:?}", entity.nbt);
                let nbt = nbt.chars().take(500).collect::<String>();
                frames.push(format!(
                    "frame pos={position:?} entity={:?} block={:?} states={:?} neighbors[up,down,north,south,west,east]={neighbors:?} nbt={nbt}",
                    entity.id,
                    state.map(|state| state.name.as_str()),
                    state.map(|state| &state.states)
                ));
            }
            for subchunk in data.subchunks.values() {
                for local_y in 0u8..16 {
                    for local_z in 0u8..16 {
                        for local_x in 0u8..16 {
                            let Some(state) = subchunk.block_state_at(local_x, local_y, local_z)
                            else {
                                continue;
                            };
                            let name = state
                                .name
                                .strip_prefix("minecraft:")
                                .unwrap_or(&state.name)
                                .to_string();
                            *names.entry(name.clone()).or_default() += 1;
                            if audit_interesting_name(&name) {
                                let entry = samples.entry(name).or_default();
                                if entry.len() < 6 {
                                    let x = pos.x * 16 + i32::from(local_x);
                                    let y = i32::from(subchunk.y) * 16 + i32::from(local_y);
                                    let z = pos.z * 16 + i32::from(local_z);
                                    entry.push(format!("({x},{y},{z}) {:?}", state.states));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    println!(
        "audit: center=({center_x},{center_z}) radius_chunks={radius} loaded_chunks={loaded_chunks} distinct_blocks={}",
        names.len()
    );
    println!("--- frames ({}) ---", frames.len());
    for frame in &frames {
        println!("{frame}");
    }
    println!("--- top block names ---");
    let mut sorted = names.into_iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| right.1.cmp(&left.1));
    for (name, count) in sorted.iter().take(40) {
        println!("{count:>8}  {name}");
    }
    println!("--- interesting samples ---");
    for (name, entries) in &samples {
        println!("{name}:");
        for entry in entries {
            println!("    {entry}");
        }
    }
}

#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD pointing at a real Bedrock world"]
fn scan_real_world_item_frames() {
    let Some(world_path) = env_path("BMCBL_WORLD3D_TEST_WORLD") else {
        eprintln!("skip: BMCBL_WORLD3D_TEST_WORLD is not set");
        return;
    };
    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = World::open(&world_path, options).expect("open world");
    let (entities, report) = world
        .scan_block_entities(::bedrock_world::surface::WorldScanOptions {
            threading: WorldThreadingOptions::Single,
            ..Default::default()
        })
        .expect("scan block entities");
    let mut frames = Vec::new();
    for entity in &entities {
        let is_frame = entity
            .id
            .as_deref()
            .is_some_and(|id| id.to_ascii_lowercase().contains("frame"));
        if !is_frame {
            continue;
        }
        let nbt = format!("{:?}", entity.nbt);
        let nbt = nbt.chars().take(320).collect::<String>();
        frames.push(format!(
            "pos={:?} id={:?} nbt={nbt}",
            entity.position, entity.id
        ));
    }
    println!(
        "block-entity scan: {} entities, {} frames, {} parse errors",
        entities.len(),
        frames.len(),
        report.parse_errors.len()
    );
    for frame in frames.iter().take(40) {
        println!("{frame}");
    }
    // The block-entity scan is dimension agnostic; probe each frame position in every dimension
    // so render tests can load the right one.
    for entity in entities
        .iter()
        .filter(|entity| {
            entity
                .id
                .as_deref()
                .is_some_and(|id| id.to_ascii_lowercase().contains("frame"))
        })
        .take(12)
    {
        let Some(position) = entity.position else {
            continue;
        };
        let mut found = String::new();
        for (label, dimension) in [
            ("overworld", Dimension::Overworld),
            ("nether", Dimension::Nether),
            ("end", Dimension::End),
        ] {
            let chunk = ChunkPos {
                x: position[0].div_euclid(16),
                z: position[2].div_euclid(16),
                dimension,
            };
            if let Ok(data) = world.query_chunk_data(chunk, audit_chunk_load_options())
                && let Some(state) =
                    audit_block_state_at(&data, position[0], position[1], position[2])
            {
                found.push_str(&format!("{label}:{} ", state.name));
            }
        }
        println!("frame dimension probe pos={position:?} -> {found}");
    }
}

#[test]
#[ignore = "requires BMCBL_WORLD3D_TEST_WORLD pointing at a real Bedrock world"]
fn profile_chunk_timings() {
    let Some(world_path) = env_path("BMCBL_WORLD3D_TEST_WORLD") else {
        eprintln!("skip: BMCBL_WORLD3D_TEST_WORLD is not set");
        return;
    };
    let package_roots = env_path("BMCBL_WORLD3D_TEST_PACKAGE")
        .into_iter()
        .collect::<Vec<_>>();
    let spawn = read_world_spawn(&world_path);
    let (world, mut service) = collect_world_with_timeout(
        &world_path,
        package_roots,
        spawn,
        1,
        1,
        Duration::from_secs(90),
    );
    service.shutdown();
    eprintln!(
        "world3d profile: {} chunks, {} textures",
        world.meshes.len(),
        world.textures.len()
    );
}

#[test]
#[ignore = "requires a real Bedrock world; profiles bounded radius-32 streaming"]
fn profile_radius32_stream_budget() {
    let Some(world_path) = env_path("BMCBL_WORLD3D_TEST_WORLD") else {
        eprintln!("skip: BMCBL_WORLD3D_TEST_WORLD is not set");
        return;
    };
    let package_roots = env_path("BMCBL_WORLD3D_TEST_PACKAGE")
        .into_iter()
        .collect::<Vec<_>>();
    let spawn = read_world_spawn(&world_path);
    let center = [
        env_f32("BMCBL_WORLD3D_TEST_X").map_or(spawn[0], f64::from),
        env_f32("BMCBL_WORLD3D_TEST_Y").map_or(spawn[1], f64::from),
        env_f32("BMCBL_WORLD3D_TEST_Z").map_or(spawn[2], f64::from),
    ];
    let seconds = env_i32("BMCBL_WORLD3D_TEST_SECONDS")
        .unwrap_or(10)
        .clamp(1, 60) as u64;
    let started = Instant::now();
    let (world, mut service) = collect_world_with_timeout(
        &world_path,
        package_roots,
        center,
        32,
        usize::MAX,
        Duration::from_secs(seconds),
    );
    let streamed_sections = world.meshes.len();
    let completed_columns = world.completed_columns.len();
    let missing_columns = world.missing_columns;
    let faces = world.meshes.values().map(|mesh| mesh.faces).sum::<usize>();
    service.shutdown();
    eprintln!(
        "world3d radius32: completed_columns={completed_columns} missing_columns={missing_columns} sections={streamed_sections} faces={faces} elapsed_ms={}",
        started.elapsed().as_millis()
    );
}
