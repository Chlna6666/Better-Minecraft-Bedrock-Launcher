//! Grid traversal with one probe-map lookup per crossed chunk instead of per voxel.

use super::{Arc, BTreeMap, ChunkPos, Dimension, World3dChunkProbe, World3dTargetBlock};

const MAX_STEPS: usize = 1024;

pub(super) fn cast(
    probes: &BTreeMap<ChunkPos, Arc<World3dChunkProbe>>,
    dimension: Dimension,
    origin: [f64; 3],
    direction: [f64; 3],
    max_distance: f64,
) -> Option<World3dTargetBlock> {
    let mut ray = GridRay::new(origin, direction)?;
    let mut previous_chunk = None;
    let mut probe = None;
    for _ in 0..MAX_STEPS {
        if ray.distance > max_distance {
            break;
        }
        let [x, y, z] = ray.position;
        let chunk = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
            dimension,
        };
        if previous_chunk != Some(chunk) {
            previous_chunk = Some(chunk);
            probe = probes.get(&chunk);
        }
        if let Some(block) = probe.and_then(|probe| probe.block_at(x, y, z)) {
            return Some(World3dTargetBlock {
                dimension,
                position: ray.position,
                normal: ray.normal,
                block_name: block.state.name.clone(),
                chunk,
                loaded: true,
                fluid: block.fluid,
            });
        }
        if !ray.advance() {
            break;
        }
    }
    None
}

struct GridRay {
    position: [i32; 3],
    step: [i32; 3],
    delta: [f64; 3],
    next: [f64; 3],
    normal: [i32; 3],
    distance: f64,
}

impl GridRay {
    fn new(origin: [f64; 3], direction: [f64; 3]) -> Option<Self> {
        if !origin
            .iter()
            .chain(&direction)
            .all(|value| value.is_finite())
            || direction.iter().all(|value| value.abs() < 1.0e-9)
        {
            return None;
        }
        let position = origin.map(|value| value.floor() as i32);
        let step = direction.map(|value| if value > 0.0 { 1 } else { -1 });
        let delta = direction.map(|value| {
            if value.abs() < 1.0e-9 {
                f64::INFINITY
            } else {
                (1.0 / value).abs()
            }
        });
        let next = std::array::from_fn(|axis| {
            if !delta[axis].is_finite() {
                f64::INFINITY
            } else {
                let remaining = if step[axis] > 0 {
                    origin[axis].floor() + 1.0 - origin[axis]
                } else {
                    origin[axis] - origin[axis].floor()
                };
                remaining.max(0.0) * delta[axis]
            }
        });
        Some(Self {
            position,
            step,
            delta,
            next,
            normal: [0; 3],
            distance: 0.0,
        })
    }

    fn advance(&mut self) -> bool {
        let axis = if self.next[0] <= self.next[1] && self.next[0] <= self.next[2] {
            0
        } else if self.next[1] <= self.next[2] {
            1
        } else {
            2
        };
        let Some(position) = self.position[axis].checked_add(self.step[axis]) else {
            return false;
        };
        self.position[axis] = position;
        self.distance = self.next[axis];
        self.next[axis] += self.delta[axis];
        self.normal = [0; 3];
        self.normal[axis] = -self.step[axis];
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::window::map_viewer::world3d::types::{PROBE_FLAG_SOLID, ProbingSubChunk};
    use bedrock_world::{BlockPalette, BlockState, SubChunk, SubChunkFormat};

    fn probes_at(position: [i32; 3]) -> BTreeMap<ChunkPos, Arc<World3dChunkProbe>> {
        let [x, y, z] = position;
        let pos = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
            dimension: Dimension::Overworld,
        };
        let mut indices = vec![0; 4096];
        indices[(x.rem_euclid(16) * 256 + z.rem_euclid(16) * 16 + y.rem_euclid(16)) as usize] = 1;
        let states = ["minecraft:air", "minecraft:stone"].map(|name| BlockState {
            name: name.into(),
            states: BTreeMap::new(),
            version: None,
        });
        let subchunk = ProbingSubChunk {
            subchunk: SubChunk {
                y: 0,
                format: SubChunkFormat::Paletted {
                    version: 9,
                    storages: vec![BlockPalette::with_unpacked_indices(
                        states.into(),
                        indices,
                        None,
                    )],
                },
            },
            flags: vec![vec![0, PROBE_FLAG_SOLID]],
        };
        BTreeMap::from([(
            pos,
            Arc::new(World3dChunkProbe {
                pos,
                min_y: 0,
                subchunks: BTreeMap::from([(0, subchunk)]),
                state_version: None,
                storage_bytes: 0,
            }),
        )])
    }

    #[test]
    fn different_hit_faces_on_one_voxel_are_distinct_targets() {
        let probes = probes_at([0, 1, 0]);
        let north = cast(
            &probes,
            Dimension::Overworld,
            [0.5, 1.5, -1.0],
            [0.0, 0.0, 1.0],
            16.0,
        )
        .unwrap();
        let west = cast(
            &probes,
            Dimension::Overworld,
            [-1.0, 1.5, 0.5],
            [1.0, 0.0, 0.0],
            16.0,
        )
        .unwrap();
        assert_eq!(north.position, west.position);
        assert_eq!(north.normal, [0, 0, -1]);
        assert_eq!(west.normal, [-1, 0, 0]);
        assert_ne!(north, west);
    }

    #[test]
    fn negative_chunk_coordinates_and_distance_limit_are_preserved() {
        let probes = probes_at([-1, 1, -1]);
        let origin = [-0.5, 1.5, -3.0];
        let direction = [0.0, 0.0, 1.0];
        assert!(cast(&probes, Dimension::Overworld, origin, direction, 1.0).is_none());
        let hit = cast(&probes, Dimension::Overworld, origin, direction, 16.0).unwrap();
        assert_eq!(hit.position, [-1, 1, -1]);
        assert_eq!((hit.chunk.x, hit.chunk.z), (-1, -1));
    }

    #[test]
    fn invalid_rays_do_not_traverse_the_probe_map() {
        let probes = BTreeMap::new();
        assert!(cast(&probes, Dimension::Overworld, [0.0; 3], [0.0; 3], 16.0).is_none());
        assert!(
            cast(
                &probes,
                Dimension::Overworld,
                [f64::NAN; 3],
                [0.0, 0.0, 1.0],
                16.0
            )
            .is_none()
        );
    }
}
