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
        // Row-backed captures own only visible pixels. Clip the two edge
        // strips exactly; full interior strips retain their stable grid key.
        // Overscan captures still demand the entire strip be available.
        let (y, end) = if layer.has_visible_row_source() {
            (grid_y.max(content_top), grid_end.min(content_bottom))
        } else { (grid_y, grid_end) };
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
}

impl Resources {
    fn buffer<D: ResourceDevice>(&mut self, device: &mut D, bytes: &[u8], usage: BufferUsage) -> Result<BufferId> {
        let buffer = device.create_buffer(&BufferDescriptor {
            label: Some("scroll raster stream".into()),
            size: bytes.len() as u64,
            usage: usage | BufferUsage::COPY_DST,
            memory_location: MemoryLocation::CpuToGpu,
        })?;
        self.buffers.push(buffer);
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
        mut buffers: FrameResourceBuffers, text: &[u8], quad_slot: Option<&mut Option<BufferId>>) -> Result<FrameResourceBuffers> {
        if !self.quads.is_empty() {
            let count = self.quads.len() / PACKED_QUAD_BYTES;
            if let Some(slot) = quad_slot.filter(|_| count <= RETAINED_OUTSIDE_QUADS) {
                buffers.quad_buffer = CompositeSlot::write(device, slot, &self.quads,
                    RETAINED_OUTSIDE_QUADS * PACKED_QUAD_BYTES, BufferUsage::STORAGE)?;
                buffers.quad_capacity = RETAINED_OUTSIDE_QUADS;
            } else {
                buffers.quad_buffer = resources.buffer(device, &self.quads, BufferUsage::STORAGE)?;
                buffers.quad_capacity = count;
            }
        }
        if !self.shadows.is_empty() {
            buffers.shadow_buffer = resources.buffer(device, &self.shadows, BufferUsage::STORAGE)?;
            buffers.shadow_capacity = self.shadows.len() / PACKED_SHADOW_BYTES;
        }
        if !self.underlines.is_empty() {
            buffers.underline_buffer = resources.buffer(device, &self.underlines, BufferUsage::STORAGE)?;
        }
        if !self.mono.is_empty() {
            buffers.mono_sprite_buffer = resources.buffer(device, &self.mono, BufferUsage::STORAGE)?;
            buffers.mono_sprite_capacity = self.mono.len() / PACKED_MONO_SPRITE_BYTES;
            buffers.text_raster_buffer = resources.buffer(device, text, BufferUsage::UNIFORM)?;
        }
        if !self.poly.is_empty() {
            buffers.poly_sprite_buffer = resources.buffer(device, &self.poly, BufferUsage::STORAGE)?;
            buffers.poly_sprite_capacity = self.poly.len() / PACKED_POLY_SPRITE_BYTES;
        }
        Ok(buffers)
    }

    fn commands<D: ResourceDevice>(&self, renderer: &NovaRenderer, device: &mut D,
        resources: &mut Resources, buffers: FrameResourceBuffers, blend: BlendPipelines,
        extension_steps: &[Vec<RenderCommand>]) -> Result<Vec<RenderCommand>> {
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
            let group = if let Some(&group) = bindings.get(&run.kind) { group } else {
                let (layout, entries) = match run.kind {
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
                let group = resources.binding(device, layout, entries)?;
                bindings.insert(run.kind, group);
                group
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
/// or its fallback. Atlas generations protect resolved physical UVs.
pub(super) struct OutsideRoot {
    source: crate::scene::Operations,
    source_ends: Vec<usize>,
    atlas_epoch: Option<[u64; 3]>,
    streams: StaticStreams,
    boundaries: Vec<usize>,
    extensions: Vec<crate::PaintRendererExtension>,
}

fn outside_atlas_epoch(renderer: &NovaRenderer) -> [u64; 3] {
    [renderer.atlas.placement_generation(), renderer.atlas.texture_set_generation(),
        renderer.atlas.scroll_content_generation()]
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

    fn pack(renderer: &NovaRenderer, ranges: &[std::ops::Range<usize>], scene: &crate::Scene) -> Option<Self> {
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
        Some(Self { source, source_ends, atlas_epoch, streams, boundaries, extensions })
    }
}

/// Small physical GPU streams owned by an existing presentation frame slot.
/// A free slot proves that all prior readers (and its writer) have retired.
/// Do not overwrite the normal renderer's cross-slot shared static streams.
#[derive(Default)]
pub(super) struct CompositeSlot {
    global: Option<BufferId>,
    sprites: Option<BufferId>,
    outside: Option<BufferId>,
    // Dedicated immutable animation sentinel; never bind a growable normal
    // renderer animation buffer into a resident tile descriptor.
    static_animation: Option<BufferId>,
}

const RETAINED_OUTSIDE_QUADS: usize = 256;

impl CompositeSlot {
    #[cfg(all(test, target_os = "windows"))]
    pub(super) fn handles(&self) -> [Option<BufferId>; 3] {
        [self.global, self.sprites, self.outside]
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

    fn destroy<D: ResourceDevice>(self, device: &mut D) {
        for buffer in [self.global, self.sprites, self.outside, self.static_animation].into_iter().flatten() {
            if let Err(error) = device.destroy_buffer(buffer) {
                log::debug!("scroll composite slot retirement failed: {error}");
            }
        }
    }
}

struct RasterPass {
    write: RasterTileWrite,
    raster: RasterTile,
    depth: TextureViewId,
    commands: Vec<RenderCommand>,
}

impl NovaRenderer {
    fn retained_outside_root(&mut self, plans: &[LayerPlan<'_>], scene: &crate::Scene) -> Option<Arc<OutsideRoot>> {
        let ranges = outside_ranges(plans, scene.len())?;
        let epoch = outside_atlas_epoch(self);
        if let Some(root) = &self.scroll_outside_root
            && root.atlas_epoch.is_none_or(|previous| previous == epoch)
            && root.matches_ranges(&scene.paint_operations, &ranges)
        {
            return Some(Arc::clone(root));
        }
        // A failed pack must not retain stale extension inputs or atlas UVs.
        self.scroll_outside_root = None;
        let root = Arc::new(OutsideRoot::pack(self, &ranges, scene)?);
        self.scroll_outside_root = Some(Arc::clone(&root));
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
        (self.atlas.scroll_content_generation(), Some(retained_upload::StaticStreamToken::from_bytes(&text)), scale.to_bits())
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
        let context = self.scroll_tile_context(packet.paint.geometry.scale_factor);
        if self.raster_tile_context != Some(context) {
            self.raster_tiles.invalidate_all();
            self.raster_tile_context = Some(context);
        }
        self.raster_tiles.begin_frame();
        // Row promotion can expose dozens of independent source IDs.
        // Reconcile cache entries in O(tiles + layers), not a nested scan.
        let layers: FxHashMap<_, _> = packet.paint.layers.frames.iter()
            .map(|layer| (layer.id, layer)).collect();
        self.raster_tiles.reconcile_tiles(|key, generation, raster| {
            let layer = layers.get(&key.layer)?;
            let x = i64::from(raster.origin.x);
            let y = i64::from(raster.origin.y);
            layer.adopt_tile(generation, [x, y, x + i64::from(raster.extent.width()),
                y + i64::from(raster.extent.height())], packet.paint.geometry.scale_factor)
        });
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
        let mut reads: Vec<Option<RasterTileRead>> = requests.iter().map(|(layer, request)| {
            self.raster_tiles.lookup_ordered(request.key, layer.generation, request.raster, format,
                |writer| self.pending_submissions.iter().any(|pending| pending.submission == writer || pending.predecessors.contains(&writer)))
        }).collect();
        let misses = reads.iter().filter(|read| read.is_none()).count();
        let mut new_writer = None;
        if misses != 0 {
            let mut resources = Resources::default();
            let mut passes = Vec::<RasterPass>::with_capacity(misses);
            let prepare = (|| -> Result<bool> {
                let mut depth_views = Vec::<(Extent2d, TextureViewId)>::new();
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
                    // Use the existing native Painter ordering for the immutable
                    // source, including equal-order type/atlas sorting. Only a miss
                    // builds this temporary stream; no persistent CPU Scene cache.
                    let placements = if layer.raster_atlas_tiles().is_empty() {
                        FxHashMap::default()
                    } else {
                        let Some(placements) = self.atlas.ready_scroll_glyphs(layer.raster_atlas_tiles()) else {
                            return Ok(false);
                        };
                        placements
                    };
                    let mut painter = crate::scene::Painter::default();
                    for operation in operations.iter() {
                        match operation {
                            PaintOperation::Primitive(primitive) => {
                                let Some(primitive) = resolve_primitive(primitive, &placements, &self.gpu_atlas_textures) else { return Ok(false); };
                                painter.insert_primitive(primitive);
                            }
                            PaintOperation::StartLayer(bounds) => painter.push_layer(*bounds),
                            PaintOperation::EndLayer => painter.pop_layer(),
                            _ => return Ok(false),
                        }
                    }
                    painter.finish();
                    let source = painter.into_scene();
                    let mut streams = StaticStreams::default();
                    // The same native batch iterator as the normal renderer.
                    for batch in source.batches() {
                        macro_rules! pack {
                            ($slice:expr, $kind:ident) => {
                                for value in $slice {
                                    if streams.push(&Primitive::$kind(*value), false).is_none() { return Ok(false); }
                                }
                            };
                        }
                        match batch {
                            crate::scene::PrimitiveBatch::Quads { quads, .. } => { pack!(quads, Quad); }
                            crate::scene::PrimitiveBatch::Shadows { shadows, .. } => { pack!(shadows, Shadow); }
                            crate::scene::PrimitiveBatch::Underlines(underlines) => { pack!(underlines, Underline); }
                            crate::scene::PrimitiveBatch::MonochromeSprites { sprites, .. } => { pack!(sprites, MonochromeSprite); }
                            crate::scene::PrimitiveBatch::PolychromeSprites { sprites, .. } => { pack!(sprites, PolychromeSprite); }
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
                        let globals = pack_global_params(DrawableSize { width: raster.extent.width(), height: raster.extent.height() },
                            true, [raster.origin.x as f32, raster.origin.y as f32]);
                        let global_buffer = resources.buffer(device, &globals, BufferUsage::UNIFORM)?;
                        let buffers = FrameResourceBuffers { global_buffer, ..source_buffers };
                        let commands = streams.commands(self, device, &mut resources, buffers, self.pipelines.premultiplied, &[])?;
                        let depth = if let Some((_, view)) = depth_views.iter().find(|(extent, _)| *extent == raster.extent) { *view } else {
                            let target = create_depth_target(device, "scroll raster", raster.extent)?;
                            resources.depths.push(target);
                            depth_views.push((raster.extent, target.1));
                            target.1
                        };
                        let Some(write) = self.raster_tiles.prepare(device, request.key, layer.generation, request.raster, format)? else {
                            return Ok(false);
                        };
                        passes.push(RasterPass { write, raster, depth, commands });
                    }
                }
                Ok(true)
            })();
            if !matches!(prepare, Ok(true)) {
                for pass in passes { self.raster_tiles.cancel_write(pass.write)?; }
                resources.destroy(device);
                return prepare.map(|_| None);
            }
            let views: Vec<_> = passes.iter().map(|pass| pass.write.texture().view).collect();
            let descriptors: Vec<_> = passes.iter().zip(&views).map(|(pass, view)| RenderPassCommands {
                color_attachments: std::slice::from_ref(view), render_pass: self.render_pass,
                commands: &pass.commands, color_load_op: LoadOp::Clear(clear_color()), clear_region: None,
                depth_attachment: Some(DepthAttachment { target: pass.depth,
                    depth_load_op: LoadOp::Clear(1.), stencil_load_op: None }),
                viewport: Some(pass.raster.viewport()), resolve_target: None,
            }).collect();
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
        let boundaries = expanded_boundaries(&outside.streams.runs, &outside.boundaries, &extension_steps)
            .context("invalid mixed root command boundaries")?;
        if self.scroll_composite_slots.len() <= slot {
            self.scroll_composite_slots.resize_with(slot + 1, CompositeSlot::default);
        }
        let mut resources = Resources::default();
        let prepare = (|| -> Result<Vec<RenderCommand>> {
            let composite_slot = &mut self.scroll_composite_slots[slot];
            let mut bytes = Vec::with_capacity(requests.len() * PACKED_POLY_SPRITE_BYTES);
            for (layer, request) in &requests {
                let (viewport, _) = layer.root_static_raster().context("missing scroll viewport")?;
                write_polychrome_sprite(&mut bytes, &PolychromeSprite {
                    order: 0, bounds: request.bounds, content_mask: ContentMask::new(viewport),
                    corner_radii: Default::default(), grayscale: false, opacity: 1., sampling: 2, animation_id: None,
                    tile: AtlasTile { texture_id: AtlasTextureId { index: u32::MAX, kind: AtlasTextureKind::Bgra },
                        tile_id: crate::TileId(u32::MAX), padding: 0,
                        bounds: crate::bounds(crate::point(DevicePixels(0), DevicePixels(0)),
                            crate::size(DevicePixels(request.raster.extent.width() as i32), DevicePixels(request.raster.extent.height() as i32))) },
                });
            }
            let premultiplied = self.surface_alpha.outputs_premultiplied_alpha();
            let globals = pack_global_params(self.active_size, premultiplied, [0., 0.]);
            let global_buffer = CompositeSlot::write(device, &mut composite_slot.global,
                &globals, GLOBAL_UPLOAD_BYTES, BufferUsage::UNIFORM)?;
            let poly_sprite_buffer = CompositeSlot::write(device, &mut composite_slot.sprites,
                &bytes, MAX_TILES * PACKED_POLY_SPRITE_BYTES, BufferUsage::STORAGE)?;
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
            let pipeline = if premultiplied { self.pipelines.premultiplied.poly_sprites } else { self.pipelines.alpha.poly_sprites };
            let root_buffers = outside.streams.upload(device, &mut resources, buffers,
                &pack_text_raster_params(&self.rendering_parameters), Some(&mut composite_slot.outside))?;
            let blend = if premultiplied { self.pipelines.premultiplied } else { self.pipelines.alpha };
            let root_commands = outside.streams.commands(self, device, &mut resources, root_buffers, blend, &extension_steps)?;
            let mut commands = Vec::with_capacity(root_commands.len() + reads.len());
            let mut root = root_commands.into_iter();
            let mut root_offset = 0;
            let mut tile_offset = 0;
            for (plan, &boundary) in plans.iter().zip(&boundaries) {
                commands.extend(root.by_ref().take(boundary - root_offset));
                root_offset = boundary;
                let end = tile_offset + plan.requests.len();
                for (index, read) in reads[tile_offset..end].iter().enumerate() {
                    let group = self.raster_tiles.sampling_binding(device, *read, slot, || BindGroupDescriptor {
                        label: Some("resident scroll composite binding".into()),
                        layout: self.poly_sprite_bind_group_layout,
                        entries: poly_atlas_resource_bindings(buffers, read.texture().view, self.atlas_sampler),
                    })?;
                    commands.push(RenderCommand::Draw(DrawCommand { pipeline, bind_groups: bind_group_list([group]),
                        vertex_count: 4, first_vertex: 0, instance_count: 1, first_instance: (tile_offset + index) as u32, scissor: None }));
                }
                tile_offset = end;
            }
            commands.extend(root);
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
        let outcome = match device.render_and_present_tracked(&PresentFrameDescriptor {
            swapchain: self.swapchain, render_pass: self.render_pass, commands: &commands,
            color_load_op: LoadOp::Clear(clear_color()), depth_attachment: Some(self.depth_attachment()),
            present_region: None,
        }) {
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
        log::trace!(target: "gpui::diagnostics", "scroll raster present: layers={} tiles={} misses={} hits={} outside_runs={}",
            plans.len(), requests.len(), misses, requests.len() - misses, outside.streams.runs.len());
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
    fn visible_row_tiles_clip_edges_without_changing_interior_grid() {
        let viewport = ContentMask::new(crate::bounds(crate::point(crate::px(0.), crate::px(0.)),
            crate::size(crate::px(128.), crate::px(512.))));
        let mut painter = crate::scene::Painter::default();
        painter.insert_primitive(Quad { bounds: viewport.scale(1.).bounds,
            content_mask: viewport.scale(1.), ..Default::default() });
        painter.finish();
        let scene = painter.into_scene();
        let mut paint = crate::window::Paint::default();
        paint.layers.visible_row_fixture(&scene, viewport, [0, 64]);
        let plan = requests(&paint.layers.frames[0], DrawableSize { width: 128, height: 512 }).unwrap();
        assert_eq!(plan.iter().map(|tile| (tile.key.y, tile.raster.origin.y, tile.raster.extent.height()))
            .collect::<Vec<_>>(), vec![(-1, -64, 64), (0, 0, 256), (1, 256, 192)]);
        assert_eq!(plan[0].source_origin, [0, 0]);
        assert_eq!(plan[1].source_origin, [0, 64]);
        assert_eq!(plan[2].source_origin, [0, 320]);
        assert_eq!(plan[0].bounds.top(), ScaledPixels(0.));
        assert_eq!(plan[2].bounds.bottom(), ScaledPixels(512.));
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
        let layers = crate::window::SceneLayers::capture_native(&scene, None);
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
        let root = OutsideRoot { source: retained, source_ends: vec![1, 2], atlas_epoch: None,
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
