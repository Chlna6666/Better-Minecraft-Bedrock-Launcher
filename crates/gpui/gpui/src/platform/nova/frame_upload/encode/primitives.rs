use super::*;

impl FrameUpload {
    pub(super) fn encode_shadows<'a>(
        &mut self,
        shadows: impl IntoIterator<Item = &'a crate::Shadow>,
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
        for sprite in sprites {
            if self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES >= MAX_MONO_SPRITES {
                break;
            }
            let Some((sprite, visual_bounds)) = self.prepare_monochrome_sprite(sprite) else {
                continue;
            };
            let primitive_index = (self.mono_sprites.len() / PACKED_MONO_SPRITE_BYTES) as u32;
            self.mono_sprites
                .write(|bytes| write_monochrome_sprite(bytes, &sprite));
            self.record_monochrome_sprite(&sprite, primitive_index, visual_bounds, summary);
        }
    }

    pub(super) fn prepare_monochrome_sprite(
        &self,
        source: &crate::MonochromeSprite,
    ) -> Option<(
        crate::MonochromeSprite,
        Option<crate::Bounds<crate::ScaledPixels>>,
    )> {
        if clip_is_degenerate(&source.content_mask) {
            return None;
        }
        let mut sprite = *source;
        if let Some(tile) = self.atlas_placements.get(&sprite.tile.tile_id) {
            sprite.tile = *tile;
        }
        let visual_bounds = static_raster_visual_bounds(
            sprite.bounds,
            sprite.content_mask.bounds,
            sprite.animation_id,
            sprite.transformation != crate::TransformationMatrix::unit(),
        );
        Some((sprite, visual_bounds))
    }

    pub(super) fn record_monochrome_sprite(
        &mut self,
        sprite: &crate::MonochromeSprite,
        primitive_index: u32,
        visual_bounds: Option<crate::Bounds<crate::ScaledPixels>>,
        summary: &mut FrameUploadSummary,
    ) {
        self.mono_atlas_tiles.push(sprite.tile);
        register_scene_animated_primitive(
            self,
            summary,
            sprite
                .animation_id
                .map(|_| crate::Primitive::MonochromeSprite(*sprite)),
            AnimatedPrimitiveKind::MonochromeSprite,
            primitive_index,
        );
        self.append_monochrome_sprite_batch(
            sprite.tile.texture_id,
            primitive_index,
            1,
            visual_bounds,
        );
        summary.mono_sprite_count = summary.mono_sprite_count.saturating_add(1);
    }

    pub(super) fn append_monochrome_sprite_batch(
        &mut self,
        texture_id: crate::AtlasTextureId,
        first: u32,
        count: u32,
        visual_bounds: Option<crate::Bounds<crate::ScaledPixels>>,
    ) {
        if count == 0 {
            return;
        }
        match self.batches.last_mut() {
            Some(UploadedBatch::MonoSprites {
                texture_id: previous_texture_id,
                first: previous_first,
                count: previous_count,
            }) if *previous_texture_id == texture_id
                && *previous_first + *previous_count == first =>
            {
                *previous_count += count;
                merge_batch_bounds(
                    self.batch_visual_bounds
                        .last_mut()
                        .expect("every encoded sprite draw has bounds"),
                    visual_bounds,
                );
            }
            _ => {
                self.batches.push(UploadedBatch::MonoSprites {
                    texture_id,
                    first,
                    count,
                });
                self.batch_visual_bounds.push(visual_bounds);
            }
        }
    }

    pub(super) fn encode_polychrome_sprites(
        &mut self,
        sprites: &[crate::PolychromeSprite],
        summary: &mut FrameUploadSummary,
        full_resolution_viewport: Option<DrawableSize>,
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
                    color_only,
                }) if *id == sprite.tile.texture_id && *first + *count == primitive_index => {
                    if *color_only
                        && !can_use_image_color_pipeline(
                            &sprite,
                            visual_bounds,
                            full_resolution_viewport,
                        )
                    {
                        *color_only = false;
                    }
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
                        color_only: can_use_image_color_pipeline(
                            &sprite,
                            visual_bounds,
                            full_resolution_viewport,
                        ),
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
mod tests;

// Only large, fully visible static color images amortize a specialized pipeline.
const MIN_COLOR_IMAGE_AREA: f32 = 128.0 * 128.0;

#[inline]
fn can_use_image_color_pipeline(
    sprite: &crate::PolychromeSprite,
    visual_bounds: Option<crate::Bounds<crate::ScaledPixels>>,
    full_resolution_viewport: Option<DrawableSize>,
) -> bool {
    if sprite.grayscale
        || sprite.animation_id.is_some()
        || !(sprite.opacity > 0.0)
        || sprite.bounds.size.width.0 * sprite.bounds.size.height.0 < MIN_COLOR_IMAGE_AREA
    {
        return false;
    }
    let Some((visual, viewport)) = visual_bounds.zip(full_resolution_viewport) else {
        return false;
    };
    // These bounds are already finite. Clamp scalars directly instead of using
    // ScaledPixels' total ordering for a second Bounds intersection.
    // Damage bounds include a two-pixel raster margin, not image coverage.
    let left = visual.origin.x.0 + 2.0;
    let top = visual.origin.y.0 + 2.0;
    let right = visual.right().0 - 2.0;
    let bottom = visual.bottom().0 - 2.0;
    let width = (right.min(viewport.width as f32) - left.max(0.0)).max(0.0);
    let height = (bottom.min(viewport.height as f32) - top.max(0.0)).max(0.0);
    // Heavily clipped images did not show a reliable gain on every measured backend.
    width * height >= MIN_COLOR_IMAGE_AREA
        && width >= sprite.bounds.size.width.0
        && height >= sprite.bounds.size.height.0
}
