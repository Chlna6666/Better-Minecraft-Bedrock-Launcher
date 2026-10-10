//! Native region relocation, ordered on the same owner command queue as rendering.
use super::*;
use objc2_metal::{MTLBlitCommandEncoder, MTLOrigin, MTLSize};

impl MetalDevice {
    pub(super) fn copy_texture_regions(&mut self, copies: &[gfx_core::TextureCopy]) -> Result<()> {
        if copies.is_empty() {
            return Ok(());
        }
        // Validate every native resource before creating/committing a command buffer. The current
        // texture-creation skeleton has no native resource; preserve Unavailable for that case.
        for copy in copies {
            let source = self.textures.get(copy.source)?;
            let destination = self.textures.get(copy.destination)?;
            copy.validate(&source.desc, &destination.desc)?;
            if source.resource.is_none() || destination.resource.is_none() {
                return Err(Error::Unavailable(
                    "Metal native texture creation is not enabled".into(),
                ));
            }
        }
        let commands = self
            .command_queue
            .commandBuffer()
            .ok_or_else(|| Error::Backend("Metal copy command buffer unavailable".into()))?;
        let blit = commands
            .blitCommandEncoder()
            .ok_or_else(|| Error::Backend("Metal blit encoder unavailable".into()))?;
        for copy in copies {
            let source = self
                .textures
                .get(copy.source)?
                .resource
                .as_ref()
                .expect("native copy source validated");
            let destination = self
                .textures
                .get(copy.destination)?
                .resource
                .as_ref()
                .expect("native copy destination validated");
            // SAFETY: The owner queue retains native resources until completion; distinct texture
            // formats, mip-zero rectangles and resource lifetimes were validated above.
            unsafe {
                blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                source, 0, 0, MTLOrigin { x: copy.source_origin.x as usize, y: copy.source_origin.y as usize, z: 0 },
                MTLSize { width: copy.size.width() as usize, height: copy.size.height() as usize, depth: 1 },
                destination, 0, 0, MTLOrigin { x: copy.destination_origin.x as usize, y: copy.destination_origin.y as usize, z: 0 });
            }
        }
        blit.endEncoding();
        commands.commit();
        Ok(())
    }
}
