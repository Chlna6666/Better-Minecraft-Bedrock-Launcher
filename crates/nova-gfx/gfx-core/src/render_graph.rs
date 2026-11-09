//! Backend-neutral GPU work graph resource dependencies.
//!
//! Passes declare logical resource access; this layer rejects reads before
//! production and identifies RAW/WAR/WAW boundaries across Graphics, Compute
//! and Transfer. It never submits work or blocks the host. Native backends
//! consume the resulting transition plan when ordered mixed-queue execution
//! is available; until then callers must NOT silently execute a dependent
//! compute stage early.
//!
//! A ResourceId represents the complete allocation, not one TextureViewId:
//! separate views of the same texture must synchronize against one another.
use std::collections::{HashMap, HashSet};
use crate::{BufferId, Error, Result, TextureId};

/// Complete allocation identity. A mip view cannot escape an aliasing hazard.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum GpuResource {
    Buffer(BufferId),
    Texture(TextureId),
}

/// Native command domain. These are execution stages, not implicit queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuWorkStage {
    Graphics,
    Compute,
    Transfer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuAccess {
    /// Shader read without writes.
    Sampled,
    /// Read-only uniform or storage buffer.
    ReadBuffer,
    /// Indirect draw arguments.
    Indirect,
    /// Color attachment output.
    ColorAttachment,
    /// Depth attachment output.
    DepthAttachment,
    /// Writable compute storage buffer or image.
    StorageWrite,
    /// Storage read/modify/write requiring explicit alias permission.
    StorageReadWrite,
    /// Transfer source.
    CopyRead,
    /// Transfer destination.
    CopyWrite,
}

impl GpuAccess {
    pub const fn reads(self) -> bool {
        matches!(
            self,
            Self::Sampled | Self::ReadBuffer | Self::Indirect
                | Self::StorageReadWrite | Self::CopyRead
        )
    }
    pub const fn writes(self) -> bool {
        matches!(
            self,
            Self::ColorAttachment | Self::DepthAttachment | Self::StorageWrite
                | Self::StorageReadWrite | Self::CopyWrite
        )
    }
}

/// Resource use in an ordered pass; whole-texture aliasing is intentional.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuResourceUse {
    pub resource: GpuResource,
    pub access: GpuAccess,
}

/// One ordered render or compute pass.
#[derive(Clone, Debug)]
pub struct GpuWorkPass {
    pub label: String,
    pub stage: GpuWorkStage,
    pub resources: Vec<GpuResourceUse>,
}

/// Resource transition needed before a pass begins. Backends must translate
/// into API-specific barriers; the planner does not claim to insert them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuTransition {
    pub resource: GpuResource,
    pub from_pass: usize,
    pub to_pass: usize,
    pub from_stage: GpuWorkStage,
    pub to_stage: GpuWorkStage,
    pub before: GpuAccess,
    pub after: GpuAccess,
}

#[derive(Default, Debug)]
pub struct GpuWorkGraph {
    imports: HashSet<GpuResource>,
    passes: Vec<GpuWorkPass>,
}

#[derive(Debug)]
pub struct GpuWorkPlan {
    pub passes: Vec<GpuWorkPass>,
    pub transitions: Vec<GpuTransition>,
}

/// Native command encoder for an already-compiled, ordered GPU work plan.
///
/// An implementation must encode every pass onto the owning GPU device, in
/// program order. `apply_transitions` must make the listed writes visible to
/// their consumers using the backend's native state/barrier rules. Returning
/// success without recording required synchronization is invalid. The methods
/// enqueue GPU work; neither method may perform a CPU fence wait.
pub trait GpuWorkExecutor {
    /// Encode native barriers/state transitions before `pass` starts.
    /// Called only when the transition slice is nonempty.
    fn apply_transitions(&mut self, transitions: &[GpuTransition]) -> Result<()>;

    /// Encode a pass. Its index matches the compiled graph's pass index.
    fn execute_pass(&mut self, index: usize, pass: &GpuWorkPass) -> Result<()>;
}

impl GpuWorkPlan {
    /// Encode the compiled plan in dependency order on a native GPU owner.
    ///
    /// The executor must provide native Graphics/Compute/Transfer recording
    /// and the transitions between them. No fallback may reorder a dependent
    /// Compute pass before its Graphics producer. This only records work; it
    /// does not submit a queue, wait for a fence, or mark resources retired.
    ///
    /// A failure stops encoding immediately. Previously encoded work may
    /// still be pending, and callers must not publish output from a failed
    /// plan. The transition cursor preserves the compile-time order and
    /// avoids per-pass sorting or temporary allocations.
    pub fn encode_ordered(&self, executor: &mut impl GpuWorkExecutor) -> Result<()> {
        let mut transition_cursor = 0;
        for (index, pass) in self.passes.iter().enumerate() {
            let start = transition_cursor;
            while transition_cursor < self.transitions.len()
                && self.transitions[transition_cursor].to_pass == index
            {
                transition_cursor += 1;
            }
            if start != transition_cursor {
                executor.apply_transitions(&self.transitions[start..transition_cursor])?;
            }
            executor.execute_pass(index, pass)?;
        }
        Ok(())
    }

    /// Validate the currently supported extension execution model: independent
    /// compute may precede recorded graphics passes, but graphics -> compute
    /// dependencies need native ordered mixed-pass recording and barriers.
    ///
    /// Until a backend implements mixed GPU work submission, a shadow-dependent
    /// Froxel injection pass must fail here instead of silently consuming a
    /// shadow map that has not been rendered yet. This never waits on the CPU.
    pub fn validate_pregraphics_compute_only(&self) -> Result<()> {
        if self.transitions.iter().any(|edge| {
            edge.from_stage == GpuWorkStage::Graphics
                && edge.to_stage == GpuWorkStage::Compute
        }) {
            return Err(Error::Unavailable(
                "GPU frame graph requires native Graphics->Compute ordering before Froxel dispatch"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Count transitions that change command domains, for audit/telemetry.
    pub fn cross_stage_transitions(&self) -> usize {
        self.transitions.iter()
            .filter(|edge| edge.from_stage != edge.to_stage)
            .count()
    }
}

impl GpuWorkGraph {
    pub fn new() -> Self { Self::default() }

    /// Import data produced by an earlier frame or an external producer.
    pub fn import(&mut self, resource: GpuResource) {
        self.imports.insert(resource);
    }

    /// Record ordered work. Two views of one texture require one explicit
    /// ReadWrite access; declaring separate read/write entries is an error.
    pub fn push(
        &mut self,
        label: impl Into<String>,
        stage: GpuWorkStage,
        resources: impl IntoIterator<Item = GpuResourceUse>,
    ) -> Result<usize> {
        let resources = resources.into_iter().collect::<Vec<_>>();
        let mut used = HashSet::new();
        for item in &resources {
            // The work graph describes logical command domains, not native
            // queue families. Invalid combinations must fail before a backend
            // attempts to translate them into native resource states.
            let valid_stage = match stage {
                GpuWorkStage::Graphics => !matches!(
                    item.access,
                    GpuAccess::CopyRead | GpuAccess::CopyWrite
                ),
                GpuWorkStage::Compute => !matches!(
                    item.access,
                    GpuAccess::ColorAttachment | GpuAccess::DepthAttachment
                        | GpuAccess::CopyRead | GpuAccess::CopyWrite
                ),
                GpuWorkStage::Transfer => matches!(
                    item.access,
                    GpuAccess::CopyRead | GpuAccess::CopyWrite
                ),
            };
            if !valid_stage {
                return Err(Error::InvalidInput(format!(
                    "GPU work graph {:?} pass uses incompatible {:?} access",
                    stage, item.access
                )));
            }
            if !used.insert(item.resource) {
                return Err(Error::InvalidInput(
                    "GPU work graph binds one allocation more than once in a pass".into(),
                ));
            }
        }
        let index = self.passes.len();
        self.passes.push(GpuWorkPass {label:label.into(),stage,resources});
        Ok(index)
    }

    /// Verify production and enumerate exact read-after-write,
    /// write-after-read and write-after-write dependencies.
    ///
    /// All resource identities include allocation generations. Unknown reads
    /// fail closed instead of sampling undefined GPU memory. Unused imported
    /// outputs are harmless (e.g. retained shadows on an unlit frame).
    pub fn compile(self) -> Result<GpuWorkPlan> {
        type Use = (usize, GpuWorkStage, GpuAccess);
        #[derive(Default)]
        struct Tracker {
            writer: Option<Use>,
            readers: Vec<Use>,
        }
        let mut available = self.imports;
        let mut tracked: HashMap<GpuResource, Tracker> = HashMap::new();
        let mut transitions = Vec::new();
        for (index,pass) in self.passes.iter().enumerate() {
            for use_ in &pass.resources {
                if use_.access.reads() && !available.contains(&use_.resource) {
                    return Err(Error::InvalidInput(format!(
                        "GPU work graph pass '{}' reads a resource before a producer or import",
                        pass.label
                    )));
                }
                let state = tracked.entry(use_.resource).or_default();
                let mut depend = |(before_pass, before_stage, before): Use| {
                    transitions.push(GpuTransition {
                        resource: use_.resource,
                        from_pass: before_pass,
                        to_pass: index,
                        from_stage: before_stage,
                        to_stage: pass.stage,
                        before,
                        after: use_.access,
                    });
                };
                if use_.access.writes() {
                    // A write must wait for every outstanding reader, not just
                    // the most recently recorded one. On separate native GPU
                    // queues they may all still be in flight simultaneously.
                    if let Some(writer) = state.writer {
                        depend(writer);
                    }
                    for &reader in &state.readers {
                        depend(reader);
                    }
                    state.readers.clear();
                    state.writer = Some((index,pass.stage,use_.access));
                    available.insert(use_.resource);
                } else if use_.access.reads() {
                    if let Some(writer) = state.writer {
                        depend(writer);
                    }
                    state.readers.push((index,pass.stage,use_.access));
                }
            }
        }
        Ok(GpuWorkPlan {passes:self.passes,transitions})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tex(index:u32)->GpuResource {
        GpuResource::Texture(TextureId::from_parts(index,1))
    }
    fn buf(index:u32)->GpuResource {
        GpuResource::Buffer(BufferId::from_parts(index,1))
    }
    fn use_(resource:GpuResource,access:GpuAccess)->GpuResourceUse {
        GpuResourceUse {resource,access}
    }

    #[test]
    fn rejects_resource_access_in_incompatible_command_domains() {
        let invalid = [
            (GpuWorkStage::Compute, GpuAccess::ColorAttachment),
            (GpuWorkStage::Compute, GpuAccess::DepthAttachment),
            (GpuWorkStage::Compute, GpuAccess::CopyWrite),
            (GpuWorkStage::Graphics, GpuAccess::CopyRead),
            (GpuWorkStage::Transfer, GpuAccess::Sampled),
            (GpuWorkStage::Transfer, GpuAccess::StorageWrite),
        ];
        for (stage, access) in invalid {
            let mut graph = GpuWorkGraph::new();
            assert!(matches!(
                graph.push("invalid domain", stage, [use_(tex(1), access)]),
                Err(Error::InvalidInput(_))
            ));
        }
        let mut graph = GpuWorkGraph::new();
        graph.push("upload", GpuWorkStage::Transfer,
            [use_(tex(1), GpuAccess::CopyWrite)]).unwrap();
        graph.push("sample", GpuWorkStage::Compute,
            [use_(tex(1), GpuAccess::Sampled)]).unwrap();
        let plan = graph.compile().unwrap();
        assert_eq!(plan.transitions.len(), 1);
        assert_eq!(plan.transitions[0].from_stage, GpuWorkStage::Transfer);
        assert_eq!(plan.transitions[0].to_stage, GpuWorkStage::Compute);
    }

    #[test]
    fn ordered_encoder_emits_barriers_before_consumers_without_reordering() {
        #[derive(Default)]
        struct Trace(Vec<String>);
        impl GpuWorkExecutor for Trace {
            fn apply_transitions(&mut self, edges: &[GpuTransition]) -> Result<()> {
                assert!(edges.iter().all(|edge| edge.to_pass == edges[0].to_pass));
                self.0.push(format!("barrier:{}:{}", edges[0].to_pass, edges.len()));
                Ok(())
            }
            fn execute_pass(&mut self, index: usize, _: &GpuWorkPass) -> Result<()> {
                self.0.push(format!("pass:{index}"));
                Ok(())
            }
        }
        let mut graph = GpuWorkGraph::new();
        graph.push("upload", GpuWorkStage::Transfer,
            [use_(buf(1), GpuAccess::CopyWrite)]).unwrap();
        graph.push("unrelated", GpuWorkStage::Compute,
            [use_(buf(2), GpuAccess::StorageWrite)]).unwrap();
        graph.push("consume", GpuWorkStage::Compute, [
            use_(buf(1), GpuAccess::ReadBuffer),
            use_(buf(2), GpuAccess::ReadBuffer),
        ]).unwrap();
        let plan = graph.compile().unwrap();
        let mut trace = Trace::default();
        plan.encode_ordered(&mut trace).unwrap();
        assert_eq!(trace.0, [
            "pass:0", "pass:1", "barrier:2:2", "pass:2",
        ]);
    }

    #[test]
    fn ordered_encoder_stops_before_consumer_if_native_barrier_fails() {
        struct RejectBarrier { passes: usize }
        impl GpuWorkExecutor for RejectBarrier {
            fn apply_transitions(&mut self, _: &[GpuTransition]) -> Result<()> {
                Err(Error::Unavailable("native barrier unsupported".into()))
            }
            fn execute_pass(&mut self, _: usize, _: &GpuWorkPass) -> Result<()> {
                self.passes += 1;
                Ok(())
            }
        }
        let mut graph = GpuWorkGraph::new();
        graph.push("produce", GpuWorkStage::Graphics,
            [use_(tex(1), GpuAccess::ColorAttachment)]).unwrap();
        graph.push("consume", GpuWorkStage::Compute,
            [use_(tex(1), GpuAccess::Sampled)]).unwrap();
        let mut executor = RejectBarrier { passes: 0 };
        assert!(matches!(
            graph.compile().unwrap().encode_ordered(&mut executor),
            Err(Error::Unavailable(_))
        ));
        assert_eq!(executor.passes, 1);
    }

    #[test]
    fn graphics_compute_graph_detects_cross_stage_hazards_without_cpu_waits() {
        let mut graph=GpuWorkGraph::new();
        graph.push("shadow",GpuWorkStage::Graphics,
            [use_(tex(1),GpuAccess::DepthAttachment)]).unwrap();
        graph.push("fog injection",GpuWorkStage::Compute,[
            use_(tex(1),GpuAccess::Sampled),
            use_(buf(2),GpuAccess::StorageWrite),
        ]).unwrap();
        graph.push("fog integration",GpuWorkStage::Compute,[
            use_(buf(2),GpuAccess::StorageReadWrite),
        ]).unwrap();
        graph.push("compositor",GpuWorkStage::Graphics,[
            use_(buf(2),GpuAccess::ReadBuffer),
            use_(tex(3),GpuAccess::ColorAttachment),
        ]).unwrap();
        let plan=graph.compile().unwrap();
        assert_eq!(plan.passes.len(),4);
        assert_eq!(plan.transitions.len(),3);
        assert_eq!(plan.transitions[0].from_stage,GpuWorkStage::Graphics);
        assert_eq!(plan.transitions[0].to_stage,GpuWorkStage::Compute);
        assert_eq!(plan.transitions[2].to_stage,GpuWorkStage::Graphics);
    }

    #[test]
    fn rejects_uninitialized_froxel_history_without_previous_frame_import() {
        let mut graph=GpuWorkGraph::new();
        graph.push("fog temporal reprojection",GpuWorkStage::Compute,
            [use_(tex(20),GpuAccess::Sampled)]).unwrap();
        assert!(matches!(graph.compile(),Err(Error::InvalidInput(_))));
    }

    #[test]
    fn imported_history_survives_a_ping_pong_update() {
        let mut graph=GpuWorkGraph::new();
        graph.import(tex(20));
        graph.push("fog reproject",GpuWorkStage::Compute,[
            use_(tex(20),GpuAccess::Sampled),
            use_(tex(21),GpuAccess::StorageWrite)
        ]).unwrap();
        graph.push("fog composite",GpuWorkStage::Graphics,[
            use_(tex(21),GpuAccess::Sampled),
            use_(tex(22),GpuAccess::ColorAttachment)
        ]).unwrap();
        assert_eq!(graph.compile().unwrap().transitions.len(),1);
    }

    #[test]
    fn blocks_same_image_alias_by_distinct_view_bindings() {
        let mut graph=GpuWorkGraph::new();
        graph.import(tex(4));
        assert!(matches!(
            graph.push("bad mip feedback",GpuWorkStage::Compute,[
                use_(tex(4),GpuAccess::Sampled),
                use_(tex(4),GpuAccess::StorageWrite)
            ]),
            Err(Error::InvalidInput(_))
        ));
    }

    #[test]
    fn write_waits_for_all_parallel_readers_in_distinct_work_domains() {
        let mut graph=GpuWorkGraph::new();
        graph.import(tex(8));
        graph.push("raster sample",GpuWorkStage::Graphics,
            [use_(tex(8),GpuAccess::Sampled)]).unwrap();
        graph.push("compute sample",GpuWorkStage::Compute,
            [use_(tex(8),GpuAccess::Sampled)]).unwrap();
        graph.push("next-frame write",GpuWorkStage::Compute,
            [use_(tex(8),GpuAccess::StorageWrite)]).unwrap();
        let plan=graph.compile().unwrap();
        assert_eq!(plan.transitions.len(),2);
        assert_eq!(plan.transitions[0].from_pass,0);
        assert_eq!(plan.transitions[1].from_pass,1);
    }

    #[test]
    fn pregraphics_compute_rejects_shadow_dependent_froxel_until_native_support() {
        let mut graph=GpuWorkGraph::new();
        graph.push("CSM",GpuWorkStage::Graphics,
            [use_(tex(1),GpuAccess::DepthAttachment)]).unwrap();
        graph.push("froxel light injection",GpuWorkStage::Compute,[
            use_(tex(1),GpuAccess::Sampled),
            use_(buf(1),GpuAccess::StorageWrite)
        ]).unwrap();
        let plan=graph.compile().unwrap();
        assert_eq!(plan.cross_stage_transitions(),1);
        assert!(matches!(
            plan.validate_pregraphics_compute_only(),
            Err(Error::Unavailable(_))
        ));
    }

    #[test]
    fn existing_light_tiles_can_precede_screen_graphics() {
        let mut graph=GpuWorkGraph::new();
        graph.push("light tiles",GpuWorkStage::Compute,
            [use_(buf(1),GpuAccess::StorageWrite)]).unwrap();
        graph.push("scene composite",GpuWorkStage::Graphics,[
            use_(buf(1),GpuAccess::ReadBuffer),
            use_(tex(2),GpuAccess::ColorAttachment)
        ]).unwrap();
        graph.compile().unwrap().validate_pregraphics_compute_only().unwrap();
    }

    #[test]
    fn no_barrier_between_two_read_only_passes() {
        let mut graph=GpuWorkGraph::new();
        graph.import(tex(2));
        graph.push("SSR",GpuWorkStage::Graphics,[use_(tex(2),GpuAccess::Sampled)]).unwrap();
        graph.push("composite",GpuWorkStage::Graphics,[use_(tex(2),GpuAccess::Sampled)]).unwrap();
        assert!(graph.compile().unwrap().transitions.is_empty());
    }
}
