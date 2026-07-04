use std::{borrow::Cow, ops};

use anyhow::Result;

use crate::{
    Bounds, DevicePixels, RenderGlyphParams, RenderImageParams, RenderImagePixelFormat,
    RenderSvgParams, Rgba, Size,
};

#[derive(PartialEq, Eq, Hash, Clone)]
pub(crate) enum AtlasKey {
    Glyph(RenderGlyphParams),
    Svg(RenderSvgParams),
    Image(RenderImageParams),
}

impl AtlasKey {
    #[cfg_attr(
        all(
            any(target_os = "linux", target_os = "freebsd"),
            not(any(feature = "x11", feature = "wayland"))
        ),
        allow(dead_code)
    )]
    pub(crate) fn texture_kind(&self) -> AtlasTextureKind {
        match self {
            AtlasKey::Glyph(params) => {
                if params.is_emoji {
                    AtlasTextureKind::Bgra
                } else {
                    AtlasTextureKind::Monochrome
                }
            }
            AtlasKey::Svg(_) => AtlasTextureKind::Monochrome,
            AtlasKey::Image(params) => match params.pixel_format {
                RenderImagePixelFormat::Bgra8 => AtlasTextureKind::Bgra,
                RenderImagePixelFormat::Rgba8 => AtlasTextureKind::Rgba,
            },
        }
    }
}

impl From<RenderGlyphParams> for AtlasKey {
    fn from(params: RenderGlyphParams) -> Self {
        Self::Glyph(params)
    }
}

impl From<RenderSvgParams> for AtlasKey {
    fn from(params: RenderSvgParams) -> Self {
        Self::Svg(params)
    }
}

impl From<RenderImageParams> for AtlasKey {
    fn from(params: RenderImageParams) -> Self {
        Self::Image(params)
    }
}

pub(crate) trait PlatformAtlas: Send + Sync {
    fn ensure_tile_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>>;

    fn refresh_tile_with<'a>(
        &self,
        key: &AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        let Some((size, bytes)) = build()? else {
            return Ok(None);
        };
        let bytes = bytes.into_owned();
        self.remove(key);
        self.ensure_tile_with(key, &mut || {
            Ok(Some((size, Cow::Borrowed(bytes.as_slice()))))
        })
    }

    fn ensure_glyph_with(
        &self,
        params: &RenderGlyphParams,
        build: &mut dyn FnMut() -> Result<GlyphRasterization>,
    ) -> Result<Option<AtlasTile>> {
        self.ensure_tile_with(&params.clone().into(), &mut || match build()? {
            GlyphRasterization::Bitmap { size, bytes } => Ok(Some((size, Cow::Owned(bytes)))),
            GlyphRasterization::ColorLayers { fallback, .. } => {
                Ok(Some((fallback.size, Cow::Owned(fallback.bytes))))
            }
        })
    }

    fn clear_glyphs(&self);
    fn remove(&self, key: &AtlasKey);
}

pub(crate) struct GlyphBitmap {
    pub(crate) size: Size<DevicePixels>,
    pub(crate) bytes: Vec<u8>,
}

#[expect(
    dead_code,
    reason = "color glyph layers are used by the Metal renderer"
)]
pub(crate) struct ColorGlyphLayer {
    pub(crate) bounds: Bounds<DevicePixels>,
    pub(crate) color: Rgba,
    pub(crate) alpha: Vec<u8>,
}

#[expect(
    dead_code,
    reason = "color glyph rasterization is used by the Metal renderer"
)]
pub(crate) enum GlyphRasterization {
    Bitmap {
        size: Size<DevicePixels>,
        bytes: Vec<u8>,
    },
    ColorLayers {
        size: Size<DevicePixels>,
        layers: Vec<ColorGlyphLayer>,
        fallback: GlyphBitmap,
    },
}

pub(crate) struct AtlasTextureList<T> {
    textures: Vec<Option<T>>,
    free_list: Vec<usize>,
}

impl<T> Default for AtlasTextureList<T> {
    fn default() -> Self {
        Self {
            textures: Vec::default(),
            free_list: Vec::default(),
        }
    }
}

impl<T> ops::Index<usize> for AtlasTextureList<T> {
    type Output = Option<T>;

    fn index(&self, index: usize) -> &Self::Output {
        &self.textures[index]
    }
}

impl<T> AtlasTextureList<T> {
    #[allow(unused)]
    pub(crate) fn drain(&mut self) -> std::vec::Drain<'_, Option<T>> {
        self.free_list.clear();
        self.textures.drain(..)
    }

    #[allow(dead_code)]
    pub(crate) fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.textures.iter().flatten()
    }

    #[allow(dead_code)]
    pub(crate) fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut T> {
        self.textures.iter_mut().flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub(crate) struct AtlasTile {
    pub(crate) texture_id: AtlasTextureId,
    pub(crate) tile_id: TileId,
    pub(crate) padding: u32,
    pub(crate) bounds: Bounds<DevicePixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub(crate) struct AtlasTextureId {
    // We use u32 instead of usize for Metal Shader Language compatibility
    pub(crate) index: u32,
    pub(crate) kind: AtlasTextureKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
#[cfg_attr(
    all(
        any(target_os = "linux", target_os = "freebsd"),
        not(any(feature = "x11", feature = "wayland"))
    ),
    allow(dead_code)
)]
pub(crate) enum AtlasTextureKind {
    Monochrome = 0,
    Bgra = 1,
    Rgba = 2,
    Subpixel = 3,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_atlas_key_uses_image_pixel_format() {
        let bgra_key = AtlasKey::Image(RenderImageParams {
            image_id: crate::ImageId(1),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Bgra8,
        });
        let rgba_key = AtlasKey::Image(RenderImageParams {
            image_id: crate::ImageId(1),
            frame_slot: 0,
            pixel_format: RenderImagePixelFormat::Rgba8,
        });

        assert!(bgra_key != rgba_key);
        assert_eq!(bgra_key.texture_kind(), AtlasTextureKind::Bgra);
        assert_eq!(rgba_key.texture_kind(), AtlasTextureKind::Rgba);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(C)]
pub(crate) struct TileId(pub(crate) u32);

impl From<etagere::AllocId> for TileId {
    fn from(id: etagere::AllocId) -> Self {
        Self(id.serialize())
    }
}

impl From<TileId> for etagere::AllocId {
    fn from(id: TileId) -> Self {
        Self::deserialize(id.0)
    }
}
