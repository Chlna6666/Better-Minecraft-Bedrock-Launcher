use super::*;

pub(super) struct DirectCompositeTarget {
    pub(super) texture_view: TextureViewId,
    /// Pass-layout resource set sampling the same retained texture. Nested backdrop filters inside
    /// the captured subtree use this when reconstructing their own source dependency chain.
    pub(super) source_resource_set: ResourceSetId,
}

impl BackdropBlurTargets {
    /// Reuses the final full-resolution Gaussian target as a retained compositor texture when an
    /// element capture has a zero-radius kernel. No copy is involved: child draw steps render
    /// directly into this texture and the existing composite resource set samples the same target.
    pub(super) fn direct_composite_target(
        &self,
        config: BackdropBlurConfig,
        frame_resource_index: usize,
    ) -> Option<DirectCompositeTarget> {
        if config.radius() > 0.0 {
            return None;
        }
        let variant = self
            .variants
            .iter()
            .find(|variant| variant.config.covers(config))?;
        let target = variant.levels.last()?;
        let source_resource_set = target
            .pass_resource_sets
            .get(frame_resource_index)
            .copied()?;
        Some(DirectCompositeTarget {
            texture_view: target.texture_view,
            source_resource_set,
        })
    }
}
