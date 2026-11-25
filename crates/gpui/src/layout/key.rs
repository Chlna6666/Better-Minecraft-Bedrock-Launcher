//! Incremental element-path hashes used as layout node identities.
//!
//! Finding the Taffy node an element had last frame needs a stable key, and the key is
//! derived for every element of every frame. Building a full `GlobalElementId` for that would
//! allocate an `Arc` and copy the whole path per element; a running hash costs one `mix` per
//! element and nothing else.
//!
//! The hash is a path hash, not a content hash: it changes when the element's address in the
//! tree changes, and it stays the same across frames for an element that keeps its address.
//! That is exactly what a cross-frame node cache needs.

use crate::ElementId;
use collections::FxHasher;
use std::hash::{Hash, Hasher};

/// One level of the layout key path.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LayoutKeyFrame {
    /// Hash of the path from the root to this element.
    pub(crate) key: u64,
    /// Children without an [`ElementId`] entered so far under this element, which is what
    /// identifies the next one.
    pub(crate) next_unidentified_child: u32,
}

/// Where layout key paths start.
pub(crate) const LAYOUT_ROOT_SEED: u64 = 0x9E37_79B9_7F4A_7C15;

/// Separates elements laid out during an element's prepaint from that same element's
/// children, which are keyed from the same point during request layout.
pub(crate) const LAYOUT_PREPAINT_SALT: u64 = 0x5BF0_3635_931A_2E77;

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

/// Derives the layout key for an element and enters it.
///
/// `id` names the element when the application gave it one; otherwise its position among its
/// parent's unidentified children names it, which is stable as long as the siblings around it
/// stay put.
pub(crate) fn push_layout_key(
    stack: &mut smallvec::SmallVec<[LayoutKeyFrame; 32]>,
    root_index: &mut u32,
    id: Option<&ElementId>,
) -> u64 {
    let component = match id {
        Some(id) => {
            let mut hasher = FxHasher::default();
            id.hash(&mut hasher);
            // Kept distinct from the positional case, so an element identified by index 3 and
            // one whose `ElementId` hashes to 3 cannot collide.
            mix(hasher.finish(), 1)
        }
        None => {
            let index = match stack.last_mut() {
                Some(parent) => &mut parent.next_unidentified_child,
                None => root_index,
            };
            let component = mix(u64::from(*index), 2);
            *index = index.saturating_add(1);
            component
        }
    };
    let parent = stack
        .last()
        .map_or(LAYOUT_ROOT_SEED, |parent| parent.key);
    let key = mix(parent, component);
    stack.push(LayoutKeyFrame {
        key,
        next_unidentified_child: 0,
    });
    key
}

/// Leaves the level [`push_layout_key`] entered.
pub(crate) fn pop_layout_key(stack: &mut smallvec::SmallVec<[LayoutKeyFrame; 32]>) {
    stack.pop();
}

/// The key of the element enclosing the one currently requesting layout.
pub(crate) fn parent_layout_key(stack: &[LayoutKeyFrame]) -> Option<u64> {
    stack.len().checked_sub(2).map(|parent| stack[parent].key)
}
