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
use std::{env, fmt, fs, process};

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
            dx12_artifact_policy: Dx12ArtifactPolicy::AllowRuntimeCompilation,
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
    /// Production applications should select [`Dx12ArtifactPolicy::RequireBytecode`] so
    /// cross-compilation cannot silently reintroduce `D3DCompile` into startup.
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
    /// shader rebuilds the artifacts. Prints `cargo::warning` when DX12 shaders had
    /// to fall back to HLSL because the build host cannot run the Direct3D compiler;
    /// that fallback is supported and the build is still valid.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Invalid`] when the set declares no shaders or reuses an entry
    /// point name, [`Error::Read`] when a WGSL file cannot be read,
    /// [`Error::Translate`] when WGSL translation fails, [`Error::Compile`] when
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

        let mut generated = String::from("// @generated by gfx-shader-build. Do not edit.\n\n");

        for backend in &backends {
            self.emit_backend(&out_dir, *backend, &shaders, &mut generated)?;
        }

        let table_path = out_dir.join("shaders_bytes.rs");
        fs::write(&table_path, generated).map_err(|error| Error::Write {
            path: table_path,
            source: error,
        })
    }

    fn emit_backend(
        &self,
        out_dir: &Path,
        backend: Backend,
        shaders: &[&Shader],
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
        for shader in shaders {
            let source = shader.source()?;
            for (entry_point, stage) in &shader.entry_points {
                let (extension, include_macro, constructor, payload) =
                    self.compile_entry(backend, shader, &source, entry_point, *stage)?;

                let relative = format!(
                    "{ARTIFACT_DIR}/{}/{entry_point}.{extension}",
                    backend.slug()
                );
                write_artifact(&out_dir.join(&relative), &payload)?;

                let constant = format!("{prefix_upper}_{}", entry_point.to_uppercase());
                generated.push_str(&format!(
                    "const {constant}: ::gfx_core::EmbeddedShader = ::gfx_core::EmbeddedShader::{constructor}({include_macro}!(concat!(env!(\"OUT_DIR\"), \"/{relative}\")));\n"
                ));
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
        backend: Backend,
        shader: &Shader,
        source: &str,
        entry_point: &str,
        stage: ShaderStage,
    ) -> Result<(&'static str, &'static str, &'static str, Vec<u8>), Error> {
        match backend {
            Backend::Dx12 => {
                let hlsl = translate(source, entry_point, stage, shader)?;
                match compile_hlsl_to_dxbc(&hlsl, entry_point, stage) {
                    Some(bytecode) => Ok(("dxbc", "include_bytes", "DxBytecode", bytecode)),
                    None => match self.dx12_artifact_policy {
                        Dx12ArtifactPolicy::AllowRuntimeCompilation => {
                            println!(
                                "cargo::warning=DX12 shader `{}` entry point `{entry_point}` is embedded as HLSL because the build host cannot run D3DCompile; renderer startup requires an explicitly compiler-enabled DX12 backend",
                                shader.name
                            );
                            Ok(("hlsl", "include_str", "Hlsl", hlsl.into_bytes()))
                        }
                        Dx12ArtifactPolicy::RequireBytecode => Err(Error::Dx12BytecodeRequired {
                            shader: shader.name.clone(),
                            entry_point: entry_point.to_string(),
                        }),
                    },
                }
            }
            Backend::Vulkan => {
                let spirv = translate_vulkan(source, entry_point, stage, shader)?;
                Ok(("spv", "include_bytes", "SpirvBytes", spirv))
            }
            Backend::Metal => {
                let msl = translate_metal(source, entry_point, stage, shader)?;
                Ok(("metal", "include_str", "Msl", msl.into_bytes()))
            }
        }
    }
}

/// Backends an emit can produce artifacts for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Backend {
    Dx12,
    Vulkan,
    Metal,
}

impl Backend {
    fn slug(self) -> &'static str {
        match self {
            Self::Dx12 => "dx12",
            Self::Vulkan => "vulkan",
            Self::Metal => "metal",
        }
    }

    fn upper(self) -> &'static str {
        match self {
            Self::Dx12 => "DX12",
            Self::Vulkan => "VULKAN",
            Self::Metal => "METAL",
        }
    }

    /// Describes what this build embedded, for the caller's startup log.
    fn artifact_kind(self) -> &'static str {
        match self {
            Self::Dx12 if cfg!(target_os = "windows") => {
                "precompiled D3D bytecode embedded at build time"
            }
            Self::Dx12 => "HLSL source compiled by D3DCompile at renderer creation",
            Self::Vulkan => "SPIR-V words generated at build time",
            Self::Metal => "MSL source generated at build time",
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
    source: &str,
    entry_point: &str,
    stage: ShaderStage,
    shader: &Shader,
) -> Result<String, Error> {
    let binary = gfx_shader::compile_wgsl_to_hlsl(source, stage, entry_point).map_err(|error| {
        Error::Translate {
            shader: shader.name.clone(),
            entry_point: entry_point.to_string(),
            message: error.to_string(),
        }
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
    source: &str,
    entry_point: &str,
    stage: ShaderStage,
    shader: &Shader,
) -> Result<String, Error> {
    let binary = gfx_shader::compile_wgsl_to_msl_with_version(
        source,
        stage,
        entry_point,
        shader.msl_version,
    )
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

fn translate_vulkan(
    source: &str,
    entry_point: &str,
    stage: ShaderStage,
    shader: &Shader,
) -> Result<Vec<u8>, Error> {
    let binary =
        gfx_shader::compile_wgsl_to_spirv(source, stage, entry_point).map_err(|error| {
            Error::Translate {
                shader: shader.name.clone(),
                entry_point: entry_point.to_string(),
                message: error.to_string(),
            }
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
/// `D3DCompile` ships with Windows only, so a non-Windows host returns `None` and the
/// caller embeds HLSL for the runtime to compile instead. Build scripts execute on the
/// host, so this is a host check, not a target check.
#[cfg(target_os = "windows")]
fn compile_hlsl_to_dxbc(hlsl: &str, entry_point: &str, stage: ShaderStage) -> Option<Vec<u8>> {
    match gfx_dx12::compile_hlsl_to_dx_bytecode(hlsl, entry_point, stage) {
        Ok(bytecode) => Some(bytecode),
        Err(error) => {
            println!(
                "cargo::warning=Direct3D bytecode compilation failed for entry point `{entry_point}`: {error}. \
                 The same HLSL would fail at runtime, so the shader must be fixed before this build can succeed."
            );
            process::exit(1);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn compile_hlsl_to_dxbc(_hlsl: &str, _entry_point: &str, _stage: ShaderStage) -> Option<Vec<u8>> {
    None
}

fn write_artifact(path: &Path, payload: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| Error::Write {
            path: parent.to_path_buf(),
            source: error,
        })?;
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
    /// A strict DX12 shader set could not be compiled to bytecode on this build host.
    #[error("DX12 shader `{shader}` entry point `{entry_point}` requires build-time D3D bytecode, but this build host cannot run D3DCompile; build the Windows artifact on a Windows host or explicitly allow runtime compilation for a non-production tool")]
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
