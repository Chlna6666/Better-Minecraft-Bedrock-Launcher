//! Large-world benchmark for gpui_3d: GPUI sidecar vs direct offscreen rendering.
//!
//! Builds one complex world (lighting, sky, an instanced city and an optional downloaded real glTF
//! model such as Sponza), then renders it through two selectable paths so performance and the API
//! call flow can be compared:
//!
//! ```text
//! # Direct (headless offscreen) — render/capture percentiles, no window needed
//! cargo run -p gpui_3d --example world_bench --features native -- \
//!   --mode=direct --backend=dx12 --frames=120 --instances=20000 --json=direct.json --trace
//!
//! # GPUI sidecar — real window, flying camera (WASD / mouse-drag / wheel), Present metrics
//! cargo run -p gpui_3d --example world_bench --features native -- \
//!   --mode=sidecar --backend=nova-dx12 --auto-exit-ms=8000 --trace
//! ```
//!
//! Download the primary model first with `pwsh -File fetch_model.ps1`. Without it the example
//! falls back to the in-repo DamagedHelmet fixture, then to a purely procedural world; use
//! `--source=studio` to skip glTF entirely.

#![allow(
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    reason = "The example entry point keeps argument parsing and mode dispatch in one reviewable flow."
)]

mod bench_direct;
mod bench_sidecar;
mod camera;
mod scene;
mod trace;

use gpui_3d::RenderQuality;
use scene::{WorldOptions, WorldSource};
use std::process::ExitCode;

/// Rendering path to benchmark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Sidecar,
    Direct,
}

/// Fully parsed configuration shared by both paths.
pub(crate) struct Config {
    mode: Mode,
    backend: String,
    width: u32,
    height: u32,
    frames: u32,
    yaw: f32,
    pitch: f32,
    orbit_degrees: f32,
    shadows: bool,
    shadow_resolution: Option<u32>,
    screen_effects: bool,
    exposure: f32,
    render_scale: f32,
    quality: Option<RenderQuality>,
    out: Option<String>,
    json: Option<String>,
    auto_exit_ms: Option<u64>,
    metrics_interval_ms: u64,
    fps_cap: Option<u32>,
    world: WorldOptions,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            mode: Mode::Sidecar,
            backend: "nova-dx12".to_string(),
            width: 1280,
            height: 720,
            frames: 120,
            yaw: 0.62,
            pitch: 0.32,
            orbit_degrees: 3.0,
            shadows: true,
            shadow_resolution: None,
            screen_effects: false,
            exposure: 1.0,
            render_scale: 1.0,
            quality: None,
            out: None,
            json: None,
            auto_exit_ms: None,
            metrics_interval_ms: 250,
            fps_cap: None,
            world: WorldOptions::default(),
        }
    }
}

fn main() -> ExitCode {
    let config = match parse_args() {
        Ok(config) => config,
        Err(message) => {
            if message == "help" {
                print_help();
                return ExitCode::SUCCESS;
            }
            eprintln!("world_bench: {message}");
            return ExitCode::FAILURE;
        }
    };
    trace::set_enabled(std::env::args().any(|arg| arg == "--trace"));

    match config.mode {
        Mode::Direct => match bench_direct::run(&config) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("world_bench: direct path failed: {error}");
                ExitCode::FAILURE
            }
        },
        Mode::Sidecar => match bench_sidecar::run(&config) {
            Ok(code) => code,
            Err(error) => {
                eprintln!("world_bench: sidecar path failed: {error}");
                ExitCode::FAILURE
            }
        },
    }
}

fn parse_args() -> Result<Config, String> {
    let mut config = Config::default();
    // A direct run without an explicit backend defaults to the headless DX12 device.
    let explicit_backend = std::env::args().any(|arg| arg.starts_with("--backend="));
    let is_direct = std::env::args().any(|arg| arg == "--mode=direct");
    if is_direct && !explicit_backend {
        config.backend = "dx12".to_string();
    }
    for argument in std::env::args().skip(1) {
        let (key, value) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(key, value)| (key, Some(value)));
        match key {
            "--mode" => {
                config.mode = match required(value, key)?.as_str() {
                    "sidecar" => Mode::Sidecar,
                    "direct" => Mode::Direct,
                    other => return Err(format!("unknown mode '{other}' (sidecar|direct)")),
                };
            }
            "--backend" => config.backend = required(value, key)?,
            "--width" => config.width = parse_number(value, key)?,
            "--height" => config.height = parse_number(value, key)?,
            "--frames" => config.frames = parse_number(value, key)?,
            "--yaw" => config.yaw = parse_number(value, key)?,
            "--pitch" => config.pitch = parse_number(value, key)?,
            "--orbit-degrees" => config.orbit_degrees = parse_number(value, key)?,
            "--shadow" => config.shadows = matches!(value, None | Some("on" | "true" | "1")),
            "--shadow-res" => config.shadow_resolution = Some(parse_number(value, key)?),
            "--screen-effects" => config.screen_effects = true,
            "--exposure" => config.exposure = parse_number(value, key)?,
            "--render-scale" => config.render_scale = parse_number(value, key)?,
            "--quality" => {
                config.quality = Some(match required(value, key)?.as_str() {
                    "low" => RenderQuality::Low,
                    "medium" => RenderQuality::Medium,
                    "high" => RenderQuality::High,
                    "ultra" => RenderQuality::Ultra,
                    other => return Err(format!("unknown quality '{other}'")),
                });
            }
            "--out" => config.out = Some(required(value, key)?),
            "--json" => config.json = Some(required(value, key)?),
            "--auto-exit-ms" => config.auto_exit_ms = Some(parse_number(value, key)?),
            "--metrics-interval-ms" => {
                config.metrics_interval_ms = parse_number(value, key)?;
            }
            "--fps-cap" => config.fps_cap = Some(parse_number(value, key)?),
            "--model" => config.world.model_path = required(value, key)?,
            "--fallback-model" => config.world.fallback_model_path = required(value, key)?,
            "--instances" => config.world.instances = parse_number(value, key)?,
            "--sun" => {
                let text = required(value, key)?;
                let (azimuth, elevation) = text
                    .split_once(',')
                    .ok_or_else(|| "--sun expects azimuth,elevation".to_string())?;
                config.world.sun = (
                    azimuth.parse().map_err(|_| "invalid sun azimuth")?,
                    elevation.parse().map_err(|_| "invalid sun elevation")?,
                );
            }
            "--source" => {
                config.world.source = match required(value, key)?.as_str() {
                    "model" => WorldSource::Model,
                    "studio" => WorldSource::Studio,
                    other => return Err(format!("unknown source '{other}' (model|studio)")),
                };
            }
            "--trace" => {}
            "--help" | "-h" => return Err("help".to_string()),
            other => return Err(format!("unknown argument '{other}' (try --help)")),
        }
    }
    Ok(config)
}

fn required(value: Option<&str>, key: &str) -> Result<String, String> {
    value
        .map(str::to_string)
        .ok_or_else(|| format!("{key} requires a value"))
}

fn parse_number<T: std::str::FromStr>(value: Option<&str>, key: &str) -> Result<T, String> {
    required(value, key)?
        .parse()
        .map_err(|_| format!("{key} requires a number"))
}

fn print_help() {
    println!(
        "{}",
        [
            "usage: world_bench [options]",
            "  --mode=sidecar|direct            rendering path (default: sidecar)",
            "  --backend=nova-dx12|nova-vulkan|nova-dx11|nova-opengl   (sidecar renderer)",
            "  --backend=dx12|dx11|vulkan|opengl                       (direct renderer)",
            "  --width=1280 --height=720 --frames=120",
            "  --source=model|studio            downloaded glTF vs procedural world",
            "  --model=path --fallback-model=path --instances=20000 --sun=135,52",
            "  --yaw=0.62 --pitch=0.32 --orbit-degrees=3.0 (camera; orbit>0 avoids frame reuse)",
            "  --shadow=on|off --shadow-res=2048 --exposure=1.0 --render-scale=1.0",
            "  --screen-effects            sidecar analytic sky + material shadows (needs compute+indirect; opt-in)",
            "  --quality=low|medium|high|ultra",
            "  --out=capture.png --json=report.json",
            "  --auto-exit-ms=8000 --metrics-interval-ms=250 (sidecar auto-exit + sampling)",
            "  --fps-cap=120             cap the sidecar window frame clock (omit = uncapped)",
            "  --trace                          print API call trace (also set GPUI_3D_TRACE_TIMINGS=1)",
            "",
            "Sidecar controls: W/A/S/D move, Q/E down/up, Shift = faster, mouse-drag = look, wheel = dolly.",
            "Run fetch_model.ps1 first to download the primary Sponza model; otherwise a fallback is used.",
        ]
        .join("\n")
    );
}
