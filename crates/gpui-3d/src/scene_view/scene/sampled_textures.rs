//! Per-SceneView descriptor slots. Texture ownership remains in the existing
//! resident TextureAssetId map; only removed textures are held here until the
//! immutable descriptor snapshots and their readers have retired.
use super::*;
use gfx_core::{SampledTextureSlots, SubmissionId};

#[derive(Clone, Default, PartialEq, Eq)]
struct Readers {
    submissions: HashSet<SubmissionId>,
    untracked: bool,
}

impl Readers {
    fn submitted(&mut self, submission: Option<SubmissionId>) {
        if let Some(submission) = submission {
            self.submissions.insert(submission);
        } else { self.untracked = true; }
    }
    fn accepted(&mut self, submission: SubmissionId) {
        if std::mem::take(&mut self.untracked) { self.submissions.insert(submission); }
    }
    fn completed(&mut self, submission: SubmissionId) { self.submissions.remove(&submission); }
    fn idle(&mut self) { self.submissions.clear(); self.untracked = false; }
    fn ready(&self) -> bool { !self.untracked && self.submissions.is_empty() }
    fn include(&mut self, other: &Self) {
        self.submissions.extend(other.submissions.iter().copied());
        self.untracked |= other.untracked;
    }
}

struct RetiredSnapshot {
    snapshot: BindGroupId,
    generation: u64,
    readers: Readers,
}

/// Images can be shared across descriptor generations and legacy draws.
/// Their lifetime is not the lifetime of only the latest replaced set.
struct RetiredTextures {
    textures: Vec<TextureResources>,
    readers: Readers,
}

pub(super) struct SceneSampledTextures {
    layout: BindGroupLayoutId,
    slots: SampledTextureSlots,
    unmapped: usize,
    readers: Readers,
    legacy_readers: Readers,
    snapshot_readers: Readers,
    generation: u64,
    // First immutable generation that can contain each allocated image.
    // An image is never moved between slots while it is live.
    first_generation: HashMap<gfx_core::TextureViewId, u64>,
    // Command publication precedes queue acceptance. Pin the exact table
    // identities during that interval, including a legacy capture (None).
    queued: HashSet<Option<BindGroupId>>,
    pending: Vec<TextureResources>,
    cleared: Vec<TextureResources>,
    retired: Vec<RetiredSnapshot>,
    retired_textures: Vec<RetiredTextures>,
    published: bool,
    remap_pending: bool,
}

impl SceneSampledTextures {
    pub(super) fn new(layout: BindGroupLayoutId, capacity: u32, fallback: gfx_core::TextureViewId) -> gpui::Result<Self> {
        Ok(Self {
            layout, slots: SampledTextureSlots::new(capacity, fallback)?,
            unmapped: 0, readers: Readers::default(), legacy_readers: Readers::default(),
            snapshot_readers: Readers::default(), generation: 0, first_generation: HashMap::new(),
            queued: HashSet::new(),
            pending: Vec::new(), cleared: Vec::new(), retired: Vec::new(),
            retired_textures: Vec::new(), published: false, remap_pending: false,
        })
    }

    pub(super) fn allocate(&mut self, texture: &mut TextureResources) {
        texture.sampled_slot = self.slots.allocate(texture.view).ok();
        if texture.sampled_slot.is_none() { self.unmapped += 1; }
        else { self.first_generation.insert(texture.view, self.generation.saturating_add(1)); }
    }

    pub(super) fn retire(&mut self, device: &mut dyn ExtensionDevice, texture: TextureResources) -> gpui::Result<()> {
        if texture.sampled_slot.is_some() {
            self.pending.push(texture);
        } else {
            self.unmapped -= 1;
            destroy_texture(device, texture)?;
        }
        Ok(())
    }

    /// Re-enter bindless only if all currently unmapped images fit. Do not
    /// scan a large legacy scene repeatedly for a partial, unusable mapping.
    pub(super) fn needs_remap(&self) -> bool {
        self.remap_pending || (self.unmapped != 0 && self.unmapped <= self.slots.available_slots())
    }

    pub(super) fn refill(&mut self, textures: &mut HashMap<TextureAssetId, TextureResources>) -> bool {
        if self.unmapped != 0 && self.unmapped <= self.slots.available_slots() {
            for texture in textures.values_mut().filter(|texture| texture.sampled_slot.is_none()) {
                texture.sampled_slot = self.slots.allocate(texture.view).ok();
                if texture.sampled_slot.is_some() {
                    self.unmapped -= 1;
                    self.first_generation.insert(texture.view, self.generation.saturating_add(1));
                    self.remap_pending = true;
                }
            }
        }
        self.remap_pending
    }

    pub(super) fn draw_rows_committed(&mut self) {
        self.remap_pending = false;
    }

    // Planning may select a remapped snapshot before encoding its rows;
    // SceneResources commits it only after all draws and bindings succeed.
    pub(super) fn selected(&self, layout: Option<BindGroupLayoutId>) -> Option<BindGroupId> {
        if self.published && self.unmapped == 0 && layout == Some(self.layout) {
            self.slots.current_snapshot()
        } else { None }
    }

    pub(super) fn matches_layout(&self, layout: Option<BindGroupLayoutId>) -> bool {
        layout == Some(self.layout)
    }

    pub(super) fn has_completed_retirement(&self) -> bool {
        self.retired.iter().any(|old| old.readers.ready() && !self.queued.contains(&Some(old.snapshot)))
            || self.retired_textures.iter().any(|old| old.readers.ready())
    }

    pub(super) fn has_retired(&self) -> bool {
        !self.pending.is_empty() || !self.cleared.is_empty() || !self.retired.is_empty()
            || !self.retired_textures.is_empty()
    }

    pub(super) fn queue_capture(&mut self, snapshot: Option<BindGroupId>) {
        self.queued.insert(snapshot);
    }

    /// Stamp the actual descriptor identities encoded in the drained batch.
    /// The current scene may already have published another snapshot.
    pub(super) fn capture_submitted(&mut self, submission: Option<SubmissionId>, fallback: Option<BindGroupId>) {
        if self.queued.is_empty() {
            self.submitted(submission, fallback);
        } else {
            for snapshot in std::mem::take(&mut self.queued) {
                self.submitted(submission, snapshot);
            }
        }
    }

    /// Legacy draws can read any live material image, so remain conservative.
    /// Sampled draws cannot read a texture allocated after their immutable
    /// descriptor generation was published.
    fn image_readers(&self, view: gfx_core::TextureViewId) -> Readers {
        let Some(&first) = self.first_generation.get(&view) else { return self.readers.clone(); };
        let mut readers = self.legacy_readers.clone();
        if self.generation >= first { readers.include(&self.snapshot_readers); }
        for old in &self.retired {
            if old.generation >= first { readers.include(&old.readers); }
        }
        readers
    }

    pub(super) fn submitted(&mut self, submission: Option<SubmissionId>, snapshot: Option<BindGroupId>) {
        self.readers.submitted(submission);
        if let Some(snapshot) = snapshot {
            if self.slots.current_snapshot() == Some(snapshot) {
                self.snapshot_readers.submitted(submission);
            } else if let Some(old) = self.retired.iter_mut().find(|old| old.snapshot == snapshot) {
                old.readers.submitted(submission);
            } else {
                debug_assert!(false, "submitted unknown sampled snapshot");
            }
        } else {
            self.legacy_readers.submitted(submission);
        }
    }

    pub(super) fn accepted(&mut self, submission: SubmissionId) {
        // Only the ordered Graphics owner may seal unfenced reads.
        self.readers.accepted(submission);
        self.legacy_readers.accepted(submission);
        self.snapshot_readers.accepted(submission);
        for old in &mut self.retired { old.readers.accepted(submission); }
        for old in &mut self.retired_textures { old.readers.accepted(submission); }
    }

    pub(super) fn completed(&mut self, submission: SubmissionId) {
        self.readers.completed(submission);
        self.legacy_readers.completed(submission);
        self.snapshot_readers.completed(submission);
        self.slots.complete(submission);
        for old in &mut self.retired { old.readers.completed(submission); }
        for old in &mut self.retired_textures { old.readers.completed(submission); }
    }

    pub(super) fn idle(&mut self) {
        for reader in self.readers.submissions.drain() { self.slots.complete(reader); }
        self.readers.idle();
        self.legacy_readers.idle();
        self.snapshot_readers.idle();
        for old in &mut self.retired { old.readers.idle(); }
        for old in &mut self.retired_textures { old.readers.idle(); }
    }

    pub(super) fn release_completed(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let mut i = 0;
        while i < self.retired.len() {
            if !self.retired[i].readers.ready() || self.queued.contains(&Some(self.retired[i].snapshot)) {
                i += 1;
                continue;
            }
            device.destroy_bind_group(self.retired[i].snapshot)?;
            self.retired.swap_remove(i);
        }
        // Destroy all ready descriptor sets before their retired images.
        let mut i = 0;
        while i < self.retired_textures.len() {
            if !self.retired_textures[i].readers.ready() { i += 1; continue; }
            let old = self.retired_textures.swap_remove(i);
            for texture in old.textures { destroy_texture(device, texture)?; }
        }
        Ok(())
    }

    pub(super) fn needs_publication(&self) -> bool {
        !self.published || (!self.readers.untracked && self.queued.is_empty() && !self.pending.is_empty())
            || self.slots.dirty_changes().next().is_some()
    }

    pub(super) fn publish(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        // Preserve every removed image and its slot until untracked native
        // work receives an actual ordered submission fence (or idle proof).
        if !self.cleared.is_empty() && (self.readers.untracked || !self.queued.is_empty()) {
            return Ok(());
        }
        if !self.readers.untracked && self.queued.is_empty() && !self.pending.is_empty() {
            for texture in std::mem::take(&mut self.pending) {
                let slot = texture.sampled_slot.expect("mapped retired texture");
                let readers = self.image_readers(texture.view);
                let submissions: Vec<_> = readers.submissions.iter().copied().collect();
                self.slots.retire(slot, &submissions)?;
                self.cleared.push(texture);
            }
        }
        let previous = self.slots.current_snapshot();
        let result = if let Some(old) = previous {
            self.slots.publish_with(old, |source, edits| device.fork_sampled_texture_table(source, edits))
        } else {
            self.slots.create_snapshot_with(|views| device.create_sampled_texture_table(self.layout, views)).map(Some)
        };
        match result {
            Ok(Some(_)) => {
                self.published = true;
                if let Some(old) = previous {
                    self.retired.push(RetiredSnapshot {
                        snapshot: old,
                        generation: self.generation,
                        readers: std::mem::take(&mut self.snapshot_readers),
                    });
                }
                // Resolve each image against the generations that existed
                // before this publication. The new snapshot contains none of
                // these retired image identities, even if a slot was reused.
                for texture in std::mem::take(&mut self.cleared) {
                    let readers = self.image_readers(texture.view);
                    self.first_generation.remove(&texture.view);
                    if let Some(cohort) = self.retired_textures.iter_mut().find(|old| old.readers == readers) {
                        cohort.textures.push(texture);
                    } else {
                        self.retired_textures.push(RetiredTextures { textures: vec![texture], readers });
                    }
                }
                self.generation = self.generation.saturating_add(1);
            }
            Ok(None) => { self.published = true; }
            Err(error) => {
                // The slot allocator preserves its dirty journal and old
                // snapshot. Keep both alive and draw through legacy PSOs.
                self.published = false;
                eprintln!("gpui-3d sampled texture publication failed: {error:#}; using legacy");
            }
        }
        self.release_completed(device)
    }

    pub(super) fn destroy(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        if let Some(snapshot) = self.slots.current_snapshot() { device.destroy_bind_group(snapshot)?; }
        for old in self.retired.drain(..) {
            device.destroy_bind_group(old.snapshot)?;
        }
        for old in self.retired_textures.drain(..) {
            for texture in old.textures { destroy_texture(device, texture)?; }
        }
        for texture in self.pending.drain(..).chain(self.cleared.drain(..)) {
            destroy_texture(device, texture)?;
        }
        Ok(())
    }
}

pub(super) fn indices(textures: &HashMap<TextureAssetId, TextureResources>, draw: &PreparedDraw) -> [u32; 4] {
    [
        draw.material.albedo_texture, draw.material.normal_texture,
        draw.material.occlusion_texture, draw.material.metallic_roughness_texture,
    ].map(|id| id.and_then(|id| textures.get(&id))
        .and_then(|texture| texture.sampled_slot).map_or(0, |slot| slot.index()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_readers_exclude_snapshots_published_before_allocation() {
        let mut table = SceneSampledTextures::new(
            BindGroupLayoutId::from_parts(1, 1), 4, gfx_core::TextureViewId::from_parts(1, 1),
        ).unwrap();
        let image = gfx_core::TextureViewId::from_parts(2, 1);
        let a = SubmissionId::from_parts(10, 1);
        let b = SubmissionId::from_parts(11, 1);
        let legacy = SubmissionId::from_parts(12, 1);
        table.generation = 2;
        table.first_generation.insert(image, 2);
        let mut old = Readers::default();
        old.submitted(Some(a));
        table.retired.push(RetiredSnapshot {
            snapshot: BindGroupId::from_parts(1, 1), generation: 1, readers: old,
        });
        table.snapshot_readers.submitted(Some(b));
        table.submitted(Some(legacy), None);
        let readers = table.image_readers(image);
        assert!(!readers.submissions.contains(&a));
        assert!(readers.submissions.contains(&b));
        assert!(readers.submissions.contains(&legacy));
        table.completed(b);
        table.completed(legacy);
        assert!(table.image_readers(image).ready());
    }

    #[test]
    fn queued_capture_pins_the_encoded_generation_until_acceptance() {
        let mut table = SceneSampledTextures::new(
            BindGroupLayoutId::from_parts(1, 1), 4, gfx_core::TextureViewId::from_parts(1, 1),
        ).unwrap();
        let first = BindGroupId::from_parts(1, 1);
        let second = BindGroupId::from_parts(2, 1);
        let reader = SubmissionId::from_parts(3, 1);
        table.slots.create_snapshot_with(|_| Ok(first)).unwrap();
        table.queue_capture(Some(first));
        table.slots.allocate(gfx_core::TextureViewId::from_parts(2, 1)).unwrap();
        table.slots.publish_with(first, |_, _| Ok(second)).unwrap();
        table.retired.push(RetiredSnapshot {
            snapshot: first, generation: 1, readers: std::mem::take(&mut table.snapshot_readers),
        });
        assert!(!table.has_completed_retirement());
        table.capture_submitted(Some(reader), Some(second));
        assert!(table.queued.is_empty());
        assert!(!table.retired[0].readers.ready());
        assert!(table.snapshot_readers.ready());
        table.completed(reader);
        assert!(table.has_completed_retirement());
    }

    #[test]
    fn sampled_snapshot_retirement_excludes_unrelated_generations_and_legacy_reads() {
        let mut table = SceneSampledTextures::new(
            BindGroupLayoutId::from_parts(1, 1), 4, gfx_core::TextureViewId::from_parts(1, 1),
        ).unwrap();
        let first = BindGroupId::from_parts(1, 1);
        let second = BindGroupId::from_parts(2, 1);
        let a = SubmissionId::from_parts(10, 1);
        let b = SubmissionId::from_parts(11, 1);
        let legacy = SubmissionId::from_parts(12, 1);
        let c = SubmissionId::from_parts(13, 1);
        table.slots.create_snapshot_with(|_| Ok(first)).unwrap();
        table.submitted(Some(a), Some(first));
        table.submitted(Some(legacy), None);
        table.slots.allocate(gfx_core::TextureViewId::from_parts(2, 1)).unwrap();
        table.slots.publish_with(first, |_, _| Ok(second)).unwrap();
        table.retired.push(RetiredSnapshot {
            snapshot: first, generation: 1, readers: std::mem::take(&mut table.snapshot_readers),
        });
        table.submitted(Some(b), Some(second));
        assert!(!table.retired[0].readers.ready());
        table.completed(b);
        assert!(!table.retired[0].readers.ready());
        table.completed(a);
        assert!(table.retired[0].readers.ready());
        assert!(!table.readers.ready(), "legacy image reader remains outstanding");
        table.submitted(None, Some(second));
        assert!(!table.snapshot_readers.ready());
        table.accepted(c);
        assert!(!table.snapshot_readers.ready());
        table.completed(c);
        assert!(table.snapshot_readers.ready());
        table.completed(legacy);
        assert!(table.readers.ready());
    }
}
