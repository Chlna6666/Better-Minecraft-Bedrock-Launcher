pub(super) mod campfires;
pub(super) mod copper_golem;
pub(super) mod furniture;
pub(super) mod lighting;
pub(super) mod objects;

use crate::model_family::ModelFamily;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if let Some(f) = objects::family_for(name) {
        return Some(f);
    }
    if copper_golem::family_for(name) {
        return Some(ModelFamily::CopperGolemStatue);
    }
    if let Some(f) = lighting::family_for(name) {
        return Some(f);
    }
    if let Some(f) = furniture::family_for(name) {
        return Some(f);
    }
    if let Some(f) = campfires::family_for(name) {
        return Some(f);
    }
    None
}
