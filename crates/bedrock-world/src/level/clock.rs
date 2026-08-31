//! Minecraft Bedrock world clock metadata stored in `level.dat`.
//!
//! Bedrock persists the visible world time in `Time`, the authoritative game-tick counter in
//! `currentTick`, and daylight-cycle progression in the `dodaylightcycle` gamerule. This module is
//! the single typed representation of those fields; higher-level world handles should delegate here
//! instead of duplicating NBT handling.

use super::{LevelDatDocument, read_level_dat_document, write_level_dat_document};
use crate::error::{BedrockWorldError, Result};
use crate::nbt::NbtTag;
use std::path::Path;

/// Minecraft Bedrock world clock state persisted in `level.dat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorldClock {
    /// Visible world time stored in the `Time` field, measured in game ticks.
    pub time: i64,
    /// Authoritative world tick counter stored in `currentTick`.
    pub current_tick: i64,
    /// Whether the `dodaylightcycle` gamerule advances [`Self::time`].
    pub daylight_cycle: bool,
}

impl Default for WorldClock {
    fn default() -> Self {
        Self {
            time: 0,
            current_tick: 0,
            daylight_cycle: true,
        }
    }
}

impl WorldClock {
    /// Advances one authoritative world tick.
    ///
    /// `currentTick` always advances. `Time` advances only while the daylight cycle is enabled.
    pub fn advance(&mut self) {
        self.current_tick = self.current_tick.saturating_add(1);
        if self.daylight_cycle {
            self.time = self.time.saturating_add(1);
        }
    }
}

impl LevelDatDocument {
    /// Reads the persisted Bedrock world clock from this `level.dat` document.
    ///
    /// Missing fields use Bedrock-compatible creation defaults. Historical integer widths are
    /// accepted so older worlds remain readable without a migration pass.
    pub fn clock(&self) -> Result<WorldClock> {
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

        Ok(WorldClock {
            time,
            current_tick,
            daylight_cycle,
        })
    }

    /// Replaces the persisted clock fields while preserving unrelated `level.dat` metadata.
    pub fn set_clock(&mut self, clock: WorldClock) -> Result<()> {
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

/// Reads the typed Bedrock world clock from an existing `level.dat` file.
pub fn read_clock(path: &Path) -> Result<WorldClock> {
    read_level_dat_document(path)?.clock()
}

/// Atomically updates only the clock fields in an existing `level.dat` file.
///
/// The complete document is round-tripped so unknown and newer metadata remains preserved.
pub fn write_clock(path: &Path, clock: WorldClock) -> Result<()> {
    let mut document = read_level_dat_document(path)?;
    document.set_clock(clock)?;
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
    fn clock_accepts_historical_numeric_widths() {
        let root = IndexMap::from([
            ("Time".to_string(), NbtTag::Int(6000)),
            ("currentTick".to_string(), NbtTag::Long(42)),
            ("dodaylightcycle".to_string(), NbtTag::Byte(0)),
        ]);
        let document = LevelDatDocument::new(10, NbtTag::Compound(root));
        assert_eq!(
            document.clock().expect("clock"),
            WorldClock {
                time: 6000,
                current_tick: 42,
                daylight_cycle: false,
            }
        );
    }

    #[test]
    fn advance_keeps_current_tick_running_when_daylight_is_frozen() {
        let mut clock = WorldClock {
            time: 13000,
            current_tick: 99,
            daylight_cycle: false,
        };
        clock.advance();
        assert_eq!(clock.time, 13000);
        assert_eq!(clock.current_tick, 100);
    }

    #[test]
    fn set_clock_preserves_unrelated_level_metadata() {
        let mut root = IndexMap::new();
        root.insert(
            "LevelName".to_string(),
            NbtTag::String("KeepMe".to_string()),
        );
        let mut document = LevelDatDocument::new(10, NbtTag::Compound(root));
        document
            .set_clock(WorldClock {
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
