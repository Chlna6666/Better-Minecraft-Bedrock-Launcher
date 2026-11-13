//! Pure image and OBJ conversion into local Bedrock block placement plans.
//!
//! The crate does not access world storage. Its plans remain local until a caller validates and
//! applies them through `bedrock-world`.

mod block_image;
mod obj;
mod obj_texture;
mod obj_voxel;
mod palette;
mod relief;

pub use block_image::{
    Dithering, FlatBlockCandidate, FlatImageInput, FlatImageOptions, plan_flat_image,
    plan_flat_image_tracked,
};
pub use obj::{ObjMaterial, ObjModel, ObjTriangle, ObjVertex, load_obj_model};
pub use obj_texture::{SourceTexture, load_obj_textures};
pub use obj_voxel::{
    ObjFill, ObjVoxelOptions, VoxelizeTexturesRequest, voxelize_obj, voxelize_textured,
};
pub use palette::default_block_candidates;
pub use relief::{
    MAX_RELIEF_BLOCK_VOLUME, ReliefImageFill, ReliefImageOptions, plan_relief_image,
};

type Result<T> = std::result::Result<T, String>;

fn validation(message: impl Into<String>) -> String {
    message.into()
}
