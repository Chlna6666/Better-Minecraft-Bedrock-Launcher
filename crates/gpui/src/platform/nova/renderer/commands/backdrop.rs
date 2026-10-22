//! Keep backdrop input damage separate from the retained color target's output damage.
use super::*;
use crate::platform::nova::frame_upload::ColorContentRange;

pub(super) struct BarrierRefresh {
    pub(super) batch_index: usize,
    pub(super) configs: Vec<BackdropBlurConfig>,
    /// Unknown config bookkeeping cannot prove a barrier safe to split across.
    pub(super) configs_known: bool,
    pub(super) damage: DirtyRegion,
    pub(super) force_full: bool,
}

impl NovaRenderer {
    pub(super) fn color_barrier_refreshes(
        &self,
        range: ColorContentRange,
        damage: &DirtyRegion,
        force_full: bool,
    ) -> Vec<BarrierRefresh> {
        let is_root = self.frame_upload.root_color.map(|root| root.index) == Some(range.index);
        direct_backdrop_barriers(&self.frame_upload, range.content_start, range.content_end)
            .into_iter()
            .filter_map(|batch_index| {
                let UploadedBatch::BackdropBlurs { first, count } =
                    self.frame_upload.batches[batch_index]
                else {
                    return None;
                };
                let configs = self.frame_upload.blur_configs_in_range(first, count);
                // Root uses window coordinates, matching the per-order source plan.
                // Isolated captures can rebase/transform their inputs, so keep their
                // existing conservative local damage path.
                let (source_full, source_damage) = if is_root {
                    backdrop_config_damage(
                        &self.command_scratch.backdrop_blur_damage_plan,
                        &configs,
                    )
                } else {
                    (false, damage.clone())
                };
                let force_full = force_full || source_full;
                let refreshed_configs =
                    blur_refresh_configs(&configs, self.active_size, &source_damage, force_full);
                Some(BarrierRefresh {
                    batch_index,
                    configs_known: !configs.is_empty(),
                    configs: refreshed_configs,
                    damage: source_damage,
                    force_full,
                })
            })
            .collect()
    }
}

pub(super) fn backdrop_config_damage(
    plan: &crate::BackdropBlurDamagePlan,
    configs: &[BackdropBlurConfig],
) -> (bool, DirtyRegion) {
    let Some(first_order) = configs
        .iter()
        .map(|config| *config.order_range().start())
        .min()
    else {
        return (false, DirtyRegion::empty());
    };
    let last_order = configs
        .iter()
        .map(|config| *config.order_range().end())
        .max()
        .unwrap_or(first_order);
    let (full_refresh, damage) = plan.order_source_damage(first_order, last_order);
    let mut region = DirtyRegion::empty();
    for bounds in damage {
        region.push(bounds);
    }
    (full_refresh, region)
}

pub(super) fn blur_refresh_configs(
    configs: &[BackdropBlurConfig],
    drawable_size: DrawableSize,
    dirty_region: &DirtyRegion,
    force_full: bool,
) -> Vec<BackdropBlurConfig> {
    if force_full {
        return configs.to_vec();
    }
    configs
        .iter()
        .copied()
        .filter(|config| blur_damage_scissors(*config, drawable_size, dirty_region).is_some())
        .collect()
}
