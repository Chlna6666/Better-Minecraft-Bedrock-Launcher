//! Public level.dat world-time helpers.
//!
//! These helpers intentionally expose Bedrock world data semantics, not a server-specific API. The
//! `Time` tag is the authoritative monotonically increasing world tick stored in level.dat; callers
//! may derive time-of-day from it with `time % 24000`.

use crate::{NbtTag, error::Result};
use crate::level_dat::{read_level_dat_document, write_level_dat_document};
use std::path::Path;

/// Number of Bedrock ticks in the vanilla day/night cycle.
pub const BEDROCK_DAY_TICKS: u64 = 24_000;

/// Stable view of the world clock values stored in level.dat.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BedrockWorldTime {
    /// Monotonically increasing world tick from the `Time` tag.
    pub time: u64,
    /// Daylight-cycle gamerule from `dodaylightcycle` when present.
    pub daylight_cycle: bool,
}

impl BedrockWorldTime {
    /// Returns `time % 24000`, the value clients use for the sky angle/daylight phase.
    #[must_use]
    pub const fn time_of_day(self) -> u64 {
        self.time % BEDROCK_DAY_TICKS
    }
}

/// Reads Bedrock world time from `level.dat` in a world folder.
pub fn read_world_time(path: impl AsRef<Path>) -> Result<BedrockWorldTime> {
    let document = read_level_dat_document(&path.as_ref().join("level.dat"))?;
    let NbtTag::Compound(root) = document.root else {
        return Ok(BedrockWorldTime::default());
    };
    let time = match root.get("Time") {
        Some(NbtTag::Long(value)) => (*value).max(0) as u64,
        Some(NbtTag::Int(value)) => (*value).max(0) as u64,
        _ => 0,
    };
    let daylight_cycle = match root.get("dodaylightcycle") {
        Some(NbtTag::Byte(value)) => *value != 0,
        Some(NbtTag::Int(value)) => *value != 0,
        _ => true,
    };
    Ok(BedrockWorldTime {
        time,
        daylight_cycle,
    })
}

/// Writes the Bedrock `Time` tag and optionally advances or preserves the daylight-cycle gamerule.
pub fn write_world_time(path: impl AsRef<Path>, value: BedrockWorldTime) -> Result<()> {
    let level_path = path.as_ref().join("level.dat");
    let mut document = read_level_dat_document(&level_path)?;
    let NbtTag::Compound(root) = &mut document.root else {
        return write_level_dat_document(&level_path, &document);
    };
    root.insert(
        "Time".to_string(),
        NbtTag::Long(i64::try_from(value.time).unwrap_or(i64::MAX)),
    );
    root.insert(
        "dodaylightcycle".to_string(),
        NbtTag::Byte(i8::from(value.daylight_cycle)),
    );
    write_level_dat_document(&level_path, &document)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_of_day_wraps_bedrock_cycle() {
        assert_eq!(
            BedrockWorldTime {
                time: 24_001,
                daylight_cycle: true,
            }
            .time_of_day(),
            1
        );
    }
}
