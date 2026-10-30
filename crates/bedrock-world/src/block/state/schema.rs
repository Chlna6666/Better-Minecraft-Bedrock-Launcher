//! Complete persisted block-state schema for the supported Bedrock block set.
//!
//! The catalogue is generated from a world palette enumeration of every block permutation and
//! merged with the vanilla block list, so unknown block ids and out-of-domain state values can be
//! rejected instead of silently dropped. Fluid ids and the fluid level property are exposed here.

use crate::block::BlockState;
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

/// One property of a block permutation.
#[derive(Debug, Deserialize)]
pub struct StatePropertySchema {
    #[serde(rename = "type")]
    kind: String,
    values: Vec<serde_json::Value>,
    value_count: usize,
    #[serde(default)]
    expanded: bool,
}

impl StatePropertySchema {
    /// Returns the persisted NBT type name, for example Int or String.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// Returns every accepted value for this property.
    #[must_use]
    pub fn values(&self) -> &[serde_json::Value] {
        &self.values
    }

    /// Returns how many accepted values this property has.
    #[must_use]
    pub fn value_count(&self) -> usize {
        self.value_count
    }

    /// Reports whether the value domain was widened beyond the enumerated world sample.
    #[must_use]
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Reports whether one NBT value is accepted by this property.
    #[must_use]
    pub fn accepts(&self, value: &NbtTag) -> bool {
        self.values
            .iter()
            .any(|allowed| json_matches_tag(allowed, value))
    }
}

/// The state schema of one block id.
#[derive(Debug, Deserialize)]
pub struct BlockSchema {
    permutations: usize,
    states: BTreeMap<String, StatePropertySchema>,
}

impl BlockSchema {
    /// Returns how many permutations were observed for this block id.
    #[must_use]
    pub fn permutations(&self) -> usize {
        self.permutations
    }

    /// Returns every state property declared for this block id.
    #[must_use]
    pub fn states(&self) -> &BTreeMap<String, StatePropertySchema> {
        &self.states
    }

    /// Returns one state property by its persisted key.
    #[must_use]
    pub fn property(&self, key: &str) -> Option<&StatePropertySchema> {
        self.states.get(key)
    }
}

/// Complete block-state catalogue for the supported Bedrock version.
#[derive(Debug, Deserialize)]
pub struct BlockStateSchema {
    format_version: u32,
    block_count: usize,
    observed_permutations: usize,
    fluids: Vec<String>,
    blocks: BTreeMap<String, BlockSchema>,
}

impl BlockStateSchema {
    /// Returns the catalogue format version.
    #[must_use]
    pub fn format_version(&self) -> u32 {
        self.format_version
    }

    /// Returns the number of block ids in the catalogue.
    #[must_use]
    pub fn block_count(&self) -> usize {
        self.block_count
    }

    /// Returns how many block permutations were observed while generating the catalogue.
    #[must_use]
    pub fn observed_permutations(&self) -> usize {
        self.observed_permutations
    }

    /// Returns every fluid block id.
    #[must_use]
    pub fn fluids(&self) -> &[String] {
        &self.fluids
    }

    /// Returns every block id and its schema.
    #[must_use]
    pub fn blocks(&self) -> &BTreeMap<String, BlockSchema> {
        &self.blocks
    }

    /// Returns the schema of one block id.
    #[must_use]
    pub fn block(&self, name: &str) -> Option<&BlockSchema> {
        self.blocks.get(name)
    }
}

static SCHEMA: OnceLock<BlockStateSchema> = OnceLock::new();

/// Returns the embedded, lazily parsed block-state catalogue.
#[must_use]
pub fn block_state_schema() -> &'static BlockStateSchema {
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!("block_state_schema.json"))
            .expect("embedded block state schema must be valid")
    })
}

/// Returns the schema of one block id, or None when the id is unknown.
#[must_use]
pub fn block_schema(name: &str) -> Option<&'static BlockSchema> {
    block_state_schema().block(name)
}

/// Reports whether a block id is a fluid.
#[must_use]
pub fn is_fluid(name: &str) -> bool {
    block_state_schema()
        .fluids()
        .iter()
        .any(|fluid| fluid == name)
}

/// Returns the persisted fluid level of a fluid block, when it stores liquid_depth.
#[must_use]
pub fn fluid_level(state: &BlockState) -> Option<u8> {
    let depth = match state.state("liquid_depth")? {
        NbtTag::Byte(value) => i64::from(*value),
        NbtTag::Short(value) => i64::from(*value),
        NbtTag::Int(value) => i64::from(*value),
        NbtTag::Long(value) => *value,
        _ => return None,
    };
    u8::try_from(depth).ok()
}

/// Validates every persisted state property of one block against the catalogue.
///
/// Unknown block ids, unknown property keys and out-of-domain values are rejected. Missing keys are
/// accepted because palette records may omit properties that equal their default value.
///
/// # Errors
///
/// Returns [`BedrockWorldError::Validation`] for the first unknown key or out-of-domain value.
pub fn validate_state(state: &BlockState) -> Result<()> {
    let Some(schema) = block_schema(&state.name) else {
        return Err(BedrockWorldError::Validation(format!(
            "unknown block id {}",
            state.name
        )));
    };
    for (key, value) in &state.states {
        let Some(property) = schema.property(key) else {
            return Err(BedrockWorldError::Validation(format!(
                "block {} has no state property {key}",
                state.name
            )));
        };
        if !property.accepts(value) {
            return Err(BedrockWorldError::Validation(format!(
                "block {} state {key} rejects value {value:?}",
                state.name
            )));
        }
    }
    Ok(())
}

fn json_matches_tag(allowed: &serde_json::Value, value: &NbtTag) -> bool {
    match (allowed, value) {
        (serde_json::Value::Number(number), NbtTag::Byte(value)) => {
            number.as_i64() == Some(i64::from(*value))
        }
        (serde_json::Value::Number(number), NbtTag::Short(value)) => {
            number.as_i64() == Some(i64::from(*value))
        }
        (serde_json::Value::Number(number), NbtTag::Int(value)) => {
            number.as_i64() == Some(i64::from(*value))
        }
        (serde_json::Value::Number(number), NbtTag::Long(value)) => number.as_i64() == Some(*value),
        (serde_json::Value::Number(number), NbtTag::Float(value)) => {
            number.as_f64() == Some(f64::from(*value))
        }
        (serde_json::Value::Number(number), NbtTag::Double(value)) => {
            number.as_f64() == Some(*value)
        }
        (serde_json::Value::String(text), NbtTag::String(value)) => text == value,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_lists_the_supported_blocks() {
        let schema = block_state_schema();
        assert!(schema.block_count() >= 1_200, "{}", schema.block_count());
        assert_eq!(schema.block_count(), schema.blocks().len());
        assert!(schema.observed_permutations() >= 10_000);
    }

    #[test]
    fn water_is_a_fluid_with_the_full_depth_range() {
        assert!(is_fluid("minecraft:water"));
        let schema = block_schema("minecraft:water").expect("water schema");
        let depth = schema.property("liquid_depth").expect("liquid_depth");
        assert_eq!(depth.value_count(), 16);
        assert!(depth.is_expanded());
    }

    #[test]
    fn stairs_expose_direction_and_upside_down_values() {
        let schema = block_schema("minecraft:oak_stairs").expect("oak stairs schema");
        let direction = schema
            .property("weirdo_direction")
            .expect("weirdo_direction");
        assert_eq!(direction.value_count(), 4);
        let upside_down = schema.property("upside_down_bit").expect("upside_down_bit");
        assert_eq!(upside_down.value_count(), 2);
    }

    #[test]
    fn validation_rejects_out_of_domain_values() {
        let mut state = BlockState {
            name: "minecraft:oak_stairs".to_string(),
            states: BTreeMap::new(),
            version: Some(1),
        };
        state
            .states
            .insert("weirdo_direction".to_string(), NbtTag::Int(9));
        assert!(validate_state(&state).is_err());

        state
            .states
            .insert("weirdo_direction".to_string(), NbtTag::Int(3));
        assert!(validate_state(&state).is_ok());
    }
}
