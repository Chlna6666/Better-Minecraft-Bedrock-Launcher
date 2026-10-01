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
    Dithering, FlatBlockCandidate, FlatImageInput, FlatImageOptions, flat_block_plan,
    flat_block_plan_with_progress,
};
pub use obj::{ObjMaterial, ObjModel, ObjTriangle, ObjVertex, load_obj_model};
pub use obj_voxel::{ObjFill, ObjVoxelOptions, voxelize_obj, voxelize_obj_with_progress};
pub use palette::default_block_candidates;
pub use relief::{
    MAX_RELIEF_BLOCK_VOLUME, ReliefImageFill, ReliefImageOptions, relief_block_plan_with_progress,
};

type Result<T> = std::result::Result<T, String>;

fn validation(message: impl Into<String>) -> String {
    message.into()
}
