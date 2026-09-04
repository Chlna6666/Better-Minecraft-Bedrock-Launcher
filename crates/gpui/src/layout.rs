mod builders;
mod cache;
mod convert;
mod engine;
mod fingerprint;
mod metrics;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod fractional_tests;

pub use builders::{absolute_fill, center, h_stack, relative_fill, v_stack};
pub use engine::TaffyLayoutEngine;
pub use metrics::{AvailableSpace, LayoutId};

impl TaffyLayoutEngine {
    /// Returns the current frame's recursive layout fingerprint for one node.
    ///
    /// The fingerprint already includes this node's layout style plus every child layout
    /// fingerprint, so it changes when a relative/flex/grid container keeps the same final bounds
    /// but one descendant moves, resizes, is inserted, or is removed. `None` means the subtree
    /// contains an unprovable measured node and retained reconciliation must stay conservative.
    pub(crate) fn retained_layout_fingerprint(&self, id: LayoutId) -> Option<u64> {
        self.node_fingerprints.get(&id).copied().flatten()
    }
}
