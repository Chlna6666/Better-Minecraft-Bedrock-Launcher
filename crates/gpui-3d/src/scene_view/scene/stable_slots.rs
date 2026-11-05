//! Stable physical GPU draw slots for streamed voxel sections.
//!
//! The prepared scene keeps a dense logical order for transparent sorting and
//! culling. DrawParams and Instance records live at stable physical addresses;
//! a compact u32 logical-to-physical table decouples GPU storage from ordering.
//! Freed slots are reused before new capacity is allocated.

use super::*;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct BaseIdentity {
    node: NodeId,
    mesh: MeshId,
    material: MaterialId,
    first_index: u32,
    index_count: u32,
    material_slot: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct SlotIdentity {
    base: BaseIdentity,
    occurrence: u32,
}

impl BaseIdentity {
    fn of(draw: &PreparedDraw) -> Self {
        Self {
            node: draw.node,
            mesh: draw.mesh.id(),
            material: draw.material.id(),
            first_index: draw.part.first_index(),
            index_count: draw.part.index_count(),
            material_slot: draw.part.material_slot(),
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct StableDrawSlots {
    occupied: HashMap<SlotIdentity, usize>,
    identities: Vec<Option<SlotIdentity>>,
    free: Vec<usize>,
}

impl StableDrawSlots {
    pub(super) fn high_water(&self) -> usize {
        self.identities.len()
    }

    /// Construct a transactional next state. No slot may be recycled until
    /// all draws from the previous scene have been accounted for.
    pub(super) fn plan(&self, draws: &[PreparedDraw]) -> gpui::Result<(Self, Vec<u32>)> {
        let mut next = self.clone();
        let mut occurrences = HashMap::<BaseIdentity, u32>::new();
        let mut wanted = Vec::with_capacity(draws.len());
        let mut active = HashSet::with_capacity(draws.len());
        for draw in draws {
            let base = BaseIdentity::of(draw);
            let count = occurrences.entry(base).or_default();
            let identity = SlotIdentity { base, occurrence: *count };
            *count = count.checked_add(1)
                .ok_or_else(|| anyhow!("GPUI 3D duplicate draw identity overflow"))?;
            active.insert(identity);
            wanted.push(identity);
        }

        // Retire stale slots before assigning new identities; replacements
        // reuse holes without raising the high-water mark unnecessarily.
        for (index, resident) in next.identities.iter_mut().enumerate() {
            if let Some(id) = *resident
                && !active.contains(&id)
            {
                next.occupied.remove(&id);
                *resident = None;
                next.free.push(index);
            }
        }

        let mut mapping = Vec::with_capacity(wanted.len());
        for id in wanted {
            let slot = if let Some(&slot) = next.occupied.get(&id) {
                slot
            } else {
                let slot = next.free.pop().unwrap_or_else(|| {
                    let slot = next.identities.len();
                    next.identities.push(None);
                    slot
                });
                next.identities[slot] = Some(id);
                next.occupied.insert(id, slot);
                slot
            };
            mapping.push(u32::try_from(slot)
                .map_err(|_| anyhow!("GPUI 3D draw slot index exceeds u32"))?);
        }
        Ok((next, mapping))
    }
}


impl SceneResources {
    /// Encode only GPU-visible instance rows whose stable physical identity
    /// changed; removing or inserting a section does not touch resident rows.
    pub(super) fn encode_stable_instances(
        &self,
        mapping: &[u32],
        high_water: usize,
        previous: &[instance_cache::InstanceSlotKey],
        previous_bytes: &[u8],
        next: &mut Vec<instance_cache::InstanceSlotKey>,
        output: &mut Vec<u8>,
        scratch: &mut Vec<u8>,
    ) -> gpui::Result<usize> {
        const STRIDE: usize = INSTANCE_STRIDE as usize;
        next.clear();
        next.extend_from_slice(previous);
        output.clear();
        output.extend_from_slice(previous_bytes);
        output.resize(high_water.max(1) * STRIDE, 0);
        let mut changed = 0;
        for (logical, &physical) in mapping.iter().enumerate() {
            let physical = physical as usize;
            let draw = &self.prepared.draws[logical];
            let key = self.instance_key(draw, true)?;
            let start = physical * STRIDE;
            let end = start + STRIDE;
            let cached = previous.get(physical) == Some(&key)
                && previous_bytes.get(start..end).is_some();
            if !cached {
                self.encode_draw_instances(std::slice::from_ref(draw), true, scratch)?;
                output[start..end].copy_from_slice(scratch);
                changed += 1;
            }
            if physical >= next.len() {
                next.resize(physical + 1, key);
            }
            next[physical] = key;
        }
        Ok(changed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Camera, Material, Mesh, Node, PreparedScene, Scene, Vec3};
    use std::sync::Arc;

    fn draws(count: usize) -> Vec<PreparedDraw> {
        let mut scene = Scene::new();
        let mesh = Arc::new(Mesh::cube());
        let material = Arc::new(Material::new());
        for _ in 0..count {
            scene.insert(None, Node::new().with_mesh(mesh.clone())
                .with_materials([material.clone()])).unwrap();
        }
        let camera = Camera::perspective(
            Vec3::new(0., 0., 20.), Vec3::ZERO, Vec3::Y, 1., 0.1, 200.,
        );
        PreparedScene::new(&scene, camera, 1.).unwrap().draws
    }

    #[test]
    fn stable_slots_survive_reordering_insertion_and_removal() {
        let original = draws(80);
        let (first, assigned) = StableDrawSlots::default().plan(&original).unwrap();
        assert_eq!(first.high_water(), 80);
        let mut changed = original.clone();
        changed.remove(15);
        changed.reverse();
        let (second, remapped) = first.plan(&changed).unwrap();
        assert_eq!(second.high_water(), 80);
        for (draw, &slot) in changed.iter().zip(&remapped) {
            let old_index = original.iter().position(|old| old.node == draw.node).unwrap();
            assert_eq!(slot, assigned[old_index], "existing draw must retain its physical slot");
        }
        assert_eq!(second.free.len(), 1);
        let mut extended = changed.clone();
        extended.push(original[15].clone());
        let (third, restored) = second.plan(&extended).unwrap();
        assert_eq!(third.high_water(), 80, "free slot must be reused");
        assert_eq!(restored.last(), Some(&assigned[15]));
    }

    #[test]
    fn indirect_mapping_recovers_exact_draw_and_instance_bytes_after_reorder() {
        let original = draws(48);
        let (first, map) = StableDrawSlots::default().plan(&original).unwrap();
        // Representative immutable GPU record bytes for two different strides.
        // No physical row is touched when an existing Section merely moves.
        for stride in [DRAW_PARAMS_STRIDE as usize, INSTANCE_STRIDE as usize] {
            let mut physical = vec![0u8; first.high_water() * stride];
            for (logical, &slot) in map.iter().enumerate() {
                physical[slot as usize * stride..(slot as usize + 1) * stride]
                    .fill(logical as u8 + 1);
            }
            let mut moved = original.clone();
            moved.remove(5);
            moved.rotate_left(11);
            let (second, new_map) = first.plan(&moved).unwrap();
            assert_eq!(second.high_water(), first.high_water());
            assert_eq!(second.free.len(), 1);
            for (logical, &slot) in new_map.iter().enumerate() {
                let original_index = original.iter().position(|draw| {
                    draw.node == moved[logical].node
                }).unwrap();
                assert_eq!(
                    &physical[slot as usize * stride..(slot as usize + 1) * stride],
                    &physical[map[original_index] as usize * stride
                        ..(map[original_index] as usize + 1) * stride],
                    "indirection must preserve every byte of a moved Section"
                );
            }
        }
    }

    #[test]
    fn duplicate_draw_identities_receive_distinct_physical_slots() {
        let mut draws = draws(1);
        draws.push(draws[0].clone());
        let (first, mapping) = StableDrawSlots::default().plan(&draws).unwrap();
        assert_eq!(mapping.len(), 2);
        assert_ne!(mapping[0], mapping[1]);
        let (_, second) = first.plan(&draws).unwrap();
        assert_eq!(mapping, second);
    }

    #[test]
    fn slot_replacement_does_not_grow_capacity() {
        let first_draws = draws(8);
        let (first, _) = StableDrawSlots::default().plan(&first_draws).unwrap();
        let other_draws = draws(8);
        let (next, mapping) = first.plan(&other_draws).unwrap();
        assert_eq!(next.high_water(), 8);
        assert_eq!(mapping.len(), 8);
    }
}
