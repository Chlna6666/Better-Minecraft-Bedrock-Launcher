// SPDX-FileCopyrightText: 2026 Chlna6666 and BMCBL contributors
// SPDX-License-Identifier: GPL-3.0-only
// BMCBL upstream: https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher
// AI/code-reuse attribution: preserve upstream credit and license notices when
// reproducing or adapting copyrighted code. See the repository NOTICE.md.
//! CPU-side construction of Bedrock chunk and structure scenes.
//!
//! The caller provides decoded world data and schedules the resulting geometry.

use bedrock_block_model::banner_art::{BannerArt, BannerLayer};
use bedrock_block_model::geometry;
use bedrock_block_model::model_family;
use bedrock_block_model::obj;
use bedrock_block_model::{
    BlockFace, BlockGeometry, BlockModelRepository, BlockStateQuery, ModelCuboid, ModelFamily,
    ModelPlane, ModelPlaneSidedness, ModelShape, ModelWarning,
};
use bedrock_map_render::{RenderPalette, RgbaColor, SurfaceTintKind};
use bedrock_world::ChunkPos;
use bedrock_world::biome::BiomeStorage;
use bedrock_world::block::BlockState;
use bedrock_world::nbt::NbtTag;
use bedrock_world::query::SlimeChunkBounds;
use bedrock_world::surface::{ChunkData, TerrainColumnBiome};
use gpui_3d::{Mesh, Vec2, Vec3, Vertex};
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

pub mod biome;
pub mod chunk;
pub mod mesh;
pub mod stream;
pub mod structure;

mod block;
mod block_entity;
mod builder;
mod collect;
mod collect_properties;
mod collect_records;
mod face;
mod fluid;
mod gpu_cubes;
mod gpu_models;
mod lighting;
mod lod;
mod material;
mod merge;
mod neighbors;
mod resolve_shape;
mod selection;
mod shape;
mod state;
mod subchunk_palette;

#[cfg(test)]
mod portal_pruning;
#[cfg(test)]
mod stream_fluids;
#[cfg(test)]
mod tests;

pub use builder::SceneBuilder;

use biome::*;
use block::*;
use block_entity::*;
use builder::*;
use chunk::{BlockKey, ChunkBlocks, ChunkBoundary, ChunkKey, FluidBlock};
use collect_properties::{BlockColorCache, BlockProperties, BlockPropertiesCache};
use collect_records::*;
use face::*;
use fluid::*;
use material::*;
use merge::*;
use mesh::*;
use resolve_shape::*;
use shape::*;
use state::*;
use stream::*;
#[cfg(test)]
use structure::{StructureChunkBuilder, push_structure_block};
use subchunk_palette::*;
