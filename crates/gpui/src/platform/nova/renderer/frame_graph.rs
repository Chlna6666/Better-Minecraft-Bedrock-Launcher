//! Resource-dependent Render Graph for Nova's GPU presentation owner.
//!
//! GPU backends still own native image layouts and memory barriers. This
//! logical graph derives producer/consumer dependencies and hazard *intents*
//! from declared reads/writes, rather than inferring safety from a fixed
//! sequence of render calls. Persistent textures never become alias candidates.
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Resource {
    PathMask,
    ElementColor,
    BackdropColor,
    Swapchain,
}

impl Resource {
    const fn index(self) -> usize {
        match self {
            Self::PathMask => 0,
            Self::ElementColor => 1,
            Self::BackdropColor => 2,
            Self::Swapchain => 3,
        }
    }

    /// All current filter targets are frame-persistent. Swapping their
    /// backing allocations with another in-flight frame would corrupt cached
    /// pixels; transient alias allocation is deliberately NOT enabled.
    const fn lifetime(self) -> Residency {
        match self {
            Self::PathMask | Self::ElementColor | Self::BackdropColor => Residency::Persistent,
            Self::Swapchain => Residency::ExternallyOwned,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Access {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Residency {
    Persistent,
    ExternallyOwned,
    /// Reserved for future per-submission scratch textures with GPU-fence
    /// scoped retirement. No currently allocated target uses this residency.
    Transient,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ResourceUse {
    pub(super) resource: Resource,
    pub(super) access: Access,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BarrierIntent {
    pub(super) resource: Resource,
    pub(super) producer: Pass,
    pub(super) consumer: Pass,
    pub(super) from: Access,
    pub(super) to: Access,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ResourceLifetime {
    pub(super) resource: Resource,
    /// Inclusive indices in the topologically ordered graph.
    pub(super) first: usize,
    pub(super) last: usize,
    pub(super) residency: Residency,
}

impl ResourceLifetime {
    /// An allocation may alias inside ONE already-fenced submission only
    /// when both scratch lifetimes are known not to overlap. A caller must
    /// separately prove the older GPU submission has retired before reuse.
    pub(super) fn can_alias_in_submission(self, other: Self) -> bool {
        self.resource != other.resource
            && self.residency == Residency::Transient
            && other.residency == Residency::Transient
            && (self.last < other.first || other.last < self.first)
    }
}

#[derive(Clone)]
struct Node {
    kind: Pass,
    dependencies: u8,
    accesses: SmallVec<[ResourceUse; 4]>,
}

impl Node {
    fn new(kind: Pass, accesses: impl IntoIterator<Item = ResourceUse>) -> Self {
        Self {
            kind,
            dependencies: 0,
            accesses: accesses.into_iter().collect(),
        }
    }
}

fn read(resource: Resource) -> ResourceUse {
    ResourceUse {
        resource,
        access: Access::Read,
    }
}

fn write(resource: Resource) -> ResourceUse {
    ResourceUse {
        resource,
        access: Access::Write,
    }
}

/// Topological order and dataflow metadata for a single presentation
/// submission. Native API barriers are still inserted by the gfx backend's
/// texture-pass transitions; these intents validate their required ordering.
pub(super) struct FrameGraphPlan {
    ordered: SmallVec<[Pass; 4]>,
    hazards: SmallVec<[BarrierIntent; 8]>,
    lifetimes: SmallVec<[ResourceLifetime; 4]>,
}

impl FrameGraphPlan {
    pub(super) fn compile(
        render_path_mask: bool,
        element_layer_count: usize,
        backdrop_group_count: usize,
    ) -> Self {
        let mut nodes = SmallVec::<[Node; 4]>::new();
        if render_path_mask {
            nodes.push(Node::new(Pass::PathMask, [write(Resource::PathMask)]));
        }
        if element_layer_count > 0 {
            let mut accesses = SmallVec::<[ResourceUse; 4]>::new();
            if render_path_mask {
                accesses.push(read(Resource::PathMask));
            }
            accesses.push(write(Resource::ElementColor));
            nodes.push(Node::new(Pass::ElementLayers, accesses));
        }
        if backdrop_group_count > 0 {
            let mut accesses = SmallVec::<[ResourceUse; 4]>::new();
            if render_path_mask {
                accesses.push(read(Resource::PathMask));
            }
            if element_layer_count > 0 {
                accesses.push(read(Resource::ElementColor));
            }
            accesses.push(write(Resource::BackdropColor));
            nodes.push(Node::new(Pass::BackdropBlur, accesses));
        }
        let mut main_accesses = SmallVec::<[ResourceUse; 4]>::new();
        if render_path_mask {
            main_accesses.push(read(Resource::PathMask));
        }
        if element_layer_count > 0 {
            main_accesses.push(read(Resource::ElementColor));
        }
        if backdrop_group_count > 0 {
            main_accesses.push(read(Resource::BackdropColor));
        }
        main_accesses.push(write(Resource::Swapchain));
        nodes.push(Node::new(Pass::MainPresent, main_accesses));

        // Track the most recent writer, plus reader dependencies before any
        // subsequent write. Read/Read never emits a GPU barrier intent.
        let mut last_access: [Option<(Pass, Access)>; 4] = [None; 4];
        let mut last_writer: [Option<Pass>; 4] = [None; 4];
        let mut readers: [u8; 4] = [0; 4];
        let mut hazards = SmallVec::<[BarrierIntent; 8]>::new();
        for node in &mut nodes {
            for usage in &node.accesses {
                let index = usage.resource.index();
                if let Some((producer, previous_access)) = last_access[index] {
                    if previous_access == Access::Write || usage.access == Access::Write {
                        hazards.push(BarrierIntent {
                            resource: usage.resource,
                            producer,
                            consumer: node.kind,
                            from: previous_access,
                            to: usage.access,
                        });
                    }
                }
                match usage.access {
                    Access::Read => {
                        if let Some(writer) = last_writer[index] {
                            node.dependencies |= writer.bit();
                        }
                        readers[index] |= node.kind.bit();
                    }
                    Access::Write => {
                        if let Some(writer) = last_writer[index] {
                            node.dependencies |= writer.bit();
                        }
                        node.dependencies |= readers[index];
                        readers[index] = 0;
                        last_writer[index] = Some(node.kind);
                    }
                }
                last_access[index] = Some((node.kind, usage.access));
            }
        }

        let active = nodes.iter().fold(0_u8, |bits, node| bits | node.kind.bit());
        let mut completed = 0_u8;
        let mut ordered = SmallVec::<[Pass; 4]>::new();
        while ordered.len() < nodes.len() {
            let next = nodes
                .iter()
                .find(|node| {
                    completed & node.kind.bit() == 0
                        && (node.dependencies & active) & !completed == 0
                })
                .expect("Nova frame graph contains a resource dependency cycle");
            ordered.push(next.kind);
            completed |= next.kind.bit();
        }

        let mut lifetimes = SmallVec::<[ResourceLifetime; 4]>::new();
        for (position, pass) in ordered.iter().copied().enumerate() {
            let node = nodes.iter().find(|node| node.kind == pass).expect("graph node");
            for usage in &node.accesses {
                if let Some(previous) = lifetimes
                    .iter_mut()
                    .find(|life| life.resource == usage.resource)
                {
                    previous.last = position;
                } else {
                    lifetimes.push(ResourceLifetime {
                        resource: usage.resource,
                        first: position,
                        last: position,
                        residency: usage.resource.lifetime(),
                    });
                }
            }
        }
        Self {
            ordered,
            hazards,
            lifetimes,
        }
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

    pub(super) fn hazard_count(&self) -> usize {
        self.hazards.len()
    }

    pub(super) fn resource_count(&self) -> usize {
        self.lifetimes.len()
    }

    #[cfg(test)]
    fn lifetime(&self, resource: Resource) -> Option<ResourceLifetime> {
        self.lifetimes.iter().copied().find(|life| life.resource == resource)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_scene_requires_only_main_swapchain_pass() {
        let graph = FrameGraphPlan::compile(false, 0, 0);
        assert_eq!(graph.ordered.as_slice(), &[Pass::MainPresent]);
        assert_eq!(graph.hazard_count(), 0);
        assert_eq!(graph.resource_count(), 1);
    }

    #[test]
    fn independent_layer_capture_is_not_forced_by_a_clean_backdrop() {
        let graph = FrameGraphPlan::compile(false, 1, 0);
        assert_eq!(graph.ordered.as_slice(), &[Pass::ElementLayers, Pass::MainPresent]);
        assert!(!graph.requires(Pass::BackdropBlur));
        assert_eq!(graph.hazard_count(), 1);
    }

    #[test]
    fn dataflow_derives_producer_before_consumer() {
        let graph = FrameGraphPlan::compile(true, 3, 2);
        assert_eq!(
            graph.ordered.as_slice(),
            &[Pass::PathMask, Pass::ElementLayers, Pass::BackdropBlur, Pass::MainPresent],
        );
        assert!(graph.hazards.iter().any(|hazard| {
            hazard.resource == Resource::ElementColor
                && hazard.producer == Pass::ElementLayers
                && hazard.consumer == Pass::BackdropBlur
                && hazard.from == Access::Write
                && hazard.to == Access::Read
        }));
        assert_eq!(graph.lifetime(Resource::PathMask).expect("mask").first, 0);
        assert_eq!(graph.lifetime(Resource::PathMask).expect("mask").last, 3);
    }

    #[test]
    fn no_read_after_read_hazard_or_duplicate_filter_passes() {
        let graph = FrameGraphPlan::compile(false, 0, 2);
        assert_eq!(graph.ordered.as_slice(), &[Pass::BackdropBlur, Pass::MainPresent]);
        assert_eq!(graph.hazard_count(), 1);
    }

    #[test]
    fn persistent_and_in_flight_resources_must_never_alias() {
        let graph = FrameGraphPlan::compile(true, 1, 1);
        let mask = graph.lifetime(Resource::PathMask).expect("mask");
        let element = graph.lifetime(Resource::ElementColor).expect("element");
        assert!(!mask.can_alias_in_submission(element));
        let transient_a = ResourceLifetime {
            resource: Resource::PathMask,
            first: 0,
            last: 1,
            residency: Residency::Transient,
        };
        let transient_b = ResourceLifetime {
            resource: Resource::ElementColor,
            first: 2,
            last: 3,
            residency: Residency::Transient,
        };
        assert!(transient_a.can_alias_in_submission(transient_b));
        assert!(
            !(ResourceLifetime {
                first: 1,
                ..transient_b
            })
            .can_alias_in_submission(transient_a)
        );
        assert!(
            !(ResourceLifetime {
                resource: transient_a.resource,
                ..transient_b
            })
            .can_alias_in_submission(transient_a)
        );
    }
}
