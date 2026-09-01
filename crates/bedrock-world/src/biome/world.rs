//! Minecraft Bedrock biome and height-map world records.
//!
//! Height maps are stored as the first 512 bytes of `Data2D`, `Data2DLegacy`, and `Data3D`.
//! Ordinary height-map edits preserve the exact biome payload that follows those bytes instead of
//! decoding and rebuilding unrelated persisted data.

use crate::world::{World, StorageBackend};
use crate::biome::Biome2dLegacy;
use crate::chunk::{ChunkKey, ChunkPos, ChunkRecordTag, ChunkVersion};
use crate::error::{BedrockWorldError, Result};
use crate::scan::{Biome2d, Biome3d, HeightMap2d, BiomeData};
use bytes::Bytes;

const HEIGHT_MAP_BYTE_LEN: usize = 256 * 2;

impl<S> World<S>
where
    S: StorageBackend,
{
    /// Reads the `Data2D`/`Data2DLegacy`/`Data3D` height map for a chunk.
    ///
    /// # Errors
    ///
    /// Returns storage errors or biome/height-map parse errors.
    pub fn heightmap(&self, pos: ChunkPos) -> Result<Option<HeightMap2d>> {
        self.biome_data(pos)?
            .map(|data| HeightMap2d::new(data.height_map))
            .transpose()
    }

    /// Writes a chunk height map without changing its persisted biome representation.
    ///
    /// `ChunkVersion::Old` updates exactly one existing `Data2D` or `Data2DLegacy` record.
    /// `ChunkVersion::New` requires an existing `Data3D` record. Missing, ambiguous, or mixed
    /// old/new biome representations are rejected instead of letting the caller select one side of
    /// conflicting persisted data or fabricating biome IDs/storages.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation errors when the
    /// requested persisted representation is missing, ambiguous, or mixed, parse errors for corrupt
    /// biome records, or storage errors.
    pub fn put_heightmap(
        &self,
        pos: ChunkPos,
        version: ChunkVersion,
        height_map: HeightMap2d,
    ) -> Result<()> {
        self.ensure_writable()?;
        let heights = HeightMap2d::new(height_map.values)?.values;
        match version {
            ChunkVersion::Old => self.put_old_heightmap(pos, &heights),
            ChunkVersion::New => self.put_data3d_heightmap(pos, &heights),
        }
    }

    /// Writes a full `Data3D` biome payload after roundtrip validation.
    ///
    /// Existing `Data2D` or `Data2DLegacy` records make this operation fail before writing. Turning
    /// an old biome representation into `Data3D` is an explicit migration and must not happen as a
    /// side effect of an ordinary typed put.
    ///
    /// # Errors
    ///
    /// Returns [`BedrockWorldError::ReadOnly`] for read-only worlds, validation errors for
    /// malformed or competing biome storage, or storage errors.
    pub fn put_biome_storage(&self, pos: ChunkPos, biome: Biome3d) -> Result<()> {
        self.ensure_writable()?;
        if self.has_old_biome_record(pos)? {
            return Err(BedrockWorldError::Validation(
                "cannot write Data3D biome storage: chunk contains Data2D or Data2DLegacy; use an explicit biome migration first"
                    .to_string(),
            ));
        }
        let value = biome.encode()?;
        Biome3d::parse(&value)?;
        self.put_raw(&ChunkKey::new(pos, ChunkRecordTag::Data3D), &value)
    }

    pub(crate) fn biome_data(&self, pos: ChunkPos) -> Result<Option<BiomeData>> {
        let mut parsed = None;
        let mut parsed_tag = None;
        for tag in [
            ChunkRecordTag::Data3D,
            ChunkRecordTag::Data2D,
            ChunkRecordTag::Data2DLegacy,
        ] {
            let key = ChunkKey::new(pos, tag).encode();
            let Some(value) = self.storage().get(&key)? else {
                continue;
            };
            if let Some(existing_tag) = parsed_tag {
                return Err(BedrockWorldError::CorruptWorld(format!(
                    "chunk ({}, {}, {:?}) contains mixed biome records {existing_tag:?} and {tag:?}",
                    pos.x, pos.z, pos.dimension
                )));
            }
            let biome_data = match tag {
                ChunkRecordTag::Data3D => crate::scan::parse_data3d(&value),
                ChunkRecordTag::Data2D => crate::scan::parse_legacy_data2d(&value),
                ChunkRecordTag::Data2DLegacy => crate::scan::parse_data2d_legacy(&value),
                _ => unreachable!("biome record loop contains only biome tags"),
            }
            .map_err(|error| BedrockWorldError::CorruptWorld(format!("biome data: {error}")))?;
            parsed = Some(biome_data);
            parsed_tag = Some(tag);
        }
        Ok(parsed)
    }

    fn put_old_heightmap(&self, pos: ChunkPos, heights: &[i16]) -> Result<()> {
        let data2d_key = ChunkKey::new(pos, ChunkRecordTag::Data2D);
        let legacy_key = ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy);
        let data3d_key = ChunkKey::new(pos, ChunkRecordTag::Data3D);
        let data2d = self.storage().get(&data2d_key.encode())?;
        let legacy = self.storage().get(&legacy_key.encode())?;
        let data3d = self.storage().get(&data3d_key.encode())?;

        if data3d.is_some() {
            return Err(BedrockWorldError::Validation(
                "cannot write old height map: chunk also contains Data3D; resolve the mixed biome representation with an explicit migration first"
                    .to_string(),
            ));
        }

        match (data2d, legacy) {
            (Some(value), None) => {
                Biome2d::parse(&value).map_err(|error| {
                    BedrockWorldError::CorruptWorld(format!("Data2D biome data: {error}"))
                })?;
                let value = replace_height_map_prefix(value, heights)?;
                self.put_raw(&data2d_key, &value)
            }
            (None, Some(value)) => {
                Biome2dLegacy::parse(&value).map_err(|error| {
                    BedrockWorldError::CorruptWorld(format!("Data2DLegacy biome data: {error}"))
                })?;
                let value = replace_height_map_prefix(value, heights)?;
                self.put_raw(&legacy_key, &value)
            }
            (None, None) => Err(BedrockWorldError::Validation(
                "cannot write old height map: chunk has neither Data2D nor Data2DLegacy"
                    .to_string(),
            )),
            (Some(_), Some(_)) => Err(BedrockWorldError::Validation(
                "cannot write old height map: chunk contains both Data2D and Data2DLegacy"
                    .to_string(),
            )),
        }
    }

    fn put_data3d_heightmap(&self, pos: ChunkPos, heights: &[i16]) -> Result<()> {
        let key = ChunkKey::new(pos, ChunkRecordTag::Data3D);
        let value = self.storage().get(&key.encode())?.ok_or_else(|| {
            BedrockWorldError::Validation(
                "cannot write new height map: chunk has no Data3D record".to_string(),
            )
        })?;
        if self.has_old_biome_record(pos)? {
            return Err(BedrockWorldError::Validation(
                "cannot write new height map: chunk also contains Data2D or Data2DLegacy; resolve the mixed biome representation with an explicit migration first"
                    .to_string(),
            ));
        }
        Biome3d::parse(&value).map_err(|error| {
            BedrockWorldError::CorruptWorld(format!("Data3D biome data: {error}"))
        })?;
        let value = replace_height_map_prefix(value, heights)?;
        self.put_raw(&key, &value)
    }

    fn has_old_biome_record(&self, pos: ChunkPos) -> Result<bool> {
        for tag in [ChunkRecordTag::Data2D, ChunkRecordTag::Data2DLegacy] {
            if self
                .storage()
                .get(&ChunkKey::new(pos, tag).encode())?
                .is_some()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

fn replace_height_map_prefix(value: Bytes, heights: &[i16]) -> Result<Vec<u8>> {
    if heights.len() != 256 {
        return Err(BedrockWorldError::Validation(format!(
            "height map must contain 256 values, got {}",
            heights.len()
        )));
    }
    if value.len() < HEIGHT_MAP_BYTE_LEN {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "biome record is {} bytes, shorter than the {HEIGHT_MAP_BYTE_LEN}-byte height map",
            value.len()
        )));
    }

    let mut value = value.to_vec();
    for (slot, height) in value[..HEIGHT_MAP_BYTE_LEN]
        .chunks_exact_mut(2)
        .zip(heights.iter().copied())
    {
        slot.copy_from_slice(&height.to_le_bytes());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::biome::data2d_to_data3d;
    use crate::chunk::LegacyBiomeSample;
    use crate::surface::{BiomeDataRequirement, ChunkDataRequest, ChunkLoadOptions};
    use crate::{Dimension, MemoryStorage, OpenOptions, WorldStorage};
    use std::sync::Arc;

    fn chunk() -> ChunkPos {
        ChunkPos {
            x: 3,
            z: -2,
            dimension: Dimension::Overworld,
        }
    }

    fn writable_world(storage: Arc<MemoryStorage>) -> World<Arc<dyn WorldStorage>> {
        World::from_storage(
            "memory",
            storage,
            OpenOptions {
                read_only: false,
                ..OpenOptions::default()
            },
        )
    }

    #[test]
    fn heightmap_write_preserves_data2d_biomes_byte_for_byte() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let biomes = (0..256).map(|index| (index % 251) as u8).collect();
        let original = Biome2d::new(vec![64; 256], biomes)
            .expect("Data2D")
            .encode()
            .expect("encode Data2D");
        let key = ChunkKey::new(pos, ChunkRecordTag::Data2D);
        storage.put(&key.encode(), &original).expect("put Data2D");
        let world = writable_world(storage.clone());

        world
            .put_heightmap(
                pos,
                ChunkVersion::Old,
                HeightMap2d::new(vec![91; 256]).expect("height map"),
            )
            .expect("write height map");

        let written = storage
            .get(&key.encode())
            .expect("get Data2D")
            .expect("Data2D exists");
        assert_eq!(
            &written[HEIGHT_MAP_BYTE_LEN..],
            &original[HEIGHT_MAP_BYTE_LEN..]
        );
        assert_eq!(
            Biome2d::parse(&written).expect("parse Data2D").height_map,
            vec![91; 256]
        );
    }

    #[test]
    fn heightmap_write_preserves_data2d_legacy_id_and_rgb_byte_for_byte() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let biomes = (0_u16..256)
            .map(|index| LegacyBiomeSample {
                biome_id: (index % 251) as u8,
                red: index as u8,
                green: index.wrapping_mul(3) as u8,
                blue: index.wrapping_mul(7) as u8,
            })
            .collect();
        let original = Biome2dLegacy::new(vec![64; 256], biomes)
            .expect("Data2DLegacy")
            .encode()
            .expect("encode Data2DLegacy");
        let key = ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy);
        storage
            .put(&key.encode(), &original)
            .expect("put Data2DLegacy");
        let world = writable_world(storage.clone());

        world
            .put_heightmap(
                pos,
                ChunkVersion::Old,
                HeightMap2d::new(vec![37; 256]).expect("height map"),
            )
            .expect("write height map");

        let written = storage
            .get(&key.encode())
            .expect("get Data2DLegacy")
            .expect("Data2DLegacy exists");
        assert_eq!(
            &written[HEIGHT_MAP_BYTE_LEN..],
            &original[HEIGHT_MAP_BYTE_LEN..]
        );
        assert_eq!(
            Biome2dLegacy::parse(&written)
                .expect("parse Data2DLegacy")
                .height_map,
            vec![37; 256]
        );
    }

    #[test]
    fn data2d_legacy_read_uses_biome_ids_instead_of_rgb_bytes() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let mut biomes = vec![
            LegacyBiomeSample {
                biome_id: 7,
                red: 201,
                green: 202,
                blue: 203,
            };
            256
        ];
        biomes[17].biome_id = 42;
        let original = Biome2dLegacy::new(vec![64; 256], biomes)
            .expect("Data2DLegacy")
            .encode()
            .expect("encode Data2DLegacy");
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy).encode(),
                &original,
            )
            .expect("put Data2DLegacy");
        let world = writable_world(storage);

        let parsed = world
            .biome_data(pos)
            .expect("read Data2DLegacy")
            .expect("biome data");
        assert_eq!(parsed.storages[0].biome_id_at(1, 0, 1), Some(42));

        let chunk = world
            .query_chunk_data(
                pos,
                ChunkLoadOptions::for_data_request(
                    ChunkDataRequest::new().biome(BiomeDataRequirement::All),
                ),
            )
            .expect("render Data2DLegacy");
        assert_eq!(
            chunk
                .biome_data
                .values()
                .next()
                .and_then(|storage| storage.biome_id_at(1, 0, 1)),
            Some(42)
        );
    }

    #[test]
    fn heightmap_write_preserves_data3d_biome_storages_byte_for_byte() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let mut biomes = vec![5; 256];
        biomes[17] = 42;
        let original = data2d_to_data3d(
            &Biome2d::new(vec![64; 256], biomes).expect("Data2D"),
            -4..=-3,
        )
        .expect("Data3D")
        .encode()
        .expect("encode Data3D");
        let key = ChunkKey::new(pos, ChunkRecordTag::Data3D);
        storage.put(&key.encode(), &original).expect("put Data3D");
        let world = writable_world(storage.clone());

        world
            .put_heightmap(
                pos,
                ChunkVersion::New,
                HeightMap2d::new(vec![123; 256]).expect("height map"),
            )
            .expect("write height map");

        let written = storage
            .get(&key.encode())
            .expect("get Data3D")
            .expect("Data3D exists");
        assert_eq!(
            &written[HEIGHT_MAP_BYTE_LEN..],
            &original[HEIGHT_MAP_BYTE_LEN..]
        );
        assert_eq!(
            Biome3d::parse(&written).expect("parse Data3D").height_map,
            vec![123; 256]
        );
    }

    #[test]
    fn heightmap_write_rejects_mixed_old_and_new_biome_records() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let data2d_key = ChunkKey::new(pos, ChunkRecordTag::Data2D);
        let data3d_key = ChunkKey::new(pos, ChunkRecordTag::Data3D);
        let data2d = Biome2d::new(vec![64; 256], vec![11; 256])
            .expect("Data2D")
            .encode()
            .expect("encode Data2D");
        let data3d = data2d_to_data3d(
            &Biome2d::new(vec![70; 256], vec![27; 256]).expect("Data2D"),
            -4..=-4,
        )
        .expect("Data3D")
        .encode()
        .expect("encode Data3D");
        storage
            .put(&data2d_key.encode(), &data2d)
            .expect("put Data2D");
        storage
            .put(&data3d_key.encode(), &data3d)
            .expect("put Data3D");
        let world = writable_world(storage.clone());

        assert!(
            world
                .put_heightmap(
                    pos,
                    ChunkVersion::Old,
                    HeightMap2d::new(vec![80; 256]).expect("old height map"),
                )
                .is_err()
        );
        assert!(
            world
                .put_heightmap(
                    pos,
                    ChunkVersion::New,
                    HeightMap2d::new(vec![90; 256]).expect("new height map"),
                )
                .is_err()
        );
        assert_eq!(
            storage.get(&data2d_key.encode()).expect("get Data2D"),
            Some(Bytes::from(data2d))
        );
        assert_eq!(
            storage.get(&data3d_key.encode()).expect("get Data3D"),
            Some(Bytes::from(data3d))
        );
    }

    #[test]
    fn biome_read_rejects_mixed_persisted_representations() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let data2d = Biome2d::new(vec![64; 256], vec![11; 256])
            .expect("Data2D")
            .encode()
            .expect("encode Data2D");
        let data3d = data2d_to_data3d(
            &Biome2d::new(vec![70; 256], vec![27; 256]).expect("Data2D"),
            -4..=-4,
        )
        .expect("Data3D")
        .encode()
        .expect("encode Data3D");
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data2D).encode(),
                &data2d,
            )
            .expect("put Data2D");
        storage
            .put(
                &ChunkKey::new(pos, ChunkRecordTag::Data3D).encode(),
                &data3d,
            )
            .expect("put Data3D");
        let world = writable_world(storage);

        let error = world
            .biome_data(pos)
            .expect_err("mixed biome records must be rejected");
        assert!(matches!(error, BedrockWorldError::CorruptWorld(_)));

        let error = world
            .query_chunk_data(
                pos,
                ChunkLoadOptions::for_data_request(ChunkDataRequest::new().height_map()),
            )
            .expect_err("batched biome reads must reject mixed records");
        assert!(matches!(error, BedrockWorldError::CorruptWorld(_)));
    }

    #[test]
    fn biome_storage_write_rejects_old_biome_record_without_mutation() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let data2d_key = ChunkKey::new(pos, ChunkRecordTag::Data2D);
        let data3d_key = ChunkKey::new(pos, ChunkRecordTag::Data3D);
        let data2d = Biome2d::new(vec![64; 256], vec![19; 256])
            .expect("Data2D")
            .encode()
            .expect("encode Data2D");
        storage
            .put(&data2d_key.encode(), &data2d)
            .expect("put Data2D");
        let world = writable_world(storage.clone());
        let data3d = data2d_to_data3d(
            &Biome2d::new(vec![72; 256], vec![31; 256]).expect("Data2D"),
            -4..=-4,
        )
        .expect("Data3D");

        assert!(world.put_biome_storage(pos, data3d).is_err());
        assert_eq!(
            storage.get(&data2d_key.encode()).expect("get Data2D"),
            Some(Bytes::from(data2d))
        );
        assert!(
            storage
                .get(&data3d_key.encode())
                .expect("get Data3D")
                .is_none()
        );
    }

    #[test]
    fn heightmap_write_rejects_missing_or_ambiguous_persisted_representation() {
        let pos = chunk();
        let storage = Arc::new(MemoryStorage::new());
        let world = writable_world(storage.clone());
        let height_map = HeightMap2d::new(vec![72; 256]).expect("height map");

        assert!(
            world
                .put_heightmap(pos, ChunkVersion::Old, height_map.clone())
                .is_err()
        );
        assert!(
            world
                .put_heightmap(pos, ChunkVersion::New, height_map.clone())
                .is_err()
        );
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::Data2D).encode())
                .expect("get Data2D")
                .is_none()
        );
        assert!(
            storage
                .get(&ChunkKey::new(pos, ChunkRecordTag::Data3D).encode())
                .expect("get Data3D")
                .is_none()
        );

        let data2d_key = ChunkKey::new(pos, ChunkRecordTag::Data2D);
        let legacy_key = ChunkKey::new(pos, ChunkRecordTag::Data2DLegacy);
        let data2d = Biome2d::new(vec![64; 256], vec![9; 256])
            .expect("Data2D")
            .encode()
            .expect("encode Data2D");
        let legacy = Biome2dLegacy::new(
            vec![65; 256],
            vec![
                LegacyBiomeSample {
                    biome_id: 10,
                    red: 20,
                    green: 30,
                    blue: 40,
                };
                256
            ],
        )
        .expect("Data2DLegacy")
        .encode()
        .expect("encode Data2DLegacy");
        storage
            .put(&data2d_key.encode(), &data2d)
            .expect("put Data2D");
        storage
            .put(&legacy_key.encode(), &legacy)
            .expect("put Data2DLegacy");

        assert!(
            world
                .put_heightmap(pos, ChunkVersion::Old, height_map)
                .is_err()
        );
        assert_eq!(
            storage.get(&data2d_key.encode()).expect("get Data2D"),
            Some(Bytes::from(data2d))
        );
        assert_eq!(
            storage.get(&legacy_key.encode()).expect("get Data2DLegacy"),
            Some(Bytes::from(legacy))
        );
    }
}
