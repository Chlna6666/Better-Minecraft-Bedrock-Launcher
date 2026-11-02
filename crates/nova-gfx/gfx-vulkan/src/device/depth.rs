//! Explicit depth attachment/sample ownership between ordered graphics passes.
use super::*;

pub(super) struct DepthAccess {
    pub texture: TextureId,
    pub image: vk::Image,
    pub old_layout: vk::ImageLayout,
    pub new_layout: vk::ImageLayout,
}

impl VulkanDevice {
    pub(super) fn depth_accesses(
        &self,
        steps: &[RenderCommand],
        attachment: Option<DepthAttachment>,
    ) -> Result<Vec<DepthAccess>> {
        let target = attachment
            .map(|attachment| {
                self.texture_views
                    .get(attachment.target)
                    .map(|view| view.texture)
            })
            .transpose()?;
        let mut textures = Vec::new();
        for step in steps {
            for &group in step.bind_groups() {
                for &texture in &self.bind_groups.get(group)?.sampled_depth {
                    if Some(texture) == target {
                        return Err(Error::InvalidInput(
                            "a depth attachment cannot be sampled by its own pass".into(),
                        ));
                    }
                    if !textures.contains(&texture) {
                        textures.push(texture);
                    }
                }
            }
        }
        let mut accesses = Vec::new();
        for (texture, new_layout) in textures
            .into_iter()
            .map(|texture| (texture, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL))
            .chain(
                target.map(|texture| (texture, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)),
            )
        {
            let image = self.textures.get(texture)?;
            accesses.push(DepthAccess {
                texture,
                image: image.image,
                old_layout: image.layout,
                new_layout,
            });
        }
        Ok(accesses)
    }
}

pub(super) fn record_depth_accesses(
    device: &ash::Device,
    commands: vk::CommandBuffer,
    accesses: &[DepthAccess],
) {
    for access in accesses {
        if access.old_layout != access.new_layout {
            transition_image_layout_levels(
                device,
                commands,
                access.image,
                access.old_layout,
                access.new_layout,
                1,
                vk::ImageAspectFlags::DEPTH,
            );
        }
    }
}
