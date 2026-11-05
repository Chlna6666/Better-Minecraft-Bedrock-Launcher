//! Reuse already-encoded CPU instance rows until their GPU-visible fields change.
//!
//! Draw ordering is owned by PreparedScene, so cache keys are positional:
//! adding/removing a section re-encodes shifted slots, while unchanged
//! positions retain their previous bytes. GPU buffer layouts are unchanged.

use super::*;
use crate::scene_view::uploads::encode_cached_records;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
};

/// CPU and GPU mesh fields contributing to one 144-byte instance record.
/// Mesh relocation and procedural geometry are reflected in vertex_word.
#[derive(Clone, Copy)]
pub(super) struct InstanceSlotKey {
    node: NodeId,
    world: Mat4,
    pixel_offset: crate::Vec2,
    depth_bias: f32,
    vertex_word: u32,
    index_count_override: Option<u32>,
}

// Default float PartialEq treats +0.0 and -0.0 as equal even though their
// GPU bytes differ. Compare packed bits so an identical cache key guarantees
// byte-identical instance records, including signed zero and NaN payloads.
impl PartialEq for InstanceSlotKey {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node
            && self.world.columns().into_iter().flatten().map(f32::to_bits)
                .eq(other.world.columns().into_iter().flatten().map(f32::to_bits))
            && self.pixel_offset.x.to_bits() == other.pixel_offset.x.to_bits()
            && self.pixel_offset.y.to_bits() == other.pixel_offset.y.to_bits()
            && self.depth_bias.to_bits() == other.depth_bias.to_bits()
            && self.vertex_word == other.vertex_word
            && self.index_count_override == other.index_count_override
    }
}
impl Eq for InstanceSlotKey {}

/// Copy matching encoded records; encode contiguous dirty rows as one run.
/// This preserves the canonical encoder's cross-part inverse-matrix sharing.

impl InstanceSlotKey {
    /// Hash all GPU-visible fields, including the relocated vertex address.
    /// Equality is checked after lookup, so collisions never affect output.
    fn fingerprint(&self) -> u64 {
        let mut hash = DefaultHasher::new();
        self.node.hash(&mut hash);
        for column in self.world.columns() {
            for component in column {
                component.to_bits().hash(&mut hash);
            }
        }
        self.pixel_offset.x.to_bits().hash(&mut hash);
        self.pixel_offset.y.to_bits().hash(&mut hash);
        self.depth_bias.to_bits().hash(&mut hash);
        self.vertex_word.hash(&mut hash);
        self.index_count_override.hash(&mut hash);
        hash.finish()
    }
}

impl SceneResources {
    pub(super) fn instance_key(
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
            InstanceSlotKey::fingerprint,
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
    fn signed_zero_changes_are_not_reused_as_equal_gpu_bytes() {
        let mut scene = crate::Scene::new();
        let node = scene.insert(None, crate::Node::new()).unwrap();
        let negative = super::InstanceSlotKey {
            node,
            world: crate::Mat4::IDENTITY,
            pixel_offset: crate::Vec2::new(-0.0, 0.0),
            depth_bias: 0.0,
            vertex_word: 0,
            index_count_override: None,
        };
        let positive = super::InstanceSlotKey {
            pixel_offset: crate::Vec2::ZERO,
            ..negative
        };
        assert!(negative != positive, "GPU byte identity must distinguish signed zero");
        assert_ne!(negative.fingerprint(), positive.fingerprint());
        let mut output = Vec::new();
        let mut encoded = 0;
        let count = encode_cached_records(
            &[positive], &[negative], &[0u8; 4], 4, &mut output,
            super::InstanceSlotKey::fingerprint,
            |range, bytes| {
                encoded += range.len();
                bytes.extend_from_slice(&[2u8; 4]);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 1);
        assert_eq!(encoded, 1);
    }

    #[test]
    fn stable_slots_skip_encoding_and_shifted_slots_reencode() {
        let previous_keys = [10, 20, 30, 40, 50];
        let previous_bytes = [10, 20, 30, 40, 50];
        let mut output = Vec::new();
        let mut runs = Vec::new();
        let current = [10, 21, 30, 41, 50];
        let count = encode_cached_records(
            &current, &previous_keys, &previous_bytes, 1, &mut output,
            |value| *value as u64,
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
            |value| *value as u64,
            |range, output| {
                runs.push(range.clone());
                output.extend_from_slice(&moved[range]);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 0, "shifted unchanged rows should be reused");
        assert!(runs.is_empty());
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
            |key| key.fingerprint(),
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
    fn moved_rows_reuse_exact_previous_bytes_even_with_hash_collisions() {
        let previous = [11u8, 22, 33, 44, 55];
        // Previously encoded records need not look like their keys.
        let old_bytes = [101u8, 102, 103, 104, 105];
        let current = [11u8, 33, 44, 55];
        let mut output = Vec::new();
        let mut encoded_runs = Vec::new();
        let count = encode_cached_records(
            &current, &previous, &old_bytes, 1, &mut output,
            |_| 0, // Force every distinct key into the same hash bucket.
            |range, output| {
                encoded_runs.push(range.clone());
                output.extend_from_slice(&current[range]);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 0);
        assert!(encoded_runs.is_empty());
        assert_eq!(output, [101, 103, 104, 105]);

        // Existing records never match unrelated keys just because hashes do.
        let updated = [11u8, 99, 44, 55];
        let count = encode_cached_records(
            &updated, &previous, &old_bytes, 1, &mut output,
            |_| 0,
            |range, output| {
                output.extend_from_slice(&updated[range]);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 1);
        assert_eq!(output, [101, 99, 104, 105]);
    }

    #[test]
    fn large_section_shift_reuses_old_rows_without_per_key_allocations() {
        let previous = (0..100u16).collect::<Vec<_>>();
        let old_bytes = previous.iter().map(|v| (*v + 30) as u8).collect::<Vec<_>>();
        let current = previous[20..].to_vec();
        let mut output = Vec::new();
        let count = encode_cached_records(
            &current, &previous, &old_bytes, 1, &mut output,
            |_| 0, // All records collide; equality prevents false hits.
            |range, output| {
                output.extend(current[range].iter().map(|v| *v as u8));
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 0, "all shifted rows already have GPU-ready bytes");
        assert_eq!(output, old_bytes[20..]);
    }

    #[test]
    fn same_length_local_edit_does_not_scan_or_relocate() {
        let previous = [1u8, 2, 3, 4];
        let current = [1u8, 7, 3, 4];
        let mut output = Vec::new();
        let count = encode_cached_records(
            &current, &previous, &previous, 1, &mut output,
            |_| panic!("single-slot edits must not build a relocation index"),
            |range, output| {
                output.extend_from_slice(&current[range]);
                Ok::<_, ()>(())
            },
        ).unwrap();
        assert_eq!(count, 1);
        assert_eq!(output, current);
    }

    #[test]
    fn new_and_empty_tables_preserve_record_stride() {
        let mut output = Vec::new();
        let keys = [1, 2, 3];
        let count = encode_cached_records(
            &keys, &[], &[], 2, &mut output,
            |value| *value as u64,
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
            |value| *value as u64,
            |_, _| Ok::<_, ()>(()),
        ).unwrap();
        assert_eq!(count, 0);
        assert_eq!(empty, [0, 0]);
    }
}
