from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def patch(path: str, old: str, new: str, label: str) -> None:
    file = ROOT / path
    text = file.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise RuntimeError(f"{label}: expected exactly one match, got {count}")
    file.write_text(text.replace(old, new, 1), encoding="utf-8")


# bedrock-block-model: keep the generic 0..3 mapping for families that use it,
# but stairs must use Bedrock's dedicated weirdo_direction encoding.
patch(
    "crates/bedrock-block-model/src/model_family/direction.rs",
    "pub(super) fn block_face(state: &BlockStateQuery) -> Option<&str> {",
    '''pub(super) fn stair_direction(state: &BlockStateQuery) -> Option<CardinalDirection> {
    state_string(state, "minecraft:cardinal_direction")
        .and_then(cardinal_direction_from_string)
        .or_else(|| {
            state_string(state, "cardinal_direction").and_then(cardinal_direction_from_string)
        })
        .or_else(|| state_string(state, "facing").and_then(cardinal_direction_from_string))
        .or_else(|| state_string(state, "direction").and_then(cardinal_direction_from_string))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(stair_direction_from_int))
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_from_int))
}

pub(super) fn block_face(state: &BlockStateQuery) -> Option<&str> {''',
    "add stair-specific direction parser",
)
patch(
    "crates/bedrock-block-model/src/model_family/direction.rs",
    "fn facing_direction_from_int(value: i64) -> Option<CardinalDirection> {",
    '''fn stair_direction_from_int(value: i64) -> Option<CardinalDirection> {
    match value.rem_euclid(4) {
        0 => Some(CardinalDirection::East),
        1 => Some(CardinalDirection::West),
        2 => Some(CardinalDirection::South),
        3 => Some(CardinalDirection::North),
        _ => None,
    }
}

fn facing_direction_from_int(value: i64) -> Option<CardinalDirection> {''',
    "add weirdo_direction mapping",
)
patch(
    "crates/bedrock-block-model/src/model_family/building/stairs.rs",
    "    CardinalDirection, cardinal_direction, state_bool, state_string,\n",
    "    CardinalDirection, stair_direction, state_bool, state_string,\n",
    "import stair direction parser",
)
patch(
    "crates/bedrock-block-model/src/model_family/building/stairs.rs",
    "    let direction = cardinal_direction(state).unwrap_or(CardinalDirection::North);\n    let shape_name = stairs_shape_name(state).unwrap_or(\"straight\");",
    "    let direction = stair_direction(state).unwrap_or(CardinalDirection::North);\n    let shape_name = stairs_shape_name(state).unwrap_or(\"straight\");",
    "use stair-specific direction parser",
)

stairs_path = ROOT / "crates/bedrock-block-model/src/model_family/building/stairs.rs"
stairs = stairs_path.read_text(encoding="utf-8")
if "legacy_weirdo_direction_matches_bedrock_stairs_encoding" not in stairs:
    stairs += r'''

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_weirdo_direction_matches_bedrock_stairs_encoding() {
        let cases = [
            (0, [0.5, 0.5, 0.0], [1.0, 1.0, 1.0]),
            (1, [0.0, 0.5, 0.0], [0.5, 1.0, 1.0]),
            (2, [0.0, 0.5, 0.5], [1.0, 1.0, 1.0]),
            (3, [0.0, 0.5, 0.0], [1.0, 1.0, 0.5]),
        ];

        for (direction, expected_min, expected_max) in cases {
            let state = BlockStateQuery::new("minecraft:oak_stairs")
                .with_state("weirdo_direction", direction)
                .with_state("upside_down_bit", false);
            let shape = shape(&state);
            assert_eq!(shape.cuboids.len(), 2);
            assert_eq!(shape.cuboids[1].min, expected_min, "direction={direction}");
            assert_eq!(shape.cuboids[1].max, expected_max, "direction={direction}");
        }
    }
}
'''
    stairs_path.write_text(stairs, encoding="utf-8")

# Java model selector mapping must use the same semantic direction as the geometry.
patch(
    "crates/bedrock-block-model/src/java.rs",
    '''        ModelFamily::Stairs => {
            if let Some(direction) = state_i64(state, "weirdo_direction")
                .and_then(cardinal_direction_0_3)
                .or_else(|| bedrock_cardinal_direction(state))
            {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
''',
    '''        ModelFamily::Stairs => {
            if let Some(direction) = bedrock_stair_direction(state) {
                properties.insert("facing".to_owned(), direction.to_owned());
            }
''',
    "use stair-specific Java direction mapping",
)
patch(
    "crates/bedrock-block-model/src/java.rs",
    "fn cardinal_direction_string(value: &str) -> Option<&'static str> {",
    '''fn bedrock_stair_direction(state: &BlockStateQuery) -> Option<&'static str> {
    state_string(state, "cardinal_direction")
        .and_then(cardinal_direction_string)
        .or_else(|| state_string(state, "facing").and_then(cardinal_direction_string))
        .or_else(|| state_string(state, "direction").and_then(cardinal_direction_string))
        .or_else(|| state_i64(state, "weirdo_direction").and_then(stair_direction_0_3))
        .or_else(|| state_i64(state, "direction").and_then(cardinal_direction_0_3))
}

fn cardinal_direction_string(value: &str) -> Option<&'static str> {''',
    "add Java stair direction parser",
)
patch(
    "crates/bedrock-block-model/src/java.rs",
    "fn trapdoor_direction(value: i64) -> Option<&'static str> {",
    '''fn stair_direction_0_3(value: i64) -> Option<&'static str> {
    match value.rem_euclid(4) {
        0 => Some("east"),
        1 => Some("west"),
        2 => Some("south"),
        3 => Some("north"),
        _ => None,
    }
}

fn trapdoor_direction(value: i64) -> Option<&'static str> {''',
    "add Java weirdo_direction mapping",
)
patch(
    "crates/bedrock-block-model/src/java.rs",
    "    #[test]\n    fn java_variant_selector_matches_property_sets() {",
    '''    #[test]
    fn stairs_weirdo_direction_matches_bedrock_encoding() {
        let cases = [(0, "east"), (1, "west"), (2, "south"), (3, "north")];
        for (direction, expected) in cases {
            let stairs = BlockStateQuery::new("minecraft:oak_stairs")
                .with_state("weirdo_direction", direction)
                .with_state("upside_down_bit", false);
            assert_eq!(
                java_properties_for_bedrock_state(&stairs)
                    .get("facing")
                    .map(String::as_str),
                Some(expected),
                "weirdo_direction={direction}",
            );
        }
    }

    #[test]
    fn java_variant_selector_matches_property_sets() {''',
    "add Java stairs direction regression test",
)

# bedrock-world typed state API must not reuse the door-specific 0..3 encoding.
patch(
    "crates/bedrock-world/src/block/block_state/properties.rs",
    '''            direction: parse_cardinal_integer(
                "weirdo_direction",
                required_state(self, "weirdo_direction")?,
            )?,''',
    '''            direction: parse_stair_integer(
                "weirdo_direction",
                required_state(self, "weirdo_direction")?,
            )?,''',
    "use typed stair parser",
)
patch(
    "crates/bedrock-world/src/block/block_state/properties.rs",
    "fn parse_trapdoor_integer(key: &'static str, value: &NbtTag) -> Result<HorizontalDirection> {",
    '''fn parse_stair_integer(key: &'static str, value: &NbtTag) -> Result<HorizontalDirection> {
    match integer(value) {
        Some(0) => Ok(HorizontalDirection::East),
        Some(1) => Ok(HorizontalDirection::West),
        Some(2) => Ok(HorizontalDirection::South),
        Some(3) => Ok(HorizontalDirection::North),
        _ => Err(invalid_state(key, "expected stair direction 0..=3")),
    }
}

fn parse_trapdoor_integer(key: &'static str, value: &NbtTag) -> Result<HorizontalDirection> {''',
    "add typed stair direction parser",
)
patch(
    "crates/bedrock-world/src/block/block_state/properties.rs",
    "                direction: HorizontalDirection::North,\n                upside_down: true,\n                corner: Some(StairCorner::InnerLeft),",
    "                direction: HorizontalDirection::South,\n                upside_down: true,\n                corner: Some(StairCorner::InnerLeft),",
    "correct existing typed stair expectation",
)
patch(
    "crates/bedrock-world/src/block/block_state/properties.rs",
    "    #[test]\n    fn reads_stair_slab_and_arbitrary_future_states() {",
    '''    #[test]
    fn legacy_stair_direction_uses_weirdo_direction_encoding() {
        let cases = [
            (0, HorizontalDirection::East),
            (1, HorizontalDirection::West),
            (2, HorizontalDirection::South),
            (3, HorizontalDirection::North),
        ];
        for (direction, expected) in cases {
            let stairs = block(
                "minecraft:oak_stairs",
                [
                    ("weirdo_direction", NbtTag::Int(direction)),
                    ("upside_down_bit", NbtTag::Byte(0)),
                ],
            );
            assert_eq!(
                stairs.stair_states().unwrap().unwrap().direction,
                expected,
                "weirdo_direction={direction}",
            );
        }
    }

    #[test]
    fn reads_stair_slab_and_arbitrary_future_states() {''',
    "add typed stairs direction regression test",
)
