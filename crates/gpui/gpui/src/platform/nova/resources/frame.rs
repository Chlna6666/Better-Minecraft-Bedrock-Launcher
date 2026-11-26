use super::super::*;
use super::bind_groups::FrameBindGroups;
use super::buffers::FrameResourceBuffers;

#[derive(Clone, Copy)]
pub(in crate::platform::nova) struct FrameResources {
    pub(in crate::platform::nova) buffers: FrameResourceBuffers,
    pub(in crate::platform::nova) bind_groups: FrameBindGroups,
    pub(in crate::platform::nova) path_bind_group: BindGroupId,
    pub(in crate::platform::nova) mono_sprite_bind_group: BindGroupId,
    pub(in crate::platform::nova) poly_sprite_bind_group: BindGroupId,
}
