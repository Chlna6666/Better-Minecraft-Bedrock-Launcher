use super::*;

pub(super) struct DirectCompositeTarget {
    pub(super) texture_view: TextureViewId,
    /// Pass-layout bind group sampling the same retained texture. Nested backdrop filters inside
    /// the captured subtree use this when reconstructing their own source dependency chain.
    pub(super) source_bind_group: BindGroupId,
}

impl BackdropBlurTargets {
    /// Reuses the final full-resolution Gaussian target as a retained compositor texture when an
    /// element capture has a zero-radius kernel. No copy is involved: child draw steps render
    /// directly into this texture and the existing composite bind group samples the same target.
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
            .take(self.active_variant_count)
            .find(|variant| variant.config.covers(config))?;
        let target = variant.levels.last()?;
        Some(DirectCompositeTarget {
            texture_view: target.texture_view,
            source_bind_group: *target.pass_bind_groups.get(frame_resource_index)?,
        })
    }
}
