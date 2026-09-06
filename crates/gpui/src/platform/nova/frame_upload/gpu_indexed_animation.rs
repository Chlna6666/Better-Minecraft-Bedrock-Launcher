use super::*;

const INDEXED_ANIMATION_ENABLED_OFFSET: usize = 12;
const ANIMATION_BINDING_ID_OFFSET: usize = 0;
const ANIMATION_BINDING_KIND_OFFSET: usize = 4;
const ANIMATION_BINDING_INDEX_OFFSET: usize = 8;
const ANIMATION_VALUE_ACTIVE_OFFSET: usize = 12;

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
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_ne_bytes(bytes[offset..offset + 4].try_into().expect("u32 field"))
}

#[inline]
fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_ne_bytes());
}

fn clear_record_heads(bytes: &mut [u8], stride: usize) {
    for record in bytes.chunks_exact_mut(stride) {
        write_u32(record, 0, 0);
    }
}

impl FrameUpload {
    /// Promotes ordinary 2D primitives to the renderer-owned indexed animation ABI.
    ///
    /// Quad, shadow, glyph and image records already carry a draw-order u32 that Nova shaders
    /// never read. After scene batching has finished, reuse that lane as `animation_slot + 1`
    /// (zero means no animation). Many primitives can therefore reference one compact timeline
    /// record and retained animation frames no longer clone/mutate/re-serialize those primitives.
    pub(in crate::platform::nova) fn promote_gpu_indexed_animations(&mut self) {
        self.gpu_indexed_animation_slots.clear();
        self.gpu_indexed_animation_values.clear();

        if self.globals.len() < INDEXED_ANIMATION_ENABLED_OFFSET + 4 {
            return;
        }
        write_u32(&mut self.globals, INDEXED_ANIMATION_ENABLED_OFFSET, 0);

        let binding_count = self.animation_bindings.len() / PACKED_ANIMATION_BINDING_BYTES;
        if self.animation_bindings.len() % PACKED_ANIMATION_BINDING_BYTES != 0
            || binding_count != self.animated_primitives.len()
        {
            debug_assert!(
                false,
                "nova animation bindings and retained animated primitives diverged"
            );
            return;
        }

        for (primitive, binding) in self
            .animated_primitives
            .iter()
            .zip(self.animation_bindings.chunks_exact(PACKED_ANIMATION_BINDING_BYTES))
        {
            if !is_gpu_indexed_kind(primitive.kind) {
                continue;
            }
            debug_assert_eq!(
                read_u32(binding, ANIMATION_BINDING_KIND_OFFSET),
                primitive.kind as u32
            );
            debug_assert_eq!(
                read_u32(binding, ANIMATION_BINDING_INDEX_OFFSET),
                primitive.index
            );
            let animation_id =
                crate::SceneAnimationId(read_u32(binding, ANIMATION_BINDING_ID_OFFSET));
            if !self.gpu_indexed_animation_slots.contains_key(&animation_id) {
                let slot = u32::try_from(self.gpu_indexed_animation_slots.len())
                    .expect("nova animation slot count fits u32");
                if slot as usize >= MAX_ANIMATION_VALUES {
                    debug_assert!(false, "nova indexed animation table exceeded capacity");
                    self.gpu_indexed_animation_slots.clear();
                    return;
                }
                self.gpu_indexed_animation_slots.insert(animation_id, slot);
            }
        }

        if self.gpu_indexed_animation_slots.is_empty() {
            return;
        }

        // Draw order is consumed by Scene batching before packing and is never read by these Nova
        // shaders. Clear every record first so static/non-animated primitives retain the zero
        // sentinel and only promoted records receive a slot index.
        clear_record_heads(&mut self.quads, PACKED_QUAD_BYTES);
        clear_record_heads(&mut self.shadows, PACKED_SHADOW_BYTES);
        clear_record_heads(&mut self.mono_sprites, PACKED_MONO_SPRITE_BYTES);
        clear_record_heads(&mut self.poly_sprites, PACKED_POLY_SPRITE_BYTES);

        for (primitive, binding) in self
            .animated_primitives
            .iter()
            .zip(self.animation_bindings.chunks_exact(PACKED_ANIMATION_BINDING_BYTES))
        {
            if !is_gpu_indexed_kind(primitive.kind) {
                continue;
            }
            let animation_id =
                crate::SceneAnimationId(read_u32(binding, ANIMATION_BINDING_ID_OFFSET));
            let slot_plus_one = self.gpu_indexed_animation_slots[&animation_id] + 1;
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

        self.animated_primitives
            .retain(|primitive| !is_gpu_indexed_kind(primitive.kind));
        write_u32(&mut self.globals, INDEXED_ANIMATION_ENABLED_OFFSET, 1);
    }

    /// GPU-indexed primitives are no longer present in `animated_primitives`, so backdrop source
    /// dependency analysis cannot cheaply recover their sampled geometry. While one of their
    /// timelines is active (or was active on the previous frame), conservatively disable retained
    /// blur self-damage suppression rather than risk reusing stale filtered pixels.
    pub(in crate::platform::nova) fn gpu_indexed_animation_affects_blur_history(&self) -> bool {
        self.sampled_animation_values.iter().any(|value| {
            self.gpu_indexed_animation_slots
                .contains_key(&value.animation_id)
        }) || self.backdrop_blur_previous_animation_ids.iter().any(|id| {
            self.gpu_indexed_animation_slots.contains_key(id)
        })
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
            // The source ABI reserved this u32 as padding. Indexed shaders reinterpret it as the
            // active flag without changing the 64-byte storage stride.
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
        write_animation_binding(
            &mut upload.animation_bindings,
            id,
            AnimatedPrimitiveKind::Quad,
            0,
        );
        write_animation_binding(
            &mut upload.animation_bindings,
            id,
            AnimatedPrimitiveKind::Quad,
            1,
        );

        upload.promote_gpu_indexed_animations();

        assert_eq!(upload.gpu_indexed_animation_slots.len(), 1);
        assert_eq!(read_u32(&upload.quads, 0), 1);
        assert_eq!(read_u32(&upload.quads, PACKED_QUAD_BYTES), 1);
        assert!(upload.animated_primitives.is_empty());
        assert_eq!(read_u32(&upload.globals, INDEXED_ANIMATION_ENABLED_OFFSET), 1);
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
        write_animation_binding(
            &mut upload.animation_bindings,
            id,
            AnimatedPrimitiveKind::Shadow,
            0,
        );

        upload.promote_gpu_indexed_animations();

        assert_eq!(upload.gpu_indexed_animation_slots.len(), 1);
        assert_eq!(read_u32(&upload.shadows, 0), 1);
        assert!(upload.animated_primitives.is_empty());
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

        upload.rebuild_gpu_indexed_animation_values();

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
