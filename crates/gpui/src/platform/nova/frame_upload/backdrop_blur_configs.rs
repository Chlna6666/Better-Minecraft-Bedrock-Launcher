use super::*;

const BLUR_DOWNSAMPLE_OFFSET: usize = 4;
const BLUR_LEVELS_OFFSET: usize = 8;
const BLUR_RADIUS_OFFSET: usize = 112;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::platform::nova) struct NovaBackdropBlurConfig {
    downsample: u8,
    levels: u8,
    offset_bits: u32,
}

impl NovaBackdropBlurConfig {
    fn new(downsample: u8, levels: u8, radius: f32) -> Self {
        let downsample = downsample.max(1);
        let levels = levels.clamp(1, MAX_BACKDROP_BLUR_LEVELS);
        Self {
            downsample,
            levels,
            offset_bits: backdrop_blur_offset(radius, downsample, levels).to_bits(),
        }
    }

    pub(in crate::platform::nova) fn downsample(self) -> u8 {
        self.downsample
    }

    pub(in crate::platform::nova) fn levels(self) -> usize {
        usize::from(self.levels)
    }

    pub(in crate::platform::nova) fn offset(self) -> f32 {
        f32::from_bits(self.offset_bits)
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
        self.legacy_downsample == other.max(&1).to_owned()
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
            let Some(end) = first.checked_add(count) else {
                continue;
            };
            for primitive_index in first..end {
                let Some(config) = self.backdrop_blur_config_at(primitive_index) else {
                    continue;
                };
                if seen.insert(config) {
                    configs.push(config);
                }
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

    pub(in crate::platform::nova) fn rebuild_backdrop_blur_passes(
        &mut self,
        configs: &[NovaBackdropBlurConfig],
    ) {
        self.backdrop_blur_passes.clear();
        self.backdrop_blur_passes
            .reserve(configs.len().saturating_mul(BACKDROP_BLUR_PASS_BYTES));
        for config in configs {
            write_backdrop_blur_pass(&mut self.backdrop_blur_passes, config.offset());
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
        let downsample = read_u32(record, BLUR_DOWNSAMPLE_OFFSET)?;
        let levels = read_u32(record, BLUR_LEVELS_OFFSET)?;
        let radius = f32::from_bits(read_u32(record, BLUR_RADIUS_OFFSET)?);
        Some(NovaBackdropBlurConfig::new(
            u8::try_from(downsample).ok()?.max(1),
            u8::try_from(levels).ok()?.clamp(1, MAX_BACKDROP_BLUR_LEVELS),
            radius,
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

    fn push_blur_record(upload: &mut NovaFrameUpload, downsample: u8, levels: u8, radius: f32) {
        let start = upload.backdrop_blurs.len();
        upload
            .backdrop_blurs
            .resize(start + PACKED_BACKDROP_BLUR_BYTES, 0);
        upload.backdrop_blurs[start + BLUR_DOWNSAMPLE_OFFSET..start + BLUR_DOWNSAMPLE_OFFSET + 4]
            .copy_from_slice(&u32::from(downsample).to_ne_bytes());
        upload.backdrop_blurs[start + BLUR_LEVELS_OFFSET..start + BLUR_LEVELS_OFFSET + 4]
            .copy_from_slice(&u32::from(levels).to_ne_bytes());
        upload.backdrop_blurs[start + BLUR_RADIUS_OFFSET..start + BLUR_RADIUS_OFFSET + 4]
            .copy_from_slice(&radius.to_bits().to_ne_bytes());
    }

    #[test]
    fn blur_configs_preserve_distinct_filter_strengths() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 2.0);
        push_blur_record(&mut upload, 2, 3, 18.0);
        upload
            .batches
            .push(NovaUploadedBatch::BackdropBlurs { first: 0, count: 2 });

        let configs = upload.backdrop_blur_configs();
        assert_eq!(configs.len(), 2);
        assert_ne!(configs[0], configs[1]);
    }

    #[test]
    fn blur_runs_split_mixed_styles_without_scene_rebatching() {
        let mut upload = NovaFrameUpload::default();
        push_blur_record(&mut upload, 1, 1, 2.0);
        push_blur_record(&mut upload, 1, 1, 2.0);
        push_blur_record(&mut upload, 2, 3, 18.0);
        let mut runs = Vec::new();
        upload.for_each_backdrop_blur_run(0, 3, |run| runs.push(run));

        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].count, 2);
        assert_eq!(runs[1].count, 1);
    }
}
