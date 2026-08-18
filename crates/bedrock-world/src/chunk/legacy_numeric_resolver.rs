//! Bridge from authoritative legacy block-version data to historical chunk decoding.

use crate::block::{BlockState, LegacyNumericBlockStateTable};
use crate::chunk::conversion::{LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource};

impl LegacyBlockResolver for LegacyNumericBlockStateTable {
    fn resolve(
        &self,
        _source: LegacyBlockSource,
        block: LegacyBlockReference,
    ) -> Option<BlockState> {
        self.get(u32::from(block.id), u32::from(block.data))
            .or_else(|| self.get(u32::from(block.id), 0))
            .cloned()
    }
}
