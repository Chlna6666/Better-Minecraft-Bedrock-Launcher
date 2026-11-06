//! Image ownership and decoding stay in the application; pack lookup stays in block-model.
use super::*;
use bedrock_block_model::{ObjResolvedPbrTextureSet, PbrTextureLayer};

#[derive(Default)]
pub(super) struct PbrCache {
    sets: HashMap<(String, [i32; 3]), Option<ObjResolvedPbrTextureSet>>,
    // The same RGBA value can represent either sRGB albedo or linear
    // normal/MER data. These must NEVER share an asset/descriptor, even if
    // their bytes match: the GPU sampling transfer function is different.
    constants: HashMap<([u8; 4], bool), Weak<TextureAsset>>,
}

impl PbrCache {
    fn constant(
        &mut self,
        assets: &mut MeshAssets,
        pixel: [u8; 4],
        is_data: bool,
    ) -> Option<(Arc<TextureAsset>, TextureAlphaContent)> {
        let key = (pixel, is_data);
        let image = RgbaImage::from_pixel(1, 1, image::Rgba(pixel));
        let content = texture_alpha_content(&image);
        if let Some(texture) = self.constants.get(&key).and_then(Weak::upgrade) {
            return Some((texture, content));
        }
        let texture = assets.intern_texture(if is_data {
            mipmapped_data_texture(image)
        } else {
            mipmapped_texture(image)
        }?);
        self.constants.insert(key, Arc::downgrade(&texture));
        Some((texture, content))
    }
}

impl Worker {
    pub(super) fn resolve_pbr(
        &mut self,
        material: &str,
        normal: [i32; 3],
    ) -> [Option<(Arc<TextureAsset>, TextureAlphaContent)>; 3] {
        let key = (material.to_owned(), normal);
        if !self.pbr.sets.contains_key(&key) {
            let set = match self
                .resolved_texture_lookup
                .get(&key)
                .and_then(Option::as_ref)
            {
                Some(source) => match self.resolver.pbr_texture_set_for(source) {
                    Ok(set) => set,
                    Err(error) => {
                        self.last_error = Some(format!("Texture Set {material}: {error}"));
                        None
                    }
                },
                None => None,
            };
            self.pbr.sets.insert(key.clone(), set);
        }
        let Some(set) = self.pbr.sets.get(&key).cloned().flatten() else {
            return [None, None, None];
        };
        let mut layers = [set.color, set.normal, set.material];
        std::array::from_fn(|index| match layers[index].take()? {
            PbrTextureLayer::Image(source) => self.resolve_texture_path_with(source, index != 0),
            PbrTextureLayer::Constant(pixel) => {
                self.pbr.constant(&mut self.mesh_assets, pixel, index != 0)
            }
        })
    }
}

#[cfg(test)]
mod constant_texture_tests {
    use super::*;

    #[test]
    fn constant_pbr_data_and_albedo_keep_distinct_transfer_functions() {
        let mut cache = PbrCache::default();
        let mut textures = MeshAssets::default();
        let sample = [128, 128, 255, 255];
        let (srgb, _) = cache.constant(&mut textures, sample, false).unwrap();
        let (linear, _) = cache.constant(&mut textures, sample, true).unwrap();
        assert_eq!(srgb.color_space(), gpui_3d::TextureColorSpace::Srgb);
        assert_eq!(linear.color_space(), gpui_3d::TextureColorSpace::Linear);
        assert_ne!(srgb.id(), linear.id());
        let (again, _) = cache.constant(&mut textures, sample, true).unwrap();
        assert!(Arc::ptr_eq(&linear, &again));
        assert_eq!(cache.constants.len(), 2);
    }
}
