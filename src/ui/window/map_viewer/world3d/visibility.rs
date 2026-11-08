//! Conservative 16^3 air-connectivity graph for streamed 64-block render bands.
//!
//! Only full opaque cubes are treated as blocking. Glass, water, detail meshes,
//! unknown states and omitted SubChunks remain passable: false visibility costs
//! performance, but false occlusion would make geometry disappear.
//! These graphs are computed once per decoded column, never per rendered frame.

use super::types::{World3dChunkMesh, World3dChunkProbe};
use bedrock_render::{ChunkPos, Dimension};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

const SIDE_COUNT: usize = 6;
const ALL_SIDES: u8 = (1 << SIDE_COUNT) - 1;
const CELL_COUNT: usize = 16 * 16 * 16;
const SOLID_WORDS: usize = CELL_COUNT / 64;

/// All six boundary planes form one connected shell when they contain no
/// certified opaque cells. Detect this in 64 bitwise operations instead of
/// flood-filling up to 4,096 air cells during every decoded Section update.
const fn border_mask() -> [u64; SOLID_WORDS] {
    let mut mask = [0u64; SOLID_WORDS];
    let mut cell = 0usize;
    while cell < CELL_COUNT {
        let x = cell & 15;
        let z = (cell >> 4) & 15;
        let y = cell >> 8;
        if x == 0 || x == 15 || z == 0 || z == 15 || y == 0 || y == 15 {
            mask[cell / 64] |= 1u64 << (cell % 64);
        }
        cell += 1;
    }
    mask
}
const BORDER_CELLS: [u64; SOLID_WORDS] = border_mask();
// Keep a camera-voxel transition bounded even for the supported 256-chunk
// radius. Large scenes keep the existing GPU frustum/Hi-Z path unchanged.
const MAX_COLUMNS: usize = 1_024;
const MAX_HEIGHT_SECTIONS: usize = 48;
const MAX_VISITED_SECTIONS: usize = 32_768;
// Canonical face order: -X, +X, -Y, +Y, -Z, +Z.
const NEIGHBORS: [(i32, i32, i32); SIDE_COUNT] = [
    (-1, 0, 0), (1, 0, 0), (0, -1, 0),
    (0, 1, 0), (0, 0, -1), (0, 0, 1),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SectionVisibility {
    solid: [u64; SOLID_WORDS],
    /// Entry face -> all exit faces connected by a passable cell component.
    portals: [u8; SIDE_COUNT],
    all_air: bool,
    /// Most terrain sections have one connected air volume. A single exit
    /// mask then answers camera movement queries in O(1), with no voxel BFS.
    uniform_camera_exits: Option<u8>,
    /// Allocate 4 KiB only when at least two distinct air components exist.
    /// Solid cells are checked via `solid` before indexing this table.
    cell_camera_exits: Option<Box<[u8; CELL_COUNT]>>,
    /// Exact air-component identity, allocated only for multi-component
    /// Sections. Equal face masks do NOT imply connected air pockets.
    /// IDs use first-voxel-index + 1 (1..=4096); zero is reserved.
    cell_region_ids: Option<Box<[u16; CELL_COUNT]>>,
}

impl SectionVisibility {
    pub(super) const OPEN: Self = Self {
        solid: [0; SOLID_WORDS],
        portals: [ALL_SIDES; SIDE_COUNT],
        all_air: true,
        uniform_camera_exits: Some(ALL_SIDES),
        cell_camera_exits: None,
        cell_region_ids: None,
    };

    pub(super) fn from_solid_bits(solid: [u64; SOLID_WORDS]) -> Self {
        if solid.iter().all(|&word| word == 0) {
            return Self::OPEN;
        }
        if solid.iter().all(|&word| word == u64::MAX) {
            return Self {
                solid,
                portals: [0; SIDE_COUNT],
                all_air: false,
                uniform_camera_exits: None,
                cell_camera_exits: None,
                cell_region_ids: None,
            };
        }
        if solid
            .iter()
            .zip(BORDER_CELLS)
            .all(|(&word, border)| word & border == 0)
        {
            // Every Section face is connected through its empty boundary
            // shell. Internal disconnected air pockets remain correct because
            // camera_exits still flood-fills from the camera voxel.
            return Self {
                solid,
                portals: [ALL_SIDES; SIDE_COUNT],
                all_air: false,
                // A sealed pocket can still exist inside an open boundary
                // shell: keep the rare camera query on its conservative BFS.
                uniform_camera_exits: None,
                cell_camera_exits: None,
                cell_region_ids: None,
            };
        }
        let mut graph = Self {
            solid,
            portals: [0; SIDE_COUNT],
            all_air: false,
            uniform_camera_exits: None,
            cell_camera_exits: None,
            cell_region_ids: None,
        };
        let mut visited = [0u64; SOLID_WORDS];
        let mut queue = Vec::<usize>::new();
        let mut first_component_faces = None;
        let mut first_component_cells = Vec::<usize>::new();
        for first in 0..CELL_COUNT {
            if graph.is_solid(first) || test_bit(&visited, first) {
                continue;
            }
            queue.clear();
            queue.push(first);
            set_bit(&mut visited, first);
            let mut head = 0;
            let mut faces = 0;
            while head < queue.len() {
                let index = queue[head];
                head += 1;
                faces |= boundary_faces(index);
                for neighbour in cell_neighbours(index).into_iter().flatten() {
                    if !graph.is_solid(neighbour) && !test_bit(&visited, neighbour) {
                        set_bit(&mut visited, neighbour);
                        queue.push(neighbour);
                    }
                }
            }
            for entry in 0..SIDE_COUNT {
                if faces & (1 << entry) != 0 {
                    graph.portals[entry] |= faces;
                }
            }

            if let (Some(exits), Some(regions)) = (
                graph.cell_camera_exits.as_mut(),
                graph.cell_region_ids.as_mut(),
            ) {
                let id = u16::try_from(first + 1).expect("Section voxel ID fits u16");
                for &index in &queue {
                    exits[index] = faces;
                    regions[index] = id;
                }
            } else if let Some(first_faces) = first_component_faces {
                let mut exits = Box::new([0u8; CELL_COUNT]);
                let mut regions = Box::new([0u16; CELL_COUNT]);
                let first_id = u16::try_from(first_component_cells[0] + 1)
                    .expect("Section voxel ID fits u16");
                let current_id = u16::try_from(first + 1)
                    .expect("Section voxel ID fits u16");
                for &index in &first_component_cells {
                    exits[index] = first_faces;
                    regions[index] = first_id;
                }
                for &index in &queue {
                    exits[index] = faces;
                    regions[index] = current_id;
                }
                graph.cell_camera_exits = Some(exits);
                graph.cell_region_ids = Some(regions);
                first_component_cells.clear();
            } else {
                first_component_faces = Some(faces);
                first_component_cells = std::mem::take(&mut queue);
            }
        }
        if graph.cell_camera_exits.is_none() {
            graph.uniform_camera_exits = first_component_faces;
        }
        graph
    }

    #[inline]
    pub(super) fn heap_bytes(&self) -> usize {
        if self.cell_camera_exits.is_some() {
            CELL_COUNT * (std::mem::size_of::<u8>() + std::mem::size_of::<u16>())
        } else {
            0
        }
    }

    #[inline]
    fn is_solid(&self, index: usize) -> bool {
        test_bit(&self.solid, index)
    }

    #[inline]
    pub(super) fn exits(&self, entry_face: usize) -> u8 {
        self.portals[entry_face]
    }

    /// Camera may be in a sealed pocket, unlike a portal-facing neighbor.
    /// A solid camera voxel disables graph culling (the editor supports noclip).
    pub(super) fn camera_exits(&self, local: [usize; 3]) -> Option<u8> {
        self.camera_region(local).map(|(_, exits)| exits)
    }

    /// Returns the exact air-component ID AND the six-face portal mask.
    /// Different sealed cavities may both have exits=0; their IDs must
    /// differ to prevent async graph results being reused across caves.
    pub(super) fn camera_region(&self, local: [usize; 3]) -> Option<(u16, u8)> {
        let start = (local[1] * 16 + local[2]) * 16 + local[0];
        if self.is_solid(start) {
            return None;
        }
        if let Some(exits) = self.uniform_camera_exits {
            return Some((0, exits));
        }
        if let (Some(exits), Some(regions)) =
            (self.cell_camera_exits.as_ref(), self.cell_region_ids.as_ref())
        {
            return Some((regions[start], exits[start]));
        }
        // The common open-shell shortcut avoids building a 4 KiB component
        // table, but a sealed internal cave is still possible. Try six
        // straight voxel rays (at most 96 bit checks) first: reaching ANY
        // clear boundary cell proves connectivity to the entire open shell.
        // Only a completely enclosed camera position falls back to BFS.
        if self.portals == [ALL_SIDES; SIDE_COUNT] {
            if boundary_faces(start) != 0 {
                return Some((0, ALL_SIDES));
            }
            for axis in 0..SIDE_COUNT {
                let mut cursor = start;
                while let Some(next) = cell_neighbours(cursor)[axis] {
                    if self.is_solid(next) {
                        break;
                    }
                    if boundary_faces(next) != 0 {
                        return Some((0, ALL_SIDES));
                    }
                    cursor = next;
                }
            }
        }
        // Camera is potentially inside a sealed internal cavity; in this
        // rare case run an exact local BFS instead of incorrectly opening all
        // six portals and producing a false visibility result.
        let mut visited = [0u64; SOLID_WORDS];
        let mut queue = Vec::with_capacity(256);
        queue.push(start);
        set_bit(&mut visited, start);
        let mut head = 0;
        let mut faces = 0;
        let mut minimum = start;
        while head < queue.len() {
            let index = queue[head];
            minimum = minimum.min(index);
            head += 1;
            faces |= boundary_faces(index);
            if faces == ALL_SIDES {
                return Some((0, ALL_SIDES));
            }
            for neighbour in cell_neighbours(index).into_iter().flatten() {
                if !self.is_solid(neighbour) && !test_bit(&visited, neighbour) {
                    set_bit(&mut visited, neighbour);
                    queue.push(neighbour);
                }
            }
        }
        // All six boundaries belong to the same open shell when the
        // border was certified empty. Interior sealed pockets instead get a
        // stable component ID derived from their canonical minimum voxel.
        if self.portals == [ALL_SIDES; SIDE_COUNT] {
            Some(if faces != 0 {
                (0, ALL_SIDES)
            } else {
                (u16::try_from(minimum + 1).expect("Section voxel index fits u16"), 0)
            })
        } else {
            Some((0, faces))
        }
    }
}

#[inline]
fn test_bit(bits: &[u64; SOLID_WORDS], index: usize) -> bool {
    bits[index / 64] & (1u64 << (index % 64)) != 0
}

#[inline]
fn set_bit(bits: &mut [u64; SOLID_WORDS], index: usize) {
    bits[index / 64] |= 1u64 << (index % 64);
}

#[inline]
fn boundary_faces(index: usize) -> u8 {
    let x = index & 15;
    let z = (index >> 4) & 15;
    let y = index >> 8;
    u8::from(x == 0)
        | (u8::from(x == 15) << 1)
        | (u8::from(y == 0) << 2)
        | (u8::from(y == 15) << 3)
        | (u8::from(z == 0) << 4)
        | (u8::from(z == 15) << 5)
}

#[inline]
fn cell_neighbours(index: usize) -> [Option<usize>; SIDE_COUNT] {
    let x = index & 15;
    let z = (index >> 4) & 15;
    let y = index >> 8;
    [
        (x > 0).then_some(index.saturating_sub(1)),
        (x < 15).then_some(index + 1),
        (y > 0).then_some(index.saturating_sub(256)),
        (y < 15).then_some(index + 256),
        (z > 0).then_some(index.saturating_sub(16)),
        (z < 15).then_some(index + 16),
    ]
}

type CellSection = (ChunkPos, i32);

/// Visit one Section through a certified entry face. A section with no air
/// still becomes visible: its opaque exterior wall is seen from its neighbor.
fn enqueue(
    queue: &mut VecDeque<(CellSection, usize)>,
    entries: &mut HashMap<CellSection, u8>,
    section: CellSection,
    entry: usize,
) {
    let bit = 1 << entry;
    let flags = entries.entry(section).or_default();
    if *flags & bit == 0 {
        *flags |= bit;
        queue.push_back((section, entry));
    }
}

fn adjacent((pos, y): CellSection, face: usize) -> Option<CellSection> {
    let (dx, dy, dz) = NEIGHBORS[face];
    Some((
        ChunkPos {
            x: pos.x.checked_add(dx)?,
            z: pos.z.checked_add(dz)?,
            ..pos
        },
        y.checked_add(dy)?,
    ))
}

fn section_graph(
    probes: &BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    (pos, y): CellSection,
) -> Option<&SectionVisibility> {
    let probe = probes.get(&pos)?;
    Some(
        i8::try_from(y)
            .ok()
            .and_then(|key| probe.visibility.get(&key))
            .map(Arc::as_ref)
            .unwrap_or(&SectionVisibility::OPEN),
    )
}

/// Invariant across all camera positions in the same connected air pocket:
/// only the Section coordinate and its reachable boundary-face mask influence
/// portal traversal. The editor uses this lightweight signature to avoid a
/// whole-world graph walk for every block stepped through an open valley.
pub(super) fn camera_signature(
    probes: &BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    dimension: Dimension,
    camera: [i32; 3],
) -> Option<(ChunkPos, i32, u16, u8)> {
    let section = (
        ChunkPos {
            x: camera[0].div_euclid(16),
            z: camera[2].div_euclid(16),
            dimension,
        },
        camera[1].div_euclid(16),
    );
    let graph = section_graph(probes, section)?;
    let local = [
        camera[0].rem_euclid(16) as usize,
        camera[1].rem_euclid(16) as usize,
        camera[2].rem_euclid(16) as usize,
    ];
    let (component, exits) = graph.camera_region(local)?;
    Some((section.0, section.1, component, exits))
}

/// Return only render bands certified unreachable. Missing columns, unknown
/// geometry, camera-in-solid and scenes above the bounded budget all fall
/// through to the existing GPU-driven path instead of hiding any geometry.
///
/// Treat every loaded region boundary as a possible *external* entry point:
/// a cave tunnel may leave the loaded radius and re-enter elsewhere. Seeding
/// those boundaries is intentionally conservative, and never fabricates a
/// connection through a certified solid wall.
pub(super) fn hidden_render_bands(
    probes: &BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    meshes: &BTreeMap<(ChunkPos, i32), Arc<World3dChunkMesh>>,
    dimension: Dimension,
    camera: [i32; 3],
    radius: i32,
) -> BTreeSet<(ChunkPos, i32)> {
    let keys = meshes.keys().copied().collect::<BTreeSet<_>>();
    hidden_render_band_keys(probes, &keys, dimension, camera, radius)
}

/// Worker-side variant that accepts prospective render bands even before a
/// Section has a CPU mesh, material table or GPU-resident Draw Slot. Unknown
/// columns remain open and the same conservative exterior seeding applies.
pub(super) fn hidden_render_band_keys(
    probes: &BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    keys: &BTreeSet<(ChunkPos, i32)>,
    dimension: Dimension,
    camera: [i32; 3],
    radius: i32,
) -> BTreeSet<(ChunkPos, i32)> {
    let empty = BTreeSet::new();
    if probes.is_empty() || keys.is_empty() || probes.len() > MAX_COLUMNS {
        return empty;
    }
    let Some((camera_pos, camera_y, _component, camera_exits)) =
        camera_signature(probes, dimension, camera)
    else {
        return empty;
    };
    let camera_section = (camera_pos, camera_y);
    let mut min_y = camera_section.1;
    let mut max_y = camera_section.1;
    for (pos, band) in keys {
        if pos.dimension != dimension {
            continue;
        }
        min_y = min_y.min(band.saturating_mul(4));
        max_y = max_y.max(band.saturating_mul(4).saturating_add(3));
    }
    for probe in probes.values().filter(|probe| probe.pos.dimension == dimension) {
        if let Some((&first, _)) = probe.subchunks.first_key_value() {
            min_y = min_y.min(i32::from(first));
        }
        if let Some((&last, _)) = probe.subchunks.last_key_value() {
            max_y = max_y.max(i32::from(last));
        }
    }
    min_y = min_y.saturating_sub(1);
    max_y = max_y.saturating_add(1);
    if (i64::from(max_y) - i64::from(min_y) + 1) as usize > MAX_HEIGHT_SECTIONS {
        return empty;
    }
    let radius = radius.max(1).saturating_add(1);
    let inside = |pos: ChunkPos| {
        pos.dimension == dimension
            && (i64::from(pos.x) - i64::from(camera_section.0.x)).abs() <= i64::from(radius)
            && (i64::from(pos.z) - i64::from(camera_section.0.z)).abs() <= i64::from(radius)
    };
    // Build one compact column index per background solve. The border
    // flood-fill uses repeated adjacency probes; hashing avoids ordered-tree
    // traversal for each Section face.
    let columns = probes.iter()
        .filter(|(pos, _)| inside(**pos))
        .map(|(pos, probe)| (*pos, probe.as_ref()))
        .collect::<HashMap<_, _>>();
    let is_known = |section: CellSection| {
        (min_y..=max_y).contains(&section.1)
            && columns.contains_key(&section.0)
    };

    let mut queue = VecDeque::new();
    let mut entries = HashMap::<CellSection, u8>::new();
    // Begin with the actual camera air component, not all components in the
    // current Section. A camera teleported between disconnected pockets must
    // change the visible set even without crossing a Section boundary.
    entries.insert(camera_section, 0);
    for face in 0..SIDE_COUNT {
        if camera_exits & (1 << face) != 0
            && let Some(neighbor) = adjacent(camera_section, face)
            && is_known(neighbor)
        {
            enqueue(&mut queue, &mut entries, neighbor, face ^ 1);
        }
    }

    // Only the top and bottom planes are exterior for an interior column.
    // Precompute side gaps once per column, instead of probing six neighbours
    // for every height slice (including all the fully surrounded Sections).
    // Missing columns remain external *openings*, never solid occluders.
    for &pos in columns.keys() {
        enqueue(&mut queue, &mut entries, (pos, min_y), 2);
        enqueue(&mut queue, &mut entries, (pos, max_y), 3);
        let side_entries = [0usize, 1, 4, 5].into_iter()
            .filter(|&face| {
                adjacent((pos, min_y), face)
                    .is_none_or(|next| !columns.contains_key(&next.0))
            })
            .collect::<Vec<_>>();
        if side_entries.is_empty() {
            continue;
        }
        for y in min_y..=max_y {
            for &face in &side_entries {
                enqueue(&mut queue, &mut entries, (pos, y), face);
            }
        }
    }
    while let Some((section, entry)) = queue.pop_front() {
        if entries.len() > MAX_VISITED_SECTIONS {
            return empty;
        }
        let Some(probe) = columns.get(&section.0) else {
            return empty;
        };
        let graph = i8::try_from(section.1)
            .ok()
            .and_then(|y| probe.visibility.get(&y))
            .map(Arc::as_ref)
            .unwrap_or(&SectionVisibility::OPEN);
        let exits = graph.exits(entry);
        for face in 0..SIDE_COUNT {
            if exits & (1 << face) == 0 {
                continue;
            }
            if let Some(neighbor) = adjacent(section, face)
                && is_known(neighbor)
            {
                enqueue(&mut queue, &mut entries, neighbor, face ^ 1);
            }
        }
    }
    let reachable = entries
        .keys()
        .map(|(pos, section_y)| (*pos, section_y.div_euclid(4)))
        .collect::<std::collections::HashSet<_>>();
    keys
        .iter()
        .filter(|(pos, band)| {
            pos.dimension == dimension
                && inside(*pos)
                && probes.contains_key(pos)
                && *band >= min_y.div_euclid(4)
                && *band <= max_y.div_euclid(4)
                && !reachable.contains(&(*pos, *band))
        })
        .copied()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_except_air(air: &[(usize, usize, usize)]) -> [u64; SOLID_WORDS] {
        let mut bits = [u64::MAX; SOLID_WORDS];
        for &(x, y, z) in air {
            let index = (y * 16 + z) * 16 + x;
            bits[index / 64] &= !(1u64 << (index % 64));
        }
        bits
    }

    #[test]
    fn solid_section_has_no_portals_and_camera_noclip_fails_open() {
        let graph = SectionVisibility::from_solid_bits([u64::MAX; SOLID_WORDS]);
        assert!((0..SIDE_COUNT).all(|entry| graph.exits(entry) == 0));
        assert_eq!(graph.camera_exits([1, 1, 1]), None);
    }

    #[test]
    fn one_connected_air_region_uses_constant_time_exit_mask() {
        let solid = bits_except_air(&(0..16)
            .map(|x| (x, 8, 8))
            .collect::<Vec<_>>());
        let graph = SectionVisibility::from_solid_bits(solid);
        assert_eq!(graph.uniform_camera_exits, Some(0b000011));
        assert!(graph.cell_camera_exits.is_none());
        for x in 0..16 {
            assert_eq!(graph.camera_exits([x, 8, 8]), Some(0b000011));
        }
        assert_eq!(graph.camera_exits([0, 0, 0]), None);
    }

    #[test]
    fn equal_portal_masks_do_not_merge_disconnected_cave_components() {
        let mut cells = [(8usize, 8usize, 8usize), (4, 4, 4)];
        let graph = SectionVisibility::from_solid_bits(bits_except_air(&cells));
        let a = graph.camera_region([8, 8, 8]).unwrap();
        let b = graph.camera_region([4, 4, 4]).unwrap();
        assert_eq!(a.1, 0);
        assert_eq!(b.1, 0);
        assert_ne!(a.0, b.0, "separate sealed cave pockets must have distinct cache IDs");
        assert!(graph.cell_region_ids.is_some());

        // The open-border fast path must distinguish two sealed pockets,
        // even if neither pocket has a portal to any Section face.
        let mut solid = [0u64; SOLID_WORDS];
        for (x,y,z) in [
            (7,8,8), (9,8,8), (8,7,8), (8,9,8), (8,8,7), (8,8,9),
            (3,4,4), (5,4,4), (4,3,4), (4,5,4), (4,4,3), (4,4,5),
        ] {
            let index = (y * 16 + z) * 16 + x;
            solid[index / 64] |= 1u64 << (index % 64);
        }
        let graph = SectionVisibility::from_solid_bits(solid);
        let a = graph.camera_region([8, 8, 8]).unwrap();
        let b = graph.camera_region([4, 4, 4]).unwrap();
        assert_eq!(a.1, 0);
        assert_eq!(b.1, 0);
        assert_ne!(a.0, b.0);
        assert_eq!(graph.camera_region([0, 0, 0]), Some((0, ALL_SIDES)));
    }

    #[test]
    fn two_disconnected_air_regions_cache_per_voxel_exit_masks() {
        let mut air = (0..16).map(|x| (x, 4, 4)).collect::<Vec<_>>();
        air.extend((0..16).map(|z| (4, 12, z)));
        air.push((8, 8, 8));
        let graph = SectionVisibility::from_solid_bits(bits_except_air(&air));
        assert!(graph.cell_camera_exits.is_some());
        assert_eq!(graph.heap_bytes(), 4096 * 3);
        assert_eq!(graph.camera_exits([8, 4, 4]), Some(0b000011));
        assert_eq!(graph.camera_exits([4, 12, 8]), Some(0b110000));
        assert_eq!(graph.camera_exits([8, 8, 8]), Some(0));
        assert_eq!(graph.camera_exits([5, 5, 5]), None);
    }

    #[test]
    fn open_border_ray_proves_full_connectivity_but_sealed_pocket_stays_closed() {
        let mut solids = [0u64; SOLID_WORDS];
        for (x, y, z) in [
            (7usize, 8usize, 8usize), (9, 8, 8),
            (8, 7, 8), (8, 9, 8),
            (8, 8, 7), (8, 8, 9),
        ] {
            let index = (y * 16 + z) * 16 + x;
            solids[index / 64] |= 1u64 << (index % 64);
        }
        let graph = SectionVisibility::from_solid_bits(solids);
        assert_eq!(graph.camera_exits([1, 3, 4]), Some(ALL_SIDES));
        assert_eq!(graph.camera_exits([8, 8, 8]), Some(0));
    }

    #[test]
    fn empty_border_shell_fast_path_does_not_open_a_sealed_cave() {
        let mut solids = [0u64; SOLID_WORDS];
        for (x, y, z) in [
            (7usize, 8usize, 8usize), (9, 8, 8),
            (8, 7, 8), (8, 9, 8),
            (8, 8, 7), (8, 8, 9),
        ] {
            let cell = (y * 16 + z) * 16 + x;
            solids[cell / 64] |= 1u64 << (cell % 64);
        }
        let graph = SectionVisibility::from_solid_bits(solids);
        assert_eq!(graph.portals, [ALL_SIDES; SIDE_COUNT]);
        assert_eq!(graph.camera_exits([8, 8, 8]), Some(0));
        assert_eq!(graph.camera_exits([0, 0, 0]), Some(ALL_SIDES));
    }

    #[test]
    fn isolated_air_chamber_does_not_connect_any_border() {
        let graph = SectionVisibility::from_solid_bits(bits_except_air(&[(8, 8, 8)]));
        assert!((0..SIDE_COUNT).all(|entry| graph.exits(entry) == 0));
        assert_eq!(graph.camera_exits([8, 8, 8]), Some(0));
    }

    #[test]
    fn narrow_tunnel_connects_opposite_faces_but_not_side_walls() {
        let air = (0..16).map(|x| (x, 8, 8)).collect::<Vec<_>>();
        let graph = SectionVisibility::from_solid_bits(bits_except_air(&air));
        assert_eq!(graph.exits(0), 0b000011);
        assert_eq!(graph.exits(1), 0b000011);
        assert!((2..SIDE_COUNT).all(|entry| graph.exits(entry) == 0));
        assert_eq!(graph.camera_exits([8, 8, 8]), Some(0b000011));
    }

    #[test]
    fn separated_tunnels_never_false_connect_different_exits() {
        let mut air = (0..16).map(|x| (x, 4, 4)).collect::<Vec<_>>();
        air.extend((0..16).map(|z| (4, 12, z)));
        let graph = SectionVisibility::from_solid_bits(bits_except_air(&air));
        assert_eq!(graph.exits(0), 0b000011);
        assert_eq!(graph.exits(4), 0b110000);
        assert_eq!(graph.exits(0) & graph.exits(4), 0);
    }

    #[test]
    fn sealed_cave_band_is_hidden_until_camera_enters_its_air_pocket() {
        let mut probes = BTreeMap::new();
        let center = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        for x in -1..=1 {
            for z in -1..=1 {
                let pos = ChunkPos { x, z, ..center };
                let mut visibility = BTreeMap::new();
                for y in 0..12i8 {
                    let solids = if (x, z, y) == (0, 0, 6) {
                        bits_except_air(&[(8, 8, 8)])
                    } else if (x, z, y) == (-1, -1, 10) {
                        [0; SOLID_WORDS]
                    } else {
                        [u64::MAX; SOLID_WORDS]
                    };
                    visibility.insert(y, Arc::new(SectionVisibility::from_solid_bits(solids)));
                }
                probes.insert(pos, Arc::new(World3dChunkProbe {
                    pos,
                    min_y: 0,
                    subchunks: BTreeMap::new(),
                    visibility,
                    state_version: None,
                    storage_bytes: 0,
                }));
            }
        }
        let meshes = (0..3)
            .map(|section_y| {
                (
                    (center, section_y),
                    Arc::new(World3dChunkMesh {
                        occluders: Arc::from([]),
                        pos: center,
                        section_y,
                        origin: [0; 3],
                        mesh: Arc::new(gpui_3d::Mesh::cube()),
                        materials: Arc::from([]),
                        textures: Arc::from([]),
                        triangles: 12,
                        faces: 6,
                        missing_texture_faces: 0,
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        // The other eight columns surround the sealed center cave. No face
        // portal reaches the cave band despite external sky/edge seeds.
        let hidden = hidden_render_bands(
            &probes, &meshes, Dimension::Overworld, [-8, 168, -8], 3,
        );
        assert!(hidden.contains(&(center, 1)));
        // Teleport into the isolated air pocket. The cave band is now reachable
        // even though there is no portal to an adjacent Section.
        let inside = hidden_render_bands(
            &probes, &meshes, Dimension::Overworld, [8, 104, 8], 3,
        );
        assert!(!inside.contains(&(center, 1)));
        // Noclip into a certified solid voxel: never hide authored geometry.
        let noclip = hidden_render_bands(
            &probes, &meshes, Dimension::Overworld, [8, 88, 8], 3,
        );
        assert!(noclip.is_empty());
    }

    #[test]
    fn missing_lateral_column_is_an_open_external_portal() {
        let dimension = Dimension::Overworld;
        let mut probes = BTreeMap::new();
        let mut meshes = BTreeSet::new();
        // Three adjacent columns. The left edge is exposed to unknown
        // terrain, while the middle cave band has a horizontal air tunnel.
        for x in 0..3 {
            let pos = ChunkPos { x, z: 0, dimension };
            let mut visibility = BTreeMap::new();
            for y in 0..12i8 {
                let solids = if y == 6 {
                    [0; SOLID_WORDS]
                } else {
                    [u64::MAX; SOLID_WORDS]
                };
                visibility.insert(y, Arc::new(SectionVisibility::from_solid_bits(solids)));
            }
            probes.insert(pos, Arc::new(World3dChunkProbe {
                pos,
                min_y: 0,
                subchunks: BTreeMap::new(),
                visibility,
                state_version: None,
                storage_bytes: 0,
            }));
            meshes.insert((pos, 1));
        }
        // From the last column, an exterior entry through the missing
        // neighbour on the first column must keep the entire tunnel visible.
        let hidden = hidden_render_band_keys(
            &probes, &meshes, dimension, [40, 104, 8], 3,
        );
        assert!(hidden.is_empty());
    }

    #[test]
    fn camera_outside_decoded_columns_disables_visibility_pruning() {
        let probes = BTreeMap::new();
        let meshes = BTreeMap::new();
        assert!(hidden_render_bands(
            &probes, &meshes, Dimension::Overworld, [0, 96, 0], 8,
        ).is_empty());
    }

    #[test]
    fn all_air_section_connects_all_six_directions() {
        let graph = SectionVisibility::from_solid_bits([0; SOLID_WORDS]);
        assert!((0..SIDE_COUNT).all(|entry| graph.exits(entry) == ALL_SIDES));
        assert_eq!(graph.camera_exits([15, 15, 15]), Some(ALL_SIDES));
    }
}
