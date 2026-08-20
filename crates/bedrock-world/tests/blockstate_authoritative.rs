use bedrock_world::block::{
    AuthoritativeBlockStateCatalog, BlockState, BlockStateSchemaSource, BlockStateStorageVersion,
};
use bedrock_world::nbt::NbtTag;
use std::collections::BTreeMap;

const SCHEMA_0011: &str = include_str!("fixtures/blockstate-schema/0011_1.10.0_to_1.12.0.json");
const SCHEMA_0121: &str =
    include_str!("fixtures/blockstate-schema/0121_1.18.10_to_1.18.20.27_beta.json");
const SCHEMA_0131: &str =
    include_str!("fixtures/blockstate-schema/0131_1.18.20.27_beta_to_1.18.30.json");

#[test]
fn real_schema_applies_added_property_and_indexed_value_remap() {
    let catalog = AuthoritativeBlockStateCatalog::from_sources(&[BlockStateSchemaSource {
        name: "0011_1.10.0_to_1.12.0.json",
        json: SCHEMA_0011,
    }])
    .expect("load real PMMP schema 0011");
    let input = BlockState {
        name: "minecraft:barrel".to_string(),
        states: BTreeMap::from([("facing_direction".to_string(), NbtTag::Int(6))]),
        version: Some(BlockStateStorageVersion::from_components(1, 10, 0, 0).raw()),
    };
    let output = catalog.upgrade(&input).expect("upgrade barrel");
    assert_eq!(output.states.get("open_bit"), Some(&NbtTag::Byte(0)));
    assert_eq!(output.states.get("facing_direction"), Some(&NbtTag::Int(0)));
}

#[test]
fn real_same_version_schema_group_is_not_skipped() {
    let catalog = AuthoritativeBlockStateCatalog::from_sources(&[
        BlockStateSchemaSource {
            name: "0121_1.18.10_to_1.18.20.27_beta.json",
            json: SCHEMA_0121,
        },
        BlockStateSchemaSource {
            name: "0131_1.18.20.27_beta_to_1.18.30.json",
            json: SCHEMA_0131,
        },
    ])
    .expect("load same-version PMMP schemas");
    let version = BlockStateStorageVersion::from_components(1, 18, 10, 1).raw();
    let input = BlockState {
        name: "minecraft:invisibleBedrock".to_string(),
        states: BTreeMap::new(),
        version: Some(version),
    };
    let output = catalog.upgrade(&input).expect("upgrade same-version state");
    assert_eq!(catalog.schema_count(), 2);
    assert_eq!(catalog.version_group_count(), 1);
    assert_eq!(catalog.output_version().raw(), version);
    assert_eq!(output.name, "minecraft:invisible_bedrock");
}
