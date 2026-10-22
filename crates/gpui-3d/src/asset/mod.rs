//! glTF 2.0 / GLB asset import.
//!
//! The importer converts a glTF document into gpui-3d scene data: meshes, metallic-roughness
//! materials, decoded textures with mip chains, node hierarchies, punctual lights, cameras,
//! and absolute-time transform animation tracks. It never touches the file system outside an
//! explicitly supplied base directory and enforces caller-configured limits for file size,
//! buffers, nodes, vertices, and texture dimensions.
//!
//! Supported core features: GLB and JSON documents, external/data-URI/bufferView buffers,
//! interleaved and un-indexed primitives, sparse accessors, POSITION, NORMAL, TANGENT,
//! `TEXCOORD_0` and `COLOR_0` attributes, material alpha modes, normal and occlusion maps, node TRS
//! and matrix transforms,
//! triangle primitives, cameras, and STEP/LINEAR/CUBICSPLINE transform animations.
//!
//! Feature status: skins and morph targets are parsed only far enough to report a warning;
//! their geometry renders in the bind pose without deformation. Compiled texture extensions
//! (KTX2/Basis) and mesh compression extensions are not decoded. Unknown extensions listed in
//! extensionsRequired fail the import; optional unknown extensions are reported as warnings.

mod environment;
mod gltf;
mod image;
#[cfg(feature = "asset-import")]
mod unified;
#[cfg(feature = "asset-import")]
pub use unified::{ImportError, ImportLimits, ImportedScene, import_scene_path};

pub use environment::{EnvironmentError, EnvironmentMap};
pub use gltf::GltfError;

use crate::{Camera, Scene, TextureAsset, TextureAssetId, TransformTrack};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Limits and optional decoding behavior for one import.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Each switch independently controls import behavior (texture decoding, mip generation, downscaling, external resources) and callers set them individually."
)]
pub struct GltfLoadOptions {
    /// Maximum document size in bytes (JSON or GLB file).
    pub max_file_bytes: usize,
    /// Maximum total decoded buffer bytes, including the GLB binary chunk.
    pub max_buffer_bytes: usize,
    /// Maximum number of glTF nodes visited.
    pub max_nodes: usize,
    /// Maximum number of vertices in one primitive.
    pub max_vertices: usize,
    /// Maximum width or height of a decoded texture.
    ///
    /// Larger images are downscaled to fit when [`GltfLoadOptions::downscale_textures`] is set,
    /// and rejected with [`GltfError::Limit`] otherwise.
    pub max_texture_dimension: u32,
    /// Downscale oversized images to [`GltfLoadOptions::max_texture_dimension`] instead of
    /// failing the import. Downscaling happens before mip generation, so it reduces GPU memory
    /// proportionally to the squared scale factor.
    pub downscale_textures: bool,
    /// Decode embedded and external images into GPU-ready textures.
    pub decode_textures: bool,
    /// Generate a complete box-filtered mip chain for decoded textures.
    pub generate_mipmaps: bool,
    /// Allow loading external buffers and images relative to this directory.
    pub allow_external_resources: bool,
}

impl Default for GltfLoadOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: 64 * 1024 * 1024,
            max_buffer_bytes: 256 * 1024 * 1024,
            max_nodes: 65_536,
            max_vertices: 4_000_000,
            max_texture_dimension: 2048,
            downscale_textures: true,
            decode_textures: true,
            generate_mipmaps: true,
            allow_external_resources: true,
        }
    }
}

/// Counts describing the imported document.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GltfStatistics {
    /// Scene nodes created, including per-primitive child nodes.
    pub nodes: usize,
    /// Source glTF meshes that produced at least one primitive.
    pub meshes: usize,
    /// Imported primitives.
    pub primitives: usize,
    /// Materials created.
    pub materials: usize,
    /// Decoded textures.
    pub textures: usize,
    /// Imported triangles.
    pub triangles: u64,
    /// Animation clips created.
    pub animations: usize,
}

/// One animation clip converted to absolute-time transform tracks.
#[derive(Clone, Debug)]
pub struct GltfAnimation {
    /// Clip name, or a generated name when the document omits it.
    pub name: String,
    /// Last keyframe time across every channel.
    pub duration: Duration,
    /// Per-node transform tracks for this clip.
    pub tracks: Vec<TransformTrack>,
}

/// Perspective or orthographic camera declared by the document.
#[derive(Clone, Debug)]
pub struct GltfCamera {
    /// Camera node name, or a generated name.
    pub name: String,
    /// Camera pose and projection converted from the node world transform.
    pub camera: Camera,
}

/// Imported glTF document expressed as gpui-3d data.
#[derive(Debug)]
pub struct GltfScene {
    /// Scene graph with node transforms, meshes, materials, and lights.
    pub scene: Scene,
    /// Decoded textures keyed by generated asset id.
    pub textures: HashMap<TextureAssetId, Arc<TextureAsset>>,
    /// Animation clips from the document.
    pub animations: Vec<GltfAnimation>,
    /// Cameras declared by nodes in the document.
    pub cameras: Vec<GltfCamera>,
    /// Non-fatal feature notes such as skipped morph targets or unsupported material extensions.
    pub warnings: Vec<String>,
    /// Imported element counts.
    pub statistics: GltfStatistics,
}

impl GltfScene {
    /// Returns the named animation clip.
    #[must_use]
    pub fn animation(&self, name: &str) -> Option<&GltfAnimation> {
        self.animations
            .iter()
            .find(|animation| animation.name == name)
    }

    /// Returns the duration of the named animation clip.
    #[must_use]
    pub fn animation_duration(&self, name: &str) -> Option<Duration> {
        self.animation(name).map(|animation| animation.duration)
    }
}

/// Parses a JSON glTF or binary GLB document.
///
/// # Errors
///
/// Returns [`GltfError`] for malformed JSON/GLB containers, unsupported required extensions,
/// out-of-range indices or offsets, invalid accessor data, unsupported primitive modes, and
/// configured limit violations.
pub fn load_bytes(
    bytes: &[u8],
    options: &GltfLoadOptions,
    base_dir: Option<&Path>,
) -> Result<GltfScene, GltfError> {
    if bytes.len() > options.max_file_bytes {
        return Err(GltfError::Limit(format!(
            "document is {} bytes, exceeding the {} byte limit",
            bytes.len(),
            options.max_file_bytes
        )));
    }
    gltf::load(bytes, options, base_dir)
}

/// Reads and parses a glTF or GLB file.
///
/// External buffers and images resolve relative to the file's parent directory when
/// [`GltfLoadOptions::allow_external_resources`] is set.
///
/// # Errors
///
/// Returns [`GltfError::Io`] when the file cannot be read, or any error from [`load_bytes`].
pub fn load_path(
    path: impl AsRef<Path>,
    options: &GltfLoadOptions,
) -> Result<GltfScene, GltfError> {
    let path: PathBuf = path.as_ref().to_path_buf();
    let metadata = std::fs::metadata(&path)
        .map_err(|error| GltfError::Io(format!("read {}: {error}", path.display())))?;
    if metadata.len() > options.max_file_bytes as u64 {
        return Err(GltfError::Limit(format!(
            "{} is {} bytes, exceeding the {} byte limit",
            path.display(),
            metadata.len(),
            options.max_file_bytes
        )));
    }
    let bytes = std::fs::read(&path)
        .map_err(|error| GltfError::Io(format!("read {}: {error}", path.display())))?;
    let base_dir = path.parent().map(Path::to_path_buf);
    load_bytes(&bytes, options, base_dir.as_deref())
}
