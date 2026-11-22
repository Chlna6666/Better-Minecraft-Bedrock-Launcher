// SPDX-FileCopyrightText: 2026 BE Community Dev
// SPDX-License-Identifier: MIT OR Apache-2.0
// Library source: https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher/tree/master/crates/bedrock-nbt
// AI/code-reuse attribution: this library has separate MIT/Apache terms;
// preserve BE Community Dev credit and the applicable license notices.
//! Bedrock NBT/SNBT codecs, independent of world versions and storage.
//!
//! The root helpers retain the little-endian compound-root contract used by
//! Bedrock saves. [`binary`] exposes named tags and explicit wire encodings;
//! [`snbt`] handles textual values. `level.dat` headers belong to the caller.

mod borrowed;
mod codec;
mod constants;
mod error;
mod events;
mod reader;
mod tag;
mod validation;

pub mod binary;
#[cfg(feature = "parallel")]
pub mod parallel;
pub mod snbt;

pub use codec::{
    parse_consecutive_roots, parse_root, parse_root_with_consumed, serialize_consecutive_roots,
    serialize_root,
};
pub use error::{Error, Result};
pub use events::{NbtEvent, visit_nbt_events};
pub use reader::{NbtReader, NbtView, NbtWriter};
pub use tag::{NbtRef, NbtTag, tags_equal_for_write};
pub use validation::validate_root_for_write;

#[cfg(test)]
mod tests;
