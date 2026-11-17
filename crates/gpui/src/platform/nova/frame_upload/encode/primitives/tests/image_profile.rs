//! Manual CPU A/B against the pre-specialization image encoder.
use super::*;
use std::hint::black_box;
use std::time::Instant;

impl FrameUpload {
    fn encode_image_reference(
        &mut self,
        sprites: &[crate::PolychromeSprite],
        summary: &mut FrameUploadSummary,
    ) {
        let mut count = 0_u32;
        for sprite in sprites {
            if self.poly_sprites.len() / PACKED_POLY_SPRITE_BYTES >= MAX_POLY_SPRITES {
                break;
            }
            if clip_is_degenerate(&sprite.content_mask) {
                continue;
            }
            let mut sprite = *sprite;
            if let Some(tile) = self.atlas_placements.get(&sprite.tile.tile_id) {
                sprite.tile = *tile;
            }
            let visual_bounds = static_raster_visual_bounds(
                sprite.bounds,
                sprite.content_mask.bounds,
                sprite.animation_id,
                false,
            );
            let primitive_index = (self.poly_sprites.len() / PACKED_POLY_SPRITE_BYTES) as u32;
            write_polychrome_sprite(&mut self.poly_sprites, &sprite);
            self.poly_atlas_tiles.push(sprite.tile);
            register_scene_animated_primitive(
                self,
                summary,
                sprite
                    .animation_id
                    .map(|_| crate::Primitive::PolychromeSprite(sprite)),
                AnimatedPrimitiveKind::PolychromeSprite,
                primitive_index,
            );
            match self.batches.last_mut() {
                Some(UploadedBatch::PolySprites {
                    texture_id: id,
                    first,
                    count,
                    ..
                }) if *id == sprite.tile.texture_id && *first + *count == primitive_index => {
                    *count += 1;
                    merge_batch_bounds(
                        self.batch_visual_bounds
                            .last_mut()
                            .expect("every encoded sprite draw has bounds"),
                        visual_bounds,
                    );
                }
                _ => {
                    self.batches.push(UploadedBatch::PolySprites {
                        texture_id: sprite.tile.texture_id,
                        first: primitive_index,
                        count: 1,
                        color_only: false,
                    });
                    self.batch_visual_bounds.push(visual_bounds);
                }
            }
            count = count.saturating_add(1);
        }
        if count > 0 {
            summary.poly_sprite_count = summary.poly_sprite_count.saturating_add(count);
        }
    }
}

#[test]
#[ignore = "manual image preparation overhead profile"]
fn image_preparation_profile() {
    let viewport = DrawableSize {
        width: 256,
        height: 256,
    };
    for count in [1, 256, 2048] {
        for kind in ["large", "small", "gray", "mixed"] {
            let input: Vec<_> = (0..count)
                .map(|i| {
                    let edge = if kind == "small" || (kind == "mixed" && i % 2 == 1) {
                        32.
                    } else {
                        192.
                    };
                    let bounds = crate::bounds(
                        crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
                        crate::size(crate::ScaledPixels(edge), crate::ScaledPixels(edge)),
                    );
                    crate::PolychromeSprite {
                        order: 0,
                        sampling: 0,
                        animation_id: None,
                        opacity: 1.,
                        grayscale: kind == "gray",
                        bounds,
                        content_mask: crate::ContentMask::new(bounds),
                        corner_radii: Default::default(),
                        tile: crate::AtlasTile {
                            texture_id: crate::AtlasTextureId {
                                index: 0,
                                kind: crate::AtlasTextureKind::Rgba,
                            },
                            ..glyph(0., crate::MonochromeSpriteSampling::Linear).tile
                        },
                    }
                })
                .collect();
            let mut frames = [FrameUpload::default(), FrameUpload::default()];
            let mut times = [Vec::new(), Vec::new()];
            let iterations = (4096 / count).max(2);
            for trial in 0..101 {
                for index in if trial % 2 == 0 { [0, 1] } else { [1, 0] } {
                    let frame = &mut frames[index];
                    let start = Instant::now();
                    for _ in 0..iterations {
                        frame.poly_sprites.clear();
                        frame.poly_atlas_tiles.clear();
                        frame.batches.clear();
                        frame.batch_visual_bounds.clear();
                        let mut summary = FrameUploadSummary::default();
                        if index == 0 {
                            frame.encode_image_reference(black_box(&input), &mut summary);
                        } else {
                            frame.encode_polychrome_sprites(
                                black_box(&input),
                                &mut summary,
                                Some(viewport),
                            );
                        }
                        black_box(&*frame);
                    }
                    if trial > 0 {
                        times[index].push(start.elapsed().as_nanos() / iterations as u128);
                    }
                }
            }
            assert_eq!(frames[0].poly_sprites, frames[1].poly_sprites);
            assert_eq!(frames[0].batch_visual_bounds, frames[1].batch_visual_bounds);
            let mut deltas: Vec<_> = times[0]
                .iter()
                .zip(&times[1])
                .map(|(a, b)| *b as i128 - *a as i128)
                .collect();
            deltas.sort_unstable();
            for times in &mut times {
                times.sort_unstable();
            }
            println!(
                "ImagePrepare/{kind}/count={count} CPU median_ns: original={} adapted={} paired_delta_per_image_ns_p10/median/p90={:.2}/{:.2}/{:.2}",
                times[0][50],
                times[1][50],
                deltas[10] as f64 / count as f64,
                deltas[50] as f64 / count as f64,
                deltas[90] as f64 / count as f64
            );
        }
    }
}
