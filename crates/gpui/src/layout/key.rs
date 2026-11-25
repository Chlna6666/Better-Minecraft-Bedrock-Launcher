//! Native node slots derived from the already incremental retained element path.
//! No second path stack or frame-wide ordinal table is required.

/// Mixes `value` into `state` with the SplitMix64 finalizer, so paths built from small child
/// indices do not collide in practice.
#[inline]
fn mix(state: u64, value: u64) -> u64 {
    let mut z = state
        .rotate_left(27)
        .wrapping_add(value)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}


/// One element may request several Taffy nodes. Number them locally in its
/// request scope and mix the slot into the full retained path hash.
#[inline]
pub(crate) fn layout_node_key(path: u64, slot: u32) -> u64 {
    mix(path, u64::from(slot))
}
