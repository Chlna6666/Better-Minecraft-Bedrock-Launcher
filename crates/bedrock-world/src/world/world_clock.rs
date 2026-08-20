//! World-handle access to Bedrock `level.dat` clock metadata.

use super::{BedrockWorld, WorldStorageHandle};
use crate::error::Result;
use crate::level::BedrockWorldClock;

impl<S> BedrockWorld<S>
where
    S: WorldStorageHandle,
{
    /// Reads the persisted Bedrock world clock through this already-open world handle.
    pub fn world_clock_blocking(&self) -> Result<BedrockWorldClock> {
        self.read_level_dat_blocking()?.world_clock()
    }

    /// Atomically persists the supplied Bedrock world clock through this world handle.
    ///
    /// Read-only handles are rejected by the existing `level.dat` write path. Unrelated and unknown
    /// metadata remains preserved because the complete document is round-tripped.
    pub fn write_world_clock_blocking(&self, clock: BedrockWorldClock) -> Result<()> {
        let mut document = self.read_level_dat_blocking()?;
        document.set_world_clock(clock)?;
        self.write_level_dat_blocking(&document)
    }
}
