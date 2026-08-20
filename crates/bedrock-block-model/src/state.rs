use std::collections::BTreeMap;

use bedrock_world::{BlockState, NbtTag};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlockStateQuery {
    pub name: String,
    pub states: BTreeMap<String, BlockStateValue>,
}

impl BlockStateQuery {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            states: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_state(
        mut self,
        name: impl Into<String>,
        value: impl Into<BlockStateValue>,
    ) -> Self {
        self.states.insert(name.into(), value.into());
        self
    }

    #[must_use]
    pub fn state(&self, name: &str) -> Option<&BlockStateValue> {
        self.states
            .get(name)
            .or_else(|| {
                name.strip_prefix("minecraft:")
                    .and_then(|plain| self.states.get(plain))
            })
            .or_else(|| {
                self.states.iter().find_map(|(key, value)| {
                    key.strip_prefix("minecraft:")
                        .filter(|plain| *plain == name)
                        .map(|_| value)
                })
            })
    }

    /// Converts a decoded world palette state into the model resolver's canonical query.
    #[must_use]
    pub fn from_world_state(state: &BlockState) -> Self {
        let mut query = Self::new(state.name.clone());
        for (name, value) in &state.states {
            if let Some(value) = BlockStateValue::from_nbt(value) {
                query.states.insert(name.clone(), value);
            }
        }
        query.name = crate::canonical_block_name_for_state(&query);
        query
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockStateValue {
    Bool(bool),
    Int(i64),
    String(String),
}

impl BlockStateValue {
    fn from_nbt(value: &NbtTag) -> Option<Self> {
        match value {
            NbtTag::Byte(value) => Some(Self::Int(i64::from(*value))),
            NbtTag::Short(value) => Some(Self::Int(i64::from(*value))),
            NbtTag::Int(value) => Some(Self::Int(i64::from(*value))),
            NbtTag::Long(value) => Some(Self::Int(*value)),
            NbtTag::String(value) => Some(Self::String(value.clone())),
            _ => None,
        }
    }

    #[must_use]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Bool(value) => *value,
            Self::Int(value) => *value != 0,
            Self::String(value) => !value.is_empty() && value != "false" && value != "0",
        }
    }

    #[must_use]
    pub fn matches_literal(&self, literal: &str) -> bool {
        match self {
            Self::Bool(value) => {
                literal.eq_ignore_ascii_case(if *value { "true" } else { "false" })
                    || (*value && literal == "1")
                    || (!*value && literal == "0")
            }
            Self::Int(value) => match literal {
                "true" => *value != 0,
                "false" => *value == 0,
                _ => literal
                    .parse::<i64>()
                    .is_ok_and(|literal_value| *value == literal_value),
            },
            Self::String(value) => value == literal,
        }
    }
}

impl From<bool> for BlockStateValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i32> for BlockStateValue {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<i64> for BlockStateValue {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<&str> for BlockStateValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_owned())
    }
}

impl From<String> for BlockStateValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockStateQuery, BlockStateValue};
    use bedrock_world::{BlockState, NbtTag};
    use std::collections::BTreeMap;

    #[test]
    fn integer_state_values_should_match_boolean_literals() {
        assert!(BlockStateValue::Int(1).matches_literal("true"));
        assert!(BlockStateValue::Int(0).matches_literal("false"));
        assert!(!BlockStateValue::Int(0).matches_literal("true"));
    }

    #[test]
    fn world_state_conversion_preserves_direction_open_and_vertical_half() {
        let state = BlockState {
            name: "minecraft:oak_trapdoor".to_string(),
            states: BTreeMap::from([
                ("direction".to_string(), NbtTag::Int(3)),
                ("open_bit".to_string(), NbtTag::Byte(1)),
                ("upside_down_bit".to_string(), NbtTag::Byte(1)),
            ]),
            version: Some(1_815_376_65),
        };

        let query = BlockStateQuery::from_world_state(&state);
        assert_eq!(query.state("direction"), Some(&BlockStateValue::Int(3)));
        assert_eq!(query.state("open_bit"), Some(&BlockStateValue::Int(1)));
        assert_eq!(
            query.state("upside_down_bit"),
            Some(&BlockStateValue::Int(1))
        );
    }
}
