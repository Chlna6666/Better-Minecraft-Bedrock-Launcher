use crate::model_family::ModelFamily;
use crate::model_family::shape::{ModelCuboid, ModelShape};
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if matches!(name, "dispenser" | "dropper" | "observer" | "target")
        || name.starts_with("daylight_detector")
    {
        Some(ModelFamily::RedstoneDevice)
    } else {
        None
    }
}

pub(crate) fn shape(name: &str, _state: &BlockStateQuery) -> ModelShape {
    if name.starts_with("daylight_detector") {
        return ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 0.375, 1.0])]);
    }
    ModelShape::from_cuboids([ModelCuboid::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])])
}
