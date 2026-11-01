//! Worker-owned light solves use one-chunk halo and retain only center samples plus one cell.
use super::*;
use bedrock_render::{LightCell, LightVolume};

pub(super) struct ChunkLighting {
    inputs: [Option<Weak<World3dChunkProbe>>; 9],
    origin: [i32; 3],
    size: [u32; 3],
    levels: Box<[u8]>,
    pub(super) ambient: f32,
    has_sky: bool,
}

impl Worker {
    pub(super) fn build_lighting(&self, pos: ChunkPos) -> Result<Arc<ChunkLighting>, String> {
        let inputs = std::array::from_fn(|i| {
            self.light_probe_arc(ChunkPos {
                x: pos.x + (i % 3) as i32 - 1,
                z: pos.z + (i / 3) as i32 - 1,
                dimension: pos.dimension,
            })
            .map(Arc::downgrade)
        });
        if let Some(old) = self.light_fields.get(&pos)
            && inputs.iter().zip(&old.inputs).all(|(a, b)| match (a, b) {
                (Some(a), Some(b)) => Weak::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            })
        {
            return Ok(old.clone());
        }
        let probe = self
            .light_probe(pos)
            .ok_or_else(|| "光照计算缺少目标区块".to_string())?;
        let version = if probe.min_y < 0 {
            ChunkVersion::New
        } else {
            ChunkVersion::Old
        };
        let (min_y, max_y) = pos.y_range(version);
        let height = u32::try_from(max_y - min_y + 1).map_err(|_| "光照高度无效")?;
        let size = [48, height, 48];
        let origin = [pos.x * 16 - 16, min_y, pos.z * 16 - 16];
        let mut cells = vec![LightCell::OPAQUE; (48 * height * 48) as usize];
        for dz in -1..=1 {
            for dx in -1..=1 {
                let neighbor = ChunkPos {
                    x: pos.x + dx,
                    z: pos.z + dz,
                    dimension: pos.dimension,
                };
                let Some(probe) = self.light_probe(neighbor) else {
                    continue;
                };
                write_probe_cells(
                    &self.models,
                    probe,
                    &mut cells,
                    [((dx + 1) * 16), min_y, ((dz + 1) * 16)],
                );
            }
        }
        let has_sky = pos.dimension == Dimension::Overworld;
        let solved = LightVolume::solve(size, &cells, has_sky).map_err(|e| e.to_string())?;
        let mut levels = Vec::with_capacity((18 * height * 18) as usize);
        for z in 15..33 {
            for y in 0..height {
                for x in 15..33 {
                    levels.push(solved.packed_at([x, y, z]).unwrap_or(0));
                }
            }
        }
        Ok(Arc::new(ChunkLighting {
            inputs,
            origin: [origin[0] + 15, min_y, origin[2] + 15],
            size: [18, height, 18],
            levels: levels.into_boxed_slice(),
            ambient: match pos.dimension {
                Dimension::Nether => 0.25,
                Dimension::End => 0.15,
                _ => 0.05,
            },
            has_sky,
        }))
    }

    fn light_probe(&self, pos: ChunkPos) -> Option<&World3dChunkProbe> {
        self.light_probe_arc(pos).map(Arc::as_ref)
    }
    fn light_probe_arc(&self, pos: ChunkPos) -> Option<&Arc<World3dChunkProbe>> {
        self.probe_stash
            .get(&pos)
            .map(|entry| &entry.0)
            .or_else(|| self.cache.get(&pos).map(|chunk| &chunk.probe))
    }
}

/// Collate complete storage layers once per palette, including waterlogged solids.
fn write_probe_cells(
    models: &bedrock_block_model::BlockModelRepository,
    probe: &World3dChunkProbe,
    cells: &mut [LightCell],
    origin: [i32; 3],
) {
    let [ox, min_y, oz] = origin;
    let height = cells.len() / (48 * 48);
    let max_y = min_y + height as i32 - 1;
    let (ox, oz) = (ox as usize, oz as usize);
    for z in oz..oz + 16 {
        for y in 0..height {
            let begin = (z * height + y) * 48 + ox;
            cells[begin..begin + 16].fill(LightCell::AIR);
        }
    }
    for (&sub_y, entry) in &probe.subchunks {
        let palettes = match &entry.subchunk.format {
            SubChunkFormat::Paletted { storages, .. } => storages
                .iter()
                .map(|storage| {
                    let properties = storage
                        .states
                        .iter()
                        .map(|state| {
                            models.light_properties(
                                &bedrock_block_model::BlockStateQuery::from_world_state(state),
                            )
                        })
                        .collect::<Vec<_>>();
                    (storage, properties)
                })
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        };
        for i in 0..4096u16 {
            let (x, y, z) = ((i % 16) as u8, (i / 16 % 16) as u8, (i / 256) as u8);
            let world_y = i32::from(sub_y) * 16 + i32::from(y);
            if !(min_y..=max_y).contains(&world_y) {
                continue;
            }
            let mut emission = 0;
            let mut dampening = 0;
            if palettes.is_empty() {
                dampening = if entry.subchunk.legacy_block_id_at(x, y, z) == Some(0) {
                    0
                } else {
                    15
                };
            } else {
                for (storage, palette) in &palettes {
                    if let Some(p) = storage
                        .palette_index_at(x, y, z)
                        .and_then(|i| palette.get(usize::from(i)))
                    {
                        emission = emission.max(p.emission);
                        dampening = dampening.max(p.dampening);
                    } else {
                        dampening = 15;
                    }
                }
            }
            let index = ((oz + usize::from(z)) * height + (world_y - min_y) as usize) * 48
                + ox
                + usize::from(x);
            cells[index] = LightCell::new(emission, dampening).unwrap_or(LightCell::OPAQUE);
        }
    }
}

impl ChunkLighting {
    pub(super) fn storage_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.levels.len()
    }

    pub(super) fn changed_bands(&self, previous: Option<&Self>) -> Vec<(i32, i32)> {
        let Some(old) = previous.filter(|old| old.origin == self.origin && old.size == self.size)
        else {
            return vec![(
                self.origin[1].div_euclid(64),
                (self.origin[1] + self.size[1] as i32 - 1).div_euclid(64),
            )];
        };
        if std::ptr::eq(self, old) {
            return Vec::new();
        }
        let mut min = i32::MAX;
        let mut max = i32::MIN;
        for (i, (&next, &previous)) in self.levels.iter().zip(old.levels.iter()).enumerate() {
            if next != previous {
                let y = self.origin[1] + (i as u32 / self.size[0] % self.size[1]) as i32;
                min = min.min(y);
                max = max.max(y);
            }
        }
        if min > max {
            Vec::new()
        } else {
            vec![(
                (min - 1).max(self.origin[1]).div_euclid(64),
                (max + 1)
                    .min(self.origin[1] + self.size[1] as i32 - 1)
                    .div_euclid(64),
            )]
        }
    }
    pub(super) fn packed_at(&self, p: [i32; 3]) -> u8 {
        let q = std::array::from_fn::<_, 3, _>(|a| i64::from(p[a]) - i64::from(self.origin[a]));
        if self.has_sky && q[1] >= i64::from(self.size[1]) {
            return 0xf0;
        }
        if (0..3).any(|a| q[a] < 0 || q[a] >= i64::from(self.size[a])) {
            return 0;
        }
        self.levels
            [((q[2] * i64::from(self.size[1]) + q[1]) * i64::from(self.size[0]) + q[0]) as usize]
    }

    pub(super) fn shade_faces(&self, faces: &mut [p3d::Preview3dStreamFace]) {
        for face in faces {
            let center: [f64; 3] =
                std::array::from_fn(|a| face.corners.iter().map(|p| p[a]).sum::<f64>() * 0.25);
            let own = self.packed_at(std::array::from_fn(|a| {
                (center[a] - f64::from(face.normal[a]) * 0.001).floor() as i32
            }));
            let next = self.packed_at(std::array::from_fn(|a| {
                (center[a] + f64::from(face.normal[a]) * 0.001).floor() as i32
            }));
            let level = (own & 15).max(own >> 4).max(next & 15).max(next >> 4);
            let value = f32::from(level) / 15.;
            let factor = self.ambient + (1. - self.ambient) * value * value;
            for light in &mut face.ambient_light {
                *light *= factor;
            }
        }
    }

    pub(super) fn gpu_levels(&self, pos: ChunkPos, band: &p3d::GpuCubeBand) -> Vec<u8> {
        (0..band.size[0] * band.size[1] * band.size[2])
            .map(|i| {
                self.packed_at([
                    pos.x * 16 + band.origin.x as i32 + (i % band.size[0]) as i32,
                    band.origin.y as i32 + (i / band.size[0] % band.size[1]) as i32,
                    pos.z * 16 + band.origin.z as i32 + (i / band.size[0] / band.size[1]) as i32,
                ])
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn field() -> ChunkLighting {
        ChunkLighting {
            inputs: std::array::from_fn(|_| None),
            origin: [-1, -64, -1],
            size: [18, 384, 18],
            levels: vec![0xf0; 18 * 384 * 18].into_boxed_slice(),
            ambient: 0.05,
            has_sky: true,
        }
    }
    #[test]
    fn sky_column_changes_invalidate_all_affected_height_bands() {
        let before = field();
        let mut after = field();
        for y in 0..=200 {
            after.levels[(7 * 384 + y) * 18 + 7] = 0;
        }
        assert_eq!(after.changed_bands(Some(&before)), vec![(-1, 2)]);
        assert!(before.changed_bands(Some(&before)).is_empty());
        let mut boundary = field();
        boundary.levels[(7 * 384 + 127) * 18 + 7] = 0;
        assert_eq!(boundary.changed_bands(Some(&before)), vec![(0, 1)]);
        assert_eq!(before.changed_bands(None), vec![(-1, 4)]);
    }

    #[test]
    fn waterlogged_storage_preserves_solid_opacity_and_unknown_payloads_stay_dark() {
        use bedrock_world::{BlockPalette, BlockState};
        let storage = |name: &str| {
            BlockPalette::with_unpacked_indices(
                vec![BlockState {
                    name: name.into(),
                    states: BTreeMap::new(),
                    version: None,
                }],
                vec![0; 4096],
                None,
            )
        };
        let pos = ChunkPos {
            x: 0,
            z: 0,
            dimension: Dimension::Overworld,
        };
        let mut probe = World3dChunkProbe {
            pos,
            min_y: 0,
            state_version: None,
            storage_bytes: 0,
            subchunks: BTreeMap::from([(
                0,
                super::super::super::types::ProbingSubChunk {
                    subchunk: bedrock_world::SubChunk {
                        y: 0,
                        format: SubChunkFormat::Paletted {
                            version: 9,
                            storages: vec![storage("minecraft:stone"), storage("minecraft:water")],
                        },
                    },
                    flags: vec![],
                },
            )]),
        };
        let models = bedrock_block_model::BlockModelRepository::new();
        let mut cells = vec![LightCell::OPAQUE; 48 * 16 * 48];
        write_probe_cells(&models, &probe, &mut cells, [16, 0, 16]);
        assert_eq!(
            LightVolume::solve([48, 16, 48], &cells, true)
                .unwrap()
                .packed_at([16, 15, 16]),
            Some(0)
        );
        if let SubChunkFormat::Paletted { storages, .. } =
            &mut probe.subchunks.get_mut(&0).unwrap().subchunk.format
        {
            storages[0] = storage("minecraft:air");
        }
        write_probe_cells(&models, &probe, &mut cells, [16, 0, 16]);
        assert_eq!(
            LightVolume::solve([48, 16, 48], &cells, true)
                .unwrap()
                .packed_at([16, 15, 16]),
            Some(13 << 4)
        );
        probe.subchunks.get_mut(&0).unwrap().subchunk.format = SubChunkFormat::Raw {
            version: None,
            bytes: Vec::new().into(),
        };
        write_probe_cells(&models, &probe, &mut cells, [16, 0, 16]);
        assert_eq!(cells[(16 * 16 + 15) * 48 + 16], LightCell::OPAQUE);
    }
}
