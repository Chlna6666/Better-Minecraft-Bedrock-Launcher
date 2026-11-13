use crate::{
    NbtEvent, NbtRef, NbtTag, Result, borrowed, events::parse_nbt_events, parse_root,
    parse_root_with_consumed, serialize_root, visit_nbt_events,
};
use std::ops::ControlFlow;

/// Reader for a single in-memory Bedrock NBT payload.
pub struct NbtReader<'a> {
    data: &'a [u8],
}

impl<'a> NbtReader<'a> {
    #[must_use]
    /// Creates a reader over a complete in-memory Bedrock NBT payload.
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Parses the payload as a single root NBT tag.
    pub fn parse_root(&self) -> Result<NbtTag> {
        parse_root(self.data)
    }

    /// Parses one root NBT tag and returns the number of bytes consumed.
    pub fn parse_root_with_consumed(&self) -> Result<(NbtTag, usize)> {
        parse_root_with_consumed(self.data)
    }

    /// Parses one root tag and exposes it through a borrowed-style view.
    pub fn parse_root_ref(&self) -> Result<NbtRef<'a>> {
        borrowed::parse_root(self.data)
    }

    /// Returns a borrowed view over the same raw payload.
    pub fn view(&self) -> NbtView<'a> {
        NbtView::new(self.data)
    }
}

#[derive(Debug, Clone, Copy)]
/// Borrowed Bedrock NBT payload view.
pub struct NbtView<'a> {
    data: &'a [u8],
}

impl<'a> NbtView<'a> {
    #[must_use]
    /// Creates a borrowed view over a complete in-memory Bedrock NBT payload.
    pub const fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    /// Parses the payload into a flat stream of borrowed NBT events.
    pub fn events(&self) -> Result<Vec<NbtEvent<'a>>> {
        parse_nbt_events(self.data)
    }

    /// Visits borrowed NBT events without materializing an event vector.
    ///
    /// Returning [`ControlFlow::Break`] stops parsing immediately.
    pub fn visit_events(&self, visitor: impl FnMut(NbtEvent<'a>) -> ControlFlow<()>) -> Result<()> {
        visit_nbt_events(self.data, visitor)
    }
}

/// Writer for Bedrock little-endian NBT roots.
pub struct NbtWriter;

impl NbtWriter {
    /// Write root.
    pub fn write_root(tag: &NbtTag) -> Result<Vec<u8>> {
        serialize_root(tag)
    }
}
