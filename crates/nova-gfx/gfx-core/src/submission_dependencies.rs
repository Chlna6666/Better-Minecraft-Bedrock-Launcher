//! Fence-verified retirement dependencies for GPU allocations.
//!
//! This is a generic resource/queue dependency model, not a renderer or a
//! command scheduler. SubmissionId MUST come from an accepted native submit.
//! Completion is reported only after polling its real fence. Cross-queue
//! ordering is valid only for GPU-side semaphore/fence waits that the backend
//! actually encoded. The planner never waits on the CPU.
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use crate::{BufferId, Error, GpuAccess, GpuResource, GpuWorkStage, Result, SubmissionId, TextureId};

/// Native queue identity scoped to exactly one GPU device/owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct GpuQueueId {
    /// Queue family (zero for non-Vulkan APIs).
    pub family: u32,
    /// Physical queue index inside the family (zero for single-queue APIs).
    pub index: u32,
}

impl GpuQueueId {
    /// The primary ordered graphics queue of a device. Use an unambiguous
    /// logical sentinel: Vulkan's actual family 0/index 0 may instead name a
    /// different native queue in an application-provided queue map.
    pub const PRIMARY: Self = Self { family: u32::MAX, index: u32::MAX };
}
impl Default for GpuQueueId {
    fn default() -> Self { Self::PRIMARY }
}

/// A whole allocation or checked half-open byte region. Texture views use
/// their parent TextureId, not one view handle, to prevent alias escapes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GpuResourceSpan {
    /// Only these physical buffer bytes can be used by a submission.
    Buffer {
        /// Physical buffer allocation containing the accessed range.
        id: BufferId,
        /// Half-open byte interval within the allocation.
        bytes: Range<u64>,
    },
    /// All bytes of a buffer, for indirect access without a proven bound.
    WholeBuffer(BufferId),
    /// A complete texture allocation, including all views and mip levels.
    Texture(TextureId),
}

impl GpuResourceSpan {
    /// Construct a nonempty buffer region; reject wrapping addresses.
    pub fn buffer(id: BufferId, offset: u64, size: u64) -> Result<Self> {
        if size == 0 {
            return Err(Error::InvalidInput("GPU resource use must have nonzero byte size".into()));
        }
        let end = offset.checked_add(size)
            .ok_or_else(|| Error::InvalidInput("GPU resource byte offset overflows u64".into()))?;
        Ok(Self::Buffer { id, bytes: offset..end })
    }

    /// True iff these two physical allocations and byte regions can alias.
    #[must_use]
    pub fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Buffer { id: a, bytes: x }, Self::Buffer { id: b, bytes: y }) =>
                a == b && x.start < y.end && y.start < x.end,
            (Self::WholeBuffer(a), Self::Buffer { id: b, .. })
            | (Self::Buffer { id: b, .. }, Self::WholeBuffer(a))
            | (Self::WholeBuffer(a), Self::WholeBuffer(b)) => a == b,
            (Self::Texture(a), Self::Texture(b)) => a == b,
            _ => false,
        }
    }

    /// The allocation whose lifetime owns this access.
    #[must_use]
    pub fn resource(&self) -> GpuResource {
        match self {
            Self::Buffer { id, .. } | Self::WholeBuffer(id) => GpuResource::Buffer(*id),
            Self::Texture(id) => GpuResource::Texture(*id),
        }
    }
}

/// One actual native GPU read or write, including transfer sources and
/// destinations. Any access can block allocation retirement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GpuSubmissionAccess {
    /// Precise byte span or complete texture accessed by this operation.
    pub span: GpuResourceSpan,
    /// Read/write mode used to infer the native synchronization hazard.
    pub access: GpuAccess,
}

/// Evidence returned only after a native GPU queue wait was successfully
/// enqueued. It is not a completed fence and cannot be obtained from queue IDs
/// or the CPU order in which API calls were issued.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GpuNativeWaitProof {
    producer: SubmissionId,
    target_queue: GpuQueueId,
}

impl GpuNativeWaitProof {
    /// Claim that a backend has actually encoded a GPU-side wait of
    /// `producer` on `target_queue`. An unsuccessful queue submission,
    /// CPU wait, or ordinary fence poll is NOT a valid reason to call this.
    ///
    /// # Safety
    ///
    /// The caller must own both native GPU queues on the same logical device,
    /// validate the producer's real queue completion primitive, and have
    /// successfully enqueued a *device-side* wait that is ordered before the
    /// consumer submission. Incorrect use permits premature resource reuse.
    // A real GPU-side wait is not constructible through a safe API:
    // consumers must explicitly uphold the backend proof obligations.
    #[allow(unsafe_code, reason = "native GPU wait proof is an intentional unsafe trust boundary")]
    pub unsafe fn after_native_wait(
        producer: SubmissionId,
        target_queue: GpuQueueId,
    ) -> Self {
        Self { producer, target_queue }
    }

    /// Native producer whose completion precedes the waiting queue's work.
    #[must_use]
    pub const fn producer(&self) -> SubmissionId { self.producer }

    /// The physical queue which contains the native wait.
    #[must_use]
    pub const fn target_queue(&self) -> GpuQueueId { self.target_queue }
}

/// Accepted native GPU submission and its resource use set.
/// Unverified GPU waits cannot be constructed through public record fields.
#[derive(Clone, Debug)]
pub struct GpuSubmissionRecord {
    /// Submission ID returned from the actual accepted native GPU submit.
    pub id: SubmissionId,
    /// Physical GPU queue that accepted this work.
    pub queue: GpuQueueId,
    /// Hardware command domain executed by this submission.
    pub stage: GpuWorkStage,
    /// Resources accessed while this native submission remains in flight.
    pub uses: Vec<GpuSubmissionAccess>,
    gpu_waits: Vec<GpuNativeWaitProof>,
}

impl GpuSubmissionRecord {
    /// Describe one accepted native GPU submission without claiming any
    /// unverified cross-queue wait; add explicit native wait proofs separately.
    #[must_use]
    pub fn new(
        id: SubmissionId, queue: GpuQueueId, stage: GpuWorkStage,
        uses: Vec<GpuSubmissionAccess>,
    ) -> Self {
        Self { id, queue, stage, uses, gpu_waits: Vec::new() }
    }

    /// Only an already encoded native GPU-side queue wait can create this edge.
    pub fn add_native_gpu_wait(&mut self, proof: GpuNativeWaitProof) -> Result<()> {
        if proof.target_queue != self.queue {
            return Err(Error::InvalidInput(
                "GPU wait target does not match consumer physical queue".into(),
            ));
        }
        if proof.producer == self.id {
            return Err(Error::InvalidInput(
                "GPU submission cannot wait on its own completion".into(),
            ));
        }
        if !self.gpu_waits.contains(&proof) {
            self.gpu_waits.push(proof);
        }
        Ok(())
    }

    /// Real GPU-side wait source IDs, never inferred from CPU ordering.
    #[must_use]
    pub fn gpu_waits(&self) -> impl Iterator<Item = SubmissionId> + '_ {
        self.gpu_waits.iter().map(|proof| proof.producer)
    }
}

/// The immutable completion obligations of a retired allocation.
#[derive(Clone, Debug, Default)]
pub struct GpuRetirementDependencies {
    pending: HashSet<SubmissionId>,
    /// Unfenced operations cannot be proved complete by an unrelated queue.
    untracked_queues: HashSet<GpuQueueId>,
}

impl GpuRetirementDependencies {
    /// The exact native submission IDs still blocking reuse.
    #[must_use]
    pub fn pending(&self) -> &HashSet<SubmissionId> { &self.pending }

    /// Unfenced queues that still need a GPU-side completion proof.
    #[must_use]
    pub fn untracked_queues(&self) -> &HashSet<GpuQueueId> { &self.untracked_queues }

    /// A submission recorded AFTER all untracked work on this same physical
    /// queue may seal it. It must be an actual native queue checkpoint, not a
    /// host frame number or arbitrary consumer from another queue.
    pub fn seal_ordered_queue(
        &mut self, graph: &GpuSubmissionDependencyGraph,
        queue: GpuQueueId, native_checkpoint: SubmissionId,
    ) -> Result<()> {
        if !graph.is_submission_on_queue(native_checkpoint, queue) {
            return Err(Error::InvalidInput(
                "retirement checkpoint has no native submission on the named queue".into(),
            ));
        }
        if self.untracked_queues.remove(&queue) {
            self.pending.insert(native_checkpoint);
        }
        Ok(())
    }

    /// A verified GPU completion also covers explicitly waited-on submissions
    /// and preceding operations on the SAME physical ordered queue.
    pub fn complete_with_proof(&mut self, proof: &GpuSubmissionCompletion) {
        for id in &proof.finished {
            self.pending.remove(id);
        }
    }

    /// No GPU access is left that can read or write the retired allocation.
    #[must_use]
    pub fn safe_to_reuse(&self) -> bool {
        self.pending.is_empty() && self.untracked_queues.is_empty()
    }
}

/// Backend-verified completion closure. This cannot be built by guessing
/// that submission IDs are monotonically increasing across queues.
#[derive(Clone, Debug)]
pub struct GpuSubmissionCompletion {
    finished: HashSet<SubmissionId>,
}

impl GpuSubmissionCompletion {
    #[must_use]
    /// Exact native submission IDs whose completion is proven by this report.
    #[must_use]
    pub fn finished(&self) -> &HashSet<SubmissionId> { &self.finished }
}

/// GPU-owner-local record of accepted work. It does not create fake fences,
/// schedule shader passes, or perform device operations.
#[derive(Clone, Default)]
pub struct GpuSubmissionDependencyGraph {
    live: HashMap<SubmissionId, GpuSubmissionRecord>,
    /// GPU completion of the key proves the transitive ancestors finished.
    ancestors: HashMap<SubmissionId, HashSet<SubmissionId>>,
    /// Only the last submitted native fence on each physical ordered queue.
    /// An accepted tail covers the complete FIFO prefix, so scanning all
    /// in-flight submissions for every Section Compute batch is unnecessary.
    queue_tails: HashMap<GpuQueueId, SubmissionId>,
    /// Work without a reliable native SubmissionId, scoped by real queue.
    unknown: Vec<(GpuQueueId, GpuSubmissionAccess)>,
}

impl GpuSubmissionDependencyGraph {
    /// Whether this owner already registered the exact native submission.
    #[must_use]
    pub fn contains_submission(&self, submission: SubmissionId) -> bool {
        self.live.contains_key(&submission)
    }

    /// A native producer must be registered on precisely this queue before
    /// its completion marker can seal unfenced resource accesses.
    #[must_use]
    pub fn is_submission_on_queue(
        &self, submission: SubmissionId, queue: GpuQueueId,
    ) -> bool {
        self.live.get(&submission).is_some_and(|work| work.queue == queue)
    }

    /// Forget all in-flight work ONLY after the native owner has proved the
    /// entire device is idle. This is never used as a normal frame boundary.
    pub fn reset_after_gpu_idle(&mut self) {
        self.live.clear();
        self.ancestors.clear();
        self.queue_tails.clear();
        self.unknown.clear();
    }

    /// Register a successfully accepted native submission. A backend must
    /// declare gpu_waits only after it has encoded the native GPU-side wait.
    /// Preceding submissions on the same physical queue are inherently FIFO.
    pub fn register(&mut self, record: GpuSubmissionRecord) -> Result<()> {
        if self.ancestors.contains_key(&record.id) {
            return Err(Error::InvalidInput("GPU submission ID was registered twice".into()));
        }
        if record.gpu_waits().any(|id| !self.live.contains_key(&id)) {
            return Err(Error::InvalidInput(
                "cross-queue wait does not reference a registered native submission".into(),
            ));
        }
        let mut covered = HashSet::new();
        if let Some(&tail) = self.queue_tails.get(&record.queue) {
            covered.insert(tail);
            if let Some(older) = self.ancestors.get(&tail) {
                covered.extend(older.iter().copied());
            }
        }
        for waited in record.gpu_waits() {
            covered.insert(waited);
            if let Some(older) = self.ancestors.get(&waited) {
                covered.extend(older.iter().copied());
            }
        }
        // The hot path is normally a single ordered queue. Build a native
        // resource index only if another outstanding physical queue may race;
        // avoid O(new_uses * in_flight_uses) for large Section batches.
        let possible_conflict = self.live.iter().any(|(id, previous)| {
            previous.queue != record.queue && !covered.contains(id)
        }) || self.unknown.iter().any(|(queue, _)| *queue != record.queue);
        if possible_conflict && !record.uses.is_empty() {
            let mut by_resource = HashMap::<crate::GpuResource, Vec<&GpuSubmissionAccess>>::new();
            for access in &record.uses {
                by_resource.entry(access.span.resource()).or_default().push(access);
            }
            // Distinct native queues are unordered unless the producer is
            // covered by an actual GPU-side wait. Concurrent read/read needs
            // no serialization, while RAW/WAR/WAW must be proven.
            for (id, previous) in &self.live {
                if previous.queue == record.queue || covered.contains(id) {
                    continue;
                }
                if previous.uses.iter().any(|old_use| {
                    by_resource.get(&old_use.span.resource()).is_some_and(|current| {
                        current.iter().any(|new_use| {
                            (new_use.access.writes() || old_use.access.writes())
                                && new_use.span.overlaps(&old_use.span)
                        })
                    })
                }) {
                    return Err(Error::InvalidInput(format!(
                        "cross-queue GPU resource hazard: {:?} must wait for {:?} before conflicting resource access",
                        record.queue, previous.queue,
                    )));
                }
            }
            // An untracked producer on another queue must first be sealed
            // with a native checkpoint from THAT queue, not a GUI fence.
            for (queue, previous) in &self.unknown {
                if *queue == record.queue {
                    continue;
                }
                if by_resource.get(&previous.span.resource()).is_some_and(|current| {
                    current.iter().any(|new_use| {
                        (new_use.access.writes() || previous.access.writes())
                            && new_use.span.overlaps(&previous.span)
                    })
                }) {
                    return Err(Error::InvalidInput(format!(
                        "cross-queue GPU resource hazard: untracked work on {:?} requires its own native checkpoint",
                        queue,
                    )));
                }
            }
        }
        self.ancestors.insert(record.id, covered);
        self.queue_tails.insert(record.queue, record.id);
        self.live.insert(record.id, record);
        Ok(())
    }

    /// Mark an actual native buffer/image use without a completion handle.
    /// This cannot be silently converted into a future GUI consumer fence.
    pub fn register_untracked(&mut self, queue: GpuQueueId, access: GpuSubmissionAccess) {
        // Streaming dispatches can read the same resident range repeatedly
        // before the GPU owner publishes a fence. Keep only one identical
        // outstanding access rather than accumulating O(frames * sections)
        // duplicate declarations. Do not deduplicate different access modes.
        if !self.unknown.iter().any(|(existing_queue, existing_access)|
            *existing_queue == queue && *existing_access == access
        ) {
            self.unknown.push((queue, access));
        }
    }

    #[must_use]
    /// Whether any admitted native GPU access lacks a trustworthy completion
    /// fence and must therefore prohibit physical range retirement.
    #[must_use]
    pub fn has_untracked_work(&self) -> bool { !self.unknown.is_empty() }

    /// Snapshot in-flight RAW/WAR/WAW readers and writers of the requested
    /// physical spans. Already-finished records must be removed by complete().
    #[must_use]
    pub fn retirement(&self, allocations: &[GpuResourceSpan]) -> GpuRetirementDependencies {
        let mut dependency = GpuRetirementDependencies::default();
        for (id, record) in &self.live {
            if record.uses.iter().any(|use_| allocations.iter().any(|allocation|
                use_.span.overlaps(allocation)
            )) {
                dependency.pending.insert(*id);
            }
        }
        for (queue, access) in &self.unknown {
            if allocations.iter().any(|allocation| access.span.overlaps(allocation)) {
                dependency.untracked_queues.insert(*queue);
            }
        }
        dependency
    }

    /// Mark a real fence completion. A completed queue submission proves
    /// transitive GPU waits and same-queue earlier work, never unrelated
    /// asynchronous work from another queue.
    pub fn complete(&mut self, submission: SubmissionId) -> Result<GpuSubmissionCompletion> {
        let ancestors = self.ancestors.get(&submission).ok_or_else(|| {
            Error::InvalidInput("completion references an unknown GPU submission".into())
        })?;
        let mut finished = ancestors.clone();
        finished.insert(submission);
        for id in &finished {
            self.live.remove(id);
            self.ancestors.remove(id);
        }
        // Release finished records and their closure: retaining every fence
        // from a 60-fps renderer would otherwise grow without bound.
        Ok(GpuSubmissionCompletion { finished })
    }

    /// Convert untracked native accesses to a checkpoint on the SAME
    /// physical queue. The backend must have queued the checkpoint after
    /// these accesses. Retirement tickets created before this call must be
    /// sealed with GpuRetirementDependencies::seal_ordered_queue as well.
    /// The checkpoint inherits the precise recorded GPU use spans.
    pub fn seal_untracked_queue(
        &mut self, queue: GpuQueueId, native_checkpoint: SubmissionId,
    ) -> Result<usize> {
        if self.live.get(&native_checkpoint).is_none_or(|record| record.queue != queue) {
            return Err(Error::InvalidInput(
                "GPU queue checkpoint is not a registered submission on this queue".into(),
            ));
        }
        let mut carried = Vec::new();
        self.unknown.retain(|(origin, access)| {
            if *origin == queue {
                carried.push(access.clone());
                false
            } else {
                true
            }
        });
        let count = carried.len();
        self.live.get_mut(&native_checkpoint)
            .expect("validated registered checkpoint")
            .uses.extend(carried);
        Ok(count)
    }

    /// Forget unfenced work only when the backend proves the entire device is
    /// idle, or after a same-queue checkpoint has been registered and assigned
    /// to all affected existing retirement tickets.
    pub fn clear_unknown_after_gpu_idle(&mut self) {
        self.unknown.clear();
    }

    /// True when a tracked submission or unknown native operation belongs
    /// to an independent queue. Draining the primary graphics queue cannot
    /// prove these operations complete without an explicit GPU-side wait.
    #[must_use]
    pub fn has_independent_queue_work(&self) -> bool {
        self.live.values().any(|record| record.queue != GpuQueueId::PRIMARY)
            || self.unknown.iter().any(|(queue, _)| *queue != GpuQueueId::PRIMARY)
    }

    /// Number of currently tracked native submissions.
    #[must_use]
    pub fn live_count(&self) -> usize { self.live.len() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(i: u32) -> SubmissionId { SubmissionId::from_parts(i, 1) }
    fn buf(offset: u64, size: u64) -> GpuResourceSpan {
        GpuResourceSpan::buffer(BufferId::from_parts(42, 1), offset, size).unwrap()
    }
    fn use_of(span: GpuResourceSpan, access: GpuAccess) -> GpuSubmissionAccess {
        GpuSubmissionAccess { span, access }
    }
    fn record(id_: u32, queue: GpuQueueId, uses: Vec<GpuSubmissionAccess>, waits: Vec<SubmissionId>) -> GpuSubmissionRecord {
        let mut record = GpuSubmissionRecord::new(id(id_), queue, GpuWorkStage::Compute, uses);
        for waited in waits {
            // SAFETY: The test explicitly models a successful native GPU wait
            // on this same target queue; no hardware submission is claimed.
            let proof = unsafe { GpuNativeWaitProof::after_native_wait(waited, queue) };
            record.add_native_gpu_wait(proof).unwrap();
        }
        record
    }

    #[test]
    fn native_wait_proof_is_bound_to_its_actual_target_queue() {
        let transfer = GpuQueueId { family: 1, index: 0 };
        let graphics = GpuQueueId::PRIMARY;
        // SAFETY: This is a unit-test model of native GPU queue waits.
        let proof = unsafe { GpuNativeWaitProof::after_native_wait(id(91), transfer) };
        let mut record = GpuSubmissionRecord::new(
            id(92), graphics, GpuWorkStage::Graphics, vec![],
        );
        assert!(record.add_native_gpu_wait(proof).is_err());
        assert_eq!(record.gpu_waits().count(), 0);
    }

    #[test]
    fn overlapping_regions_and_writer_hazards_are_exact() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let transfer = GpuQueueId { family: 1, index: 0 };
        graph.register(record(1, transfer, vec![use_of(buf(0, 64), GpuAccess::CopyWrite)], vec![])).unwrap();
        graph.register(record(2, GpuQueueId::PRIMARY,
            vec![use_of(buf(64, 64), GpuAccess::ReadBuffer)], vec![])).unwrap();
        let mut left = graph.retirement(&[buf(0, 64)]);
        assert_eq!(left.pending(), &HashSet::from([id(1)]));
        let mut right = graph.retirement(&[buf(64, 64)]);
        assert_eq!(right.pending(), &HashSet::from([id(2)]));
        let unrelated = graph.complete(id(2)).unwrap();
        left.complete_with_proof(&unrelated);
        assert!(!left.safe_to_reuse());
        right.complete_with_proof(&unrelated);
        assert!(right.safe_to_reuse());
        left.complete_with_proof(&graph.complete(id(1)).unwrap());
        assert!(left.safe_to_reuse());
    }

    #[test]
    fn native_cross_queue_wait_can_prove_transitive_completion() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let compute = GpuQueueId { family: 1, index: 0 };
        let transfer = GpuQueueId { family: 2, index: 0 };
        graph.register(record(10, transfer, vec![use_of(buf(0, 4), GpuAccess::CopyWrite)], vec![])).unwrap();
        graph.register(record(11, compute, vec![use_of(buf(0, 4), GpuAccess::StorageReadWrite)], vec![id(10)])).unwrap();
        let mut slot = graph.retirement(&[buf(0, 4)]);
        assert_eq!(slot.pending(), &HashSet::from([id(10), id(11)]));
        // The backend's native GPU wait establishes transfer -> compute.
        slot.complete_with_proof(&graph.complete(id(11)).unwrap());
        assert!(slot.safe_to_reuse());
    }

    #[test]
    fn queue_tail_preserves_fifo_order_without_scanning_every_live_submission() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let queue = GpuQueueId::PRIMARY;
        let unrelated = GpuQueueId { family: 12, index: 1 };
        graph.register(record(201, queue,
            vec![use_of(buf(0, 16), GpuAccess::StorageWrite)], vec![])).unwrap();
        graph.register(record(202, unrelated,
            vec![use_of(buf(16, 16), GpuAccess::StorageWrite)], vec![])).unwrap();
        graph.register(record(203, queue,
            vec![use_of(buf(32, 16), GpuAccess::ReadBuffer)], vec![])).unwrap();
        let mut old_primary = graph.retirement(&[buf(0, 16)]);
        let mut independent = graph.retirement(&[buf(16, 16)]);
        old_primary.complete_with_proof(&graph.complete(id(203)).unwrap());
        assert!(old_primary.safe_to_reuse());
        assert!(!independent.safe_to_reuse());
        independent.complete_with_proof(&graph.complete(id(202)).unwrap());
        assert!(independent.safe_to_reuse());
        graph.register(record(204, queue,
            vec![use_of(buf(48, 16), GpuAccess::ReadBuffer)], vec![])).unwrap();
        assert_eq!(graph.live_count(), 1);
        graph.complete(id(204)).unwrap();
        assert_eq!(graph.live_count(), 0);
    }

    #[test]
    fn same_queue_submissions_complete_in_fifo_order_only() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        graph.register(record(20, GpuQueueId::PRIMARY, vec![use_of(buf(0, 8), GpuAccess::ReadBuffer)], vec![])).unwrap();
        graph.register(record(21, GpuQueueId::PRIMARY, vec![use_of(buf(0, 8), GpuAccess::ReadBuffer)], vec![])).unwrap();
        let mut slot = graph.retirement(&[buf(0, 8)]);
        slot.complete_with_proof(&graph.complete(id(21)).unwrap());
        assert!(slot.safe_to_reuse());
    }

    #[test]
    fn unknown_async_queue_does_not_accept_graphics_fence() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let async_queue = GpuQueueId { family: 8, index: 1 };
        graph.register_untracked(async_queue, use_of(buf(0, 32), GpuAccess::StorageWrite));
        let mut dependency = graph.retirement(&[buf(0, 32)]);
        graph.register(record(50, GpuQueueId::PRIMARY, vec![], vec![])).unwrap();
        graph.register(record(51, async_queue, vec![], vec![])).unwrap();
        dependency.seal_ordered_queue(&graph, GpuQueueId::PRIMARY, id(50)).unwrap();
        assert!(!dependency.safe_to_reuse());
        assert!(dependency.seal_ordered_queue(&graph, async_queue, id(50)).is_err());
        dependency.seal_ordered_queue(&graph, async_queue, id(51)).unwrap();
        assert!(!dependency.safe_to_reuse());
        dependency.complete_with_proof(&graph.complete(id(51)).unwrap());
        assert!(dependency.safe_to_reuse());
    }

    #[test]
    fn submission_metadata_is_bounded_after_completion() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        for serial in 1..=1000 {
            graph.register(record(serial, GpuQueueId::PRIMARY,
                vec![use_of(buf(0, 4), GpuAccess::ReadBuffer)], vec![])).unwrap();
            graph.complete(id(serial)).unwrap();
        }
        assert_eq!(graph.live_count(), 0);
        assert!(graph.ancestors.is_empty());
    }

    #[test]
    fn independent_queue_survives_primary_graphics_completion() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let secondary = GpuQueueId { family: 3, index: 1 };
        graph.register(record(70, GpuQueueId::PRIMARY, vec![], vec![])).unwrap();
        graph.register(record(71, secondary, vec![use_of(buf(0, 8), GpuAccess::StorageWrite)], vec![])).unwrap();
        assert!(graph.has_independent_queue_work());
        graph.complete(id(70)).unwrap();
        assert!(graph.has_independent_queue_work());
        graph.complete(id(71)).unwrap();
        assert!(!graph.has_independent_queue_work());
    }

    #[test]
    fn unknown_accesses_are_only_sealed_by_their_own_queue() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let compute = GpuQueueId { family: 1, index: 0 };
        graph.register_untracked(compute, use_of(buf(8, 8), GpuAccess::StorageWrite));
        graph.register(record(60, GpuQueueId::PRIMARY, vec![], vec![])).unwrap();
        assert!(graph.seal_untracked_queue(compute, id(60)).is_err());
        graph.register(record(61, compute, vec![], vec![])).unwrap();
        assert_eq!(graph.seal_untracked_queue(compute, id(61)).unwrap(), 1);
        let mut ticket = graph.retirement(&[buf(8, 8)]);
        assert_eq!(ticket.pending(), &HashSet::from([id(61)]));
        assert!(ticket.untracked_queues().is_empty());
        ticket.complete_with_proof(&graph.complete(id(60)).unwrap());
        assert!(!ticket.safe_to_reuse());
        ticket.complete_with_proof(&graph.complete(id(61)).unwrap());
        assert!(ticket.safe_to_reuse());
    }

    #[test]
    fn conflicting_cross_queue_writes_require_native_gpu_wait_proof() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let compute = GpuQueueId { family: 1, index: 0 };
        let transfer = GpuQueueId { family: 2, index: 0 };
        graph.register(record(81, transfer,
            vec![use_of(buf(0, 32), GpuAccess::CopyWrite)], vec![])).unwrap();

        // No GPU wait: neither a second physical queue nor a later CPU API
        // call establishes GPU execution order for overlapping writes.
        assert!(graph.register(record(82, compute,
            vec![use_of(buf(16, 32), GpuAccess::StorageWrite)], vec![])).is_err());
        assert_eq!(graph.live_count(), 1);

        // Disjoint physical byte regions and simultaneous immutable readers
        // do not need artificial cross-queue serialization.
        graph.register(record(83, compute,
            vec![use_of(buf(32, 32), GpuAccess::StorageWrite)], vec![])).unwrap();
        graph.register(record(84, GpuQueueId::PRIMARY,
            vec![use_of(buf(0, 32), GpuAccess::CopyRead)], vec![id(81)])).unwrap();

        // A true native producer->consumer wait certifies the overlapping
        // transfer-write -> compute-read/modify/write dependency.
        graph.register(record(85, compute,
            vec![use_of(buf(0, 32), GpuAccess::StorageReadWrite)],
            vec![id(81), id(84)])).unwrap();
    }

    #[test]
    fn unfenced_other_queue_work_requires_own_checkpoint_before_conflict() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let compute = GpuQueueId { family: 3, index: 0 };
        graph.register_untracked(compute,
            use_of(buf(0, 32), GpuAccess::StorageWrite));
        assert!(graph.register(record(86, GpuQueueId::PRIMARY,
            vec![use_of(buf(0, 32), GpuAccess::ReadBuffer)], vec![])).is_err());

        graph.register(record(87, compute, vec![], vec![])).unwrap();
        graph.seal_untracked_queue(compute, id(87)).unwrap();
        graph.register(record(88, GpuQueueId::PRIMARY,
            vec![use_of(buf(0, 32), GpuAccess::ReadBuffer)], vec![id(87)])).unwrap();
    }

    #[test]
    fn independent_readers_can_share_immutable_resource_without_waits() {
        let mut graph = GpuSubmissionDependencyGraph::default();
        let compute = GpuQueueId { family: 3, index: 0 };
        let transfer = GpuQueueId { family: 4, index: 0 };
        graph.register(record(89, compute,
            vec![use_of(buf(0, 32), GpuAccess::ReadBuffer)], vec![])).unwrap();
        graph.register(record(90, transfer,
            vec![use_of(buf(0, 32), GpuAccess::CopyRead)], vec![])).unwrap();
    }

    #[test]
    fn invalid_offsets_and_unverified_queue_waits_fail_closed() {
        let buffer = BufferId::from_parts(42, 1);
        assert!(GpuResourceSpan::buffer(buffer, u64::MAX, 2).is_err());
        assert!(GpuResourceSpan::buffer(buffer, 0, 0).is_err());
        let mut graph = GpuSubmissionDependencyGraph::default();
        assert!(graph.register(record(5, GpuQueueId::PRIMARY,
            vec![use_of(buf(0, 1), GpuAccess::ReadBuffer)], vec![id(99)])).is_err());
        assert_eq!(graph.live_count(), 0);
    }
}
