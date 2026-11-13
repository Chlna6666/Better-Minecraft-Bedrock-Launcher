//! Bedrock resource-pack block model resolution.
//!
//! The resolver keeps resource-pack parsing separate from mesh generation. It reads block,
//! terrain texture, material instance, permutation, and geometry JSON data, then returns a
//! `ResolvedBlockModel` suitable for a renderer or OBJ exporter to consume.

mod error;
pub mod geometry;
mod geometry_shape;
pub mod item_icons;
mod json;
mod lighting;
mod material;
pub use lighting::BlockLightProperties;
pub mod model_family;
pub mod neighbor;
pub mod obj;
mod pack;
mod permutation;
mod resolver;
mod resource;
pub mod resource_pack;
pub mod runtime_capture;
mod state;
mod texture;
mod voxel_shape;

pub use error::{BlockModelError, Result};
pub use geometry::{BlockGeometry, GeometryBone, GeometryCube, GeometryLibrary, GeometryUv};
pub use item_icons::ItemEntry;
pub use material::{
    BlockComponents, BlockFace, BlockTransformation, MaterialInstance, TextureReference, TextureSet,
};
pub use model_family::{ModelCuboid, ModelFamily, ModelPlane, ModelShape};
pub use neighbor::{
    DerivedModelVariant, HorizontalDirection, HorizontalMask, NeighborBlockDescriptor,
    NeighborModelKind, RedstoneConnection, StairShape, WallConnection,
};
pub use obj::{
    FALLBACK_MATERIAL_NAME, LocalLightDefinition, MATERIAL_SLOT_SEPARATOR, NamedObjMaterial,
    ObjDocumentHeader, ObjExport, ObjExportMaterial, ObjExportTarget, ObjExportWriteSummary,
    ObjFace, ObjMaterial, ObjMaterialSample, ObjMeshFace, ObjMeshFaceSource,
    ObjResolvedPbrTextureSet, ObjResolvedTexture, ObjTextureCopy, ObjTextureResolver,
    PbrTextureLayer,
};
pub use pack::{BlockDefinition, BlockModelRepository};
pub use permutation::{BlockPermutation, ConditionEvaluation};
pub use resolver::{ModelWarning, ResolvedBlockModel, ResolvedMaterialInstance, ResolvedTexture};
pub use resource::ResourceFile;
pub use runtime_capture::RuntimeCaptureCatalog;
pub use state::{BlockStateQuery, BlockStateValue};
pub use texture::{TerrainTexture, TerrainTextureAtlas};
pub use voxel_shape::{
    VoxelBox, VoxelFaceCoverage, VoxelShape, VoxelShapeLibrary, vanilla_voxel_shapes,
};

#[cfg(test)]
mod native_fallback_tests {
    use super::*;

    #[test]
    fn double_plant_slots_follow_bedrock_half_state() {
        for (upper, expected) in [(false, "down"), (true, "up")] {
            let state =
                BlockStateQuery::new("minecraft:sunflower").with_state("upper_block_bit", upper);
            let mut shape = ModelShape::default()
                .with_planes([ModelPlane::new([[0.0, 0.0, 0.0]; 4], [0, 1, 0])]);
            model_family::apply_double_plant_material_slots(&state, &mut shape);
            assert_eq!(shape.planes[0].material_slot.as_deref(), Some(expected));
        }
    }

    #[test]
    fn non_plant_blocks_keep_original_material_slots() {
        let state = BlockStateQuery::new("minecraft:stone");
        let mut shape =
            ModelShape::default().with_planes([
                ModelPlane::new([[0.0, 0.0, 0.0]; 4], [0, 1, 0]).with_material_slot("original")
            ]);
        model_family::apply_double_plant_material_slots(&state, &mut shape);
        assert_eq!(shape.planes[0].material_slot.as_deref(), Some("original"));
    }

    #[test]
    fn every_family_shape_is_uv_complete() {
        // Every element must carry UVs so no consumer has to invent a fallback. This covers
        // one representative state per model family, including thin elements that previously
        // sampled a cropped strip of their texture.
        const SAMPLES: &[&str] = &[
            "minecraft:stone_slab",
            "minecraft:oak_stairs",
            "minecraft:ladder",
            "minecraft:oak_fence",
            "minecraft:oak_fence_gate",
            "minecraft:cobblestone_wall",
            "minecraft:glass_pane",
            "minecraft:iron_bars",
            "minecraft:oak_trapdoor",
            "minecraft:oak_door",
            "minecraft:white_carpet",
            "minecraft:rail",
            "minecraft:redstone_wire",
            "minecraft:poppy",
            "minecraft:cactus",
            "minecraft:bamboo",
            "minecraft:cocoa",
            "minecraft:azalea",
            "minecraft:vine",
            "minecraft:fire",
            "minecraft:water",
            "minecraft:chorus_plant",
            "minecraft:pointed_dripstone",
            "minecraft:bubble_column",
            "minecraft:trip_wire",
            "minecraft:sculk_sensor",
            "minecraft:spore_blossom",
            "minecraft:big_dripleaf",
            "minecraft:mangrove_propagule",
            "minecraft:farmland",
            "minecraft:torch",
            "minecraft:lantern",
            "minecraft:candle",
            "minecraft:end_rod",
            "minecraft:lightning_rod",
            "minecraft:chain",
            "minecraft:campfire",
            "minecraft:bell",
            "minecraft:enchanting_table",
            "minecraft:flower_pot",
            "minecraft:decorated_pot",
            "minecraft:standing_banner",
            "minecraft:piston",
            "minecraft:piston_arm_collision",
            "minecraft:observer",
            "minecraft:dispenser",
            "minecraft:unpowered_repeater",
            "minecraft:unpowered_comparator",
            "minecraft:daylight_detector",
            "minecraft:stone_button",
            "minecraft:stone_pressure_plate",
            "minecraft:lever",
            "minecraft:chest",
            "minecraft:ender_chest",
            "minecraft:barrel",
            "minecraft:blue_shulker_box",
            "minecraft:furnace",
            "minecraft:hopper",
            "minecraft:cauldron",
            "minecraft:composter",
            "minecraft:anvil",
            "minecraft:chiseled_bookshelf",
            "minecraft:stonecutter_block",
            "minecraft:grindstone",
            "minecraft:lectern",
            "minecraft:brewing_stand",
            "minecraft:beacon",
            "minecraft:conduit",
            "minecraft:heavy_core",
            "minecraft:bed",
            "minecraft:oak_shelf",
            "minecraft:cake",
            "minecraft:scaffolding",
            "minecraft:sea_pickle",
            "minecraft:turtle_egg",
            "minecraft:dragon_egg",
            "minecraft:end_portal_frame",
            "minecraft:oak_standing_sign",
            "minecraft:creeper_head",
            "minecraft:skeleton_skull",
            "minecraft:dragon_head",
            "minecraft:piglin_head",
            "minecraft:end_portal",
            "minecraft:nether_portal",
            "minecraft:dried_ghast",
            "minecraft:copper_golem_statue",
        ];
        for name in SAMPLES {
            let state = BlockStateQuery::new(*name)
                .with_state("facing_direction", 3)
                .with_state("minecraft:cardinal_direction", "south");
            let Some(shape) = model_family::resolve_shape(&state) else {
                panic!("{name}: no procedural shape");
            };
            assert!(
                !shape.cuboids.is_empty() || !shape.planes.is_empty(),
                "{name}: empty shape"
            );
            for cuboid in &shape.cuboids {
                assert_eq!(
                    cuboid.face_uvs.len(),
                    6,
                    "{name}: cuboid {:?}..{:?} is missing UVs",
                    cuboid.min,
                    cuboid.max
                );
            }
            for plane in &shape.planes {
                assert!(plane.uv.is_some(), "{name}: plane without UV");
            }
        }
    }
}
