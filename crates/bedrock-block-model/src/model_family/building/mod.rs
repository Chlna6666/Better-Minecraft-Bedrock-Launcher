pub(super) mod doors;
pub(super) mod fences;
pub(super) mod panes;
pub(super) mod slabs;
pub(super) mod stairs;
pub(super) mod trapdoors;
pub(super) mod walls;

use crate::model_family::ModelFamily;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if let Some(f) = fences::family_for(name) {
        return Some(f);
    }
    if let Some(f) = walls::family_for(name) {
        return Some(f);
    }
    if let Some(f) = panes::family_for(name) {
        return Some(f);
    }
    if let Some(f) = slabs::family_for(name) {
        return Some(f);
    }
    if let Some(f) = stairs::family_for(name) {
        return Some(f);
    }
    if let Some(f) = doors::family_for(name) {
        return Some(f);
    }
    if let Some(f) = trapdoors::family_for(name) {
        return Some(f);
    }
    None
}
