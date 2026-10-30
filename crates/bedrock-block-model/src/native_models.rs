//! Embedded native block-model catalog generated from a Minecraft Bedrock installation.
//!
//! The catalog is produced by tools/generate_native_models.py --game-dir <version dir> and
//! embedded here so the 3D editor never has to scan Minecraft.Windows.exe at runtime.
//! It contains one entry for every block id in data/resource_packs/*/blocks.json; entries with
//! native geometry (voxel shapes / resource-pack block models) carry cuboids, the rest carry a
//! full-cube placeholder plus the block texture slots and the engine geometry-template ids.
use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::{BlockFace, BlockStateQuery, ModelCuboid, ModelShape, canonical_block_name_for_state};

const NATIVE_MODELS_JSON: &str = include_str!("../generated/native_models.json");

#[derive(Debug, Deserialize)]
struct NativeCatalogJson {
    schema: u32,
    game: NativeGameJson,
    blocks: BTreeMap<String, NativeBlockJson>,
}

#[derive(Debug, Deserialize)]
struct NativeGameJson {
    version: String,
    #[serde(default)]
    exe_sha256: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NativeBlockJson {
    #[serde(default)]
    pub textures: BTreeMap<String, String>,
    #[serde(default)]
    pub templates: Vec<String>,
    #[serde(default)]
    pub render_source: String,
    #[serde(default)]
    pub shape: Option<String>,
    #[serde(default)]
    pub occlusion_shape: Option<String>,
    #[serde(default)]
    pub cuboids: Vec<NativeCuboidJson>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NativeCuboidJson {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// One native block entry; cuboids may come from a native voxel shape.
#[derive(Debug, Clone)]
pub struct NativeBlockModel {
    pub block: String,
    pub render_source: NativeRenderSource,
    pub cuboids: Vec<ModelCuboid>,
    pub templates: Vec<String>,
    pub shape: Option<String>,
    pub occlusion_shape: Option<String>,
    pub textures: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeRenderSource {
    Cube,
    VoxelShape,
    PackGeometry,
    VoxelShapeOcclusion,
}

impl NativeRenderSource {
    fn from_str(value: &str) -> Self {
        match value {
            "voxel_shape" => Self::VoxelShape,
            "pack_geometry" => Self::PackGeometry,
            "voxel_shape_occlusion" => Self::VoxelShapeOcclusion,
            _ => Self::Cube,
        }
    }

    /// Whether this entry carries real render geometry (not the full-cube placeholder).
    #[must_use]
    pub const fn is_render_geometry(self) -> bool {
        matches!(self, Self::VoxelShape | Self::PackGeometry)
    }
}

struct Catalog {
    version: String,
    exe_sha256: Option<String>,
    blocks: BTreeMap<String, NativeBlockModel>,
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        let parsed: NativeCatalogJson = serde_json::from_str(NATIVE_MODELS_JSON)
            .expect("embedded native_models.json must parse");
        assert!(parsed.schema >= 1, "unsupported native model schema");
        let mut blocks = BTreeMap::new();
        for (block, entry) in parsed.blocks {
            let render_source = NativeRenderSource::from_str(&entry.render_source);
            let cuboids = entry
                .cuboids
                .iter()
                .map(|cuboid| native_cuboid(cuboid, &entry.textures))
                .collect();
            blocks.insert(
                block.clone(),
                NativeBlockModel {
                    block,
                    render_source,
                    cuboids,
                    templates: entry.templates,
                    shape: entry.shape,
                    occlusion_shape: entry.occlusion_shape,
                    textures: entry.textures,
                },
            );
        }
        Catalog {
            version: parsed.game.version,
            exe_sha256: parsed.game.exe_sha256,
            blocks,
        }
    })
}

fn native_cuboid(cuboid: &NativeCuboidJson, textures: &BTreeMap<String, String>) -> ModelCuboid {
    let mut model = ModelCuboid::new(cuboid.min, cuboid.max);
    for face in [
        BlockFace::Up,
        BlockFace::Down,
        BlockFace::North,
        BlockFace::South,
        BlockFace::East,
        BlockFace::West,
    ] {
        let Some(slot) = face_slot(face, textures) else {
            continue;
        };
        model.face_material_slots.insert(face, slot.to_owned());
    }
    model
}

fn face_slot(face: BlockFace, textures: &BTreeMap<String, String>) -> Option<&str> {
    let face_name = match face {
        BlockFace::Up => "up",
        BlockFace::Down => "down",
        BlockFace::North => "north",
        BlockFace::South => "south",
        BlockFace::East => "east",
        BlockFace::West => "west",
        _ => return None,
    };
    textures
        .get(face_name)
        .or_else(|| textures.get("side"))
        .or_else(|| textures.get("all"))
        .map(String::as_str)
}

fn source_preference() -> &'static str {
    static PREFERENCE: OnceLock<String> = OnceLock::new();
    PREFERENCE
        .get_or_init(|| {
            std::env::var("BMCBL_BLOCK_MODEL_SOURCE").unwrap_or_else(|_| "native".to_owned())
        })
        .as_str()
}

/// Whether the embedded native catalog should be consulted before the Java/family fallback.
#[must_use]
pub fn use_native_models() -> bool {
    !matches!(source_preference(), "legacy" | "java" | "family")
}

/// Whether the legacy Java Edition model database may be used as a fallback.
#[must_use]
pub fn use_java_models() -> bool {
    matches!(source_preference(), "java" | "legacy")
}

/// Whether native voxel occlusion shapes may be used as render geometry as a last resort.
#[must_use]
pub fn use_occlusion_shapes() -> bool {
    source_preference() == "native-all"
}

/// Game version recorded when the catalog was generated.
#[must_use]
pub fn native_models_version() -> &'static str {
    catalog().version.as_str()
}

/// Executable hash recorded when the catalog was generated.
#[must_use]
pub fn native_models_exe_sha256() -> Option<&'static str> {
    catalog().exe_sha256.as_deref()
}

/// Number of block entries in the embedded catalog.
#[must_use]
pub fn native_block_count() -> usize {
    catalog().blocks.len()
}

/// Look up one native block entry by block id (with or without the minecraft: prefix).
#[must_use]
pub fn native_block_model(block: &str) -> Option<&'static NativeBlockModel> {
    let normalized = block.strip_prefix("minecraft:").unwrap_or(block);
    catalog().blocks.get(normalized)
}

/// Look up the native entry for a resolved block state.
#[must_use]
pub fn native_block_model_for_state(state: &BlockStateQuery) -> Option<&'static NativeBlockModel> {
    let canonical = canonical_block_name_for_state(state);
    native_block_model(&canonical)
}

/// Resolve a native render shape for the block state when the catalog has real geometry.
///
/// Full-cube placeholders are not returned: the renderer already has a dedicated cube path, and
/// occlusion shapes are only used when BMCBL_BLOCK_MODEL_SOURCE=native-all.
#[must_use]
pub fn native_model_shape_for_block_state(state: &BlockStateQuery) -> Option<ModelShape> {
    let entry = native_block_model_for_state(state)?;
    let usable = entry.render_source.is_render_geometry()
        || (use_occlusion_shapes()
            && entry.render_source == NativeRenderSource::VoxelShapeOcclusion);
    if !usable || entry.cuboids.is_empty() {
        return None;
    }
    Some(ModelShape::from_cuboids(entry.cuboids.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_covers_every_block() {
        assert!(
            native_block_count() > 1000,
            "catalog should contain every blocks.json entry"
        );
        assert!(!native_models_version().is_empty());
    }

    #[test]
    fn fence_gate_keeps_engine_template_ids() {
        let entry = native_block_model("fence_gate").expect("fence_gate entry");
        assert!(
            entry
                .templates
                .iter()
                .any(|template| template == "minecraft:geometry.template_fence_gate")
        );
        assert_eq!(
            entry.occlusion_shape.as_deref(),
            Some("minecraft:fence_gate_occlusion")
        );
    }

    #[test]
    fn anvil_uses_native_voxel_shape() {
        let entry = native_block_model("anvil").expect("anvil entry");
        assert_eq!(entry.render_source, NativeRenderSource::VoxelShape);
        assert!(entry.cuboids.len() >= 5);
    }

    #[test]
    fn cube_placeholder_is_not_returned_as_render_geometry() {
        let entry = native_block_model("stone").expect("stone entry");
        assert_eq!(entry.render_source, NativeRenderSource::Cube);
    }
}
