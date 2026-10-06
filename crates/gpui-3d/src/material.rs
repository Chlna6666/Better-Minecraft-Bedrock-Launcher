use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEXTURE_ID: AtomicU64 = AtomicU64::new(1);

/// Stable material identity.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MaterialId(pub u64);

/// Renderer-neutral identity for a texture asset supplied by an application.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TextureAssetId(pub u64);

/// Immutable RGBA8 image and optional caller-supplied mip levels referenced by a material.
///
/// The renderer uploads every provided level as a sampled texture using the declared color space.
/// Texture assets are shared by ID across scene snapshots; their GPU resources follow the
/// scene-view renderer resource lifetime. Image decoding and mip generation remain application-owned.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureAsset {
    id: TextureAssetId,
    width: u32,
    height: u32,
    mip_level_count: u32,
    color_space: TextureColorSpace,
    sampling: TextureSampling,
    rgba8: Arc<[u8]>,
    mipmaps: Arc<[Arc<[u8]>]>,
}

/// Interpretation of RGBA8 texture channel values.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextureColorSpace {
    /// Color channels use the sRGB transfer function and are decoded during sampling.
    #[default]
    Srgb,
    /// Channel values are sampled as linear data, as required for normal maps.
    Linear,
}

/// Texel filtering used when sampling a texture asset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TextureSampling {
    /// Blends neighbouring texels; appropriate for photographic or smoothly varying images.
    #[default]
    Linear,
    /// Reads the nearest texel; required for atlas images whose regions must not bleed together.
    Nearest,
}

/// Invalid RGBA8 texture data.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum TextureError {
    /// Width or height is zero.
    #[error("texture dimensions must be non-zero")]
    EmptyExtent,
    /// Pixel storage does not contain exactly four bytes per texel.
    #[error("texture pixel data has {actual} bytes; expected {expected}")]
    InvalidDataLength {
        /// Required byte count for the declared dimensions.
        expected: usize,
        /// Supplied byte count.
        actual: usize,
    },
    /// A mip chain contains no base image.
    #[error("texture mip chain must contain its base image")]
    EmptyMipChain,
    /// A mip chain contains more levels than the dimensions allow.
    #[error("texture mip chain contains too many levels")]
    TooManyMipLevels,
    /// A mip level does not contain four bytes per texel for its derived extent.
    #[error("texture mip level {level} has {actual} bytes; expected {expected}")]
    InvalidMipDataLength {
        /// Index of the mip level with invalid pixel storage.
        level: u32,
        /// Required byte count for the derived mip extent.
        expected: usize,
        /// Supplied byte count.
        actual: usize,
    },
    /// The required byte count does not fit the current address space.
    #[error("texture dimensions exceed the addressable byte range")]
    TooLarge,
}

impl TextureAsset {
    /// Creates an sRGB RGBA8 image from tightly packed rows.
    ///
    /// The texture uses linear filtering and clamp-to-edge addressing in the GPUI scene view.
    /// Applications should convert source pixels to RGBA8 before creating the asset. The renderer
    /// does not decode image files or alter alpha values.
    ///
    /// # Errors
    ///
    /// Returns [`TextureError::EmptyExtent`] for a zero dimension,
    /// [`TextureError::TooLarge`] when the image byte count overflows, or
    /// [`TextureError::InvalidDataLength`] when `rgba8` is not exactly `width * height * 4` bytes.
    pub fn rgba8(
        width: u32,
        height: u32,
        rgba8: impl Into<Arc<[u8]>>,
    ) -> Result<Self, TextureError> {
        Self::new(width, height, rgba8.into(), TextureColorSpace::Srgb)
    }

    /// Creates an sRGB RGBA8 image from caller-supplied mip levels.
    ///
    /// `levels` includes the base image first. Each following level uses half the previous width
    /// and height, rounded down and clamped to one. A chain may stop before `1x1`; the renderer
    /// uploads the provided levels and samples them with linear mip filtering. Image decoding and
    /// mip generation remain application responsibilities.
    ///
    /// # Errors
    ///
    /// Returns [`TextureError::EmptyExtent`] for a zero dimension,
    /// [`TextureError::TooLarge`] when a mip extent overflows the addressable byte range,
    /// [`TextureError::EmptyMipChain`] when no base image is supplied,
    /// [`TextureError::InvalidDataLength`] when the base image byte count is wrong,
    /// [`TextureError::TooManyMipLevels`] when the chain exceeds the complete mip count, or
    /// [`TextureError::InvalidMipDataLength`] when a level's byte count does not match its extent.
    pub fn rgba8_mip_chain(
        width: u32,
        height: u32,
        levels: impl IntoIterator<Item = impl Into<Arc<[u8]>>>,
    ) -> Result<Self, TextureError> {
        Self::with_mip_chain(width, height, levels, TextureColorSpace::Srgb)
    }

    /// Creates a linear RGBA8 image for data textures such as tangent-space normal maps.
    ///
    /// The byte layout and validation rules match [`TextureAsset::rgba8`], but the renderer does
    /// not apply sRGB decoding when sampling this image.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`TextureAsset::rgba8`].
    pub fn linear_rgba8(
        width: u32,
        height: u32,
        rgba8: impl Into<Arc<[u8]>>,
    ) -> Result<Self, TextureError> {
        Self::new(width, height, rgba8.into(), TextureColorSpace::Linear)
    }

    /// Creates a linear RGBA8 image from caller-supplied mip levels.
    ///
    /// The level layout and validation rules match [`TextureAsset::rgba8_mip_chain`]. Use this
    /// for data textures such as normal and ambient-occlusion maps.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`TextureAsset::rgba8_mip_chain`].
    pub fn linear_rgba8_mip_chain(
        width: u32,
        height: u32,
        levels: impl IntoIterator<Item = impl Into<Arc<[u8]>>>,
    ) -> Result<Self, TextureError> {
        Self::with_mip_chain(width, height, levels, TextureColorSpace::Linear)
    }

    /// Switches this image to nearest-texel sampling.
    ///
    /// Pixel-art sources such as Minecraft skin atlases need exact texel reads. Linear filtering
    /// blends neighbouring atlas regions into each other and produces translucent fringes along
    /// skin-part borders. Mip filtering stays linear; call this on base-level-only images when a
    /// downscaled view must keep crisp texel edges.
    #[must_use]
    pub const fn with_point_sampling(mut self) -> Self {
        self.sampling = TextureSampling::Nearest;
        self
    }

    /// Whether this image samples with nearest-texel filtering.
    #[must_use]
    pub const fn sampling(&self) -> TextureSampling {
        self.sampling
    }

    fn new(
        width: u32,
        height: u32,
        rgba8: Arc<[u8]>,
        color_space: TextureColorSpace,
    ) -> Result<Self, TextureError> {
        validate_extent(width, height)?;
        let expected = rgba8_byte_len(width, height)?;
        if rgba8.len() != expected {
            return Err(TextureError::InvalidDataLength {
                expected,
                actual: rgba8.len(),
            });
        }
        Ok(Self {
            id: TextureAssetId(NEXT_TEXTURE_ID.fetch_add(1, Ordering::Relaxed)),
            width,
            height,
            mip_level_count: 1,
            color_space,
            sampling: TextureSampling::Linear,
            rgba8,
            mipmaps: Arc::from([]),
        })
    }

    fn with_mip_chain(
        width: u32,
        height: u32,
        levels: impl IntoIterator<Item = impl Into<Arc<[u8]>>>,
        color_space: TextureColorSpace,
    ) -> Result<Self, TextureError> {
        validate_extent(width, height)?;
        let mut levels = levels.into_iter().map(Into::into);
        let rgba8 = levels.next().ok_or(TextureError::EmptyMipChain)?;
        let expected = rgba8_byte_len(width, height)?;
        if rgba8.len() != expected {
            return Err(TextureError::InvalidDataLength {
                expected,
                actual: rgba8.len(),
            });
        }

        let max_level_count = u32::BITS - width.max(height).leading_zeros();
        let mut mipmaps = Vec::new();
        let mut mip_width = width;
        let mut mip_height = height;
        for (index, rgba8) in levels.enumerate() {
            let level = u32::try_from(index + 1).map_err(|_| TextureError::TooManyMipLevels)?;
            if level >= max_level_count {
                return Err(TextureError::TooManyMipLevels);
            }
            mip_width = (mip_width / 2).max(1);
            mip_height = (mip_height / 2).max(1);
            let expected = rgba8_byte_len(mip_width, mip_height)?;
            if rgba8.len() != expected {
                return Err(TextureError::InvalidMipDataLength {
                    level,
                    expected,
                    actual: rgba8.len(),
                });
            }
            mipmaps.push(rgba8);
        }

        let mip_level_count = u32::try_from(mipmaps.len())
            .map_err(|_| TextureError::TooManyMipLevels)?
            .checked_add(1)
            .ok_or(TextureError::TooManyMipLevels)?;
        Ok(Self {
            id: TextureAssetId(NEXT_TEXTURE_ID.fetch_add(1, Ordering::Relaxed)),
            width,
            height,
            mip_level_count,
            color_space,
            sampling: TextureSampling::Linear,
            rgba8,
            mipmaps: mipmaps.into(),
        })
    }

    /// Stable identity for this immutable image asset.
    #[must_use]
    pub const fn id(&self) -> TextureAssetId {
        self.id
    }

    /// Image width in texels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Image height in texels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Number of available mip levels, including the base image.
    #[must_use]
    pub fn mip_level_count(&self) -> u32 {
        self.mip_level_count
    }

    /// Returns the extent of a mip level, or `None` when the level is not present.
    #[must_use]
    pub fn mip_extent(&self, level: u32) -> Option<(u32, u32)> {
        if level >= self.mip_level_count() {
            return None;
        }
        Some(((self.width >> level).max(1), (self.height >> level).max(1)))
    }

    /// Channel interpretation used when the GPU samples this image.
    #[must_use]
    pub const fn color_space(&self) -> TextureColorSpace {
        self.color_space
    }

    /// Tightly packed RGBA8 source bytes.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.rgba8
    }

    /// Packed RGBA8 bytes for a mip level, or `None` when the level is not present.
    #[must_use]
    pub fn mip_pixels(&self, level: u32) -> Option<&[u8]> {
        if level == 0 {
            Some(&self.rgba8)
        } else {
            self.mipmaps
                .get(usize::try_from(level - 1).ok()?)
                .map(|pixels| pixels.as_ref())
        }
    }
}

fn validate_extent(width: u32, height: u32) -> Result<(), TextureError> {
    if width == 0 || height == 0 {
        return Err(TextureError::EmptyExtent);
    }
    if width.checked_mul(4).is_none() {
        return Err(TextureError::TooLarge);
    }
    Ok(())
}

fn rgba8_byte_len(width: u32, height: u32) -> Result<usize, TextureError> {
    usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(usize::try_from(height).ok()?))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(TextureError::TooLarge)
}

/// Surface alpha handling.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AlphaMode {
    /// Write opaque color and depth.
    #[default]
    Opaque,
    /// Discard fragments below `cutoff`, preserving depth for the rest.
    Mask,
    /// Blend premultiplied surface color without writing depth.
    Blend,
}

/// Surface lighting model.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShadingModel {
    /// Metallic-roughness microfacet lighting evaluated by the scene-view shader.
    #[default]
    MetallicRoughness,
    /// Use vertex and base colors directly without scene lighting or tone mapping.
    Unlit,
}

/// Metallic-roughness surface material.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    id: MaterialId,
    /// Linear base RGBA color.
    pub base_color: [f32; 4],
    /// Metallic weight in `0..=1`.
    pub metallic: f32,
    /// Perceptual roughness in `0..=1`.
    pub roughness: f32,
    /// Linear emissive RGB and intensity.
    pub emissive: [f32; 4],
    /// Alpha rendering mode.
    pub alpha_mode: AlphaMode,
    /// Alpha threshold for [`AlphaMode::Mask`].
    pub alpha_cutoff: f32,
    /// Surface lighting model.
    pub shading_model: ShadingModel,
    /// Whether back-facing triangles remain visible.
    pub double_sided: bool,
    /// Optional renderer-neutral albedo asset key.
    pub albedo_texture: Option<TextureAssetId>,
    /// Optional renderer-neutral normal-map asset key.
    ///
    /// The texture asset must use linear color space. Lit metallic-roughness shading requires a
    /// mesh tangent stream for UV set zero; use [`crate::Mesh::generate_tangents`] or
    /// [`crate::Mesh::with_tangents`] to provide it.
    pub normal_texture: Option<TextureAssetId>,
    /// Optional linear red-channel ambient-occlusion texture, sampled with the mesh UVs.
    /// Create the asset with [`TextureAsset::linear_rgba8`].
    pub occlusion_texture: Option<TextureAssetId>,
    /// Strength of the occlusion texture in `0..=1`; zero disables occlusion.
    ///
    /// This attenuates ambient illumination only; direct lights remain unaffected.
    pub occlusion_strength: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self::new()
    }
}

impl Material {
    /// Creates a white, non-metallic, medium-roughness material.
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: MaterialId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
            base_color: [1.0; 4],
            metallic: 0.0,
            roughness: 0.5,
            emissive: [0.0; 4],
            alpha_mode: AlphaMode::Opaque,
            alpha_cutoff: 0.5,
            shading_model: ShadingModel::MetallicRoughness,
            double_sided: false,
            albedo_texture: None,
            normal_texture: None,
            occlusion_texture: None,
            occlusion_strength: 1.0,
        }
    }

    /// Stable identity for this material snapshot.
    pub fn id(&self) -> MaterialId {
        self.id
    }

    /// Returns `None` for non-finite colors or surface values; otherwise clamps normalized values.
    pub fn validated(mut self) -> Option<Self> {
        if self
            .base_color
            .iter()
            .chain(self.emissive.iter())
            .any(|value| !value.is_finite())
            || !self.metallic.is_finite()
            || !self.roughness.is_finite()
            || !self.alpha_cutoff.is_finite()
            || !self.occlusion_strength.is_finite()
        {
            return None;
        }
        self.metallic = self.metallic.clamp(0.0, 1.0);
        self.roughness = self.roughness.clamp(0.0, 1.0);
        self.alpha_cutoff = self.alpha_cutoff.clamp(0.0, 1.0);
        self.occlusion_strength = self.occlusion_strength.clamp(0.0, 1.0);
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_clamps_physical_weights_and_rejects_non_finite_values() {
        let mut material = Material::new();
        material.metallic = 2.0;
        material.roughness = -1.0;
        material.occlusion_strength = 2.0;
        let material = material.validated().unwrap();
        assert!((material.metallic - 1.0).abs() < f32::EPSILON);
        assert!(material.roughness.abs() < f32::EPSILON);
        assert!((material.occlusion_strength - 1.0).abs() < f32::EPSILON);

        let mut material = Material::new();
        material.occlusion_strength = -0.2;
        assert!(material.validated().unwrap().occlusion_strength.abs() < f32::EPSILON);

        let mut material = Material::new();
        material.base_color[0] = f32::NAN;
        assert!(material.validated().is_none());

        let mut material = Material::new();
        material.occlusion_strength = f32::NAN;
        assert!(material.validated().is_none());
    }

    #[test]
    fn texture_asset_requires_exact_rgba8_storage() {
        assert_eq!(
            TextureAsset::rgba8(2, 1, Arc::<[u8]>::from([0; 7])).unwrap_err(),
            TextureError::InvalidDataLength {
                expected: 8,
                actual: 7,
            }
        );
        let texture = TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([1, 2, 3, 4])).unwrap();
        assert_eq!((texture.width(), texture.height()), (1, 1));
        assert_eq!(texture.pixels(), &[1, 2, 3, 4]);
        assert_ne!(
            texture.id(),
            TextureAsset::rgba8(1, 1, Arc::<[u8]>::from([1, 2, 3, 4]))
                .unwrap()
                .id()
        );
    }

    #[test]
    fn texture_asset_validates_and_exposes_mip_levels() {
        let texture =
            TextureAsset::rgba8_mip_chain(4, 4, [vec![1; 64], vec![2; 16], vec![3; 4]]).unwrap();
        assert_eq!(texture.mip_level_count(), 3);
        assert_eq!(texture.mip_extent(0), Some((4, 4)));
        assert_eq!(texture.mip_extent(1), Some((2, 2)));
        assert_eq!(texture.mip_extent(2), Some((1, 1)));
        assert_eq!(texture.mip_extent(3), None);
        assert_eq!(texture.mip_pixels(2), Some(&[3; 4][..]));
    }

    #[test]
    fn texture_asset_rejects_invalid_mip_chains() {
        assert_eq!(
            TextureAsset::rgba8_mip_chain(2, 2, Vec::<Vec<u8>>::new()).unwrap_err(),
            TextureError::EmptyMipChain
        );
        assert_eq!(
            TextureAsset::rgba8_mip_chain(2, 2, [vec![0; 16], vec![0; 5]]).unwrap_err(),
            TextureError::InvalidMipDataLength {
                level: 1,
                expected: 4,
                actual: 5,
            }
        );
        assert_eq!(
            TextureAsset::rgba8_mip_chain(1, 1, [vec![0; 4], vec![0; 4]]).unwrap_err(),
            TextureError::TooManyMipLevels
        );
    }
}
