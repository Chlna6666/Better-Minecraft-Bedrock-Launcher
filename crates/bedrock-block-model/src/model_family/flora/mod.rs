pub(super) mod crops;
pub(super) mod cross_plants;
pub(super) mod natural;
pub(super) mod special;
pub(super) mod vines;

use crate::model_family::ModelFamily;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if let Some(f) = special::family_for(name) {
        return Some(f);
    }
    if let Some(f) = natural::family_for(name) {
        return Some(f);
    }
    if let Some(f) = cross_plants::family_for(name) {
        return Some(f);
    }
    if let Some(f) = crops::family_for(name) {
        return Some(f);
    }
    if let Some(f) = vines::family_for(name) {
        return Some(f);
    }
    None
}
