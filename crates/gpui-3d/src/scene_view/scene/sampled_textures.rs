//! Per-SceneView descriptor slots. Texture ownership remains in the existing
//! resident TextureAssetId map; only removed textures are held here until the
//! immutable descriptor snapshots and their readers have retired.
use super::*;
use gfx_core::{SampledTextureBanks, SampledTextureSlots, SubmissionId, TextureViewId};

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
    // Inclusive interval of descriptor generations that contained these
    // images. Late submissions of a committed/queued old snapshot must
    // extend the image proof, not merely the descriptor-set proof.
    first_generation: u64,
    last_generation: u64,
}

type MaterialTextures = [Option<TextureAssetId>; 4];

fn material_textures(draw: &PreparedDraw) -> MaterialTextures {
    [draw.material.albedo_texture, draw.material.normal_texture,
        draw.material.occlusion_texture, draw.material.metallic_roughness_texture]
}

// The overflow path publishes one immutable generation at a time. The
// ordinary single-table streaming path remains unchanged. A generation is
// rebuilt only when a material tuple or its layout changes.
struct BankGeneration {
    generation: u64,
    readers: Readers,
    layout: BindGroupLayoutId,
    materials: HashMap<MaterialTextures, (usize, [u32; 4])>,
    views: HashSet<TextureViewId>,
    snapshots: Vec<BindGroupId>,
}

impl BankGeneration {
    fn build(
        device: &mut dyn ExtensionDevice, layout: BindGroupLayoutId, capacity: u32,
        fallback: TextureViewId, draws: &[PreparedDraw],
        textures: &HashMap<TextureAssetId, TextureResources>,
    ) -> gfx_core::Result<Self> {
        let mut allocator = SampledTextureBanks::new(capacity, fallback, 32)?;
        let mut materials = HashMap::new();
        let mut views = HashSet::new();
        for draw in draws {
            let key = material_textures(draw);
            if materials.contains_key(&key) { continue; }
            let mut images = [None; 4];
            for (index, asset) in key.iter().enumerate() {
                if let Some(asset) = asset {
                    let view = textures.get(asset).ok_or_else(||
                        gfx_core::Error::InvalidInput("bank material texture not uploaded".into()))?.view;
                    views.insert(view);
                    images[index] = Some(view);
                }
            }
            let lease = allocator.acquire_material(images)?;
            materials.insert(key, (lease.bank(), lease.indices()));
            // This allocator is generation-local: all leases remain live
            // until its immutable snapshots are created.
        }
        let mut snapshots = Vec::with_capacity(allocator.bank_count());
        for bank in 0..allocator.bank_count() {
            match allocator.publish_bank_with(bank,
                |views| device.create_sampled_texture_table(layout, views),
                |_, _| unreachable!("new bank has no previous snapshot"))
            {
                Ok((_, snapshot)) => snapshots.push(snapshot),
                Err(error) => {
                    for snapshot in snapshots { device.destroy_bind_group(snapshot)?; }
                    return Err(error);
                }
            }
        }
        Ok(Self { generation: 0, readers: Readers::default(), layout, materials, views, snapshots })
    }
}

pub(super) struct SceneSampledTextures {
    layout: BindGroupLayoutId,
    fallback: TextureViewId,
    banked: Option<BankGeneration>,
    retired_banks: Vec<BankGeneration>,
    retired_unmapped: Vec<TextureResources>,
    bank_transition_pending: bool,
    bank_build_failed: bool,
    next_bank_generation: u64,
    committed_bank: Option<u64>,
    queued_banks: HashSet<u64>,
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
    committed: Option<BindGroupId>,
    layout_transition_pending: bool,
}

impl SceneSampledTextures {
    pub(super) fn new(layout: BindGroupLayoutId, capacity: u32, fallback: gfx_core::TextureViewId) -> gpui::Result<Self> {
        Ok(Self {
            layout, fallback, banked: None, retired_banks: Vec::new(),
            retired_unmapped: Vec::new(), bank_transition_pending: false, bank_build_failed: false,
            next_bank_generation: 1, committed_bank: None, queued_banks: HashSet::new(),
            slots: SampledTextureSlots::new(capacity, fallback)?,
            unmapped: 0, readers: Readers::default(), legacy_readers: Readers::default(),
            snapshot_readers: Readers::default(), generation: 0, first_generation: HashMap::new(),
            queued: HashSet::new(),
            pending: Vec::new(), cleared: Vec::new(), retired: Vec::new(),
            retired_textures: Vec::new(), published: false, remap_pending: false,
            committed: None, layout_transition_pending: false,
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
            // Unmapped images may belong to a banked or legacy draw.
            self.retired_unmapped.push(texture);
            self.release_completed(device)?;
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

    /// The pipeline family owner must keep its previous layout and PSOs
    /// alive independently until their native readers have retired.
    /// This migrates only the descriptor allocation, not the pipeline family.
    pub(super) fn try_migrate_layout(
        &mut self, device: &mut dyn ExtensionDevice, layout: BindGroupLayoutId, capacity: u32,
    ) {
        if (layout == self.layout && capacity == self.slots.capacity())
            || self.layout_transition_pending || !self.pending.is_empty() || !self.cleared.is_empty()
            || !self.slots.can_resize(capacity)
        { return; }
        match self.slots.resize_snapshot_with(capacity, |views| device.create_sampled_texture_table(layout, views)) {
            Ok((previous, _)) => {
                if let Some(snapshot) = previous {
                    self.retired.push(RetiredSnapshot {
                        snapshot, generation: self.generation,
                        readers: std::mem::take(&mut self.snapshot_readers),
                    });
                }
                self.generation = self.generation.saturating_add(1);
                self.layout = layout;
                self.published = true;
                self.layout_transition_pending = true;
            }
            Err(error) => eprintln!("gpui-3d sampled capacity migration deferred: {error:#}"),
        }
    }

    pub(super) fn layout_transition_pending(&self) -> bool {
        self.layout_transition_pending || self.bank_transition_pending
    }

    pub(super) fn can_migrate_capacity(&self, capacity: u32) -> bool {
        self.pending.is_empty() && self.cleared.is_empty()
            && !self.layout_transition_pending() && self.slots.can_resize(capacity)
    }

    pub(super) fn committed_banked(&self) -> bool { self.committed_bank.is_some() }
    pub(super) fn banked(&self) -> bool { self.banked.is_some() }

    pub(super) fn bank_index(&self, draw: &PreparedDraw) -> Option<usize> {
        self.banked.as_ref()?.materials.get(&material_textures(draw)).map(|entry| entry.0)
    }

    pub(super) fn bank_snapshot(&self, draw: &PreparedDraw) -> Option<BindGroupId> {
        let bank = self.banked.as_ref()?;
        let (index, _) = bank.materials.get(&material_textures(draw))?;
        bank.snapshots.get(*index).copied()
    }

    pub(super) fn draw_indices(&self, textures: &HashMap<TextureAssetId, TextureResources>, draw: &PreparedDraw) -> [u32; 4] {
        if let Some(bank) = &self.banked {
            return bank.materials[&material_textures(draw)].1;
        }
        indices(textures, draw)
    }

    /// Overflow-only material banking. The ordinary single-table fast path
    /// does not walk materials or allocate banks. Existing generations are
    /// kept until the scene commits and their native readers complete.
    pub(super) fn prepare_banks(
        &mut self, device: &mut dyn ExtensionDevice, layout: Option<BindGroupLayoutId>,
        single: Option<BindGroupId>, draws: &[PreparedDraw],
        textures: &HashMap<TextureAssetId, TextureResources>, refresh: bool,
    ) {
        let eligible = single.is_none() && self.unmapped != 0 && layout == Some(self.layout);
        if !eligible {
            self.bank_build_failed = false;
            if let Some(old) = self.banked.take() {
                self.retired_banks.push(old);
                self.bank_transition_pending = true;
            }
            return;
        }
        // Allocation or bank-budget exhaustion must not recreate a failed
        // generation on every camera-only redraw. Retry on changed material
        // demand or a capacity/layout migration.
        if self.bank_build_failed && !refresh { return; }
        if self.banked.as_ref().is_some_and(|bank| bank.layout == self.layout && !refresh) {
            return;
        }
        // If the material tuples and image identities are unchanged, retain
        // the native bank generation across ordinary geometry changes.
        if let Some(bank) = &self.banked {
            if bank.layout == self.layout {
                let mut keys = HashSet::new();
                let mut views = HashSet::new();
                for draw in draws {
                    let key = material_textures(draw);
                    if keys.insert(key) {
                        views.extend(key.iter().flatten().filter_map(|id| textures.get(id).map(|texture| texture.view)));
                    }
                }
                if keys.len() == bank.materials.len() && keys.iter().all(|key| bank.materials.contains_key(key))
                    && views == bank.views { return; }
            }
        }
        let next = match BankGeneration::build(device, self.layout, self.slots.capacity(), self.fallback, draws, textures) {
            Ok(mut next) => {
                next.generation = self.next_bank_generation;
                self.next_bank_generation = self.next_bank_generation.checked_add(1)
                    .expect("sampled bank generation exhausted");
                Some(next)
            }
            Err(error) => {
                eprintln!("gpui-3d sampled bank publication failed: {error:#}; using legacy");
                None
            }
        };
        if let Some(old) = self.banked.take() {
            self.retired_banks.push(old);
            self.bank_transition_pending = true;
        }
        self.bank_build_failed = next.is_none();
        if next.is_some() { self.bank_transition_pending = true; }
        self.banked = next;
    }

    fn bank_references(&self, view: TextureViewId) -> bool {
        self.banked.iter().chain(self.retired_banks.iter()).any(|bank| bank.views.contains(&view))
    }

    pub(super) fn commit_snapshot(&mut self, snapshot: Option<BindGroupId>) {
        self.committed = snapshot;
        self.layout_transition_pending = false;
        self.committed_bank = self.banked.as_ref().map(|bank| bank.generation);
        self.bank_transition_pending = false;
    }

    fn committed_previous_snapshot(&self) -> bool {
        self.committed.is_some() && self.committed != self.slots.current_snapshot()
    }

    pub(super) fn has_completed_retirement(&self) -> bool {
        self.retired.iter().any(|old| old.readers.ready()
            && self.committed != Some(old.snapshot) && !self.queued.contains(&Some(old.snapshot)))
            || (!self.committed_previous_snapshot() && self.queued.is_empty()
                && self.retired_textures.iter().any(|old| old.readers.ready()
                    && old.textures.iter().all(|texture| !self.bank_references(texture.view))))
            || self.retired_banks.iter().any(|bank| self.bank_ready(bank))
            || (self.readers.ready() && self.queued.is_empty() && !self.bank_transition_pending
                && self.retired_unmapped.iter().any(|texture| !self.bank_references(texture.view)))
    }

    pub(super) fn has_retired(&self) -> bool {
        !self.pending.is_empty() || !self.cleared.is_empty() || !self.retired.is_empty()
            || !self.retired_textures.is_empty() || !self.retired_banks.is_empty()
            || !self.retired_unmapped.is_empty()
    }

    fn bank_ready(&self, bank: &BankGeneration) -> bool {
        bank.readers.ready() && self.committed_bank != Some(bank.generation)
            && !self.queued_banks.contains(&bank.generation)
    }

    fn bank_submitted(&mut self, generation: u64, submission: Option<SubmissionId>) {
        if let Some(bank) = self.banked.iter_mut().chain(self.retired_banks.iter_mut())
            .find(|bank| bank.generation == generation)
        {
            bank.readers.submitted(submission);
        } else {
            debug_assert!(false, "submitted unknown sampled bank generation");
        }
    }

    pub(super) fn queue_capture(&mut self, snapshot: Option<BindGroupId>) {
        if snapshot.is_none() && let Some(generation) = self.committed_bank {
            self.queued_banks.insert(generation);
        }
        self.queued.insert(snapshot);
    }

    /// Discard only unsubmitted capture pins. An attempted batch can have
    /// accepted some work without returning a fence, so promote those pins
    /// to conservative readers instead of freeing them.
    pub(super) fn abandon_capture(&mut self, may_have_submitted: bool) {
        let queued = std::mem::take(&mut self.queued);
        let queued_banks = std::mem::take(&mut self.queued_banks);
        if may_have_submitted {
            for generation in queued_banks { self.bank_submitted(generation, None); }
            for snapshot in queued { self.submitted(None, snapshot); }
        }
    }

    /// Stamp the actual descriptor identities encoded in the drained batch.
    /// The current scene may already have published another snapshot.
    pub(super) fn capture_submitted(&mut self, submission: Option<SubmissionId>, fallback: Option<BindGroupId>) {
        if self.queued.is_empty() {
            if fallback.is_none() && let Some(generation) = self.committed_bank {
                self.bank_submitted(generation, submission);
            }
            self.submitted(submission, fallback);
        } else {
            for generation in std::mem::take(&mut self.queued_banks) {
                self.bank_submitted(generation, submission);
            }
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
                let generation = old.generation;
                for textures in &mut self.retired_textures {
                    if (textures.first_generation..=textures.last_generation).contains(&generation) {
                        textures.readers.submitted(submission);
                    }
                }
            } else {
                debug_assert!(false, "submitted unknown sampled snapshot");
                // In release builds keep the conservative image lifetime.
                for textures in &mut self.retired_textures { textures.readers.submitted(submission); }
            }
        } else {
            self.legacy_readers.submitted(submission);
            for textures in &mut self.retired_textures { textures.readers.submitted(submission); }
        }
    }

    pub(super) fn accepted(&mut self, submission: SubmissionId) {
        // Only the ordered Graphics owner may seal unfenced reads.
        self.readers.accepted(submission);
        self.legacy_readers.accepted(submission);
        self.snapshot_readers.accepted(submission);
        for old in &mut self.retired { old.readers.accepted(submission); }
        for old in &mut self.retired_textures { old.readers.accepted(submission); }
        for bank in self.banked.iter_mut().chain(self.retired_banks.iter_mut()) {
            bank.readers.accepted(submission);
        }
    }

    pub(super) fn completed(&mut self, submission: SubmissionId) {
        self.readers.completed(submission);
        self.legacy_readers.completed(submission);
        self.snapshot_readers.completed(submission);
        self.slots.complete(submission);
        for old in &mut self.retired { old.readers.completed(submission); }
        for old in &mut self.retired_textures { old.readers.completed(submission); }
        for bank in self.banked.iter_mut().chain(self.retired_banks.iter_mut()) {
            bank.readers.completed(submission);
        }
    }

    pub(super) fn idle(&mut self) {
        for reader in self.readers.submissions.drain() { self.slots.complete(reader); }
        self.readers.idle();
        self.legacy_readers.idle();
        self.snapshot_readers.idle();
        for old in &mut self.retired { old.readers.idle(); }
        for old in &mut self.retired_textures { old.readers.idle(); }
        for bank in self.banked.iter_mut().chain(self.retired_banks.iter_mut()) { bank.readers.idle(); }
    }

    pub(super) fn release_completed(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let mut bank_index = 0;
        while bank_index < self.retired_banks.len() {
            if !self.bank_ready(&self.retired_banks[bank_index]) {
                bank_index += 1;
                continue;
            }
            let bank = &mut self.retired_banks[bank_index];
            while let Some(&snapshot) = bank.snapshots.last() {
                device.destroy_bind_group(snapshot)?;
                bank.snapshots.pop();
            }
            self.retired_banks.swap_remove(bank_index);
        }
        if self.readers.ready() && self.queued.is_empty() && !self.bank_transition_pending {
            let mut index = 0;
            while index < self.retired_unmapped.len() {
                if self.bank_references(self.retired_unmapped[index].view) {
                    index += 1;
                    continue;
                }
                let texture = self.retired_unmapped.swap_remove(index);
                destroy_texture(device, texture)?;
            }
        }
        let mut i = 0;
        while i < self.retired.len() {
            if !self.retired[i].readers.ready() || self.committed == Some(self.retired[i].snapshot)
                || self.queued.contains(&Some(self.retired[i].snapshot)) {
                i += 1;
                continue;
            }
            device.destroy_bind_group(self.retired[i].snapshot)?;
            self.retired.swap_remove(i);
        }
        // A failed prepare can leave the previous descriptor generation
        // committed. Preserve its images as well as the descriptor set.
        if self.committed_previous_snapshot() || !self.queued.is_empty() { return Ok(()); }
        // Destroy all ready descriptor sets before their retired images.
        let mut i = 0;
        while i < self.retired_textures.len() {
            if !self.retired_textures[i].readers.ready() || self.retired_textures[i].textures.iter()
                .any(|texture| self.bank_references(texture.view))
            { i += 1; continue; }
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
        if !self.readers.untracked && self.queued.is_empty() && !self.pending.is_empty()
            && !self.committed_previous_snapshot() {
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
                    let first_generation = self.first_generation.remove(&texture.view).unwrap_or(0);
                    let last_generation = self.generation;
                    if let Some(cohort) = self.retired_textures.iter_mut().find(|old|
                        old.readers == readers && old.first_generation == first_generation
                            && old.last_generation == last_generation)
                    {
                        cohort.textures.push(texture);
                    } else {
                        self.retired_textures.push(RetiredTextures {
                            textures: vec![texture], readers, first_generation, last_generation,
                        });
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
        for bank in self.banked.take().into_iter().chain(self.retired_banks.drain(..)) {
            for snapshot in bank.snapshots { device.destroy_bind_group(snapshot)?; }
        }
        for texture in self.retired_unmapped.drain(..) { destroy_texture(device, texture)?; }
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
    fn bank_generation_retirement_is_independent_of_new_scene_readers() {
        let layout = BindGroupLayoutId::from_parts(1, 1);
        let mut table = SceneSampledTextures::new(layout, 4, TextureViewId::from_parts(1, 1)).unwrap();
        let bank = |generation| BankGeneration {
            generation, readers: Readers::default(), layout, materials: HashMap::new(),
            views: HashSet::new(), snapshots: Vec::new(),
        };
        let a = SubmissionId::from_parts(10, 1);
        let b = SubmissionId::from_parts(11, 1);
        table.banked = Some(bank(1));
        table.commit_snapshot(None);
        table.queue_capture(None);
        table.retired_banks.push(table.banked.take().unwrap());
        table.banked = Some(bank(2));
        table.commit_snapshot(None);
        assert!(!table.bank_ready(&table.retired_banks[0]));
        table.capture_submitted(Some(a), None);
        table.queue_capture(None);
        table.capture_submitted(Some(b), None);
        assert!(!table.bank_ready(&table.retired_banks[0]));
        table.completed(a);
        assert!(table.bank_ready(&table.retired_banks[0]));
        assert!(!table.readers.ready());
        assert!(!table.banked.as_ref().unwrap().readers.ready());
    }

    #[test]
    fn late_old_generation_submission_extends_retired_image_readers() {
        let mut table = SceneSampledTextures::new(
            BindGroupLayoutId::from_parts(1, 1), 4, gfx_core::TextureViewId::from_parts(1, 1),
        ).unwrap();
        let old = BindGroupId::from_parts(1, 1);
        let next = BindGroupId::from_parts(2, 1);
        let fence = SubmissionId::from_parts(3, 1);
        table.slots.create_snapshot_with(|_| Ok(next)).unwrap();
        table.retired.push(RetiredSnapshot { snapshot: old, generation: 2, readers: Readers::default() });
        for (first_generation, last_generation) in [(1, 2), (3, 4)] {
            table.retired_textures.push(RetiredTextures {
                textures: Vec::new(), readers: Readers::default(), first_generation, last_generation,
            });
        }
        table.queue_capture(Some(old));
        table.capture_submitted(Some(fence), Some(next));
        assert!(!table.retired_textures[0].readers.ready());
        assert!(table.retired_textures[1].readers.ready());
        table.completed(fence);
        assert!(table.retired_textures[0].readers.ready());
    }

    #[test]
    fn previous_committed_snapshot_remains_pinned_until_scene_commit() {
        let mut table = SceneSampledTextures::new(
            BindGroupLayoutId::from_parts(1, 1), 4, gfx_core::TextureViewId::from_parts(1, 1),
        ).unwrap();
        let first = BindGroupId::from_parts(1, 1);
        let second = BindGroupId::from_parts(2, 1);
        table.slots.create_snapshot_with(|_| Ok(first)).unwrap();
        table.commit_snapshot(Some(first));
        table.slots.resize_snapshot_with(8, |_| Ok(second)).unwrap();
        table.retired.push(RetiredSnapshot {
            snapshot: first, generation: 1, readers: Readers::default(),
        });
        assert!(table.committed_previous_snapshot());
        assert!(!table.has_completed_retirement());
        table.commit_snapshot(Some(second));
        assert!(!table.committed_previous_snapshot());
        assert!(table.has_completed_retirement());
    }

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
    fn abandoned_capture_drops_only_unsubmitted_pins() {
        let mut table = SceneSampledTextures::new(
            BindGroupLayoutId::from_parts(1, 1), 4, gfx_core::TextureViewId::from_parts(1, 1),
        ).unwrap();
        let snapshot = BindGroupId::from_parts(1, 1);
        table.slots.create_snapshot_with(|_| Ok(snapshot)).unwrap();
        table.queue_capture(Some(snapshot));
        table.abandon_capture(false);
        assert!(table.queued.is_empty());
        assert!(table.snapshot_readers.ready());
        table.queue_capture(Some(snapshot));
        table.abandon_capture(true);
        assert!(table.queued.is_empty());
        assert!(!table.snapshot_readers.ready());
        let fence = SubmissionId::from_parts(3, 1);
        table.accepted(fence);
        assert!(!table.snapshot_readers.ready());
        table.completed(fence);
        assert!(table.snapshot_readers.ready());
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
