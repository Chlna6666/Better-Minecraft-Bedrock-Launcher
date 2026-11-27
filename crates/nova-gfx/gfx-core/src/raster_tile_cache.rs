//! Physical tile residency. Generation adoption follows GPUI Fast's clean-tile rule.
use crate::{BindGroupDescriptor, BindGroupId, Error, Format, MemoryLocation, RasterTile, ResourceDevice, Result,
    Extent2d, ScissorRect, SubmissionId, TextureDescriptor, TextureDimension, TextureId, TextureUsage,
    TextureViewDescriptor, TextureViewId};
use smallvec::SmallVec;
use std::collections::HashMap;

/// Stable layer and signed tile coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RasterTileKey {
    /// Content layer identity.
    pub layer: u64,
    /// Tile column.
    pub x: i32,
    /// Tile row.
    pub y: i32,
}

/// Borrowed native handles owned by the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterTileTexture {
    /// Physical texture.
    pub texture: TextureId,
    /// Attachment and sampling view.
    pub view: TextureViewId,
}

/// Sampling lease for a completed tile or a writer ordered before the reader.
/// This is not a completion certificate. The caller must establish native
/// queue ordering (or a GPU wait) before submitting the sampling commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[must_use]
pub struct RasterTileRead {
    key: RasterTileKey,
    serial: u64,
    texture: RasterTileTexture,
    writer: Option<SubmissionId>,
    region: ScissorRect,
}
impl RasterTileRead {
    /// Native sampled texture.
    pub fn texture(self) -> RasterTileTexture { self.texture }
    /// The outstanding writer which must precede the consumer.
    pub fn writer(self) -> Option<SubmissionId> { self.writer }
    /// Exact texels occupied by this logical tile in the physical attachment.
    pub fn region(self) -> ScissorRect { self.region }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State { Invalid, Reserved, Submitted(SubmissionId), Ready, Quarantined }

/// Exclusive raster reservation. Dropping this does not establish completion.
#[must_use]
pub struct RasterTileWrite {
    key: RasterTileKey,
    serial: u64,
    texture: RasterTileTexture,
    region: ScissorRect,
    target_extent: Extent2d,
}
impl RasterTileWrite {
    /// Attachment to clear and rasterize.
    pub fn texture(&self) -> RasterTileTexture { self.texture }
    /// Attachment-local rectangle; no draw or clear may escape it.
    pub fn region(&self) -> ScissorRect { self.region }
    /// Physical attachment extent (may contain several logical tiles).
    pub fn target_extent(&self) -> Extent2d { self.target_extent }
}

struct Entry {
    raster: RasterTile,
    format: Format,
    texture: TextureId,
    view: Option<TextureViewId>,
    bytes: u64,
    page_slot: Option<u8>,
    generation: u64,
    serial: u64,
    state: State,
    stale: bool,
    last_used: u64,
    readers: SmallVec<[SubmissionId; 4]>,
    // A sampled descriptor belongs to the physical texture view, not the
    // current content generation or a transient reader submission.
    sampling_bindings: SmallVec<[(usize, BindGroupId); 2]>,
}
impl Entry {
    fn idle(&self) -> bool {
        !matches!(self.state, State::Reserved | State::Submitted(_) | State::Quarantined) && self.readers.is_empty()
    }
    fn invalidate(&mut self) {
        self.stale = true;
        if self.state == State::Ready { self.state = State::Invalid; }
    }
    fn region(&self, page: Option<&Page>) -> ScissorRect {
        ScissorRect { x: 0, y: page.zip(self.page_slot).map_or(0, |(page, slot)| u32::from(slot) * page.cell.height()),
            width: self.raster.extent.width(), height: self.raster.extent.height() }
    }
    fn handles(&self) -> Option<RasterTileTexture> {
        Some(RasterTileTexture { texture: self.texture, view: self.view? })
    }
}

// Fixed size classes avoid a second atlas allocator and fragmentation
// metadata. Pages contain two to four independently fenced vertical cells.
const MAX_PAGE_CELLS: u8 = 4;

fn page_cells(demand: usize) -> u8 {
    demand.min(usize::from(MAX_PAGE_CELLS)) as u8
}

fn page_cell(extent: Extent2d) -> Option<Extent2d> {
    if extent.width() > 2048 || extent.height() > 128 { return None; }
    Extent2d::new(extent.width().next_power_of_two().max(64),
        extent.height().next_power_of_two().max(16)).ok()
}

struct Page {
    cell: Extent2d,
    extent: Extent2d,
    format: Format,
    view: Option<TextureViewId>,
    bytes: u64,
    cells: u8,
    occupied: u8,
    sampling_bindings: SmallVec<[(usize, BindGroupId); 2]>,
}

impl Page {
    fn free_slot(&self) -> Option<u8> {
        (0..self.cells).find(|&slot| self.occupied & (1 << slot) == 0)
    }
}

/// Device-local raster texture cache with explicit native completion tracking.
/// The caller must register accepted readers before further cache mutation.
/// Cache-owned sampling descriptors retire before their views; any other
/// external descriptors remain the caller's responsibility. No CPU waits
/// are inserted; unavailable or busy tiles require ordinary fallback.
pub struct RasterTileCache {
    entries: HashMap<RasterTileKey, Entry>,
    pages: HashMap<TextureId, Page>,
    frame: u64,
    serial: u64,
    bytes: u64,
    budget: u64,
}
impl RasterTileCache {
    /// Hard logical texture byte budget (excludes driver padding).
    pub fn new(budget: u64) -> Self {
        Self { entries: HashMap::new(), pages: HashMap::new(), frame: 0, serial: 0, bytes: 0, budget }
    }

    /// Begin a frame; used tiles are protected from LRU eviction in this frame.
    pub fn begin_frame(&mut self) { self.frame = self.frame.saturating_add(1); }

    /// Resident physical texel bytes, including busy invalid allocations.
    pub fn bytes(&self) -> u64 { self.bytes }

    /// Adopt clean tiles (including pending writes) across consecutive generations only.
    /// Missed generations have no complete dirty-set proof.
    pub fn advance_layer(&mut self, layer: u64, generation: u64, mut dirty: impl FnMut(RasterTileKey) -> bool) {
        for (&key, entry) in &mut self.entries {
            if key.layer != layer || entry.generation == generation { continue; }
            if entry.generation.checked_add(1) == Some(generation)
                && matches!(entry.state, State::Ready | State::Submitted(_))
                && !entry.stale && !dirty(key)
            {
                entry.generation = generation;
            } else {
                entry.invalidate();
            }
        }
    }

    /// Invalidate layer pixels without discarding outstanding native users.
    pub fn invalidate_layer(&mut self, layer: u64) {
        for (key, entry) in &mut self.entries {
            if key.layer == layer { entry.invalidate(); }
        }
    }

    /// Invalidate every content version without retiring native GPU users.
    pub fn invalidate_all(&mut self) {
        for e in self.entries.values_mut() { e.invalidate(); }
    }

    /// Number of live logical allocations, including invalid or busy cells.
    pub fn logical_tiles(&self) -> usize { self.entries.len() }

    /// Number of physical color allocations, including shared pages.
    pub fn physical_textures(&self) -> usize {
        self.pages.len() + self.entries.values().filter(|entry| entry.page_slot.is_none()).count()
    }

    /// True while a native submission still owns a cached texture.
    pub fn has_pending_users(&self) -> bool {
        self.entries.values().any(|e| !e.readers.is_empty() || matches!(e.state, State::Submitted(_)))
    }

    /// Reconcile committed layer generations in one cache walk.
    /// Until exact tile damage is available, a changed or absent generation
    /// invalidates its pixels without discarding native GPU users.
    pub fn retain_generations(&mut self, mut generation: impl FnMut(u64) -> Option<u64>) {
        for (key, e) in &mut self.entries {
            if generation(key.layer) != Some(e.generation) { e.invalidate(); }
        }
    }

    /// Adopt an existing native tile only when the caller proves its exact
    /// raster rectangle survived the generation transition. Pending writers
    /// retain their original fence; no new readiness is implied.
    pub fn reconcile_tiles(
        &mut self,
        mut adopt: impl FnMut(RasterTileKey, u64, RasterTile) -> Option<u64>,
    ) {
        for (&key, e) in &mut self.entries {
            if e.stale { continue; }
            match adopt(key, e.generation, e.raster) {
                Some(generation) if generation == e.generation => {}
                Some(generation) if e.generation.checked_add(1) == Some(generation)
                    && matches!(e.state, State::Ready | State::Submitted(_)) => {
                    e.generation = generation;
                }
                _ => e.invalidate(),
            }
        }
    }

    /// Resolve completed standalone pixels, independent of viewport placement.
    /// Paged consumers must use `lookup_ordered` to obtain the exact subrectangle.
    pub fn lookup(&mut self, key: RasterTileKey, generation: u64, raster: RasterTile, format: Format) -> Option<RasterTileTexture> {
        let e = self.entries.get_mut(&key)?;
        if e.page_slot.is_some() || e.state != State::Ready || e.stale || e.generation != generation
            || e.raster != raster || e.format != format { return None; }
        e.last_used = self.frame;
        e.handles()
    }

    /// Resolve a sampling lease. Pending writes may be selected only when
    /// the caller can establish ordering after their exact native submission.
    /// Ordinary lookup continues to require completed pixels.
    pub fn lookup_ordered(
        &mut self, key: RasterTileKey, generation: u64, raster: RasterTile, format: Format,
        ordered_after: impl FnOnce(SubmissionId) -> bool,
    ) -> Option<RasterTileRead> {
        let e = self.entries.get_mut(&key)?;
        if e.stale || e.generation != generation || e.raster != raster || e.format != format {
            return None;
        }
        let writer = match e.state {
            State::Ready => None,
            State::Submitted(id) if ordered_after(id) => Some(id),
            _ => return None,
        };
        let texture = e.handles()?;
        e.last_used = self.frame;
        Some(RasterTileRead { key, serial: e.serial, texture, writer, region: e.region(self.pages.get(&e.texture)) })
    }

    /// Reuse a sampled descriptor for a physical tile and stable consumer
    /// slot. The descriptor factory is called only on a miss. All external
    /// resources captured by this slot (buffers, sampler and layout) must
    /// remain unchanged and alive until the cache is cleared. Register the
    /// accepted reader before mutating the cache, exactly as for a normal
    /// sampling lease.
    pub fn sampling_binding<D: ResourceDevice + ?Sized>(
        &mut self, device: &mut D, read: RasterTileRead, slot: usize,
        descriptor: impl FnOnce() -> BindGroupDescriptor,
    ) -> Result<BindGroupId> {
        let e = self.entries.get_mut(&read.key)
            .filter(|e| e.serial == read.serial && e.handles() == Some(read.texture)
                && !e.stale && match e.state {
                    State::Ready => true,
                    State::Submitted(writer) => read.writer == Some(writer),
                    _ => false,
                })
            .ok_or_else(|| Error::InvalidInput("binding references a superseded tile lease".into()))?;
        // Shared pages sample the same physical view and stable consumer
        // buffers. Region selection is an instance attribute, not a binding.
        let bindings = if e.page_slot.is_some() {
            &mut self.pages.get_mut(&e.texture).expect("resident page").sampling_bindings
        } else { &mut e.sampling_bindings };
        if let Some((_, binding)) = bindings.iter().find(|(key, _)| *key == slot) {
            return Ok(*binding);
        }
        let binding = device.create_bind_group(&descriptor())?;
        bindings.push((slot, binding));
        Ok(binding)
    }

    /// Register an accepted native reader of an ordered lease. The consumer
    /// must actually have been submitted after the lease's writer; neither
    /// recording this fence nor the dependency makes that writer complete.
    /// Call before further cache mutation, including on rejected WSI presents.
    pub fn record_ordered_reader(&mut self, read: RasterTileRead, submission: SubmissionId) -> Result<()> {
        let e = self.entries.get_mut(&read.key)
            .filter(|e| e.serial == read.serial && e.handles() == Some(read.texture)
                && !e.stale && match e.state {
                    State::Ready => true,
                    State::Submitted(writer) => read.writer == Some(writer) && writer != submission,
                    _ => false,
                })
            .ok_or_else(|| Error::InvalidInput("reader references an unordered or superseded tile".into()))?;
        if !e.readers.contains(&submission) { e.readers.push(submission); }
        Ok(())
    }

    /// Reserve an attachment for repaint. Call lookup first; a busy or
    /// budget-limited miss returns None rather than stalling the GPU owner.
    pub fn prepare<D: ResourceDevice + ?Sized>(&mut self, device: &mut D,
        key: RasterTileKey, generation: u64, raster: RasterTile, format: Format,
    ) -> Result<Option<RasterTileWrite>> {
        self.prepare_internal(device, key, generation, raster, format, None)
    }

    /// Reserve a page subrectangle when a suitable size class is available.
    /// The caller MUST use exact regional clears, clip all draws to the
    /// returned region, and order all page writes and readers on one queue.
    /// Otherwise use the standalone `prepare` API. No extra fence domain
    /// is introduced: each logical entry owns its writer and reader leases.
    pub fn prepare_paged<D: ResourceDevice + ?Sized>(&mut self, device: &mut D,
        key: RasterTileKey, generation: u64, raster: RasterTile, format: Format,
    ) -> Result<Option<RasterTileWrite>> {
        self.prepare_paged_with_demand(device, key, generation, raster, format, usize::from(MAX_PAGE_CELLS))
    }

    /// Size class for batching demand; no physical allocation or lookup.
    pub fn page_size_class(extent: Extent2d) -> Option<(u32, u32)> {
        page_cell(extent).map(|cell| (cell.width(), cell.height()))
    }

    /// Allocate a new shared page only for multiple compatible pending
    /// writes; a single miss can still use an existing free cell. Demand
    /// includes this write. Existing live pages never resize or migrate.
    pub fn prepare_paged_with_demand<D: ResourceDevice + ?Sized>(&mut self, device: &mut D,
        key: RasterTileKey, generation: u64, raster: RasterTile, format: Format, demand: usize,
    ) -> Result<Option<RasterTileWrite>> {
        self.prepare_internal(device, key, generation, raster, format, Some(page_cells(demand)))
    }

    fn prepare_internal<D: ResourceDevice + ?Sized>(&mut self, device: &mut D,
        key: RasterTileKey, generation: u64, raster: RasterTile, format: Format, page_cells: Option<u8>,
    ) -> Result<Option<RasterTileWrite>> {
        if let Some(e) = self.entries.get(&key) {
            if !e.idle() || (e.state == State::Ready && e.last_used == self.frame) {
                return Ok(None);
            }
            if e.raster.extent != raster.extent || e.format != format || e.view.is_none()
                || (page_cells.is_none() && e.page_slot.is_some()) {
                self.release(device, key)?;
            }
        }
        let serial = self.serial.checked_add(1)
            .ok_or_else(|| Error::InvalidInput("tile reservation overflow".into()))?;
        if !self.entries.contains_key(&key) && let Some(cells) = page_cells {
            if let Some(cell) = page_cell(raster.extent) {
                self.allocate_page_slot(device, key, generation, raster, format, serial, cell, cells)?;
            }
        }
        if !self.entries.contains_key(&key) {
            let bytes = u64::from(raster.extent.width())
                .checked_mul(u64::from(raster.extent.height()))
                .and_then(|n| n.checked_mul(u64::from(format.bytes_per_pixel())))
                .ok_or_else(|| Error::InvalidInput("tile byte size overflow".into()))?;
            if bytes > self.budget { return Ok(None); }
            while self.bytes > self.budget - bytes {
                let Some(victim) = self.entries.iter()
                    .filter(|(_, e)| e.idle() && e.last_used != self.frame)
                    .min_by_key(|(_, e)| e.last_used).map(|(&k, _)| k)
                else { return Ok(None); };
                self.release(device, victim)?;
            }
            let texture = device.create_texture(&TextureDescriptor {
                label: Some("resident raster tile".into()), size: raster.extent,
                mip_level_count: 1, format,
                usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED | TextureUsage::COPY_SRC,
                memory_location: MemoryLocation::GpuOnly, dimension: TextureDimension::D2,
                sample_count: 1,
            })?;
            // Retain ownership even if view creation and cleanup fail.
            self.entries.insert(key, Entry {
                raster, format, texture, view: None, bytes, page_slot: None, generation, serial,
                state: State::Invalid, stale: true, last_used: self.frame,
                readers: SmallVec::new(), sampling_bindings: SmallVec::new(),
            });
            self.bytes += bytes;
            match device.create_texture_view(&TextureViewDescriptor {
                label: None, texture, format, base_mip_level: 0, mip_level_count: 1,
            }) {
                Ok(view) => self.entries.get_mut(&key).unwrap().view = Some(view),
                Err(error) => { self.release(device, key)?; return Err(error); }
            }
        }
        self.serial = serial;
        let e = self.entries.get_mut(&key).unwrap();
        e.raster = raster;
        e.generation = generation;
        e.serial = serial;
        e.state = State::Reserved;
        e.stale = false;
        e.last_used = self.frame;
        let page = self.pages.get(&e.texture);
        Ok(Some(RasterTileWrite { key, serial, texture: e.handles().unwrap(),
            region: e.region(page), target_extent: page.map_or(raster.extent, |page| page.extent) }))
    }

    fn allocate_page_slot<D: ResourceDevice + ?Sized>(&mut self, device: &mut D,
        key: RasterTileKey, generation: u64, raster: RasterTile, format: Format, serial: u64, cell: Extent2d, cells: u8,
    ) -> Result<()> {
        // Fill the densest compatible page first, limiting the number
        // of live views and increasing contiguous composite batch hits.
        let candidate = self.pages.iter().filter_map(|(&texture, page)| {
            if page.cell != cell || page.format != format || page.view.is_none() { return None; }
            Some((texture, page.free_slot()?, page.occupied.count_ones()))
        }).max_by_key(|&(_, _, occupancy)| occupancy).map(|(texture, slot, _)| (texture, slot));
        let (texture, slot) = if let Some(candidate) = candidate { candidate } else {
            if cells < 2 { return Ok(()); }
            let extent = Extent2d::new(cell.width(), cell.height() * u32::from(cells))?;
            let bytes = u64::from(extent.width()) * u64::from(extent.height()) * u64::from(format.bytes_per_pixel());
            // Do not evict useful pixels just to speculate on an empty page.
            // The standalone allocator retains its existing exact-budget LRU.
            if bytes > self.budget.saturating_sub(self.bytes) { return Ok(()); }
            let texture = device.create_texture(&TextureDescriptor {
                label: Some("resident raster tile page".into()), size: extent,
                mip_level_count: 1, format,
                usage: TextureUsage::COLOR_ATTACHMENT | TextureUsage::SAMPLED | TextureUsage::COPY_SRC,
                memory_location: MemoryLocation::GpuOnly, dimension: TextureDimension::D2,
                sample_count: 1,
            })?;
            self.pages.insert(texture, Page { cell, extent, format, view: None, bytes,
                cells, occupied: 1, sampling_bindings: SmallVec::new() });
            self.bytes += bytes;
            self.entries.insert(key, Entry { raster, format, texture, view: None, bytes: 0,
                page_slot: Some(0), generation, serial, state: State::Invalid, stale: true,
                last_used: self.frame, readers: SmallVec::new(), sampling_bindings: SmallVec::new() });
            let view = match device.create_texture_view(&TextureViewDescriptor {
                label: None, texture, format, base_mip_level: 0, mip_level_count: 1,
            }) {
                Ok(view) => view,
                Err(error) => { self.release(device, key)?; return Err(error); }
            };
            self.pages.get_mut(&texture).unwrap().view = Some(view);
            self.entries.get_mut(&key).unwrap().view = Some(view);
            return Ok(());
        };
        let page = self.pages.get_mut(&texture).expect("allocated page");
        page.occupied |= 1 << slot;
        self.entries.insert(key, Entry { raster, format, texture, view: page.view, bytes: 0,
            page_slot: Some(slot), generation, serial, state: State::Invalid, stale: true,
            last_used: self.frame, readers: SmallVec::new(), sampling_bindings: SmallVec::new() });
        Ok(())
    }

    fn reservation(&mut self, write: &RasterTileWrite) -> Result<&mut Entry> {
        self.entries.get_mut(&write.key)
            .filter(|e| e.serial == write.serial && e.state == State::Reserved)
            .ok_or_else(|| Error::InvalidInput("stale tile reservation".into()))
    }

    /// Associate an accepted raster submission. None requires already-proven
    /// synchronous GPU completion (e.g. native readback verification).
    pub fn finish_write(&mut self, write: RasterTileWrite, submission: Option<SubmissionId>) -> Result<()> {
        let e = self.reservation(&write)?;
        e.state = match submission {
            Some(id) => State::Submitted(id),
            None if e.stale => State::Invalid,
            None => State::Ready,
        };
        Ok(())
    }

    /// Cancel only work known never to have reached the GPU.
    pub fn cancel_write(&mut self, write: RasterTileWrite) -> Result<()> {
        let e = self.reservation(&write)?;
        e.invalidate();
        e.state = State::Invalid;
        Ok(())
    }

    /// Register an accepted sampling submission before any cache mutation.
    pub fn record_reader(&mut self, key: RasterTileKey, texture: RasterTileTexture, submission: SubmissionId) -> Result<()> {
        let e = self.entries.get_mut(&key)
            .filter(|e| e.page_slot.is_none() && e.state == State::Ready && e.handles() == Some(texture))
            .ok_or_else(|| Error::InvalidInput("reader references nonresident tile".into()))?;
        if !e.readers.contains(&submission) { e.readers.push(submission); }
        Ok(())
    }

    /// An error after native command recording may hide an accepted reader
    /// fence. Never reuse or destroy this allocation on the basis of an
    /// unrelated completion. Its native handles remain device-owned until
    /// teardown; normal known-fence readers still use record_ordered_reader.
    pub fn quarantine_reader(&mut self, read: RasterTileRead) {
        if let Some(e) = self.entries.get_mut(&read.key)
            .filter(|e| e.serial == read.serial && e.handles() == Some(read.texture))
        {
            e.stale = true;
            e.state = State::Quarantined;
        }
    }

    /// Observe a real completed native fence. A queue wait is not completion.
    pub fn complete(&mut self, submission: SubmissionId) {
        for e in self.entries.values_mut() {
            e.readers.retain(|id| *id != submission);
            if e.state == State::Submitted(submission) {
                e.state = if e.stale { State::Invalid } else { State::Ready };
            }
        }
    }

    /// Retire idle tiles outside the requested retention window.
    pub fn trim<D: ResourceDevice + ?Sized>(&mut self, device: &mut D, max_age: u64) -> Result<()> {
        let victims: Vec<_> = self.entries.iter()
            .filter(|(_, e)| e.idle() && self.frame.saturating_sub(e.last_used) > max_age)
            .map(|(&key, _)| key).collect();
        for key in victims { self.release(device, key)?; }
        Ok(())
    }

    /// Explicitly release all resources after their readers and writers retire.
    pub fn clear<D: ResourceDevice + ?Sized>(&mut self, device: &mut D) -> Result<()> {
        if self.entries.values().any(|e| !e.idle()) {
            return Err(Error::InvalidInput("tiles still have native GPU users".into()));
        }
        let keys: Vec<_> = self.entries.keys().copied().collect();
        for key in keys { self.release(device, key)?; }
        Ok(())
    }

    fn release<D: ResourceDevice + ?Sized>(&mut self, device: &mut D, key: RasterTileKey) -> Result<()> {
        let e = self.entries.get_mut(&key).unwrap();
        debug_assert!(e.idle());
        e.invalidate();
        if let Some(slot) = e.page_slot {
            let page = self.pages.get_mut(&e.texture).expect("resident page");
            let mask = 1 << slot;
            if page.occupied != mask {
                page.occupied &= !mask;
                self.entries.remove(&key);
                return Ok(());
            }
            // Last logical slot is idle, hence every prior page user has
            // retired. Destroy shared sampling bindings before the view.
            while let Some((_, binding)) = page.sampling_bindings.last().copied() {
                device.destroy_bind_group(binding)?;
                page.sampling_bindings.pop();
            }
            if let Some(view) = page.view {
                device.destroy_texture_view(view)?;
                page.view = None;
                e.view = None;
            }
            device.destroy_texture(e.texture)?;
            self.bytes -= page.bytes;
            self.pages.remove(&e.texture);
            self.entries.remove(&key);
            return Ok(());
        }
        while let Some((_, binding)) = e.sampling_bindings.last().copied() {
            device.destroy_bind_group(binding)?;
            e.sampling_bindings.pop();
        }
        if let Some(view) = e.view {
            device.destroy_texture_view(view)?;
            e.view = None;
        }
        device.destroy_texture(e.texture)?;
        self.bytes -= e.bytes;
        self.entries.remove(&key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SignedOrigin2d;
    fn key(x: i32) -> RasterTileKey { RasterTileKey { layer: 7, x, y: -1 } }
    fn raster() -> RasterTile {
        RasterTile { origin: SignedOrigin2d::ZERO, extent: Extent2d::new(16, 16).unwrap() }
    }
    fn entry(generation: u64, state: State) -> Entry {
        Entry {
            raster: raster(), format: Format::Bgra8Unorm,
            texture: TextureId::from_parts(1, 1), view: Some(TextureViewId::from_parts(1, 1)),
            bytes: 1024, page_slot: None, generation, serial: 1, state, stale: false,
            last_used: 0, readers: SmallVec::new(), sampling_bindings: SmallVec::new(),
        }
    }
    #[test]
    fn page_size_classes_bound_waste_and_find_independent_cells() {
        let cell = page_cell(Extent2d::new(300, 17).unwrap()).unwrap();
        assert_eq!(cell, Extent2d::new(512, 32).unwrap());
        assert!(page_cell(Extent2d::new(2049, 32).unwrap()).is_none());
        let mut page = Page { cell, extent: Extent2d::new(512, 128).unwrap(),
            format: Format::Bgra8Unorm, view: None, bytes: 512 * 128 * 4,
            cells: 4, occupied: 0b0101, sampling_bindings: SmallVec::new() };
        assert_eq!(page.free_slot(), Some(1));
        let mut tile = entry(1, State::Ready);
        tile.page_slot = Some(2);
        assert_eq!(tile.region(Some(&page)), ScissorRect { x: 0, y: 64, width: 16, height: 16 });
        page.occupied = 0b1111;
        assert_eq!(page.free_slot(), None);
        page.cells = 2;
        page.occupied = 0b0011;
        assert_eq!(page.free_slot(), None);
        assert_eq!(page_cells(1), 1);
        assert_eq!(page_cells(2), 2);
        assert_eq!(page_cells(3), 3);
        assert_eq!(page_cells(64), 4);
    }

    #[test]
    fn clean_generation_adoption() {
        let mut c = RasterTileCache::new(4096);
        c.entries.insert(key(0), entry(3, State::Ready));
        c.entries.insert(key(1), entry(3, State::Ready));
        c.advance_layer(7, 4, |k| k.x == 1);
        assert!(c.lookup(key(0), 4, raster(), Format::Bgra8Unorm).is_some());
        assert!(c.lookup(key(1), 4, raster(), Format::Bgra8Unorm).is_none());
        c.advance_layer(7, 6, |_| false);
        assert!(c.lookup(key(0), 6, raster(), Format::Bgra8Unorm).is_none());
    }
    #[test]
    fn invalidated_writer_cannot_resurrect_pixels() {
        let mut c = RasterTileCache::new(4096);
        let fence = SubmissionId::from_parts(4, 1);
        c.entries.insert(key(0), entry(3, State::Submitted(fence)));
        assert!(!c.entries[&key(0)].idle());
        assert!(c.lookup(key(0), 3, raster(), Format::Bgra8Unorm).is_none());
        c.advance_layer(7, 4, |_| true);
        c.complete(fence);
        assert!(c.entries[&key(0)].idle());
        assert!(c.lookup(key(0), 3, raster(), Format::Bgra8Unorm).is_none());
    }
    #[test]
    fn clean_pending_generation_waits_for_its_real_writer() {
        let mut c = RasterTileCache::new(4096);
        let fence = SubmissionId::from_parts(4, 1);
        c.entries.insert(key(0), entry(3, State::Submitted(fence)));
        c.advance_layer(7, 4, |_| false);
        assert!(c.has_pending_users());
        assert!(c.lookup(key(0), 4, raster(), Format::Bgra8Unorm).is_none());
        c.complete(fence);
        assert!(!c.has_pending_users());
        assert!(c.lookup(key(0), 4, raster(), Format::Bgra8Unorm).is_some());
        c.retain_generations(|_| None);
        assert!(c.lookup(key(0), 4, raster(), Format::Bgra8Unorm).is_none());
    }

    #[test]
    fn ordered_reader_keeps_writer_and_reader_fences_distinct() {
        let mut c = RasterTileCache::new(4096);
        let writer = SubmissionId::from_parts(4, 1);
        let reader = SubmissionId::from_parts(5, 1);
        c.entries.insert(key(0), entry(3, State::Submitted(writer)));
        assert!(c.lookup_ordered(key(0), 3, raster(), Format::Bgra8Unorm, |_| false).is_none());
        let read = c.lookup_ordered(key(0), 3, raster(), Format::Bgra8Unorm, |id| id == writer).unwrap();
        assert_eq!(read.writer(), Some(writer));
        assert!(c.record_ordered_reader(read, writer).is_err());
        c.record_ordered_reader(read, reader).unwrap();
        assert!(c.lookup(key(0), 3, raster(), Format::Bgra8Unorm).is_none());
        c.complete(writer);
        assert!(c.lookup(key(0), 3, raster(), Format::Bgra8Unorm).is_some());
        c.invalidate_layer(7);
        assert!(!c.entries[&key(0)].idle());
        c.complete(reader);
        assert!(c.entries[&key(0)].idle());
        assert!(c.lookup_ordered(key(0), 3, raster(), Format::Bgra8Unorm, |_| true).is_none());
    }

    #[test]
    fn ordered_reader_completion_does_not_certify_writer() {
        let mut c = RasterTileCache::new(4096);
        let writer = SubmissionId::from_parts(4, 1);
        let reader = SubmissionId::from_parts(5, 1);
        c.entries.insert(key(0), entry(3, State::Submitted(writer)));
        let read = c.lookup_ordered(key(0), 3, raster(), Format::Bgra8Unorm, |id| id == writer).unwrap();
        c.record_ordered_reader(read, reader).unwrap();
        c.complete(reader);
        assert!(!c.entries[&key(0)].idle());
        assert!(c.lookup(key(0), 3, raster(), Format::Bgra8Unorm).is_none());
        c.invalidate_all();
        c.complete(writer);
        assert!(c.entries[&key(0)].idle());
        assert!(c.lookup(key(0), 3, raster(), Format::Bgra8Unorm).is_none());
    }

    #[test]
    fn unknown_reader_fence_never_allows_reclamation() {
        let mut c = RasterTileCache::new(4096);
        let writer = SubmissionId::from_parts(4, 1);
        c.entries.insert(key(0), entry(3, State::Submitted(writer)));
        let read = c.lookup_ordered(key(0), 3, raster(), Format::Bgra8Unorm, |_| true).unwrap();
        c.quarantine_reader(read);
        c.complete(writer);
        c.invalidate_all();
        assert!(!c.entries[&key(0)].idle());
        assert!(c.lookup_ordered(key(0), 3, raster(), Format::Bgra8Unorm, |_| true).is_none());
    }

    #[test]
    fn all_readers_must_retire() {
        let mut c = RasterTileCache::new(4096);
        c.entries.insert(key(0), entry(3, State::Ready));
        let t = c.lookup(key(0), 3, raster(), Format::Bgra8Unorm).unwrap();
        let a = SubmissionId::from_parts(5, 1);
        let b = SubmissionId::from_parts(6, 1);
        c.record_reader(key(0), t, a).unwrap();
        c.record_reader(key(0), t, b).unwrap();
        c.invalidate_layer(7);
        c.complete(b);
        assert!(!c.entries[&key(0)].idle());
        c.complete(a);
        assert!(c.entries[&key(0)].idle());
    }
}
