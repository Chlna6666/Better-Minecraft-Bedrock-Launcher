pub(super) use super::tile_render_stable::*;

use bedrock_render::RenderLayout;

pub(super) const fn tile_texture_render_layout(
    world_layout: RenderLayout,
    _viewport_scale: f32,
) -> RenderLayout {
    RenderLayout {
        chunks_per_tile: world_layout.chunks_per_tile,
        blocks_per_pixel: 1,
        pixels_per_block: 1,
    }
}
