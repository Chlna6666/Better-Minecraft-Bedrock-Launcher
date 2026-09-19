use super::*;

const INDEXED_ANIMATION_ENABLED_OFFSET: usize = 12;
const ANIMATION_VALUE_ACTIVE_OFFSET: usize = 12;
const UNDERLINE_ANIMATION_SLOT_OFFSET: usize = 4;
const PAINT_BLUR_COMPOSITE_KIND_OFFSET: usize = 132;
const PAINT_BLUR_COMPOSITE_KIND_MASK: u32 = 3;
const ELEMENT_COMPOSITE_KIND: u32 = 1;
const GPU_INDEXED_WORKING_SET_MIN_ENTRIES: usize = 64;
const GPU_INDEXED_WORKING_SET_TRIM_MULTIPLIER: usize = 4;

#[inline]
fn is_gpu_indexed_kind(kind: AnimatedPrimitiveKind) -> bool {
    matches!(
        kind,
        AnimatedPrimitiveKind::Quad
            | AnimatedPrimitiveKind::Shadow
            | AnimatedPrimitiveKind::MonochromeSprite
            | AnimatedPrimitiveKind::PolychromeSprite
    )
}

#[inline]
fn composite_animation_property(
    sampled_animation_values: &[crate::SceneAnimationValue],
    animation_id: crate::SceneAnimationId,
) -> Option<AnimationProperty> {
    sampled_animation_values
        .iter()
        .find(|value| value.animation_id == animation_id)
        .and_then(|value| AnimationProperty::from_transition_property(value.property))
}

#[inline]
fn can_promote_element_blur(
    sampled_animation_values: &[crate::SceneAnimationValue],
    primitive: &AnimatedUpload,
) -> bool {
    if primitive.kind != AnimatedPrimitiveKind::BackdropBlur
        || primitive.base_paint_blur().is_none()
    {
        return false;
    }
    matches!(
        composite_animation_property(sampled_animation_values, primitive.animation_id),
        Some(
            AnimationProperty::Opacity
                | AnimationProperty::Transform
                | AnimationProperty::Translation
                | AnimationProperty::Scale
                | AnimationProperty::ClipReveal
        )
    )
}

#[inline]
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("u32 field"))
}

#[inline]
fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn ensure_gpu_indexed_slot(
    slots: &mut FxHashMap<crate::SceneAnimationId, u32>,
    animation_id: crate::SceneAnimationId,
) -> Option<u32> {
    if let Some(&slot) = slots.get(&animation_id) {
        return Some(slot);
    }
    let slot = u32::try_from(slots.len()).ok()?;
    if slot as usize >= MAX_ANIMATION_VALUES {
        return None;
    }
    slots.insert(animation_id, slot);
    Some(slot)
}

fn trim_indexed_vec_capacity<T>(vec: &mut Vec<T>, working_set: usize, floor: usize) {
    let target = floor.max(working_set);
    if vec.capacity() > target.saturating_mul(GPU_INDEXED_WORKING_SET_TRIM_MULTIPLIER) {
        vec.shrink_to(target);
    }
}

fn underline_is_visible(underline: &crate::Underline) -> bool {
    underline.content_mask.bounds.size.width > crate::ScaledPixels(0.0)
        && underline.content_mask.bounds.size.height > crate::ScaledPixels(0.0)
}

fn collect_gpu_indexed_underline_owners(
    scene: &crate::Scene,
    output: &mut Vec<Option<crate::SceneAnimationId>>,
    packed_limit: usize,
) {
    if output.len() >= packed_limit {
        return;
    }
    for batch in scene.prepared_batches() {
        match batch {
            PreparedSceneBatch::Underlines(range) => {
                for underline in &scene.underlines[range.clone()] {
                    if output.len() >= packed_limit {
                        return;
                    }
                    if underline_is_visible(underline) {
                        output.push(underline.animation_id);
                    }
                }
            }
            PreparedSceneBatch::Blurs(range) => {
                for blur in &scene.blurs[range.clone()] {
                    collect_gpu_indexed_underline_owners(&blur.content, output, packed_limit);
                    if output.len() >= packed_limit {
                        return;
                    }
                }
            }
            _ => {}
        }
    }
}

impl FrameUpload {
    /// Captures animation ownership for packed primitive streams whose record ABI already has a
    /// spare lane but whose scene type is not represented by `AnimatedUpload`.
    ///
    /// This traversal deliberately mirrors recursive `encode_scene` order. Underline ownership can
    /// therefore be retained beside the static 96-byte records without adding a binding stream or
    /// touching the encode hot path.
    pub(in crate::platform::nova) fn prepare_gpu_indexed_special_primitives(
        &mut self,
        scene: &crate::Scene,
    ) {
        let packed_underline_count = self.underlines.len() / PACKED_UNDERLINE_BYTES;
        self.gpu_indexed_underline_animation_ids.clear();
        trim_indexed_vec_capacity(
            &mut self.gpu_indexed_underline_animation_ids,
            packed_underline_count,
            GPU_INDEXED_WORKING_SET_MIN_ENTRIES,
        );
        self.gpu_indexed_underline_animation_ids
            .reserve(packed_underline_count);
        collect_gpu_indexed_underline_owners(
            scene,
            &mut self.gpu_indexed_underline_animation_ids,
            packed_underline_count,
        );
        self.gpu_indexed_underline_animation_ids
            .resize(packed_underline_count, None);
        debug_assert_eq!(
            self.gpu_indexed_underline_animation_ids.len(),
            packed_underline_count
        );
    }

    /// Promotes ordinary 2D primitives to the renderer-owned indexed animation ABI.
    ///
    /// Quad, shadow, glyph and image records use their otherwise-unused first u32. Underlines keep
    /// draw order in the first lane and repurpose their existing second `pad` lane. Animation
    /// ownership therefore costs no additional primitive bytes and promoted records leave
    /// `animated_primitives`, eliminating per-frame CPU serialization for those streams.
    pub(in crate::platform::nova) fn promote_gpu_indexed_animations(&mut self) {
        self.gpu_indexed_animation_slots.clear();
        self.gpu_indexed_animation_values.clear();
        self.gpu_indexed_source_animation_ids.clear();
        self.gpu_indexed_composite_animation_ids.clear();
        self.gpu_indexed_composite_element_blur_animation_ids
            .clear();

        if self.globals.len() < INDEXED_ANIMATION_ENABLED_OFFSET + 4 {
            return;
        }
        write_u32(&mut self.globals, INDEXED_ANIMATION_ENABLED_OFFSET, 0);

        let mut slots_overflowed = false;
        for animation_id in self
            .gpu_indexed_underline_animation_ids
            .iter()
            .flatten()
            .copied()
        {
            if ensure_gpu_indexed_slot(&mut self.gpu_indexed_animation_slots, animation_id)
                .is_none()
            {
                slots_overflowed = true;
                break;
            }
        }
        if !slots_overflowed {
            let sampled_animation_values = &self.sampled_animation_values;
            for primitive in &self.animated_primitives {
                if !is_gpu_indexed_kind(primitive.kind)
                    && !can_promote_element_blur(sampled_animation_values, primitive)
                {
                    continue;
                }
                if ensure_gpu_indexed_slot(
                    &mut self.gpu_indexed_animation_slots,
                    primitive.animation_id,
                )
                .is_none()
                {
                    slots_overflowed = true;
                    break;
                }
            }
        }
        if slots_overflowed {
            debug_assert!(false, "nova indexed animation table exceeded capacity");
            self.gpu_indexed_animation_slots.clear();
            self.gpu_indexed_source_animation_ids.clear();
            self.gpu_indexed_composite_animation_ids.clear();
            self.gpu_indexed_composite_element_blur_animation_ids
                .clear();
            return;
        }

        if self.gpu_indexed_animation_slots.is_empty() {
            return;
        }

        // The second underline u32 is ABI padding. Clear every record when the global indexed
        // animation feature gate is enabled so static underlines remain an explicit zero sentinel.
        for index in 0..self.underlines.len() / PACKED_UNDERLINE_BYTES {
            write_u32(
                &mut self.underlines,
                index * PACKED_UNDERLINE_BYTES + UNDERLINE_ANIMATION_SLOT_OFFSET,
                0,
            );
        }
        for (index, animation_id) in self
            .gpu_indexed_underline_animation_ids
            .iter()
            .copied()
            .enumerate()
        {
            let Some(animation_id) = animation_id else {
                continue;
            };
            let slot_plus_one = self.gpu_indexed_animation_slots[&animation_id] + 1;
            write_u32(
                &mut self.underlines,
                index * PACKED_UNDERLINE_BYTES + UNDERLINE_ANIMATION_SLOT_OFFSET,
                slot_plus_one,
            );
            self.gpu_indexed_source_animation_ids.insert(animation_id);
        }

        let sampled_animation_values = &self.sampled_animation_values;
        for primitive in &self.animated_primitives {
            let Some(&slot) = self
                .gpu_indexed_animation_slots
                .get(&primitive.animation_id)
            else {
                continue;
            };
            let slot_plus_one = slot + 1;
            if can_promote_element_blur(sampled_animation_values, primitive) {
                let offset = primitive.index as usize * PACKED_BACKDROP_BLUR_BYTES;
                debug_assert!(
                    offset + PAINT_BLUR_COMPOSITE_KIND_OFFSET + 4 <= self.backdrop_blurs.len()
                );
                let composite_kind = read_u32(
                    &self.backdrop_blurs,
                    offset + PAINT_BLUR_COMPOSITE_KIND_OFFSET,
                ) & PAINT_BLUR_COMPOSITE_KIND_MASK;
                debug_assert_eq!(composite_kind, ELEMENT_COMPOSITE_KIND);
                write_u32(
                    &mut self.backdrop_blurs,
                    offset + PAINT_BLUR_COMPOSITE_KIND_OFFSET,
                    composite_kind | (slot_plus_one << 2),
                );
                self.gpu_indexed_composite_animation_ids
                    .insert(primitive.animation_id);
                if primitive
                    .base_paint_blur()
                    .is_some_and(|blur| blur.content.animation_ids().is_empty())
                {
                    self.gpu_indexed_composite_element_blur_animation_ids
                        .insert(primitive.index, primitive.animation_id);
                }
                continue;
            }
            if !is_gpu_indexed_kind(primitive.kind) {
                continue;
            }
            self.gpu_indexed_source_animation_ids
                .insert(primitive.animation_id);
            let (bytes, stride) = match primitive.kind {
                AnimatedPrimitiveKind::Quad => (&mut self.quads, PACKED_QUAD_BYTES),
                AnimatedPrimitiveKind::Shadow => (&mut self.shadows, PACKED_SHADOW_BYTES),
                AnimatedPrimitiveKind::MonochromeSprite => {
                    (&mut self.mono_sprites, PACKED_MONO_SPRITE_BYTES)
                }
                AnimatedPrimitiveKind::PolychromeSprite => {
                    (&mut self.poly_sprites, PACKED_POLY_SPRITE_BYTES)
                }
                AnimatedPrimitiveKind::BackdropBlur => {
                    unreachable!("filtered before indexed primitive patch")
                }
            };
            let offset = primitive.index as usize * stride;
            debug_assert!(offset + 4 <= bytes.len());
            write_u32(bytes, offset, slot_plus_one);
        }

        let sampled_animation_values = &self.sampled_animation_values;
        self.animated_primitives.retain(|primitive| {
            !is_gpu_indexed_kind(primitive.kind)
                && !can_promote_element_blur(sampled_animation_values, primitive)
        });
        write_u32(&mut self.globals, INDEXED_ANIMATION_ENABLED_OFFSET, 1);
    }

    /// GPU-indexed primitives are no longer present in `animated_primitives`, so backdrop source
    /// dependency analysis cannot cheaply recover their sampled geometry. While one of their
    /// timelines is active (or was active on the previous frame), conservatively disable retained
    /// blur self-damage suppression rather than risk reusing stale filtered pixels.
    pub(in crate::platform::nova) fn gpu_indexed_animation_affects_blur_history(&self) -> bool {
        self.sampled_animation_values.iter().any(|value| {
            self.gpu_indexed_source_animation_ids
                .contains(&value.animation_id)
        }) || self
            .backdrop_blur_previous_animation_ids
            .iter()
            .any(|id| self.gpu_indexed_source_animation_ids.contains(id))
    }

    /// Builds one dense, frame-local timeline table for all GPU-indexed ordinary primitives.
    /// The source scene stream remains compact; inactive retained slots stay zeroed so primitive
    /// slot indices never change while the static upload is reused.
    pub(in crate::platform::nova) fn rebuild_gpu_indexed_animation_values(&mut self) {
        let byte_len = self
            .gpu_indexed_animation_slots
            .len()
            .saturating_mul(PACKED_ANIMATION_VALUE_BYTES);
        self.gpu_indexed_animation_values.clear();
        trim_indexed_vec_capacity(
            &mut self.gpu_indexed_animation_values,
            byte_len,
            GPU_INDEXED_WORKING_SET_MIN_ENTRIES.saturating_mul(PACKED_ANIMATION_VALUE_BYTES),
        );
        self.gpu_indexed_animation_values.resize(byte_len, 0);
        if byte_len == 0 {
            return;
        }

        debug_assert_eq!(
            self.animation_values.len(),
            self.sampled_animation_values.len() * PACKED_ANIMATION_VALUE_BYTES
        );

        // CPU resolution preserves the first duplicate animation value. Iterate in reverse while
        // overwriting dense slots so the first source record wins here as well.
        for (value, source) in self.sampled_animation_values.iter().rev().zip(
            self.animation_values
                .chunks_exact(PACKED_ANIMATION_VALUE_BYTES)
                .rev(),
        ) {
            let Some(&slot) = self.gpu_indexed_animation_slots.get(&value.animation_id) else {
                continue;
            };
            let offset = slot as usize * PACKED_ANIMATION_VALUE_BYTES;
            let destination = &mut self.gpu_indexed_animation_values
                [offset..offset + PACKED_ANIMATION_VALUE_BYTES];
            destination.copy_from_slice(source);
            write_u32(destination, ANIMATION_VALUE_ACTIVE_OFFSET, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_glyph_style_animation_uses_one_gpu_slot() {
        let id = crate::SceneAnimationId(7);
        let primitive = |index| {
            AnimatedUpload::new(
                crate::Primitive::Quad(crate::Quad {
                    animation_id: Some(id),
                    ..Default::default()
                }),
                AnimatedPrimitiveKind::Quad,
                index,
            )
        };
        let mut upload = FrameUpload {
            globals: vec![0; GLOBAL_UPLOAD_BYTES],
            quads: vec![0; 2 * PACKED_QUAD_BYTES],
            animated_primitives: vec![primitive(0), primitive(1)],
            ..Default::default()
        };

        upload.promote_gpu_indexed_animations();

        assert_eq!(upload.gpu_indexed_animation_slots.len(), 1);
        assert_eq!(read_u32(&upload.quads, 0), 1);
        assert_eq!(read_u32(&upload.quads, PACKED_QUAD_BYTES), 1);
        assert!(upload.animated_primitives.is_empty());
        assert_eq!(
            read_u32(&upload.globals, INDEXED_ANIMATION_ENABLED_OFFSET),
            1
        );
    }

    #[test]
    fn element_blur_composite_uses_kind_high_bits_without_losing_overlap_flag() {
        let id = crate::SceneAnimationId(19);
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(10.0), crate::ScaledPixels(20.0)),
            crate::size(crate::ScaledPixels(120.0), crate::ScaledPixels(80.0)),
        );
        let blur = crate::PaintBlur {
            order: 1,
            animation_id: Some(id),
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
            content: std::sync::Arc::new(crate::Scene::default()),
        };
        let mut blur_bytes = Vec::new();
        write_paint_blur(
            &mut blur_bytes,
            &blur,
            DrawableSize {
                width: 640,
                height: 480,
            },
        );
        let mut upload = FrameUpload {
            globals: vec![0; GLOBAL_UPLOAD_BYTES],
            backdrop_blurs: blur_bytes,
            animated_primitives: vec![AnimatedUpload::new(
                crate::Primitive::Blur(blur),
                AnimatedPrimitiveKind::BackdropBlur,
                0,
            )],
            sampled_animation_values: vec![crate::SceneAnimationValue {
                animation_id: id,
                property: crate::TransitionProperty::Translation,
                progress: 0.5,
                from: [0.0; 4],
                to: [32.0, 0.0, 0.0, 0.0],
            }],
            ..Default::default()
        };

        upload.promote_gpu_indexed_animations();

        assert_eq!(upload.gpu_indexed_animation_slots.get(&id), Some(&0));
        assert_eq!(
            read_u32(&upload.backdrop_blurs, PAINT_BLUR_COMPOSITE_KIND_OFFSET),
            ELEMENT_COMPOSITE_KIND | (1 << 2)
        );
        assert_eq!(read_u32(&upload.backdrop_blurs, 12), 1);
        assert!(upload.animated_primitives.is_empty());
        assert!(!upload.gpu_indexed_animation_affects_blur_history());
        assert!(
            upload
                .gpu_indexed_composite_element_blur_animation_ids
                .contains_key(&0)
        );
    }

    #[test]
    fn shadow_animation_uses_gpu_slot() {
        let id = crate::SceneAnimationId(4);
        let shadow = AnimatedUpload::new(
            crate::Primitive::Shadow(crate::Shadow {
                animation_id: Some(id),
                ..Default::default()
            }),
            AnimatedPrimitiveKind::Shadow,
            0,
        );
        let mut upload = FrameUpload {
            globals: vec![0; GLOBAL_UPLOAD_BYTES],
            shadows: vec![0; PACKED_SHADOW_BYTES],
            animated_primitives: vec![shadow],
            ..Default::default()
        };

        upload.promote_gpu_indexed_animations();

        assert_eq!(upload.gpu_indexed_animation_slots.len(), 1);
        assert_eq!(read_u32(&upload.shadows, 0), 1);
        assert!(upload.animated_primitives.is_empty());
    }

    #[test]
    fn underline_animation_reuses_existing_padding_lane() {
        let id = crate::SceneAnimationId(9);
        let mut upload = FrameUpload {
            globals: vec![0; GLOBAL_UPLOAD_BYTES],
            underlines: vec![0; 2 * PACKED_UNDERLINE_BYTES],
            gpu_indexed_underline_animation_ids: vec![None, Some(id)],
            ..Default::default()
        };

        upload.promote_gpu_indexed_animations();

        assert_eq!(upload.gpu_indexed_animation_slots.len(), 1);
        assert_eq!(
            read_u32(&upload.underlines, UNDERLINE_ANIMATION_SLOT_OFFSET),
            0
        );
        assert_eq!(
            read_u32(
                &upload.underlines,
                PACKED_UNDERLINE_BYTES + UNDERLINE_ANIMATION_SLOT_OFFSET
            ),
            1
        );
        assert_eq!(
            read_u32(&upload.globals, INDEXED_ANIMATION_ENABLED_OFFSET),
            1
        );
    }

    #[test]
    fn dense_value_table_preserves_animation_abi_and_sets_active_flag() {
        let id = crate::SceneAnimationId(11);
        let value = crate::SceneAnimationValue {
            animation_id: id,
            property: crate::TransitionProperty::Translation,
            progress: 0.5,
            from: [0.0; 4],
            to: [20.0, 4.0, 0.0, 0.0],
        };
        let mut upload = FrameUpload::default();
        upload.gpu_indexed_animation_slots.insert(id, 0);
        upload.sampled_animation_values.push(value);
        write_animation_value(
            &mut upload.animation_values,
            id,
            AnimationProperty::Translation,
            value.progress,
            value.from,
            value.to,
        );

        upload
            .gpu_indexed_animation_values
            .reserve(4096 * PACKED_ANIMATION_VALUE_BYTES);
        let oversized_value_capacity = upload.gpu_indexed_animation_values.capacity();
        upload.rebuild_gpu_indexed_animation_values();

        assert!(
            upload.gpu_indexed_animation_values.capacity() < oversized_value_capacity,
            "indexed animation values should retire a one-frame high-water mark"
        );
        let retained_value_capacity = upload.gpu_indexed_animation_values.capacity();
        upload.rebuild_gpu_indexed_animation_values();
        assert_eq!(
            upload.gpu_indexed_animation_values.capacity(),
            retained_value_capacity,
            "stable indexed animation working sets should reuse the value buffer"
        );

        assert_eq!(
            upload.gpu_indexed_animation_values.len(),
            PACKED_ANIMATION_VALUE_BYTES
        );
        assert_eq!(read_u32(&upload.gpu_indexed_animation_values, 0), id.0);
        assert_eq!(
            read_u32(&upload.gpu_indexed_animation_values, 4),
            AnimationProperty::Translation as u32
        );
        assert_eq!(
            read_u32(
                &upload.gpu_indexed_animation_values,
                ANIMATION_VALUE_ACTIVE_OFFSET
            ),
            1
        );
    }
}
