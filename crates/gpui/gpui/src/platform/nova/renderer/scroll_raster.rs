//! Native direct scroll rasterization for certified ordered static roots.
//! Ordinary root spans and tile misses share the same ordered stream encoder.
//! Only a cache miss reads the immutable operation pages. All texture writes
//! and sampling are ordered on the backend's primary graphics queue.
use super::*;
use gfx_core::{OffscreenQueuePreference, RasterTile, RasterTileKey, RasterTileRead,
    RasterTileWrite, RenderPassCommands, SignedOrigin2d};
use crate::{ContentMask, ScaledPixels, scene::{PaintOperation, Primitive}};

const TILE_HEIGHT: i32 = 256;
const MAX_TILES: usize = 64;

struct Request {
    key: RasterTileKey,
    raster: RasterTile,
    source_origin: [i32; 2],
    bounds: Bounds<ScaledPixels>,
}

fn requests(layer: &crate::window::LayerFrame, size: DrawableSize) -> Option<Vec<Request>> {
    let (viewport, translation) = layer.root_static_raster()?;
    let integer = |value: ScaledPixels| {
        (value.0.is_finite() && value.0.fract() == 0. && value.0.abs() <= 1_048_576.)
            .then_some(value.0 as i32)
    };
    let left = integer(viewport.left())?;
    let top = integer(viewport.top())?;
    let right = integer(viewport.right())?;
    let bottom = integer(viewport.bottom())?;
    if left < 0 || top < 0 || right <= left || bottom <= top
        || right as u32 > size.width || bottom as u32 > size.height {
        return None;
    }
    let [dx, dy] = translation;
    if let Some(row) = layer.native_row_raster() {
        let x = integer(row.left())?;
        let y = integer(row.top())?;
        let end_x = integer(row.right())?;
        let end_y = integer(row.bottom())?;
        let extent = Extent2d::new(u32::try_from(end_x.checked_sub(x)?).ok()?,
            u32::try_from(end_y.checked_sub(y)?).ok()?).ok()?;
        let (_, source_origin) = layer.raster_source_for_tile([i64::from(x), i64::from(y),
            i64::from(end_x), i64::from(end_y)])?;
        // A fixed texture and key across edge clipping, including partial
        // rows. The original list viewport remains the composite mask.
        return Some(vec![Request {
            key: RasterTileKey { layer: layer.id, x, y },
            raster: RasterTile { origin: SignedOrigin2d { x, y }, extent },
            source_origin,
            bounds: crate::bounds(crate::point(ScaledPixels(x.checked_add(dx)? as f32),
                ScaledPixels(y.checked_add(dy)? as f32)),
                crate::size(ScaledPixels(extent.width() as f32), ScaledPixels(extent.height() as f32))),
        }]);
    }
    let content_left = left.checked_sub(dx)?;
    let content_top = top.checked_sub(dy)?;
    let content_bottom = bottom.checked_sub(dy)?;
    // Overscan is vertical. A strip uses the exact captured width; no
    // unproven texels are fabricated at the horizontal edges.
    let height = TILE_HEIGHT.min(bottom - top);
    let first = content_top.div_euclid(height);
    let last = (content_bottom - 1).div_euclid(height);
    if usize::try_from(last.checked_sub(first)?.checked_add(1)?).ok()? > MAX_TILES {
        return None;
    }
    let mut result = Vec::with_capacity((last - first + 1) as usize);
    for row in first..=last {
        let grid_y = row.checked_mul(height)?;
        let grid_end = grid_y.checked_add(height)?;
        let (y, end) = (grid_y, grid_end);
        let extent = Extent2d::new((right - left) as u32, (end - y) as u32).ok()?;
        let rect = [i64::from(content_left), i64::from(y),
            i64::from(content_left) + i64::from(extent.width()), i64::from(end)];
        let (_, source_origin) = layer.raster_source_for_tile(rect)?;
        result.push(Request {
            key: RasterTileKey { layer: layer.id, x: content_left, y: row },
            raster: RasterTile { origin: SignedOrigin2d { x: content_left, y }, extent },
            source_origin,
            bounds: crate::bounds(
                crate::point(ScaledPixels(left as f32), ScaledPixels(y.checked_add(dy)? as f32)),
                crate::size(ScaledPixels(extent.width() as f32), ScaledPixels(extent.height() as f32))),
        });
    }
    Some(result)
}

/// Native transient descriptors and upload storage. This is a fence-owned
/// allocation cohort, not another content or texture cache.
#[derive(Default)]
pub(super) struct Resources {
    buffers: Vec<BufferId>,
    bindings: Vec<BindGroupId>,
    depths: Vec<(TextureId, TextureViewId)>,
    gpu_bytes: u64,
}

fn vec_capacity_bytes<T>(values: &Vec<T>) -> u64 {
    (values.capacity() as u64).saturating_mul(std::mem::size_of::<T>() as u64)
}

impl Resources {
    pub(super) fn cpu_capacity_bytes(&self) -> u64 {
        [vec_capacity_bytes(&self.buffers), vec_capacity_bytes(&self.bindings),
            vec_capacity_bytes(&self.depths)].into_iter().fold(0u64, u64::saturating_add)
    }

    pub(super) fn gpu_bytes(&self) -> u64 {
        self.gpu_bytes
    }

    fn allocate_buffer<D: ResourceDevice>(&mut self, device: &mut D, size: usize, usage: BufferUsage) -> Result<BufferId> {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("scroll raster stream".into()),
            size: size as u64,
            usage: usage | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?;
        self.buffers.push(buffer);
        self.gpu_bytes = self.gpu_bytes.saturating_add(size as u64);
        Ok(buffer)
    }

    fn buffer<D: ResourceDevice>(&mut self, device: &mut D, bytes: &[u8], usage: BufferUsage) -> Result<BufferId> {
        let buffer = self.allocate_buffer(device, bytes.len(), usage)?;
        device.write_buffer(buffer, 0, bytes)?;
        Ok(buffer)
    }

    fn binding<D: ResourceDevice>(&mut self, device: &mut D, layout: BindGroupLayoutId,
        entries: Vec<BindGroupEntry>) -> Result<BindGroupId> {
        let binding = device.create_bind_group(&BindGroupDescriptor {
            label: Some("scroll raster binding".into()), layout, entries,
        })?;
        self.bindings.push(binding);
        Ok(binding)
    }

    fn destroy<D: ResourceDevice>(self, device: &mut D) {
        for binding in self.bindings {
            if let Err(error) = device.destroy_bind_group(binding) {
                log::debug!("scroll raster binding retirement failed: {error}");
            }
        }
        for buffer in self.buffers {
            if let Err(error) = device.destroy_buffer(buffer) {
                log::debug!("scroll raster buffer retirement failed: {error}");
            }
        }
        for (texture, view) in self.depths {
            destroy_depth_target(device, texture, view, "scroll raster");
        }
    }
}

// One aligned allocation and one upload for all projection uniforms in
// an accepted writer cohort. Descriptor offsets use the native device's
// reported limits; unknown limits keep the existing dedicated-buffer path.
struct ProjectionArena {
    buffer: Option<BufferId>,
    stride: usize,
    capacity: usize,
    bytes: Vec<u8>,
}

fn projection_arena_layout(limits: gfx_core::BufferBindingLimits, records: usize) -> Option<(usize, usize)> {
    let alignment = usize::try_from(limits.uniform_offset_alignment).ok()?;
    if alignment == 0 || limits.max_uniform_binding_bytes < GLOBAL_UPLOAD_BYTES as u64 {
        return None;
    }
    let stride = GLOBAL_UPLOAD_BYTES.checked_add(alignment - 1)? / alignment * alignment;
    let capacity = stride.checked_mul(records)?;
    // D3D11's entire constant allocation, not only each descriptor
    // range, is bounded by 64 KiB. Use a portable conservative limit.
    (capacity != 0 && capacity <= 64 * 1024).then_some((stride, capacity))
}

impl ProjectionArena {
    fn new<D: ResourceDevice>(device: &mut D, storage: &mut ProjectionStorage, records: usize) -> Result<Self> {
        let Some((stride, capacity)) = projection_arena_layout(device.buffer_binding_limits(), records) else {
            return Ok(Self { buffer: None, stride: 0, capacity: 0, bytes: Vec::new() });
        };
        // The selected physical frame slot has no outstanding reader or
        // producer. Every earlier projection descriptor has retired.
        let buffer = storage.ensure(device, capacity)?;
        Ok(Self { buffer: Some(buffer), stride, capacity, bytes: Vec::with_capacity(capacity) })
    }

    fn push<D: ResourceDevice>(&mut self, device: &mut D, resources: &mut Resources,
        globals: &[u8]) -> Result<(BufferId, u64)> {
        if let Some(buffer) = self.buffer {
            let offset = self.bytes.len();
            anyhow::ensure!(globals.len() == GLOBAL_UPLOAD_BYTES && offset + self.stride <= self.capacity,
                "scroll projection arena capacity exceeded");
            self.bytes.extend_from_slice(globals);
            self.bytes.resize(offset + self.stride, 0);
            Ok((buffer, offset as u64))
        } else {
            Ok((resources.buffer(device, globals, BufferUsage::UNIFORM)?, 0))
        }
    }

    fn upload<D: ResourceDevice>(&self, device: &mut D) -> Result<()> {
        if let Some(buffer) = self.buffer && !self.bytes.is_empty() {
            device.write_buffer(buffer, 0, &self.bytes)?;
        }
        Ok(())
    }
}

#[derive(Default)]
struct ProjectionStorage {
    buffer: Option<BufferId>,
    capacity: usize,
}

impl ProjectionStorage {
    fn ensure<D: ResourceDevice>(&mut self, device: &mut D, required: usize) -> Result<BufferId> {
        if let Some(buffer) = self.buffer && self.capacity >= required { return Ok(buffer); }
        let capacity = required.next_power_of_two();
        // Create before releasing the old buffer so a failed allocation
        // leaves the previous slot usable.
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("resident scroll projection arena".into()),
            size: capacity as u64, usage: BufferUsage::UNIFORM | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?;
        if let Some(old) = self.buffer {
            if let Err(error) = device.destroy_buffer(old) {
                let _ = device.destroy_buffer(buffer);
                return Err(error.into());
            }
        }
        self.buffer = Some(buffer);
        self.capacity = capacity;
        Ok(buffer)
    }
}

fn projection_binding_offset(entries: &mut [BindGroupEntry], offset: u64) {
    if offset == 0 { return; }
    for entry in entries {
        if entry.binding == 0 && let BindingResource::Buffer(binding) = &mut entry.resource {
            binding.offset = offset;
        }
    }
}

// Only the original root prefix/suffix are read here. No projected layer
// primitive is touched on a hit. The proof lives on the published LayerFrame;
// this temporary upload is not a retained CPU scene or coordination cache.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
enum SourceKind {
    Quad,
    Shadow,
    Underline,
    Mono(AtlasTextureId),
    Poly(AtlasTextureId),
    Extension,
}

#[derive(Clone, Copy)]
struct SourceRun {
    kind: SourceKind,
    first: u32,
    count: u32,
}

/// One ephemeral ordered encoder for both a raster miss and the root spans.
/// Separate packed streams use the renderer's existing shader ABI.
#[derive(Default)]
struct StaticStreams {
    // LCD is a root-only overlay, never a raster tile source.
    root_lcd: bool,
    quads: Vec<u8>,
    shadows: Vec<u8>,
    underlines: Vec<u8>,
    mono: Vec<u8>,
    poly: Vec<u8>,
    runs: Vec<SourceRun>,
}

impl StaticStreams {
    fn binding_resident(&self, kind: SourceKind) -> bool {
        let bytes = match kind {
            SourceKind::Quad => self.quads.len(),
            SourceKind::Shadow => self.shadows.len(),
            SourceKind::Underline => self.underlines.len(),
            SourceKind::Mono(_) => self.mono.len(),
            SourceKind::Poly(_) => self.poly.len(),
            SourceKind::Extension => return false,
        };
        bytes <= RETAINED_OUTSIDE_STREAM_BYTES
    }

    fn streams_resident(&self) -> bool {
        // Every static run refers to one of these packed streams.
        // Extension admission is checked separately by the root owner.
        [self.quads.len(), self.shadows.len(), self.underlines.len(),
            self.mono.len(), self.poly.len()].into_iter()
            .all(|bytes| bytes <= RETAINED_OUTSIDE_STREAM_BYTES)
    }

    fn push(&mut self, primitive: &Primitive, split: bool) -> Option<()> {
        macro_rules! append {
            ($stream:ident, $bytes:ident, $limit:ident, $write:ident, $value:expr, $kind:expr) => {{
                let first = self.$stream.len() / $bytes;
                if first >= $limit { return None; }
                $write(&mut self.$stream, $value);
                ($kind, first as u32)
            }};
        }
        let (kind, first) = match primitive {
            Primitive::Quad(q) => append!(quads, PACKED_QUAD_BYTES, MAX_QUADS, write_quad, q, SourceKind::Quad),
            Primitive::Shadow(s) => append!(shadows, PACKED_SHADOW_BYTES, MAX_SHADOWS, write_shadow, s, SourceKind::Shadow),
            Primitive::Underline(u) => append!(underlines, PACKED_UNDERLINE_BYTES, MAX_UNDERLINES, write_underline, u, SourceKind::Underline),
            Primitive::MonochromeSprite(s) if s.tile.texture_id.kind == AtlasTextureKind::Monochrome
                || (self.root_lcd && s.tile.texture_id.kind == AtlasTextureKind::Subpixel) =>
                append!(mono, PACKED_MONO_SPRITE_BYTES, MAX_MONO_SPRITES, write_monochrome_sprite, s, SourceKind::Mono(s.tile.texture_id)),
            Primitive::PolychromeSprite(s) if s.sampling != 2 &&
                matches!(s.tile.texture_id.kind, AtlasTextureKind::Bgra | AtlasTextureKind::Rgba) =>
                append!(poly, PACKED_POLY_SPRITE_BYTES, MAX_POLY_SPRITES, write_polychrome_sprite, s, SourceKind::Poly(s.tile.texture_id)),
            _ => return None,
        };
        if !split && let Some(last) = self.runs.last_mut()
            && last.kind == kind && last.first + last.count == first
        {
            last.count += 1;
        } else {
            self.runs.push(SourceRun { kind, first, count: 1 });
        }
        Some(())
    }

    fn upload<D: ResourceDevice>(&self, device: &mut D, resources: &mut Resources,
        mut buffers: FrameResourceBuffers, text: &[u8], mut retained: Option<(&mut OutsideGpuStreams, bool)>) -> Result<FrameResourceBuffers> {
        // The same stream ABI serves raster misses and the retained root.
        // Only the root has a completed presentation slot that owns buffers.
        macro_rules! upload {
            ($field:ident, $bytes:expr, $usage:expr) => {
                if let Some((slot, unchanged)) = retained.as_mut() {
                    slot.$field.upload(device, resources, $bytes, $usage, *unchanged)?
                } else {
                    resources.buffer(device, $bytes, $usage)?
                }
            };
        }
        if !self.quads.is_empty() {
            buffers.quad_buffer = upload!(quads, &self.quads, BufferUsage::STORAGE);
            buffers.quad_capacity = self.quads.len() / PACKED_QUAD_BYTES;
        }
        if !self.shadows.is_empty() {
            buffers.shadow_buffer = upload!(shadows, &self.shadows, BufferUsage::STORAGE);
            buffers.shadow_capacity = self.shadows.len() / PACKED_SHADOW_BYTES;
        }
        if !self.underlines.is_empty() {
            buffers.underline_buffer = upload!(underlines, &self.underlines, BufferUsage::STORAGE);
        }
        if !self.mono.is_empty() {
            buffers.mono_sprite_buffer = upload!(mono, &self.mono, BufferUsage::STORAGE);
            buffers.mono_sprite_capacity = self.mono.len() / PACKED_MONO_SPRITE_BYTES;
            buffers.text_raster_buffer = if let Some((slot, _)) = retained.as_mut() {
                CompositeSlot::write_changed(device, &mut slot.text, &mut slot.text_bytes,
                    text, text.len(), text.len(), BufferUsage::UNIFORM)?
            } else {
                resources.buffer(device, text, BufferUsage::UNIFORM)?
            };
        }
        if !self.poly.is_empty() {
            buffers.poly_sprite_buffer = upload!(poly, &self.poly, BufferUsage::STORAGE);
            buffers.poly_sprite_capacity = self.poly.len() / PACKED_POLY_SPRITE_BYTES;
        }
        Ok(buffers)
    }

    fn commands<D: ResourceDevice>(&self, renderer: &NovaRenderer, device: &mut D,
        resources: &mut Resources, buffers: FrameResourceBuffers, global_offset: u64, blend: BlendPipelines,
        extension_steps: &[Vec<RenderCommand>],
        mut resident: Option<&mut FxHashMap<SourceKind, BindGroupId>>) -> Result<Vec<RenderCommand>> {
        let mut bindings = FxHashMap::default();
        let mut commands = Vec::with_capacity(self.runs.len());
        for run in &self.runs {
            if run.kind == SourceKind::Extension {
                commands.extend_from_slice(extension_steps.get(run.first as usize)
                    .context("missing prepared root extension commands")?);
                continue;
            }
            let pipeline = match run.kind {
                SourceKind::Quad => blend.quads,
                SourceKind::Shadow => blend.shadows,
                SourceKind::Underline => blend.underlines,
                #[cfg(target_os = "windows")]
                SourceKind::Mono(id) if id.kind == AtlasTextureKind::Subpixel => blend.subpixel_sprites,
                SourceKind::Mono(_) => blend.mono_sprites,
                SourceKind::Poly(_) => blend.poly_sprites,
                SourceKind::Extension => unreachable!("extension run was expanded"),
            };
            // Bounded private streams own the descriptor. Atlas replacement
            // explicitly retires every slot's descriptors before its view.
            let retain = self.binding_resident(run.kind);
            let cached = if retain { resident.as_ref().and_then(|map| map.get(&run.kind)).copied() } else { None };
            let group = if let Some(group) = cached.or_else(|| bindings.get(&run.kind).copied()) { group } else {
                let (layout, mut entries) = match run.kind {
                    SourceKind::Quad => (renderer.quad_bind_group_layout, quad_resource_bindings(&buffers)),
                    SourceKind::Shadow => (renderer.shadow_bind_group_layout, shadow_resource_bindings(&buffers)),
                    SourceKind::Underline => {
                        let mut entries = underline_resource_bindings(&buffers);
                        // The regular renderer uses a fixed maximum capacity;
                        // this transient stream binds only its actual bytes.
                        for entry in &mut entries {
                            if entry.binding == 7 && let BindingResource::Buffer(binding) = &mut entry.resource {
                                binding.size = self.underlines.len() as u64;
                            }
                        }
                        (renderer.underline_bind_group_layout, entries)
                    }
                    SourceKind::Mono(id) => (renderer.mono_sprite_bind_group_layout,
                        mono_atlas_resource_bindings(buffers, renderer.gpu_atlas_textures[&id].texture_view, renderer.atlas_sampler)),
                    SourceKind::Poly(id) => (renderer.poly_sprite_bind_group_layout,
                        poly_atlas_resource_bindings(buffers, renderer.gpu_atlas_textures[&id].texture_view, renderer.atlas_sampler)),
                    SourceKind::Extension => unreachable!("extension run has no static binding"),
                };
                projection_binding_offset(&mut entries, global_offset);
                if retain && let Some(map) = resident.as_mut() {
                    let group = device.create_bind_group(&BindGroupDescriptor {
                        label: Some("resident scroll outside binding".into()), layout, entries,
                    })?;
                    map.insert(run.kind, group);
                    group
                } else {
                    let group = resources.binding(device, layout, entries)?;
                    bindings.insert(run.kind, group);
                    group
                }
            };
            commands.push(RenderCommand::Draw(DrawCommand {
                pipeline, bind_groups: bind_group_list([group]), vertex_count: 4,
                first_vertex: 0, instance_count: run.count, first_instance: run.first, scissor: None,
            }));
        }
        Ok(commands)
    }
}

struct LayerPlan<'a> {
    layer: &'a crate::window::LayerFrame,
    span: std::ops::Range<usize>,
    requests: Vec<Request>,
}

fn layer_plan<'a>(layers: &'a [crate::window::LayerFrame], scene_len: usize, size: DrawableSize) -> Vec<LayerPlan<'a>> {
    let mut candidates: Vec<_> = layers.iter().filter_map(|layer| {
        let span = layer.root_static_span()?.clone();
        if span.start > span.end || span.end > scene_len { return None; }
        Some(LayerPlan { layer, span, requests: requests(layer, size)? })
    }).collect();
    candidates.sort_by_key(|plan| (plan.span.start, std::cmp::Reverse(plan.span.end)));
    let mut end = 0;
    let mut total = 0;
    candidates.retain(|plan| {
        // Nested or overlapping captures cannot both replace the same root
        // operations. Prefer the enclosing span; leave other content intact.
        if plan.span.start < end || total + plan.requests.len() > MAX_TILES { return false; }
        total += plan.requests.len();
        end = plan.span.end;
        true
    });
    candidates
}

fn all_deferred_scrolls_planned(scene: &crate::Scene, plans: &[LayerPlan<'_>]) -> bool {
    let mut plans = plans.iter().peekable();
    scene.paint_operations.deferred_scroll_indices().all(|index| {
        while plans.peek().is_some_and(|plan| plan.span.end <= index) { plans.next(); }
        plans.next().is_some_and(|plan| plan.span.start == index && plan.span.end == index + 1)
    })
}

fn outside_ranges(plans: &[LayerPlan<'_>], scene_len: usize) -> Option<Vec<std::ops::Range<usize>>> {
    let mut ranges = Vec::with_capacity(plans.len() + 1);
    let mut start = 0;
    for plan in plans {
        if start > plan.span.start || plan.span.end > scene_len { return None; }
        ranges.push(start..plan.span.start);
        start = plan.span.end;
    }
    ranges.push(start..scene_len);
    Some(ranges)
}

/// One retained encoding of the exact complement of promoted root spans.
/// Share existing immutable operation pages, never retain a previous Scene
/// or its fallback. Exact Atlas placements protect resolved physical UVs.
pub(super) struct OutsideRoot {
    source: crate::scene::Operations,
    source_ends: Vec<usize>,
    atlas_tiles: Vec<AtlasTile>,
    streams: StaticStreams,
    boundaries: Vec<usize>,
    extensions: Vec<crate::PaintRendererExtension>,
}

/// Validation belongs to the GPU owner, not the shared immutable encoding.
pub(super) struct OutsideRootCache {
    root: Arc<OutsideRoot>,
    atlas_epoch: Option<[u64; 4]>,
}

impl OutsideRootCache {
    pub(super) fn cpu_capacity_bytes(&self) -> u64 {
        let root = &self.root;
        let streams = &root.streams;
        [root.source.private_capacity_bytes(), vec_capacity_bytes(&root.source_ends),
            vec_capacity_bytes(&root.atlas_tiles), vec_capacity_bytes(&root.boundaries),
            vec_capacity_bytes(&root.extensions), vec_capacity_bytes(&streams.quads),
            vec_capacity_bytes(&streams.shadows), vec_capacity_bytes(&streams.underlines),
            vec_capacity_bytes(&streams.mono), vec_capacity_bytes(&streams.poly),
            vec_capacity_bytes(&streams.runs)].into_iter().fold(0u64, u64::saturating_add)
    }
}

fn outside_atlas_epoch(renderer: &NovaRenderer) -> [u64; 4] {
    [renderer.atlas.placement_generation(), renderer.atlas.texture_set_generation(),
        renderer.atlas.scroll_content_generation(), renderer.atlas.scroll_damage_generation()]
}

/// Translate run boundaries to command boundaries once. An extension may
/// emit any number of commands, including zero; do not scan all extension
/// runs again for each promoted layer.
fn expanded_boundaries(runs: &[SourceRun], boundaries: &[usize], steps: &[Vec<RenderCommand>]) -> Option<Vec<usize>> {
    let mut result = Vec::with_capacity(boundaries.len());
    let mut cursor = 0;
    let mut commands = 0usize;
    for &end in boundaries {
        for run in runs.get(cursor..end)? {
            commands = commands.checked_add(if run.kind == SourceKind::Extension {
                steps.get(run.first as usize)?.len()
            } else { 1 })?;
        }
        result.push(commands);
        cursor = end;
    }
    Some(result)
}

fn resolve_primitive(primitive: &Primitive, placements: &FxHashMap<crate::TileId, AtlasTile>,
    gpu: &FxHashMap<AtlasTextureId, NovaGpuAtlasTexture>) -> Option<Primitive> {
    let mut primitive = primitive.clone();
    let tile = match &mut primitive {
        Primitive::MonochromeSprite(sprite) => &mut sprite.tile,
        Primitive::PolychromeSprite(sprite) => &mut sprite.tile,
        _ => return Some(primitive),
    };
    *tile = *placements.get(&tile.tile_id)?;
    gpu.contains_key(&tile.texture_id).then_some(primitive)
}

impl OutsideRoot {
    fn matches_ranges(&self, scene: &crate::scene::Operations, ranges: &[std::ops::Range<usize>]) -> bool {
        if ranges.len() != self.source_ends.len() { return false; }
        let mut start = 0;
        for (range, &end) in ranges.iter().zip(&self.source_ends) {
            if !self.source.range(start..end).matches_by(scene.range(range.clone()), PaintOperation::same_paint) {
                return false;
            }
            start = end;
        }
        true
    }

    fn pack(renderer: &NovaRenderer, ranges: &[std::ops::Range<usize>], scene: &crate::Scene) -> Option<OutsideRootCache> {
        let before = outside_atlas_epoch(renderer);
        let mut ids = Vec::new();
        for range in ranges {
            for operation in scene.paint_operations.get_range(range.clone())?.iter() {
                match operation {
                    PaintOperation::Primitive(Primitive::MonochromeSprite(s)) => ids.push((s.tile.texture_id, s.tile.tile_id.0)),
                    PaintOperation::Primitive(Primitive::PolychromeSprite(s)) => ids.push((s.tile.texture_id, s.tile.tile_id.0)),
                    _ => {}
                }
            }
        }
        let placements = if ids.is_empty() { FxHashMap::default() }
            else { renderer.atlas.ready_root_glyphs(&ids)? };
        let mut streams = StaticStreams { root_lcd: cfg!(target_os = "windows"), ..Default::default() };
        let mut boundaries = Vec::with_capacity(ranges.len().saturating_sub(1));
        let mut extensions = Vec::new();
        for (index, range) in ranges.iter().enumerate() {
            let mut split = true;
            for operation in scene.paint_operations.get_range(range.clone())?.iter() {
                match operation {
                    PaintOperation::Primitive(Primitive::RendererExtension(input)) => {
                        let first = u32::try_from(extensions.len()).ok()?;
                        extensions.push(input.clone());
                        streams.runs.push(SourceRun { kind: SourceKind::Extension, first, count: 1 });
                        split = true;
                    }
                    PaintOperation::Primitive(primitive) => {
                        let primitive = resolve_primitive(primitive, &placements, &renderer.gpu_atlas_textures)?;
                        streams.push(&primitive, split)?;
                        split = false;
                    }
                    PaintOperation::StartLayer(_) | PaintOperation::EndLayer => {}
                    _ => return None,
                }
            }
            if index + 1 < ranges.len() { boundaries.push(streams.runs.len()); }
        }
        let atlas_epoch = if ids.is_empty() { None } else {
            if outside_atlas_epoch(renderer) != before { return None; }
            Some(before)
        };
        let mut source = crate::scene::Operations::default();
        let mut source_ends = Vec::with_capacity(ranges.len());
        for range in ranges {
            source.share(&scene.paint_operations, range.clone());
            source_ends.push(source.len());
        }
        source.compact();
        let atlas_tiles = placements.into_values().collect();
        Some(OutsideRootCache {
            root: Arc::new(Self { source, source_ends, atlas_tiles, streams, boundaries, extensions }),
            atlas_epoch,
        })
    }
}

/// Small physical GPU streams owned by an existing presentation frame slot.
/// A free slot proves that all prior readers (and its writer) have retired.
/// Do not overwrite the normal renderer's cross-slot shared static streams.
#[derive(Default)]
pub(in crate::platform::nova) struct CompositeSlot {
    global: Option<BufferId>,
    sprites: Option<BufferId>,
    global_bytes: Vec<u8>,
    sprite_bytes: Vec<u8>,
    sprite_records: Vec<Option<CompositeRecord>>,
    layer_slots: StableSlots<u64>,
    tile_slots: StableSlots<RasterTileKey>,
    placements: Option<BufferId>,
    placement_bytes: Vec<u8>,
    projection: ProjectionStorage,
    outside: OutsideGpuStreams,
    commands: Option<ResidentCompositeCommands>,
    // Dedicated immutable animation sentinel; never bind a growable normal
    // renderer animation buffer into a resident tile descriptor.
    static_animation: Option<BufferId>,
}

// Retain only commands whose complete native dependencies are resident.
// Dynamic extensions and oversized streams remain fence-owned transients.
struct ResidentCompositeCommands {
    root: std::sync::Weak<OutsideRoot>,
    pipeline: RenderPipelineId,
    tiles: Vec<(u32, BindGroupId)>,
    boundaries: Vec<(usize, usize)>,
    commands: Arc<[RenderCommand]>,
}

impl ResidentCompositeCommands {
    fn matches(&self, root: &Arc<OutsideRoot>, pipeline: RenderPipelineId,
        tiles: &[(u32, BindGroupId)], boundaries: &[(usize, usize)]) -> bool {
        std::ptr::eq(self.root.as_ptr(), Arc::as_ptr(root))
            && self.pipeline == pipeline && self.tiles == tiles && self.boundaries == boundaries
    }
}

// The existing free presentation slot is the ownership boundary. Keep
// surviving identities at their physical indices; only departed identities
// release slots. Paint order is carried by commands, not buffer order.
struct StableSlots<K> {
    slots: FxHashMap<K, u32>,
}

impl<K> Default for StableSlots<K> {
    fn default() -> Self { Self { slots: FxHashMap::default() } }
}

impl<K: Copy + Eq + std::hash::Hash> StableSlots<K> {
    fn reconcile(&mut self, keys: impl IntoIterator<Item = K>) -> Result<SmallVec<[u32; 64]>> {
        let keys: SmallVec<[K; 64]> = keys.into_iter().collect();
        anyhow::ensure!(keys.len() <= MAX_TILES, "invalid scroll composite identities");

        // A pure translation or reorder keeps the same physical identities.
        // Resolve directly into inline storage; the physical slot mask also
        // proves uniqueness, without rebuilding a hash set on every frame.
        let mut ordered = SmallVec::<[u32; 64]>::new();
        let mut occupied = 0u64;
        for key in &keys {
            let Some(&slot) = self.slots.get(key) else { break; };
            let bit = 1u64 << slot;
            anyhow::ensure!(occupied & bit == 0, "invalid scroll composite identities");
            occupied |= bit;
            ordered.push(slot);
        }
        if ordered.len() == keys.len() {
            if self.slots.len() != keys.len() {
                self.slots.retain(|_, slot| occupied & (1u64 << *slot) != 0);
            }
            return Ok(ordered);
        }

        // New identities need a complete membership proof before mutating
        // the stable map. An invalid request must leave all slots untouched.
        let live: FxHashSet<_> = keys.iter().copied().collect();
        anyhow::ensure!(live.len() == keys.len(), "invalid scroll composite identities");
        self.slots.retain(|key, _| live.contains(key));
        occupied = self.slots.values().fold(0u64, |mask, &slot| mask | (1u64 << slot));
        ordered.clear();
        for key in keys {
            let slot = *self.slots.entry(key).or_insert_with(|| {
                let slot = (!occupied).trailing_zeros();
                debug_assert!(slot < MAX_TILES as u32);
                occupied |= 1u64 << slot;
                slot
            });
            ordered.push(slot);
        }
        Ok(ordered)
    }
}

// Physical region and content-space geometry remain immutable while a
// layer scrolls. The compact per-layer table owns translation and clipping.
#[derive(Clone, Copy, PartialEq)]
struct CompositeRecord {
    layer_slot: u32,
    raster: RasterTile,
    region: gfx_core::ScissorRect,
}

impl CompositeRecord {
    fn encode(self, bytes: &mut Vec<u8>) {
        let bounds = crate::bounds(crate::point(ScaledPixels(self.raster.origin.x as f32),
            ScaledPixels(self.raster.origin.y as f32)),
            crate::size(ScaledPixels(self.raster.extent.width() as f32), ScaledPixels(self.raster.extent.height() as f32)));
        let start = bytes.len();
        write_polychrome_sprite(bytes, &PolychromeSprite {
            order: 0, bounds, content_mask: ContentMask::new(bounds),
            corner_radii: Default::default(), grayscale: false, opacity: 1., sampling: 2, animation_id: None,
            tile: AtlasTile { texture_id: AtlasTextureId { index: u32::MAX, kind: AtlasTextureKind::Bgra },
                tile_id: crate::TileId(u32::MAX), padding: 0,
                bounds: crate::bounds(crate::point(DevicePixels(self.region.x as i32), DevicePixels(self.region.y as i32)),
                    crate::size(DevicePixels(self.region.width as i32), DevicePixels(self.region.height as i32))) },
        });
        // Reuse binding 17's 64-byte record ABI; sampling=2 selects the
        // placement interpretation. Slot zero retains the existing sprite
        // semantics, including ordinary native raster tests.
        bytes[start..start + 4].copy_from_slice(&self.layer_slot.to_ne_bytes());
    }
}

fn append_placement(bytes: &mut Vec<u8>, viewport: Bounds<ScaledPixels>, translation: [i32; 2]) {
    let mut record = [0u8; PACKED_ANIMATION_VALUE_BYTES];
    for (index, value) in [translation[0] as f32, translation[1] as f32].into_iter().enumerate() {
        record[16 + index * 4..20 + index * 4].copy_from_slice(&value.to_ne_bytes());
    }
    for (index, value) in [viewport.origin.x.0, viewport.origin.y.0,
        viewport.size.width.0, viewport.size.height.0].into_iter().enumerate() {
        record[32 + index * 4..36 + index * 4].copy_from_slice(&value.to_ne_bytes());
    }
    bytes.extend_from_slice(&record);
}

// Limit physical retention independently of Scene size. Large streams
// use the existing fence-owned transient path, not unbounded slot growth.
const RETAINED_OUTSIDE_STREAM_BYTES: usize = 256 * 1024;

#[derive(Default)]
struct OutsideBuffer {
    buffer: Option<BufferId>,
    capacity: usize,
}

impl OutsideBuffer {
    fn upload<D: ResourceDevice>(&mut self, device: &mut D, resources: &mut Resources,
        bytes: &[u8], usage: BufferUsage, unchanged: bool) -> Result<BufferId> {
        if bytes.len() > RETAINED_OUTSIDE_STREAM_BYTES {
            return resources.buffer(device, bytes, usage);
        }
        if self.capacity < bytes.len() {
            // This slot is free and its transient descriptors have already
            // retired; replacing its private buffer cannot race a reader.
            if let Some(old) = self.buffer {
                device.destroy_buffer(old)?;
                self.buffer = None;
                self.capacity = 0;
            }
        }
        if let Some(buffer) = self.buffer {
            if !unchanged { device.write_buffer(buffer, 0, bytes)?; }
            return Ok(buffer);
        }
        let capacity = bytes.len().next_power_of_two();
        let buffer = CompositeSlot::write(device, &mut self.buffer, bytes, capacity, usage)?;
        self.capacity = capacity;
        Ok(buffer)
    }
}

#[derive(Default)]
struct OutsideGpuStreams {
    root: std::sync::Weak<OutsideRoot>,
    bindings: FxHashMap<SourceKind, BindGroupId>,
    quads: OutsideBuffer,
    shadows: OutsideBuffer,
    underlines: OutsideBuffer,
    mono: OutsideBuffer,
    poly: OutsideBuffer,
    text: Option<BufferId>,
    text_bytes: Vec<u8>,
}

impl OutsideGpuStreams {
    fn retire_bindings<D: ResourceDevice>(&mut self, device: &mut D) -> Result<()> {
        // Called only for a free physical frame slot, before buffer growth.
        while let Some((&kind, &id)) = self.bindings.iter().next() {
            device.destroy_bind_group(id)?;
            self.bindings.remove(&kind);
        }
        Ok(())
    }

    fn handles(&self) -> [Option<BufferId>; 6] {
        [self.quads.buffer, self.shadows.buffer, self.underlines.buffer,
            self.mono.buffer, self.poly.buffer, self.text]
    }
}

/// Compare complete ABI records and merge adjacent changed intervals.
/// A shorter active stream never needs its unreachable tail cleared.
fn changed_record_ranges(previous: &[u8], next: &[u8], stride: usize) -> Vec<std::ops::Range<usize>> {
    debug_assert!(stride != 0 && next.len() % stride == 0);
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (index, record) in next.chunks_exact(stride).enumerate() {
        let start = index * stride;
        let end = start + stride;
        if previous.get(start..end) == Some(record) { continue; }
        if let Some(last) = ranges.last_mut() && last.end == start {
            last.end = end;
        } else {
            ranges.push(start..end);
        }
    }
    ranges
}

impl CompositeSlot {
    /// Private CPU backing capacities. Hash table buckets, nested extension
    /// payloads and allocator overhead are excluded rather than estimated.
    pub(super) fn cpu_capacity_bytes(&self) -> u64 {
        let commands = self.commands.as_ref().map_or(0, |cached| {
            vec_capacity_bytes(&cached.tiles)
                .saturating_add(vec_capacity_bytes(&cached.boundaries))
                .saturating_add((cached.commands.len() as u64)
                    .saturating_mul(std::mem::size_of::<RenderCommand>() as u64))
        });
        [vec_capacity_bytes(&self.global_bytes), vec_capacity_bytes(&self.sprite_bytes),
            vec_capacity_bytes(&self.placement_bytes), vec_capacity_bytes(&self.sprite_records),
            vec_capacity_bytes(&self.outside.text_bytes), commands]
            .into_iter().fold(0u64, u64::saturating_add)
    }

    /// Logical sizes of buffers actually allocated by this physical slot.
    /// Tile pages and fence-owned transient allocations are counted elsewhere.
    pub(super) fn gpu_bytes(&self) -> u64 {
        let fixed = [
            (self.global, GLOBAL_UPLOAD_BYTES),
            (self.sprites, MAX_TILES * PACKED_POLY_SPRITE_BYTES),
            (self.placements, MAX_TILES * PACKED_ANIMATION_VALUE_BYTES),
            (self.static_animation, PACKED_ANIMATION_VALUE_BYTES),
            (self.outside.text, TEXT_RASTER_UPLOAD_BYTES),
            (self.projection.buffer, self.projection.capacity),
        ];
        let outside = [&self.outside.quads, &self.outside.shadows, &self.outside.underlines,
            &self.outside.mono, &self.outside.poly];
        fixed.into_iter().filter(|(buffer, _)| buffer.is_some()).map(|(_, bytes)| bytes as u64)
            .chain(outside.into_iter().filter(|stream| stream.buffer.is_some())
                .map(|stream| stream.capacity as u64))
            .fold(0u64, u64::saturating_add)
    }

    /// Retire only the descriptors sampling the physical page being removed
    /// or replaced. Like ordinary Atlas descriptors, accepted native work
    /// remains owned by the backend's submission retirement mechanism.
    pub(in crate::platform::nova) fn retire_atlas_bindings<D: ResourceDevice>(
        &mut self, device: &mut D, atlas: AtlasTextureId,
    ) -> Result<()> {
        for kind in [SourceKind::Mono(atlas), SourceKind::Poly(atlas)] {
            if let Some(&group) = self.outside.bindings.get(&kind) {
                self.commands = None;
                device.destroy_bind_group(group)?;
                self.outside.bindings.remove(&kind);
            }
        }
        Ok(())
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(super) fn outside_quad_binding(&self) -> Option<BindGroupId> {
        self.outside.bindings.get(&SourceKind::Quad).copied()
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(super) fn handles(&self) -> [Option<BufferId>; 3] {
        [self.global, self.sprites, self.outside.quads.buffer]
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(super) fn resident_command_list(&self) -> Option<Arc<[RenderCommand]>> {
        self.commands.as_ref().map(|commands| Arc::clone(&commands.commands))
    }

    #[cfg(all(test, target_os = "windows"))]
    pub(super) fn projection_buffer(&self) -> Option<BufferId> {
        self.projection.buffer
    }

    fn write<D: ResourceDevice>(device: &mut D, buffer: &mut Option<BufferId>,
        bytes: &[u8], capacity: usize, usage: BufferUsage) -> Result<BufferId> {
        debug_assert!(bytes.len() <= capacity);
        let id = if let Some(id) = *buffer {
            id
        } else {
            let id = device.create_buffer(&BufferDescriptor {
                label: Some("scroll composite frame slot".into()),
                size: capacity as u64, usage: usage | BufferUsage::COPY_DST,
                memory_location: MemoryLocation::CpuToGpu,
            })?;
            *buffer = Some(id);
            id
        };
        device.write_buffer(id, 0, bytes)?;
        Ok(id)
    }

    fn write_changed<D: ResourceDevice>(device: &mut D, buffer: &mut Option<BufferId>,
        previous: &mut Vec<u8>, bytes: &[u8], capacity: usize, stride: usize, usage: BufferUsage) -> Result<BufferId> {
        if buffer.is_none() {
            previous.clear();
            let id = Self::write(device, buffer, bytes, capacity, usage)?;
            previous.extend_from_slice(bytes);
            return Ok(id);
        }
        let id = buffer.expect("resident composite buffer");
        if previous.as_slice() == bytes { return Ok(id); }
        let ranges = changed_record_ranges(previous, bytes, stride);
        for range in ranges {
            if let Err(error) = device.write_buffer(id, range.start as u64, &bytes[range]) {
                // Partial native writes are not a valid comparison baseline.
                previous.clear();
                return Err(error.into());
            }
        }
        previous.clear();
        previous.extend_from_slice(bytes);
        Ok(id)
    }

    fn destroy<D: ResourceDevice>(mut self, device: &mut D) {
        if let Err(error) = self.outside.retire_bindings(device) {
            // Do not destroy buffers while a descriptor could still own them.
            log::debug!("scroll outside binding retirement failed: {error}");
            return;
        }
        for buffer in [self.global, self.sprites, self.static_animation, self.placements, self.projection.buffer].into_iter()
            .chain(self.outside.handles()).flatten() {
            if let Err(error) = device.destroy_buffer(buffer) {
                log::debug!("scroll composite slot retirement failed: {error}");
            }
        }
    }
}

// Consecutive instances on the same page share the native binding. Never
// merge across ordinary root or extension commands, and never reorder.
fn append_composite_tile(commands: &mut Vec<RenderCommand>, previous_tile: bool,
    pipeline: RenderPipelineId, group: BindGroupId, instance: u32) {
    if previous_tile && let Some(RenderCommand::Draw(previous)) = commands.last_mut()
        && previous.pipeline == pipeline && previous.bind_groups.as_slice() == [group]
        && previous.first_instance + previous.instance_count == instance
    {
        previous.instance_count += 1;
        return;
    }
    commands.push(RenderCommand::Draw(DrawCommand { pipeline, bind_groups: bind_group_list([group]),
        vertex_count: 4, first_vertex: 0, instance_count: 1, first_instance: instance, scissor: None }));
}

struct RasterPass {
    write: RasterTileWrite,
    target_extent: Extent2d,
    paged: bool,
    depth: TextureViewId,
    commands: Vec<RenderCommand>,
}

// All five native backends use a Replace draw for regional clearing.
// OpenGL requires its ordinary full-attachment viewport, not the legacy
// PixelExact sub-viewport mapping (which flips scissors independently).
fn shared_raster_pages<D: PresentationDevice>(device: &D, backend: Option<gfx_core::BackendKind>) -> bool {
    device.exact_offscreen_region_clear_supported()
        || matches!(backend, Some(gfx_core::BackendKind::Metal | gfx_core::BackendKind::OpenGl))
}

struct PhysicalRasterPass {
    view: TextureViewId,
    extent: Extent2d,
    depth: TextureViewId,
    paged: bool,
    commands: Vec<RenderCommand>,
}

fn coalesce_raster_pages(passes: impl IntoIterator<Item = PhysicalRasterPass>) -> Vec<PhysicalRasterPass> {
    let mut physical = Vec::<PhysicalRasterPass>::new();
    let mut pages = FxHashMap::default();
    for mut pass in passes {
        if pass.paged && let Some(&index) = pages.get(&pass.view) {
            let target: &mut PhysicalRasterPass = &mut physical[index];
            debug_assert_eq!(target.extent, pass.extent);
            debug_assert_eq!(target.depth, pass.depth);
            target.commands.append(&mut pass.commands);
            continue;
        }
        if pass.paged { pages.insert(pass.view, physical.len()); }
        physical.push(pass);
    }
    physical
}

impl NovaRenderer {
    fn retained_outside_root(&mut self, plans: &[LayerPlan<'_>], scene: &crate::Scene) -> Option<Arc<OutsideRoot>> {
        let ranges = outside_ranges(plans, scene.len())?;
        let epoch = outside_atlas_epoch(self);
        if let Some(cache) = &mut self.scroll_outside_root
            && cache.root.matches_ranges(&scene.paint_operations, &ranges)
        {
            if cache.atlas_epoch.is_none_or(|previous| previous == epoch) {
                return Some(Arc::clone(&cache.root));
            }
            // Unrelated allocations, uploads and relocations must not
            // discard the encoded root. While uploads are pending, let the
            // ordinary renderer publish them without losing this encoding.
            let root = Arc::clone(&cache.root);
            let Some(matches) = self.atlas.root_placements_match(&root.atlas_tiles) else {
                return None;
            };
            if matches && root.atlas_tiles.iter().all(|tile| self.gpu_atlas_textures.contains_key(&tile.texture_id)) {
                if outside_atlas_epoch(self) != epoch { return None; }
                self.scroll_outside_root.as_mut()?.atlas_epoch = Some(epoch);
                return Some(root);
            }
        }
        // A changed placement or source must be packed again; never keep
        // invalid physical UVs or stale extension inputs.
        self.scroll_outside_root = None;
        let cache = OutsideRoot::pack(self, &ranges, scene)?;
        let root = Arc::clone(&cache.root);
        self.scroll_outside_root = Some(cache);
        Some(root)
    }

    pub(super) fn destroy_scroll_composite_slots(&mut self) {
        // An unknown reader or failed fence is not safe to reclaim. The
        // device registry retains the allocations until device teardown.
        if self.scroll_raster_failed || !self.pending_submissions.is_empty()
            || !self.scroll_raster_resources.is_empty() || self.raster_tiles.bytes() != 0
        {
            return;
        }
        let slots = std::mem::take(&mut self.scroll_composite_slots);
        if slots.is_empty() { return; }
        match &mut *lock_backend(&self.backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => for slot in slots { slot.destroy(device); },
            #[cfg(all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")))]
            NovaBackend::OpenGl(device) => for slot in slots { slot.destroy(device); },
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => for slot in slots { slot.destroy(device); },
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => for slot in slots { slot.destroy(device); },
            #[cfg(all(feature = "nova-gfx-vulkan", any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
            NovaBackend::Vulkan(device) => for slot in slots { slot.destroy(device); },
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    pub(super) fn retire_scroll_raster_resources(&mut self, submission: SubmissionId) {
        let mut i = 0;
        while i < self.scroll_raster_resources.len() {
            if self.scroll_raster_resources[i].0 != submission {
                i += 1;
                continue;
            }
            let (_, resources) = self.scroll_raster_resources.swap_remove(i);
            match &mut *lock_backend(&self.backend) {
                #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
                NovaBackend::Dx11(device) => resources.destroy(device),
                #[cfg(all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")))]
                NovaBackend::OpenGl(device) => resources.destroy(device),
                #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
                NovaBackend::Dx12(device) => resources.destroy(device),
                #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
                NovaBackend::Metal(device) => resources.destroy(device),
                #[cfg(all(feature = "nova-gfx-vulkan", any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
                NovaBackend::Vulkan(device) => resources.destroy(device),
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }
    }

    /// Both ordinary and direct rendering must describe the same atlas
    /// content and text raster ABI. Atlas allocation/placement changes
    /// alone do not invalidate pixels already rasterized into a tile.
    pub(super) fn scroll_tile_context(&self, scale: f32) -> (u64, Option<retained_upload::StaticStreamToken>, u32) {
        let text = pack_text_raster_params(&self.rendering_parameters);
        (self.atlas.scroll_damage_generation(), Some(retained_upload::StaticStreamToken::from_bytes(&text)), scale.to_bits())
    }

    /// Reconcile the original logical Paint in both presentation paths.
    /// Newly allocated Atlas glyphs must not evict unrelated cached pixels.
    /// A same-ID refresh invalidates only layers that sample that identity.
    pub(super) fn reconcile_scroll_residency(&mut self, paint: &crate::window::Paint) {
        let scale = paint.geometry.scale_factor;
        let context = self.scroll_tile_context(scale);
        self.raster_tiles.begin_frame();
        if self.raster_tiles.bytes() != 0 {
            let dirty = match self.raster_tile_context {
                Some(previous) if previous.1 == context.1 && previous.2 == context.2 => {
                    self.atlas.scroll_damage_since(previous.0, context.0)
                }
                _ => None,
            };
            if let Some(dirty) = dirty {
                // Filter once per source. Only dirty sources need a
                // geometric dependency query, and only on a damage epoch.
                let layers: FxHashMap<_, _> = paint.layers.frames.iter()
                    .map(|layer| (layer.id, (layer, !dirty.is_empty() && layer.raster_atlas_tiles().iter()
                        .any(|(_, id)| dirty.contains(id))))).collect();
                self.raster_tiles.reconcile_tiles(|key, generation, raster| {
                    let (layer, affected) = layers.get(&key.layer)?;
                    let x = i64::from(raster.origin.x);
                    let y = i64::from(raster.origin.y);
                    let rect = [x, y, x + i64::from(raster.extent.width()),
                        y + i64::from(raster.extent.height())];
                    let adopted = layer.adopt_tile(generation, rect, scale)?;
                    if *affected && layer.raster_tile_uses_atlas_damage(rect, &dirty) {
                        return None;
                    }
                    Some(adopted)
                });
            } else {
                self.raster_tiles.invalidate_all();
            }
        }
        self.raster_tile_context = Some(context);
    }

    pub(super) fn try_scroll_raster_present(&mut self, packet: &mut PresentationPacket) -> Result<Option<bool>> {
        // A None submission is NOT a completed fence. All admitted backends
        // must return a tracked writer from the ordered offscreen API.
        if packet.paint.layers.frames.is_empty() {
            self.scroll_outside_root = None;
            return Ok(None);
        }
        if self.scroll_raster_failed || self.live_resize_active
            || self.presentation_submission_mode() == GpuSubmissionMode::Synchronous
            || self.surface_format != Format::Bgra8Unorm
            || !packet.presentation_animation_values.is_empty()
            || !packet.presentation_animation_timelines.is_empty()
        {
            return Ok(None);
        }
        let plans = layer_plan(&packet.paint.layers.frames, packet.paint.scene.len(), self.active_size);
        if plans.is_empty() { return Ok(None); }
        // All logical operations must be covered by exact admitted plans.
        // A nested overlap, tile budget limit or uncovered viewport must
        // fall back before any writer or present is issued.
        if packet.paint.scene.paint_operations.has_deferred_scroll()
            && !all_deferred_scrolls_planned(&packet.paint.scene, &plans)
        {
            return Ok(None);
        }
        // Keep the normal atlas retirement path reachable even during a long
        // run of direct presentations. Only scan dependencies when removals exist.
        if self.atlas.has_pending_removals() {
            let mut live = FxHashSet::default();
            packet.paint.collect_atlas_tile_ids_into(&mut live);
            if self.atlas.has_retirable_pending_removals(&live) { return Ok(None); }
        }
        let backend = Arc::clone(&self.backend);
        // All five native backends implement explicit ordered offscreen
        // fences. No independent graphics queue is used for scroll tiles.
        let supported = match &*lock_backend(&backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(_) => true,
            #[cfg(all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")))]
            NovaBackend::OpenGl(_) => true,
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(_) => true,
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(_) => true,
            #[cfg(all(feature = "nova-gfx-vulkan", any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
            NovaBackend::Vulkan(_) => true,
            #[allow(unreachable_patterns)]
            _ => false,
        };
        if !supported || !lock_backend(&backend).async_capabilities().async_presentation {
            return Ok(None);
        }
        self.poll_pending_submissions()?;
        let Ok(slot) = self.next_available_frame_resource_index() else {
            self.submission_retry_pending = true;
            return Ok(Some(false));
        };
        // Use the exact same text raster ABI as ordinary Scene encoding.
        // Changing monitor parameters must invalidate cached glyph coverage.
        #[cfg(target_os = "windows")]
        if packet.dirty_region.is_full() && self.rendering_parameters.refresh_monitor_parameters() {
            self.retained_upload.invalidate_encode_key();
        }
        self.reconcile_scroll_residency(&packet.paint);
        let result = match &mut *lock_backend(&backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => self.scroll_raster_present(device, &plans, &packet.paint.scene, slot, packet.frame_time),
            #[cfg(all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")))]
            NovaBackend::OpenGl(device) => self.scroll_raster_present(device, &plans, &packet.paint.scene, slot, packet.frame_time),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => self.scroll_raster_present(device, &plans, &packet.paint.scene, slot, packet.frame_time),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => self.scroll_raster_present(device, &plans, &packet.paint.scene, slot, packet.frame_time),
            #[cfg(all(feature = "nova-gfx-vulkan", any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
            NovaBackend::Vulkan(device) => self.scroll_raster_present(device, &plans, &packet.paint.scene, slot, packet.frame_time),
            #[allow(unreachable_patterns)]
            _ => Ok(None),
        };
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(error) => {
                // An unclassified native failure cannot certify any tiles.
                self.scroll_raster_failed = true;
                self.raster_tiles.invalidate_all();
                self.retained_frame_valid = false;
                return Err(error);
            }
        };
        if let Some(presented) = outcome {
            self.root_color.invalidate();
            self.filters.invalidate();
            self.path_mask_residency.invalidate();
            self.retained_upload.invalidate_encode_key();
            self.invalidate_command_cache();
            self.retained_frame_valid = presented;
            if presented {
                self.submitted_frames = self.submitted_frames.saturating_add(1);
                self.swapchain_warmup_frames = self.swapchain_warmup_frames.saturating_sub(1);
                self.submission_retry_pending = false;
                crate::diagnostics::performance_metrics::record_present();
                crate::diagnostics::performance_metrics::record_direct_present();
                packet.consume_submitted_damage();
            } else {
                self.submission_retry_pending = true;
            }
        }
        Ok(outcome)
    }

    fn scroll_raster_present<D: ResourceDevice + PresentationDevice + SurfaceDevice + PipelineDevice + SubmissionDevice>(
        &mut self, device: &mut D, plans: &[LayerPlan<'_>], scene: &crate::Scene, slot: usize,
        frame_time: Instant,
    ) -> Result<Option<bool>> {
        let Some(outside) = self.retained_outside_root(plans, scene) else { return Ok(None); };
        let format = self.surface_format;
        let original = self.frame_resources[slot].buffers;
        if self.submitted_frames % 120 == 0 {
            self.raster_tiles.trim(device, 120)?;
        }
        // All writes for these tiles are issued on OrderedGraphics by this
        // owner. Accept a pending lease only if its exact writer is tracked.
        let requests: Vec<_> = plans.iter().flat_map(|plan| plan.requests.iter().map(move |request| (plan.layer, request))).collect();
        let mut resident_bindings = SmallVec::<[Option<BindGroupId>; 64]>::new();
        let mut reads: Vec<Option<RasterTileRead>> = requests.iter().map(|(layer, request)| {
            let resolved = self.raster_tiles.lookup_ordered_with_binding(
                request.key, layer.generation, request.raster, format, slot,
                |writer| self.pending_submissions.iter().any(|pending| pending.submission == writer || pending.predecessors.contains(&writer)));
            resident_bindings.push(resolved.and_then(|(_, binding)| binding));
            resolved.map(|(read, _)| read)
        }).collect();
        let misses = reads.iter().filter(|read| read.is_none()).count();
        let mut new_writer = None;
        if self.scroll_composite_slots.len() <= slot {
            self.scroll_composite_slots.resize_with(slot + 1, CompositeSlot::default);
        }
        if misses != 0 {
            let mut resources = Resources::default();
            let mut passes = Vec::<RasterPass>::with_capacity(misses);
            let prepare = (|| -> Result<bool> {
                let mut projection = ProjectionArena::new(device, &mut self.scroll_composite_slots[slot].projection, misses * 2)?;
                let mut depth_views = Vec::<(Extent2d, TextureViewId)>::new();
                let mut clear_bindings = FxHashMap::<(u32, u32, [u32; 4]), BindGroupId>::default();
                let paged = shared_raster_pages(device, self.backend_info.kind().ok());
                let mut demand = FxHashMap::<(u32, u32), usize>::default();
                if paged {
                    for ((_, request), read) in requests.iter().zip(&reads) {
                        if read.is_none() && let Some(class) = gfx_core::RasterTileCache::page_size_class(request.raster.extent) {
                            *demand.entry(class).or_default() += 1;
                        }
                    }
                }
                let mut offset = 0;
                for plan in plans {
                    let layer = plan.layer;
                    let end = offset + plan.requests.len();
                    let layer_reads = &reads[offset..end];
                    offset = end;
                    let Some(first) = layer_reads.iter().position(Option::is_none) else { continue; };
                    let request = &plan.requests[first];
                    let x = i64::from(request.raster.origin.x);
                    let y = i64::from(request.raster.origin.y);
                    let (operations, _) = layer.raster_source_for_tile([x, y,
                        x + i64::from(request.raster.extent.width()), y + i64::from(request.raster.extent.height())])
                        .context("missing immutable scroll source")?;
                    // Native placement admission proves that the captured
                    // paint order produces the same pixels as a sorted Scene.
                    let background = layer.raster_background();
                    let placements = if layer.raster_atlas_tiles().is_empty() {
                        FxHashMap::default()
                    } else {
                        let ids = layer.raster_atlas_tiles();
                        let placements = if background.is_some() {
                            self.atlas.ready_root_glyphs(ids)
                        } else {
                            self.atlas.ready_scroll_glyphs(ids)
                        };
                        let Some(placements) = placements else {
                            return Ok(false);
                        };
                        placements
                    };
                    let mut streams = StaticStreams { root_lcd: background.is_some(), ..Default::default() };
                    for operation in operations.iter() {
                        match operation {
                            PaintOperation::Primitive(primitive) => {
                                if primitive.bounds().intersect(&primitive.content_mask().bounds).is_empty() {
                                    continue;
                                }
                                let Some(primitive) = resolve_primitive(primitive, &placements, &self.gpu_atlas_textures) else { return Ok(false); };
                                if streams.push(&primitive, false).is_none() { return Ok(false); }
                            }
                            PaintOperation::StartLayer(_) | PaintOperation::EndLayer => {}
                            _ => return Ok(false),
                        }
                    }
                    if streams.runs.is_empty() { return Ok(false); }
                    let source_buffers = streams.upload(device, &mut resources, original,
                        &pack_text_raster_params(&self.rendering_parameters), None)?;
                    for (request, read) in plan.requests.iter().zip(layer_reads) {
                        if read.is_some() { continue; }
                        let raster = RasterTile {
                            origin: SignedOrigin2d { x: request.source_origin[0], y: request.source_origin[1] },
                            extent: request.raster.extent,
                        };
                        let write = if paged {
                            let remaining = gfx_core::RasterTileCache::page_size_class(request.raster.extent)
                                .and_then(|class| demand.get_mut(&class)).map_or(1, |count| {
                                    let remaining = *count;
                                    *count = count.saturating_sub(1);
                                    remaining
                                });
                            self.raster_tiles.prepare_paged_with_demand(device, request.key, layer.generation, request.raster, format, remaining)?
                        } else {
                            self.raster_tiles.prepare(device, request.key, layer.generation, request.raster, format)?
                        };
                        let Some(write) = write else { return Ok(false); };
                        let target_extent = write.target_extent();
                        let region = write.region();
                        let paged = target_extent != raster.extent;
                        // Map original capture coordinates into the page's
                        // physical pixels. Full-page viewport keeps vertex
                        // and fragment origin arithmetic in the same space.
                        let prepared = (|| -> Result<_> {
                            let globals = pack_global_params(DrawableSize { width: target_extent.width(), height: target_extent.height() },
                                background.is_none(), [raster.origin.x as f32 - region.x as f32, raster.origin.y as f32 - region.y as f32]);
                            let (global_buffer, global_offset) = projection.push(device, &mut resources, &globals)?;
                            let buffers = FrameResourceBuffers { global_buffer, ..source_buffers };
                            let blend = if background.is_some() { self.pipelines.alpha } else { self.pipelines.premultiplied };
                            let mut commands = streams.commands(self, device, &mut resources, buffers, global_offset, blend, &[], None)?;
                            if paged {
                                for command in &mut commands {
                                    let RenderCommand::Draw(draw) = command else {
                                        anyhow::bail!("non-raster command in a shared scroll page");
                                    };
                                    draw.scissor = Some(region);
                                }
                            }
                            if paged || background.is_some() {
                                // Reuse one clear binding per physical extent
                                // and color. Scissor selects the exact cell.
                                let color = background.unwrap_or_default();
                                let class = (target_extent.width(), target_extent.height(),
                                    [color.r, color.g, color.b, color.a].map(f32::to_bits));
                                let group = if let Some(&group) = clear_bindings.get(&class) { group } else {
                                    let bounds = crate::bounds(crate::point(ScaledPixels(0.), ScaledPixels(0.)),
                                        crate::size(ScaledPixels(class.0 as f32), ScaledPixels(class.1 as f32)));
                                    let mut clear = Vec::with_capacity(PACKED_QUAD_BYTES);
                                    write_quad(&mut clear, &Quad { bounds, content_mask: ContentMask::new(bounds),
                                        background: color.into(), ..Default::default() });
                                    let clear_buffer = resources.buffer(device, &clear, BufferUsage::STORAGE)?;
                                    let globals = pack_global_params(DrawableSize { width: class.0, height: class.1 }, true, [0., 0.]);
                                    let (global_buffer, global_offset) = projection.push(device, &mut resources, &globals)?;
                                    let clear_buffers = FrameResourceBuffers { global_buffer, quad_buffer: clear_buffer, quad_capacity: 1, ..buffers };
                                    let mut entries = quad_resource_bindings(&clear_buffers);
                                    projection_binding_offset(&mut entries, global_offset);
                                    let group = resources.binding(device, self.quad_bind_group_layout, entries)?;
                                    clear_bindings.insert(class, group);
                                    group
                                };
                                commands.insert(0, RenderCommand::Draw(DrawCommand {
                                    pipeline: self.pipelines.retained_clear, bind_groups: bind_group_list([group]),
                                    vertex_count: 4, first_vertex: 0, instance_count: 1, first_instance: 0,
                                    scissor: Some(region),
                                }));
                            }
                            let depth = if let Some((_, view)) = depth_views.iter().find(|(extent, _)| *extent == target_extent) { *view } else {
                                let target = create_depth_target(device, "scroll raster", target_extent)?;
                                resources.depths.push(target);
                                // Depth32Float, one sample and mip. Count each
                                // physical allocation once, not per tile/pass.
                                resources.gpu_bytes = resources.gpu_bytes.saturating_add(
                                    u64::from(target_extent.width()) * u64::from(target_extent.height()) * 4);
                                depth_views.push((target_extent, target.1));
                                target.1
                            };
                            Ok((depth, commands))
                        })();
                        match prepared {
                            Ok((depth, commands)) => passes.push(RasterPass { write, target_extent, paged, depth, commands }),
                            Err(error) => { self.raster_tiles.cancel_write(write)?; return Err(error); }
                        }
                    }
                }
                projection.upload(device)?;
                Ok(true)
            })();
            if !matches!(prepare, Ok(true)) {
                for pass in passes { self.raster_tiles.cancel_write(pass.write)?; }
                resources.destroy(device);
                return prepare.map(|_| None);
            }
            // Independent cells never sample another scroll page while
            // rasterizing. Their producer order commutes, so gather by
            // physical attachment; keep logical reservations for fences.
            let physical = coalesce_raster_pages(passes.iter_mut().map(|pass| PhysicalRasterPass {
                view: pass.write.texture().view, extent: pass.target_extent, depth: pass.depth,
                paged: pass.paged, commands: std::mem::take(&mut pass.commands),
            }));
            let descriptors: Vec<_> = physical.iter().map(|pass| RenderPassCommands {
                color_attachments: std::slice::from_ref(&pass.view), render_pass: self.render_pass,
                commands: &pass.commands,
                color_load_op: if pass.paged { LoadOp::Load } else { LoadOp::Clear(clear_color()) },
                clear_region: None,
                depth_attachment: Some(DepthAttachment { target: pass.depth,
                    depth_load_op: LoadOp::Clear(1.), stencil_load_op: None }),
                // Projection already maps content into attachment pixels.
                // The full attachment viewport is implicit; in GL this
                // selects the same row-zero convention as texture uploads,
                // sampling and ordinary full-target draws.
                viewport: None, resolve_target: None,
            }).collect();
            log::trace!(target: "gpui::diagnostics", "scroll raster producers: logical_writes={} physical_passes={}", passes.len(), physical.len());
            let tags = vec![None; descriptors.len()];
            let submitted = device.render_to_textures_on_queue(&descriptors, &tags, OffscreenQueuePreference::OrderedGraphics);
            drop(descriptors);
            let writer = match submitted {
                Ok(Some(writer)) => writer,
                // Unknown acceptance: do not cancel reservations or destroy
                // native handles that may have reached the queue. Disable the
                // fast path; the device registry retains their ownership.
                Ok(None) => anyhow::bail!("ordered scroll raster returned no native fence"),
                Err(error) => return Err(error.into()),
            };
            self.pending_submissions.push(PendingSubmission { submission: writer,
                predecessors: SmallVec::new(), frame_resource_index: slot, publication: None });
            self.scroll_raster_resources.push((writer, resources));
            new_writer = Some(writer);
            for pass in passes { self.raster_tiles.finish_write(pass.write, Some(writer))?; }
            for ((layer, request), read) in requests.iter().zip(&mut reads) {
                if read.is_none() {
                    *read = self.raster_tiles.lookup_ordered(request.key, layer.generation, request.raster, format,
                        |dependency| dependency == writer);
                }
            }
        }
        let reads: Vec<_> = reads.into_iter().collect::<Option<Vec<_>>>().context("incomplete scroll tile set")?;
        // All decline/fallback decisions precede extension preparation.
        // From here a custom producer may accept native work; errors retain
        // the exact registered fences, rather than preparing it a second time.
        let mut extension_steps = Vec::new();
        self.prepare_renderer_extensions_on(&outside.extensions, &mut extension_steps, frame_time, device)?;
        // No cache reservation, eviction or external producer occurs on
        // this all-hit static path. The combined lookup's native binding
        // identities remain valid. Mixed/miss paths revalidate every lease.
        let reuse_lookup_bindings = misses == 0 && outside.extensions.is_empty();
        // Without extensions every source run emits exactly one command.
        // The immutable root already owns the correct boundaries.
        let expanded;
        let boundaries = if outside.extensions.is_empty() {
            outside.boundaries.as_slice()
        } else {
            expanded = expanded_boundaries(&outside.streams.runs, &outside.boundaries, &extension_steps)
                .context("invalid mixed root command boundaries")?;
            expanded.as_slice()
        };
        let mut resources = Resources::default();
        let prepare = (|| -> Result<Arc<[RenderCommand]>> {
            let composite_slot = &mut self.scroll_composite_slots[slot];
            let layer_slots = composite_slot.layer_slots.reconcile(plans.iter().map(|plan| plan.layer.id))?;
            let instances = composite_slot.tile_slots.reconcile(requests.iter().map(|(_, request)| request.key))?;
            let mut placement_bytes = composite_slot.placement_bytes.clone();
            let mut changes = Vec::new();
            let mut offset = 0;
            for (plan, &layer_slot) in plans.iter().zip(&layer_slots) {
                let (viewport, translation) = plan.layer.root_static_raster().context("missing scroll viewport")?;
                let start = layer_slot as usize * PACKED_ANIMATION_VALUE_BYTES;
                let end = start + PACKED_ANIMATION_VALUE_BYTES;
                if placement_bytes.len() < end { placement_bytes.resize(end, 0); }
                let mut record = Vec::with_capacity(PACKED_ANIMATION_VALUE_BYTES);
                append_placement(&mut record, viewport, translation);
                placement_bytes[start..end].copy_from_slice(&record);
                for (index, request) in plan.requests.iter().enumerate() {
                    debug_assert_eq!(request.bounds.origin, crate::point(
                        ScaledPixels((request.raster.origin.x + translation[0]) as f32),
                        ScaledPixels((request.raster.origin.y + translation[1]) as f32)));
                    let instance = instances[offset + index] as usize;
                    let record = CompositeRecord { layer_slot: layer_slot + 1, raster: request.raster,
                        region: reads[offset + index].region() };
                    if composite_slot.sprite_records.get(instance).and_then(|entry| *entry) != Some(record)
                        || composite_slot.sprite_bytes.len() < (instance + 1) * PACKED_POLY_SPRITE_BYTES {
                        changes.push((instance, record));
                    }
                }
                offset += plan.requests.len();
            }
            let placement_buffer = CompositeSlot::write_changed(device, &mut composite_slot.placements,
                &mut composite_slot.placement_bytes, &placement_bytes, MAX_TILES * PACKED_ANIMATION_VALUE_BYTES,
                PACKED_ANIMATION_VALUE_BYTES, BufferUsage::STORAGE)?;
            let premultiplied = self.surface_alpha.outputs_premultiplied_alpha();
            let globals = pack_global_params(self.active_size, premultiplied, [0., 0.]);
            let global_buffer = CompositeSlot::write_changed(device, &mut composite_slot.global,
                &mut composite_slot.global_bytes, &globals, GLOBAL_UPLOAD_BYTES, globals.len(), BufferUsage::UNIFORM)?;
            let poly_sprite_buffer = if changes.is_empty() && let Some(buffer) = composite_slot.sprites {
                buffer
            } else {
                // Sparse slots retain their bytes even while unreachable.
                // Reordering or inserting a row never moves surviving records.
                let mut bytes = composite_slot.sprite_bytes.clone();
                let mut encoded = Vec::with_capacity(PACKED_POLY_SPRITE_BYTES);
                for &(instance, record) in &changes {
                    let start = instance * PACKED_POLY_SPRITE_BYTES;
                    let end = start + PACKED_POLY_SPRITE_BYTES;
                    if bytes.len() < end { bytes.resize(end, 0); }
                    encoded.clear();
                    record.encode(&mut encoded);
                    bytes[start..end].copy_from_slice(&encoded);
                }
                let buffer = CompositeSlot::write_changed(device, &mut composite_slot.sprites,
                    &mut composite_slot.sprite_bytes, &bytes, MAX_TILES * PACKED_POLY_SPRITE_BYTES,
                    PACKED_POLY_SPRITE_BYTES, BufferUsage::STORAGE)?;
                for (instance, record) in changes {
                    if composite_slot.sprite_records.len() <= instance {
                        composite_slot.sprite_records.resize(instance + 1, None);
                    }
                    composite_slot.sprite_records[instance] = Some(record);
                }
                buffer
            };
            let animation_value_buffer = if let Some(buffer) = composite_slot.static_animation {
                buffer
            } else {
                CompositeSlot::write(device, &mut composite_slot.static_animation,
                    &[0; PACKED_ANIMATION_VALUE_BYTES], PACKED_ANIMATION_VALUE_BYTES, BufferUsage::STORAGE)?
            };
            let buffers = FrameResourceBuffers {
                global_buffer, poly_sprite_buffer, poly_sprite_capacity: MAX_TILES,
                animation_value_buffer, animation_value_capacity: 1, ..original
            };
            let tile_buffers = FrameResourceBuffers { animation_value_buffer: placement_buffer,
                animation_value_capacity: MAX_TILES, ..buffers };
            let pipeline = if premultiplied { self.pipelines.premultiplied.poly_sprites } else { self.pipelines.alpha.poly_sprites };
            // Resolve live sampling leases before testing command identity.
            // An evicted/recreated view cannot reuse a stale descriptor ID.
            let mut tiles = Vec::with_capacity(reads.len());
            for ((&instance, read), &resident) in instances.iter().zip(&reads).zip(&resident_bindings) {
                let group = if reuse_lookup_bindings && let Some(group) = resident {
                    group
                } else {
                    self.raster_tiles.sampling_binding(device, *read, slot, || BindGroupDescriptor {
                        label: Some("resident scroll composite binding".into()),
                        layout: self.poly_sprite_bind_group_layout,
                        entries: poly_atlas_resource_bindings(tile_buffers, read.texture().view, self.atlas_sampler),
                    })?
                };
                tiles.push((instance, group));
            }
            let unchanged = std::ptr::eq(composite_slot.outside.root.as_ptr(), Arc::as_ptr(&outside));
            let cacheable = outside.extensions.is_empty() && outside.streams.streams_resident();
            // The same flattened instances may be partitioned differently
            // around root runs. Include both root and tile span ends.
            let mut end = 0;
            let command_boundaries: Vec<_> = plans.iter().zip(boundaries).map(|(plan, &boundary)| {
                end += plan.requests.len();
                (boundary, end)
            }).collect();
            if unchanged && cacheable && let Some(cached) = &composite_slot.commands
                && cached.matches(&outside, pipeline, &tiles, &command_boundaries) {
                // Text raster parameters can change without replacing the
                // immutable root or its descriptors. Refresh the private
                // uniform before reusing the resident command list.
                if !outside.streams.mono.is_empty() {
                    let text = pack_text_raster_params(&self.rendering_parameters);
                    CompositeSlot::write_changed(device, &mut composite_slot.outside.text,
                        &mut composite_slot.outside.text_bytes, &text, text.len(),
                        text.len(), BufferUsage::UNIFORM)?;
                }
                return Ok(Arc::clone(&cached.commands));
            }
            composite_slot.commands = None;
            if !unchanged {
                composite_slot.outside.retire_bindings(device)?;
            }
            // A failed write must never certify the next use of this slot.
            composite_slot.outside.root = std::sync::Weak::new();
            let root_buffers = outside.streams.upload(device, &mut resources, buffers,
                &pack_text_raster_params(&self.rendering_parameters), Some((&mut composite_slot.outside, unchanged)))?;
            composite_slot.outside.root = Arc::downgrade(&outside);
            let blend = if premultiplied { self.pipelines.premultiplied } else { self.pipelines.alpha };
            let mut resident = std::mem::take(&mut composite_slot.outside.bindings);
            let root_commands = outside.streams.commands(self, device, &mut resources, root_buffers, 0, blend, &extension_steps, Some(&mut resident));
            self.scroll_composite_slots[slot].outside.bindings = resident;
            let root_commands = root_commands?;
            let mut commands = Vec::with_capacity(root_commands.len() + reads.len());
            let mut root = root_commands.into_iter();
            let mut root_offset = 0;
            let mut tile_offset = 0;
            let mut previous_tile = false;
            for (plan, &boundary) in plans.iter().zip(boundaries) {
                if boundary != root_offset { previous_tile = false; }
                commands.extend(root.by_ref().take(boundary - root_offset));
                root_offset = boundary;
                let end = tile_offset + plan.requests.len();
                for &(instance, group) in &tiles[tile_offset..end] {
                    append_composite_tile(&mut commands, previous_tile, pipeline, group, instance);
                    previous_tile = true;
                }
                tile_offset = end;
            }
            commands.extend(root);
            let commands: Arc<[RenderCommand]> = commands.into();
            if cacheable {
                self.scroll_composite_slots[slot].commands = Some(ResidentCompositeCommands {
                    root: Arc::downgrade(&outside), pipeline, tiles,
                    boundaries: command_boundaries, commands: Arc::clone(&commands),
                });
            }
            Ok(commands)
        })();
        let commands = match prepare {
            Ok(commands) => commands,
            Err(error) => { resources.destroy(device); return Err(error); }
        };
        if let Err(error) = device.set_swapchain_source_extent(self.swapchain,
            Extent2d::new(self.active_size.width, self.active_size.height)?) {
            resources.destroy(device);
            return Err(error.into());
        }
        // Submit custom native producers using exactly the ordinary
        // renderer's Graphics/Compute contract, before the root consumer.
        let extension_submission = if !outside.extensions.is_empty() {
            match self.submit_prepared_extensions(device, slot) {
                Ok(submission) => submission,
                Err(error) => { resources.destroy(device); return Err(error); }
            }
        } else { None };
        // Preserve root prefix -> resident tiles -> root suffix. The
        // promoted fragment is never encoded or drawn on this path.
        let frame = PresentFrameDescriptor {
            swapchain: self.swapchain, render_pass: self.render_pass, commands: &commands,
            color_load_op: LoadOp::Clear(clear_color()), depth_attachment: Some(self.depth_attachment()),
            present_region: None,
        };
        // Only publish an owned identity for an actually resident command
        // stream. One-shot extensions/oversized streams must not pollute
        // backend native caches or retain their weak allocations.
        let resident = self.scroll_composite_slots[slot].commands.as_ref()
            .is_some_and(|cached| Arc::ptr_eq(&cached.commands, &commands));
        let presented = if resident {
            device.render_and_present_owned(&frame, &commands)
        } else {
            device.render_and_present_tracked(&frame)
        };
        let outcome = match presented {
            Ok(outcome) => outcome,
            Err(error) => {
                // DX11/GL may record native draws before Present or fence
                // creation fails. No accepted reader ID means no retirement
                // proof. Leave the views and transient bindings device-owned.
                for read in reads { self.raster_tiles.quarantine_reader(read); }
                return Err(error.into());
            }
        };
        if let Some(reader) = outcome.frame().and_then(|frame| frame.submission) {
            // A producer and its consumer occupy one presentation slot.
            // Keep the producer independently pollable as a predecessor;
            // do not count it as a second frame in the pacing controller.
            let mut predecessors = SmallVec::new();
            if let Some(writer) = new_writer {
                if let Some(index) = self.pending_submissions.iter().position(|entry| entry.submission == writer) {
                    self.pending_submissions.remove(index);
                    predecessors.push(writer);
                }
            }
            self.pending_submissions.push(PendingSubmission { submission: reader,
                predecessors, frame_resource_index: slot, publication: None });
            self.scroll_raster_resources.push((reader, resources));
            super::present::attach_extension_submission(&mut self.pending_submissions, extension_submission);
            self.renderer_registry.gpu_submission_accepted(reader);
            for read in reads { self.raster_tiles.record_ordered_reader(read, reader)?; }
        } else if outcome.frame().is_none() {
            resources.destroy(device);
        } else {
            for read in reads { self.raster_tiles.quarantine_reader(read); }
            anyhow::bail!("scroll present returned no native reader fence");
        }
        log::trace!(target: "gpui::diagnostics", "scroll raster present: layers={} tiles={} misses={} hits={} outside_runs={} logical_resident={} physical_textures={}",
            plans.len(), requests.len(), misses, requests.len() - misses, outside.streams.runs.len(),
            self.raster_tiles.logical_tiles(), self.raster_tiles.physical_textures());
        Ok(Some(outcome.was_presented()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(translation: [i32; 2], capture_top: f32, capture_height: f32) -> crate::window::Paint {
        let viewport = ContentMask::new(crate::bounds(crate::point(crate::px(0.), crate::px(0.)),
            crate::size(crate::px(128.), crate::px(512.))));
        let coverage = crate::bounds(crate::point(ScaledPixels(0.), ScaledPixels(capture_top)),
            crate::size(ScaledPixels(128.), ScaledPixels(capture_height)));
        let mut operations = crate::scene::Operations::default();
        operations.push(PaintOperation::Primitive(Primitive::Quad(Quad {
            bounds: viewport.scale(1.).bounds, content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        let mut paint = crate::window::Paint::default();
        paint.layers.raster_fixture(operations, coverage, viewport, translation);
        paint
    }

    #[test]
    fn resident_command_identity_requires_exact_order_and_native_bindings() {
        let root = Arc::new(OutsideRoot { source: crate::scene::Operations::default(),
            source_ends: Vec::new(), atlas_tiles: Vec::new(), streams: StaticStreams::default(),
            boundaries: Vec::new(), extensions: Vec::new() });
        let a = BindGroupId::from_parts(1, 1);
        let b = BindGroupId::from_parts(2, 1);
        let pipeline = RenderPipelineId::from_parts(1, 1);
        let cached = ResidentCompositeCommands { root: Arc::downgrade(&root), pipeline,
            tiles: vec![(0, a), (2, b)], boundaries: vec![(1, 1), (2, 2)],
            commands: Vec::<RenderCommand>::new().into() };
        assert!(cached.matches(&root, pipeline, &[(0, a), (2, b)], &[(1, 1), (2, 2)]));
        assert!(!cached.matches(&root, pipeline, &[(2, b), (0, a)], &[(1, 1), (2, 2)]));
        assert!(!cached.matches(&root, pipeline, &[(0, b), (2, b)], &[(1, 1), (2, 2)]));
        assert!(!cached.matches(&root, pipeline, &[(0, a), (2, b)], &[(0, 1), (2, 2)]));
        assert!(!cached.matches(&root, pipeline, &[(0, a), (2, b)], &[(1, 2), (2, 2)]));
        assert!(!cached.matches(&root, RenderPipelineId::from_parts(2, 1), &[(0, a), (2, b)], &[(1, 1), (2, 2)]));
    }

    #[test]
    fn cpu_capacity_counts_allocations_not_live_record_lengths() {
        let mut slot = CompositeSlot::default();
        assert_eq!(slot.cpu_capacity_bytes(), 0);
        slot.sprite_bytes.reserve(256);
        slot.placement_bytes.reserve(128);
        slot.sprite_records.reserve(8);
        let expected = vec_capacity_bytes(&slot.sprite_bytes)
            + vec_capacity_bytes(&slot.placement_bytes)
            + vec_capacity_bytes(&slot.sprite_records);
        assert_eq!(slot.cpu_capacity_bytes(), expected);
        slot.sprite_bytes.extend_from_slice(&[0; 16]);
        slot.sprite_bytes.clear();
        assert_eq!(slot.cpu_capacity_bytes(), expected);
    }

    #[test]
    fn stable_slots_preserve_survivors_on_insertion_reorder_and_removal() {
        let mut slots = StableSlots::<u64>::default();
        assert_eq!(slots.reconcile([10, 20, 30]).unwrap().as_slice(), &[0, 1, 2]);
        assert_eq!(slots.reconcile([40, 30, 10, 20]).unwrap().as_slice(), &[3, 2, 0, 1]);
        assert_eq!(slots.reconcile([40, 20, 50, 10]).unwrap().as_slice(), &[3, 1, 2, 0]);
        assert!(slots.reconcile([10, 10]).is_err());
        assert!(slots.reconcile([60, 60]).is_err());
        let reordered = slots.reconcile([10, 50, 20, 40]).unwrap();
        assert_eq!(reordered.as_slice(), &[0, 2, 1, 3]);
        assert!(!reordered.spilled(), "stable hit must stay in inline storage");
        assert_eq!(slots.reconcile([40, 10]).unwrap().as_slice(), &[3, 0]);
        assert_eq!(slots.reconcile([40, 70, 10]).unwrap().as_slice(), &[3, 1, 0]);
        assert!(slots.reconcile([]).unwrap().is_empty());
        assert_eq!(slots.reconcile([80]).unwrap().as_slice(), &[0]);
        let mut full = StableSlots::<u64>::default();
        assert_eq!(full.reconcile(0..64).unwrap().as_slice(), (0..64u32).collect::<Vec<_>>().as_slice());
        assert_eq!(full.reconcile((1..64).chain([64])).unwrap()[63], 0);
        assert!(full.reconcile(0..65).is_err());
    }

    #[test]
    fn projection_arena_uses_native_alignment_and_bounds_allocation() {
        let limits = gfx_core::BufferBindingLimits { uniform_offset_alignment: 256,
            max_uniform_binding_bytes: 64 * 1024, ..Default::default() };
        assert_eq!(projection_arena_layout(limits, 128), Some((256, 32768)));
        assert_eq!(projection_arena_layout(limits, 257), None);
        assert_eq!(projection_arena_layout(Default::default(), 4), None);
        let limits = gfx_core::BufferBindingLimits { uniform_offset_alignment: 512, ..limits };
        assert_eq!(projection_arena_layout(limits, 128), Some((512, 65536)));
        assert_eq!(projection_arena_layout(limits, 129), None);
    }

    #[test]
    fn placement_updates_leave_content_space_sprite_records_unchanged() {
        let raster = RasterTile { origin: SignedOrigin2d { x: 4, y: -64 }, extent: Extent2d::new(32, 64).unwrap() };
        let record = CompositeRecord { layer_slot: 1, raster, region: gfx_core::ScissorRect { x: 0, y: 64, width: 32, height: 64 } };
        let mut sprites = Vec::new();
        record.encode(&mut sprites);
        assert_eq!(sprites.len(), PACKED_POLY_SPRITE_BYTES);
        assert_eq!(&sprites[..4], &1u32.to_ne_bytes());
        let viewport = crate::bounds(crate::point(ScaledPixels(0.), ScaledPixels(0.)),
            crate::size(ScaledPixels(128.), ScaledPixels(256.)));
        let mut before = Vec::new();
        let mut after = Vec::new();
        append_placement(&mut before, viewport, [0, 64]);
        append_placement(&mut after, viewport, [0, 128]);
        assert_eq!(before.len(), PACKED_ANIMATION_VALUE_BYTES);
        assert_ne!(before, after);
        assert_eq!(&before[32..48], &after[32..48]);
        assert_eq!(&after[20..24], &128f32.to_ne_bytes());
        let mut same = Vec::new();
        record.encode(&mut same);
        assert_eq!(sprites, same);
    }

    #[test]
    fn page_passes_group_nonadjacent_cells_without_reordering_cell_commands() {
        let view = |id| TextureViewId::from_parts(id, 1);
        let pass = |id, instance, paged| {
            let mut commands = Vec::new();
            append_composite_tile(&mut commands, false, RenderPipelineId::from_parts(1, 1),
                BindGroupId::from_parts(1, 1), instance);
            PhysicalRasterPass { view: view(id), extent: Extent2d::new(64, 128).unwrap(),
                depth: view(9), paged, commands }
        };
        let groups = coalesce_raster_pages([pass(1, 0, true), pass(2, 1, true),
            pass(1, 2, true), pass(3, 3, false)]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups.iter().map(|p| p.view).collect::<Vec<_>>(), [view(1), view(2), view(3)]);
        assert_eq!(groups[0].commands.iter().map(|command| match command {
            RenderCommand::Draw(draw) => draw.first_instance,
            _ => unreachable!(),
        }).collect::<Vec<_>>(), [0, 2]);
        assert!(!groups[2].paged);
    }

    #[test]
    fn atlas_damage_intersects_capture_space_not_composite_space() {
        let viewport = ContentMask::new(crate::bounds(crate::point(crate::px(0.), crate::px(0.)),
            crate::size(crate::px(128.), crate::px(512.))));
        let coverage = crate::bounds(crate::point(ScaledPixels(0.), ScaledPixels(-512.)),
            crate::size(ScaledPixels(128.), ScaledPixels(1536.)));
        let glyph_bounds = crate::bounds(crate::point(ScaledPixels(8.), ScaledPixels(300.)),
            crate::size(ScaledPixels(16.), ScaledPixels(20.)));
        let tile = AtlasTile { texture_id: AtlasTextureId { index: 0, kind: AtlasTextureKind::Monochrome },
            tile_id: crate::TileId(42), padding: 0,
            bounds: crate::bounds(crate::point(DevicePixels(0), DevicePixels(0)),
                crate::size(DevicePixels(16), DevicePixels(20))) };
        let mut ops = crate::scene::Operations::default();
        ops.push(PaintOperation::Primitive(Primitive::MonochromeSprite(crate::MonochromeSprite {
            order: 0, pad: 0, animation_id: None, bounds: glyph_bounds,
            content_mask: ContentMask::new(coverage), color: crate::rgb(0xffffff), tile,
            transformation: crate::TransformationMatrix::unit(),
        })));
        let mut paint = crate::window::Paint::default();
        paint.layers.raster_fixture(ops, coverage, viewport, [0, 64]);
        let layer = &paint.layers.frames[0];
        let dirty = [42].into_iter().collect();
        assert!(!layer.raster_tile_uses_atlas_damage([0, -256, 128, 0], &dirty));
        assert!(layer.raster_tile_uses_atlas_damage([0, 0, 128, 256], &dirty));
        // Capture-space glyph ends at y=320. The two-pixel guard can
        // affect the adjoining tile until y=322.
        assert!(layer.raster_tile_uses_atlas_damage([0, 256, 128, 512], &dirty));
        assert!(!layer.raster_tile_uses_atlas_damage([0, 258, 128, 512], &dirty));
        assert!(!layer.raster_tile_uses_atlas_damage([0, 0, 128, 256], &[43].into_iter().collect()));
    }

    #[test]
    fn signed_tiles_use_stable_content_keys_and_original_capture_projection() {
        let paint = fixture([0, 64], -512., 1536.);
        let layer = &paint.layers.frames[0];
        let plan = requests(layer, DrawableSize { width: 128, height: 512 }).unwrap();
        assert_eq!(plan.len(), 3);
        assert_eq!(plan[0].key.y, -1);
        assert_eq!(plan[0].raster.origin.y, -256);
        assert_eq!(plan[0].source_origin, [0, -192]);
        assert_eq!(plan[0].bounds.top(), ScaledPixels(-192.));
        assert_eq!(plan[1].key.y, 0);
        assert_eq!(plan[2].bounds.top(), ScaledPixels(320.));
        assert_eq!(plan[2].bounds.bottom(), ScaledPixels(576.));
    }

    #[test]
    fn multiple_scroll_layers_preserve_root_gaps_and_reject_nested_duplicate_draws() {
        let viewport = ContentMask::new(crate::bounds(crate::point(crate::px(0.), crate::px(0.)),
            crate::size(crate::px(128.), crate::px(512.))));
        let coverage = crate::bounds(crate::point(ScaledPixels(0.), ScaledPixels(-512.)),
            crate::size(ScaledPixels(128.), ScaledPixels(1536.)));
        let quad = Quad { bounds: viewport.scale(1.).bounds, content_mask: viewport.scale(1.),
            ..Default::default() };
        let mut painter = crate::scene::Painter::default();
        for _ in 0..7 { painter.insert_primitive(quad); }
        painter.finish();
        let scene = painter.into_scene();
        let mut paint = crate::window::Paint::default();
        for span in [4..6, 1..3, 1..2] {
            let mut source = crate::scene::Operations::default();
            source.push(PaintOperation::Primitive(Primitive::Quad(Quad {
                content_mask: ContentMask::new(coverage), ..quad
            })));
            paint.layers.raster_fixture(source, coverage, viewport, [0, 0]);
            paint.layers.certify_fixture_root(&scene, span);
        }
        let plans = layer_plan(&paint.layers.frames, scene.len(), DrawableSize { width: 128, height: 512 });
        assert_eq!(plans.iter().map(|p| (p.layer.id, p.span.clone())).collect::<Vec<_>>(),
            vec![(2, 1..3), (1, 4..6)]);
        assert_eq!(outside_ranges(&plans, scene.len()), Some(vec![0..1, 3..4, 6..7]));
        assert_eq!(plans.iter().map(|p| p.requests.len()).sum::<usize>(), 4);
        assert_ne!(plans[0].requests[0].key, plans[1].requests[0].key);
    }

    #[test]
    fn compact_native_row_uses_one_fixed_texture_when_clipped_by_list_edge() {
        let viewport = ContentMask::new(crate::bounds(crate::point(crate::px(0.), crate::px(0.)),
            crate::size(crate::px(128.), crate::px(512.))));
        let coverage = crate::bounds(crate::point(ScaledPixels(0.), ScaledPixels(-512.)),
            crate::size(ScaledPixels(128.), ScaledPixels(1536.)));
        let bounds = crate::bounds(crate::point(ScaledPixels(8.), ScaledPixels(24.)),
            crate::size(ScaledPixels(100.), ScaledPixels(32.)));
        let mut ops = crate::scene::Operations::default();
        ops.push(PaintOperation::Primitive(Primitive::Quad(Quad {
            bounds, content_mask: ContentMask::new(coverage), ..Default::default()
        })));
        let placement = crate::window::scroll_paint::native_row_fixture(ops, coverage, viewport,
            crate::point(ScaledPixels(0.), ScaledPixels(-32.)));
        let mut painter = crate::scene::Painter::default();
        assert!(painter.push_deferred_scroll(placement));
        painter.finish();
        let scene = painter.into_scene();
        let layers = crate::window::SceneLayers::capture(&scene, None, true);
        let layer = &layers.frames[0];
        let requests = requests(layer, DrawableSize { width: 128, height: 512 }).unwrap();
        assert_eq!(requests.len(), 1);
        let row = &requests[0];
        assert_eq!(row.source_origin, [6, 22]);
        assert_eq!(row.raster.extent, Extent2d::new(104, 36).unwrap());
        assert_eq!(row.bounds.top(), ScaledPixels(-10.));
        assert_eq!(row.bounds.bottom(), ScaledPixels(26.));
        assert_eq!(layer.root_static_raster().unwrap().0, viewport.scale(1.).bounds);
        assert_eq!(layer.adopt_tile(1, [6, 22, 110, 58], 1.), Some(1));
    }

    #[test]
    fn composite_batches_only_adjacent_instances_with_the_same_page_binding() {
        let pipeline = RenderPipelineId::from_parts(1, 1);
        let a = BindGroupId::from_parts(1, 1);
        let b = BindGroupId::from_parts(2, 1);
        let mut commands = Vec::new();
        append_composite_tile(&mut commands, false, pipeline, a, 0);
        append_composite_tile(&mut commands, true, pipeline, a, 1);
        append_composite_tile(&mut commands, true, pipeline, b, 2);
        append_composite_tile(&mut commands, false, pipeline, b, 3);
        append_composite_tile(&mut commands, true, pipeline, b, 5);
        assert_eq!(commands.iter().map(|command| match command {
            RenderCommand::Draw(draw) => (draw.first_instance, draw.instance_count),
            _ => unreachable!(),
        }).collect::<Vec<_>>(), vec![(0, 2), (2, 1), (3, 1), (5, 1)]);
    }

    #[test]
    fn composite_upload_merges_only_changed_record_intervals() {
        let previous = [1, 1, 2, 2, 3, 3, 4, 4];
        assert!(changed_record_ranges(&previous, &previous, 2).is_empty());
        assert_eq!(changed_record_ranges(&previous, &[1, 1, 9, 9, 8, 8, 4, 4], 2), vec![2..6]);
        assert_eq!(changed_record_ranges(&previous, &[0, 0, 2, 2, 0, 0, 4, 4], 2), vec![0..2, 4..6]);
        assert_eq!(changed_record_ranges(&previous[..4], &previous, 2), vec![4..8]);
        assert!(changed_record_ranges(&previous, &previous[..4], 2).is_empty());
    }

    #[test]
    fn outside_root_reuses_exact_ranges_but_not_changed_insertion_boundaries() {
        let mut source = crate::scene::Operations::default();
        let quad = || PaintOperation::Primitive(Primitive::Quad(Quad::default()));
        source.push(quad());
        source.push(quad());
        source.push(quad());
        source.finish();
        let mut retained = crate::scene::Operations::default();
        retained.share(&source, 0..1);
        retained.share(&source, 2..3);
        let root = OutsideRoot { source: retained, source_ends: vec![1, 2], atlas_tiles: Vec::new(),
            streams: StaticStreams::default(), boundaries: vec![1], extensions: vec![] };
        assert!(root.matches_ranges(&source, &[0..1, 2..3]));
        assert!(!root.matches_ranges(&source, &[0..2, 3..3]));
        assert!(!root.matches_ranges(&source, &[0..3]));
    }

    #[test]
    fn extension_command_expansion_preserves_tile_insertion_boundaries() {
        let runs = [
            SourceRun { kind: SourceKind::Quad, first: 0, count: 1 },
            SourceRun { kind: SourceKind::Extension, first: 0, count: 1 },
            SourceRun { kind: SourceKind::Extension, first: 1, count: 1 },
            SourceRun { kind: SourceKind::Quad, first: 1, count: 1 },
        ];
        // Zero-draw extensions are valid, including consecutive boundaries.
        assert_eq!(expanded_boundaries(&runs, &[0, 1, 2, 3, 4], &[vec![], vec![]]),
            Some(vec![0, 1, 1, 1, 2]));
        assert!(expanded_boundaries(&runs, &[2, 4], &[vec![]]).is_none());
    }

    #[test]
    fn static_streams_keep_kind_order_and_never_merge_across_promoted_span() {
        let quad = Primitive::Quad(Quad::default());
        let shadow = Primitive::Shadow(crate::Shadow::default());
        let mut streams = StaticStreams::default();
        streams.push(&shadow, false).unwrap();
        streams.push(&quad, false).unwrap();
        streams.push(&quad, false).unwrap();
        streams.push(&quad, true).unwrap();
        streams.push(&shadow, false).unwrap();
        assert_eq!(streams.runs.iter().map(|run| (run.kind, run.first, run.count)).collect::<Vec<_>>(),
            vec![(SourceKind::Shadow, 0, 1), (SourceKind::Quad, 0, 2),
                 (SourceKind::Quad, 2, 1), (SourceKind::Shadow, 1, 1)]);
    }

    #[test]
    fn cannot_sample_unproven_edge_pixels() {
        let paint = fixture([0, 64], 0., 512.);
        assert!(requests(&paint.layers.frames[0], DrawableSize { width: 128, height: 512 }).is_none());
    }

    #[test]
    fn viewport_outside_drawable_is_not_admitted() {
        let paint = fixture([0, 0], -512., 1536.);
        assert!(requests(&paint.layers.frames[0], DrawableSize { width: 127, height: 512 }).is_none());
    }
}
