//! Light metadata stays independent of the propagation and rendering implementation.
use crate::{BlockModelRepository, BlockStateQuery, BlockStateValue};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::OnceLock};

/// Light emitted and attenuated by a block, on the Bedrock 0..=15 scale.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockLightProperties {
    /// Intrinsic source level; opaque light sources may emit into adjacent air.
    pub emission: u8,
    /// Incoming-light attenuation; 15 blocks transmission.
    pub dampening: u8,
}

#[derive(Deserialize)]
struct LightTable {
    blocks: BTreeMap<String, [u8; 2]>,
}

impl BlockModelRepository {
    /// Resolves resource components and matching permutations over versioned vanilla defaults.
    /// Unknown names default to non-emitting, with attenuation inferred from their model family.
    #[must_use]
    pub fn light_properties(&self, state: &BlockStateQuery) -> BlockLightProperties {
        static TABLE: OnceLock<LightTable> = OnceLock::new();
        let table = TABLE.get_or_init(|| {
            serde_json::from_slice(include_bytes!("../generated/block_lighting.json"))
                .expect("embedded block lighting metadata must be valid")
        });
        let name = state.name.strip_prefix("minecraft:").unwrap_or(&state.name);
        let default = if state.name.starts_with("minecraft:") || !state.name.contains(':') {
            table.blocks.get(name).copied()
        } else {
            None
        };
        let [mut emission, mut dampening] = default.unwrap_or([
            0,
            if crate::is_full_opaque_block(name) {
                15
            } else {
                0
            },
        ]);
        if name == "water" || name == "flowing_water" {
            dampening = 2;
        }
        if name.contains("leaves") {
            dampening = 1;
        }
        let off = |key| {
            matches!(
                state.state(key),
                Some(BlockStateValue::Bool(false) | BlockStateValue::Int(0))
            )
        };
        let on = |key| {
            matches!(
                state.state(key),
                Some(BlockStateValue::Bool(true) | BlockStateValue::Int(1))
            )
        };
        if off("lit") || on("extinguished") {
            emission = 0;
        }
        if let Some(block) = self.blocks.get(&state.name) {
            let (components, _, _) = crate::resolver::resolved_components(block, state);
            emission = components.light_emission.unwrap_or(emission);
            dampening = components.light_dampening.unwrap_or(dampening);
        }
        BlockLightProperties {
            emission: emission.min(15),
            dampening: dampening.min(15),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_permutations_override_defaults_without_cross_namespace_fallback() {
        let mut models = BlockModelRepository::new();
        models.merge_blocks_value(&serde_json::json!({"minecraft:block": {"description":{"identifier":"test:lamp"},
            "components":{"minecraft:light_emission":2,"minecraft:light_dampening":0},
            "permutations":[{"condition":"q.block_state('lit') == true","components":{"minecraft:light_emission":15}}]}}));
        assert_eq!(
            models.light_properties(&BlockStateQuery::new("test:lamp").with_state("lit", true)),
            BlockLightProperties {
                emission: 15,
                dampening: 0
            }
        );
        assert_eq!(
            models
                .light_properties(&BlockStateQuery::new("test:torch"))
                .emission,
            0
        );
        assert_eq!(
            models
                .light_properties(&BlockStateQuery::new("minecraft:torch"))
                .emission,
            14
        );
        assert_eq!(
            models
                .light_properties(&BlockStateQuery::new("minecraft:water"))
                .dampening,
            2
        );
    }
}
