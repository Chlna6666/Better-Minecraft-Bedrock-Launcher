use std::fs;
use std::path::Path;

use bedrock_block_model::{BlockFace, BlockModelRepository, BlockStateQuery};
use tempfile::TempDir;

#[test]
fn resolver_should_resolve_legacy_blocks_json_face_textures() {
    let pack = TestPack::new();
    pack.write(
        "textures/terrain_texture.json",
        r#"{
            "texture_data": {
                "grass_top": { "textures": "textures/blocks/grass_top" },
                "dirt": { "textures": "textures/blocks/dirt" },
                "grass_side": { "textures": "textures/blocks/grass_side" }
            }
        }"#,
    );
    pack.write(
        "blocks.json",
        r#"{
            "minecraft:grass_block": {
                "textures": {
                    "up": "grass_top",
                    "down": "dirt",
                    "side": "grass_side"
                }
            }
        }"#,
    );

    let repository = must_ok(BlockModelRepository::load_packs([pack.path()]));
    let resolved = repository.resolve_block(&BlockStateQuery::new("minecraft:grass_block"));

    assert_eq!(
        resolved.face_textures[&BlockFace::Up].path.as_deref(),
        Some("textures/blocks/grass_top")
    );
    assert_eq!(
        resolved.face_textures[&BlockFace::North].path.as_deref(),
        Some("textures/blocks/grass_side")
    );
}

#[test]
fn resolver_should_apply_matching_permutation_material_instances() {
    let pack = TestPack::new();
    pack.write(
        "textures/terrain_texture.json",
        r#"{
            "texture_data": {
                "trapdoor_closed": { "textures": "textures/blocks/trapdoor_closed" },
                "trapdoor_open": { "textures": "textures/blocks/trapdoor_open" }
            }
        }"#,
    );
    pack.write(
        "blocks/trapdoor.json",
        r#"{
            "minecraft:block": {
                "description": { "identifier": "example:trapdoor" },
                "components": {
                    "minecraft:geometry": "geometry.example.trapdoor_closed",
                    "minecraft:material_instances": {
                        "*": {
                            "texture": "trapdoor_closed",
                            "render_method": "alpha_test"
                        }
                    }
                },
                "permutations": [{
                    "condition": "q.block_state('open_bit')",
                    "components": {
                        "minecraft:geometry": "geometry.example.trapdoor_open",
                        "minecraft:material_instances": {
                            "*": {
                                "texture": "trapdoor_open",
                                "render_method": "alpha_test"
                            }
                        }
                    }
                }]
            }
        }"#,
    );
    pack.write(
        "models/blocks/trapdoor.json",
        r#"{
            "minecraft:geometry": [{
                "description": { "identifier": "geometry.example.trapdoor_open" },
                "bones": [{ "name": "root", "cubes": [{ "origin": [0, 0, 0], "size": [16, 3, 16] }] }]
            }]
        }"#,
    );

    let repository = must_ok(BlockModelRepository::load_packs([pack.path()]));
    let resolved = repository
        .resolve_block(&BlockStateQuery::new("example:trapdoor").with_state("open_bit", true));

    assert_eq!(
        resolved.geometry_identifier.as_deref(),
        Some("geometry.example.trapdoor_open")
    );
    assert_eq!(
        resolved.materials["*"].texture_path.as_deref(),
        Some("textures/blocks/trapdoor_open")
    );
    assert!(resolved.geometry.is_some());
}

#[test]
fn resolver_should_not_load_item_texture_json_for_block_materials() {
    let pack = TestPack::new();
    pack.write(
        "textures/terrain_texture.json",
        r#"{
            "texture_data": {
                "bricks": { "textures": "textures/blocks/brick_block" }
            }
        }"#,
    );
    pack.write(
        "textures/item_texture.json",
        r#"{
            "texture_data": {
                "bricks": { "textures": "textures/items/brick" }
            }
        }"#,
    );
    pack.write(
        "blocks.json",
        r#"{
            "minecraft:brick_block": { "textures": "bricks" }
        }"#,
    );

    let repository = must_ok(BlockModelRepository::load_packs([pack.path()]));
    let resolved = repository.resolve_block(&BlockStateQuery::new("minecraft:brick_block"));

    assert_eq!(
        resolved.materials["*"].texture_path.as_deref(),
        Some("textures/blocks/brick_block")
    );
}

struct TestPack {
    directory: TempDir,
}

impl TestPack {
    fn new() -> Self {
        Self {
            directory: must_ok(TempDir::new()),
        }
    }

    fn path(&self) -> &Path {
        self.directory.path()
    }

    fn write(&self, relative_path: &str, content: &str) {
        let path = self.path().join(relative_path);
        if let Some(parent) = path.parent() {
            must_ok(fs::create_dir_all(parent));
        }
        must_ok(fs::write(path, content));
    }
}

fn must_ok<T, E: std::fmt::Debug>(result: std::result::Result<T, E>) -> T {
    match result {
        Ok(value) => value,
        Err(error) => panic!("unexpected error: {error:?}"),
    }
}
