//! Build-time WGSL compilation and embedding for nova-gfx backends.
//!
//! A crate calls [`ShaderSet::emit`] from its `build.rs`, then includes the generated
//! table from a small module:
//!
//! ```no_run
//! // build.rs
//! use gfx_shader_build::{Shader, ShaderSet, ShaderStage};
//!
//! let shaders = ShaderSet::new("viewer").shader(
//!     Shader::new("scene_view")
//!         .wgsl_file("src/scene_view.wgsl")
//!         .entry("vs_main", ShaderStage::Vertex)
//!         .entry("fs_main", ShaderStage::Fragment),
//! );
//! if let Err(error) = shaders.emit() {
//!     println!("cargo::error={error}");
//!     std::process::exit(1);
//! }
//! ```
//!
//! ```ignore
//! // src/shader_table.rs
//! include!(concat!(env!("OUT_DIR"), "/shaders_bytes.rs"));
//! ```
//!
//! The generated module exposes, per enabled backend, a `{name}_{backend}_shader`
//! lookup returning [`gfx_core::EmbeddedShader`] values plus a
//! `{NAME}_{BACKEND}_SHADER_ARTIFACT_KIND` description intended for a one-time
//! startup log, so an application log states which shader path a binary uses.
//!
//! The calling crate must depend on `gfx-core`, because the generated table names
//! `gfx_core::EmbeddedShader` directly.

use std::path::{Path, PathBuf};
use std::{env, fmt, fs};

/// Shader stage an entry point belongs to.
///
/// Re-exported so a build script only needs this crate to declare shaders.
pub use gfx_core::ShaderStage;
/// Target Metal Shading Language version for [`Shader::msl_version`].
pub use gfx_shader::MslVersion;

/// Directory created inside `OUT_DIR` for generated shader payloads.
const ARTIFACT_DIR: &str = "gfx_shaders";

/// Which backends [`ShaderSet::emit`] produces artifacts for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendSelection {
    /// Only backends the calling crate requested through its own Cargo features.
    ///
    /// This is the default. Use it when the crate also gates its runtime code on those
    /// same features, so the generated table and the runtime agree.
    Features,
    /// Every backend the target platform supports, regardless of Cargo features.
    ///
    /// Use this when the shaders must work with whichever backend the host application
    /// selected and the crate's own features do not decide that. Embedding the extra
    /// artifacts costs a few kilobytes.
    Platform,
}

/// Policy for DX12 artifacts when the build host cannot run the Direct3D compiler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Dx12ArtifactPolicy {
    /// Embed translated HLSL and let an explicitly compiler-enabled DX12 runtime compile it.
    AllowRuntimeCompilation,
    /// Require build-time DXBC. A Windows target built on a host without `D3DCompile` fails.
    RequireBytecode,
}

/// One WGSL shader and the entry points compiled from it.
///
/// A shader is composed from an optional prelude followed by one or more WGSL
/// sources, matching how Nova's built-in shaders share `core.wgsl` and friends
/// between bundles.
#[derive(Clone, Debug)]
#[must_use]
pub struct Shader {
    name: String,
    prelude: String,
    parts: Vec<ShaderPart>,
    entry_points: Vec<(String, ShaderStage)>,
    target_os: Option<String>,
    msl_version: MslVersion,
}

#[derive(Clone, Debug)]
enum ShaderPart {
    File(PathBuf),
    Inline(String),
}

impl Shader {
    /// Starts a shader identified by `name`, which is used for build diagnostics
    /// and artifact file names.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prelude: String::new(),
            parts: Vec::new(),
            entry_points: Vec::new(),
            target_os: None,
            msl_version: MslVersion::V1_0,
        }
    }

    /// Selects the Metal Shading Language version generated for this shader.
    ///
    /// Defaults to MSL 1.0. Raise it only when the shader needs a newer language
    /// feature, such as the `instance_id` attribute added in MSL 1.2.
    pub fn msl_version(mut self, version: MslVersion) -> Self {
        self.msl_version = version;
        self
    }

    /// Restricts this shader to one target operating system.
    ///
    /// Use this for shaders that only a single platform's renderer binds, so other
    /// targets neither translate them nor fail on features they cannot express. The
    /// value is compared against `CARGO_CFG_TARGET_OS`.
    pub fn target_os(mut self, target_os: impl Into<String>) -> Self {
        self.target_os = Some(target_os.into());
        self
    }

    /// Prepends a WGSL prelude, such as a required `enable` directive.
    pub fn prelude(mut self, prelude: impl Into<String>) -> Self {
        self.prelude = prelude.into();
        self
    }

    /// Appends a WGSL file, read relative to the package root.
    pub fn wgsl_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.parts.push(ShaderPart::File(path.into()));
        self
    }

    /// Appends inline WGSL source.
    pub fn wgsl(mut self, source: impl Into<String>) -> Self {
        self.parts.push(ShaderPart::Inline(source.into()));
        self
    }

    /// Declares one entry point compiled from this shader.
    pub fn entry(mut self, entry_point: impl Into<String>, stage: ShaderStage) -> Self {
        self.entry_points.push((entry_point.into(), stage));
        self
    }

    /// Returns the shader name used in diagnostics.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Reads and concatenates the declared WGSL sources.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Read`] when a declared file cannot be read.
    fn source(&self) -> Result<String, Error> {
        let mut source = self.prelude.clone();
        for part in &self.parts {
            match part {
                ShaderPart::Inline(inline) => source.push_str(inline),
                ShaderPart::File(path) => {
                    let contents = fs::read_to_string(path).map_err(|error| Error::Read {
                        path: path.clone(),
                        source: error,
                    })?;
                    source.push_str(&contents);
                }
            }
            source.push('\n');
        }
        Ok(source)
    }

    /// Returns whether this shader applies to `target_os`.
    fn applies_to(&self, target_os: &str) -> bool {
        self.target_os
            .as_deref()
            .is_none_or(|declared| declared == target_os)
    }

    /// Returns the WGSL files this shader reads, for `rerun-if-changed` reporting.
    fn watched_files(&self) -> impl Iterator<Item = &Path> {
        self.parts.iter().filter_map(|part| match part {
            ShaderPart::File(path) => Some(path.as_path()),
            ShaderPart::Inline(_) => None,
        })
    }
}

/// A set of shaders compiled into one generated table.
#[derive(Clone, Debug)]
#[must_use]
pub struct ShaderSet {
    name: String,
    shaders: Vec<Shader>,
    backend_selection: BackendSelection,
    dx12_artifact_policy: Dx12ArtifactPolicy,
}

impl ShaderSet {
    /// Starts a set. `name` prefixes every generated symbol, so it must be a valid
    /// Rust identifier fragment such as `nova` or `viewer`.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            shaders: Vec::new(),
            backend_selection: BackendSelection::Features,
            dx12_artifact_policy: Dx12ArtifactPolicy::RequireBytecode,
        }
    }

    /// Chooses which backends artifacts are generated for.
    ///
    /// Defaults to [`BackendSelection::Features`].
    pub fn backend_selection(mut self, selection: BackendSelection) -> Self {
        self.backend_selection = selection;
        self
    }

    /// Chooses whether a Windows/DX12 target may fall back to runtime HLSL compilation.
    ///
    /// [`Dx12ArtifactPolicy::RequireBytecode`] is the default. Tools that intentionally
    /// accept startup compilation must opt into [`Dx12ArtifactPolicy::AllowRuntimeCompilation`].
    pub fn dx12_artifact_policy(mut self, policy: Dx12ArtifactPolicy) -> Self {
        self.dx12_artifact_policy = policy;
        self
    }

    /// Adds one shader to the set.
    pub fn shader(mut self, shader: Shader) -> Self {
        self.shaders.push(shader);
        self
    }

    /// Compiles every declared entry point for the enabled backends and writes the
    /// generated table to `OUT_DIR/shaders_bytes.rs`.
    ///
    /// Emits `cargo:rerun-if-changed` for every WGSL file it reads, so editing a
    /// shader rebuilds the artifacts. WGSL is parsed and validated once per declared
    /// shader bundle, then reused for every entry point and enabled backend. DX12
    /// fallback to embedded HLSL exists only when explicitly selected by policy.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the set declares no shaders or reuses an entry
    /// point name, [`Error::Read`] when a WGSL file cannot be read,
    /// [`Error::Validate`] when WGSL parsing/validation fails,
    /// [`Error::Translate`] when backend translation fails, [`Error::Compile`] when
    /// Direct3D bytecode compilation fails, and [`Error::Write`] when a generated
    /// file cannot be written.
    pub fn emit(self) -> Result<(), Error> {
        let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|error| {
            Error::Invalid(format!(
                "OUT_DIR is not set; ShaderSet::emit must run from a build script: {error}"
            ))
        })?);
        let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        let shaders: Vec<&Shader> = self
            .shaders
            .iter()
            .filter(|shader| shader.applies_to(&target_os))
            .collect();
        validate(&self.name, &shaders)?;

        let backends = enabled_backends(&target_os, self.backend_selection);

        for shader in &shaders {
            for path in shader.watched_files() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }

        let prepared = shaders
            .iter()
            .map(|shader| PreparedShader::new(shader))
            .collect::<Result<Vec<_>, _>>()?;

        let mut generated = String::from("// @generated by gfx-shader-build. Do not edit.\n\n");

        for backend in &backends {
            self.emit_backend(&out_dir, *backend, &prepared, &mut generated)?;
        }

        let table_path = out_dir.join("shaders_bytes.rs");
        write_artifact(&table_path, generated.as_bytes())
    }

    fn emit_backend(
        &self,
        out_dir: &Path,
        backend: Backend,
        shaders: &[PreparedShader<'_>],
        generated: &mut String,
    ) -> Result<(), Error> {
        let prefix = format!("{}_{}", self.name, backend.slug());
        let prefix_upper = format!("{}_{}", self.name.to_uppercase(), backend.upper());

        generated.push_str(&format!(
            "/// How this build supplies {} shaders.\npub const {}_SHADER_ARTIFACT_KIND: &str = \"{}\";\n\n",
            backend.upper(),
            prefix_upper,
            backend.artifact_kind(),
        ));

        let mut arms = String::new();
        for prepared in shaders {
            let shader = prepared.shader;
            for (entry_point, stage) in &shader.entry_points {
                let (extension, include_macro, constructor, payload, glsl) =
                    self.compile_entry(out_dir, backend, prepared, entry_point, *stage)?;

                let relative = format!(
                    "{ARTIFACT_DIR}/{}/{entry_point}.{extension}",
                    backend.slug()
                );
                write_artifact(&out_dir.join(&relative), &payload)?;

                let constant = format!("{prefix_upper}_{}", entry_point.to_uppercase());
                if let Some(glsl) = glsl {
                    let buffers = glsl.buffers.iter().map(|buffer| format!("::gfx_core::GlslBufferBinding {{ name: ::std::borrow::Cow::Borrowed({:?}), binding: {}, kind: ::gfx_core::ResourceBindingType::{:?} }}", buffer.name, buffer.binding, buffer.kind)).collect::<Vec<_>>().join(",");
                    let textures = glsl.textures.iter().map(|texture| format!("::gfx_core::GlslTextureBinding {{ name: ::std::borrow::Cow::Borrowed({:?}), texture: {}, sampler: {:?} }}", texture.name, texture.texture, texture.sampler)).collect::<Vec<_>>().join(",");
                    generated.push_str(&format!("const {constant}: ::gfx_core::EmbeddedShader = ::gfx_core::EmbeddedShader::Glsl {{ source: include_str!(concat!(env!(\"OUT_DIR\"), \"/{relative}\")), buffers: &[{buffers}], textures: &[{textures}] }};\n"));
                } else {
                    generated.push_str(&format!(
                    "const {constant}: ::gfx_core::EmbeddedShader = ::gfx_core::EmbeddedShader::{constructor}({include_macro}!(concat!(env!(\"OUT_DIR\"), \"/{relative}\")));\n"
                    ));
                }
                arms.push_str(&format!("        \"{entry_point}\" => Some({constant}),\n"));
            }
        }

        generated.push_str(&format!(
            "/// Resolves a build-generated {0} shader by entry point.\n///\n/// Returns `None` when this build did not embed an artifact for `entry_point`.\npub fn {prefix}_shader(entry_point: &str) -> Option<::gfx_core::EmbeddedShader> {{\n    match entry_point {{\n{arms}        _ => None,\n    }}\n}}\n\n",
            backend.upper(),
        ));

        Ok(())
    }

    /// Produces one artifact's payload and how the generated table embeds it.
    fn compile_entry(
        &self,
        out_dir: &Path,
        backend: Backend,
        prepared: &PreparedShader<'_>,
        entry_point: &str,
        stage: ShaderStage,
    ) -> Result<
        (
            &'static str,
            &'static str,
            &'static str,
            Vec<u8>,
            Option<gfx_core::GlslShader>,
        ),
        Error,
    > {
        let shader = prepared.shader;
        match backend {
            Backend::OpenGl => {
                let binary = prepared
                    .module
                    .compile_glsl(stage, entry_point)
                    .map_err(|error| Error::Translate {
                        shader: shader.name.clone(),
                        entry_point: entry_point.into(),
                        message: error.to_string(),
                    })?;
                let gfx_core::ShaderCode::Glsl(glsl) = binary.code else {
                    return Err(Error::Invalid(
                        "OpenGL translation did not produce GLSL".into(),
                    ));
                };
                Ok((
                    "glsl",
                    "include_str",
                    "Glsl",
                    glsl.source.as_bytes().to_vec(),
                    Some(glsl),
                ))
            }
            Backend::Dx11 => {
                let binary = prepared
                    .module
                    .compile_hlsl_dx11(stage, entry_point)
                    .map_err(|error| Error::Translate {
                        shader: shader.name.clone(),
                        entry_point: entry_point.to_owned(),
                        message: error.to_string(),
                    })?;
                let gfx_core::ShaderCode::Hlsl(hlsl) = binary.code else {
                    return Err(Error::Invalid(
                        "DX11 translation did not produce HLSL".into(),
                    ));
                };
                let bytecode = compile_hlsl_to_dxbc(&hlsl, entry_point, stage, true)
                    .map_err(|message| Error::Compile {
                        shader: shader.name.clone(),
                        entry_point: entry_point.to_owned(),
                        message,
                    })?
                    .ok_or_else(|| {
                        Error::Invalid(
                            "DX11 requires build-time D3DCompile on a Windows host".into(),
                        )
                    })?;
                Ok(("dxbc", "include_bytes", "DxBytecode", bytecode, None))
            }
            Backend::Dx12 => {
                let hlsl = translate(&prepared.module, entry_point, stage, shader)?;
                match compile_hlsl_to_dxbc(&hlsl, entry_point, stage, false).map_err(|message| {
                    Error::Compile {
                        shader: shader.name.clone(),
                        entry_point: entry_point.to_string(),
                        message,
                    }
                })? {
                    Some(bytecode) => Ok(("dxbc", "include_bytes", "DxBytecode", bytecode, None)),
                    None => match self.dx12_artifact_policy {
                        Dx12ArtifactPolicy::AllowRuntimeCompilation => {
                            println!(
                                "cargo::warning=DX12 shader `{}` entry point `{entry_point}` is embedded as HLSL because the build host cannot run D3DCompile; renderer startup requires an explicitly compiler-enabled DX12 backend",
                                shader.name
                            );
                            Ok(("hlsl", "include_str", "Hlsl", hlsl.into_bytes(), None))
                        }
                        Dx12ArtifactPolicy::RequireBytecode => Err(Error::Dx12BytecodeRequired {
                            shader: shader.name.clone(),
                            entry_point: entry_point.to_string(),
                        }),
                    },
                }
            }
            Backend::Vulkan => {
                let spirv = translate_vulkan(&prepared.module, entry_point, stage, shader)?;
                Ok(("spv", "include_bytes", "SpirvBytes", spirv, None))
            }
            Backend::Metal => {
                let msl = translate_metal(&prepared.module, entry_point, stage, shader)?;
                let metallib =
                    compile_msl_to_metallib(out_dir, entry_point, &msl).map_err(|message| {
                        Error::Compile {
                            shader: shader.name.clone(),
                            entry_point: entry_point.to_string(),
                            message,
                        }
                    })?;
                Ok(("metallib", "include_bytes", "Metallib", metallib, None))
            }
        }
    }
}

struct PreparedShader<'a> {
    shader: &'a Shader,
    module: gfx_shader::WgslModule,
}

impl<'a> PreparedShader<'a> {
    fn new(shader: &'a Shader) -> Result<Self, Error> {
        let source = shader.source()?;
        let module = gfx_shader::WgslModule::parse(&source).map_err(|error| Error::Validate {
            shader: shader.name.clone(),
            message: error.to_string(),
        })?;
        Ok(Self { shader, module })
    }
}
/// Backends an emit can produce artifacts for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Backend {
    OpenGl,
    Dx11,
    Dx12,
    Vulkan,
    Metal,
}

impl Backend {
    fn slug(self) -> &'static str {
        match self {
            Self::OpenGl => "opengl",
            Self::Dx11 => "dx11",
            Self::Dx12 => "dx12",
            Self::Vulkan => "vulkan",
            Self::Metal => "metal",
        }
    }

    fn upper(self) -> &'static str {
        match self {
            Self::OpenGl => "OPENGL",
            Self::Dx11 => "DX11",
            Self::Dx12 => "DX12",
            Self::Vulkan => "VULKAN",
            Self::Metal => "METAL",
        }
    }

    /// Describes what this build embedded, for the caller's startup log.
    fn artifact_kind(self) -> &'static str {
        match self {
            Self::OpenGl => "GLSL 4.50 and resource reflection generated at build time",
            Self::Dx11 => "precompiled D3D11 SM5.0 bytecode embedded at build time",
            Self::Dx12 if cfg!(target_os = "windows") => {
                "precompiled D3D bytecode embedded at build time"
            }
            Self::Dx12 => "HLSL source explicitly allowed for runtime compilation",
            Self::Vulkan => "SPIR-V words generated at build time",
            Self::Metal => "precompiled metallib embedded at build time",
        }
    }
}

/// Returns the backends this build should emit artifacts for.
///
/// A backend must be valid for the target platform. Under
/// [`BackendSelection::Features`] the calling crate must also request it, so the
/// generated table matches runtime code gated on the same features.
fn enabled_backends(target_os: &str, selection: BackendSelection) -> Vec<Backend> {
    let requested = |name: &str| {
        selection == BackendSelection::Platform
            || env::var_os(format!("CARGO_FEATURE_{name}")).is_some()
    };

    let mut backends = Vec::new();
    if requested("NOVA_GFX_OPENGL") && matches!(target_os, "windows" | "linux") {
        backends.push(Backend::OpenGl);
    }
    if requested("NOVA_GFX_DX11") && target_os == "windows" {
        backends.push(Backend::Dx11);
    }
    if requested("NOVA_GFX_DX12") && target_os == "windows" {
        backends.push(Backend::Dx12);
    }
    if requested("NOVA_GFX_VULKAN") && matches!(target_os, "windows" | "linux" | "freebsd") {
        backends.push(Backend::Vulkan);
    }
    if requested("NOVA_GFX_METAL") && target_os == "macos" {
        backends.push(Backend::Metal);
    }
    backends
}

/// Validates the shaders that apply to the current target.
fn validate(set_name: &str, shaders: &[&Shader]) -> Result<(), Error> {
    if shaders.is_empty() {
        return Err(Error::Invalid(format!(
            "shader set `{set_name}` declares no shaders for this target"
        )));
    }
    if !set_name
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return Err(Error::Invalid(format!(
            "shader set name `{set_name}` must be a Rust identifier fragment"
        )));
    }

    // Entry point names are the runtime lookup key, so a duplicate would make the
    // generated table silently drop one of the two declarations.
    let mut seen: Vec<&str> = Vec::new();
    for shader in shaders {
        for (entry_point, _) in &shader.entry_points {
            if seen.contains(&entry_point.as_str()) {
                return Err(Error::Invalid(format!(
                    "entry point `{entry_point}` is declared more than once in shader set `{set_name}`"
                )));
            }
            seen.push(entry_point);
        }
    }

    Ok(())
}

fn translate(
    module: &gfx_shader::WgslModule,
    entry_point: &str,
    stage: ShaderStage,
    shader: &Shader,
) -> Result<String, Error> {
    let binary = module
        .compile_hlsl(stage, entry_point)
        .map_err(|error| Error::Translate {
            shader: shader.name.clone(),
            entry_point: entry_point.to_string(),
            message: error.to_string(),
        })?;

    match binary.code {
        gfx_core::ShaderCode::Hlsl(hlsl) => Ok(hlsl),
        _ => Err(Error::Translate {
            shader: shader.name.clone(),
            entry_point: entry_point.to_string(),
            message: "WGSL translation did not produce HLSL source".to_string(),
        }),
    }
}

fn translate_metal(
    module: &gfx_shader::WgslModule,
    entry_point: &str,
    stage: ShaderStage,
    shader: &Shader,
) -> Result<String, Error> {
    let binary = module
        .compile_msl_with_version(stage, entry_point, shader.msl_version)
        .map_err(|error| Error::Translate {
            shader: shader.name.clone(),
            entry_point: entry_point.to_string(),
            message: error.to_string(),
        })?;

    match binary.code {
        gfx_core::ShaderCode::Msl(msl) => Ok(msl),
        _ => Err(Error::Translate {
            shader: shader.name.clone(),
            entry_point: entry_point.to_string(),
            message: "WGSL translation did not produce MSL source".to_string(),
        }),
    }
}

#[cfg(target_os = "macos")]
fn compile_msl_to_metallib(
    out_dir: &Path,
    entry_point: &str,
    msl: &str,
) -> Result<Vec<u8>, String> {
    use std::process::Command;

    let safe_entry = entry_point
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let build_dir = out_dir.join(ARTIFACT_DIR).join("metal-build");
    fs::create_dir_all(&build_dir)
        .map_err(|error| format!("failed to create Metal shader build directory: {error}"))?;

    let source_path = build_dir.join(format!("{safe_entry}.metal"));
    let air_path = build_dir.join(format!("{safe_entry}.air"));
    let metallib_path = build_dir.join(format!("{safe_entry}.metallib"));
    write_artifact(&source_path, msl.as_bytes()).map_err(|error| error.to_string())?;

    let mut metal = Command::new("xcrun");
    metal
        .args(["-sdk", "macosx", "metal", "-c"])
        .arg(&source_path)
        .arg("-o")
        .arg(&air_path);
    if let Ok(deployment_target) = env::var("MACOSX_DEPLOYMENT_TARGET")
        && !deployment_target.trim().is_empty()
    {
        metal.arg(format!("-mmacosx-version-min={}", deployment_target.trim()));
    }
    let output = metal
        .output()
        .map_err(|error| format!("failed to run xcrun metal: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "metal compilation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let output = Command::new("xcrun")
        .args(["-sdk", "macosx", "metallib"])
        .arg(&air_path)
        .arg("-o")
        .arg(&metallib_path)
        .output()
        .map_err(|error| format!("failed to run xcrun metallib: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "metallib link failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let bytes = fs::read(&metallib_path)
        .map_err(|error| format!("failed to read generated metallib: {error}"))?;
    if bytes.is_empty() {
        return Err("generated metallib is empty".to_string());
    }
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
fn compile_msl_to_metallib(
    _out_dir: &Path,
    _entry_point: &str,
    _msl: &str,
) -> Result<Vec<u8>, String> {
    Err(
        "Metal production shaders require build-time metallib compilation on a macOS build host"
            .to_string(),
    )
}

fn translate_vulkan(
    module: &gfx_shader::WgslModule,
    entry_point: &str,
    stage: ShaderStage,
    shader: &Shader,
) -> Result<Vec<u8>, Error> {
    let binary = module
        .compile_spirv(stage, entry_point)
        .map_err(|error| Error::Translate {
            shader: shader.name.clone(),
            entry_point: entry_point.to_string(),
            message: error.to_string(),
        })?;

    match binary.code {
        gfx_core::ShaderCode::Spirv(words) => {
            Ok(words.into_iter().flat_map(u32::to_le_bytes).collect())
        }
        _ => Err(Error::Translate {
            shader: shader.name.clone(),
            entry_point: entry_point.to_string(),
            message: "WGSL translation did not produce SPIR-V words".to_string(),
        }),
    }
}
/// Compiles generated HLSL to Direct3D bytecode when the build host can run the
/// Direct3D compiler.
///
/// This helper intentionally lives in the build crate instead of depending on the
/// complete DX12 runtime backend. GPUI build scripts therefore do not pull device,
/// swapchain, allocator, and presentation code into the host graph merely to call FXC.
#[cfg(target_os = "windows")]
#[expect(unsafe_code, reason = "FXC D3DCompile and returned ID3DBlob byte buffer require Windows FFI")]
fn compile_hlsl_to_dxbc(
    hlsl: &str,
    entry_point: &str,
    stage: ShaderStage,
    dx11: bool,
) -> Result<Option<Vec<u8>>, String> {
    use windows::Win32::Graphics::Direct3D::Fxc::{
        D3DCOMPILE_ENABLE_STRICTNESS, D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile,
    };
    use windows::core::PCSTR;

    let target = match (stage, dx11) {
        (ShaderStage::Vertex, true) => b"vs_5_0\0",
        (ShaderStage::Fragment, true) => b"ps_5_0\0",
        (ShaderStage::Vertex, false) => b"vs_5_1\0",
        (ShaderStage::Fragment, false) => b"ps_5_1\0",
    };
    let entry_point = std::ffi::CString::new(entry_point)
        .map_err(|error| format!("entry point contains a NUL byte: {error}"))?;
    let mut bytecode = None;
    let mut errors = None;
    // SAFETY: source, entry-point and target pointers remain valid for the call.
    let result = unsafe {
        D3DCompile(
            hlsl.as_ptr().cast(),
            hlsl.len(),
            PCSTR::null(),
            None,
            None,
            PCSTR(entry_point.as_ptr().cast()),
            PCSTR(target.as_ptr()),
            D3DCOMPILE_ENABLE_STRICTNESS | D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &raw mut bytecode,
            Some(&raw mut errors),
        )
    };

    if let Err(error) = result {
        let diagnostics = errors
            .as_ref()
            .map(d3d_blob_message)
            .filter(|message| !message.is_empty())
            .unwrap_or_else(|| error.to_string());
        return Err(diagnostics);
    }

    let bytecode = bytecode
        .ok_or_else(|| "D3DCompile succeeded without returning shader bytecode".to_string())?;
    // SAFETY: D3DCompile returned a live immutable blob for the duration of this copy.
    let bytes = unsafe {
        std::slice::from_raw_parts(
            bytecode.GetBufferPointer().cast::<u8>(),
            bytecode.GetBufferSize(),
        )
    };
    if bytes.len() < 4 || &bytes[..4] != b"DXBC" {
        return Err("D3DCompile returned a payload without a DXBC container header".to_string());
    }
    Ok(Some(bytes.to_vec()))
}

#[cfg(target_os = "windows")]
#[expect(unsafe_code, reason = "Reading a live ID3DBlob pointer requires Windows FFI")]
fn d3d_blob_message(blob: &windows::Win32::Graphics::Direct3D::ID3DBlob) -> String {
    // SAFETY: the blob owns a byte range valid for its lifetime.
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize())
    };
    String::from_utf8_lossy(bytes)
        .trim_end_matches(char::from(0))
        .trim()
        .to_string()
}

#[cfg(not(target_os = "windows"))]
fn compile_hlsl_to_dxbc(
    _hlsl: &str,
    _entry_point: &str,
    _stage: ShaderStage,
    _dx11: bool,
) -> Result<Option<Vec<u8>>, String> {
    Ok(None)
}
fn write_artifact(path: &Path, payload: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| Error::Write {
            path: parent.to_path_buf(),
            source: error,
        })?;
    }

    // Build scripts may execute even when the translated payload is byte-for-byte identical.
    // Preserve the existing file timestamp in that case so downstream include_bytes!/include!
    // consumers are not needlessly rebuilt by Cargo/rustc.
    if fs::read(path).is_ok_and(|existing| existing == payload) {
        return Ok(());
    }

    fs::write(path, payload).map_err(|error| Error::Write {
        path: path.to_path_buf(),
        source: error,
    })
}

/// Failure while compiling or embedding shaders.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The declared shader set cannot produce a usable table.
    #[error("invalid shader set: {0}")]
    Invalid(String),
    /// A declared WGSL file could not be read.
    #[error("failed to read WGSL source {path}: {source}")]
    Read {
        /// File that could not be read.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
    /// WGSL parsing or validation failed for a declared shader bundle.
    #[error("WGSL validation failed for `{shader}`: {message}")]
    Validate {
        /// Shader declaration name.
        shader: String,
        /// Parser or validator diagnostic.
        message: String,
    },
    /// WGSL translation failed for one entry point.
    #[error("WGSL translation failed for `{shader}` entry point `{entry_point}`: {message}")]
    Translate {
        /// Shader name from the declaration.
        shader: String,
        /// Entry point being translated.
        entry_point: String,
        /// Translator diagnostic.
        message: String,
    },
    /// Direct3D bytecode compilation failed for translated HLSL.
    #[error(
        "Direct3D bytecode compilation failed for `{shader}` entry point `{entry_point}`: {message}"
    )]
    Compile {
        /// Shader declaration name.
        shader: String,
        /// Entry point that failed compilation.
        entry_point: String,
        /// Compiler diagnostic.
        message: String,
    },
    /// A strict DX12 shader set could not be compiled to bytecode on this build host.
    #[error(
        "DX12 shader `{shader}` entry point `{entry_point}` requires build-time D3D bytecode, but this build host cannot run D3DCompile; build the Windows artifact on a Windows host or explicitly allow runtime compilation for a non-production tool"
    )]
    Dx12BytecodeRequired {
        /// Shader declaration name.
        shader: String,
        /// Entry point that could not be precompiled.
        entry_point: String,
    },
    /// A generated file could not be written.
    #[error("failed to write generated shader output {path}: {source}")]
    Write {
        /// Path that could not be written.
        path: PathBuf,
        /// Underlying I/O error.
        source: std::io::Error,
    },
}

impl fmt::Display for Backend {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(self.upper())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUAL_ENTRY_WGSL: &str = r#"
        @vertex
        fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
            let x = f32(index & 1u);
            let y = f32((index >> 1u) & 1u);
            return vec4<f32>(x, y, 0.0, 1.0);
        }

        @fragment
        fn fs_main() -> @location(0) vec4<f32> {
            return vec4<f32>(1.0, 1.0, 1.0, 1.0);
        }
    "#;

    #[test]
    fn dx12_bytecode_is_strict_by_default() {
        let set = ShaderSet::new("strict_default");
        assert_eq!(
            set.dx12_artifact_policy,
            Dx12ArtifactPolicy::RequireBytecode
        );
    }

    #[test]
    fn prepared_shader_module_serves_multiple_entry_points() {
        let shader = Shader::new("dual_entry")
            .wgsl(DUAL_ENTRY_WGSL)
            .entry("vs_main", ShaderStage::Vertex)
            .entry("fs_main", ShaderStage::Fragment);
        let prepared = PreparedShader::new(&shader).expect("WGSL bundle should validate once");

        let vertex = translate(
            &prepared.module,
            "vs_main",
            ShaderStage::Vertex,
            prepared.shader,
        )
        .expect("validated module should translate vertex entry");
        let fragment = translate(
            &prepared.module,
            "fs_main",
            ShaderStage::Fragment,
            prepared.shader,
        )
        .expect("validated module should translate fragment entry");

        assert!(!vertex.is_empty());
        assert!(!fragment.is_empty());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn non_windows_build_host_reports_no_dxbc_compiler() {
        assert_eq!(
            compile_hlsl_to_dxbc("", "vs_main", ShaderStage::Vertex, false)
                .expect("host capability probe should not fail"),
            None
        );
    }
}
