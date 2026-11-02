//! Shared immutable atmosphere and water images; native retirement belongs to the backend.
use super::super::{SceneView, gpu};
use super::ScreenEffects;
use crate::TextureAssetId;
use gfx_core::TextureViewId;
use gpui::ExtensionDevice;
use std::collections::{HashMap, HashSet};

pub(super) struct Textures {
    images: HashMap<TextureAssetId, gpu::TextureResources>,
    fallback: gpu::TextureResources,
}
impl Textures {
    pub(super) fn new(device: &mut dyn ExtensionDevice) -> gpui::Result<Self> {
        let asset = crate::TextureAsset::linear_rgba8(1, 1, vec![0, 0, 0, 0])?;
        Ok(Self {
            images: HashMap::new(),
            fallback: gpu::create_texture(device, &asset)?,
        })
    }
    pub(super) fn views(
        &mut self,
        device: &mut dyn ExtensionDevice,
        view: &SceneView,
        settings: ScreenEffects,
    ) -> gpui::Result<[TextureViewId; 3]> {
        let mut views = [self.fallback.view; 3];
        for (index, id) in settings.texture_ids().into_iter().enumerate() {
            let Some(id) = id else {
                continue;
            };
            let asset = view.textures.get(&id).ok_or_else(|| {
                anyhow::anyhow!("effect texture {id:?} is missing from the scene texture table")
            })?;
            if index > 0 && asset.height() % asset.width() != 0 {
                anyhow::bail!("water flipbook must contain square frames in a vertical strip");
            }
            if !self.images.contains_key(&id) {
                self.images.insert(id, gpu::create_texture(device, asset)?);
            }
            views[index] = self.images[&id].view;
        }
        if settings.water_flow_texture.is_none() {
            views[2] = views[1];
        }
        Ok(views)
    }
    pub(super) fn retain(
        &mut self,
        device: &mut dyn ExtensionDevice,
        used: &HashSet<TextureAssetId>,
    ) -> gpui::Result<()> {
        let expired = self
            .images
            .keys()
            .filter(|id| !used.contains(id))
            .copied()
            .collect::<Vec<_>>();
        for id in expired {
            if let Some(texture) = self.images.remove(&id) {
                gpu::destroy_texture(device, texture)?;
            }
        }
        Ok(())
    }
    pub(super) fn destroy(self, device: &mut dyn ExtensionDevice) -> gpui::Result<()> {
        for texture in self.images.into_values() {
            gpu::destroy_texture(device, texture)?;
        }
        gpu::destroy_texture(device, self.fallback)
    }
}
