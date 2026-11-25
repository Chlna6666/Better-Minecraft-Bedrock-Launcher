//! Per-SceneView descriptor slots. Texture ownership remains in the existing
//! resident TextureAssetId map; only removed textures are held here until the
//! immutable descriptor snapshots and their readers have retired.
use super::*;
use gfx_core::{SampledTextureSlots, SubmissionId};

struct RetiredSnapshot {
    snapshot: BindGroupId,
    textures: Vec<TextureResources>,
    readers: HashSet<SubmissionId>,
    untracked: bool,
}

pub(super) struct SceneSampledTextures {
    layout: BindGroupLayoutId,
    slots: SampledTextureSlots,
    unmapped: usize,
    readers: HashSet<SubmissionId>,
    untracked: bool,
    pending: Vec<TextureResources>,
    cleared: Vec<TextureResources>,
    retired: Vec<RetiredSnapshot>,
    published: bool,
    remap_pending: bool,
}

impl SceneSampledTextures {
    pub(super) fn new(layout: BindGroupLayoutId, fallback: gfx_core::TextureViewId) -> gpui::Result<Self> {
        Ok(Self {
            layout, slots: SampledTextureSlots::new(super::super::resources::SAMPLED_TABLE_CAPACITY, fallback)?,
            unmapped: 0, readers: HashSet::new(), untracked: false,
            pending: Vec::new(), cleared: Vec::new(), retired: Vec::new(), published: false, remap_pending: false,
        })
    }

    pub(super) fn allocate(&mut self, texture: &mut TextureResources) {
        texture.sampled_slot = self.slots.allocate(texture.view).ok();
        if texture.sampled_slot.is_none() { self.unmapped += 1; }
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
                    self.remap_pending = true;
                }
            }
        }
        self.remap_pending
    }

    pub(super) fn draw_rows_committed(&mut self) {
        self.remap_pending = false;
    }

    pub(super) fn selected(&self, layout: Option<BindGroupLayoutId>) -> Option<BindGroupId> {
        if self.published && !self.remap_pending && self.unmapped == 0 && layout == Some(self.layout) {
            self.slots.current_snapshot()
        } else { None }
    }

    pub(super) fn has_retired(&self) -> bool {
        !self.pending.is_empty() || !self.cleared.is_empty() || !self.retired.is_empty()
    }

    pub(super) fn submitted(&mut self, submission: Option<SubmissionId>) {
        if let Some(submission) = submission {
            self.readers.insert(submission);
        } else {
            self.untracked = true;
        }
    }

    pub(super) fn accepted(&mut self, submission: SubmissionId) {
        // Only the existing ordered Graphics owner callback may seal an
        // untracked scene draw; an unrelated queue completion cannot.
        if self.untracked {
            self.readers.insert(submission);
            self.untracked = false;
        }
        for old in &mut self.retired {
            if old.untracked {
                old.readers.insert(submission);
                old.untracked = false;
            }
        }
    }

    pub(super) fn completed(&mut self, submission: SubmissionId) {
        self.readers.remove(&submission);
        self.slots.complete(submission);
        for old in &mut self.retired { old.readers.remove(&submission); }
    }

    pub(super) fn idle(&mut self) {
        for reader in self.readers.drain() { self.slots.complete(reader); }
        self.untracked = false;
        for old in &mut self.retired {
            old.readers.clear();
            old.untracked = false;
        }
    }

    pub(super) fn release_completed(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        let mut i = 0;
        while i < self.retired.len() {
            if self.retired[i].untracked || !self.retired[i].readers.is_empty() {
                i += 1;
                continue;
            }
            let old = self.retired.swap_remove(i);
            device.destroy_bind_group(old.snapshot)?;
            for texture in old.textures { destroy_texture(device, texture)?; }
        }
        Ok(())
    }

    pub(super) fn needs_publication(&self) -> bool {
        !self.published || (!self.untracked && !self.pending.is_empty())
            || self.slots.dirty_changes().next().is_some()
    }

    pub(super) fn publish(&mut self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        // Preserve every removed image and its slot until untracked native
        // work receives an actual ordered submission fence (or idle proof).
        if !self.untracked && !self.pending.is_empty() {
            let readers: Vec<_> = self.readers.iter().copied().collect();
            for texture in self.pending.drain(..) {
                let slot = texture.sampled_slot.expect("mapped retired texture");
                self.slots.retire(slot, &readers)?;
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
                        snapshot: old, textures: std::mem::take(&mut self.cleared),
                        readers: self.readers.clone(), untracked: self.untracked,
                    });
                } else {
                    for texture in self.cleared.drain(..) { destroy_texture(device, texture)?; }
                }
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
