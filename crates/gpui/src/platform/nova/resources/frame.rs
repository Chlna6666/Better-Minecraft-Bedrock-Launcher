use super::super::*;
use super::buffers::FrameResourceBuffers;
use super::resource_sets::FrameResourceSets;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct FrameResources {
    pub(in crate::platform::nova) buffers: FrameResourceBuffers,
    pub(in crate::platform::nova) resource_sets: FrameResourceSets,
    pub(in crate::platform::nova) path_resource_set: ResourceSetId,
    pub(in crate::platform::nova) mono_sprite_resource_set: ResourceSetId,
    pub(in crate::platform::nova) poly_sprite_resource_set: ResourceSetId,
}
