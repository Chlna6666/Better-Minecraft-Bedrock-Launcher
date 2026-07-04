use crate::platform::blade::blade_renderer::{EmojiLayerParams, ShaderEmojiLayerData};
use crate::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ColorGlyphLayer, DevicePixels,
    GlyphRasterization, PlatformAtlas, Point, RenderGlyphParams, Size, platform::AtlasTextureList,
};
use anyhow::Result;
use blade_graphics as gpu;
use blade_util::{BufferBelt, BufferBeltDescriptor};
use collections::FxHashMap;
use etagere::BucketedAtlasAllocator;
use parking_lot::Mutex;
use std::{borrow::Cow, ops, sync::Arc};

pub(crate) struct BladeAtlas(Mutex<BladeAtlasState>);

struct PendingUpload {
    id: AtlasTextureId,
    bounds: Bounds<DevicePixels>,
    data: gpu::BufferPiece,
}

struct PendingEmojiLayer {
    target_id: AtlasTextureId,
    target_bounds: Bounds<DevicePixels>,
    clear_target: bool,
    layer: ColorGlyphLayer,
    mask_texture: gpu::Texture,
    mask_view: gpu::TextureView,
    mask_data: gpu::BufferPiece,
}

struct BladeAtlasState {
    gpu: Arc<gpu::Context>,
    upload_belt: BufferBelt,
    storage: BladeAtlasStorage,
    tiles_by_key: FxHashMap<AtlasKey, AtlasTile>,
    initializations: Vec<AtlasTextureId>,
    uploads: Vec<PendingUpload>,
    emoji_layers: Vec<PendingEmojiLayer>,
}

#[cfg(gles)]
unsafe impl Send for BladeAtlasState {}

impl BladeAtlasState {
    fn destroy(&mut self) {
        self.storage.destroy(&self.gpu);
        self.upload_belt.destroy(&self.gpu);
        for layer in self.emoji_layers.drain(..) {
            self.gpu.destroy_texture_view(layer.mask_view);
            self.gpu.destroy_texture(layer.mask_texture);
        }
    }
}

pub struct BladeTextureInfo {
    pub raw_view: gpu::TextureView,
}

impl BladeAtlas {
    pub(crate) fn new(gpu: &Arc<gpu::Context>) -> Self {
        BladeAtlas(Mutex::new(BladeAtlasState {
            gpu: Arc::clone(gpu),
            upload_belt: BufferBelt::new(BufferBeltDescriptor {
                memory: gpu::Memory::Upload,
                min_chunk_size: 0x10000,
                alignment: 64, // Vulkan `optimalBufferCopyOffsetAlignment` on Intel XE
            }),
            storage: BladeAtlasStorage::default(),
            tiles_by_key: Default::default(),
            initializations: Vec::new(),
            uploads: Vec::new(),
            emoji_layers: Vec::new(),
        }))
    }

    pub(crate) fn destroy(&self) {
        self.0.lock().destroy();
    }

    pub fn before_frame(
        &self,
        gpu_encoder: &mut gpu::CommandEncoder,
        emoji_pipeline: &gpu::RenderPipeline,
        emoji_sampler: gpu::Sampler,
    ) {
        let mut lock = self.0.lock();
        lock.flush(gpu_encoder, emoji_pipeline, emoji_sampler);
    }

    pub fn after_frame(&self, sync_point: &gpu::SyncPoint) {
        let mut lock = self.0.lock();
        lock.upload_belt.flush(sync_point);
    }

    pub fn texture_info(&self, id: AtlasTextureId) -> BladeTextureInfo {
        let lock = self.0.lock();
        let texture = &lock.storage[id];
        BladeTextureInfo {
            raw_view: texture.raw_view,
        }
    }
}

impl PlatformAtlas for BladeAtlas {
    fn ensure_tile_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let mut lock = self.0.lock();
        if let Some(tile) = lock.tiles_by_key.get(key) {
            Ok(Some(tile.clone()))
        } else {
            profiling::scope!("new tile");
            let Some((size, bytes)) = build()? else {
                return Ok(None);
            };
            let tile = lock.allocate(size, key.texture_kind());
            lock.upload_texture(tile.texture_id, tile.bounds, &bytes);
            lock.tiles_by_key.insert(key.clone(), tile.clone());
            Ok(Some(tile))
        }
    }

    fn refresh_tile_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let mut lock = self.0.lock();
        if let Some(tile) = lock.tiles_by_key.get(key).cloned()
            && tile.bounds.size == size
        {
            lock.upload_texture(tile.texture_id, tile.bounds, &bytes);
            return Ok(Some(tile));
        }
        drop(lock);
        self.remove(key);
        self.ensure_tile_with(key, &mut || Ok(Some((size, bytes.clone()))))
    }

    fn ensure_glyph_with(
        &self,
        params: &RenderGlyphParams,
        build: &mut dyn FnMut() -> Result<GlyphRasterization>,
    ) -> Result<Option<AtlasTile>> {
        let key = AtlasKey::Glyph(params.clone());
        let mut lock = self.0.lock();
        if let Some(tile) = lock.tiles_by_key.get(&key) {
            return Ok(Some(tile.clone()));
        }
        match build()? {
            GlyphRasterization::Bitmap { size, bytes } => {
                let tile = lock.allocate(size, key.texture_kind());
                lock.upload_texture(tile.texture_id, tile.bounds, &bytes);
                lock.tiles_by_key.insert(key, tile.clone());
                Ok(Some(tile))
            }
            GlyphRasterization::ColorLayers {
                size,
                layers,
                fallback,
            } => {
                if layers.is_empty() {
                    let tile = lock.allocate(fallback.size, key.texture_kind());
                    lock.upload_texture(tile.texture_id, tile.bounds, &fallback.bytes);
                    lock.tiles_by_key.insert(key, tile.clone());
                    return Ok(Some(tile));
                }
                let tile = lock.allocate(size, AtlasTextureKind::Bgra);
                lock.queue_emoji_layers(tile.texture_id, tile.bounds, layers)?;
                lock.tiles_by_key.insert(key, tile.clone());
                Ok(Some(tile))
            }
        }
    }

    fn clear_glyphs(&self) {
        let keys = {
            let lock = self.0.lock();
            lock.tiles_by_key
                .keys()
                .filter(|key| matches!(key, AtlasKey::Glyph(_)))
                .cloned()
                .collect::<Vec<_>>()
        };
        for key in keys {
            self.remove(&key);
        }
    }

    fn remove(&self, key: &AtlasKey) {
        let mut lock = self.0.lock();

        let Some(tile) = lock.tiles_by_key.remove(key) else {
            return;
        };
        let id = tile.texture_id;

        let Some(texture_slot) = lock.storage[id.kind].textures.get_mut(id.index as usize) else {
            return;
        };

        if let Some(mut texture) = texture_slot.take() {
            texture.deallocate(tile.tile_id);
            texture.decrement_ref_count();
            if texture.is_unreferenced() {
                lock.storage[id.kind]
                    .free_list
                    .push(texture.id.index as usize);
                texture.destroy(&lock.gpu);
            } else {
                *texture_slot = Some(texture);
            }
        }
    }
}

impl BladeAtlasState {
    fn allocate(&mut self, size: Size<DevicePixels>, texture_kind: AtlasTextureKind) -> AtlasTile {
        {
            let textures = &mut self.storage[texture_kind];

            if let Some(tile) = textures
                .iter_mut()
                .rev()
                .find_map(|texture| texture.allocate(size))
            {
                return tile;
            }
        }

        let texture = self.push_texture(size, texture_kind);
        texture.allocate(size).unwrap()
    }

    fn push_texture(
        &mut self,
        min_size: Size<DevicePixels>,
        kind: AtlasTextureKind,
    ) -> &mut BladeAtlasTexture {
        const DEFAULT_ATLAS_SIZE: Size<DevicePixels> = Size {
            width: DevicePixels(1024),
            height: DevicePixels(1024),
        };

        let size = min_size.max(&DEFAULT_ATLAS_SIZE);
        let format;
        let usage;
        match kind {
            AtlasTextureKind::Monochrome => {
                format = gpu::TextureFormat::R8Unorm;
                usage = gpu::TextureUsage::COPY | gpu::TextureUsage::RESOURCE;
            }
            AtlasTextureKind::Bgra => {
                format = gpu::TextureFormat::Bgra8Unorm;
                usage = gpu::TextureUsage::COPY
                    | gpu::TextureUsage::RESOURCE
                    | gpu::TextureUsage::TARGET;
            }
            AtlasTextureKind::Rgba => {
                format = gpu::TextureFormat::Rgba8Unorm;
                usage = gpu::TextureUsage::COPY | gpu::TextureUsage::RESOURCE;
            }
        }

        let raw = self.gpu.create_texture(gpu::TextureDesc {
            name: "atlas",
            format,
            size: gpu::Extent {
                width: size.width.into(),
                height: size.height.into(),
                depth: 1,
            },
            array_layer_count: 1,
            mip_level_count: 1,
            sample_count: 1,
            dimension: gpu::TextureDimension::D2,
            usage,
            external: None,
        });
        let raw_view = self.gpu.create_texture_view(
            raw,
            gpu::TextureViewDesc {
                name: "",
                format,
                dimension: gpu::ViewDimension::D2,
                subresources: &Default::default(),
            },
        );

        let texture_list = &mut self.storage[kind];
        let index = texture_list.free_list.pop();

        let atlas_texture = BladeAtlasTexture {
            id: AtlasTextureId {
                index: index.unwrap_or(texture_list.textures.len()) as u32,
                kind,
            },
            size,
            allocator: etagere::BucketedAtlasAllocator::new(device_size_to_etagere(size)),
            format,
            raw,
            raw_view,
            live_atlas_keys: 0,
        };

        self.initializations.push(atlas_texture.id);

        if let Some(ix) = index {
            texture_list.textures[ix] = Some(atlas_texture);
            texture_list.textures.get_mut(ix).unwrap().as_mut().unwrap()
        } else {
            texture_list.textures.push(Some(atlas_texture));
            texture_list.textures.last_mut().unwrap().as_mut().unwrap()
        }
    }

    fn upload_texture(&mut self, id: AtlasTextureId, bounds: Bounds<DevicePixels>, bytes: &[u8]) {
        let data = self.upload_belt.alloc_bytes(bytes, &self.gpu);
        self.uploads.push(PendingUpload { id, bounds, data });
    }

    fn queue_emoji_layers(
        &mut self,
        id: AtlasTextureId,
        target_bounds: Bounds<DevicePixels>,
        layers: Vec<ColorGlyphLayer>,
    ) -> Result<()> {
        for (ix, layer) in layers.into_iter().enumerate() {
            let texture = self.gpu.create_texture(gpu::TextureDesc {
                name: "emoji layer mask",
                format: gpu::TextureFormat::R8Unorm,
                size: gpu::Extent {
                    width: layer.bounds.size.width.0.max(1) as u32,
                    height: layer.bounds.size.height.0.max(1) as u32,
                    depth: 1,
                },
                array_layer_count: 1,
                mip_level_count: 1,
                sample_count: 1,
                dimension: gpu::TextureDimension::D2,
                usage: gpu::TextureUsage::COPY | gpu::TextureUsage::RESOURCE,
                external: None,
            });
            let view = self.gpu.create_texture_view(
                texture,
                gpu::TextureViewDesc {
                    name: "emoji layer mask view",
                    format: gpu::TextureFormat::R8Unorm,
                    dimension: gpu::ViewDimension::D2,
                    subresources: &Default::default(),
                },
            );
            let data = self.upload_belt.alloc_bytes(&layer.alpha, &self.gpu);
            self.emoji_layers.push(PendingEmojiLayer {
                target_id: id,
                target_bounds,
                clear_target: ix == 0,
                layer,
                mask_texture: texture,
                mask_view: view,
                mask_data: data,
            });
        }
        Ok(())
    }

    fn flush_initializations(&mut self, encoder: &mut gpu::CommandEncoder) {
        for id in self.initializations.drain(..) {
            let texture = &self.storage[id];
            encoder.init_texture(texture.raw);
        }
    }

    fn flush(
        &mut self,
        encoder: &mut gpu::CommandEncoder,
        emoji_pipeline: &gpu::RenderPipeline,
        emoji_sampler: gpu::Sampler,
    ) {
        self.flush_initializations(encoder);

        let mut transfers = encoder.transfer("atlas");
        for upload in self.uploads.drain(..) {
            let texture = &self.storage[upload.id];
            transfers.copy_buffer_to_texture(
                upload.data,
                upload.bounds.size.width.to_bytes(texture.bytes_per_pixel()),
                gpu::TexturePiece {
                    texture: texture.raw,
                    mip_level: 0,
                    array_layer: 0,
                    origin: [
                        upload.bounds.origin.x.into(),
                        upload.bounds.origin.y.into(),
                        0,
                    ],
                },
                gpu::Extent {
                    width: upload.bounds.size.width.into(),
                    height: upload.bounds.size.height.into(),
                    depth: 1,
                },
            );
        }
        for layer in &self.emoji_layers {
            transfers.copy_buffer_to_texture(
                layer.mask_data,
                layer.layer.bounds.size.width.to_bytes(1),
                gpu::TexturePiece {
                    texture: layer.mask_texture,
                    mip_level: 0,
                    array_layer: 0,
                    origin: [0, 0, 0],
                },
                gpu::Extent {
                    width: layer.layer.bounds.size.width.into(),
                    height: layer.layer.bounds.size.height.into(),
                    depth: 1,
                },
            );
        }
        drop(transfers);
        self.flush_emoji_layers(encoder, emoji_pipeline, emoji_sampler);
    }

    fn flush_emoji_layers(
        &mut self,
        encoder: &mut gpu::CommandEncoder,
        emoji_pipeline: &gpu::RenderPipeline,
        emoji_sampler: gpu::Sampler,
    ) {
        for layer in self.emoji_layers.drain(..) {
            encoder.init_texture(layer.mask_texture);
            let target = &self.storage[layer.target_id];
            let texture_size = target.size;
            let render_target = gpu::RenderTarget {
                view: target.raw_view,
                init_op: if layer.clear_target {
                    gpu::InitOp::Clear(gpu::TextureColor::TransparentBlack)
                } else {
                    gpu::InitOp::Load
                },
                finish_op: gpu::FinishOp::Store,
            };
            if let mut pass = encoder.render(
                "emoji layer",
                gpu::RenderTargetSet {
                    colors: &[render_target],
                    depth_stencil: None,
                },
            ) {
                let mut pipeline = pass.with(emoji_pipeline);
                pipeline.bind(
                    0,
                    &ShaderEmojiLayerData {
                        layer: EmojiLayerParams::new(
                            layer.target_bounds,
                            texture_size,
                            &layer.layer,
                        ),
                        t_sprite: layer.mask_view,
                        s_sprite: emoji_sampler,
                    },
                );
                pipeline.draw(0, 4, 0, 1);
            }
            self.gpu.destroy_texture_view(layer.mask_view);
            self.gpu.destroy_texture(layer.mask_texture);
        }
    }
}

#[derive(Default)]
struct BladeAtlasStorage {
    monochrome_textures: AtlasTextureList<BladeAtlasTexture>,
    polychrome_textures: AtlasTextureList<BladeAtlasTexture>,
    rgba_textures: AtlasTextureList<BladeAtlasTexture>,
}

impl ops::Index<AtlasTextureKind> for BladeAtlasStorage {
    type Output = AtlasTextureList<BladeAtlasTexture>;
    fn index(&self, kind: AtlasTextureKind) -> &Self::Output {
        match kind {
            crate::AtlasTextureKind::Monochrome => &self.monochrome_textures,
            crate::AtlasTextureKind::Bgra => &self.polychrome_textures,
            crate::AtlasTextureKind::Rgba => &self.rgba_textures,
        }
    }
}

impl ops::IndexMut<AtlasTextureKind> for BladeAtlasStorage {
    fn index_mut(&mut self, kind: AtlasTextureKind) -> &mut Self::Output {
        match kind {
            crate::AtlasTextureKind::Monochrome => &mut self.monochrome_textures,
            crate::AtlasTextureKind::Bgra => &mut self.polychrome_textures,
            crate::AtlasTextureKind::Rgba => &mut self.rgba_textures,
        }
    }
}

impl ops::Index<AtlasTextureId> for BladeAtlasStorage {
    type Output = BladeAtlasTexture;
    fn index(&self, id: AtlasTextureId) -> &Self::Output {
        let textures = match id.kind {
            crate::AtlasTextureKind::Monochrome => &self.monochrome_textures,
            crate::AtlasTextureKind::Bgra => &self.polychrome_textures,
            crate::AtlasTextureKind::Rgba => &self.rgba_textures,
        };
        textures[id.index as usize].as_ref().unwrap()
    }
}

impl BladeAtlasStorage {
    fn destroy(&mut self, gpu: &gpu::Context) {
        for mut texture in self.monochrome_textures.drain().flatten() {
            texture.destroy(gpu);
        }
        for mut texture in self.polychrome_textures.drain().flatten() {
            texture.destroy(gpu);
        }
        for mut texture in self.rgba_textures.drain().flatten() {
            texture.destroy(gpu);
        }
    }
}

struct BladeAtlasTexture {
    id: AtlasTextureId,
    size: Size<DevicePixels>,
    allocator: BucketedAtlasAllocator,
    raw: gpu::Texture,
    raw_view: gpu::TextureView,
    format: gpu::TextureFormat,
    live_atlas_keys: u32,
}

impl BladeAtlasTexture {
    fn allocate(&mut self, size: Size<DevicePixels>) -> Option<AtlasTile> {
        let allocation = self.allocator.allocate(device_size_to_etagere(size))?;
        let tile = AtlasTile {
            texture_id: self.id,
            tile_id: allocation.id.into(),
            padding: 0,
            bounds: Bounds {
                origin: etagere_point_to_device(allocation.rectangle.min),
                size,
            },
        };
        self.live_atlas_keys += 1;
        Some(tile)
    }

    fn destroy(&mut self, gpu: &gpu::Context) {
        gpu.destroy_texture(self.raw);
        gpu.destroy_texture_view(self.raw_view);
    }

    fn bytes_per_pixel(&self) -> u8 {
        self.format.block_info().size
    }

    fn decrement_ref_count(&mut self) {
        self.live_atlas_keys -= 1;
    }

    fn deallocate(&mut self, tile_id: crate::TileId) {
        self.allocator.deallocate(tile_id.into());
    }

    fn is_unreferenced(&mut self) -> bool {
        self.live_atlas_keys == 0
    }
}

fn device_size_to_etagere(size: Size<DevicePixels>) -> etagere::Size {
    etagere::Size::new(size.width.into(), size.height.into())
}

fn etagere_point_to_device(value: etagere::Point) -> Point<DevicePixels> {
    Point {
        x: DevicePixels::from(value.x),
        y: DevicePixels::from(value.y),
    }
}
