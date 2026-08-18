//! Adapter from the block-domain authoritative numeric table to historical chunk decoding.

use crate::block::{BlockState, LegacyNumericBlockStateTable};
use crate::chunk::migration::{LegacyBlockReference, LegacyBlockResolver, LegacyBlockSource};

impl LegacyBlockResolver for LegacyNumericBlockStateTable {
    fn resolve(
        &self,
        _source: LegacyBlockSource,
        block: LegacyBlockReference,
    ) -> Option<BlockState> {
        // PocketMine's authoritative upgrader falls back to metadata 0 when a specific legacy
        // metadata value is absent. Reproduce that behaviour here while keeping `get()` an exact,
        // allocation-free block-domain lookup primitive.
        self.get(u32::from(block.id), u32::from(block.data))
            .or_else(|| self.get(u32::from(block.id), 0))
            .cloned()
    }
}
