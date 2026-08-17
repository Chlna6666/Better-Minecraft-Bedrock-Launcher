//! Minecraft Bedrock `level.dat` parsing, validation and atomic writes.

#[path = "level/impl.rs"]
mod implementation;

pub use implementation::*;
