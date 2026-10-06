use super::*;

#[derive(Default)]
pub(super) struct DrawStepScratch {
    draw_steps: Vec<RenderStepDescriptor>,
    draw_step_cache: Vec<DrawStepCacheEntry>,
    prepared_draw_step_slot: Option<usize>,
    pub(super) draw_step_cache_hit: bool,
    path_mask_steps: Vec<DrawStepDescriptor>,
    path_mask_cache: Vec<PathMaskCacheEntry>,
    prepared_path_mask_slot: Option<usize>,
    pub(super) path_mask_cache_hit: bool,
    pub(super) backdrop_blur_passes: Vec<BackdropBlurRenderPass>,
    pub(super) backdrop_blur_damage_region: DirtyRegion,
    pub(super) backdrop_blur_damage_plan: crate::BackdropBlurDamagePlan,
    pub(super) force_full_backdrop_blur_refresh: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct DrawStepCacheKey {
    pub(super) scene_revision: u64,
    pub(super) dynamic_frame_id: Option<u64>,
    pub(super) size: DrawableSize,
    pub(super) frame_resource_index: usize,
    pub(super) atlas_texture_generation: Option<u64>,
    pub(super) atlas_texture_count: usize,
    pub(super) premultiplied_alpha: bool,
}

#[derive(Default)]
struct DrawStepCacheEntry {
    key: Option<DrawStepCacheKey>,
    steps: Vec<RenderStepDescriptor>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct PathMaskCacheKey {
    pub(super) scene_revision: u64,
    pub(super) path_rasterization_resource_set: ResourceSetId,
}

#[derive(Default)]
struct PathMaskCacheEntry {
    key: Option<PathMaskCacheKey>,
    steps: Vec<DrawStepDescriptor>,
}

impl DrawStepScratch {
    pub(super) fn invalidate_draw_steps(&mut self) {
        for cache in &mut self.draw_step_cache {
            cache.key = None;
        }
        self.draw_step_cache_hit = false;
    }

    #[cfg(test)]
    fn path_capacity_bytes(&self) -> usize {
        (self.path_mask_steps.capacity()
            + self
                .path_mask_cache
                .iter()
                .map(|cache| cache.steps.capacity())
                .sum::<usize>())
            * std::mem::size_of::<DrawStepDescriptor>()
    }

    pub(super) fn path_steps(&self) -> &[DrawStepDescriptor] {
        match self.prepared_path_mask_slot {
            Some(slot) => &self.path_mask_cache[slot].steps,
            None => &self.path_mask_steps,
        }
    }

    pub(super) fn prepare_path_steps(
        &mut self,
        key: PathMaskCacheKey,
        slot: usize,
        slot_count: usize,
        build: impl FnOnce(&mut Vec<DrawStepDescriptor>),
    ) {
        self.path_mask_cache_hit = false;
        self.path_mask_cache
            .resize_with(slot_count, Default::default);
        self.prepared_path_mask_slot =
            (key.scene_revision != 0 && slot < slot_count).then_some(slot);
        if let Some(slot) = self.prepared_path_mask_slot {
            self.path_mask_steps.clear();
            let cache = &mut self.path_mask_cache[slot];
            if cache.key == Some(key) {
                self.path_mask_cache_hit = true;
                return;
            }
            // The resource set belongs to this frame slot. Retain that slot's descriptors
            // directly, and publish the key only after the complete list has been built.
            cache.key = None;
            build(&mut cache.steps);
            cache.key = Some(key);
        } else {
            build(&mut self.path_mask_steps);
        }
    }

    pub(super) fn steps(&self) -> &[RenderStepDescriptor] {
        match self.prepared_draw_step_slot {
            Some(slot) => &self.draw_step_cache[slot].steps,
            None => &self.draw_steps,
        }
    }

    pub(super) fn prepare_steps(
        &mut self,
        key: DrawStepCacheKey,
        slot_count: usize,
        build: impl FnOnce(&mut Vec<RenderStepDescriptor>),
    ) {
        self.draw_step_cache_hit = false;
        self.draw_step_cache
            .resize_with(slot_count, Default::default);
        self.prepared_draw_step_slot = (key.scene_revision != 0
            && key.frame_resource_index < slot_count)
            .then_some(key.frame_resource_index);
        if let Some(slot) = self.prepared_draw_step_slot {
            self.draw_steps.clear();
            let cached = &mut self.draw_step_cache[slot];
            if cached.key == Some(key) {
                self.draw_step_cache_hit = true;
                return;
            }
            // Build directly into the slot's retained allocation. Present borrows this vector
            // on both misses and hits, so static scenes never copy the descriptor list.
            cached.key = None;
            build(&mut cached.steps);
            cached.key = Some(key);
        } else {
            build(&mut self.draw_steps);
        }
    }

    pub(super) fn trim_retained_capacity(&mut self, level: GpuiMemoryTrimLevel) {
        self.prepared_draw_step_slot = None;
        self.prepared_path_mask_slot = None;
        let multiplier = match level {
            GpuiMemoryTrimLevel::Light => 16,
            GpuiMemoryTrimLevel::Moderate => 8,
            GpuiMemoryTrimLevel::Aggressive => 1,
        };
        trim_vec_capacity(&mut self.draw_steps, 64, multiplier);
        trim_vec_capacity(&mut self.path_mask_steps, 32, multiplier);
        trim_vec_capacity(&mut self.backdrop_blur_passes, 16, multiplier);
        for cache in &mut self.path_mask_cache {
            if cache.steps.capacity() > 32usize.saturating_mul(multiplier.max(1)) {
                *cache = PathMaskCacheEntry::default();
            }
        }
        for cache in &mut self.draw_step_cache {
            if cache.steps.capacity() > 64usize.saturating_mul(multiplier.max(1)) {
                *cache = DrawStepCacheEntry::default();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_mask_cache_reuses_rotating_slots_without_rebuilding() {
        let mut scratch = DrawStepScratch::default();
        for slot in 0..2 {
            scratch.prepare_path_steps(path_key(slot, 1), slot, 2, |steps| {
                steps.clear();
                steps.push(path_step(slot as u32 + 1));
            });
        }
        for slot in [0, 1, 0] {
            scratch.prepare_path_steps(path_key(slot, 1), slot, 2, |steps| {
                steps.clear();
                steps.push(path_step(slot as u32 + 1));
            });
            assert!(
                scratch.path_mask_cache_hit,
                "rotating frame slot rebuilt its path mask"
            );
            assert_eq!(scratch.path_steps()[0].instance_count, slot as u32 + 1);
            assert_eq!(
                scratch.path_steps().as_ptr(),
                scratch.path_mask_cache[slot].steps.as_ptr()
            );
        }
        assert_eq!(scratch.path_mask_steps.capacity(), 0);
    }

    #[test]
    fn path_mask_cache_invalidates_resource_sets_and_revisions() {
        let mut scratch = DrawStepScratch::default();
        let mut key = path_key(0, 1);
        scratch.prepare_path_steps(key, 0, 2, |steps| steps.push(path_step(1)));
        key.path_rasterization_resource_set = ResourceSetId::new(99);
        for revision in [1, 2, 0] {
            key.scene_revision = revision;
            scratch.prepare_path_steps(key, 0, 2, |steps| {
                steps.clear();
                steps.push(path_step(revision as u32 + 2));
            });
            assert!(!scratch.path_mask_cache_hit);
            assert_eq!(scratch.path_steps()[0].instance_count, revision as u32 + 2);
        }
        key.scene_revision = 2;
        scratch.prepare_path_steps(key, 0, 2, |_| {
            panic!("uncached scene replaced retained cache")
        });
        assert!(scratch.path_mask_cache_hit);
        assert_eq!(scratch.path_steps()[0].instance_count, 4);
    }

    #[test]
    fn path_mask_cache_rebuilds_after_trim_and_slot_resize() {
        let mut scratch = DrawStepScratch::default();
        scratch.prepare_path_steps(path_key(1, 1), 1, 2, |steps| {
            steps.reserve(2048);
            steps.push(path_step(1));
        });
        scratch.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);
        assert_eq!(scratch.path_capacity_bytes(), 0);
        assert!(scratch.path_steps().is_empty());
        for (slot, slots) in [(1, 2), (0, 1), (1, 2)] {
            scratch.prepare_path_steps(path_key(slot, 1), slot, slots, |steps| {
                steps.push(path_step(2));
            });
            assert!(!scratch.path_mask_cache_hit);
            assert_eq!(scratch.path_steps()[0].instance_count, 2);
        }
        scratch.prepare_path_steps(path_key(1, 1), 1, 0, |steps| steps.push(path_step(3)));
        assert!(!scratch.path_mask_cache_hit);
        assert_eq!(scratch.path_steps()[0].instance_count, 3);
    }

    #[test]
    fn cached_steps_release_unused_uncached_scratch_lengths() {
        let mut scratch = DrawStepScratch::default();
        let mut draw_key = draw_step_key(0);
        draw_key.scene_revision = 0;
        scratch.prepare_steps(draw_key, 2, |steps| steps.resize(2048, draw_step(1)));
        scratch.prepare_path_steps(path_key(0, 0), 0, 2, |steps| {
            steps.resize(2048, path_step(1))
        });
        draw_key.scene_revision = 1;
        scratch.prepare_steps(draw_key, 2, |steps| steps.push(draw_step(2)));
        scratch.prepare_path_steps(path_key(0, 1), 0, 2, |steps| steps.push(path_step(2)));
        assert!(scratch.draw_steps.is_empty());
        assert!(scratch.path_mask_steps.is_empty());
        scratch.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);
        assert!(scratch.draw_steps.capacity() <= 64);
        assert!(scratch.path_mask_steps.capacity() <= 32);
    }
    fn draw_step_key(slot: usize) -> DrawStepCacheKey {
        DrawStepCacheKey {
            scene_revision: 1,
            dynamic_frame_id: None,
            size: DrawableSize {
                width: 800,
                height: 600,
            },
            frame_resource_index: slot,
            atlas_texture_generation: Some(1),
            atlas_texture_count: 1,
            premultiplied_alpha: false,
        }
    }

    fn draw_step(instances: u32) -> RenderStepDescriptor {
        RenderStepDescriptor::Draw(DrawStepDescriptor {
            pipeline: RenderPipelineId::new(1),
            resource_sets: gfx_core::resource_set_list_empty(),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: instances,
            first_instance: 0,
            scissor: None,
        })
    }

    #[test]
    fn draw_step_cache_borrows_each_frame_slot_without_rebuilding() {
        let mut scratch = DrawStepScratch::default();
        for slot in 0..2 {
            scratch.prepare_steps(draw_step_key(slot), 2, |steps| {
                steps.push(draw_step(slot as u32 + 1));
            });
            assert!(!scratch.draw_step_cache_hit);
        }
        for slot in [0, 1, 0] {
            scratch.prepare_steps(draw_step_key(slot), 2, |_| panic!("cache hit rebuilt"));
            assert!(scratch.draw_step_cache_hit);
            assert_eq!(scratch.steps(), &[draw_step(slot as u32 + 1)]);
            assert_eq!(
                scratch.steps().as_ptr(),
                scratch.draw_step_cache[slot].steps.as_ptr()
            );
        }
        assert_eq!(scratch.draw_steps.capacity(), 0);
    }

    #[test]
    fn draw_step_cache_rebuilds_changed_resources_and_uncached_scenes() {
        let mut scratch = DrawStepScratch::default();
        let mut key = draw_step_key(0);
        scratch.prepare_steps(key, 2, |steps| steps.push(draw_step(1)));
        key.atlas_texture_generation = Some(2);
        scratch.prepare_steps(key, 2, |steps| {
            steps.clear();
            steps.push(draw_step(2));
        });
        assert!(!scratch.draw_step_cache_hit);
        assert_eq!(scratch.steps(), &[draw_step(2)]);
        key.scene_revision = 0;
        for instances in [3, 4] {
            scratch.prepare_steps(key, 2, |steps| {
                steps.clear();
                steps.push(draw_step(instances));
            });
            assert!(!scratch.draw_step_cache_hit);
            assert_eq!(scratch.steps(), &[draw_step(instances)]);
        }
        key.scene_revision = 1;
        scratch.prepare_steps(key, 2, |_| panic!("uncached frame replaced retained slot"));
        assert_eq!(scratch.steps(), &[draw_step(2)]);
    }

    #[test]
    fn draw_step_cache_rebuilds_after_trim_and_slot_count_change() {
        let mut scratch = DrawStepScratch::default();
        scratch.prepare_steps(draw_step_key(1), 2, |steps| {
            steps.reserve(2048);
            steps.push(draw_step(1));
        });
        scratch.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);
        assert!(scratch.steps().is_empty());
        scratch.prepare_steps(draw_step_key(1), 2, |steps| steps.push(draw_step(2)));
        assert!(!scratch.draw_step_cache_hit);
        assert_eq!(scratch.steps(), &[draw_step(2)]);
        scratch.prepare_steps(draw_step_key(0), 1, |steps| steps.push(draw_step(3)));
        assert_eq!(scratch.steps(), &[draw_step(3)]);
        scratch.prepare_steps(draw_step_key(1), 2, |steps| steps.push(draw_step(4)));
        assert!(!scratch.draw_step_cache_hit);
        assert_eq!(scratch.steps(), &[draw_step(4)]);
    }

    #[test]
    fn draw_step_scratch_aggressive_trim_shrinks_retained_capacity() {
        let mut scratch = DrawStepScratch::default();
        scratch.draw_steps.reserve(2048);
        scratch.path_mask_steps.reserve(1024);

        scratch.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);

        assert!(scratch.draw_steps.capacity() <= 64);
        assert!(scratch.path_mask_steps.capacity() <= 32);
    }

    fn path_key(slot: usize, revision: u64) -> PathMaskCacheKey {
        PathMaskCacheKey {
            scene_revision: revision,
            path_rasterization_resource_set: ResourceSetId::new(slot as u64 + 1),
        }
    }

    fn path_step(instances: u32) -> DrawStepDescriptor {
        DrawStepDescriptor {
            pipeline: RenderPipelineId::new(1),
            resource_sets: gfx_core::resource_set_list_empty(),
            vertex_count: 4,
            first_vertex: 0,
            instance_count: instances,
            first_instance: 0,
            scissor: None,
        }
    }

    #[test]
    #[ignore = "release CPU benchmark; run explicitly with --ignored --nocapture"]
    fn path_mask_cache_benchmark() {
        for (name, slots, count, invalidates) in [
            ("small_single_slot", 1, 32, false),
            ("small_rotating_slots", 2, 32, false),
            ("large_single_slot", 1, 1024, false),
            ("large_rotating_slots", 2, 1024, false),
            ("three_slot_stress", 3, 1024, false),
            ("large_invalidated", 2, 1024, true),
        ] {
            let fixture = vec![path_step(1); count];
            let mut scratch = DrawStepScratch::default();
            for slot in 0..slots {
                scratch.prepare_path_steps(path_key(slot, 1), slot, slots, |steps| {
                    steps.clone_from(&fixture);
                });
            }
            let (batches, builds, hits) =
                measure_path_cache(&mut scratch, &fixture, slots, invalidates);
            println!(
                "{}",
                serde_json::json!({
                    "benchmark": "path_mask_cache", "case": name,
                    "slots": slots, "descriptors": count,
                    "iterations_per_batch": 10_000, "batch_nanoseconds": batches,
                    "builds": builds, "hits": hits,
                    "retained_descriptor_capacity_bytes": scratch.path_capacity_bytes(),
                })
            );
        }
    }

    fn measure_path_cache(
        scratch: &mut DrawStepScratch,
        fixture: &[DrawStepDescriptor],
        slots: usize,
        invalidates: bool,
    ) -> (Vec<u64>, usize, usize) {
        const SAMPLES: usize = 30;
        const ITERATIONS: usize = 10_000;
        let mut builds = 0;
        let mut hits = 0;
        let mut batches = Vec::with_capacity(SAMPLES);
        for sample in 0..SAMPLES {
            let started = std::time::Instant::now();
            for iteration in 0..ITERATIONS {
                let slot = iteration % slots;
                let revision = if invalidates {
                    (sample * ITERATIONS + iteration + 2) as u64
                } else {
                    1
                };
                scratch.prepare_path_steps(path_key(slot, revision), slot, slots, |steps| {
                    builds += 1;
                    steps.clear();
                    steps.extend_from_slice(std::hint::black_box(fixture));
                });
                hits += usize::from(scratch.path_mask_cache_hit);
                std::hint::black_box(scratch.path_steps());
            }
            batches.push(started.elapsed().as_nanos() as u64);
        }
        (batches, builds, hits)
    }
}
