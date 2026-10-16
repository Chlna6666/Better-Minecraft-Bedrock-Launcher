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
    RootColor,
    BackdropBlur,
    MainPresent,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum Resource {
    PathMask,
    ElementColor,
    RootColor,
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
            Self::RootColor => 4,
        }
    }

    /// All current filter targets are frame-persistent. Swapping their
    /// backing allocations with another in-flight frame would corrupt cached
    /// pixels; transient alias allocation is deliberately NOT enabled.
    const fn lifetime(self) -> Residency {
        match self {
            Self::PathMask | Self::ElementColor | Self::BackdropColor | Self::RootColor => Residency::Persistent,
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

/// Topological order and dataflow metadata for a single presentation
/// submission. Native API barriers are still inserted by the gfx backend's
/// texture-pass transitions; these intents validate their required ordering.
pub(super) struct FrameGraphPlan {
    ordered: SmallVec<[Pass; 5]>,
    hazards: SmallVec<[BarrierIntent; 8]>,
    lifetimes: SmallVec<[ResourceLifetime; 5]>,
}

/// Root residency is independent of Filter refresh and native backbuffer validity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RootColorAccess {
    Absent,
    Resident,
    Refresh,
}

impl FrameGraphPlan {
    pub(super) fn compile(
        render_path_mask: bool,
        element_layer_count: usize,
        backdrop_group_count: usize,
        root_color: RootColorAccess,
    ) -> Self {
        // Root raster has its own producer and lifetime. Cached Root sampling
        // remains a read even on frames that submit only the native Present sink.
        let has_elements = element_layer_count != 0;
        let has_backdrop = backdrop_group_count != 0;
        let mut ordered = SmallVec::<[Pass; 5]>::new();
        if render_path_mask {
            ordered.push(Pass::PathMask);
        }
        if has_elements {
            ordered.push(Pass::ElementLayers);
        }
        if root_color == RootColorAccess::Refresh {
            ordered.push(Pass::RootColor);
        }
        if has_backdrop {
            ordered.push(Pass::BackdropBlur);
        }
        ordered.push(Pass::MainPresent);

        // Most static/retained frames need only the swapchain sink. Keep this
        // on the same graph representation, without per-frame dependency
        // construction or a second cached copy of graph state.
        if ordered.len() == 1 && root_color == RootColorAccess::Absent {
            let mut lifetimes = SmallVec::new();
            lifetimes.push(ResourceLifetime {
                resource: Resource::Swapchain,
                first: 0,
                last: 0,
                residency: Residency::ExternallyOwned,
            });
            return Self {
                ordered,
                hazards: SmallVec::new(),
                lifetimes,
            };
        }

        // Resolve the fixed read/write declarations in painter order. Native
        // graphics backends still own barriers; these are graph-level hazard
        // intents and persistent residency lifetimes, not GPU allocations.
        let mut last_access: [Option<(Pass, Access)>; 5] = [None; 5];
        let mut resources: [Option<ResourceLifetime>; 5] = [None; 5];
        let mut hazards = SmallVec::<[BarrierIntent; 8]>::new();
        for (position, pass) in ordered.iter().copied().enumerate() {
            let mut access = |resource: Resource, current: Access| {
                let index = resource.index();
                if let Some((producer, previous)) = last_access[index]
                    && (previous == Access::Write || current == Access::Write)
                {
                    hazards.push(BarrierIntent {
                        resource,
                        producer,
                        consumer: pass,
                        from: previous,
                        to: current,
                    });
                }
                last_access[index] = Some((pass, current));
                if let Some(lifetime) = &mut resources[index] {
                    lifetime.last = position;
                } else {
                    resources[index] = Some(ResourceLifetime {
                        resource,
                        first: position,
                        last: position,
                        residency: resource.lifetime(),
                    });
                }
            };

            match pass {
                Pass::PathMask => access(Resource::PathMask, Access::Write),
                Pass::ElementLayers => {
                    if render_path_mask {
                        access(Resource::PathMask, Access::Read);
                    }
                    access(Resource::ElementColor, Access::Write);
                }
                Pass::RootColor => {
                    if render_path_mask {
                        access(Resource::PathMask, Access::Read);
                    }
                    if has_elements {
                        access(Resource::ElementColor, Access::Read);
                    }
                    access(Resource::RootColor, Access::Write);
                }
                Pass::BackdropBlur => {
                    if render_path_mask {
                        access(Resource::PathMask, Access::Read);
                    }
                    if has_elements {
                        access(Resource::ElementColor, Access::Read);
                    }
                    access(Resource::BackdropColor, Access::Write);
                }
                Pass::MainPresent => {
                    if root_color != RootColorAccess::Absent {
                        access(Resource::RootColor, Access::Read);
                    }
                    if render_path_mask {
                        access(Resource::PathMask, Access::Read);
                    }
                    if has_elements {
                        access(Resource::ElementColor, Access::Read);
                    }
                    if has_backdrop {
                        access(Resource::BackdropColor, Access::Read);
                    }
                    access(Resource::Swapchain, Access::Write);
                }
            }
        }
        let lifetimes = resources.into_iter().flatten().collect();
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
        let graph = FrameGraphPlan::compile(false, 0, 0, RootColorAccess::Absent);
        assert_eq!(graph.ordered.as_slice(), &[Pass::MainPresent]);
        assert_eq!(graph.hazard_count(), 0);
        assert_eq!(graph.resource_count(), 1);
    }

    #[test]
    fn resident_root_is_read_without_filter_or_root_raster_pass() {
        let graph = FrameGraphPlan::compile(false, 0, 0, RootColorAccess::Resident);
        assert_eq!(graph.ordered.as_slice(), &[Pass::MainPresent]);
        assert!(!graph.requires(Pass::RootColor));
        assert!(!graph.requires(Pass::ElementLayers));
        assert_eq!(graph.hazard_count(), 0);
        assert_eq!(graph.lifetime(Resource::RootColor).unwrap().residency, Residency::Persistent);
    }

    #[test]
    fn root_raster_samples_element_outputs_before_final_composite() {
        let graph = FrameGraphPlan::compile(true, 2, 0, RootColorAccess::Refresh);
        assert_eq!(graph.ordered.as_slice(), &[
            Pass::PathMask, Pass::ElementLayers, Pass::RootColor, Pass::MainPresent,
        ]);
        assert!(graph.hazards.iter().any(|hazard| {
            hazard.resource == Resource::ElementColor
                && hazard.producer == Pass::ElementLayers
                && hazard.consumer == Pass::RootColor
        }));
        assert!(graph.hazards.iter().any(|hazard| {
            hazard.resource == Resource::RootColor
                && hazard.producer == Pass::RootColor
                && hazard.consumer == Pass::MainPresent
        }));
    }

    #[test]
    fn every_offscreen_topology_preserves_order_and_resource_lifetimes() {
        for path in [false, true] {
            for elements in [false, true] {
                for backdrop in [false, true] {
                    let graph = FrameGraphPlan::compile(
                        path,
                        usize::from(elements),
                        usize::from(backdrop),
                        RootColorAccess::Absent,
                    );
                    let mut expected = Vec::new();
                    if path {
                        expected.push(Pass::PathMask);
                    }
                    if elements {
                        expected.push(Pass::ElementLayers);
                    }
                    if backdrop {
                        expected.push(Pass::BackdropBlur);
                    }
                    expected.push(Pass::MainPresent);
                    assert_eq!(graph.ordered.as_slice(), expected.as_slice());
                    let last = expected.len() - 1;
                    let offscreen_count =
                        usize::from(path) + usize::from(elements) + usize::from(backdrop);
                    assert_eq!(graph.hazard_count(), offscreen_count);
                    assert_eq!(graph.resource_count(), offscreen_count + 1);
                    let swapchain = graph.lifetime(Resource::Swapchain).expect("swapchain");
                    assert_eq!((swapchain.first, swapchain.last), (last, last));
                    assert_eq!(swapchain.residency, Residency::ExternallyOwned);
                    for (enabled, resource, first) in [
                        (path, Resource::PathMask, 0),
                        (elements, Resource::ElementColor, usize::from(path)),
                        (
                            backdrop,
                            Resource::BackdropColor,
                            usize::from(path) + usize::from(elements),
                        ),
                    ] {
                        assert_eq!(graph.lifetime(resource).is_some(), enabled);
                        if enabled {
                            let lifetime = graph.lifetime(resource).expect("resident target");
                            assert_eq!((lifetime.first, lifetime.last), (first, last));
                            assert_eq!(lifetime.residency, Residency::Persistent);
                        }
                    }
                    assert!(graph.hazards.iter().all(|intent| {
                        expected.iter().position(|pass| *pass == intent.producer)
                            < expected.iter().position(|pass| *pass == intent.consumer)
                    }));
                }
            }
        }
    }

    #[test]
    fn read_after_read_does_not_add_extra_mask_barriers() {
        let graph = FrameGraphPlan::compile(true, 1, 1, RootColorAccess::Absent);
        let mask_hazards: Vec<_> = graph.hazards.iter()
            .filter(|intent| intent.resource == Resource::PathMask)
            .collect();
        assert_eq!(mask_hazards.len(), 1);
        assert_eq!(mask_hazards[0].producer, Pass::PathMask);
        assert_eq!(mask_hazards[0].consumer, Pass::ElementLayers);
        assert_eq!(mask_hazards[0].from, Access::Write);
        assert_eq!(mask_hazards[0].to, Access::Read);
    }

    #[test]
    fn independent_layer_capture_is_not_forced_by_a_clean_backdrop() {
        let graph = FrameGraphPlan::compile(false, 1, 0, RootColorAccess::Absent);
        assert_eq!(graph.ordered.as_slice(), &[Pass::ElementLayers, Pass::MainPresent]);
        assert!(!graph.requires(Pass::BackdropBlur));
        assert_eq!(graph.hazard_count(), 1);
    }

    #[test]
    fn dataflow_derives_producer_before_consumer() {
        let graph = FrameGraphPlan::compile(true, 3, 2, RootColorAccess::Absent);
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
        let graph = FrameGraphPlan::compile(false, 0, 2, RootColorAccess::Absent);
        assert_eq!(graph.ordered.as_slice(), &[Pass::BackdropBlur, Pass::MainPresent]);
        assert_eq!(graph.hazard_count(), 1);
    }

    #[test]
    fn persistent_and_in_flight_resources_must_never_alias() {
        let graph = FrameGraphPlan::compile(true, 1, 1, RootColorAccess::Absent);
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
