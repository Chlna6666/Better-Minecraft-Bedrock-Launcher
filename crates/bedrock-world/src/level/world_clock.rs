//! Bedrock world clock metadata stored in `level.dat`.
//!
//! Minecraft Bedrock persists the visible day/night clock in `Time`, the game-tick counter in
//! `currentTick`, and the daylight-cycle gamerule in `dodaylightcycle`. This module keeps those
//! fields behind a typed public API so callers do not duplicate raw NBT field handling.

use super::{LevelDatDocument, read_level_dat_document, write_level_dat_document};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use std::path::Path;

/// Typed Bedrock world clock metadata persisted in `level.dat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BedrockWorldClock {
    /// Visible world time stored in `Time`.
    pub time: i64,
    /// Monotonic world/game tick counter stored in `currentTick`.
    pub current_tick: i64,
    /// Whether the `dodaylightcycle` gamerule advances [`Self::time`].
    pub daylight_cycle: bool,
}

impl Default for BedrockWorldClock {
    fn default() -> Self {
        Self {
            time: 0,
            current_tick: 0,
            daylight_cycle: true,
        }
    }
}

impl BedrockWorldClock {
    /// Advances one authoritative world tick.
    ///
    /// `currentTick` always advances. `Time` advances only while daylight cycling is enabled,
    /// matching the persisted Bedrock gamerule semantics.
    pub fn advance(&mut self) {
        self.current_tick = self.current_tick.saturating_add(1);
        if self.daylight_cycle {
            self.time = self.time.saturating_add(1);
        }
    }
}

impl LevelDatDocument {
    /// Reads `Time`, `currentTick`, and `dodaylightcycle` from this document.
    ///
    /// Missing fields use Bedrock-compatible creation defaults. Numeric fields accept historical
    /// Byte/Short/Int/Long representations so old worlds remain readable without migration.
    pub fn world_clock(&self) -> Result<BedrockWorldClock> {
        let NbtTag::Compound(root) = &self.root else {
            return Err(BedrockWorldError::CorruptWorld(
                "level.dat root is not a compound".to_string(),
            ));
        };

        let time = root
            .get("Time")
            .map(|tag| read_i64_field("Time", tag))
            .transpose()?
            .unwrap_or(0);
        let current_tick = root
            .get("currentTick")
            .map(|tag| read_i64_field("currentTick", tag))
            .transpose()?
            .unwrap_or(0);
        let daylight_cycle = root
            .get("dodaylightcycle")
            .map(|tag| read_bool_field("dodaylightcycle", tag))
            .transpose()?
            .unwrap_or(true);

        Ok(BedrockWorldClock {
            time,
            current_tick,
            daylight_cycle,
        })
    }

    /// Replaces the typed world-clock fields while preserving all unrelated `level.dat` metadata.
    pub fn set_world_clock(&mut self, clock: BedrockWorldClock) -> Result<()> {
        let NbtTag::Compound(root) = &mut self.root else {
            return Err(BedrockWorldError::CorruptWorld(
                "level.dat root is not a compound".to_string(),
            ));
        };
        root.insert("Time".to_string(), NbtTag::Long(clock.time));
        root.insert("currentTick".to_string(), NbtTag::Long(clock.current_tick));
        root.insert(
            "dodaylightcycle".to_string(),
            NbtTag::Byte(if clock.daylight_cycle { 1 } else { 0 }),
        );
        Ok(())
    }
}

/// Reads typed world-clock metadata from an existing Bedrock `level.dat`.
pub fn read_level_dat_world_clock(path: &Path) -> Result<BedrockWorldClock> {
    read_level_dat_document(path)?.world_clock()
}

/// Atomically updates only the world-clock fields in an existing Bedrock `level.dat`.
///
/// The complete document is parsed and rewritten so unknown/newer metadata remains preserved.
pub fn write_level_dat_world_clock(path: &Path, clock: BedrockWorldClock) -> Result<()> {
    let mut document = read_level_dat_document(path)?;
    document.set_world_clock(clock)?;
    write_level_dat_document(path, &document)
}

fn read_i64_field(name: &str, tag: &NbtTag) -> Result<i64> {
    match tag {
        NbtTag::Byte(value) => Ok(i64::from(*value)),
        NbtTag::Short(value) => Ok(i64::from(*value)),
        NbtTag::Int(value) => Ok(i64::from(*value)),
        NbtTag::Long(value) => Ok(*value),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "level.dat {name} uses unsupported NBT type: {other:?}"
        ))),
    }
}

fn read_bool_field(name: &str, tag: &NbtTag) -> Result<bool> {
    match tag {
        NbtTag::Byte(value) => Ok(*value != 0),
        NbtTag::Short(value) => Ok(*value != 0),
        NbtTag::Int(value) => Ok(*value != 0),
        NbtTag::Long(value) => Ok(*value != 0),
        other => Err(BedrockWorldError::CorruptWorld(format!(
            "level.dat {name} uses unsupported NBT type: {other:?}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn world_clock_accepts_historical_numeric_widths() {
        let root = IndexMap::from([
            ("Time".to_string(), NbtTag::Int(6000)),
            ("currentTick".to_string(), NbtTag::Long(42)),
            ("dodaylightcycle".to_string(), NbtTag::Byte(0)),
        ]);
        let document = LevelDatDocument::new(10, NbtTag::Compound(root));
        assert_eq!(
            document.world_clock().expect("clock"),
            BedrockWorldClock {
                time: 6000,
                current_tick: 42,
                daylight_cycle: false,
            }
        );
    }

    #[test]
    fn advance_keeps_current_tick_running_when_daylight_is_frozen() {
        let mut clock = BedrockWorldClock {
            time: 13000,
            current_tick: 99,
            daylight_cycle: false,
        };
        clock.advance();
        assert_eq!(clock.time, 13000);
        assert_eq!(clock.current_tick, 100);
    }

    #[test]
    fn set_world_clock_preserves_unrelated_level_metadata() {
        let mut root = IndexMap::new();
        root.insert(
            "LevelName".to_string(),
            NbtTag::String("KeepMe".to_string()),
        );
        let mut document = LevelDatDocument::new(10, NbtTag::Compound(root));
        document
            .set_world_clock(BedrockWorldClock {
                time: 123,
                current_tick: 456,
                daylight_cycle: true,
            })
            .expect("set clock");
        let NbtTag::Compound(root) = document.root else {
            panic!("compound")
        };
        assert_eq!(
            root.get("LevelName"),
            Some(&NbtTag::String("KeepMe".to_string()))
        );
        assert_eq!(root.get("Time"), Some(&NbtTag::Long(123)));
        assert_eq!(root.get("currentTick"), Some(&NbtTag::Long(456)));
    }
}
