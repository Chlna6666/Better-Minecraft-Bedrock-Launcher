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
        _texture_id: crate::AtlasTextureId,
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
                }
                _ => self.batches.push(UploadedBatch::MonoSprites {
                    texture_id: sprite.tile.texture_id,
                    first: primitive_index,
                    count: 1,
                }),
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
        _texture_id: crate::AtlasTextureId,
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
                }
                _ => self.batches.push(UploadedBatch::PolySprites {
                    texture_id: sprite.tile.texture_id,
                    first: primitive_index,
                    count: 1,
                }),
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
        for underline in underlines {
            if self.underlines.len() / PACKED_UNDERLINE_BYTES >= MAX_UNDERLINES {
                break;
            }
            if clip_is_degenerate(&underline.content_mask) {
                continue;
            }
            write_underline(&mut self.underlines, underline);
            count = count.saturating_add(1);
        }
        if count > 0 {
            self.batches
                .push(UploadedBatch::Underlines { first, count });
            summary.underline_count = summary.underline_count.saturating_add(count);
        }
    }
}
