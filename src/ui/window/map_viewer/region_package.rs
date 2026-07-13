use super::editor::copy_chunks_blocking;
use super::model::*;
use super::prelude::*;

const REGION_PACKAGE_MAGIC: &[u8] = b"BMCBLREGION\0";
const REGION_PACKAGE_VERSION: u32 = 1;

pub(super) const REGION_PACKAGE_EXTENSION: &str = "bmcblregion";

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionPackageDisk {
    version: u32,
    source: RegionChunkPosDisk,
    chunks: Vec<RegionChunkDisk>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionChunkDisk {
    chunk: RegionChunkPosDisk,
    records: Vec<RegionRecordDisk>,
    block_entities: Vec<RegionBlockEntityDisk>,
    hardcoded_spawn_areas: Vec<RegionHardcodedSpawnAreaDisk>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionChunkPosDisk {
    x: i32,
    z: i32,
    dimension_id: i32,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionRecordDisk {
    key: Vec<u8>,
    value: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionBlockEntityDisk {
    id: Option<String>,
    position: Option<[i32; 3]>,
    is_movable: Option<bool>,
    custom_name: Option<String>,
    nbt: NbtTag,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegionHardcodedSpawnAreaDisk {
    kind: u8,
    min: [i32; 3],
    max: [i32; 3],
}

impl RegionPackageDisk {
    fn from_copied_chunk(copied_chunk: &CopiedChunkData) -> Self {
        Self {
            version: REGION_PACKAGE_VERSION,
            source: copied_chunk.source.into(),
            chunks: copied_chunk
                .chunks
                .iter()
                .map(RegionChunkDisk::from_snapshot)
                .collect(),
        }
    }

    fn into_copied_chunk(self) -> Result<CopiedChunkData, String> {
        if self.version != REGION_PACKAGE_VERSION {
            return Err(format!(
                "区域包版本不支持：{}（当前支持 {}）",
                self.version, REGION_PACKAGE_VERSION
            ));
        }
        if self.chunks.is_empty() {
            return Err("区域包没有可导入的 chunk".to_string());
        }

        let source = self.source.into_chunk_pos();
        let mut chunks = Vec::with_capacity(self.chunks.len());
        for chunk in self.chunks {
            chunks.push(chunk.into_snapshot()?);
        }
        Ok(CopiedChunkData { source, chunks })
    }
}

impl RegionChunkDisk {
    fn from_snapshot(snapshot: &CopiedChunkSnapshot) -> Self {
        Self {
            chunk: snapshot.chunk.into(),
            records: snapshot
                .records
                .iter()
                .map(|record| RegionRecordDisk {
                    key: record.key.encode().to_vec(),
                    value: record.value.as_ref().to_vec(),
                })
                .collect(),
            block_entities: snapshot
                .block_entities
                .iter()
                .map(RegionBlockEntityDisk::from_entity)
                .collect(),
            hardcoded_spawn_areas: snapshot
                .hardcoded_spawn_areas
                .iter()
                .map(RegionHardcodedSpawnAreaDisk::from_area)
                .collect(),
        }
    }

    fn into_snapshot(self) -> Result<CopiedChunkSnapshot, String> {
        let chunk = self.chunk.into_chunk_pos();
        let mut records = Vec::with_capacity(self.records.len());
        for record in self.records {
            let mut key = ChunkKey::decode(&record.key)
                .map_err(|error| format!("区域包包含无效 chunk key：{error}"))?;
            key.pos = chunk;
            records.push(ChunkRecord {
                key,
                value: Bytes::from(record.value),
            });
        }

        let block_entities = self
            .block_entities
            .into_iter()
            .map(RegionBlockEntityDisk::into_entity)
            .collect();
        let hardcoded_spawn_areas = self
            .hardcoded_spawn_areas
            .into_iter()
            .map(RegionHardcodedSpawnAreaDisk::into_area)
            .collect();

        Ok(CopiedChunkSnapshot {
            chunk,
            records,
            block_entities,
            hardcoded_spawn_areas,
        })
    }
}

impl From<ChunkPos> for RegionChunkPosDisk {
    fn from(chunk: ChunkPos) -> Self {
        Self {
            x: chunk.x,
            z: chunk.z,
            dimension_id: chunk.dimension.id(),
        }
    }
}

impl RegionChunkPosDisk {
    fn into_chunk_pos(self) -> ChunkPos {
        ChunkPos {
            x: self.x,
            z: self.z,
            dimension: Dimension::from_id(self.dimension_id),
        }
    }
}

impl RegionBlockEntityDisk {
    fn from_entity(entity: &ParsedBlockEntity) -> Self {
        Self {
            id: entity.id.clone(),
            position: entity.position,
            is_movable: entity.is_movable,
            custom_name: entity.custom_name.clone(),
            nbt: entity.nbt.clone(),
        }
    }

    fn into_entity(self) -> ParsedBlockEntity {
        let id = self.id.or_else(|| nbt_string_field(&self.nbt, "id"));
        let position = self.position.or_else(|| nbt_block_position(&self.nbt));
        let is_movable = self
            .is_movable
            .or_else(|| nbt_bool_field(&self.nbt, "isMovable"));
        let custom_name = self
            .custom_name
            .or_else(|| nbt_string_field(&self.nbt, "CustomName"));
        ParsedBlockEntity {
            id,
            position,
            is_movable,
            custom_name,
            items: Vec::new(),
            nbt: self.nbt,
        }
    }
}

impl RegionHardcodedSpawnAreaDisk {
    fn from_area(area: &ParsedHardcodedSpawnArea) -> Self {
        Self {
            kind: area.kind.byte(),
            min: area.min,
            max: area.max,
        }
    }

    fn into_area(self) -> ParsedHardcodedSpawnArea {
        ParsedHardcodedSpawnArea {
            kind: HardcodedSpawnAreaKind::from_byte(self.kind),
            min: self.min,
            max: self.max,
        }
    }
}

pub(super) fn default_region_package_file_name(selection: ChunkSelection) -> String {
    let bounds = selection.bounds();
    format!(
        "chunk-selection-{}-{}-{}-{}.{}",
        bounds.min_chunk_x,
        bounds.min_chunk_z,
        bounds.max_chunk_x,
        bounds.max_chunk_z,
        REGION_PACKAGE_EXTENSION
    )
}

pub(super) fn is_region_package_path(path: &PathBuf) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case(REGION_PACKAGE_EXTENSION)
                || extension.eq_ignore_ascii_case("bmcbl-region")
        })
}

pub(super) fn export_region_package_blocking(
    world_path: &Path,
    source_anchor: ChunkPos,
    chunks: Vec<ChunkPos>,
    cancel: Option<&CancelFlag>,
    mut progress: impl FnMut(ChunkTransferProgress),
) -> Result<Vec<u8>, String> {
    let world = BedrockWorld::open_blocking(world_path, bedrock_world::OpenOptions::default())
        .map_err(|error| error.to_string())?;
    let editor = MapWorldEditor::from_world(world);
    let copied_chunk = copy_chunks_blocking(
        &editor,
        source_anchor,
        chunks,
        cancel,
        |transfer_progress| {
            progress(transfer_progress);
        },
    )
    .map_err(|error| error.to_string())?;
    drop(editor);

    let package = RegionPackageDisk::from_copied_chunk(&copied_chunk);
    let json =
        serde_json::to_vec(&package).map_err(|error| format!("区域包序列化失败：{error}"))?;
    let compressed = zstd::stream::encode_all(std::io::Cursor::new(json), 3)
        .map_err(|error| format!("区域包压缩失败：{error}"))?;
    let mut bytes = Vec::with_capacity(REGION_PACKAGE_MAGIC.len() + compressed.len());
    bytes.extend_from_slice(REGION_PACKAGE_MAGIC);
    bytes.extend_from_slice(&compressed);
    Ok(bytes)
}

pub(super) fn write_region_package(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let temp_path = path.with_extension(format!("{REGION_PACKAGE_EXTENSION}.tmp"));
    std::fs::write(&temp_path, bytes).map_err(|error| {
        format!(
            "写入区域包临时文件失败（{}）：{}",
            temp_path.display(),
            error
        )
    })?;

    match std::fs::rename(&temp_path, path) {
        Ok(()) => Ok(()),
        Err(_error) if path.exists() => {
            std::fs::remove_file(path)
                .map_err(|remove_error| format!("替换已有区域包失败：{remove_error}"))?;
            std::fs::rename(&temp_path, path)
                .map_err(|rename_error| format!("保存区域包失败：{rename_error}"))?;
            Ok(())
        }
        Err(error) => {
            if let Err(cleanup_error) = std::fs::remove_file(&temp_path) {
                tracing::debug!(
                    %cleanup_error,
                    path = %temp_path.display(),
                    "failed to remove temporary region package after rename failure"
                );
            }
            Err(format!("保存区域包失败：{error}"))
        }
    }
}

pub(super) fn read_region_package(path: &Path) -> Result<CopiedChunkData, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("读取区域包失败（{}）：{}", path.display(), error))?;
    if !bytes.starts_with(REGION_PACKAGE_MAGIC) {
        return Err("不是有效的 BMCBL 区域包".to_string());
    }
    let compressed = &bytes[REGION_PACKAGE_MAGIC.len()..];
    let json = zstd::stream::decode_all(std::io::Cursor::new(compressed))
        .map_err(|error| format!("区域包解压失败：{error}"))?;
    let package: RegionPackageDisk =
        serde_json::from_slice(&json).map_err(|error| format!("区域包解析失败：{error}"))?;
    package.into_copied_chunk()
}

fn nbt_string_field(nbt: &NbtTag, field: &str) -> Option<String> {
    let NbtTag::Compound(root) = nbt else {
        return None;
    };
    match root.get(field) {
        Some(NbtTag::String(value)) => Some(value.clone()),
        _ => None,
    }
}

fn nbt_i32_field(nbt: &NbtTag, field: &str) -> Option<i32> {
    let NbtTag::Compound(root) = nbt else {
        return None;
    };
    match root.get(field) {
        Some(NbtTag::Byte(value)) => Some(i32::from(*value)),
        Some(NbtTag::Short(value)) => Some(i32::from(*value)),
        Some(NbtTag::Int(value)) => Some(*value),
        Some(NbtTag::Long(value)) => i32::try_from(*value).ok(),
        _ => None,
    }
}

fn nbt_bool_field(nbt: &NbtTag, field: &str) -> Option<bool> {
    nbt_i32_field(nbt, field).map(|value| value != 0)
}

fn nbt_block_position(nbt: &NbtTag) -> Option<[i32; 3]> {
    Some([
        nbt_i32_field(nbt, "x")?,
        nbt_i32_field(nbt, "y")?,
        nbt_i32_field(nbt, "z")?,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn recognizes_region_package_extensions() {
        assert!(is_region_package_path(&PathBuf::from(
            "selection.bmcblregion"
        )));
        assert!(is_region_package_path(&PathBuf::from(
            "selection.bmcbl-region"
        )));
        assert!(!is_region_package_path(&PathBuf::from("selection.png")));
    }
}
