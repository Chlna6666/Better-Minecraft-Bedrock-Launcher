pub(super) mod containers;
pub(super) mod crafting;
pub(super) mod stations;

use crate::model_family::ModelFamily;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if let Some(f) = containers::family_for(name) {
        return Some(f);
    }
    if let Some(f) = stations::family_for(name) {
        return Some(f);
    }
    if let Some(f) = crafting::family_for(name) {
        return Some(f);
    }
    None
}
