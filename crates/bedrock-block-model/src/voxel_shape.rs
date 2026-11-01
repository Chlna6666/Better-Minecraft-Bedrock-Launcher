//! Versioned voxel volumes, independent of visual geometry and material opacity.

use std::{collections::BTreeMap, sync::OnceLock};

use serde::Deserialize;

use crate::{BlockFace, BlockModelError, Result};

/// A voxel box in normalized block coordinates. It carries no UV or material data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelBox {
    min: [f32; 3],
    max: [f32; 3],
}

impl VoxelBox {
    /// Inclusive minimum in block units (one block = 1).
    #[must_use]
    pub const fn min(&self) -> [f32; 3] {
        self.min
    }

    /// Maximum in block units.
    #[must_use]
    pub const fn max(&self) -> [f32; 3] {
        self.max
    }
}

/// Conservative 16 × 16 boundary coverage, with one bit per complete cell.
///
/// A covered bit proves geometric coverage only. A renderer must independently
/// establish state matching and material opacity before using it for occlusion.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct VoxelFaceCoverage([u64; 4]);

impl VoxelFaceCoverage {
    /// Whether every boundary cell is covered.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.0 == [u64::MAX; 4]
    }

    /// Whether no boundary cells are proven covered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0 == [0; 4]
    }

    /// Tests a cell; out-of-range coordinates return false.
    /// Axes are X/Z for up/down, X/Y for north/south, Z/Y for east/west.
    #[must_use]
    pub fn contains(&self, u: u8, v: u8) -> bool {
        if u >= 16 || v >= 16 {
            return false;
        }
        let bit = usize::from(v) * 16 + usize::from(u);
        self.0[bit / 64] & (1 << (bit % 64)) != 0
    }
}

/// A named volume, separate from textured [`crate::ModelShape`] geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct VoxelShape {
    format_version: String,
    boxes: Vec<VoxelBox>,
    coverage: [VoxelFaceCoverage; 6],
}

impl VoxelShape {
    /// Version of the source voxel-shape document, not the current game version.
    #[must_use]
    pub fn format_version(&self) -> &str {
        &self.format_version
    }

    /// Normalized source volumes, without inferred textures or state mappings.
    #[must_use]
    pub fn boxes(&self) -> &[VoxelBox] {
        &self.boxes
    }

    /// Returns precomputed boundary coverage for one axis face.
    ///
    /// Fractional cells covered only by a union of separate boxes may remain
    /// unmarked; this deliberately underestimates coverage rather than hiding holes.
    /// Material aliases (`Side`, `All`, `Default`) return `None`.
    #[must_use]
    pub fn boundary_coverage(&self, face: BlockFace) -> Option<&VoxelFaceCoverage> {
        let index = match face {
            BlockFace::Up => 0,
            BlockFace::Down => 1,
            BlockFace::North => 2,
            BlockFace::South => 3,
            BlockFace::East => 4,
            BlockFace::West => 5,
            _ => return None,
        };
        Some(&self.coverage[index])
    }
}

/// Identifier-indexed voxel volumes. No block-name or BlockState inference occurs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoxelShapeLibrary {
    shapes: BTreeMap<String, VoxelShape>,
}

impl VoxelShapeLibrary {
    /// Parses an array of decoded `minecraft:voxel_shape` documents (16-unit space).
    ///
    /// # Errors
    /// Rejects malformed documents, duplicate/empty identifiers, missing versions,
    /// and non-finite, degenerate or inverted boxes. Empty shapes are valid.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let documents: Vec<ShapeDocument> = serde_json::from_slice(bytes)
            .map_err(|error| BlockModelError::Message(format!("invalid voxel shapes: {error}")))?;
        let mut shapes = BTreeMap::new();
        for document in documents {
            let identifier = &document.description.identifier;
            if document.root != "minecraft:voxel_shape"
                || identifier.trim().is_empty()
                || document.format_version.trim().is_empty()
                || shapes.contains_key(identifier)
            {
                return Err(BlockModelError::Message(format!(
                    "invalid or duplicate voxel shape: {identifier}"
                )));
            }
            let shape = decode_shape(&document)?;
            shapes.insert(document.description.identifier, shape);
        }
        Ok(Self { shapes })
    }

    /// Looks up an exact shape identifier; no namespace or block-name fallback.
    #[must_use]
    pub fn get(&self, identifier: &str) -> Option<&VoxelShape> {
        self.shapes.get(identifier)
    }

    /// Number of named shapes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shapes.len()
    }

    /// Whether the library contains no shapes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Iterates named shapes in deterministic identifier order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &VoxelShape)> {
        self.shapes.iter().map(|(id, shape)| (id.as_str(), shape))
    }
}

/// The 218 decoded vanilla voxel volumes from format version 1.21.110.
/// This is a volume database, not a replacement for captured visual models.
#[must_use]
pub fn vanilla_voxel_shapes() -> &'static VoxelShapeLibrary {
    static LIBRARY: OnceLock<VoxelShapeLibrary> = OnceLock::new();
    LIBRARY.get_or_init(|| {
        VoxelShapeLibrary::from_json(include_bytes!("../generated/vanilla_voxel_shapes.json"))
            .expect("embedded voxel-shape documents must pass validation")
    })
}

#[derive(Deserialize)]
struct ShapeDocument {
    format_version: String,
    root: String,
    description: ShapeDescription,
    #[serde(rename = "minecraft:voxel_shape")]
    shape: ShapeBoxes,
}

#[derive(Deserialize)]
struct ShapeDescription {
    identifier: String,
}

#[derive(Deserialize)]
struct ShapeBoxes {
    boxes: Vec<ShapeBox>,
}

#[derive(Deserialize)]
struct ShapeBox {
    min: [f32; 3],
    max: [f32; 3],
}

fn decode_shape(document: &ShapeDocument) -> Result<VoxelShape> {
    let mut boxes = Vec::with_capacity(document.shape.boxes.len());
    for bounds in &document.shape.boxes {
        if (0..3).any(|axis| {
            !bounds.min[axis].is_finite()
                || !bounds.max[axis].is_finite()
                || bounds.min[axis] >= bounds.max[axis]
        }) {
            return Err(BlockModelError::Message(format!(
                "invalid voxel box in {}",
                document.description.identifier
            )));
        }
        boxes.push(VoxelBox {
            min: bounds.min.map(|value| value / 16.0),
            max: bounds.max.map(|value| value / 16.0),
        });
    }
    let coverage = [
        (1, true),
        (1, false),
        (2, false),
        (2, true),
        (0, true),
        (0, false),
    ]
    .map(|(axis, positive)| boundary_coverage(&boxes, axis, positive));
    Ok(VoxelShape {
        format_version: document.format_version.clone(),
        boxes,
        coverage,
    })
}

fn boundary_coverage(boxes: &[VoxelBox], axis: usize, positive: bool) -> VoxelFaceCoverage {
    let (u_axis, v_axis) = match axis {
        0 => (2, 1),
        1 => (0, 2),
        _ => (0, 1),
    };
    let mut coverage = VoxelFaceCoverage::default();
    for bounds in boxes {
        let touches = if positive {
            bounds.min[axis] < 1.0 && bounds.max[axis] >= 1.0
        } else {
            bounds.min[axis] <= 0.0 && bounds.max[axis] > 0.0
        };
        if !touches {
            continue;
        }
        for v in 0..16 {
            for u in 0..16 {
                let cell_inside = [(u_axis, u), (v_axis, v)].into_iter().all(|(a, cell)| {
                    bounds.min[a] <= cell as f32 / 16.0 && bounds.max[a] >= (cell + 1) as f32 / 16.0
                });
                if cell_inside {
                    let bit = v * 16 + u;
                    coverage.0[bit / 64] |= 1 << (bit % 64);
                }
            }
        }
    }
    coverage
}
