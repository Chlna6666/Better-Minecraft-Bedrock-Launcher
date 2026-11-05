//! Reuse already-encoded CPU instance rows until their GPU-visible fields change.
//!
//! Draw ordering is owned by PreparedScene, so cache keys are positional:
//! adding/removing a section re-encodes shifted slots, while unchanged
//! positions retain their previous bytes. GPU buffer layouts are unchanged.

use super::*;
use std::ops::Range;

/// CPU and GPU mesh fields contributing to one 144-byte instance record.
/// Mesh relocation and procedural geometry are reflected in vertex_word.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct InstanceSlotKey {
    node: NodeId,
    world: Mat4,
    pixel_offset: crate::Vec2,
    depth_bias: f32,
    vertex_word: u32,
    index_count_override: Option<u32>,
}

/// Copy matching encoded records; encode contiguous dirty rows as one run.
/// This preserves the canonical encoder's cross-part inverse-matrix sharing.
pub(super) fn encode_cached_records<K: PartialEq, E>(
    current: &[K],
    previous: &[K],
    previous_bytes: &[u8],
    stride: usize,
    output: &mut Vec<u8>,
    mut encode_range: impl FnMut(Range<usize>, &mut Vec<u8>) -> Result<(), E>,
) -> Result<usize, E> {
    debug_assert!(stride > 0);
    output.clear();
    output.reserve(current.len().max(1).saturating_mul(stride));
    let mut dirty_start = None;
    let mut encoded = 0usize;
    for (index, key) in current.iter().enumerate() {
        let start = index * stride;
        let end = start + stride;
        let reusable = previous.get(index) == Some(key)
            && previous_bytes.get(start..end).is_some();
        if reusable {
            if let Some(first) = dirty_start.take() {
                encode_range(first..index, output)?;
                encoded += index - first;
            }
            output.extend_from_slice(&previous_bytes[start..end]);
        } else if dirty_start.is_none() {
            dirty_start = Some(index);
        }
    }
    if let Some(first) = dirty_start {
        encode_range(first..current.len(), output)?;
        encoded += current.len() - first;
    }
    if current.is_empty() {
        output.resize(stride, 0);
    }
    debug_assert_eq!(output.len(), current.len().max(1) * stride);
    Ok(encoded)
}

impl SceneResources {
    fn instance_key(
        &self,
        draw: &PreparedDraw,
        voxel_shader: bool,
    ) -> gpui::Result<InstanceSlotKey> {
        let (vertex_word, index_count_override) = if voxel_shader {
            let mesh = self.meshes.get(&mesh_key(draw, true))
                .ok_or_else(|| anyhow!("GPUI 3D mesh was not uploaded"))?;
            let count = (self.culling.is_some() && mesh.procedural_quads)
                .then(|| draw.part.index_count());
            let vertex = if mesh.procedural_quads {
                mesh.vertex_base
                    .checked_add(
                        draw.part.first_index() / 6
                            * if mesh.generated.is_some() { 1 } else { 4 },
                    )
                    .filter(|base| *base < 0x8000_0000)
                    .ok_or_else(|| anyhow!("GPUI 3D voxel quad address exceeds shader range"))?
                    | 0x8000_0000
            } else {
                mesh.vertex_base
            };
            (vertex, count)
        } else {
            (0, None)
        };
        Ok(InstanceSlotKey {
            node: draw.node,
            world: draw.world,
            pixel_offset: draw.pixel_offset,
            depth_bias: draw.depth_bias,
            vertex_word,
            index_count_override,
        })
    }

    /// Re-encode only changed instance rows. All full encodes are delegated
    /// to the existing scene instance encoder to preserve GPU byte format.
    pub(super) fn encode_draw_instances_cached(
        &self,
        draws: &[PreparedDraw],
        voxel_shader: bool,
        previous_keys: &[InstanceSlotKey],
        previous_bytes: &[u8],
        next_keys: &mut Vec<InstanceSlotKey>,
        bytes: &mut Vec<u8>,
        run_scratch: &mut Vec<u8>,
    ) -> gpui::Result<usize> {
        next_keys.clear();
        next_keys.reserve(draws.len());
        for draw in draws {
            next_keys.push(self.instance_key(draw, voxel_shader)?);
        }
        encode_cached_records(
            next_keys,
            previous_keys,
            previous_bytes,
            INSTANCE_STRIDE as usize,
            bytes,
            |range, output| {
                self.encode_draw_instances(&draws[range], voxel_shader, run_scratch)?;
                output.extend_from_slice(run_scratch);
                Ok(())
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::encode_cached_records;

    #[test]
    fn stable_slots_skip_encoding_and_shifted_slots_reencode() {
        let previous_keys = [10, 20, 30, 40, 50];
        let previous_bytes = [10, 20, 30, 40, 50];
        let mut output = Vec::new();
        let mut runs = Vec::new();
        let current = [10, 21, 30, 41, 50];
        let count = encode_cached_records(
            &current, &previous_keys, &previous_bytes, 1, &mut output,
            |range, output| {
                runs.push(range.clone());
                output.extend_from_slice(&current[range]);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 2);
        assert_eq!(runs, [1..2, 3..4]);
        assert_eq!(output, current);

        runs.clear();
        let moved = [10, 30, 40, 50];
        let count = encode_cached_records(
            &moved, &previous_keys, &previous_bytes, 1, &mut output,
            |range, output| {
                runs.push(range.clone());
                output.extend_from_slice(&moved[range]);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 3);
        assert_eq!(runs, [1..4]);
        assert_eq!(output, moved);
    }

    #[test]
    fn generated_vertex_addresses_and_culling_counts_invalidate_slots() {
        let mut scene = crate::Scene::new();
        let node = scene.insert(None, crate::Node::new()).unwrap();
        let original = super::InstanceSlotKey {
            node,
            world: crate::Mat4::IDENTITY,
            pixel_offset: crate::Vec2::ZERO,
            depth_bias: 0.0,
            vertex_word: 64,
            index_count_override: None,
        };
        let relocated = super::InstanceSlotKey {
            vertex_word: 128,
            ..original
        };
        let culled = super::InstanceSlotKey {
            index_count_override: Some(24),
            ..original
        };
        let keys = [relocated, culled];
        let previous = [original, original];
        let stride = crate::scene_view::INSTANCE_STRIDE as usize;
        let previous_bytes = vec![5u8; stride * 2];
        let mut encoded = Vec::new();
        let mut invocations = 0;
        let count = encode_cached_records(
            &keys, &previous, &previous_bytes, stride, &mut encoded,
            |range, bytes| {
                invocations += 1;
                bytes.resize(bytes.len() + range.len() * stride, 9);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 2);
        assert_eq!(invocations, 1);
        assert_eq!(encoded, vec![9; stride * 2]);
    }

    #[test]
    fn new_and_empty_tables_preserve_record_stride() {
        let mut output = Vec::new();
        let keys = [1, 2, 3];
        let count = encode_cached_records(
            &keys, &[], &[], 2, &mut output,
            |range, output| {
                for value in &keys[range] {
                    output.extend_from_slice(&[*value, *value]);
                }
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 3);
        assert_eq!(output, [1, 1, 2, 2, 3, 3]);
        let mut empty = Vec::new();
        let count = encode_cached_records(
            &[] as &[u8], &keys, &output, 2, &mut empty,
            |_, _| Ok::<_, ()>(()),
        ).unwrap();
        assert_eq!(count, 0);
        assert_eq!(empty, [0, 0]);
    }
}
