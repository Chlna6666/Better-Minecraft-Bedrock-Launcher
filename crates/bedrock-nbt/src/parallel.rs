//! Parallel decoding of independent documents using Rayon's shared bounded pool.
//!
//! Boundaries must come from the caller: a single recursive NBT tree or a string
//! cannot safely be split at arbitrary byte offsets. Output order matches input,
//! and each failure retains its document index instead of discarding other results.

use crate::{NbtTag, Result, binary, parse_root, snbt};
use rayon::prelude::*;

/// Parses independent little-endian compound roots, preserving result order.
pub fn parse_roots(inputs: &[&[u8]]) -> Vec<Result<NbtTag>> {
    inputs.par_iter().map(|input| parse_root(input)).collect()
}

/// Parses independent named roots in the specified wire encoding.
pub fn parse_named(
    inputs: &[&[u8]],
    encoding: binary::Encoding,
) -> Vec<Result<(binary::NamedTag, usize)>> {
    inputs
        .par_iter()
        .map(|input| binary::parse(input, encoding))
        .collect()
}

/// Parses independent SNBT documents, preserving result order.
pub fn parse_snbt(inputs: &[&str]) -> Vec<Result<NbtTag>> {
    inputs.par_iter().map(|input| snbt::parse(input)).collect()
}
