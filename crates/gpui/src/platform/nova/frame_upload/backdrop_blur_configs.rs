use super::*;

const BLUR_ORDER_OFFSET: usize = 0;
const BLUR_DOWNSAMPLE_OFFSET: usize = 4;
const BLUR_LEVELS_OFFSET: usize = 8;
const BLUR_BOUNDS_X_OFFSET: usize = 16;
const BLUR_BOUNDS_Y_OFFSET: usize = 20;
const BLUR_BOUNDS_WIDTH_OFFSET: usize = 24;
const BLUR_BOUNDS_HEIGHT_OFFSET: usize = 28;
const BLUR_RADIUS_OFFSET: usize = 112;

/// One renderer-side blur configuration.
///
/// Draw order and bounds deliberately participate in identity. Two glass surfaces with the same
/// radius do not have the same backdrop when either their draw position or sampling rectangle is
/// different. Treating those surfaces as one renderer target was the root cause of unrelated
/// background/titlebar/popover filters reusing each other's cached result.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::platform::nova) struct NovaBackdropBlurConfig {
    order: u32,
    downsample: u8,
    levels: u8,
    radius_bits: u32,
    bounds_bits: [u32; 4],
}

impl NovaBackdropBlurConfig {
    fn new(
        order: u32,
        downsample: u8,
        levels: u8,
        radius: f32,
        bounds: [f32; 4],
    ) -> Self {
        let downsample = downsample.max(1);
        let levels = levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS);
        let radius = if radius.is_finite() {
            radius.max(0.0)
        } else {
            0.0
        };
        let bounds = bounds.map(|value| if value.is_finite() { value } else { 0.0 });
        Self {
            order,
            downsample,
            levels,
            radius_bits: radius.to_bits(),
            bounds_bits: bounds.map(f32::to_bits),
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

    /// Compatibility accessor retained for diagnostics/tests that previously referred to the
    /// Dual-Kawase sample offset. The Gaussian pipeline now treats this as the real radius.
    pub(in crate::platform::nova) fn offset(self) -> f32 {
        self.radius()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
        let mut seen = FxHashSet::default();
        for batch in &self.batches {
            let NovaUploadedBatch::BackdropBlurs { first, count } = *batch else {
                continue;
            };
            for config in self.backdrop_blur_configs_for_range(first, count) {
                if seen.insert(config) {
                    configs.push(config);
                }
            }
        }
        configs
    }

    pub(in crate::platform::nova) fn backdrop_blur_configs_for_range(
        &self,
        first: u32,
        count: u32,
    ) -> Vec<NovaBackdropBlurConfig> {
        let mut configs = Vec::new();
        let mut seen = FxHashSet::default();
        let Some(end) = first.checked_add(count) else {
            return configs;
        };
        for primitive_index in first..end {
            let Some(config) = self.backdrop_blur_config_at(primitive_index) else {
                continue;
            };
            if seen.insert(config) {
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

    /// Rebuilds the pass parameter buffer for a separable Gaussian filter.
    ///
    /// Each configuration owns two consecutive pass records: horizontal first, vertical second.
    /// The horizontal pass works in source pixels; the vertical pass runs in the downsampled
    /// intermediate target and therefore uses radius/downsample. The shader derives direction from
    /// instance parity, which keeps this 16-byte record compact.
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
            write_backdrop_blur_pass(
                &mut self.backdrop_blur_passes,
                radius / f32::from(config.downsample().max(1)),
            );
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
        let Some(mut current_config) = self.backdrop_blur_config_at(first) else {
            return;
        };
        let mut run_first = first;
        for primitive_index in first.saturating_add(1)..end {
            let Some(config) = self.backdrop_blur_config_at(primitive_index) else {
                continue;
            };
            if config == current_config {
                continue;
            }
            visit(NovaBackdropBlurRun {
                first: run_first,
                count: primitive_index.saturating_sub(run_first),
                config: current_config,
            });
            run_first = primitive_index;
            current_config = config;
        }
        if run_first < end {
            visit(NovaBackdropBlurRun {
                first: run_first,
                count: end.saturating_sub(run_first),
                config: current_config,
            });
        }
    }

    fn backdrop_blur_config_at(&self, primitive_index: u32) -> Option<NovaBackdropBlurConfig> {
        let primitive_index = usize::try_from(primitive_index).ok()?;
        let offset = primitive_index.checked_mul(PACKED_BACKDROP_BLUR_BYTES)?;
        let record = self
            .backdrop_blurs
            .get(offset..offset.checked_add(PACKED_BACKDROP_BLUR_BYTES)?)?;
        let order = read_u32(record, BLUR_ORDER_OFFSET)?;
        let downsample = read_u32(record, BLUR_DOWNSAMPLE_OFFSET)?;
        let levels = read_u32(record, BLUR_LEVELS_OFFSET)?;
        let radius = f32::from_bits(read_u32(record, BLUR_RADIUS_OFFSET)?);
        let bounds = [
            f32::from_bits(read_u32(record, BLUR_BOUNDS_X_OFFSET)?),
            f32::from_bits(read_u32(record, BLUR_BOUNDS_Y_OFFSET)?),
            f32::from_bits(read_u32(record, BLUR_BOUNDS_WIDTH_OFFSET)?),
            f32::from_bits(read_u32(record, BLUR_BOUNDS_HEIGHT_OFFSET)?),
        ];
        Some(NovaBackdropBlurConfig::new(
            order,
            u8::try_from(downsample).ok()?.max(1),
            u8::try_from(levels).ok()?.clamp(1, MAX_BACKDROP_BLUR_LEVELS),
            radius,
            bounds,
        ))
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let bytes: [u8; 4] = bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?;
    Some(u32::from_ne_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn push_blur_record_with_bounds(
        upload: &mut NovaFrameUpload,
        order: u32,
        downsample: u8,
        levels: u8,
        radius: f32,
        bounds: [f32; 4],
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

    fn push_blur_record(
        upload: &mut NovaFrameUpload,
        order: u32,
        downsample: u8,
        levels: u8,
        radius: f32,
    ) {
        push_blur_record_with_bounds(
            upload,
            order,
            downsample,
            levels,
            radius,
            [8.0, 12.0, 320.0, 180.0],
        );
    }

    #[test]
    fn blur_configs_preserve_distinct_filter_strengths() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 1, 2.0);
        push_blur_record(&mut upload, 2, 2, 3, 18.0);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        let configs = upload.backdrop_blur_configs();
        assert_eq!(configs.len(), 2);
        assert_ne!(configs[0], configs[1]);
    }

    #[test]
    fn identical_radius_at_different_draw_orders_is_isolated() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 10, 1, 1, 18.0);
        push_blur_record(&mut upload, 20, 1, 1, 18.0);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        let configs = upload.backdrop_blur_configs();
        assert_eq!(configs.len(), 2);
        assert_ne!(configs[0].order(), configs[1].order());
    }

    #[test]
    fn identical_style_at_different_bounds_is_isolated() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record_with_bounds(&mut upload, 10, 1, 2, 18.0, [0.0, 0.0, 300.0, 60.0]);
        push_blur_record_with_bounds(
            &mut upload,
            10,
            1,
            2,
            18.0,
            [800.0, 80.0, 360.0, 520.0],
        );
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        let configs = upload.backdrop_blur_configs();
        assert_eq!(configs.len(), 2);
        assert_ne!(configs[0].bounds(), configs[1].bounds());
    }

    #[test]
    fn subpixel_blur_radius_is_not_quantized_to_one_pixel() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 1, 0.1);
        push_blur_record(&mut upload, 2, 1, 1, 1.0);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        let configs = upload.backdrop_blur_configs();
        assert_eq!(configs.len(), 2);
        assert!((configs[0].radius() - 0.1).abs() <= f32::EPSILON);
        assert!((configs[1].radius() - 1.0).abs() <= f32::EPSILON);
    }

    #[test]
    fn gaussian_pass_parameters_keep_subpixel_radius() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 1, 0.1);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 1 });
        upload.rebuild_backdrop_blur_passes_for_current_frame();

        assert_eq!(
            upload.backdrop_blur_passes.len(),
            BACKDROP_BLUR_PASS_BYTES * 2
        );
        let horizontal =
            f32::from_ne_bytes(upload.backdrop_blur_passes[0..4].try_into().unwrap());
        let vertical = f32::from_ne_bytes(
            upload.backdrop_blur_passes
                [BACKDROP_BLUR_PASS_BYTES..BACKDROP_BLUR_PASS_BYTES + 4]
                .try_into()
                .unwrap(),
        );
        assert!((horizontal - 0.1).abs() <= f32::EPSILON);
        assert!((vertical - 0.1).abs() <= f32::EPSILON);
    }

    #[test]
    fn blur_runs_split_mixed_styles_without_scene_rebatching() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 1, 2.0);
        push_blur_record(&mut upload, 1, 1, 1, 2.0);
        push_blur_record(&mut upload, 2, 2, 3, 18.0);
        let mut runs = Vec::new();
        upload.for_each_backdrop_blur_run(0, 3, |run| runs.push(run));

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].count, 2);
        assert_eq!(runs[1].count, 1);
    }
}
