//! Stable physical GPU draw slots for streamed voxel sections.
//!
//! The prepared scene keeps a dense logical order for transparent sorting and
//! culling. DrawParams and Instance records live at stable physical addresses;
//! a compact u32 logical-to-physical table decouples GPU storage from ordering.
//! Freed slots are reused before new capacity is allocated.

use super::*;
use std::collections::HashMap;

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
pub(in crate::scene_view) struct StableDrawSlots {
    occupied: HashMap<SlotIdentity, usize>,
    identities: Vec<Option<SlotIdentity>>,
    free: Vec<usize>,
}

impl StableDrawSlots {
    pub(in crate::scene_view) fn high_water(&self) -> usize {
        self.identities.len()
    }

    /// Validate a retained physical slot before applying a sparse Section
    /// change. Material, mesh and part identity changes require a complete
    /// transactional re-plan (including duplicate occurrence assignment).
    pub(in crate::scene_view) fn matches_resident_draw(
        &self,
        slot: u32,
        draw: &PreparedDraw,
    ) -> bool {
        self.identities
            .get(slot as usize)
            .and_then(Option::as_ref)
            .is_some_and(|identity| identity.base == BaseIdentity::of(draw))
    }

    /// Construct a transactional next state. No slot may be recycled until
    /// all draws from the previous scene have been accounted for.
    pub(in crate::scene_view) fn plan(&self, draws: &[PreparedDraw]) -> gpui::Result<(Self, Vec<u32>)> {
        let mut occurrences = HashMap::<BaseIdentity, u32>::new();
        let mut wanted = Vec::with_capacity(draws.len());
        let mut live_slots = vec![false; self.identities.len()];
        for draw in draws {
            let base = BaseIdentity::of(draw);
            let count = occurrences.entry(base).or_default();
            let identity = SlotIdentity { base, occurrence: *count };
            *count = count.checked_add(1)
                .ok_or_else(|| anyhow!("GPUI 3D duplicate draw identity overflow"))?;
            let old_slot = self.occupied.get(&identity).copied();
            if let Some(slot) = old_slot {
                live_slots[slot] = true;
            }
            wanted.push((identity, old_slot));
        }

        // Clone only compact physical metadata. The previous HashMap clone
        // rehashed every resident draw before removing missing ones; construct
        // only the live successor entries instead.
        let mut next = Self {
            occupied: HashMap::with_capacity(wanted.len()),
            identities: self.identities.clone(),
            free: self.free.clone(),
        };
        // Recycle all retired identities before assigning new slots. Preserve
        // deterministic old LIFO free-list order and transactional semantics.
        for (index, identity) in next.identities.iter_mut().enumerate() {
            if identity.is_some() && !live_slots[index] {
                *identity = None;
                next.free.push(index);
            }
        }

        let mut mapping = Vec::with_capacity(wanted.len());
        for (id, old_slot) in wanted {
            let slot = old_slot.unwrap_or_else(|| {
                next.free.pop().unwrap_or_else(|| {
                    let slot = next.identities.len();
                    next.identities.push(None);
                    slot
                })
            });
            next.identities[slot] = Some(id);
            let replaced = next.occupied.insert(id, slot);
            debug_assert!(replaced.is_none(), "draw occurrences must be unique");
            mapping.push(u32::try_from(slot)
                .map_err(|_| anyhow!("GPUI 3D draw slot index exceeds u32"))?);
        }
        Ok((next, mapping))
    }}


/// Transactional structural GPU slot plan. Only changed Sections are hashed
/// into/out of the resident slot registry; untouched identities retain their
/// exact physical addresses, even if their dense logical indices move.
pub(in crate::scene_view) struct StructuralSlotPlan {
    pub mapping: Vec<u32>,
    pub high_water: usize,
    retired: Vec<(SlotIdentity, usize)>,
    added: Vec<(SlotIdentity, usize, usize)>,
    free: Vec<usize>,
}

impl StructuralSlotPlan {
    pub(in crate::scene_view) fn removed_slots(&self) -> impl Iterator<Item = usize> + '_ {
        self.retired.iter().map(|(_, slot)| *slot)
    }

    pub(in crate::scene_view) fn updated_slots(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.added.iter().map(|(_, slot, logical)| (*slot, *logical))
    }

    /// Called only after the GPU mapping and bounds writes have succeeded.
    pub(in crate::scene_view) fn commit(self, slots: &mut StableDrawSlots) {
        for (identity, slot) in self.retired {
            debug_assert_eq!(slots.occupied.remove(&identity), Some(slot));
            slots.identities[slot] = None;
        }
        slots.identities.resize(self.high_water, None);
        for (identity, slot, _) in self.added {
            debug_assert!(slots.identities[slot].is_none());
            slots.identities[slot] = Some(identity);
            debug_assert!(slots.occupied.insert(identity, slot).is_none());
        }
        slots.free = self.free;
    }
}

impl StableDrawSlots {
    /// Plan ordered old/new Section splices while copying only the compact
    /// logical u32 mapping for unaffected ranges. No O(all meshes) identity
    /// lookup, HashMap clone or physical Draw/Instance relocation is needed.
    ///
    /// Malformed or overlapping edits return None, allowing the established
    /// full transactional planner to handle mixed or reordered layouts.
    pub(in crate::scene_view) fn plan_structural(
        &self,
        draws: &[PreparedDraw],
        old_mapping: &[u32],
        delta: &crate::prepare::StructuralDrawDelta,
    ) -> gpui::Result<Option<StructuralSlotPlan>> {
        if delta.previous_count != old_mapping.len() || delta.edits.is_empty() {
            return Ok(None);
        }
        let mut next_mapping = Vec::with_capacity(draws.len());
        let mut retired = Vec::<(SlotIdentity, usize)>::new();
        let mut additions = Vec::<(SlotIdentity, usize)>::new();
        let mut occurrences = HashMap::<BaseIdentity, u32>::new();
        let mut old_cursor = 0usize;
        let mut new_cursor = 0usize;
        for edit in &delta.edits {
            if edit.old.start < old_cursor || edit.new.start < new_cursor
                || edit.old.end > old_mapping.len() || edit.new.end > draws.len()
                || edit.old.start > edit.old.end || edit.new.start > edit.new.end
                || edit.old.start - old_cursor != edit.new.start - new_cursor
            {
                return Ok(None);
            }
            next_mapping.extend_from_slice(&old_mapping[old_cursor..edit.old.start]);
            for &physical in &old_mapping[edit.old.clone()] {
                let slot = physical as usize;
                let Some(identity) = self.identities.get(slot).copied().flatten() else {
                    return Ok(None);
                };
                retired.push((identity, slot));
            }
            for logical in edit.new.clone() {
                let base = BaseIdentity::of(&draws[logical]);
                let occurrence = occurrences.entry(base).or_default();
                let identity = SlotIdentity { base, occurrence: *occurrence };
                *occurrence = occurrence.checked_add(1).ok_or_else(|| {
                    anyhow!("GPUI 3D changed Section duplicate draw occurrence overflow")
                })?;
                additions.push((identity, logical));
                next_mapping.push(0); // Filled only after all retired slots are known.
            }
            old_cursor = edit.old.end;
            new_cursor = edit.new.end;
        }
        if old_mapping.len() - old_cursor != draws.len() - new_cursor {
            return Ok(None);
        }
        next_mapping.extend_from_slice(&old_mapping[old_cursor..]);
        if next_mapping.len() != draws.len() {
            return Ok(None);
        }

        // Recycle only retired Section slots. An identity that survives its
        // own replacement keeps the same slot (including duplicate parts).
        let retired_lookup = retired.iter().copied().collect::<HashMap<_, _>>();
        if retired_lookup.len() != retired.len() {
            return Ok(None);
        }
        let mut seen = std::collections::HashSet::with_capacity(additions.len());
        let mut reused = std::collections::HashSet::with_capacity(additions.len());
        for &(identity, _) in &additions {
            if !seen.insert(identity) {
                return Ok(None);
            }
            if let Some(&slot) = retired_lookup.get(&identity) {
                reused.insert(slot);
            } else if self.occupied.contains_key(&identity) {
                // An allegedly inserted identity belongs to an untouched
                // Section; the splice description was incomplete.
                return Ok(None);
            }
        }
        let mut free = self.free.clone();
        for &(_, slot) in &retired {
            if !reused.contains(&slot) {
                free.push(slot);
            }
        }
        let mut high_water = self.identities.len();
        let mut added = Vec::with_capacity(additions.len());
        for (identity, logical) in additions {
            let slot = if let Some(&slot) = retired_lookup.get(&identity) {
                slot
            } else if let Some(slot) = free.pop() {
                slot
            } else {
                let slot = high_water;
                high_water = high_water.checked_add(1)
                    .ok_or_else(|| anyhow!("GPU physical Draw Slot overflow"))?;
                slot
            };
            next_mapping[logical] = u32::try_from(slot)
                .map_err(|_| anyhow!("GPU physical Draw Slot exceeds u32 index"))?;
            added.push((identity, slot, logical));
        }
        Ok(Some(StructuralSlotPlan {
            mapping: next_mapping, high_water, retired, added, free,
        }))
    }
}

impl SceneResources {
    /// Encode only the exact physical Instance rows referenced by the last
    /// fixed-layout Section Delta. Unchanged GPU data is never visited or
    /// copied, and no CPU mirror is changed before GPU upload succeeds.
    pub(super) fn encode_resident_instance_delta(
        &self,
        mapping: &[u32],
        dirty: &[usize],
        previous: &[instance_cache::InstanceSlotKey],
        previous_bytes: &[u8],
        scratch: &mut Vec<u8>,
    ) -> gpui::Result<Vec<(usize, instance_cache::InstanceSlotKey, [u8; INSTANCE_STRIDE as usize])>> {
        const STRIDE: usize = INSTANCE_STRIDE as usize;
        if mapping.len() != self.prepared.draws.len()
            || previous_bytes.len() != previous.len() * STRIDE
        {
            return Err(anyhow!("GPUI 3D dirty Instance requires committed resident layout").into());
        }
        let mut patches = Vec::with_capacity(dirty.len());
        let mut seen = std::collections::HashSet::with_capacity(dirty.len());
        for &logical in dirty {
            if !seen.insert(logical) { continue; }
            let draw = self.prepared.draws.get(logical)
                .ok_or_else(|| anyhow!("dirty Instance logical index outside scene"))?;
            let physical = *mapping.get(logical)
                .ok_or_else(|| anyhow!("missing resident Instance slot"))? as usize;
            let key = self.instance_key(draw, true)?;
            if previous.get(physical) == Some(&key)
                && previous_bytes.len() >= (physical + 1) * STRIDE
            {
                continue;
            }
            // A structural insert can allocate beyond the previous
            // high-water mark; the new record must be encoded even if no
            // previous key/byte range exists for that physical slot.
            scratch.clear();
            self.encode_draw_instances(std::slice::from_ref(draw), true, scratch)?;
            let bytes: [u8; STRIDE] = scratch.get(..STRIDE)
                .ok_or_else(|| anyhow!("incomplete dirty Instance record"))?
                .try_into()
                .expect("a validated Instance record has the full stride");
            patches.push((physical, key, bytes));
        }
        Ok(patches)
    }

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
        // Resolve exact physical record identities first. When none changed,
        // the last committed dense CPU snapshot can be moved after successful
        // GPU/BindGroup work instead of copied at all.
        let mut dirty = Vec::new();
        for (logical, &physical) in mapping.iter().enumerate() {
            let physical = physical as usize;
            let draw = &self.prepared.draws[logical];
            let key = self.instance_key(draw, true)?;
            let start = physical * STRIDE;
            let end = start + STRIDE;
            if previous.get(physical) != Some(&key)
                || previous_bytes.get(start..end).is_none()
            {
                dirty.push((logical, physical));
            }
            if physical >= next.len() {
                next.resize(physical + 1, key);
            }
            next[physical] = key;
        }
        output.clear();
        if dirty.is_empty()
            && next == previous
            && previous_bytes.len() == high_water.max(1) * STRIDE
        {
            return Ok(0);
        }
        output.extend_from_slice(previous_bytes);
        output.resize(high_water.max(1) * STRIDE, 0);
        for &(logical, physical) in &dirty {
            let start = physical * STRIDE;
            let draw = &self.prepared.draws[logical];
            self.encode_draw_instances(std::slice::from_ref(draw), true, scratch)?;
            output[start..start + STRIDE].copy_from_slice(scratch);
        }
        Ok(dirty.len())
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
    fn structural_splices_keep_survivor_slots_and_reuse_retired_storage() {
        let original = draws(96);
        let (mut resident, map) = StableDrawSlots::default().plan(&original).unwrap();
        // Delete a middle Section. All remaining physical addresses should
        // survive even though their compact logical indices have shifted.
        let mut removed = original.clone();
        removed.remove(19);
        let plan = resident.plan_structural(
            &removed, &map,
            &crate::prepare::StructuralDrawDelta {
                previous_count: original.len(),
                edits: vec![crate::prepare::StructuralDrawEdit {
                    old: 19..20, new: 19..19,
                }],
            },
        ).unwrap().unwrap();
        let mapping = plan.mapping.clone();
        assert_eq!(mapping[..19], map[..19]);
        assert_eq!(mapping[19..], map[20..]);
        assert_eq!(plan.high_water, resident.high_water());
        plan.commit(&mut resident);
        assert_eq!(resident.free.len(), 1);

        // New root inserted into the middle of the dense logical ordering:
        // the retired physical row is reused, without relocating survivors.
        let extra = draws(97).pop().unwrap();
        assert!(!original.iter().any(|draw| draw.node == extra.node));
        let mut inserted = removed.clone();
        inserted.insert(31, extra);
        let plan = resident.plan_structural(
            &inserted, &mapping,
            &crate::prepare::StructuralDrawDelta {
                previous_count: removed.len(),
                edits: vec![crate::prepare::StructuralDrawEdit {
                    old: 31..31, new: 31..32,
                }],
            },
        ).unwrap().unwrap();
        let next_mapping = plan.mapping.clone();
        assert_eq!(&next_mapping[..31], &mapping[..31]);
        assert_eq!(&next_mapping[32..], &mapping[31..]);
        assert_eq!(next_mapping[31], map[19]);
        plan.commit(&mut resident);
        assert_eq!(resident.high_water(), 96);
        assert!(resident.free.is_empty());
        for (draw, slot) in inserted.iter().zip(next_mapping) {
            assert!(resident.matches_resident_draw(slot, draw));
        }
    }

    #[test]
    fn structural_splices_can_move_one_identity_without_reassigning_its_slot() {
        let original = draws(40);
        let (mut resident, map) = StableDrawSlots::default().plan(&original).unwrap();
        let mut shifted = original.clone();
        let moved = shifted.remove(12);
        shifted.insert(0, moved);
        let delta = crate::prepare::StructuralDrawDelta {
            previous_count: original.len(),
            edits: vec![
                crate::prepare::StructuralDrawEdit {
                    old: 0..0, new: 0..1,
                },
                crate::prepare::StructuralDrawEdit {
                    old: 12..13, new: 13..13,
                },
            ],
        };
        let plan = resident.plan_structural(&shifted, &map, &delta)
            .unwrap().unwrap();
        assert_eq!(plan.mapping[0], map[12]);
        assert_eq!(&plan.mapping[1..13], &map[..12]);
        assert_eq!(&plan.mapping[13..], &map[13..]);
        let next_map = plan.mapping.clone();
        plan.commit(&mut resident);
        assert!(resident.free.is_empty());
        for (draw, slot) in shifted.iter().zip(next_map) {
            assert!(resident.matches_resident_draw(slot, draw));
        }
    }

    #[test]
    fn structural_splices_reject_invalid_ranges_without_committing_slots() {
        let original = draws(32);
        let (resident, map) = StableDrawSlots::default().plan(&original).unwrap();
        let invalid = crate::prepare::StructuralDrawDelta {
            previous_count: original.len(),
            edits: vec![crate::prepare::StructuralDrawEdit {
                old: 15..16, new: 14..15,
            }],
        };
        assert!(resident.plan_structural(&original, &map, &invalid)
            .unwrap().is_none());
        assert_eq!(resident.occupied.len(), original.len());
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
    #[test]
    fn physical_draw_encoder_avoids_intermediate_table_and_keeps_dirty_rows() {
        let original = draws(40);
        let (slots, first_map) = StableDrawSlots::default().plan(&original).unwrap();
        let mut cache = DrawRecordCache::default();
        let mut keys = Vec::new();
        let mut bytes = Vec::new();
        let mut scratch = Vec::new();
        let encoded = cache.encode_stable(
            &original, 0, &first_map, slots.high_water(),
            &[], &[], &mut keys, &mut bytes, &mut scratch
        ).unwrap();
        assert_eq!(encoded, 40);
        let mut baseline = Vec::new();
        let mut baseline_keys = Vec::new();
        DrawRecordCache::default()
            .encode(&original, 0, &mut baseline, &mut baseline_keys)
            .unwrap();
        for (logical, &physical) in first_map.iter().enumerate() {
            let stride = DRAW_PARAMS_STRIDE as usize;
            assert_eq!(
                &bytes[physical as usize * stride..(physical as usize + 1) * stride],
                &baseline[logical * stride..(logical + 1) * stride]
            );
        }

        let mut reordered = original.clone();
        reordered.reverse();
        reordered.remove(11);
        let (next_slots, remap) = slots.plan(&reordered).unwrap();
        let mut next_keys = Vec::new();
        let mut next_bytes = Vec::new();
        let unchanged = cache.encode_stable(
            &reordered, 0, &remap, next_slots.high_water(),
            &keys, &bytes, &mut next_keys, &mut next_bytes, &mut scratch
        ).unwrap();
        assert_eq!(unchanged, 0);
        assert!(next_bytes.is_empty(), "logical reorder must not copy the physical table");
        assert_eq!(next_keys, keys);

        // One immutable Arc snapshot changes while every other physical
        // DrawParams record and material identity remains resident.
        Arc::make_mut(&mut reordered[3].material).base_color = [0.3, 0.2, 0.7, 1.0];
        let (last_slots, last_map) = next_slots.plan(&reordered).unwrap();
        let dirty = cache.encode_stable(
            &reordered, 0, &last_map, last_slots.high_water(),
            &keys, &bytes, &mut next_keys, &mut next_bytes, &mut scratch
        ).unwrap();
        assert_eq!(dirty, 1, "only the modified material's slot should be rebuilt");
        for &slot in &last_map {
            let stride = DRAW_PARAMS_STRIDE as usize;
            let start = slot as usize * stride;
            if slot != last_map[3] {
                assert_eq!(
                    &next_bytes[start..start + stride],
                    &bytes[start..start + stride],
                    "unchanged physical records are bit-for-bit reused"
                );
            } else {
                assert_ne!(&next_bytes[start..start + stride], &bytes[start..start + stride]);
            }
        }
    }

    #[test]
    fn drawparam_delta_encodes_only_changed_physical_material_record() {
        let original = draws(40);
        let (slots, mapping) = StableDrawSlots::default().plan(&original).unwrap();
        let mut cache = DrawRecordCache::default();
        let mut keys = Vec::new();
        let mut baseline = Vec::new();
        let mut scratch = Vec::new();
        cache.encode_stable(
            &original, 0, &mapping, slots.high_water(),
            &[], &[], &mut keys, &mut baseline, &mut scratch,
        ).unwrap();
        assert!(cache.encode_resident_delta(
            &original, 0, &mapping, &[], &keys, &baseline, &mut scratch,
        ).unwrap().is_empty());
        let mut changed = original.clone();
        Arc::make_mut(&mut changed[8].material).base_color = [0.2, 0.4, 0.6, 1.0];
        let patches = cache.encode_resident_delta(
            &changed, 0, &mapping, &[8], &keys, &baseline, &mut scratch,
        ).unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].0, mapping[8] as usize);
        assert_ne!(patches[0].1, keys[mapping[8] as usize]);
        let canonical = crate::scene_view::gpu::encode_draw_records(
            std::slice::from_ref(&changed[8]), 0,
        ).unwrap();
        assert_eq!(patches[0].2.as_slice(), canonical.as_slice());
        let mut patched = baseline.clone();
        let start = patches[0].0 * DRAW_PARAMS_STRIDE as usize;
        patched[start..start + DRAW_PARAMS_STRIDE as usize]
            .copy_from_slice(&patches[0].2);
        let mut next_keys = Vec::new();
        let mut full = Vec::new();
        DrawRecordCache::default().encode_stable(
            &changed, 0, &mapping, slots.high_water(),
            &keys, &baseline, &mut next_keys, &mut full, &mut scratch,
        ).unwrap();
        assert_eq!(patched, full, "sparse GPU patch must match canonical full encode");
    }

    #[test]
    fn incremental_plan_keeps_exact_one_to_one_resident_slot_state() {
        let original = draws(128);
        let (mut slots, _) = StableDrawSlots::default().plan(&original).unwrap();
        for round in 0..7 {
            let mut changed = original.clone();
            let rotate = (round * 13) % changed.len();
            changed.rotate_left(rotate);
            changed.drain(5..(9 + round));
            changed.push(changed[0].clone());
            let (next, mapping) = slots.plan(&changed).unwrap();
            assert_eq!(next.occupied.len(), changed.len());
            assert_eq!(mapping.len(), changed.len());
            for (&slot, draw) in mapping.iter().zip(&changed) {
                let expected = BaseIdentity::of(draw);
                let resident = next.identities[slot as usize]
                    .expect("active physical slot must have identity");
                assert_eq!(resident.base, expected);
                assert_eq!(next.occupied.get(&resident), Some(&(slot as usize)));
            }
            for &hole in &next.free {
                assert!(next.identities[hole].is_none());
            }
            slots = next;
        }
    }

}
