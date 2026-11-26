//! Native node slots, using GPUI Fast's integer key ownership model.
//! Copyright 2022-2025 Zed Industries, Inc. Copyright 2026 Longbridge.
//! Adapted from longbridge/gpui-fast 1100a214532bdeac0282acd1c5c6d2062d8ab1a7.
//! SPDX-License-Identifier: Apache-2.0

/// A native node is identified by its running path/slot hash. Row retention
/// is scope metadata, not a second element identity or an owned path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeLayoutKey {
    pub key: u64,
    pub lingers: bool,
}

pub(crate) fn is_list_row_segment(segment: &crate::ElementId) -> bool {
    matches!(segment, crate::ElementId::NamedInteger(name, _)
        if matches!(name.as_str(), "__gpui_retained_list_item" | "__gpui_retained_uniform_row"))
}

/// Where layout key paths start.
pub(crate) const LAYOUT_ROOT_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Separates the elements laid out during an element's prepaint from that same
/// element's children, which are keyed from the same point during request
/// layout and would otherwise land on the same keys.
pub(crate) const LAYOUT_PREPAINT_SALT: u64 = 0x5BF0_3635_931A_2E77;

/// Mixes `value` into `state`, well enough that path hashes built out of small
/// child indices do not collide in practice.
///
/// This is the finalizer from SplitMix64 applied to the combined value.
pub(crate) fn mix(state: u64, value: u64) -> u64 {
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
