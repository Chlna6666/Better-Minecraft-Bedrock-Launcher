//! Shares mesh assets without keeping unloaded chunks' assets alive.

use super::{MaterialSlotKey, mipmapped_texture, texture_storage_bytes};
use gpui_3d::{AlphaMode, Material, ShadingModel, TextureAsset, TextureAssetId};
use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Weak};

#[derive(Default)]
pub(super) struct MeshAssets {
    // Canonicalize exact texture contents across different resource-pack
    // paths and texture aliases. Only weak references are retained, so a
    // streamed-out world releases both its CPU images and GPU resources.
    // GPU materials then see one stable TextureAssetId, enabling real
    // cross-Section batching without changing any texture coordinates.
    content_textures: HashMap<u64, Vec<Weak<TextureAsset>>>,
    materials: HashMap<MaterialSlotKey, Weak<Material>>,
    grass_side_tints: HashMap<(TextureAssetId, [u8; 3]), TintEntry>,
    grayscale_masks: HashMap<TextureAssetId, bool>,
}

struct TintEntry {
    // Keep the source's identity stable while a derived texture is in use. Otherwise the path
    // cache's weak reference expires and every chunk reloads and re-decodes the same source.
    _source: Arc<TextureAsset>,
    texture: Weak<TextureAsset>,
}

impl MeshAssets {
    // TextureAsset::PartialEq includes its generated ID, so it cannot be
    // used for content interning. Compare the full immutable GPU sampling
    // contract, with a hash only as a fast candidate lookup. Collisions are
    // resolved with exact byte comparison, never by the hash alone.
    fn texture_fingerprint(texture: &TextureAsset) -> u64 {
        let mut hash = DefaultHasher::new();
        texture.dimensions().hash(&mut hash);
        texture.mip_level_count().hash(&mut hash);
        (texture.color_space() as u8).hash(&mut hash);
        (texture.sampling() as u8).hash(&mut hash);
        for level in 0..texture.mip_level_count() {
            texture.mip_pixels(level).expect("validated texture mip").hash(&mut hash);
        }
        hash.finish()
    }

    fn identical_texels(a: &TextureAsset, b: &TextureAsset) -> bool {
        a.dimensions() == b.dimensions()
            && a.mip_level_count() == b.mip_level_count()
            && a.color_space() == b.color_space()
            && a.sampling() == b.sampling()
            && (0..a.mip_level_count()).all(|level| a.mip_pixels(level) == b.mip_pixels(level))
    }

    pub(super) fn intern_texture(&mut self, texture: TextureAsset) -> Arc<TextureAsset> {
        let fingerprint = Self::texture_fingerprint(&texture);
        let candidates = self.content_textures.entry(fingerprint).or_default();
        // Opportunistic cleanup avoids an unbounded weak list if images are
        // frequently unloaded and reloaded before the periodic prune pass.
        candidates.retain(|old| old.strong_count() > 0);
        if let Some(existing) = candidates.iter().filter_map(Weak::upgrade)
            .find(|old| Self::identical_texels(old, &texture))
        {
            return existing;
        }
        let shared = Arc::new(texture);
        candidates.push(Arc::downgrade(&shared));
        shared
    }

    pub(super) fn prune(&mut self) {
        self.content_textures.retain(|_, bucket| {
            bucket.retain(|image| image.strong_count() > 0);
            !bucket.is_empty()
        });
        self.materials.retain(|_, asset| asset.strong_count() > 0);
        self.grass_side_tints
            .retain(|_, entry| entry.texture.strong_count() > 0);
    }

    pub(super) fn tinted_texture_usage(&self) -> (usize, usize) {
        self.grass_side_tints
            .values()
            .filter_map(|entry| entry.texture.upgrade())
            .fold((0, 0), |(count, bytes), texture| {
                (count + 1, bytes + texture_storage_bytes(&texture))
            })
    }

    pub(super) fn material(
        &mut self,
        key: MaterialSlotKey,
        alpha_mode: AlphaMode,
    ) -> Arc<Material> {
        if let Some(material) = self.materials.get(&key).and_then(Weak::upgrade) {
            return material;
        }
        let mut material = Material::new();
        material.surface = if key.water {
            gpui_3d::SurfaceKind::Water
        } else if key.glass {
            gpui_3d::SurfaceKind::Glass
        } else {
            gpui_3d::SurfaceKind::Standard
        };
        material.shading_model = ShadingModel::Unlit;
        material.alpha_mode = alpha_mode;
        material.alpha_cutoff = 0.35;
        material.double_sided = key.double_sided;
        material.front_face = gpui_3d::FrontFace::Cw;
        let color = key.base_color_bits.map(f32::from_bits);
        material.base_color = [color[0], color[1], color[2], 1.0];
        material.albedo_texture = key.texture;
        material.normal_texture = key.normal;
        material.metallic_roughness_texture = key.mer;
        // Missing Texture Sets describe ordinary dry blocks, not semi-polished surfaces.
        material.roughness = if key.water || key.glass { 0.08 } else { 1.0 };
        material.pbr_texture_layout = gpui_3d::PbrTextureLayout::MetallicEmissiveRoughness;
        let material = Arc::new(material);
        self.materials.insert(key, Arc::downgrade(&material));
        material
    }

    /// Tint only the green overlay, preserving the dirt base of carried grass textures.
    pub(super) fn tinted_grass_side(
        &mut self,
        base: Option<&Arc<TextureAsset>>,
        tint: [f32; 4],
    ) -> Option<Arc<TextureAsset>> {
        // Bedrock plains grass tint (#7bbf5b), the reference the carried side texture is
        // authored against; the vertex color now carries this tint alone.
        const PLAINS: [f32; 3] = [0.4824, 0.7490, 0.3569];
        let base = base?;
        let key = [
            (tint[0].clamp(0.0, 1.0) * 255.0).round() as u8,
            (tint[1].clamp(0.0, 1.0) * 255.0).round() as u8,
            (tint[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        ];
        let key = (base.id(), key);
        if let Some(cached) = self
            .grass_side_tints
            .get(&key)
            .and_then(|entry| entry.texture.upgrade())
        {
            return Some(cached);
        }
        let mut pixels = base.pixels().to_vec();
        let ratio = [
            (tint[0] / PLAINS[0]).clamp(0.0, 3.0),
            (tint[1] / PLAINS[1]).clamp(0.0, 3.0),
            (tint[2] / PLAINS[2]).clamp(0.0, 3.0),
        ];
        for pixel in pixels.chunks_exact_mut(4) {
            let r = f32::from(pixel[0]);
            let g = f32::from(pixel[1]);
            let b = f32::from(pixel[2]);
            if g > r + 12.0 && g > b + 12.0 {
                pixel[0] = (r * ratio[0]).clamp(0.0, 255.0) as u8;
                pixel[1] = (g * ratio[1]).clamp(0.0, 255.0) as u8;
                pixel[2] = (b * ratio[2]).clamp(0.0, 255.0) as u8;
            }
        }
        let image = image::RgbaImage::from_raw(base.width(), base.height(), pixels)?;
        let asset = self.intern_texture(mipmapped_texture(image)?);
        self.grass_side_tints.insert(
            key,
            TintEntry {
                _source: base.clone(),
                texture: Arc::downgrade(&asset),
            },
        );
        Some(asset)
    }

    /// Whether a decoded texture is a neutral mask that expects a biome tint.
    ///
    /// Vanilla grass, leaves and plant textures are grayscale masks; resource packs may ship
    /// pre-tinted variants instead, which must render without multiplying a second tint.
    pub(super) fn is_grayscale_mask(&mut self, texture: &TextureAsset) -> bool {
        let id = texture.id();
        if let Some(masked) = self.grayscale_masks.get(&id) {
            return *masked;
        }
        let pixels = texture.pixels();
        let pixel_count = pixels.len() / 4;
        let step = (pixel_count / 1024).max(1);
        let mut sum = [0_u64; 3];
        let mut samples = 0_u64;
        for index in (0..pixel_count).step_by(step) {
            let pixel = &pixels[index * 4..index * 4 + 4];
            if pixel[3] < 8 {
                continue;
            }
            for (channel, value) in pixel[..3].iter().enumerate() {
                sum[channel] += u64::from(*value);
            }
            samples += 1;
        }
        let mask = samples > 0 && {
            let average = sum.map(|value| value / samples);
            let maximum = average[0].max(average[1]).max(average[2]);
            let minimum = average[0].min(average[1]).min(average[2]);
            maximum - minimum <= 12
        };
        self.grayscale_masks.insert(id, mask);
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_resource_pack_paths_share_exact_gpu_texture_identity() {
        let mut assets = MeshAssets::default();
        let pixels = [25, 90, 155, 255, 10, 20, 30, 128];
        let first = assets.intern_texture(TextureAsset::rgba8(2, 1, pixels.to_vec()).unwrap());
        let other = assets.intern_texture(TextureAsset::rgba8(2, 1, pixels.to_vec()).unwrap());
        assert!(Arc::ptr_eq(&first, &other));
        assert_eq!(first.id(), other.id());
        assert_eq!(assets.content_textures.len(), 1);
        let weak = Arc::downgrade(&first);
        drop((first, other));
        assert!(weak.upgrade().is_none());
        assets.prune();
        assert!(assets.content_textures.is_empty());
    }

    #[test]
    fn content_interning_preserves_sampling_color_space_and_mips() {
        let mut assets = MeshAssets::default();
        let rgba = [20, 40, 60, 255];
        let srgb = assets.intern_texture(TextureAsset::rgba8(1, 1, rgba.to_vec()).unwrap());
        let point = assets.intern_texture(
            TextureAsset::rgba8(1, 1, rgba.to_vec()).unwrap().with_point_sampling(),
        );
        let linear = assets.intern_texture(
            TextureAsset::linear_rgba8(1, 1, rgba.to_vec()).unwrap(),
        );
        let mip = assets.intern_texture(
            TextureAsset::rgba8_mip_chain(2, 2, [
                vec![20, 40, 60, 255].repeat(4),
                rgba.to_vec(),
            ]).unwrap(),
        );
        assert_ne!(srgb.id(), point.id());
        assert_ne!(srgb.id(), linear.id());
        assert_ne!(srgb.id(), mip.id());
        assert_eq!(assets.content_textures.len(), 4);
    }

    #[test]
    fn alpha_coverage_does_not_select_glass_or_polished_roughness() {
        let mut assets = MeshAssets::default();
        let key = MaterialSlotKey {
            water: false,
            glass: false,
            texture: None,
            normal: None,
            mer: None,
            alpha_mode: AlphaMode::Blend as u8,
            double_sided: true,
            base_color_bits: [1.0_f32.to_bits(); 3],
        };
        let diffuse = assets.material(key, AlphaMode::Blend);
        assert_eq!(diffuse.surface, gpui_3d::SurfaceKind::Standard);
        assert_eq!(diffuse.roughness, 1.);
        let glass = assets.material(MaterialSlotKey { glass: true, ..key }, AlphaMode::Blend);
        assert_eq!(glass.surface, gpui_3d::SurfaceKind::Glass);
        assert_eq!(glass.roughness, 0.08);
        assert_ne!(diffuse.id(), glass.id());
    }

    #[test]
    fn shared_material_is_reused_until_last_owner_drops() {
        let mut assets = MeshAssets::default();
        let key = MaterialSlotKey {
            water: false,
            glass: false,
            texture: None,
            normal: None,
            mer: None,
            alpha_mode: AlphaMode::Opaque as u8,
            double_sided: false,
            base_color_bits: [1.0_f32.to_bits(); 3],
        };
        let first = assets.material(key, AlphaMode::Opaque);
        assert_eq!(
            first.roughness, 1.,
            "ordinary blocks without MER must use a dry, rough fallback"
        );
        let second = assets.material(key, AlphaMode::Opaque);
        assert!(Arc::ptr_eq(&first, &second));
        assets.prune();
        assert_eq!(assets.materials.len(), 1);
        let weak = Arc::downgrade(&first);
        let id = first.id();
        drop((first, second));
        assert!(weak.upgrade().is_none());
        let replacement = assets.material(key, AlphaMode::Opaque);
        assert_ne!(replacement.id(), id);
        drop(replacement);
        assets.prune();
        assert!(assets.materials.is_empty());
    }

    #[test]
    fn grayscale_masks_request_tint_and_pre_tinted_textures_do_not() {
        let mut assets = MeshAssets::default();
        let mask = Arc::new(
            TextureAsset::rgba8(
                2,
                1,
                Arc::<[u8]>::from([82, 82, 82, 200, 120, 118, 122, 255]),
            )
            .unwrap(),
        );
        assert!(assets.is_grayscale_mask(&mask));
        let colored =
            Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([51, 127, 8, 255])).unwrap());
        assert!(!assets.is_grayscale_mask(&colored));
        assert!(assets.is_grayscale_mask(&mask));
    }

    #[test]
    fn tinted_texture_is_reused_without_retaining_unloaded_pixels() {
        let mut assets = MeshAssets::default();
        let base = Arc::new(
            TextureAsset::rgba8(
                2,
                1,
                Arc::<[u8]>::from([80, 180, 60, 255, 120, 80, 50, 255]),
            )
            .unwrap(),
        );
        let tint = [0.3, 0.5, 0.2, 1.0];
        let first = assets.tinted_grass_side(Some(&base), tint).unwrap();
        let second = assets.tinted_grass_side(Some(&base), tint).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(&first.pixels()[4..8], &base.pixels()[4..8]);
        assets.prune();
        assert_eq!(assets.grass_side_tints.len(), 1);
        let weak = Arc::downgrade(&first);
        drop((first, second));
        assert!(weak.upgrade().is_none());
        assets.prune();
        assert!(assets.grass_side_tints.is_empty());
        assert!(assets.tinted_grass_side(Some(&base), tint).is_some());
    }

    #[test]
    fn identical_biome_tints_do_not_alias_different_source_textures() {
        let mut assets = MeshAssets::default();
        let first_base =
            Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([80, 180, 60, 255])).unwrap());
        let second_base =
            Arc::new(TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([120, 80, 50, 255])).unwrap());
        let tint = [0.3, 0.5, 0.2, 1.0];
        let first = assets.tinted_grass_side(Some(&first_base), tint).unwrap();
        let second = assets.tinted_grass_side(Some(&second_base), tint).unwrap();
        assert_ne!(first.id(), second.id());
        assert_ne!(first.pixels(), second.pixels());
        assert_eq!(assets.tinted_texture_usage(), (2, 8));
        let source = Arc::downgrade(&first_base);
        drop(first_base);
        assert!(source.upgrade().is_some());
        drop(first);
        assets.prune();
        assert!(source.upgrade().is_none());
        assert_eq!(assets.tinted_texture_usage(), (1, 4));
    }
}
