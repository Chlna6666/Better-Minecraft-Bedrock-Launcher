use super::model::{
    CopiedChunkData, CopiedChunkPreviewImage, CopiedChunkSnapshot, ImportedStructureData,
    MAP_OPERATION_CANCELLED_MESSAGE, PasteRotation, PasteTransform,
};
use super::prelude::*;
use super::selection::ChunkSelection;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(super) const MCSTRUCTURE_EXTENSION: &str = "mcstructure";

pub(super) struct McStructureImport {
    pub(super) copied_chunk: CopiedChunkData,
    pub(super) imported_structure: ImportedStructureData,
    pub(super) preview_images: BTreeMap<ChunkPos, CopiedChunkPreviewImage>,
    pub(super) size: bedrock_world::McStructureSize,
}

pub(super) fn is_mcstructure_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(MCSTRUCTURE_EXTENSION))
}

pub(super) fn default_mcstructure_file_name(selection: ChunkSelection, center_y: i32) -> String {
    let bounds = selection.bounds();
    let (min_y, max_y) = export_y_range(bounds.dimension, center_y);
    format!(
        "selection-{}-{}-{}-{}-y{}-{}.mcstructure",
        bounds.min_chunk_x,
        bounds.min_chunk_z,
        bounds.max_chunk_x,
        bounds.max_chunk_z,
        min_y,
        max_y
    )
}

pub(super) fn export_selection_mcstructure_blocking(
    world_path: &Path,
    bounds: SlimeChunkBounds,
    center_y: i32,
    output_path: &Path,
    cancel: Option<&CancelFlag>,
    mut progress: impl FnMut(ChunkTransferProgress),
) -> Result<PathBuf, String> {
    let chunk_count = bounds.chunk_count();
    if chunk_count == 0 {
        return Err("没有可导出的 chunk".to_string());
    }
    check_mcstructure_export_cancelled(cancel)?;
    let total = chunk_count.max(1);
    progress(ChunkTransferProgress {
        phase: SharedString::from("读取结构"),
        completed: 0,
        total,
    });

    let world = BedrockWorld::open_blocking(world_path, bedrock_world::OpenOptions::default())
        .map_err(|error| error.to_string())?;
    check_mcstructure_export_cancelled(cancel)?;
    let min_x = bounds.min_chunk_x.saturating_mul(16);
    let min_z = bounds.min_chunk_z.saturating_mul(16);
    let width = bounds
        .max_chunk_x
        .saturating_sub(bounds.min_chunk_x)
        .saturating_add(1)
        .saturating_mul(16);
    let depth = bounds
        .max_chunk_z
        .saturating_sub(bounds.min_chunk_z)
        .saturating_add(1)
        .saturating_mul(16);
    let (min_y, max_y) = export_y_range(bounds.dimension, center_y);
    let height = max_y.saturating_sub(min_y).saturating_add(1);
    let size = bedrock_world::McStructureSize::new(width, height, depth)
        .map_err(|error| format!("选区结构尺寸超过当前安全上限：{error}"))?;
    let structure = bedrock_world::McStructureFile::from_world_region_blocking(
        &world,
        bounds.dimension,
        min_x,
        min_y,
        min_z,
        size,
    )
    .map_err(|error| error.to_string())?;
    check_mcstructure_export_cancelled(cancel)?;
    progress(ChunkTransferProgress {
        phase: SharedString::from("写入结构"),
        completed: total.saturating_sub(1),
        total,
    });
    bedrock_world::write_mcstructure_file(output_path, &structure)
        .map_err(|error| error.to_string())?;
    check_mcstructure_export_cancelled(cancel)?;
    progress(ChunkTransferProgress {
        phase: SharedString::from("写入结构"),
        completed: total,
        total,
    });
    Ok(output_path.to_path_buf())
}

fn check_mcstructure_export_cancelled(cancel: Option<&CancelFlag>) -> Result<(), String> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err(MAP_OPERATION_CANCELLED_MESSAGE.to_string());
    }
    Ok(())
}

pub(super) fn export_y_range(dimension: Dimension, _center_y: i32) -> (i32, i32) {
    ChunkPos {
        x: 0,
        z: 0,
        dimension,
    }
    .y_range(bedrock_world::ChunkVersion::New)
}

pub(super) fn read_mcstructure_as_copied_chunk(
    path: &Path,
    anchor_chunk: ChunkPos,
    origin_y: i32,
) -> Result<McStructureImport, String> {
    let structure = bedrock_world::read_mcstructure_file(path)
        .map_err(|error| format!("读取 .mcstructure 失败（{}）：{}", path.display(), error))?;
    let copied_chunk = structure_to_copied_chunk(&structure, anchor_chunk)?;
    let preview_images =
        super::import_preview::mcstructure_preview_images(&structure, anchor_chunk)?;
    let size = structure.size;
    let imported_structure = ImportedStructureData {
        structure: Arc::new(structure),
        source_anchor: anchor_chunk,
        origin_y,
    };
    Ok(McStructureImport {
        size,
        copied_chunk,
        imported_structure,
        preview_images,
    })
}

fn structure_to_copied_chunk(
    structure: &bedrock_world::McStructureFile,
    anchor_chunk: ChunkPos,
) -> Result<CopiedChunkData, String> {
    let placement = bedrock_world::McStructurePlacement {
        source_anchor: anchor_chunk,
        target_anchor: anchor_chunk,
        origin_y: structure.world_origin[1],
        rotation: bedrock_world::McStructureRotation::None,
        mirror_x: false,
        mirror_z: false,
    };
    let targets = structure
        .target_chunks(placement)
        .map_err(|error| format!("结构目标 chunk 计算失败：{error}"))?;
    if targets.is_empty() {
        return Err("结构文件没有可导入的方块".to_string());
    }
    let mut chunks = Vec::with_capacity(targets.len());
    for chunk in targets {
        chunks.push(CopiedChunkSnapshot {
            chunk,
            records: Vec::new(),
            block_entities: Vec::new(),
            hardcoded_spawn_areas: Vec::new(),
        });
    }
    Ok(CopiedChunkData {
        source: anchor_chunk,
        chunks,
    })
}

pub(super) fn imported_structure_targets(
    import: &ImportedStructureData,
    target_anchor: ChunkPos,
    transform: PasteTransform,
) -> BTreeSet<ChunkPos> {
    import
        .structure
        .target_chunks(bedrock_world::McStructurePlacement {
            source_anchor: import.source_anchor,
            target_anchor,
            origin_y: import.origin_y,
            rotation: mcstructure_rotation(transform.rotation),
            mirror_x: transform.mirror_x,
            mirror_z: transform.mirror_z,
        })
        .unwrap_or_default()
}

pub(super) fn paste_imported_structure_blocking(
    world: &BedrockWorld,
    import: &ImportedStructureData,
    target_anchor: ChunkPos,
    transform: PasteTransform,
    guard: &WriteGuard,
    cancel: Option<&CancelFlag>,
    progress: &mut impl FnMut(ChunkTransferProgress),
) -> bedrock_world::Result<(String, MapEditInvalidation)> {
    check_mcstructure_paste_cancelled(cancel)?;
    let result = import.structure.write_to_world_blocking(
        world,
        bedrock_world::McStructurePlacement {
            source_anchor: import.source_anchor,
            target_anchor,
            origin_y: import.origin_y,
            rotation: mcstructure_rotation(transform.rotation),
            mirror_x: transform.mirror_x,
            mirror_z: transform.mirror_z,
        },
        guard,
        |write_progress| {
            progress(ChunkTransferProgress {
                phase: SharedString::from(match write_progress.phase {
                    bedrock_world::McStructureWritePhase::Prepare => "准备结构",
                    bedrock_world::McStructureWritePhase::WriteChunks => "粘贴结构",
                }),
                completed: write_progress.completed,
                total: write_progress.total,
            });
        },
    )?;
    check_mcstructure_paste_cancelled(cancel)?;

    Ok((
        format!(
            "已粘贴结构 {}x{}x{} 到 chunk {},{}（{} 个 chunk）",
            import.structure.size.x,
            import.structure.size.y,
            import.structure.size.z,
            target_anchor.x,
            target_anchor.z,
            result.affected_chunks.len()
        ),
        MapEditInvalidation::chunks(result.affected_chunks).with_metadata(),
    ))
}

pub(super) fn replace_transformed_chunk_seed(
    world: &BedrockWorld,
    snapshot: &CopiedChunkSnapshot,
    target_chunk: ChunkPos,
    transform: PasteTransform,
    guard: &WriteGuard,
) -> bedrock_world::Result<()> {
    guard.validate(world)?;
    let mut transaction = world.transaction();
    transaction.delete_chunk(target_chunk)?;
    for record in &snapshot.records {
        let Some(value) = transformed_chunk_seed_value(record, transform)? else {
            continue;
        };
        let mut key = record.key.clone();
        key.pos = target_chunk;
        transaction.put_raw_record(&key, value);
    }
    transaction.commit()
}

fn transformed_chunk_seed_value(
    record: &ChunkRecord,
    transform: PasteTransform,
) -> bedrock_world::Result<Option<Bytes>> {
    let value = match record.key.tag {
        ChunkRecordTag::Data3D => {
            let mut biome = Biome3d::parse(&record.value)?;
            transform_horizontal_values(&mut biome.height_map, transform)?;
            for storage in &mut biome.storages {
                transform_biome_storage(storage, transform)?;
            }
            Bytes::from(biome.encode()?)
        }
        ChunkRecordTag::Data2D | ChunkRecordTag::Data2DLegacy => {
            let mut biome = bedrock_world::Biome2d::parse(&record.value)?;
            transform_horizontal_values(&mut biome.height_map, transform)?;
            transform_horizontal_values(&mut biome.biomes, transform)?;
            Bytes::from(biome.encode()?)
        }
        ChunkRecordTag::Version | ChunkRecordTag::VersionOld | ChunkRecordTag::LegacyVersion => {
            record.value.clone()
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

fn transform_horizontal_values<T: Copy>(
    values: &mut Vec<T>,
    transform: PasteTransform,
) -> bedrock_world::Result<()> {
    if values.len() != 256 {
        return Err(bedrock_world::BedrockWorldError::Validation(format!(
            "horizontal chunk data must contain 256 values, got {}",
            values.len()
        )));
    }
    let source = values.clone();
    for source_z in 0..16_u8 {
        for source_x in 0..16_u8 {
            let (target_x, target_z) = transformed_local_xz(source_x, source_z, transform);
            values[usize::from(target_z) * 16 + usize::from(target_x)] =
                source[usize::from(source_z) * 16 + usize::from(source_x)];
        }
    }
    Ok(())
}

fn transform_biome_storage(
    storage: &mut bedrock_world::ParsedBiomeStorage,
    transform: PasteTransform,
) -> bedrock_world::Result<()> {
    let Some(indices) = storage.indices.as_mut() else {
        return Ok(());
    };
    if storage.y.is_none() {
        return transform_horizontal_values(indices, transform);
    }
    if indices.len() != 4096 {
        return Err(bedrock_world::BedrockWorldError::Validation(format!(
            "3D biome storage must contain 4096 indices, got {}",
            indices.len()
        )));
    }
    let source = indices.clone();
    for source_z in 0..16_u8 {
        for source_x in 0..16_u8 {
            let (target_x, target_z) = transformed_local_xz(source_x, source_z, transform);
            for local_y in 0..16_u8 {
                indices[bedrock_world::block_storage_index(target_x, local_y, target_z)] =
                    source[bedrock_world::block_storage_index(source_x, local_y, source_z)];
            }
        }
    }
    Ok(())
}

const fn transformed_local_xz(source_x: u8, source_z: u8, transform: PasteTransform) -> (u8, u8) {
    let source_x = if transform.mirror_x {
        15 - source_x
    } else {
        source_x
    };
    let source_z = if transform.mirror_z {
        15 - source_z
    } else {
        source_z
    };
    match transform.rotation {
        PasteRotation::NoRotation => (source_x, source_z),
        PasteRotation::Clockwise90 => (15 - source_z, source_x),
        PasteRotation::Rotate180 => (15 - source_x, 15 - source_z),
        PasteRotation::CounterClockwise90 => (source_z, 15 - source_x),
    }
}

fn check_mcstructure_paste_cancelled(cancel: Option<&CancelFlag>) -> bedrock_world::Result<()> {
    if cancel.is_some_and(CancelFlag::is_cancelled) {
        return Err(bedrock_world::BedrockWorldError::Validation(
            MAP_OPERATION_CANCELLED_MESSAGE.to_string(),
        ));
    }
    Ok(())
}

const fn mcstructure_rotation(rotation: PasteRotation) -> bedrock_world::McStructureRotation {
    match rotation {
        PasteRotation::NoRotation => bedrock_world::McStructureRotation::None,
        PasteRotation::Clockwise90 => bedrock_world::McStructureRotation::Clockwise90,
        PasteRotation::Rotate180 => bedrock_world::McStructureRotation::Rotate180,
        PasteRotation::CounterClockwise90 => bedrock_world::McStructureRotation::CounterClockwise90,
    }
}
