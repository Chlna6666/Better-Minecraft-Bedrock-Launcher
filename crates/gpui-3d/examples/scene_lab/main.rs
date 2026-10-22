//! Scripted offscreen 3D scene lab with deterministic captures.
//!
//! The lab builds a procedural studio scene, renders it through the multi-pass offscreen
//! renderer (directional shadow map, PBR scene pass, post pass), and writes a PNG plus optional
//! JSON statistics. It is intentionally scriptable: no window or manual interaction is required,
//! so automated captures and GPU pixel checks can run through the same public API.
//!
//! ```text
//! cargo run -p gpui-3d --example scene_lab -- --backend=dx12 --out=scene-lab.png
//! cargo run -p gpui-3d --example scene_lab -- --backend=all --out=scene-lab.png
//! cargo run -p gpui-3d --example scene_lab -- --backend=vulkan --debug=shadow --shadow=on
//! ```

#![allow(
    clippy::cast_precision_loss,
    clippy::manual_midpoint,
    clippy::map_unwrap_or,
    clippy::too_many_lines,
    clippy::unnecessary_lazy_evaluations,
    reason = "The lab entry point keeps argument parsing, scene selection, and capture scheduling in one reviewable flow; each stage is a small helper."
)]

mod backend;
mod studio;

use gpui_3d::{DebugView, GltfLoadOptions, RenderOptions, RenderQuality, Tonemap};
use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

const DEFAULT_WIDTH: u32 = 1280;
const DEFAULT_HEIGHT: u32 = 720;
const DEFAULT_FRAMES: u32 = 3;

struct Args {
    backend: String,
    width: u32,
    height: u32,
    out: Option<String>,
    json: Option<String>,
    frames: u32,
    debug: DebugView,
    shadows: bool,
    shadow_resolution: Option<u32>,
    exposure: f32,
    tonemap: Tonemap,
    post_process: bool,
    sun: (f32, f32),
    yaw: f32,
    pitch: f32,
    distance: Option<f32>,
    model: Option<String>,
    instances: Option<u32>,
    animation: Option<String>,
    animation_time: f32,
    max_texture: Option<u32>,
    budget_mb: Option<u64>,
    pick: Option<(f32, f32)>,
    quality: Option<RenderQuality>,
    orbit_degrees: f32,
    render_scale: f32,
    environment: Option<String>,
    bloom: Option<bool>,
    fxaa: Option<bool>,
    list: bool,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            backend: "dx12".to_string(),
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            out: None,
            json: None,
            frames: DEFAULT_FRAMES,
            debug: DebugView::Shaded,
            shadows: true,
            shadow_resolution: None,
            exposure: 1.0,
            tonemap: Tonemap::Aces,
            post_process: true,
            sun: (135.0, 52.0),
            yaw: 0.62,
            pitch: 0.30,
            distance: None,
            model: None,
            instances: None,
            animation: None,
            animation_time: 0.0,
            max_texture: None,
            budget_mb: None,
            pick: None,
            quality: None,
            orbit_degrees: 0.0,
            render_scale: 1.0,
            environment: None,
            bloom: None,
            fxaa: None,
            list: false,
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("scene_lab: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    if args.list {
        println!(
            "available backends: {}",
            backend::available_backends().join(", ")
        );
        return Ok(());
    }
    let studio_options = studio::StudioOptions {
        yaw: args.yaw,
        pitch: args.pitch,
        distance: args.distance.unwrap_or(11.5),
        sun_azimuth_degrees: args.sun.0,
        sun_elevation_degrees: args.sun.1,
    };
    let mut field_scene = match args.instances {
        Some(count) if args.model.is_none() => Some(
            studio::build_instance_field(count)
                .map_err(|error| format!("build instance field: {error}"))?,
        ),
        _ => None,
    };
    let field_textures = HashMap::new();
    let mut studio_scene = if args.model.is_none() && field_scene.is_none() {
        Some(
            studio::build(&studio_options)
                .map_err(|error| format!("build studio scene: {error}"))?,
        )
    } else {
        None
    };
    let mut load_options = GltfLoadOptions::default();
    if let Some(max_texture) = args.max_texture {
        load_options.max_texture_dimension = max_texture.max(1);
    }
    let mut imported = match &args.model {
        Some(path) => Some(
            gpui_3d::load_path(path, &load_options)
                .map_err(|error| format!("import {path}: {error}"))?,
        ),
        None => None,
    };
    if let Some(imported) = imported.as_mut() {
        add_studio_lights(&mut imported.scene, &args)?;
    }
    if let Some(imported) = &imported {
        let statistics = imported.statistics;
        println!(
            "imported: nodes={} meshes={} primitives={} materials={} textures={} triangles={} animations={}",
            statistics.nodes,
            statistics.meshes,
            statistics.primitives,
            statistics.materials,
            statistics.textures,
            statistics.triangles,
            statistics.animations
        );
        for warning in &imported.warnings {
            println!("import warning: {warning}");
        }
    }
    let environment = match &args.environment {
        Some(name) if name == "studio" => Some(std::sync::Arc::new(
            gpui_3d::EnvironmentMap::procedural_studio(),
        )),
        Some(path) => Some(std::sync::Arc::new(
            gpui_3d::EnvironmentMap::load_path(path)
                .map_err(|error| format!("load environment {path}: {error}"))?,
        )),
        None => None,
    };
    if let Some(environment) = &environment {
        if let Some(studio) = studio_scene.as_mut() {
            studio.scene.set_environment(Some(environment.clone()));
        }
        if let Some(imported) = imported.as_mut() {
            imported.scene.set_environment(Some(environment.clone()));
        }
        if let Some((field, _)) = field_scene.as_mut() {
            field.set_environment(Some(environment.clone()));
        }
        println!(
            "environment: {} texture={}x{} mips={} intensity={:.2}",
            if args.environment.as_deref() == Some("studio") {
                "procedural studio"
            } else {
                "equirectangular image"
            },
            environment.texture().width(),
            environment.texture().height(),
            environment.texture().mip_level_count(),
            environment.intensity()
        );
    }
    let camera = if let Some((field, radius)) = &field_scene {
        field_camera(field, *radius, &args)
    } else {
        match (&studio_scene, &imported) {
            (Some(_), _) => studio::studio_camera(&studio_options),
            (_, Some(imported)) => imported
                .cameras
                .first()
                .map(|camera| camera.camera)
                .unwrap_or_else(|| imported_camera(imported, &args)),
            _ => studio::studio_camera(&studio_options),
        }
    };
    if let Some((field, radius)) = &field_scene {
        println!("instance field: nodes={} radius={radius:.3}", field.len());
    }
    let animation = imported.as_ref().and_then(|imported| {
        let clip = match &args.animation {
            Some(name) => imported.animation(name),
            None => imported.animations.first(),
        };
        clip.map(|clip| {
            (
                clip.tracks.as_slice(),
                Duration::from_secs_f32(args.animation_time.max(0.0)),
                clip.name.clone(),
                clip.duration,
            )
        })
    });
    if let Some((_, time, name, duration)) = &animation {
        println!(
            "animation clip {name} sampled at {:.3}s of {:.3}s",
            time.as_secs_f32(),
            duration.as_secs_f32()
        );
    }
    let mut options = args
        .quality
        .map_or_else(RenderOptions::default, RenderQuality::render_options);
    options.shadows.enabled = args.shadows;
    if let Some(resolution) = args.shadow_resolution {
        options.shadows.resolution = resolution;
    }
    options.exposure = args.exposure;
    options.tonemap = args.tonemap;
    options.debug_view = args.debug;
    options.post_process = args.post_process;
    if let Some(enabled) = args.bloom {
        options.bloom.enabled = enabled;
    }
    if let Some(enabled) = args.fxaa {
        options.antialias = if enabled {
            gpui_3d::AntialiasMode::Fxaa
        } else {
            gpui_3d::AntialiasMode::None
        };
    }
    if let Some(quality) = args.quality {
        println!("quality preset: {quality:?}");
    }
    let backends = if args.backend == "all" {
        backend::available_backends()
            .iter()
            .map(|name| (*name).to_string())
            .collect::<Vec<_>>()
    } else {
        vec![args.backend.clone()]
    };
    let mut failures = Vec::new();
    for backend_name in backends {
        let (scene, textures) = if let Some((field, _)) = &field_scene {
            (field, &field_textures)
        } else {
            match (&studio_scene, &imported) {
                (Some(studio), _) => (&studio.scene, &studio.textures),
                (_, Some(imported)) => (&imported.scene, &imported.textures),
                _ => unreachable!("one scene source is always selected"),
            }
        };
        let request = backend::RenderRequest {
            scene,
            textures,
            camera,
            options: &options,
            width: args.width,
            height: args.height,
            frames: args.frames,
            memory_budget: args.budget_mb.map(|mb| mb.saturating_mul(1024 * 1024)),
            pick: args.pick,
            orbit_degrees: args.orbit_degrees,
            render_scale: args.render_scale,
            animation: animation
                .as_ref()
                .map(|(tracks, time, _, _)| (*tracks, *time)),
        };
        match backend::render(&backend_name, &request) {
            Ok(report) => {
                if let Some(path) = &args.out {
                    let path = if args.backend == "all" {
                        backend_artifact_path(path, &backend_name)
                    } else {
                        path.clone()
                    };
                    save_png(&path, &report.image)?;
                    println!("wrote {path}");
                }
                if let Some(path) = &args.json {
                    let path = if args.backend == "all" {
                        backend_artifact_path(path, &backend_name)
                    } else {
                        path.clone()
                    };
                    let json = report_json(&report, &options);
                    std::fs::write(&path, json)
                        .map_err(|error| format!("write {path}: {error}"))?;
                    println!("wrote {path}");
                }
                print_report(&report);
            }
            Err(error) => {
                eprintln!("backend {backend_name} failed: {error}");
                failures.push(backend_name);
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!("backends failed: {}", failures.join(", ")))
    }
}

fn add_studio_lights(scene: &mut gpui_3d::Scene, args: &Args) -> Result<(), String> {
    use gpui_3d::{Light, Node, Vec3};
    let azimuth = args.sun.0.to_radians();
    let elevation = args.sun.1.to_radians();
    let direction = Vec3::new(
        -(elevation.cos() * azimuth.sin()),
        -elevation.sin(),
        -(elevation.cos() * azimuth.cos()),
    );
    scene
        .insert(
            None,
            Node::new().with_light(Light::Directional {
                direction,
                color: [1.0, 0.93, 0.82],
                intensity: 5.0,
            }),
        )
        .map_err(|error| format!("insert studio key light: {error}"))?;
    scene
        .insert(
            None,
            Node::new().with_light(Light::Ambient {
                color: [0.44, 0.53, 0.72],
                intensity: 0.22,
            }),
        )
        .map_err(|error| format!("insert studio ambient light: {error}"))?;
    scene
        .insert(
            None,
            Node::new().with_light(Light::Point {
                position: Vec3::new(2.6, 2.4, 2.2),
                color: [1.0, 0.62, 0.3],
                intensity: 24.0,
                range: 14.0,
            }),
        )
        .map_err(|error| format!("insert studio fill light: {error}"))?;
    scene
        .insert(
            None,
            Node::new().with_light(Light::Directional {
                direction: Vec3::new(0.35, 0.1, 0.9),
                color: [0.55, 0.68, 0.95],
                intensity: 1.6,
            }),
        )
        .map_err(|error| format!("insert studio rim light: {error}"))?;
    Ok(())
}

fn imported_camera(imported: &gpui_3d::GltfScene, args: &Args) -> gpui_3d::Camera {
    let (center, radius) = scene_bounds(&imported.scene);
    framed_camera(center, radius, args)
}

fn field_camera(scene: &gpui_3d::Scene, radius: f32, args: &Args) -> gpui_3d::Camera {
    let (center, _) = scene_bounds(scene);
    framed_camera(center, radius.max(0.05), args)
}

fn scene_bounds(scene: &gpui_3d::Scene) -> (gpui_3d::Vec3, f32) {
    use gpui_3d::Vec3;
    let Some(bounds) = scene.bounds().ok().flatten() else {
        return (Vec3::ZERO, 1.0);
    };
    let center = Vec3::new(
        (bounds.min.x + bounds.max.x) * 0.5,
        (bounds.min.y + bounds.max.y) * 0.5,
        (bounds.min.z + bounds.max.z) * 0.5,
    );
    let extent = Vec3::new(
        bounds.max.x - bounds.min.x,
        bounds.max.y - bounds.min.y,
        bounds.max.z - bounds.min.z,
    );
    (
        center,
        (extent.x * extent.x + extent.y * extent.y + extent.z * extent.z).sqrt() * 0.5,
    )
}

fn framed_camera(center: gpui_3d::Vec3, radius: f32, args: &Args) -> gpui_3d::Camera {
    use gpui_3d::{Camera, Vec3};
    let radius = radius.max(0.05);
    let distance = args.distance.unwrap_or_else(|| radius * 2.1);
    let horizontal = distance * args.pitch.cos();
    let target = Vec3::new(center.x, center.y - radius * 0.1, center.z);
    let eye = Vec3::new(
        target.x + horizontal * args.yaw.sin(),
        target.y + distance * args.pitch.sin(),
        target.z + horizontal * args.yaw.cos(),
    );
    Camera::perspective(
        eye,
        target,
        Vec3::Y,
        42.0_f32.to_radians(),
        (radius * 0.02).max(0.01),
        distance * 8.0 + radius * 4.0,
    )
}

fn backend_artifact_path(path: &str, backend: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, extension)) => format!("{stem}-{backend}.{extension}"),
        None => format!("{path}-{backend}"),
    }
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    for argument in std::env::args().skip(1) {
        let (key, value) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(key, value)| (key, Some(value)));
        match key {
            "--backend" => args.backend = required(value, key)?,
            "--width" => args.width = parse_number(value, key)?,
            "--height" => args.height = parse_number(value, key)?,
            "--out" => args.out = Some(required(value, key)?),
            "--json" => args.json = Some(required(value, key)?),
            "--frames" => args.frames = parse_number(value, key)?,
            "--shadow" => args.shadows = parse_switch(value, key)?,
            "--shadow-res" => args.shadow_resolution = Some(parse_number(value, key)?),
            "--exposure" => args.exposure = parse_float(value, key)?,
            "--post" => args.post_process = parse_switch(value, key)?,
            "--yaw" => args.yaw = parse_float(value, key)?,
            "--pitch" => args.pitch = parse_float(value, key)?,
            "--distance" => args.distance = Some(parse_float(value, key)?),
            "--model" => args.model = Some(required(value, key)?),
            "--instances" => args.instances = Some(parse_number(value, key)?),
            "--bloom" => args.bloom = Some(parse_on_off(value, key)?),
            "--fxaa" => args.fxaa = Some(parse_on_off(value, key)?),
            "--orbit-degrees" => args.orbit_degrees = parse_float(value, key)?,
            "--render-scale" => args.render_scale = parse_float(value, key)?,
            "--env-map" => args.environment = Some(required(value, key)?),
            "--quality" => {
                args.quality = Some(match required(value, key)?.as_str() {
                    "low" => RenderQuality::Low,
                    "medium" => RenderQuality::Medium,
                    "high" => RenderQuality::High,
                    "ultra" => RenderQuality::Ultra,
                    other => {
                        return Err(format!("unknown quality '{other}' (low|medium|high|ultra)"));
                    }
                });
            }
            "--animation" => args.animation = Some(required(value, key)?),
            "--animation-time" => args.animation_time = parse_float(value, key)?,
            "--max-texture" => args.max_texture = Some(parse_number(value, key)?),
            "--budget-mb" => args.budget_mb = Some(parse_number(value, key)?),
            "--pick" => {
                let text = required(value, key)?;
                let (x, y) = text
                    .split_once(',')
                    .ok_or_else(|| "--pick expects x,y".to_string())?;
                args.pick = Some((
                    x.parse().map_err(|_| "invalid pick x")?,
                    y.parse().map_err(|_| "invalid pick y")?,
                ));
            }
            "--sun" => {
                let text = required(value, key)?;
                let (azimuth, elevation) = text
                    .split_once(',')
                    .ok_or_else(|| "--sun expects azimuth,elevation".to_string())?;
                args.sun = (
                    azimuth.parse().map_err(|_| "invalid sun azimuth")?,
                    elevation.parse().map_err(|_| "invalid sun elevation")?,
                );
            }
            "--tonemap" => {
                args.tonemap = match required(value, key)?.as_str() {
                    "none" => Tonemap::None,
                    "reinhard" => Tonemap::Reinhard,
                    "aces" => Tonemap::Aces,
                    other => return Err(format!("unknown tonemap '{other}'")),
                };
            }
            "--debug" => {
                args.debug = match required(value, key)?.as_str() {
                    "shaded" => DebugView::Shaded,
                    "albedo" => DebugView::Albedo,
                    "normal" => DebugView::Normal,
                    "depth" => DebugView::Depth,
                    "uv" => DebugView::Uv,
                    "shadow" => DebugView::Shadow,
                    "object-id" => DebugView::ObjectId,
                    other => return Err(format!("unknown debug view '{other}'")),
                };
            }
            "--list-backends" => args.list = true,
            "--help" | "-h" => return Err(help_text()),
            other => return Err(format!("unknown argument '{other}'\n{}", help_text())),
        }
    }
    Ok(args)
}

fn required(value: Option<&str>, key: &str) -> Result<String, String> {
    value
        .map(str::to_string)
        .ok_or_else(|| format!("{key} requires a value"))
}

fn parse_on_off(value: Option<&str>, key: &str) -> Result<bool, String> {
    match required(value, key)?.as_str() {
        "on" | "true" | "1" => Ok(true),
        "off" | "false" | "0" => Ok(false),
        other => Err(format!("{key} expects on or off, got '{other}'")),
    }
}

fn parse_number<T: std::str::FromStr>(value: Option<&str>, key: &str) -> Result<T, String> {
    required(value, key)?
        .parse()
        .map_err(|_| format!("{key} requires a number"))
}

fn parse_float(value: Option<&str>, key: &str) -> Result<f32, String> {
    parse_number(value, key)
}

fn parse_switch(value: Option<&str>, key: &str) -> Result<bool, String> {
    match value {
        None | Some("on" | "true") => Ok(true),
        Some("off" | "false") => Ok(false),
        Some(other) => Err(format!(
            "{key} expects on, off, true, or false (got '{other}')"
        )),
    }
}

fn help_text() -> String {
    [
        "usage: scene_lab [options]",
        "  --backend=dx12|dx11|vulkan|opengl|all",
        "  --width=1280 --height=720 --frames=3",
        "  --out=capture.png --json=report.json",
        "  --shadow=on|off --shadow-res=2048 --exposure=1.0 --post=on|off",
        "  --tonemap=none|reinhard|aces",
        "  --debug=shaded|albedo|normal|depth|uv|shadow|object-id",
        "  --yaw=0.62 --pitch=0.30 --distance=11.5 --sun=135,52",
        "  --model=model.glb --animation=clip --animation-time=1.5",
        "  --instances=20000  (procedural instanced cube field)",
        "  --quality=high|ultra  (shadow map, bloom, FXAA, grading preset)",
        "  --bloom=on|off --fxaa=on|off --orbit-degrees=3.0 --render-scale=0.75|2.0",
        "  --env-map=studio|path/to/equirect.png  (image-based lighting + sky background)",
        "  --max-texture=1024 --budget-mb=256 --pick=640,360",
        "  --list-backends",
    ]
    .join("\n")
}

fn save_png(path: &str, image: &gpui_3d::CapturedImage) -> Result<(), String> {
    image::save_buffer_with_format(
        path,
        image.rgba(),
        image.width(),
        image.height(),
        image::ExtendedColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|error| format!("write PNG {path}: {error}"))
}

fn print_report(report: &backend::BackendReport) {
    let frames = report.frames.max(1);
    println!(
        "backend={} frames={} size={}x{} render_avg={:.2}ms capture_avg={:.2}ms",
        report.backend,
        report.frames,
        report.image.width(),
        report.image.height(),
        report.render_nanos as f64 / f64::from(frames) / 1.0e6,
        report.capture_nanos as f64 / f64::from(frames) / 1.0e6,
    );
    let first = report.first_frame;
    let steady = report.steady_frame;
    println!(
        "first_frame: draws={} shadow_draws={} triangles={} instances={} mesh_uploads={} texture_uploads={} upload_bytes={} prepare={:.2}ms submit={:.2}ms",
        first.draw_calls,
        first.shadow_draw_calls,
        first.triangles,
        first.instances,
        first.mesh_uploads,
        first.texture_uploads,
        first.upload_bytes,
        first.cpu_prepare_nanos as f64 / 1.0e6,
        first.submit_nanos as f64 / 1.0e6,
    );
    println!(
        "steady_frame: draws={} shadow_draws={} triangles={} instances={} mesh_uploads={} texture_uploads={} upload_bytes={} prepare={:.2}ms submit={:.2}ms reused={} camera_only={}",
        steady.draw_calls,
        steady.shadow_draw_calls,
        steady.triangles,
        steady.instances,
        steady.mesh_uploads,
        steady.texture_uploads,
        steady.upload_bytes,
        steady.cpu_prepare_nanos as f64 / 1.0e6,
        steady.submit_nanos as f64 / 1.0e6,
        steady.frame_reused,
        steady.camera_only_update,
    );
    let renderer = report.renderer;
    println!(
        "renderer: live_meshes={} live_textures={} bind_groups={} buffers={}KB targets={}KB cached_mesh={}KB cached_texture={}KB draw_capacity={} instance_capacity={} shadow={} render_target={}x{}",
        renderer.live_meshes,
        renderer.live_textures,
        renderer.bind_group_count,
        renderer.resident_buffer_bytes / 1024,
        renderer.target_bytes / 1024,
        renderer.cached_mesh_bytes / 1024,
        renderer.cached_texture_bytes / 1024,
        renderer.draw_capacity,
        renderer.instance_capacity,
        renderer.shadow_resolution,
        renderer.render_target_width,
        renderer.render_target_height,
    );
    println!(
        "buffers: allocations={} slack={}KB staging={}KB pooled={}KB({}) reuses={}",
        renderer.buffer_allocation_count,
        renderer.buffer_slack_bytes / 1024,
        renderer.cpu_staging_bytes / 1024,
        renderer.pooled_buffer_bytes / 1024,
        renderer.pooled_buffer_count,
        renderer.buffer_reuses,
    );
    println!(
        "memory: resident={}KB peak={}KB budget={} upload_total={}KB cache_hits(mesh={},texture={}) misses(mesh={},texture={}) evictions={} tracker_cpu={}KB",
        (renderer.resident_buffer_bytes + renderer.resident_texture_bytes) / 1024,
        renderer.peak_resident_bytes / 1024,
        renderer
            .memory_budget_bytes
            .map_or_else(|| "none".to_string(), |bytes| format!("{}KB", bytes / 1024)),
        renderer.upload_bytes_total / 1024,
        renderer.mesh_cache_hits,
        renderer.texture_cache_hits,
        renderer.mesh_cache_misses,
        renderer.texture_cache_misses,
        renderer.evictions,
        renderer.tracker_cpu_bytes / 1024,
    );
    if report.frames > 1 {
        let mut render_samples = report
            .samples
            .iter()
            .map(|sample| sample.render_nanos)
            .collect::<Vec<_>>();
        let mut capture_samples = report
            .samples
            .iter()
            .map(|sample| sample.capture_nanos)
            .collect::<Vec<_>>();
        render_samples.sort_unstable();
        capture_samples.sort_unstable();
        println!(
            "frame_times: render p50={:.2}ms p95={:.2}ms p99={:.2}ms max={:.2}ms | capture p50={:.2}ms p95={:.2}ms p99={:.2}ms",
            percentile(&render_samples, 50) as f64 / 1.0e6,
            percentile(&render_samples, 95) as f64 / 1.0e6,
            percentile(&render_samples, 99) as f64 / 1.0e6,
            render_samples.last().copied().unwrap_or(0) as f64 / 1.0e6,
            percentile(&capture_samples, 50) as f64 / 1.0e6,
            percentile(&capture_samples, 95) as f64 / 1.0e6,
            percentile(&capture_samples, 99) as f64 / 1.0e6,
        );
    }
    if let Some(pick) = &report.cpu_pick {
        println!("cpu_pick: {pick}");
    } else if report.cpu_pick.is_none() && report.frames > 0 {
        println!("cpu_pick: none");
    }
    if let Some(pick) = &report.object_pick {
        println!("object_pick: {pick}");
    }
}

fn percentile(sorted: &[u64], percent: usize) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) * percent) / 100;
    sorted[index]
}

fn report_json(report: &backend::BackendReport, options: &RenderOptions) -> String {
    let frame = report.steady_frame;
    let first = report.first_frame;
    let renderer = report.renderer;
    format!(
        concat!(
            "{{\n",
            "  \"backend\": \"{}\",\n",
            "  \"width\": {},\n",
            "  \"height\": {},\n",
            "  \"frames\": {},\n",
            "  \"render_nanos_total\": {},\n",
            "  \"capture_nanos_total\": {},\n",
            "  \"shadows\": {},\n",
            "  \"shadow_resolution\": {},\n",
            "  \"exposure\": {},\n",
            "  \"debug_view\": \"{:?}\",\n",
            "  \"first_frame\": {{\"draw_calls\":{},\"shadow_draw_calls\":{},\"triangles\":{},\"instances\":{},\"mesh_uploads\":{},\"texture_uploads\":{},\"upload_bytes\":{},\"prepare_nanos\":{},\"submit_nanos\":{}}},\n",
            "  \"steady_frame\": {{\"draw_calls\":{},\"shadow_draw_calls\":{},\"triangles\":{},\"instances\":{},\"mesh_uploads\":{},\"texture_uploads\":{},\"upload_bytes\":{},\"prepare_nanos\":{},\"submit_nanos\":{}}},\n",
            "  \"renderer\": {{\"live_meshes\":{},\"live_textures\":{},\"bind_groups\":{},\"resident_buffer_bytes\":{},\"resident_texture_bytes\":{},\"draw_capacity\":{},\"instance_capacity\":{},\"light_capacity\":{},\"render_scale\":{:.3},\"render_target_width\":{},\"render_target_height\":{}}},\n",
            "  \"memory\": {{\"target_bytes\":{},\"cached_mesh_bytes\":{},\"cached_texture_bytes\":{},\"resident_bytes\":{},\"peak_resident_bytes\":{},\"upload_bytes_total\":{},\"mesh_cache_hits\":{},\"mesh_cache_misses\":{},\"texture_cache_hits\":{},\"texture_cache_misses\":{},\"evictions\":{},\"tracker_cpu_bytes\":{},\"budget_bytes\":{},\"cpu_staging_bytes\":{},\"buffer_allocations\":{},\"buffer_slack_bytes\":{},\"pooled_bytes\":{},\"pooled_buffers\":{},\"buffer_reuses\":{}}},\n",
            "  \"frame_times_nanos\": {{\"render_p50\":{},\"render_p95\":{},\"render_p99\":{},\"capture_p50\":{},\"capture_p95\":{},\"capture_p99\":{}}},\n",
            "  \"cpu_pick\": {},\n",
            "  \"object_pick\": {},\n",
            "  \"debug_view\": \"{:?}\"\n",
            "}}\n",
        ),
        report.backend,
        report.image.width(),
        report.image.height(),
        report.frames,
        report.render_nanos,
        report.capture_nanos,
        options.shadows.enabled,
        options.shadows.resolution,
        options.exposure,
        options.debug_view,
        first.draw_calls,
        first.shadow_draw_calls,
        first.triangles,
        first.instances,
        first.mesh_uploads,
        first.texture_uploads,
        first.upload_bytes,
        first.cpu_prepare_nanos,
        first.submit_nanos,
        frame.draw_calls,
        frame.shadow_draw_calls,
        frame.triangles,
        frame.instances,
        frame.mesh_uploads,
        frame.texture_uploads,
        frame.upload_bytes,
        frame.cpu_prepare_nanos,
        frame.submit_nanos,
        renderer.live_meshes,
        renderer.live_textures,
        renderer.bind_group_count,
        renderer.resident_buffer_bytes,
        renderer.resident_texture_bytes,
        renderer.draw_capacity,
        renderer.instance_capacity,
        renderer.light_capacity,
        f64::from(renderer.render_target_width) / f64::from(renderer.target_width.max(1)),
        renderer.render_target_width,
        renderer.render_target_height,
        renderer.target_bytes,
        renderer.cached_mesh_bytes,
        renderer.cached_texture_bytes,
        renderer.resident_buffer_bytes + renderer.resident_texture_bytes,
        renderer.peak_resident_bytes,
        renderer.upload_bytes_total,
        renderer.mesh_cache_hits,
        renderer.mesh_cache_misses,
        renderer.texture_cache_hits,
        renderer.texture_cache_misses,
        renderer.evictions,
        renderer.tracker_cpu_bytes,
        renderer
            .memory_budget_bytes
            .map_or_else(|| "null".to_string(), |bytes| bytes.to_string()),
        renderer.cpu_staging_bytes,
        renderer.buffer_allocation_count,
        renderer.buffer_slack_bytes,
        renderer.pooled_buffer_bytes,
        renderer.pooled_buffer_count,
        renderer.buffer_reuses,
        percentile_of(report, PercentileKind::Render, 50),
        percentile_of(report, PercentileKind::Render, 95),
        percentile_of(report, PercentileKind::Render, 99),
        percentile_of(report, PercentileKind::Capture, 50),
        percentile_of(report, PercentileKind::Capture, 95),
        percentile_of(report, PercentileKind::Capture, 99),
        json_string(report.cpu_pick.as_deref()),
        json_string(report.object_pick.as_deref()),
        options.debug_view,
    )
}

#[derive(Clone, Copy)]
enum PercentileKind {
    Render,
    Capture,
}

fn percentile_of(report: &backend::BackendReport, kind: PercentileKind, percent: usize) -> u64 {
    let mut samples = report
        .samples
        .iter()
        .map(|sample| match kind {
            PercentileKind::Render => sample.render_nanos,
            PercentileKind::Capture => sample.capture_nanos,
        })
        .collect::<Vec<_>>();
    samples.sort_unstable();
    percentile(&samples, percent)
}

fn json_string(value: Option<&str>) -> String {
    match value {
        Some(value) => format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")),
        None => "null".to_string(),
    }
}
