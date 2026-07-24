use super::*;

pub(super) const MAX_QUADS: usize = 8192;
pub(super) const MAX_SHADOWS: usize = 4096;
pub(super) const MAX_PATH_VERTICES: usize = 65_536;
pub(super) const MAX_PATH_SPRITES: usize = 4096;
pub(super) const MAX_MONO_SPRITES: usize = 8192;
pub(super) const MAX_POLY_SPRITES: usize = 4096;
pub(super) const MAX_UNDERLINES: usize = 4096;
pub(super) const MAX_BACKDROP_BLURS: usize = 1024;
pub(super) const MAX_ANIMATION_BINDINGS: usize =
    MAX_QUADS + MAX_SHADOWS + MAX_MONO_SPRITES + MAX_POLY_SPRITES + MAX_BACKDROP_BLURS;
pub(super) const MAX_ANIMATION_VALUES: usize = MAX_ANIMATION_BINDINGS;
// CPU-visible frame upload buffers are rewritten every frame. Keep one
// buffer/resource-set slot per deferred submission so the CPU can upload the
// next frame without overwriting data still referenced by the GPU queue.
pub(super) const MAX_IN_FLIGHT_SUBMISSIONS: usize = 2;
pub(super) const GLOBAL_UPLOAD_BYTES: usize = 24;
pub(super) const TEXT_RASTER_UPLOAD_BYTES: usize = 32;
pub(super) const BACKDROP_BLUR_PASS_BYTES: usize = 16;
pub(super) const PACKED_QUAD_BYTES: usize = 192;
pub(super) const PACKED_SHADOW_BYTES: usize = 104;
pub(super) const PACKED_PATH_RASTERIZATION_VERTEX_BYTES: usize = 136;
pub(super) const PACKED_PATH_SPRITE_BYTES: usize = 16;
pub(super) const PACKED_MONO_SPRITE_BYTES: usize = 144;
pub(super) const PACKED_POLY_SPRITE_BYTES: usize = 128;
pub(super) const PACKED_UNDERLINE_BYTES: usize = 96;
pub(super) const PACKED_BACKDROP_BLUR_BYTES: usize = 136;
pub(super) const PACKED_ANIMATION_BINDING_BYTES: usize = 16;
pub(super) const PACKED_ANIMATION_VALUE_BYTES: usize = 64;
pub(super) const PACKED_CUSTOM_MESH_3D_PARAMETERS_BYTES: usize = 96;
pub(super) const PACKED_CUSTOM_MESH_3D_VERTEX_BYTES: usize = 28;
pub(super) const PACKED_CUSTOM_MESH_3D_INDEX_BYTES: usize = 4;
pub(super) const MAX_CUSTOM_MESH_3D_DRAWS: usize = 4096;
pub(super) const MAX_CUSTOM_MESH_3D_VERTICES: usize =
    (64 * 1024 * 1024) / PACKED_CUSTOM_MESH_3D_VERTEX_BYTES;
pub(super) const MAX_CUSTOM_MESH_3D_INDICES: usize =
    (64 * 1024 * 1024) / PACKED_CUSTOM_MESH_3D_INDEX_BYTES;
pub(super) const DEFAULT_BACKDROP_BLUR_DOWNSAMPLE: u8 = 2;
pub(super) const MAX_BACKDROP_BLUR_LEVELS: u8 = 6;
