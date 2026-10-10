//! Buffer identities and capacities travel together when a slot adopts a resident version.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::platform::nova) enum BufferStream {
    TextRaster,
    Quad,
    Shadow,
    PathVertices,
    PathSprites,
    MonoSprites,
    PolySprites,
    Underlines,
    BlurPasses,
    BlurRecords,
}

impl BufferStream {
    pub(in crate::platform::nova) const ALL: [Self; 10] = [
        Self::TextRaster,
        Self::Quad,
        Self::Shadow,
        Self::PathVertices,
        Self::PathSprites,
        Self::MonoSprites,
        Self::PolySprites,
        Self::Underlines,
        Self::BlurPasses,
        Self::BlurRecords,
    ];

    pub(in crate::platform::nova) fn allocation(
        self,
        buffers: FrameResourceBuffers,
    ) -> (BufferId, usize, usize, usize) {
        match self {
            Self::TextRaster => (buffers.text_raster_buffer, 1, TEXT_RASTER_UPLOAD_BYTES, 1),
            Self::Quad => (
                buffers.quad_buffer,
                buffers.quad_capacity,
                PACKED_QUAD_BYTES,
                MAX_QUADS,
            ),
            Self::Shadow => (
                buffers.shadow_buffer,
                buffers.shadow_capacity,
                PACKED_SHADOW_BYTES,
                MAX_SHADOWS,
            ),
            Self::PathVertices => (
                buffers.path_rasterization_vertex_buffer,
                buffers.path_rasterization_vertex_capacity,
                PACKED_PATH_RASTERIZATION_VERTEX_BYTES,
                MAX_PATH_VERTICES,
            ),
            Self::PathSprites => (
                buffers.path_sprite_buffer,
                buffers.path_sprite_capacity,
                PACKED_PATH_SPRITE_BYTES,
                MAX_PATH_SPRITES,
            ),
            Self::MonoSprites => (
                buffers.mono_sprite_buffer,
                buffers.mono_sprite_capacity,
                PACKED_MONO_SPRITE_BYTES,
                MAX_MONO_SPRITES,
            ),
            Self::PolySprites => (
                buffers.poly_sprite_buffer,
                buffers.poly_sprite_capacity,
                PACKED_POLY_SPRITE_BYTES,
                MAX_POLY_SPRITES,
            ),
            Self::Underlines => (
                buffers.underline_buffer,
                MAX_UNDERLINES,
                PACKED_UNDERLINE_BYTES,
                MAX_UNDERLINES,
            ),
            Self::BlurPasses => (
                buffers.backdrop_blur_pass_buffer,
                MAX_BACKDROP_BLURS * 2,
                BACKDROP_BLUR_PASS_BYTES,
                MAX_BACKDROP_BLURS * 2,
            ),
            Self::BlurRecords => (
                buffers.backdrop_blur_buffer,
                MAX_BACKDROP_BLURS,
                PACKED_BACKDROP_BLUR_BYTES,
                MAX_BACKDROP_BLURS,
            ),
        }
    }

    pub(in crate::platform::nova) fn replace(
        self,
        buffers: &mut FrameResourceBuffers,
        buffer: BufferId,
        capacity: usize,
    ) {
        match self {
            Self::TextRaster => buffers.text_raster_buffer = buffer,
            Self::Quad => {
                buffers.quad_buffer = buffer;
                buffers.quad_capacity = capacity;
            }
            Self::Shadow => {
                buffers.shadow_buffer = buffer;
                buffers.shadow_capacity = capacity;
            }
            Self::PathVertices => {
                buffers.path_rasterization_vertex_buffer = buffer;
                buffers.path_rasterization_vertex_capacity = capacity;
            }
            Self::PathSprites => {
                buffers.path_sprite_buffer = buffer;
                buffers.path_sprite_capacity = capacity;
            }
            Self::MonoSprites => {
                buffers.mono_sprite_buffer = buffer;
                buffers.mono_sprite_capacity = capacity;
            }
            Self::PolySprites => {
                buffers.poly_sprite_buffer = buffer;
                buffers.poly_sprite_capacity = capacity;
            }
            Self::Underlines => buffers.underline_buffer = buffer,
            Self::BlurPasses => buffers.backdrop_blur_pass_buffer = buffer,
            Self::BlurRecords => buffers.backdrop_blur_buffer = buffer,
        }
    }
}

impl FrameResourceBuffers {
    pub(in crate::platform::nova) fn ids(self) -> [BufferId; 12] {
        [
            self.global_buffer,
            self.text_raster_buffer,
            self.quad_buffer,
            self.shadow_buffer,
            self.path_rasterization_vertex_buffer,
            self.path_sprite_buffer,
            self.mono_sprite_buffer,
            self.poly_sprite_buffer,
            self.underline_buffer,
            self.backdrop_blur_pass_buffer,
            self.backdrop_blur_buffer,
            self.animation_value_buffer,
        ]
    }
}
