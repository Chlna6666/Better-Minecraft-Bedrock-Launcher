use super::*;

const BLUR_ORDER_OFFSET: usize = 0;
const BLUR_DOWNSAMPLE_OFFSET: usize = 4;
const BLUR_LEVELS_OFFSET: usize = 8;
const BLUR_RECOMPUTE_OVERLAP_OFFSET: usize = 12;
const BLUR_BOUNDS_X_OFFSET: usize = 16;
const BLUR_BOUNDS_Y_OFFSET: usize = 20;
const BLUR_BOUNDS_WIDTH_OFFSET: usize = 24;
const BLUR_BOUNDS_HEIGHT_OFFSET: usize = 28;
const BLUR_RADIUS_OFFSET: usize = 112;

/// Renderer filter identity shared by compatible blur primitives in one source group.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::platform::nova) struct NovaBackdropBlurReuseKey {
    source_group: u32,
    downsample: u8,
    levels: u8,
    radius_bits: u32,
}

/// One renderer-side canonical blur configuration.
///
/// `member_first..=member_last` identifies the primitive span owned by one reusable GPU target.
/// Bounds deliberately do not participate in target identity, so an animated popover can move
/// without destroying and recreating its ping/pong textures every frame.
#[derive(Clone, Copy, Debug)]
pub(in crate::platform::nova) struct NovaBackdropBlurConfig {
    source_group: u32,
    member_first: u32,
    member_last: u32,
    order: u32,
    downsample: u8,
    levels: u8,
    radius_bits: u32,
    bounds_bits: [u32; 4],
    recompute_overlap: bool,
}

impl NovaBackdropBlurConfig {
    fn new(
        source_group: u32,
        member_index: u32,
        order: u32,
        downsample: u8,
        levels: u8,
        radius: f32,
        bounds: [f32; 4],
        recompute_overlap: bool,
    ) -> Self {
        let requested_downsample = downsample.max(1);
        let levels = levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS);
        let radius = if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        };
        // Medium-radius UI glass is sensitive to half-resolution reconstruction at thin edges.
        // Keep it full-resolution and reserve downsampling for genuinely large kernels where the
        // bandwidth reduction outweighs reconstruction loss. This also makes application-level
        // auto_quality() a hint rather than a hard quality downgrade for titlebars/popovers.
        let downsample = if radius <= 24.0 {
            1
        } else {
            requested_downsample
        };
        let bounds = bounds.map(|value| if value.is_finite() { value } else { 0.0 });
        Self {
            source_group,
            member_first: member_index,
            member_last: member_index,
            order,
            downsample,
            levels,
            radius_bits: radius.to_bits(),
            bounds_bits: bounds.map(f32::to_bits),
            recompute_overlap,
        }
    }

    pub(in crate::platform::nova) fn order(self) -> u32 {
        self.order
    }

    pub(in crate::platform::nova) fn downsample(self) -> u8 {
        self.downsample
    }

    pub(in crate::platform::nova) fn levels(self) -> usize {
        usize::from(self.levels)
    }

    /// Blur radius in source/device pixels. It is intentionally not quantized; 0.1px stays 0.1px.
    pub(in crate::platform::nova) fn radius(self) -> f32 {
        f32::from_bits(self.radius_bits)
    }

    /// Returns `[x, y, width, height]` in source/device pixels.
    pub(in crate::platform::nova) fn bounds(self) -> [f32; 4] {
        self.bounds_bits.map(f32::from_bits)
    }

    pub(in crate::platform::nova) fn reuse_key(self) -> NovaBackdropBlurReuseKey {
        NovaBackdropBlurReuseKey {
            source_group: self.source_group,
            downsample: self.downsample,
            levels: self.levels,
            radius_bits: self.radius_bits,
        }
    }

    /// Stable GPU-target identity. Animated bounds and draw order are intentionally excluded.
    pub(in crate::platform::nova) fn same_target_slot(self, other: Self) -> bool {
        self.reuse_key() == other.reuse_key()
            && self.member_first == other.member_first
            && self.member_last == other.member_last
            && self.recompute_overlap == other.recompute_overlap
    }

    /// Returns whether this canonical target owns the primitive represented by `other`.
    pub(in crate::platform::nova) fn owns(self, other: Self) -> bool {
        self.reuse_key() == other.reuse_key()
            && other.member_first == other.member_last
            && other.member_first >= self.member_first
            && other.member_first <= self.member_last
            && (!self.recompute_overlap || self.same_target_slot(other))
    }

    pub(in crate::platform::nova) fn covers(self, other: Self) -> bool {
        self.owns(other)
    }

    fn should_merge_with(self, other: Self) -> bool {
        !self.recompute_overlap
            && !other.recompute_overlap
            && self.reuse_key() == other.reuse_key()
            && self.member_last.saturating_add(1) == other.member_first
            && source_regions_overlap(self, other)
    }

    fn union_bounds(self, other: Self) -> Self {
        let left = self.bounds();
        let right = other.bounds();
        let x = left[0].min(right[0]);
        let y = left[1].min(right[1]);
        let right_edge = (left[0] + left[2]).max(right[0] + right[2]);
        let bottom_edge = (left[1] + left[3]).max(right[1] + right[3]);
        let mut merged = Self::new(
            self.source_group,
            self.member_first,
            self.order.min(other.order),
            self.downsample,
            self.levels,
            self.radius(),
            [x, y, (right_edge - x).max(0.0), (bottom_edge - y).max(0.0)],
            false,
        );
        merged.member_last = other.member_last.max(self.member_last);
        merged
    }

    pub(in crate::platform::nova) fn offset(self) -> f32 {
        self.radius()
    }
}

impl PartialEq for NovaBackdropBlurConfig {
    fn eq(&self, other: &Self) -> bool {
        self.same_target_slot(*other)
    }
}

impl Eq for NovaBackdropBlurConfig {}

#[cfg(test)]
pub(in crate::platform::nova) fn test_backdrop_blur_config(
    downsample: u8,
    levels: u8,
) -> NovaBackdropBlurConfig {
    NovaBackdropBlurConfig::new(
        0,
        0,
        0,
        downsample,
        levels,
        12.0,
        [0.0, 0.0, 64.0, 32.0],
        false,
    )
}

#[derive(Clone, Debug)]
pub(in crate::platform::nova) struct NovaBackdropBlurConfigSet {
    configs: Vec<NovaBackdropBlurConfig>,
    legacy_downsample: u8,
}

impl NovaBackdropBlurConfigSet {
    pub(in crate::platform::nova) fn new(
        configs: Vec<NovaBackdropBlurConfig>,
        legacy_downsample: u8,
    ) -> Self {
        Self {
            configs,
            legacy_downsample: legacy_downsample.max(1),
        }
    }

    pub(in crate::platform::nova) fn configs(&self) -> &[NovaBackdropBlurConfig] {
        &self.configs
    }

    pub(in crate::platform::nova) fn representative_downsample(&self) -> u8 {
        self.configs
            .iter()
            .map(|config| config.downsample())
            .min()
            .unwrap_or(self.legacy_downsample)
            .max(1)
    }
}

impl PartialEq for NovaBackdropBlurConfigSet {
    fn eq(&self, other: &Self) -> bool {
        self.legacy_downsample == other.legacy_downsample
            && self.configs.len() == other.configs.len()
            && self
                .configs
                .iter()
                .zip(other.configs())
                .all(|(left, right)| left.same_target_slot(*right))
    }
}

impl Eq for NovaBackdropBlurConfigSet {}

impl From<NovaBackdropBlurConfigSet> for usize {
    fn from(value: NovaBackdropBlurConfigSet) -> Self {
        usize::from(value.representative_downsample())
    }
}

impl PartialEq<u8> for NovaBackdropBlurConfigSet {
    fn eq(&self, other: &u8) -> bool {
        self.legacy_downsample == (*other).max(1)
    }
}

impl PartialEq<NovaBackdropBlurConfigSet> for u8 {
    fn eq(&self, other: &NovaBackdropBlurConfigSet) -> bool {
        other == self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::platform::nova) struct NovaBackdropBlurRun {
    pub(in crate::platform::nova) first: u32,
    pub(in crate::platform::nova) count: u32,
    pub(in crate::platform::nova) config: NovaBackdropBlurConfig,
}

impl NovaFrameUpload {
    pub(in crate::platform::nova) fn backdrop_blur_configs(&self) -> Vec<NovaBackdropBlurConfig> {
        let mut configs = Vec::new();
        for batch in &self.batches {
            let NovaUploadedBatch::BackdropBlurs { first, count } = *batch else {
                continue;
            };
            configs.extend(self.backdrop_blur_configs_for_range(first, count));
        }
        configs
    }

    pub(in crate::platform::nova) fn backdrop_blur_configs_for_range(
        &self,
        first: u32,
        count: u32,
    ) -> Vec<NovaBackdropBlurConfig> {
        let mut configs = Vec::<NovaBackdropBlurConfig>::new();
        let Some(end) = first.checked_add(count) else {
            return configs;
        };
        for primitive_index in first..end {
            let Some(config) = self.backdrop_blur_config_at(primitive_index, first) else {
                continue;
            };
            if let Some(previous) = configs.last_mut()
                && previous.should_merge_with(config)
            {
                *previous = previous.union_bounds(config);
            } else {
                configs.push(config);
            }
        }
        configs
    }

    pub(in crate::platform::nova) fn backdrop_blur_config_set(&self) -> NovaBackdropBlurConfigSet {
        NovaBackdropBlurConfigSet::new(
            self.backdrop_blur_configs(),
            self.backdrop_blur_downsample.max(1),
        )
    }

    /// Rebuilds the two axis pass records. Both source axes are still full resolution at the point
    /// where their convolution runs; X is downsampled by the first target and Y by the second.
    pub(in crate::platform::nova) fn rebuild_backdrop_blur_passes(
        &mut self,
        configs: &[NovaBackdropBlurConfig],
    ) {
        self.backdrop_blur_passes.clear();
        self.backdrop_blur_passes
            .reserve(configs.len().saturating_mul(BACKDROP_BLUR_PASS_BYTES * 2));
        for config in configs {
            let radius = config.radius().max(1.0 / 4096.0);
            write_backdrop_blur_pass(&mut self.backdrop_blur_passes, radius);
            write_backdrop_blur_pass(&mut self.backdrop_blur_passes, radius);
        }
    }

    pub(in crate::platform::nova) fn rebuild_backdrop_blur_passes_for_current_frame(&mut self) {
        let configs = self.backdrop_blur_configs();
        self.rebuild_backdrop_blur_passes(&configs);
    }

    pub(in crate::platform::nova) fn for_each_backdrop_blur_run(
        &self,
        first: u32,
        count: u32,
        mut visit: impl FnMut(NovaBackdropBlurRun),
    ) {
        let Some(end) = first.checked_add(count) else {
            return;
        };
        for primitive_index in first..end {
            let Some(config) = self.backdrop_blur_config_at(primitive_index, first) else {
                continue;
            };
            visit(NovaBackdropBlurRun {
                first: primitive_index,
                count: 1,
                config,
            });
        }
    }

    fn backdrop_blur_config_at(
        &self,
        primitive_index: u32,
        source_group: u32,
    ) -> Option<NovaBackdropBlurConfig> {
        let primitive_usize = usize::try_from(primitive_index).ok()?;
        let offset = primitive_usize.checked_mul(PACKED_BACKDROP_BLUR_BYTES)?;
        let record = self
            .backdrop_blurs
            .get(offset..offset.checked_add(PACKED_BACKDROP_BLUR_BYTES)?)?;
        let order = read_u32(record, BLUR_ORDER_OFFSET)?;
        let downsample = read_u32(record, BLUR_DOWNSAMPLE_OFFSET)?;
        let levels = read_u32(record, BLUR_LEVELS_OFFSET)?;
        let recompute_overlap = read_u32(record, BLUR_RECOMPUTE_OVERLAP_OFFSET)? != 0;
        let radius = f32::from_bits(read_u32(record, BLUR_RADIUS_OFFSET)?);
        let bounds = [
            f32::from_bits(read_u32(record, BLUR_BOUNDS_X_OFFSET)?),
            f32::from_bits(read_u32(record, BLUR_BOUNDS_Y_OFFSET)?),
            f32::from_bits(read_u32(record, BLUR_BOUNDS_WIDTH_OFFSET)?),
            f32::from_bits(read_u32(record, BLUR_BOUNDS_HEIGHT_OFFSET)?),
        ];
        Some(NovaBackdropBlurConfig::new(
            source_group,
            primitive_index,
            order,
            u8::try_from(downsample).ok()?.max(1),
            u8::try_from(levels).ok()?.clamp(1, MAX_BACKDROP_BLUR_LEVELS),
            radius,
            bounds,
            recompute_overlap,
        ))
    }
}

fn source_regions_overlap(left: NovaBackdropBlurConfig, right: NovaBackdropBlurConfig) -> bool {
    let support = left.radius().max(right.radius()).max(0.0) + 1.0;
    let left = dilated_bounds(left.bounds(), support);
    let right = dilated_bounds(right.bounds(), support);
    rects_overlap(left, right)
}

fn dilated_bounds(bounds: [f32; 4], amount: f32) -> [f32; 4] {
    [
        bounds[0] - amount,
        bounds[1] - amount,
        (bounds[2] + amount * 2.0).max(0.0),
        (bounds[3] + amount * 2.0).max(0.0),
    ]
}

fn rects_overlap(left: [f32; 4], right: [f32; 4]) -> bool {
    let left_right = left[0] + left[2];
    let left_bottom = left[1] + left[3];
    let right_right = right[0] + right[2];
    let right_bottom = right[1] + right[3];
    left[0] <= right_right
        && right[0] <= left_right
        && left[1] <= right_bottom
        && right[1] <= left_bottom
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let bytes: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_ne_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_blur_record(
        upload: &mut NovaFrameUpload,
        order: u32,
        downsample: u8,
        levels: u8,
        radius: f32,
        bounds: [f32; 4],
        recompute_overlap: bool,
    ) {
        let start = upload.backdrop_blurs.len();
        upload
            .backdrop_blurs
            .resize(start + PACKED_BACKDROP_BLUR_BYTES, 0);
        upload.backdrop_blurs[start + BLUR_ORDER_OFFSET..start + BLUR_ORDER_OFFSET + 4]
            .copy_from_slice(&order.to_ne_bytes());
        upload.backdrop_blurs[start + BLUR_DOWNSAMPLE_OFFSET..start + BLUR_DOWNSAMPLE_OFFSET + 4]
            .copy_from_slice(&u32::from(downsample).to_ne_bytes());
        upload.backdrop_blurs[start + BLUR_LEVELS_OFFSET..start + BLUR_LEVELS_OFFSET + 4]
            .copy_from_slice(&u32::from(levels).to_ne_bytes());
        upload.backdrop_blurs
            [start + BLUR_RECOMPUTE_OVERLAP_OFFSET..start + BLUR_RECOMPUTE_OVERLAP_OFFSET + 4]
            .copy_from_slice(&u32::from(recompute_overlap).to_ne_bytes());
        for (offset, value) in [
            (BLUR_BOUNDS_X_OFFSET, bounds[0]),
            (BLUR_BOUNDS_Y_OFFSET, bounds[1]),
            (BLUR_BOUNDS_WIDTH_OFFSET, bounds[2]),
            (BLUR_BOUNDS_HEIGHT_OFFSET, bounds[3]),
        ] {
            upload.backdrop_blurs[start + offset..start + offset + 4]
                .copy_from_slice(&value.to_bits().to_ne_bytes());
        }
        upload.backdrop_blurs[start + BLUR_RADIUS_OFFSET..start + BLUR_RADIUS_OFFSET + 4]
            .copy_from_slice(&radius.to_bits().to_ne_bytes());
    }

    #[test]
    fn overlapping_adjacent_blurs_share_one_filter_target_by_default() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 10, 1, 2, 18.0, [0.0, 0.0, 300.0, 80.0], false);
        push_blur_record(&mut upload, 10, 1, 2, 18.0, [200.0, 20.0, 300.0, 80.0], false);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        let configs = upload.backdrop_blur_configs();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].bounds(), [0.0, 0.0, 500.0, 100.0]);
    }

    #[test]
    fn disjoint_equal_blurs_keep_separate_filter_targets() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 10, 1, 2, 18.0, [0.0, 0.0, 100.0, 60.0], false);
        push_blur_record(&mut upload, 10, 1, 2, 18.0, [800.0, 400.0, 100.0, 60.0], false);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        assert_eq!(upload.backdrop_blur_configs().len(), 2);
    }

    #[test]
    fn overlap_recompute_parameter_keeps_independent_targets() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 10, 1, 2, 18.0, [0.0, 0.0, 300.0, 80.0], true);
        push_blur_record(&mut upload, 10, 1, 2, 18.0, [200.0, 20.0, 300.0, 80.0], true);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        assert_eq!(upload.backdrop_blur_configs().len(), 2);
    }

    #[test]
    fn target_set_equality_ignores_animated_bounds() {
        let left = NovaBackdropBlurConfig::new(
            0, 0, 10, 1, 2, 18.0, [0.0, 0.0, 300.0, 80.0], false,
        );
        let right = NovaBackdropBlurConfig::new(
            0, 0, 10, 1, 2, 18.0, [12.0, 4.0, 320.0, 80.0], false,
        );
        assert_eq!(left, right);
        assert_eq!(
            NovaBackdropBlurConfigSet::new(vec![left], 1),
            NovaBackdropBlurConfigSet::new(vec![right], 1)
        );
    }

    #[test]
    fn different_source_groups_never_share_targets() {
        let left = NovaBackdropBlurConfig::new(
            0, 0, 10, 1, 2, 18.0, [0.0, 0.0, 300.0, 80.0], false,
        );
        let right = NovaBackdropBlurConfig::new(
            2, 0, 10, 1, 2, 18.0, [0.0, 0.0, 300.0, 80.0], false,
        );
        assert_ne!(left.reuse_key(), right.reuse_key());
    }

    #[test]
    fn subpixel_blur_radius_is_not_quantized_to_one_pixel() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 1, 0.1, [8.0, 12.0, 320.0, 180.0], false);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });

        let configs = upload.backdrop_blur_configs();
        assert_eq!(configs.len(), 1);
        assert!((configs[0].radius() - 0.1).abs() <= f32::EPSILON);
    }

    #[test]
    fn gaussian_pass_parameters_are_precomputed_and_normalized() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 1, 0.1, [8.0, 12.0, 320.0, 180.0], false);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
        upload.rebuild_backdrop_blur_passes_for_current_frame();

        assert_eq!(
            upload.backdrop_blur_passes.len(),
            BACKDROP_BLUR_PASS_BYTES * 2
        );
        for pass in 0..2 {
            let base = pass * BACKDROP_BLUR_PASS_BYTES;
            let read_f32 = |offset: usize| {
                f32::from_ne_bytes(
                    upload.backdrop_blur_passes[base + offset..base + offset + 4]
                        .try_into()
                        .unwrap(),
                )
            };
            let offsets = [read_f32(0), read_f32(4), read_f32(8), read_f32(12)];
            let weights = [read_f32(16), read_f32(20), read_f32(24), read_f32(28)];
            let center = read_f32(32);
            assert!(offsets.windows(2).all(|pair| pair[0] < pair[1]));
            assert!(weights.iter().all(|weight| *weight > 0.0));
            let sum = center + weights.iter().sum::<f32>() * 2.0;
            assert!((sum - 1.0).abs() < 1e-5);
        }
    }
}
