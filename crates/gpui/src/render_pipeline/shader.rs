//! Runtime WGSL shader loading helpers.
//!
//! GPUI's built-in renderer owns the normal UI pipeline. These helpers are for
//! applications that render custom GPU content and want to keep shader source
//! outside the binary or generate it at runtime.
//!
//! The loader validates WGSL with `naga` so parse and validation diagnostics can
//! include the user-provided label or file path.
//!
//! ```no_run
//! # fn build_shader() -> Result<gpui::WgslShaderSource, gpui::WgslShaderError> {
//! let shader = gpui::compile_wgsl_shader_module_from_path("viewer.wgsl")?;
//! # Ok(shader)
//! # }
//! ```

use std::{fmt, fs, path::Path};

/// Errors returned while loading or compiling runtime WGSL shader sources.
#[derive(Debug)]
pub enum WgslShaderError {
    /// The WGSL source file could not be read.
    Read {
        /// Path of the shader source file.
        path: String,
        /// Underlying filesystem error.
        source: std::io::Error,
    },
    /// The WGSL source failed parsing or validation.
    Compile {
        /// User-provided source label or path.
        label: String,
        /// Formatted diagnostic with the source label embedded.
        diagnostic: String,
    },
}

impl fmt::Display for WgslShaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "failed to read WGSL shader {path}: {source}"),
            Self::Compile { label, diagnostic } => {
                write!(f, "failed to compile WGSL shader {label}:\n{diagnostic}")
            }
        }
    }
}

impl std::error::Error for WgslShaderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Compile { .. } => None,
        }
    }
}

/// A validated runtime WGSL source.
///
/// Use this type when the same shader source needs to be cross-compiled or
/// passed to a backend more than once, or when callers need access to the
/// validated source text for logging or pipeline setup. For one-shot loading, use
/// [`compile_wgsl_shader_module`] or [`compile_wgsl_shader_module_from_path`].
#[derive(Clone, Debug)]
pub struct WgslShaderSource {
    label: String,
    source: String,
}

impl WgslShaderSource {
    /// Creates a WGSL shader source from an in-memory string and validates it.
    ///
    /// `label` is included in diagnostics and should identify where the source
    /// came from, such as `"inline-material-shader"` or a generated filename.
    pub fn from_source(
        label: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<Self, WgslShaderError> {
        let label = label.into();
        let source = source.into();
        validate_wgsl(&label, &source)?;
        Ok(Self { label, source })
    }

    /// Reads a WGSL shader source file and validates it.
    ///
    /// File read errors and WGSL diagnostics both include the path in their
    /// display message.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, WgslShaderError> {
        let path = path.as_ref();
        let label = path.display().to_string();
        let source = fs::read_to_string(path).map_err(|source| WgslShaderError::Read {
            path: label.clone(),
            source,
        })?;
        Self::from_source(label, source)
    }

    /// User-visible source label, usually a path.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// WGSL source text.
    pub fn source(&self) -> &str {
        &self.source
    }
}

/// Loads and validates an in-memory WGSL shader source.
///
/// This is the shortest path for generated shader source or small examples.
/// The `label` appears in validation diagnostics.
pub fn compile_wgsl_shader_module(
    label: impl Into<String>,
    source: impl Into<String>,
) -> Result<WgslShaderSource, WgslShaderError> {
    WgslShaderSource::from_source(label, source)
}

/// Loads and validates a WGSL shader file.
///
/// This is intended for applications that ship editable `.wgsl` files next to
/// custom GPU rendering code. The file path is used as the shader label.
pub fn compile_wgsl_shader_module_from_path(
    path: impl AsRef<Path>,
) -> Result<WgslShaderSource, WgslShaderError> {
    WgslShaderSource::from_path(path)
}

fn validate_wgsl(label: &str, source: &str) -> Result<(), WgslShaderError> {
    let module =
        naga::front::wgsl::parse_str(source).map_err(|error| WgslShaderError::Compile {
            label: label.to_string(),
            diagnostic: error.emit_to_string_with_path(source, label),
        })?;

    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .map_err(|error| WgslShaderError::Compile {
            label: label.to_string(),
            diagnostic: error.emit_to_string_with_path(source, label),
        })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const VALID_SHADER: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(vertex_index) - 1);
    return vec4<f32>(x, 0.0, 0.0, 1.0);
}

@fragment
fn fs_main() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 0.0, 0.0, 1.0);
}
"#;

    #[test]
    fn in_memory_wgsl_shader_loads() {
        let shader = WgslShaderSource::from_source("inline-test", VALID_SHADER).unwrap();

        assert_eq!(shader.label(), "inline-test");
        assert!(shader.source().contains("vs_main"));
    }

    #[test]
    fn missing_wgsl_shader_file_reports_path() {
        let missing_path = std::env::temp_dir().join("gpui-missing-shader-file.wgsl");
        let error = WgslShaderSource::from_path(&missing_path).unwrap_err();
        let message = error.to_string();

        assert!(message.contains(&missing_path.display().to_string()));
    }

    #[test]
    fn invalid_wgsl_shader_reports_source_label() {
        let error = WgslShaderSource::from_source("broken-inline", "fn nope(").unwrap_err();
        let message = error.to_string();

        assert!(message.contains("broken-inline"));
    }

    #[test]
    fn file_wgsl_shader_loads() {
        let path = std::env::temp_dir().join("gpui-valid-shader-file.wgsl");
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(VALID_SHADER.as_bytes()).unwrap();

        let shader = WgslShaderSource::from_path(&path).unwrap();

        assert_eq!(shader.label(), path.display().to_string());
        let _ = fs::remove_file(path);
    }
}
