//! Little-endian Minecraft Bedrock NBT codec.
//!
//! Implementation lives under `codec/nbt/` so reader, writer, visitor and borrowed-view paths can be
//! split independently while preserving `bedrock_world::nbt::*`.

#[path = "codec/nbt/impl.rs"]
mod implementation;

pub use implementation::*;
