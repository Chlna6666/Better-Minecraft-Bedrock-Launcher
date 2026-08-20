use crate::model_family::ModelFamily;
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name.contains("campfire") {
        Some(ModelFamily::Campfire)
    } else {
        None
    }
}

pub(crate) fn shape(_state: &BlockStateQuery) -> ModelShape {
    ModelShape::from_cuboids([
        ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.25, 0.25]),
        ModelCuboid::new([0.0, 0.0, 0.75], [1.0, 0.25, 1.0]),
        ModelCuboid::new([0.0, 0.25, 0.0], [0.25, 0.5, 1.0]),
        ModelCuboid::new([0.75, 0.25, 0.0], [1.0, 0.5, 1.0]),
    ])
}
