//! Image ownership and decoding stay in the application; pack lookup stays in block-model.
use super::*;
use bedrock_block_model::{ObjResolvedPbrTextureSet, PbrTextureLayer};

#[derive(Default)]
pub(super) struct PbrCache {
    sets: HashMap<(String, [i32; 3]), Option<ObjResolvedPbrTextureSet>>,
    constants: HashMap<[u8; 4], Weak<TextureAsset>>,
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
        [set.color, set.normal, set.material].map(|layer| match layer? {
            PbrTextureLayer::Image(source) => self.resolve_texture_path(source),
            PbrTextureLayer::Constant(pixel) => {
                let image = RgbaImage::from_pixel(1, 1, image::Rgba(pixel));
                let content = texture_alpha_content(&image);
                let asset = match self.pbr.constants.get(&pixel).and_then(Weak::upgrade) {
                    Some(asset) => asset,
                    None => {
                        let asset = Arc::new(mipmapped_texture(image)?);
                        self.pbr.constants.insert(pixel, Arc::downgrade(&asset));
                        asset
                    }
                };
                Some((asset, content))
            }
        })
    }
}
