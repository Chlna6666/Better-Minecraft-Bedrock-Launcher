//! Render Graph scheduling for the retained Nova presentation lane.
//!
//! Each node names a real backend pass (or ordered batch of passes). Dependency edges
//! preserve the current painter contract: a mask must precede any sampled path,
//! retained element targets must precede backdrops that can sample them, and the
//! swapchain reads only successfully produced offscreen targets.
//! This planner never assumes a rotating swapchain image can retain stale pixels.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(super) enum Pass {
    PathMask,
    ElementLayers,
    BackdropBlur,
    MainPresent,
}

impl Pass {
    const fn bit(self) -> u8 {
        match self {
            Self::PathMask => 1 << 0,
            Self::ElementLayers => 1 << 1,
            Self::BackdropBlur => 1 << 2,
            Self::MainPresent => 1 << 3,
        }
    }
}

/// One graph node's direct dependencies. A dependency is active only if both
/// corresponding passes are present in the compiled graph.
#[derive(Clone, Copy, Debug)]
struct Node {
    kind: Pass,
    dependencies: u8,
}

/// The small DAG is compiled once for the current presentation packet; no
/// per-backend scheduler or native resource ownership is created here.
pub(super) struct FrameGraphPlan {
    ordered: SmallVec<[Pass; 4]>,
}

impl FrameGraphPlan {
    pub(super) fn compile(
        render_path_mask: bool,
        element_layer_count: usize,
        backdrop_group_count: usize,
    ) -> Self {
        let mut nodes = SmallVec::<[Node; 4]>::new();
        if render_path_mask {
            nodes.push(Node {
                kind: Pass::PathMask,
                dependencies: 0,
            });
        }
        if element_layer_count > 0 {
            nodes.push(Node {
                kind: Pass::ElementLayers,
                dependencies: Pass::PathMask.bit(),
            });
        }
        if backdrop_group_count > 0 {
            nodes.push(Node {
                kind: Pass::BackdropBlur,
                dependencies: Pass::PathMask.bit() | Pass::ElementLayers.bit(),
            });
        }
        // All dependency edges into MainPresent are read-after-write barriers.
        // Backend submission may still batch the source/filter passes internally.
        nodes.push(Node {
            kind: Pass::MainPresent,
            dependencies: Pass::PathMask.bit()
                | Pass::ElementLayers.bit()
                | Pass::BackdropBlur.bit(),
        });

        let active = nodes.iter().fold(0_u8, |mask, node| mask | node.kind.bit());
        let mut completed = 0_u8;
        let mut ordered = SmallVec::<[Pass; 4]>::new();
        while ordered.len() < nodes.len() {
            let next = nodes
                .iter()
                .find(|node| {
                    completed & node.kind.bit() == 0
                        && (node.dependencies & active) & !completed == 0
                })
                .expect("Nova frame graph contains a dependency cycle");
            ordered.push(next.kind);
            completed |= next.kind.bit();
        }
        Self { ordered }
    }

    pub(super) fn offscreen_passes(&self) -> impl Iterator<Item = Pass> + '_ {
        self.ordered
            .iter()
            .copied()
            .take_while(|kind| *kind != Pass::MainPresent)
    }

    pub(super) fn requires(&self, pass: Pass) -> bool {
        self.ordered.contains(&pass)
    }

    pub(super) fn node_count(&self) -> usize {
        self.ordered.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_scene_requires_only_main_swapchain_pass() {
        let graph = FrameGraphPlan::compile(false, 0, 0);
        assert_eq!(graph.ordered.as_slice(), &[Pass::MainPresent]);
        assert_eq!(graph.offscreen_passes().count(), 0);
    }

    #[test]
    fn independent_layer_capture_is_not_forced_by_a_clean_backdrop() {
        let graph = FrameGraphPlan::compile(false, 1, 0);
        assert_eq!(
            graph.ordered.as_slice(),
            &[Pass::ElementLayers, Pass::MainPresent],
        );
        assert!(!graph.requires(Pass::BackdropBlur));
    }

    #[test]
    fn dependency_chain_preserves_painter_read_after_write_order() {
        let graph = FrameGraphPlan::compile(true, 3, 2);
        assert_eq!(
            graph.ordered.as_slice(),
            &[
                Pass::PathMask,
                Pass::ElementLayers,
                Pass::BackdropBlur,
                Pass::MainPresent,
            ],
        );
        assert_eq!(graph.node_count(), 4);
    }

    #[test]
    fn filtered_backdrop_without_element_capture_skips_that_pass() {
        let graph = FrameGraphPlan::compile(false, 0, 2);
        assert_eq!(
            graph.ordered.as_slice(),
            &[Pass::BackdropBlur, Pass::MainPresent],
        );
    }
}
