#![allow(clippy::disallowed_methods, reason = "build scripts are exempt")]
#![cfg_attr(any(not(target_os = "macos"), feature = "macos-blade"), allow(unused))]

// TODO: consider generating shader code for WGSL.
// TODO: deprecate "runtime-shaders" and "macos-blade".

use std::env;

fn main() {
    let target = env::var("CARGO_CFG_TARGET_OS");
    println!("cargo::rustc-check-cfg=cfg(gles)");

    #[cfg(feature = "build-shader-validation")]
    check_nova_wgsl_shaders();

    #[cfg(all(
        feature = "build-shader-validation",
        any(
            not(any(target_os = "macos", target_os = "windows")),
            all(target_os = "macos", feature = "macos-blade")
        )
    ))]
    check_blade_wgsl_shaders();

    match target.as_deref() {
        Ok("macos") => {
            #[cfg(target_os = "macos")]
            macos::build();
        }
        Ok("windows") => {
            #[cfg(target_os = "windows")]
            windows::build();
        }
        _ => (),
    };
}

#[cfg(feature = "build-shader-validation")]
fn check_nova_wgsl_shaders() {
    use std::collections::BTreeSet;

    const CORE: &str = "./src/platform/nova/shaders/core.wgsl";
    const SHAPE: &str = "./src/platform/nova/shaders/shape.wgsl";
    const QUAD_COMMON: &str = "./src/platform/nova/shaders/quad_common.wgsl";

    let shader_bundles = [
        (
            "nova solid quad shader",
            &[CORE, "./src/platform/nova/shaders/solid_quad.wgsl"][..],
        ),
        (
            "nova mono sprite shader",
            &[
                CORE,
                "./src/platform/nova/shaders/text.wgsl",
                "./src/platform/nova/shaders/mono_sprite.wgsl",
            ][..],
        ),
        (
            "nova quad shader",
            &[
                CORE,
                SHAPE,
                QUAD_COMMON,
                "./src/platform/nova/shaders/quad.wgsl",
            ][..],
        ),
        (
            "nova shadow shader",
            &[CORE, SHAPE, "./src/platform/nova/shaders/shadow.wgsl"][..],
        ),
        (
            "nova path shader",
            &[CORE, QUAD_COMMON, "./src/platform/nova/shaders/path.wgsl"][..],
        ),
        (
            "nova poly sprite shader",
            &[CORE, SHAPE, "./src/platform/nova/shaders/poly_sprite.wgsl"][..],
        ),
        (
            "nova underline shader",
            &[CORE, "./src/platform/nova/shaders/underline.wgsl"][..],
        ),
        (
            "nova surface shader",
            &[CORE, "./src/platform/nova/shaders/surface.wgsl"][..],
        ),
        (
            "nova backdrop blur shader",
            &[
                CORE,
                SHAPE,
                "./src/platform/nova/shaders/backdrop_blur.wgsl",
            ][..],
        ),
    ];

    let mut covered_shader_paths = BTreeSet::new();
    for (bundle_name, shader_source_paths) in shader_bundles {
        for shader_source_path in shader_source_paths {
            covered_shader_paths.insert(*shader_source_path);
        }
        check_wgsl_shader_bundle(bundle_name, shader_source_paths);
    }

    check_nova_wgsl_shader_coverage(&covered_shader_paths);
}

#[cfg(feature = "build-shader-validation")]
fn check_nova_wgsl_shader_coverage(covered_shader_paths: &std::collections::BTreeSet<&str>) {
    use std::path::Path;
    use std::process;

    const SHADER_DIR: &str = "./src/platform/nova/shaders";

    println!("cargo:rerun-if-changed={SHADER_DIR}");
    for shader_entry in std::fs::read_dir(SHADER_DIR).unwrap_or_else(|error| {
        println!("cargo::error=Failed to read Nova WGSL shader directory {SHADER_DIR}: {error}");
        process::exit(1);
    }) {
        let shader_entry = shader_entry.unwrap_or_else(|error| {
            println!("cargo::error=Failed to read Nova WGSL shader directory entry: {error}");
            process::exit(1);
        });
        let shader_path = shader_entry.path();
        if shader_path
            .extension()
            .and_then(|extension| extension.to_str())
            != Some("wgsl")
        {
            continue;
        }

        let shader_path = shader_path.to_string_lossy().replace('\\', "/");
        let shader_path = shader_path
            .strip_prefix("./")
            .map_or(shader_path.as_str(), |path| path);
        let normalized_shader_path = format!("./{shader_path}");

        if !covered_shader_paths.contains(normalized_shader_path.as_str()) {
            let shader_name = Path::new(&normalized_shader_path)
                .file_name()
                .and_then(|file_name| file_name.to_str())
                .unwrap_or(&normalized_shader_path);
            if shader_name == "basic_quad.wgsl" {
                println!(
                    "cargo::error=basic_quad.wgsl is deprecated and must not live in the production Nova shader directory"
                );
            } else {
                println!(
                    "cargo::error=Nova WGSL shader {normalized_shader_path} is not covered by build shader validation"
                );
            }
            process::exit(1);
        }
    }
}

#[cfg(all(
    feature = "build-shader-validation",
    any(
        not(any(target_os = "macos", target_os = "windows")),
        all(target_os = "macos", feature = "macos-blade")
    )
))]
fn check_blade_wgsl_shaders() {
    check_wgsl_shader("./src/platform/blade/shaders.wgsl");
}

#[cfg(any(
    all(
        feature = "build-shader-validation",
        any(
            target_os = "linux",
            target_os = "freebsd",
            target_os = "windows",
            not(any(target_os = "macos", target_os = "windows"))
        )
    ),
    all(
        feature = "build-shader-validation",
        target_os = "macos",
        feature = "macos-blade"
    )
))]
fn check_wgsl_shader(shader_source_path: &str) {
    use std::path::PathBuf;
    use std::str::FromStr;

    let shader_path = PathBuf::from_str(shader_source_path).unwrap();
    println!("cargo:rerun-if-changed={}", &shader_path.display());

    let shader_source = std::fs::read_to_string(&shader_path).unwrap();
    validate_wgsl_shader(&shader_source, shader_source_path);
}

#[cfg(feature = "build-shader-validation")]
fn check_wgsl_shader_bundle(bundle_name: &str, shader_source_paths: &[&str]) {
    use std::path::PathBuf;
    use std::str::FromStr;

    let mut shader_source = String::new();
    for shader_source_path in shader_source_paths {
        let shader_path = PathBuf::from_str(shader_source_path).unwrap();
        println!("cargo:rerun-if-changed={}", &shader_path.display());
        shader_source.push_str(&std::fs::read_to_string(&shader_path).unwrap());
        shader_source.push('\n');
    }

    validate_wgsl_shader(&shader_source, bundle_name);
}

#[cfg(feature = "build-shader-validation")]
fn validate_wgsl_shader(shader_source: &str, shader_source_path: &str) {
    use std::process;

    match naga::front::wgsl::parse_str(&shader_source) {
        Ok(module) => {
            let mut validator = naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            );
            if let Err(e) = validator.validate(&module) {
                println!(
                    "cargo::error=WGSL shader validation failed:\n{}",
                    e.emit_to_string_with_path(&shader_source, shader_source_path)
                );
                process::exit(1);
            }
        }
        Err(e) => {
            println!(
                "cargo::error=WGSL shader compilation failed:\n{}",
                e.emit_to_string_with_path(&shader_source, shader_source_path)
            );
            process::exit(1);
        }
    }
}
#[cfg(target_os = "macos")]
mod macos {
    use std::{
        env,
        path::{Path, PathBuf},
    };

    use cbindgen::Config;

    pub(super) fn build() {
        generate_dispatch_bindings();
        #[cfg(not(feature = "macos-blade"))]
        {
            let header_path = generate_shader_bindings();

            #[cfg(feature = "runtime_shaders")]
            emit_stitched_shaders(&header_path);
            #[cfg(not(feature = "runtime_shaders"))]
            compile_metal_shaders(&header_path);
        }
    }

    fn generate_dispatch_bindings() {
        println!("cargo:rustc-link-lib=framework=System");

        let bindings = bindgen::Builder::default()
            .header("src/platform/mac/dispatch.h")
            .allowlist_var("_dispatch_main_q")
            .allowlist_var("_dispatch_source_type_data_add")
            .allowlist_var("DISPATCH_QUEUE_PRIORITY_HIGH")
            .allowlist_var("DISPATCH_TIME_NOW")
            .allowlist_function("dispatch_get_global_queue")
            .allowlist_function("dispatch_async_f")
            .allowlist_function("dispatch_after_f")
            .allowlist_function("dispatch_time")
            .allowlist_function("dispatch_source_merge_data")
            .allowlist_function("dispatch_source_create")
            .allowlist_function("dispatch_source_set_event_handler_f")
            .allowlist_function("dispatch_resume")
            .allowlist_function("dispatch_suspend")
            .allowlist_function("dispatch_source_cancel")
            .allowlist_function("dispatch_set_context")
            .parse_callbacks(Box::new(bindgen::CargoCallbacks::new()))
            .layout_tests(false)
            .generate()
            .expect("unable to generate bindings");

        let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
        bindings
            .write_to_file(out_path.join("dispatch_sys.rs"))
            .expect("couldn't write dispatch bindings");
    }

    fn generate_shader_bindings() -> PathBuf {
        let output_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("scene.h");
        let crate_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
        let mut config = Config {
            include_guard: Some("SCENE_H".into()),
            language: cbindgen::Language::C,
            no_includes: true,
            ..Default::default()
        };
        config.export.include.extend([
            "Bounds".into(),
            "Corners".into(),
            "Edges".into(),
            "Size".into(),
            "Pixels".into(),
            "PointF".into(),
            "Hsla".into(),
            "ContentMask".into(),
            "Uniforms".into(),
            "AtlasTile".into(),
            "PathRasterizationInputIndex".into(),
            "PathVertex_ScaledPixels".into(),
            "PathRasterizationVertex".into(),
            "ShadowInputIndex".into(),
            "Shadow".into(),
            "QuadInputIndex".into(),
            "Underline".into(),
            "UnderlineInputIndex".into(),
            "Quad".into(),
            "BorderStyle".into(),
            "SpriteInputIndex".into(),
            "MonochromeSprite".into(),
            "PolychromeSprite".into(),
            "PathSprite".into(),
            "SurfaceInputIndex".into(),
            "SurfaceBounds".into(),
            "TransformationMatrix".into(),
        ]);
        config.no_includes = true;
        config.enumeration.prefix_with_name = true;

        let mut builder = cbindgen::Builder::new();

        let src_paths = [
            crate_dir.join("src/scene.rs"),
            crate_dir.join("src/geometry.rs"),
            crate_dir.join("src/color.rs"),
            crate_dir.join("src/window.rs"),
            crate_dir.join("src/platform.rs"),
            crate_dir.join("src/platform/mac/metal_renderer.rs"),
        ];
        for src_path in src_paths {
            println!("cargo:rerun-if-changed={}", src_path.display());
            builder = builder.with_src(src_path);
        }

        builder
            .with_config(config)
            .generate()
            .expect("Unable to generate bindings")
            .write_to_file(&output_path);

        output_path
    }

    /// To enable runtime compilation, we need to "stitch" the shaders file with the generated header
    /// so that it is self-contained.
    #[cfg(feature = "runtime_shaders")]
    fn emit_stitched_shaders(header_path: &Path) {
        use std::str::FromStr;
        fn stitch_header(header: &Path, shader_path: &Path) -> std::io::Result<PathBuf> {
            let header_contents = std::fs::read_to_string(header)?;
            let shader_contents = std::fs::read_to_string(shader_path)?;
            let stitched_contents = format!("{header_contents}\n{shader_contents}");
            let out_path =
                PathBuf::from(env::var("OUT_DIR").unwrap()).join("stitched_shaders.metal");
            std::fs::write(&out_path, stitched_contents)?;
            Ok(out_path)
        }
        let shader_source_path = "./src/platform/mac/shaders.metal";
        let shader_path = PathBuf::from_str(shader_source_path).unwrap();
        stitch_header(header_path, &shader_path).unwrap();
        println!("cargo:rerun-if-changed={}", &shader_source_path);
    }

    #[cfg(not(feature = "runtime_shaders"))]
    fn compile_metal_shaders(header_path: &Path) {
        use std::process::{self, Command};
        let shader_path = "./src/platform/mac/shaders.metal";
        let air_output_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("shaders.air");
        let metallib_output_path =
            PathBuf::from(env::var("OUT_DIR").unwrap()).join("shaders.metallib");
        println!("cargo:rerun-if-changed={}", shader_path);

        let output = Command::new("xcrun")
            .args([
                "-sdk",
                "macosx",
                "metal",
                "-gline-tables-only",
                "-mmacosx-version-min=10.15.7",
                "-MO",
                "-c",
                shader_path,
                "-include",
                (header_path.to_str().unwrap()),
                "-o",
            ])
            .arg(&air_output_path)
            .output()
            .unwrap();

        if !output.status.success() {
            println!(
                "cargo::error=metal shader compilation failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            process::exit(1);
        }

        let output = Command::new("xcrun")
            .args(["-sdk", "macosx", "metallib"])
            .arg(air_output_path)
            .arg("-o")
            .arg(metallib_output_path)
            .output()
            .unwrap();

        if !output.status.success() {
            println!(
                "cargo::error=metallib compilation failed:\n{}",
                String::from_utf8_lossy(&output.stderr)
            );
            process::exit(1);
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use std::path::Path;

    pub(super) fn build() {
        #[cfg(feature = "windows-manifest")]
        embed_resource();
    }

    #[cfg(feature = "windows-manifest")]
    fn embed_resource() {
        let manifest = std::path::Path::new("resources/windows/gpui.manifest.xml");
        let rc_file = std::path::Path::new("resources/windows/gpui.rc");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!("cargo:rerun-if-changed={}", rc_file.display());
        embed_resource::compile(rc_file, embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
