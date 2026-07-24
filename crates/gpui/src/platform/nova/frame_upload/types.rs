use super::*;

const REDUCED_BACKDROP_BLUR_MIN_DOWNSAMPLE: u8 = 4;
const REDUCED_BACKDROP_BLUR_LEVELS: u8 = 1;
const REDUCED_BACKDROP_BLUR_RADIUS_SCALE: f32 = 0.5;
pub(in crate::platform::nova) const MAX_PATH_RASTERIZATION_CACHE_ENTRIES: usize = 4096;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(in crate::platform::nova) struct NovaPathRasterizationCacheKey {
    pub(in crate::platform::nova) path_id: crate::PathCacheId,
    pub(in crate::platform::nova) generation: crate::PathGeometryGeneration,
    pub(in crate::platform::nova) vertex_count: usize,
    pub(in crate::platform::nova) geometry_hash: u64,
    pub(in crate::platform::nova) paint_key: Vec<u8>,
}

#[derive(Clone, Debug)]
pub(in crate::platform::nova) struct NovaPathRasterizationCacheEntry {
    pub(in crate::platform::nova) bytes: Arc<[u8]>,
    pub(in crate::platform::nova) vertex_count: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::platform::nova) enum NovaBackdropBlurQuality {
    #[default]
    Full,
    Reduced,
    Disabled,
}

impl NovaBackdropBlurQuality {
    pub(in crate::platform::nova) fn adjusted_blur<'a>(
        self,
        blur: &'a crate::PaintBackdropBlur,
    ) -> Option<Cow<'a, crate::PaintBackdropBlur>> {
        match self {
            Self::Full => Some(Cow::Borrowed(blur)),
            Self::Reduced => {
                let mut blur = blur.clone();
                blur.radius = crate::ScaledPixels(
                    (blur.radius.0 * REDUCED_BACKDROP_BLUR_RADIUS_SCALE).max(0.0),
                );
                blur.downsample = blur.downsample.max(REDUCED_BACKDROP_BLUR_MIN_DOWNSAMPLE);
                blur.levels = REDUCED_BACKDROP_BLUR_LEVELS;
                Some(Cow::Owned(blur))
            }
            Self::Disabled => None,
        }
    }
}

#[derive(Default)]
pub(in crate::platform::nova) struct FrameUploadSummary {
    pub(in crate::platform::nova) quad_count: u32,
    pub(in crate::platform::nova) shadow_count: u32,
    pub(in crate::platform::nova) animation_binding_count: u32,
    pub(in crate::platform::nova) animation_value_count: u32,
    pub(in crate::platform::nova) path_vertex_count: u32,
    pub(in crate::platform::nova) path_sprite_count: u32,
    pub(in crate::platform::nova) mono_sprite_count: u32,
    pub(in crate::platform::nova) poly_sprite_count: u32,
    pub(in crate::platform::nova) underline_count: u32,
    pub(in crate::platform::nova) unsupported_batches: UnsupportedBatchSummary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(in crate::platform::nova) enum NovaAnimatedPrimitiveKind {
    Quad = 0,
    Shadow = 1,
    MonochromeSprite = 2,
    PolychromeSprite = 3,
    BackdropBlur = 4,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub(in crate::platform::nova) enum NovaAnimationProperty {
    Opacity = 0,
    Transform = 1,
    Translation = 2,
    Scale = 3,
    Rotation = 4,
    SolidColor = 5,
    BlurRadius = 6,
    Shadow = 7,
}

impl NovaAnimationProperty {
    pub(in crate::platform::nova) fn from_transition_property(
        property: crate::TransitionProperty,
    ) -> Option<Self> {
        match property {
            crate::TransitionProperty::Opacity => Some(Self::Opacity),
            crate::TransitionProperty::Transform => Some(Self::Transform),
            crate::TransitionProperty::Translation => Some(Self::Translation),
            crate::TransitionProperty::Scale => Some(Self::Scale),
            crate::TransitionProperty::Rotation => Some(Self::Rotation),
            crate::TransitionProperty::Color => Some(Self::SolidColor),
            crate::TransitionProperty::Blur => Some(Self::BlurRadius),
            crate::TransitionProperty::Shadow => Some(Self::Shadow),
            crate::TransitionProperty::Width
            | crate::TransitionProperty::Height
            | crate::TransitionProperty::Inset
            | crate::TransitionProperty::Margin
            | crate::TransitionProperty::Padding
            | crate::TransitionProperty::Gap
            | crate::TransitionProperty::BorderWidth => None,
        }
    }
}

#[derive(Clone, Copy, Default)]
pub(in crate::platform::nova) struct UnsupportedBatchSummary {
    pub(in crate::platform::nova) paths: u32,
    pub(in crate::platform::nova) surfaces: u32,
    pub(in crate::platform::nova) backdrop_blurs: u32,
    pub(in crate::platform::nova) backdrop_blur_tint_fallbacks: u32,
    pub(in crate::platform::nova) gpu_meshes_3d: u32,
}

impl UnsupportedBatchSummary {
    pub(in crate::platform::nova) fn total(self) -> u32 {
        self.paths
            .saturating_add(self.surfaces)
            .saturating_add(self.backdrop_blurs)
            .saturating_add(self.backdrop_blur_tint_fallbacks)
            .saturating_add(self.gpu_meshes_3d)
    }
}

#[derive(Clone, Copy)]
pub(in crate::platform::nova) enum NovaUploadedBatch {
    SolidQuads {
        first: u32,
        count: u32,
    },
    Quads {
        first: u32,
        count: u32,
    },
    Shadows {
        first: u32,
        count: u32,
    },
    PathRasterization {
        first_vertex: u32,
        vertex_count: u32,
    },
    Paths {
        first: u32,
        count: u32,
    },
    MonoSprites {
        texture_id: AtlasTextureId,
        first: u32,
        count: u32,
    },
    PolySprites {
        texture_id: AtlasTextureId,
        first: u32,
        count: u32,
    },
    Underlines {
        first: u32,
        count: u32,
    },
    BackdropBlurs {
        first: u32,
        count: u32,
    },
    CustomMesh3d {
        mesh_id: GpuMesh3dId,
        generation: u64,
        shader_id: GpuMesh3dShaderId,
        range: GpuMesh3dRange,
        first_parameter_index: u32,
    },
}

#[derive(Default)]
pub(in crate::platform::nova) struct NovaFrameUpload {
    pub(in crate::platform::nova) globals: Vec<u8>,
    pub(in crate::platform::nova) text_raster_params: Vec<u8>,
    pub(in crate::platform::nova) quads: Vec<u8>,
    pub(in crate::platform::nova) shadows: Vec<u8>,
    pub(in crate::platform::nova) path_rasterization_vertices: Vec<u8>,
    pub(in crate::platform::nova) path_sprites: Vec<u8>,
    pub(in crate::platform::nova) mono_sprites: Vec<u8>,
    pub(in crate::platform::nova) poly_sprites: Vec<u8>,
    pub(in crate::platform::nova) underlines: Vec<u8>,
    pub(in crate::platform::nova) backdrop_blur_passes: Vec<u8>,
    pub(in crate::platform::nova) backdrop_blurs: Vec<u8>,
    pub(in crate::platform::nova) animation_bindings: Vec<u8>,
    pub(in crate::platform::nova) animation_values: Vec<u8>,
    pub(in crate::platform::nova) custom_mesh_3d_parameters: Vec<u8>,
    pub(in crate::platform::nova) custom_mesh_3d_meshes: Vec<Arc<GpuMesh3d>>,
    pub(in crate::platform::nova) custom_mesh_3d_shaders: Vec<Arc<GpuMesh3dShader>>,
    pub(in crate::platform::nova) batches: Vec<NovaUploadedBatch>,
    pub(in crate::platform::nova) backdrop_blur_downsample: u8,
    pub(in crate::platform::nova) backdrop_blur_levels: u8,
    pub(in crate::platform::nova) path_rasterization_cache:
        FxHashMap<NovaPathRasterizationCacheKey, NovaPathRasterizationCacheEntry>,
    pub(in crate::platform::nova) path_rasterization_cache_hits: u64,
    pub(in crate::platform::nova) path_rasterization_cache_misses: u64,
}
