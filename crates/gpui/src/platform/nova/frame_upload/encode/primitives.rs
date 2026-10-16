use super::*;

impl FrameUpload {
    pub(super) fn encode_shadows(
        &mut self,
        shadows: &[crate::Shadow],
        summary: &mut FrameUploadSummary,
    ) {
        let first = (self.shadows.len() / PACKED_SHADOW_BYTES) as u32;
        let mut count = 0_u32;
        for shadow in shadows {
            if self.shadows.len() / PACKED_SHADOW_BYTES >= MAX_SHADOWS {
                break;
            }
            if clip_is_degenerate(&shadow.content_mask) {
                continue;
            }
            let primitive_index = (self.shadows.len() / PACKED_SHADOW_BYTES) as u32;
            write_shadow(&mut self.shadows, shadow);
            register_scene_animated_primitive(
                self,
                summary,
                shadow
                    .animation_id
                    .map(|_| crate::Primitive::Shadow(*shadow)),
                AnimatedPrimitiveKind::Shadow,
                primitive_index,
            );
            count = count.saturating_add(1);
        }
        if count > 0 {
            self.batches.push(UploadedBatch::Shadows { first, count });
            summary.shadow_count = summary.shadow_count.saturating_add(count);
        }
    }

    pub(super) fn encode_monochrome_sprites(
        &mut self,
        sprites: &[crate::MonochromeSprite],
        summary: &mut FrameUploadSummary,
    ) {
        let mut count = 0_u32;
        for sprite in sprites {
            if self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES >= MAX_MONO_SPRITES {
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
                sprite.transformation != crate::TransformationMatrix::unit(),
            );
            let primitive_index = (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
            write_monochrome_sprite(&mut self.mono_sprites, &sprite);
            self.mono_atlas_tiles.push(sprite.tile);
            register_scene_animated_primitive(
                self,
                summary,
                sprite
                    .animation_id
                    .map(|_| crate::Primitive::MonochromeSprite(sprite)),
                AnimatedPrimitiveKind::MonochromeSprite,
                primitive_index,
            );
            match self.batches.last_mut() {
                Some(UploadedBatch::MonoSprites {
                    texture_id: id,
                    first,
                    count,
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
                    self.batches.push(UploadedBatch::MonoSprites {
                        texture_id: sprite.tile.texture_id,
                        first: primitive_index,
                        count: 1,
                    });
                    self.batch_visual_bounds.push(visual_bounds);
                }
            }
            count = count.saturating_add(1);
        }
        if count > 0 {
            summary.mono_sprite_count = summary.mono_sprite_count.saturating_add(count);
        }
    }

    pub(super) fn encode_polychrome_sprites(
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

    pub(super) fn encode_underlines(
        &mut self,
        underlines: &[crate::Underline],
        summary: &mut FrameUploadSummary,
    ) {
        let first = (self.underlines.len() / PACKED_UNDERLINE_BYTES) as u32;
        let mut count = 0_u32;
        let mut visual_bounds = None;
        for underline in underlines {
            if self.underlines.len() / PACKED_UNDERLINE_BYTES >= MAX_UNDERLINES {
                break;
            }
            if clip_is_degenerate(&underline.content_mask) {
                continue;
            }
            let next = static_raster_visual_bounds(
                underline.bounds,
                underline.content_mask.bounds,
                underline.animation_id,
                false,
            );
            if count == 0 {
                visual_bounds = next;
            } else {
                merge_batch_bounds(&mut visual_bounds, next);
            }
            write_underline(&mut self.underlines, underline);
            count = count.saturating_add(1);
        }
        if count > 0 {
            self.batches
                .push(UploadedBatch::Underlines { first, count });
            self.batch_visual_bounds.push(visual_bounds);
            summary.underline_count = summary.underline_count.saturating_add(count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(x: f32) -> crate::Bounds<crate::ScaledPixels> {
        crate::bounds(
            crate::point(crate::ScaledPixels(x), crate::ScaledPixels(4.)),
            crate::size(crate::ScaledPixels(8.), crate::ScaledPixels(8.)),
        )
    }

    fn glyph(x: f32, sampling: crate::MonochromeSpriteSampling) -> crate::MonochromeSprite {
        crate::MonochromeSprite {
            order: 0,
            pad: sampling as u32,
            animation_id: None,
            bounds: area(x),
            content_mask: crate::ContentMask::new(area(x)),
            color: crate::rgb(0xffffff),
            tile: crate::AtlasTile {
                texture_id: crate::AtlasTextureId {
                    index: 0,
                    kind: crate::AtlasTextureKind::Monochrome,
                },
                tile_id: crate::TileId(0),
                padding: 1,
                bounds: crate::bounds(
                    crate::point(crate::DevicePixels(1), crate::DevicePixels(1)),
                    crate::size(crate::DevicePixels(8), crate::DevicePixels(8)),
                ),
            },
            transformation: crate::TransformationMatrix::unit(),
        }
    }

    fn encode(sprites: &[crate::MonochromeSprite]) -> FrameUpload {
        let mut painter = crate::scene::Painter::default();
        for sprite in sprites {
            painter.push_layer(sprite.content_mask.bounds);
            painter.insert_primitive(*sprite);
            painter.pop_layer();
        }
        painter.finish();
        assert_eq!(painter.prepared_batches().len(), sprites.len());
        let mut upload = FrameUpload::default();
        upload.encode(
            &mut RetainedQuadChunkCache::default(),
            &painter,
            &crate::window::retained::RetainedPaint::default(),
            &[],
            DrawableSize {
                width: 64,
                height: 64,
            },
            &RenderingParameters::system(),
            false,
            BackdropBlurQuality::Full,
        );
        upload
    }

    #[test]
    fn merged_glyph_batches_cover_every_sampling_run() {
        let first = glyph(4., crate::MonochromeSpriteSampling::Glyph);
        let second = glyph(40., crate::MonochromeSpriteSampling::Linear);
        let upload = encode(&[first, second]);
        assert_eq!(upload.batches.len(), 1);
        let bounds = upload.batch_visual_bounds[0].expect("static merged batch");
        assert_eq!(bounds.union(&first.bounds), bounds);
        assert_eq!(
            bounds.union(&second.bounds),
            bounds,
            "merged draw must cover the second sampling run"
        );
    }

    #[test]
    fn merged_glyph_batches_keep_animation_and_transform_uncullable() {
        for transformed in [false, true] {
            let first = glyph(4., crate::MonochromeSpriteSampling::Glyph);
            let mut second = glyph(40., crate::MonochromeSpriteSampling::Linear);
            if transformed {
                second.transformation = crate::TransformationMatrix::unit().translate(
                    crate::point(crate::ScaledPixels(12.), crate::ScaledPixels(0.)),
                );
            } else {
                second.animation_id = Some(crate::SceneAnimationId(7));
            }
            let upload = encode(&[first, second]);
            assert_eq!(upload.batches.len(), 1);
            assert_eq!(upload.batch_visual_bounds, vec![None]);
        }
    }

    #[test]
    fn relocated_sprite_draws_keep_independent_bounds_and_merge_coverage() {
        let source = glyph(4., crate::MonochromeSpriteSampling::Glyph);
        let sprite = |x, id, animation_id| crate::PolychromeSprite {
            order: 0,
            sampling: 0,
            grayscale: false,
            opacity: 1.,
            animation_id,
            bounds: area(x),
            content_mask: crate::ContentMask::new(area(x)),
            corner_radii: Default::default(),
            tile: crate::AtlasTile {
                tile_id: crate::TileId(id),
                texture_id: crate::AtlasTextureId {
                    kind: crate::AtlasTextureKind::Rgba,
                    ..source.tile.texture_id
                },
                ..source.tile
            },
        };
        let left = sprite(4., 1, None);
        let moving = sprite(24., 2, Some(crate::SceneAnimationId(5)));
        let right = sprite(44., 3, None);
        let mut upload = FrameUpload::default();
        upload.atlas_placements.insert(
            moving.tile.tile_id,
            crate::AtlasTile {
                texture_id: crate::AtlasTextureId {
                    index: 7,
                    ..moving.tile.texture_id
                },
                ..moving.tile
            },
        );
        let mut summary = FrameUploadSummary::default();
        upload.encode_polychrome_sprites(&[left, moving, right], &mut summary);
        assert_eq!(upload.batches.len(), 3);
        assert_eq!(
            upload.batch_visual_bounds,
            vec![
                Some(left.bounds.dilate(crate::ScaledPixels(2.))),
                None,
                Some(right.bounds.dilate(crate::ScaledPixels(2.))),
            ]
        );
        // A second prepared run relocated onto the last draw's page must extend it.
        let tail = sprite(54., 4, None);
        upload.encode_polychrome_sprites(&[tail], &mut summary);
        assert_eq!(upload.batches.len(), 3);
        let coverage = upload.batch_visual_bounds[2].expect("merged static image draw");
        assert_eq!(coverage.union(&tail.bounds), coverage);
    }

    #[test]
    fn underline_bounds_ignore_clipped_instances_but_preserve_animation() {
        let line = |x, animation_id| crate::Underline {
            order: 0,
            pad: 0,
            animation_id,
            bounds: area(x),
            content_mask: crate::ContentMask::new(area(x)),
            color: crate::rgb(0xffffff),
            thickness: crate::ScaledPixels(1.),
            wavy: 0,
        };
        let first = line(4., None);
        let mut clipped = line(40., Some(crate::SceneAnimationId(3)));
        clipped.content_mask.bounds.size.width = crate::ScaledPixels(0.);
        let mut upload = FrameUpload::default();
        let mut summary = FrameUploadSummary::default();
        upload.encode_underlines(&[first, clipped], &mut summary);
        assert_eq!(summary.underline_count, 1);
        assert_eq!(
            upload.batch_visual_bounds,
            vec![Some(first.bounds.dilate(crate::ScaledPixels(2.)))]
        );
        upload.encode_underlines(
            &[line(20., Some(crate::SceneAnimationId(3))), first],
            &mut summary,
        );
        assert_eq!(upload.batch_visual_bounds[1], None);
    }

    #[test]
    fn nonfinite_source_or_clip_never_becomes_cullable_after_intersection() {
        for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for component in 0..8 {
                let mut source = area(4.);
                let mut mask = area(4.);
                let coordinates = [
                    &mut source.origin.x.0,
                    &mut source.origin.y.0,
                    &mut source.size.width.0,
                    &mut source.size.height.0,
                    &mut mask.origin.x.0,
                    &mut mask.origin.y.0,
                    &mut mask.size.width.0,
                    &mut mask.size.height.0,
                ];
                *coordinates
                    .into_iter()
                    .nth(component)
                    .expect("eight geometry components") = invalid;
                assert_eq!(static_raster_visual_bounds(source, mask, None, false), None);
            }
        }
    }
}
