use super::super::*;
use super::bind_groups::FrameResourceSets;
use super::buffers::FrameResourceBuffers;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct FrameResources {
    pub(in crate::platform::nova) buffers: FrameResourceBuffers,
    pub(in crate::platform::nova) bind_groups: FrameResourceSets,
    pub(in crate::platform::nova) path_resource_set: BindGroupId,
    pub(in crate::platform::nova) mono_sprite_resource_set: BindGroupId,
    pub(in crate::platform::nova) poly_sprite_resource_set: BindGroupId,
}
