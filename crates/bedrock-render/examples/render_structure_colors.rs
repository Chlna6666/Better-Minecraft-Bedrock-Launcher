//! API-only conversion roundtrip: place a flat structure in memory and render its real pixels.
//! All target chunks are synthetic fixtures; no original Minecraft world is opened or modified.

use std::{error::Error, fs, path::PathBuf, sync::Arc};

use bedrock_render::{
    ImageFormat, MapRenderer, RenderBackend, RenderJob, RenderMode, RenderOptions, RenderPalette,
    RenderThreadingOptions, TileCoord,
};
use bedrock_world::{
    Biome3d, ChunkKey, ChunkPos, ChunkRecordTag, Dimension, OpenOptions, World, WorldStorage,
    WriteGuard,
    storage::MemoryStorage,
    structure::{McStructureFile, McStructurePlacement, McStructureRotation},
};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err("usage: render_structure_colors <square-flat.mcstructure> <output.png>".into());
    }
    let structure = McStructureFile::from_bytes(&fs::read(&arguments[0])?)?;
    if structure.size.y != 1 || structure.size.x != structure.size.z {
        return Err("this comparison requires a square flat structure".into());
    }
    let side = u32::try_from(structure.size.x)?;
    let origin = ChunkPos {
        x: 0,
        z: 0,
        dimension: Dimension::Overworld,
    };
    let placement = McStructurePlacement {
        source_anchor: origin,
        target_anchor: origin,
        origin_y: 64,
        rotation: McStructureRotation::None,
        mirror_x: false,
        mirror_z: false,
    };
    let storage = Arc::new(MemoryStorage::new());
    for chunk in structure.target_chunks(placement)? {
        storage.put(
            &ChunkKey::new(chunk, ChunkRecordTag::Data3D).encode(),
            &Biome3d::new(vec![63; 256], Vec::new())?.encode()?,
        )?;
    }
    let world = Arc::new(World::from_storage(
        "color-roundtrip-memory",
        storage,
        OpenOptions {
            read_only: false,
            ..OpenOptions::default()
        },
    ));
    let placed = structure.write_to_world(
        &world,
        placement,
        &WriteGuard::confirmed("color-roundtrip-memory", "fixture placement"),
        |_| {},
    )?;
    let renderer = MapRenderer::new(world, RenderPalette::default());
    let tile = renderer.render_tile(
        RenderJob {
            tile_size: side,
            ..RenderJob::new(
                TileCoord {
                    x: 0,
                    z: 0,
                    dimension: Dimension::Overworld,
                },
                RenderMode::SurfaceBlocks,
            )
        },
        &RenderOptions {
            format: ImageFormat::Rgba,
            backend: RenderBackend::Cpu,
            threading: RenderThreadingOptions::Single,
            ..RenderOptions::default()
        },
    )?;
    let output = PathBuf::from(&arguments[1]);
    image::save_buffer(&output, &tile.rgba, side, side, image::ColorType::Rgba8)?;
    println!(
        "rendered={}x{} placed_chunks={} output={}",
        side,
        side,
        placed.affected_chunks.len(),
        output.display()
    );
    Ok(())
}
