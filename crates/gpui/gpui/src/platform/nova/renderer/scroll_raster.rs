//! Native direct scroll rasterization. The initial admission covers a whole
//! ordered static quad roots, with ordinary quads before/after a promoted layer.
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
    let (viewport, translation) = layer.root_quad_raster()?;
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
    let extent = Extent2d::new((right - left) as u32, height as u32).ok()?;
    let mut result = Vec::with_capacity((last - first + 1) as usize);
    for row in first..=last {
        let y = row.checked_mul(height)?;
        let rect = [i64::from(content_left), i64::from(y),
            i64::from(content_left) + i64::from(extent.width()), i64::from(y) + i64::from(height)];
        let (_, source_origin) = layer.raster_source_for_tile(rect)?;
        result.push(Request {
            key: RasterTileKey { layer: layer.id, x: content_left, y: row },
            raster: RasterTile { origin: SignedOrigin2d { x: content_left, y }, extent },
            source_origin,
            bounds: crate::bounds(
                crate::point(ScaledPixels(left as f32), ScaledPixels(y.checked_add(dy)? as f32)),
                crate::size(ScaledPixels(extent.width() as f32), ScaledPixels(height as f32))),
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
    depth: Option<(TextureId, TextureViewId)>,
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
        if let Some((texture, view)) = self.depth {
            destroy_depth_target(device, texture, view, "scroll raster");
        }
    }
}

// Only the original root prefix/suffix are read here. No projected layer
// primitive is touched on a hit. The proof lives on the published LayerFrame;
// this temporary upload is not a retained CPU scene or coordination cache.
struct OutsideQuads {
    bytes: Vec<u8>,
    prefix: u32,
    suffix: u32,
}

impl OutsideQuads {
    fn pack(layer: &crate::window::LayerFrame, scene: &crate::Scene) -> Option<Self> {
        let ranges = layer.root_quad_outside()?;
        let mut bytes = Vec::new();
        let mut counts = [0_u32; 2];
        for (range, count) in ranges.iter().zip(&mut counts) {
            for operation in scene.paint_operations.get_range(range.clone())?.iter() {
                match operation {
                    PaintOperation::Primitive(Primitive::Quad(quad)) => {
                        if bytes.len() / PACKED_QUAD_BYTES >= MAX_QUADS { return None; }
                        write_quad(&mut bytes, quad);
                        *count += 1;
                    }
                    PaintOperation::StartLayer(_) | PaintOperation::EndLayer => {}
                    _ => return None,
                }
            }
        }
        Some(Self { bytes, prefix: counts[0], suffix: counts[1] })
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
        for buffer in [self.global, self.sprites, self.outside].into_iter().flatten() {
            if let Err(error) = device.destroy_buffer(buffer) {
                log::debug!("scroll composite slot retirement failed: {error}");
            }
        }
    }
}

struct RasterPass {
    write: RasterTileWrite,
    raster: RasterTile,
    commands: Vec<RenderCommand>,
}

impl NovaRenderer {
    pub(super) fn destroy_scroll_composite_slots(&mut self) {
        // An unknown reader or failed fence is not safe to reclaim. The
        // device registry retains the allocations until device teardown.
        if self.scroll_raster_failed || !self.pending_submissions.is_empty()
            || !self.scroll_raster_resources.is_empty()
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

    pub(super) fn try_scroll_raster_present(&mut self, packet: &mut PresentationPacket) -> Result<Option<bool>> {
        // A None submission is NOT a completed fence. All admitted backends
        // must return a tracked writer from the ordered offscreen API.
        if self.scroll_raster_failed || self.live_resize_active
            || self.presentation_submission_mode() == GpuSubmissionMode::Synchronous
            || self.surface_format != Format::Bgra8Unorm
            || !packet.presentation_animation_values.is_empty()
            || !packet.presentation_animation_timelines.is_empty()
            || !self.frame_upload.renderer_extensions.is_empty()
            || packet.paint.layers.frames.is_empty()
        {
            return Ok(None);
        }
        let Some(layer) = packet.paint.layers.frames.iter().find(|layer| layer.root_quad_raster().is_some()) else {
            return Ok(None);
        };
        let Some(requests) = requests(layer, self.active_size) else { return Ok(None); };
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
        self.raster_tiles.begin_frame();
        self.raster_tiles.reconcile_tiles(|key, generation, raster| {
            let layer = packet.paint.layers.frames.iter().find(|layer| layer.id == key.layer)?;
            let x = i64::from(raster.origin.x);
            let y = i64::from(raster.origin.y);
            layer.adopt_tile(generation, [x, y, x + i64::from(raster.extent.width()),
                y + i64::from(raster.extent.height())], packet.paint.geometry.scale_factor)
        });
        let result = match &mut *lock_backend(&backend) {
            #[cfg(all(feature = "nova-gfx-dx11", target_os = "windows"))]
            NovaBackend::Dx11(device) => self.scroll_raster_present(device, layer, &packet.paint.scene, &requests, slot),
            #[cfg(all(feature = "nova-gfx-opengl", any(target_os = "windows", target_os = "linux")))]
            NovaBackend::OpenGl(device) => self.scroll_raster_present(device, layer, &packet.paint.scene, &requests, slot),
            #[cfg(all(feature = "nova-gfx-dx12", target_os = "windows"))]
            NovaBackend::Dx12(device) => self.scroll_raster_present(device, layer, &packet.paint.scene, &requests, slot),
            #[cfg(all(feature = "nova-gfx-metal", target_os = "macos"))]
            NovaBackend::Metal(device) => self.scroll_raster_present(device, layer, &packet.paint.scene, &requests, slot),
            #[cfg(all(feature = "nova-gfx-vulkan", any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
            NovaBackend::Vulkan(device) => self.scroll_raster_present(device, layer, &packet.paint.scene, &requests, slot),
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

    fn scroll_raster_present<D: ResourceDevice + PresentationDevice + SurfaceDevice>(
        &mut self, device: &mut D, layer: &crate::window::LayerFrame, scene: &crate::Scene, requests: &[Request], slot: usize,
    ) -> Result<Option<bool>> {
        let Some(outside) = OutsideQuads::pack(layer, scene) else { return Ok(None); };
        let format = self.surface_format;
        let original = self.frame_resources[slot].buffers;
        if self.submitted_frames % 120 == 0 {
            self.raster_tiles.trim(device, 120)?;
        }
        // All writes for these tiles are issued on OrderedGraphics by this
        // owner. Accept a pending lease only if its exact writer is tracked.
        let mut reads: Vec<Option<RasterTileRead>> = requests.iter().map(|request| {
            self.raster_tiles.lookup_ordered(request.key, layer.generation, request.raster, format,
                |writer| self.pending_submissions.iter().any(|pending| pending.submission == writer || pending.predecessors.contains(&writer)))
        }).collect();
        let misses = reads.iter().filter(|read| read.is_none()).count();
        let mut new_writer = None;
        if misses != 0 {
            let mut resources = Resources::default();
            let mut passes = Vec::<RasterPass>::with_capacity(misses);
            let prepare = (|| -> Result<bool> {
                let first = reads.iter().position(Option::is_none).unwrap();
                let request = &requests[first];
                let x = i64::from(request.raster.origin.x);
                let y = i64::from(request.raster.origin.y);
                let (operations, _) = layer.raster_source_for_tile([x, y,
                    x + i64::from(request.raster.extent.width()), y + i64::from(request.raster.extent.height())])
                    .context("missing immutable scroll source")?;
                // Compile in original painter order once for this miss batch.
                // A hit never accesses these pages or the visible Scene.
                let mut bytes = Vec::new();
                for operation in operations.iter() {
                    match operation {
                        PaintOperation::Primitive(Primitive::Quad(quad)) => {
                            if bytes.len() / PACKED_QUAD_BYTES >= MAX_QUADS { return Ok(false); }
                            write_quad(&mut bytes, quad);
                        }
                        PaintOperation::StartLayer(_) | PaintOperation::EndLayer => {}
                        _ => return Ok(false),
                    }
                }
                if bytes.is_empty() { return Ok(false); }
                let quad_buffer = resources.buffer(device, &bytes, BufferUsage::STORAGE)?;
                let count = bytes.len() / PACKED_QUAD_BYTES;
                resources.depth = Some(create_depth_target(device, "scroll raster", request.raster.extent)?);
                for (request, read) in requests.iter().zip(&reads) {
                    if read.is_some() { continue; }
                    let raster = RasterTile {
                        origin: SignedOrigin2d { x: request.source_origin[0], y: request.source_origin[1] },
                        extent: request.raster.extent,
                    };
                    let globals = pack_global_params(DrawableSize { width: raster.extent.width(), height: raster.extent.height() },
                        true, [raster.origin.x as f32, raster.origin.y as f32]);
                    let global_buffer = resources.buffer(device, &globals, BufferUsage::UNIFORM)?;
                    let buffers = FrameResourceBuffers { global_buffer, quad_buffer, quad_capacity: count, ..original };
                    let group = create_quad_bind_group(device, "scroll raster", self.quad_bind_group_layout, &buffers)?;
                    resources.bindings.push(group);
                    let Some(write) = self.raster_tiles.prepare(device, request.key, layer.generation, request.raster, format)? else {
                        return Ok(false);
                    };
                    passes.push(RasterPass { write, raster, commands: vec![RenderCommand::Draw(DrawCommand {
                        pipeline: self.pipelines.premultiplied.quads,
                        bind_groups: bind_group_list([group]), vertex_count: 4, first_vertex: 0,
                        instance_count: count as u32, first_instance: 0, scissor: None,
                    })] });
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
                depth_attachment: Some(DepthAttachment { target: resources.depth.unwrap().1,
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
            for (request, read) in requests.iter().zip(&mut reads) {
                if read.is_none() {
                    *read = self.raster_tiles.lookup_ordered(request.key, layer.generation, request.raster, format,
                        |dependency| dependency == writer);
                }
            }
        }
        let reads: Vec<_> = reads.into_iter().collect::<Option<Vec<_>>>().context("incomplete scroll tile set")?;
        let (viewport, _) = layer.root_quad_raster().context("missing scroll viewport")?;
        if self.scroll_composite_slots.len() <= slot {
            self.scroll_composite_slots.resize_with(slot + 1, CompositeSlot::default);
        }
        let composite_slot = &mut self.scroll_composite_slots[slot];
        let mut resources = Resources::default();
        let prepare = (|| -> Result<Vec<RenderCommand>> {
            let mut bytes = Vec::with_capacity(requests.len() * PACKED_POLY_SPRITE_BYTES);
            for request in requests {
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
            let buffers = FrameResourceBuffers { global_buffer, poly_sprite_buffer, poly_sprite_capacity: MAX_TILES, ..original };
            let pipeline = if premultiplied { self.pipelines.premultiplied.poly_sprites } else { self.pipelines.alpha.poly_sprites };
            let mut commands = Vec::with_capacity(reads.len() + 2);
            let outside_draw = if outside.bytes.is_empty() { None } else {
                let quad_count = (outside.prefix + outside.suffix) as usize;
                let (quad_buffer, quad_capacity) = if quad_count <= RETAINED_OUTSIDE_QUADS {
                    (CompositeSlot::write(device, &mut composite_slot.outside, &outside.bytes,
                        RETAINED_OUTSIDE_QUADS * PACKED_QUAD_BYTES, BufferUsage::STORAGE)?, RETAINED_OUTSIDE_QUADS)
                } else {
                    // Bound long-lived slot storage. Large outside streams
                    // retain the existing transient allocation/fence path.
                    (resources.buffer(device, &outside.bytes, BufferUsage::STORAGE)?, quad_count)
                };
                let root_buffers = FrameResourceBuffers { quad_buffer, quad_capacity, ..buffers };
                let group = create_quad_bind_group(device, "scroll root outside", self.quad_bind_group_layout, &root_buffers)?;
                resources.bindings.push(group);
                let pipeline = if premultiplied { self.pipelines.premultiplied.quads } else { self.pipelines.alpha.quads };
                Some((pipeline, group))
            };
            let outside_command = |first_instance, instance_count| {
                let (pipeline, group) = outside_draw?;
                (instance_count != 0).then(|| RenderCommand::Draw(DrawCommand {
                    pipeline, bind_groups: bind_group_list([group]), vertex_count: 4, first_vertex: 0,
                    instance_count, first_instance, scissor: None,
                }))
            };
            commands.extend(outside_command(0, outside.prefix));
            for (index, read) in reads.iter().enumerate() {
                let group = resources.binding(device, self.poly_sprite_bind_group_layout,
                    poly_atlas_resource_bindings(buffers, read.texture().view, self.atlas_sampler))?;
                commands.push(RenderCommand::Draw(DrawCommand { pipeline, bind_groups: bind_group_list([group]),
                    vertex_count: 4, first_vertex: 0, instance_count: 1, first_instance: index as u32, scissor: None }));
            }
            commands.extend(outside_command(outside.prefix, outside.suffix));
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
            for read in reads { self.raster_tiles.record_ordered_reader(read, reader)?; }
        } else if outcome.frame().is_none() {
            resources.destroy(device);
        } else {
            for read in reads { self.raster_tiles.quarantine_reader(read); }
            anyhow::bail!("scroll present returned no native reader fence");
        }
        log::trace!(target: "gpui::diagnostics", "scroll raster present: tiles={} misses={} hits={} outside_quads={}",
            requests.len(), misses, requests.len() - misses, outside.prefix + outside.suffix);
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
