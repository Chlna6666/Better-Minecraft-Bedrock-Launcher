use super::model::{
    CopiedChunkData, CopiedChunkPreviewImage, CopiedChunkSnapshot, ImportedStructureData,
    PasteRotation, PasteTransform,
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
    mut progress: impl FnMut(ChunkTransferProgress),
) -> Result<PathBuf, String> {
    let chunk_count = bounds.chunk_count();
    if chunk_count == 0 {
        return Err("没有可导出的 chunk".to_string());
    }
    let total = chunk_count.max(1);
    progress(ChunkTransferProgress {
        phase: SharedString::from("读取结构"),
        completed: 0,
        total,
    });

    let world = BedrockWorld::open_blocking(world_path, bedrock_world::OpenOptions::default())
        .map_err(|error| error.to_string())?;
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
    progress(ChunkTransferProgress {
        phase: SharedString::from("写入结构"),
        completed: total.saturating_sub(1),
        total,
    });
    bedrock_world::write_mcstructure_file(output_path, &structure)
        .map_err(|error| error.to_string())?;
    progress(ChunkTransferProgress {
        phase: SharedString::from("写入结构"),
        completed: total,
        total,
    });
    Ok(output_path.to_path_buf())
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
        if copied_chunk.chunk_count() <= super::import_preview::PREVIEW_IMAGE_CHUNK_LIMIT {
            super::import_preview::mcstructure_preview_images(&structure, anchor_chunk)?
        } else {
            BTreeMap::new()
        };
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
    progress: &mut impl FnMut(ChunkTransferProgress),
) -> bedrock_world::Result<(String, MapEditInvalidation)> {
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

const fn mcstructure_rotation(rotation: PasteRotation) -> bedrock_world::McStructureRotation {
    match rotation {
        PasteRotation::NoRotation => bedrock_world::McStructureRotation::None,
        PasteRotation::Clockwise90 => bedrock_world::McStructureRotation::Clockwise90,
        PasteRotation::Rotate180 => bedrock_world::McStructureRotation::Rotate180,
        PasteRotation::CounterClockwise90 => bedrock_world::McStructureRotation::CounterClockwise90,
    }
}
