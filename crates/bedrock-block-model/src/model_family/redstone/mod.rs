pub(super) mod devices;
pub(super) mod interactive;
pub(super) mod pistons;
pub(super) mod repeaters;
pub(super) mod wire;

use crate::model_family::ModelFamily;
use crate::model_family::shape::ModelShape;
use crate::state::BlockStateQuery;

pub(super) fn family_for(name: &str) -> Option<ModelFamily> {
    if name == "redstone_wire" {
        return Some(ModelFamily::RedstoneWire);
    }
    if let Some(f) = interactive::family_for(name) {
        return Some(f);
    }
    if let Some(f) = repeaters::family_for(name) {
        return Some(f);
    }
    if let Some(f) = pistons::family_for(name) {
        return Some(f);
    }
    if let Some(f) = devices::family_for(name) {
        return Some(f);
    }
    None
}

pub(crate) fn redstone_device_shape(name: &str, state: &BlockStateQuery) -> ModelShape {
    if name == "lever" {
        return interactive::lever_shape(state);
    }
    if name == "tripwire_hook" {
        return interactive::tripwire_hook_shape(state);
    }
    if name.contains("repeater") || name.contains("comparator") {
        return repeaters::shape(name, state);
    }
    if name.contains("piston") {
        return pistons::shape(state);
    }
    devices::shape(name, state)
}
