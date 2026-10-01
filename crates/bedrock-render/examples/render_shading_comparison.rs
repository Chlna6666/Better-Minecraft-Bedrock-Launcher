use bedrock_render::{
    AtlasRenderOptions, BlockBoundaryRenderOptions, BlockVolumeRenderOptions, ChunkRegion,
    ChunkTileLayout, MapRenderer, RenderMode, RenderOptions, RenderPalette, SurfaceRenderOptions,
    TerrainGradientAlgorithm, TerrainLightingOptions, TerrainShadingMode,
};
use bedrock_world::{Dimension, OpenOptions, World};
use image::{ColorType, ImageFormat, save_buffer_with_format};
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

const PIXELS_PER_BLOCK: u32 = 2;

const HEIGHT_ALGORITHMS: [(&str, TerrainGradientAlgorithm, TerrainShadingMode); 9] = [
    (
        "01-horn-directional",
        TerrainGradientAlgorithm::Horn,
        TerrainShadingMode::Directional,
    ),
    (
        "02-zt-directional",
        TerrainGradientAlgorithm::ZevenbergenThorne,
        TerrainShadingMode::Directional,
    ),
    (
        "03-scharr-directional",
        TerrainGradientAlgorithm::Scharr,
        TerrainShadingMode::Directional,
    ),
    (
        "04-horn-multidirectional",
        TerrainGradientAlgorithm::Horn,
        TerrainShadingMode::MultiDirectional,
    ),
    (
        "05-zt-multidirectional",
        TerrainGradientAlgorithm::ZevenbergenThorne,
        TerrainShadingMode::MultiDirectional,
    ),
    (
        "06-scharr-multidirectional",
        TerrainGradientAlgorithm::Scharr,
        TerrainShadingMode::MultiDirectional,
    ),
    (
        "07-horn-slope-weighted",
        TerrainGradientAlgorithm::Horn,
        TerrainShadingMode::SlopeWeighted,
    ),
    (
        "08-zt-slope-weighted",
        TerrainGradientAlgorithm::ZevenbergenThorne,
        TerrainShadingMode::SlopeWeighted,
    ),
    (
        "09-scharr-slope-weighted",
        TerrainGradientAlgorithm::Scharr,
        TerrainShadingMode::SlopeWeighted,
    ),
];

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 6 {
        return Err("usage: render_shading_comparison <world> <output> <min-chunk-x> <min-chunk-z> <max-chunk-x> <max-chunk-z>".into());
    }

    let world_path = PathBuf::from(&arguments[0]);
    let output_dir = PathBuf::from(&arguments[1]);
    let region = ChunkRegion {
        dimension: Dimension::Overworld,
        min_chunk_x: arguments[2].to_string_lossy().parse()?,
        min_chunk_z: arguments[3].to_string_lossy().parse()?,
        max_chunk_x: arguments[4].to_string_lossy().parse()?,
        max_chunk_z: arguments[5].to_string_lossy().parse()?,
    };

    std::fs::create_dir_all(&output_dir)?;
    let world = Arc::new(World::open(
        world_path,
        OpenOptions {
            read_only: true,
            ..OpenOptions::default()
        },
    )?);
    let renderer = MapRenderer::new(world, RenderPalette::default());
    let layout = ChunkTileLayout {
        chunks_per_tile: 16,
        blocks_per_pixel: 1,
        pixels_per_block: PIXELS_PER_BLOCK,
    };

    for (name, surface) in comparison_surfaces() {
        let mut options = RenderOptions::default();
        options.surface = surface;
        let tile_set =
            renderer.render_region_tiles(region, RenderMode::SurfaceBlocks, layout, options)?;
        for tile in tile_set.tiles {
            let path = output_dir.join(format!("{name}-{}-{}.png", tile.coord.x, tile.coord.z));
            save_buffer_with_format(
                path,
                &tile.rgba,
                tile.width,
                tile.height,
                ColorType::Rgba8,
                ImageFormat::Png,
            )?;
        }
    }
    Ok(())
}

fn comparison_surfaces() -> Vec<(&'static str, SurfaceRenderOptions)> {
    let mut surfaces = Vec::with_capacity(20);
    surfaces.push(("00-flat", flat_surface()));
    surfaces.extend(
        HEIGHT_ALGORITHMS.map(|(name, gradient, mode)| (name, terrain_surface(gradient, mode))),
    );
    surfaces.push(("10-block-boundaries-contact", boundary_surface()));
    surfaces.push(("11-block-faces-contact-cast", block_surface(0.55)));
    surfaces.push(("12-bedrockmaprender-drop-shadow", drop_shadow_surface()));
    let volume = block_volume_options(0.55);
    surfaces.push((
        "13-block-faces-no-cast",
        block_surface_options(BlockVolumeRenderOptions {
            cast_shadow_strength: 0.0,
            cast_shadow_max_blocks: 0,
            ..volume
        }),
    ));
    surfaces.push((
        "14-block-faces-short-cast",
        block_surface_options(BlockVolumeRenderOptions {
            cast_shadow_strength: 0.18,
            cast_shadow_max_blocks: 2,
            ..volume
        }),
    ));
    surfaces.push((
        "15-block-faces-gentle",
        block_surface_options(BlockVolumeRenderOptions {
            face_shadow_strength: 0.55,
            contact_shadow_strength: 0.35,
            cast_shadow_strength: 0.12,
            cast_shadow_max_blocks: 2,
            max_shadow: 28.0,
            height_threshold: 1.5,
            softness: 0.8,
            ..volume
        }),
    ));
    surfaces.push((
        "16-block-faces-only",
        block_surface_options(BlockVolumeRenderOptions {
            contact_shadow_strength: 0.0,
            cast_shadow_strength: 0.0,
            cast_shadow_max_blocks: 0,
            ..volume
        }),
    ));
    for (name, threshold) in [
        ("17-block-faces-threshold-2", 2.0),
        ("18-block-faces-threshold-4", 4.0),
        ("19-block-faces-threshold-6", 6.0),
    ] {
        surfaces.push((
            name,
            block_surface_options(BlockVolumeRenderOptions {
                contact_shadow_strength: 0.45,
                cast_shadow_strength: 0.0,
                cast_shadow_max_blocks: 0,
                height_threshold: threshold,
                ..volume
            }),
        ));
    }
    surfaces
}

fn flat_surface() -> SurfaceRenderOptions {
    SurfaceRenderOptions {
        height_shading: true,
        lighting: TerrainLightingOptions::off(),
        block_boundaries: BlockBoundaryRenderOptions::off(),
        block_volume: BlockVolumeRenderOptions::off(),
        atlas: disabled_atlas(),
        ..SurfaceRenderOptions::default()
    }
}

fn terrain_surface(
    gradient_algorithm: TerrainGradientAlgorithm,
    shading_mode: TerrainShadingMode,
) -> SurfaceRenderOptions {
    SurfaceRenderOptions {
        height_shading: true,
        lighting: TerrainLightingOptions {
            gradient_algorithm,
            shading_mode,
            normal_strength: 2.35,
            shadow_strength: 0.66,
            highlight_strength: 0.42,
            ambient_occlusion: 0.075,
            max_shadow: 50.0,
            land_slope_softness: 7.0,
            edge_relief_strength: 0.0,
            ..TerrainLightingOptions::soft()
        },
        block_boundaries: BlockBoundaryRenderOptions::off(),
        block_volume: BlockVolumeRenderOptions::off(),
        atlas: disabled_atlas(),
        ..SurfaceRenderOptions::default()
    }
}

fn block_surface(cast_shadow_strength: f32) -> SurfaceRenderOptions {
    block_surface_options(block_volume_options(cast_shadow_strength))
}

fn block_volume_options(cast_shadow_strength: f32) -> BlockVolumeRenderOptions {
    BlockVolumeRenderOptions {
        enabled: true,
        face_width_pixels: 1.0,
        face_shadow_strength: 0.9,
        contact_shadow_strength: 0.9,
        cast_shadow_strength,
        cast_shadow_max_blocks: 5,
        cast_shadow_height_scale: 0.15,
        highlight_strength: 0.22,
        max_shadow: 42.0,
        max_highlight: 16.0,
        height_threshold: 0.0,
        softness: 0.35,
    }
}

fn block_surface_options(block_volume: BlockVolumeRenderOptions) -> SurfaceRenderOptions {
    SurfaceRenderOptions {
        height_shading: true,
        lighting: TerrainLightingOptions::off(),
        block_boundaries: BlockBoundaryRenderOptions::off(),
        block_volume,
        atlas: disabled_atlas(),
        ..SurfaceRenderOptions::default()
    }
}

fn boundary_surface() -> SurfaceRenderOptions {
    SurfaceRenderOptions {
        height_shading: true,
        lighting: TerrainLightingOptions::off(),
        block_boundaries: BlockBoundaryRenderOptions {
            enabled: true,
            strength: 0.85,
            flat_strength: 0.15,
            height_threshold: 0.5,
            max_shadow: 38.0,
            highlight_strength: 0.24,
            softness: 0.25,
            line_width_pixels: 1.0,
        },
        block_volume: BlockVolumeRenderOptions::off(),
        atlas: disabled_atlas(),
        ..SurfaceRenderOptions::default()
    }
}

fn drop_shadow_surface() -> SurfaceRenderOptions {
    SurfaceRenderOptions {
        height_shading: true,
        lighting: TerrainLightingOptions {
            shading_mode: TerrainShadingMode::DirectionalDropShadow,
            ..TerrainLightingOptions::soft()
        },
        block_boundaries: BlockBoundaryRenderOptions::off(),
        block_volume: BlockVolumeRenderOptions::off(),
        atlas: disabled_atlas(),
        ..SurfaceRenderOptions::default()
    }
}

fn disabled_atlas() -> AtlasRenderOptions {
    AtlasRenderOptions {
        enabled: false,
        ..AtlasRenderOptions::default()
    }
}
