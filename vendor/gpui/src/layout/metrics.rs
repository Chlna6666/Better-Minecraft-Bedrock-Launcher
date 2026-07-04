use crate::{LayoutFrameMetrics, Pixels, Size};
use std::hash::Hash;
use taffy::{TraversePartialTree as _, style::AvailableSpace as TaffyAvailableSpace, tree::NodeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(super) struct LayoutRootCacheKey {
    pub(super) root_fingerprint: u64,
    pub(super) available_space: Size<AvailableSpaceKey>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(super) enum AvailableSpaceKey {
    Definite(u32),
    #[default]
    MinContent,
    MaxContent,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RetainedLayoutNode {
    pub(super) fingerprint: u64,
    pub(super) bounds: crate::Bounds<Pixels>,
    pub(super) measure_input: Option<(Size<Option<Pixels>>, Size<AvailableSpace>)>,
}

impl From<AvailableSpace> for AvailableSpaceKey {
    fn from(value: AvailableSpace) -> Self {
        match value {
            AvailableSpace::Definite(pixels) => AvailableSpaceKey::Definite(pixels.0.to_bits()),
            AvailableSpace::MinContent => AvailableSpaceKey::MinContent,
            AvailableSpace::MaxContent => AvailableSpaceKey::MaxContent,
        }
    }
}

/// A unique identifier for a layout node, generated when requesting a layout from Taffy
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[repr(transparent)]
pub struct LayoutId(pub(super) NodeId);

impl LayoutId {
    pub(super) fn to_taffy_slice(node_ids: &[Self]) -> &[taffy::NodeId] {
        // SAFETY: LayoutId is repr(transparent) to taffy::tree::NodeId.
        unsafe { std::mem::transmute::<&[LayoutId], &[taffy::NodeId]>(node_ids) }
    }
}

impl std::hash::Hash for LayoutId {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        u64::from(self.0).hash(state);
    }
}

impl From<NodeId> for LayoutId {
    fn from(node_id: NodeId) -> Self {
        Self(node_id)
    }
}

impl From<LayoutId> for NodeId {
    fn from(layout_id: LayoutId) -> NodeId {
        layout_id.0
    }
}

/// The space available for an element to be laid out in
#[derive(Copy, Clone, Default, Debug, Eq, PartialEq)]
pub enum AvailableSpace {
    /// The amount of space available is the specified number of pixels
    Definite(Pixels),
    /// The amount of space available is indefinite and the node should be laid out under a min-content constraint
    #[default]
    MinContent,
    /// The amount of space available is indefinite and the node should be laid out under a max-content constraint
    MaxContent,
}

impl AvailableSpace {
    /// Returns a `Size` with both width and height set to `AvailableSpace::MinContent`.
    ///
    /// This function is useful when you want to create a `Size` with the minimum content constraints
    /// for both dimensions.
    ///
    /// # Examples
    ///
    /// ```
    /// use gpui::AvailableSpace;
    /// let min_content_size = AvailableSpace::min_size();
    /// assert_eq!(min_content_size.width, AvailableSpace::MinContent);
    /// assert_eq!(min_content_size.height, AvailableSpace::MinContent);
    /// ```
    pub const fn min_size() -> Size<Self> {
        Size {
            width: Self::MinContent,
            height: Self::MinContent,
        }
    }
}

impl From<AvailableSpace> for TaffyAvailableSpace {
    fn from(space: AvailableSpace) -> TaffyAvailableSpace {
        match space {
            AvailableSpace::Definite(Pixels(value)) => TaffyAvailableSpace::Definite(value),
            AvailableSpace::MinContent => TaffyAvailableSpace::MinContent,
            AvailableSpace::MaxContent => TaffyAvailableSpace::MaxContent,
        }
    }
}

impl From<TaffyAvailableSpace> for AvailableSpace {
    fn from(space: TaffyAvailableSpace) -> AvailableSpace {
        match space {
            TaffyAvailableSpace::Definite(value) => AvailableSpace::Definite(Pixels(value)),
            TaffyAvailableSpace::MinContent => AvailableSpace::MinContent,
            TaffyAvailableSpace::MaxContent => AvailableSpace::MaxContent,
        }
    }
}

impl From<Pixels> for AvailableSpace {
    fn from(pixels: Pixels) -> Self {
        AvailableSpace::Definite(pixels)
    }
}

impl From<Size<Pixels>> for Size<AvailableSpace> {
    fn from(size: Size<Pixels>) -> Self {
        Size {
            width: AvailableSpace::Definite(size.width),
            height: AvailableSpace::Definite(size.height),
        }
    }
}

use super::engine::TaffyLayoutEngine;

impl TaffyLayoutEngine {
    // Used to understand performance
    #[allow(dead_code)]
    pub(super) fn count_all_children(&self, parent: LayoutId) -> anyhow::Result<u32> {
        let mut count = 0;

        for child in self.taffy.children(parent.0)? {
            // Count this child.
            count += 1;

            // Count all of this child's children.
            count += self.count_all_children(LayoutId(child))?
        }

        Ok(count)
    }

    // Used to understand performance
    #[allow(dead_code)]
    pub(super) fn max_depth(&self, depth: u32, parent: LayoutId) -> anyhow::Result<u32> {
        println!(
            "{parent:?} at depth {depth} has {} children",
            self.taffy.child_count(parent.0)
        );

        let mut max_child_depth = 0;

        for child in self.taffy.children(parent.0)? {
            max_child_depth = std::cmp::max(max_child_depth, self.max_depth(0, LayoutId(child))?);
        }

        Ok(depth + 1 + max_child_depth)
    }

    // Used to understand performance
    #[allow(dead_code)]
    pub(super) fn edges(&self, parent: LayoutId) -> anyhow::Result<Vec<(LayoutId, LayoutId)>> {
        let mut edges = Vec::new();

        for child in self.taffy.children(parent.0)? {
            edges.push((parent, LayoutId(child)));

            edges.extend(self.edges(LayoutId(child))?);
        }

        Ok(edges)
    }

    pub fn frame_metrics(&self) -> LayoutFrameMetrics {
        LayoutFrameMetrics {
            nodes: self.nodes_requested,
            measured_nodes: self.measured_nodes_requested,
            roots: self.roots_computed,
            bounds_cache_hits: self.bounds_cache_hits,
            bounds_cache_misses: self.bounds_cache_misses,
            cache_reused_roots: self.layout_cache_reused_roots,
            cache_saved_roots: self.layout_cache_saved_roots,
        }
    }

    pub fn layout_cache_metrics(&self) -> (usize, usize) {
        (self.layout_cache_hits, self.layout_cache_misses)
    }
}
