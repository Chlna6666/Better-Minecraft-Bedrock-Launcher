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
        let mut available = self.imports;
        let mut last: HashMap<GpuResource,(usize,GpuWorkStage,GpuAccess)> = HashMap::new();
        let mut transitions = Vec::new();
        for (index,pass) in self.passes.iter().enumerate() {
            for use_ in &pass.resources {
                if use_.access.reads() && !available.contains(&use_.resource) {
                    return Err(Error::InvalidInput(format!(
                        "GPU work graph pass '{}' reads a resource before a producer or import",
                        pass.label
                    )));
                }
                if let Some(&(previous, prior_stage, prior_access))=last.get(&use_.resource) {
                    if prior_access.writes() || use_.access.writes() {
                        transitions.push(GpuTransition {
                            resource:use_.resource,
                            from_pass:previous,
                            to_pass:index,
                            from_stage:prior_stage,
                            to_stage:pass.stage,
                            before:prior_access,
                            after:use_.access,
                        });
                    }
                }
                if use_.access.writes() { available.insert(use_.resource); }
                last.insert(use_.resource,(index,pass.stage,use_.access));
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
    fn no_barrier_between_two_read_only_passes() {
        let mut graph=GpuWorkGraph::new();
        graph.import(tex(2));
        graph.push("SSR",GpuWorkStage::Graphics,[use_(tex(2),GpuAccess::Sampled)]).unwrap();
        graph.push("composite",GpuWorkStage::Graphics,[use_(tex(2),GpuAccess::Sampled)]).unwrap();
        assert!(graph.compile().unwrap().transitions.is_empty());
    }
}
