use super::*;
use crate::{SceneAnimationValue, TransitionProperty};

pub(super) const MESH_ANIMATION_PROPERTY_NONE: u32 = 0;
pub(super) const MESH_ANIMATION_PROPERTY_OPACITY: u32 = 1;
pub(super) const MESH_ANIMATION_PROPERTY_SCALE: u32 = 2;
pub(super) const MESH_ANIMATION_PROPERTY_TRANSLATION: u32 = 3;
pub(super) const MESH_ANIMATION_PROPERTY_TRANSFORM: u32 = 4;

#[derive(Clone, Copy)]
struct ResolvedMeshAnimation {
    property: u32,
    sampled: [f32; 4],
}

impl ResolvedMeshAnimation {
    fn from_scene_value(value: &SceneAnimationValue) -> Option<Self> {
        let progress = if value.progress.is_finite() {
            value.progress
        } else {
            0.0
        };
        let mut sampled = std::array::from_fn(|index| {
            value.from[index] + (value.to[index] - value.from[index]) * progress
        });
        let property = match value.property {
            TransitionProperty::Opacity => {
                sampled[0] = sampled[0].clamp(0.0, 1.0);
                MESH_ANIMATION_PROPERTY_OPACITY
            }
            TransitionProperty::Scale => {
                sampled[0] = sanitized_scale(sampled[0]);
                MESH_ANIMATION_PROPERTY_SCALE
            }
            TransitionProperty::Translation => MESH_ANIMATION_PROPERTY_TRANSLATION,
            TransitionProperty::Transform => {
                sampled[0] = sanitized_scale(sampled[0]);
                sampled[1] = sampled[1].clamp(0.0, 1.0);
                MESH_ANIMATION_PROPERTY_TRANSFORM
            }
            // Rotation remains a subtree/composite transform. Raw custom meshes must not rotate
            // independently or nested animation ownership would diverge from ordinary primitives.
            _ => return None,
        };
        Some(Self { property, sampled })
    }
}

#[inline]
fn sanitized_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.max(0.0)
    } else {
        1.0
    }
}

impl FrameUpload {
    /// Rebuilds the compact per-draw custom-mesh animation sidecar.
    ///
    /// The record index is deliberately identical to the custom-mesh draw-parameter index, so
    /// shaders resolve animation state with one `animation[instance_index]` storage-buffer load.
    /// Retained frames therefore rewrite only this sidecar instead of mesh parameters or vertices.
    pub(super) fn rebuild_custom_mesh_3d_animations(&mut self) {
        self.custom_mesh_3d_animations.clear();
        let draw_count = self.custom_mesh_3d_animation_ids.len();
        if draw_count == 0 {
            return;
        }
        self.custom_mesh_3d_animations
            .reserve(draw_count.saturating_mul(PACKED_CUSTOM_MESH_3D_ANIMATION_BYTES));

        let mut resolved = FxHashMap::default();
        resolved.reserve(self.sampled_animation_values.len());
        for value in &self.sampled_animation_values {
            let Some(resolved_value) = ResolvedMeshAnimation::from_scene_value(value) else {
                continue;
            };
            // Match Nova's ordinary animation resolver: the first value for a duplicate id wins.
            resolved
                .entry(value.animation_id)
                .or_insert(resolved_value);
        }

        for animation_id in &self.custom_mesh_3d_animation_ids {
            write_mesh_animation_record(
                &mut self.custom_mesh_3d_animations,
                animation_id
                    .as_ref()
                    .and_then(|animation_id| resolved.get(animation_id).copied()),
            );
        }
        debug_assert_eq!(
            self.custom_mesh_3d_animations.len(),
            draw_count * PACKED_CUSTOM_MESH_3D_ANIMATION_BYTES
        );
    }
}

fn write_mesh_animation_record(bytes: &mut Vec<u8>, value: Option<ResolvedMeshAnimation>) {
    let start = bytes.len();
    bytes.resize(start + PACKED_CUSTOM_MESH_3D_ANIMATION_BYTES, 0);
    let Some(value) = value else {
        return;
    };
    bytes[start..start + 4].copy_from_slice(&value.property.to_ne_bytes());
    bytes[start + 4..start + 8].copy_from_slice(&1u32.to_ne_bytes());
    for (index, component) in value.sampled.into_iter().enumerate() {
        let offset = start + 16 + index * 4;
        bytes[offset..offset + 4].copy_from_slice(&component.to_ne_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SceneAnimationId, SceneAnimationValue};

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn read_f32(bytes: &[u8], offset: usize) -> f32 {
        f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn mesh_animation_sidecar_is_draw_indexed_and_preserves_first_value() {
        let translation_id = SceneAnimationId(7);
        let opacity_id = SceneAnimationId(8);
        let mut upload = FrameUpload::default();
        upload.custom_mesh_3d_animation_ids = vec![Some(translation_id), None, Some(opacity_id)];
        upload.sampled_animation_values = vec![
            SceneAnimationValue {
                animation_id: translation_id,
                property: TransitionProperty::Translation,
                progress: 0.5,
                from: [2.0, 4.0, 0.0, 0.0],
                to: [10.0, 20.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: translation_id,
                property: TransitionProperty::Translation,
                progress: 1.0,
                from: [0.0; 4],
                to: [99.0, 99.0, 0.0, 0.0],
            },
            SceneAnimationValue {
                animation_id: opacity_id,
                property: TransitionProperty::Opacity,
                progress: 0.5,
                from: [1.0, 0.0, 0.0, 0.0],
                to: [-1.0, 0.0, 0.0, 0.0],
            },
        ];

        upload.rebuild_custom_mesh_3d_animations();

        assert_eq!(
            upload.custom_mesh_3d_animations.len(),
            3 * PACKED_CUSTOM_MESH_3D_ANIMATION_BYTES
        );
        let first = 0;
        assert_eq!(
            read_u32(&upload.custom_mesh_3d_animations, first),
            MESH_ANIMATION_PROPERTY_TRANSLATION
        );
        assert_eq!(read_u32(&upload.custom_mesh_3d_animations, first + 4), 1);
        assert_eq!(read_f32(&upload.custom_mesh_3d_animations, first + 16), 6.0);
        assert_eq!(read_f32(&upload.custom_mesh_3d_animations, first + 20), 12.0);

        let second = PACKED_CUSTOM_MESH_3D_ANIMATION_BYTES;
        assert_eq!(
            read_u32(&upload.custom_mesh_3d_animations, second),
            MESH_ANIMATION_PROPERTY_NONE
        );
        assert_eq!(read_u32(&upload.custom_mesh_3d_animations, second + 4), 0);

        let third = 2 * PACKED_CUSTOM_MESH_3D_ANIMATION_BYTES;
        assert_eq!(
            read_u32(&upload.custom_mesh_3d_animations, third),
            MESH_ANIMATION_PROPERTY_OPACITY
        );
        assert_eq!(read_u32(&upload.custom_mesh_3d_animations, third + 4), 1);
        assert_eq!(read_f32(&upload.custom_mesh_3d_animations, third + 16), 0.0);
    }
}
