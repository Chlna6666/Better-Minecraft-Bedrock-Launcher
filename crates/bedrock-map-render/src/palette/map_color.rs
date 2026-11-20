//! Fixed-version Bedrock map colors, separate from resource-pack appearance colors.

use super::import::RgbaColor;
use bedrock_world::{BlockState, NbtTag};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

const BUILTIN_MAP_COLOR_JSON: &str = include_str!("../../data/colors/bedrock-map-color.json");

/// Vanilla Bedrock map tint attached to a block's base map color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapTintMethod {
    /// No biome or block-specific tint.
    None,
    /// Birch foliage tint.
    BirchFoliage,
    /// Default foliage tint.
    DefaultFoliage,
    /// Dry foliage tint.
    DryFoliage,
    /// Evergreen foliage tint.
    EvergreenFoliage,
    /// Grass tint.
    Grass,
    /// Redstone wire power tint.
    RedStoneWire,
    /// Stem growth tint.
    Stem,
    /// Water tint.
    Water,
}

impl MapTintMethod {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "None" => Self::None,
            "BirchFoliage" => Self::BirchFoliage,
            "DefaultFoliage" => Self::DefaultFoliage,
            "DryFoliage" => Self::DryFoliage,
            "EvergreenFoliage" => Self::EvergreenFoliage,
            "Grass" => Self::Grass,
            "RedStoneWire" => Self::RedStoneWire,
            "Stem" => Self::Stem,
            "Water" => Self::Water,
            _ => return None,
        })
    }
}

/// Base map color and its map-specific tint method for one Bedrock block state.
///
/// This is sourced from pinned Bedrock 1.26.32.2 data. It is not an appearance/texture color and
/// does not include the actual biome-dependent tint or 2.5D height shade.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapColor {
    /// Untinted RGBA map color.
    pub color: RgbaColor,
    /// Tint required when projecting this block into an in-game map.
    pub tint_method: MapTintMethod,
}

/// One enumerable map-color candidate, with an optional exact string-state requirement.
#[derive(Debug, Clone, Copy)]
pub struct MapColorEntry<'a> {
    /// Canonical Bedrock block identifier.
    pub block: &'a str,
    /// State property name and string value, for color-changing block states.
    pub state: Option<(&'a str, &'a str)>,
    /// Base color and tint method.
    pub map_color: MapColor,
}

#[derive(Debug, Clone)]
struct MapStateColor {
    state_name: String,
    state_value: String,
    color: MapColor,
}

/// Embedded map-color table shared by palette instances.
#[derive(Debug)]
pub(super) struct MapColorTable {
    colors: BTreeMap<String, MapColor>,
    state_colors: BTreeMap<String, Vec<MapStateColor>>,
}

impl MapColorTable {
    pub(super) fn builtin() -> Arc<Self> {
        static TABLE: OnceLock<Arc<MapColorTable>> = OnceLock::new();
        Arc::clone(TABLE.get_or_init(|| {
            Arc::new(Self::parse(BUILTIN_MAP_COLOR_JSON).expect("valid bundled Bedrock map colors"))
        }))
    }

    pub(super) fn color(&self, state: &BlockState) -> Option<MapColor> {
        if let Some(rules) = self.state_colors.get(&state.name) {
            return rules.iter().find_map(|rule| {
                matches!(
                    state.states.get(&rule.state_name),
                    Some(NbtTag::String(value)) if value == &rule.state_value
                )
                .then_some(rule.color)
            });
        }
        self.colors.get(&state.name).copied()
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = MapColorEntry<'_>> {
        self.colors
            .iter()
            .map(|(block, color)| MapColorEntry {
                block,
                state: None,
                map_color: *color,
            })
            .chain(self.state_colors.iter().flat_map(|(block, rules)| {
                rules.iter().map(move |rule| MapColorEntry {
                    block,
                    state: Some((&rule.state_name, &rule.state_value)),
                    map_color: rule.color,
                })
            }))
    }

    fn parse(json: &str) -> Option<Self> {
        let root: Value = serde_json::from_str(json).ok()?;
        if root.get("format")?.as_str()? != "bedrock-map-color-v1"
            || root.get("bedrock_version")?.as_str()? != "1.26.32.2"
            || root.get("color_encoding")?.as_str()? != "#RRGGBBAA"
        {
            return None;
        }
        let mut colors = BTreeMap::new();
        for (name, entry) in root.get("colors")?.as_object()? {
            colors.insert(name.clone(), parse_color_entry(entry)?);
        }
        let mut state_colors = BTreeMap::new();
        for (name, entry) in root.get("state_colors")?.as_object()? {
            let state_name = entry.get("state")?.as_str()?;
            let mut rules = Vec::new();
            for (state_value, color) in entry.get("values")?.as_object()? {
                rules.push(MapStateColor {
                    state_name: state_name.to_string(),
                    state_value: state_value.clone(),
                    color: parse_color_entry(color)?,
                });
            }
            state_colors.insert(name.clone(), rules);
        }
        Some(Self {
            colors,
            state_colors,
        })
    }
}

fn parse_color_entry(entry: &Value) -> Option<MapColor> {
    let text = entry.get("color")?.as_str()?;
    if text.len() != 9 || !text.starts_with('#') {
        return None;
    }
    let channel = |offset| u8::from_str_radix(text.get(offset..offset + 2)?, 16).ok();
    Some(MapColor {
        color: RgbaColor::new(channel(1)?, channel(3)?, channel(5)?, channel(7)?),
        tint_method: MapTintMethod::parse(entry.get("tint_method")?.as_str()?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RenderPalette;

    #[test]
    fn pinned_map_colors_cover_all_names_and_log_axes() {
        let table = MapColorTable::builtin();
        assert_eq!(table.colors.len(), 1347);
        assert_eq!(table.state_colors.len(), 9);
        assert_eq!(table.entries().count(), 1374);
        let mut oak = BlockState {
            name: "minecraft:oak_log".to_string(),
            states: BTreeMap::from([("pillar_axis".to_string(), NbtTag::String("y".to_string()))]),
            version: None,
        };
        assert_eq!(
            table.color(&oak).unwrap().color.to_array(),
            [143, 119, 72, 255]
        );
        oak.states
            .insert("pillar_axis".to_string(), NbtTag::String("x".to_string()));
        assert_eq!(
            table.color(&oak).unwrap().color.to_array(),
            [129, 86, 49, 255]
        );
        oak.states.clear();
        assert!(table.color(&oak).is_none());
    }

    #[test]
    fn appearance_override_does_not_change_map_color() {
        let palette =
            RenderPalette::new().with_block_color("minecraft:stone", RgbaColor::new(1, 2, 3, 255));
        let stone = BlockState {
            name: "minecraft:stone".to_string(),
            states: BTreeMap::new(),
            version: None,
        };
        assert_eq!(
            palette.block_color("minecraft:stone").to_array(),
            [1, 2, 3, 255]
        );
        assert_ne!(
            palette.map_color(&stone).unwrap().color.to_array(),
            [1, 2, 3, 255]
        );
    }
}
