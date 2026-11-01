use super::*;

#[test]
fn sprites_group_by_texture_and_sampling_within_draw_order() {
    let mut scene = Painter::default();
    for (index, (texture_index, sampling)) in [(0, 0), (1, 0), (0, 1), (1, 1), (0, 0), (0, 1)]
        .into_iter()
        .enumerate()
    {
        let mut sprite = monochrome_sprite(0, sampling);
        sprite.bounds.origin.x = ScaledPixels(index as f32 * 1.5);
        sprite.tile.texture_id.index = texture_index;
        sprite.tile.tile_id = crate::TileId(index as u32);
        scene.insert_primitive(sprite);
    }
    use crate::AtlasTextureKind::{Bgra, Rgba};
    for (index, (texture_index, kind)) in [
        (0, Bgra),
        (0, Rgba),
        (1, Bgra),
        (0, Bgra),
        (0, Rgba),
        (1, Bgra),
    ]
    .into_iter()
    .enumerate()
    {
        let mut tile = monochrome_sprite(0, 0).tile;
        tile.texture_id = AtlasTextureId {
            index: texture_index,
            kind,
        };
        tile.tile_id = crate::TileId(index as u32);
        scene.insert_primitive(PolychromeSprite {
            order: 0,
            sampling: 0,
            grayscale: false,
            opacity: 1.0,
            animation_id: None,
            bounds: bounds(
                point(ScaledPixels(index as f32 * 1.5), ScaledPixels(2.0)),
                size(ScaledPixels(1.0), ScaledPixels(1.0)),
            ),
            content_mask: monochrome_sprite(0, 0).content_mask,
            corner_radii: Default::default(),
            tile,
        });
    }
    scene.finish();

    let monochrome_batches = scene
        .batches()
        .filter_map(|batch| match batch {
            PrimitiveBatch::MonochromeSprites {
                texture_id,
                sampling,
                sprites,
            } => Some((texture_id.index, sampling as u32, sprites.len())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        monochrome_batches,
        [(0, 0, 2), (0, 1, 2), (1, 0, 1), (1, 1, 1)]
    );
    let polychrome_batches = scene
        .batches()
        .filter_map(|batch| match batch {
            PrimitiveBatch::PolychromeSprites {
                texture_id,
                sprites,
            } => Some((texture_id.kind, texture_id.index, sprites.len())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        polychrome_batches,
        [(Bgra, 0, 2), (Bgra, 1, 2), (Rgba, 0, 2)]
    );
}

#[test]
fn overlapping_sprites_preserve_draw_order_across_textures() {
    let mut scene = Painter::default();
    for (index, texture_index) in [0, 1, 0].into_iter().enumerate() {
        let mut sprite = monochrome_sprite(0, 0);
        sprite.tile.texture_id.index = texture_index;
        sprite.tile.tile_id = crate::TileId(index as u32);
        scene.insert_primitive(sprite);
    }
    scene.finish();

    let batches = scene
        .batches()
        .filter_map(|batch| match batch {
            PrimitiveBatch::MonochromeSprites { texture_id, .. } => Some(texture_id.index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(batches, [0, 1, 0]);
}
