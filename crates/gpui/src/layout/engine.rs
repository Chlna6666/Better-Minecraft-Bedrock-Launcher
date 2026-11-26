use crate::{
    App, Bounds, DefiniteLength, ElementId, GlobalElementId, GpuiMemoryTrimLevel, LayoutStyle, Length, Pixels,
    Size, Style, Window, point, relative, size,
};
use collections::{FxHashMap, FxHashSet, FxHasher};
use std::{
    hash::{Hash, Hasher},
    ops::Range,
    time::{Duration, Instant},
};
use taffy::{TaffyTree, TraversePartialTree as _};

use super::{
    ClaimBlockMap, LayoutClaimBlocks, SpanRelocations,
    convert::ToTaffy,
    fingerprint::{
        hash_absolute_length, hash_display, hash_edges_absolute_length, hash_edges_definite_length,
        hash_edges_length, hash_flex_direction, hash_flex_wrap, hash_grid_location, hash_length,
        hash_optional_align_content, hash_optional_align_items, hash_overflow_point, hash_position,
        hash_size_definite_length, hash_size_length,
    },
    metrics::{AvailableSpace, AvailableSpaceKey, LayoutId},
};

#[cfg(feature = "stacker")]
type StackSafe<T> = stacksafe::StackSafe<T>;
#[cfg(not(feature = "stacker"))]
type StackSafe<T> = T;

type MeasureFn =
    dyn FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>;
pub(super) type NodeMeasureFn = StackSafe<Box<MeasureFn>>;

#[derive(Clone)]
struct NodeLayoutMetadata {
    style: LayoutStyle,
    rem_size: Pixels,
    scale_factor: f32,
}

// The common text leaf always requests the default layout style. Its
// native style and fingerprint depend only on rem size and device scale.
struct DefaultMeasuredStyle {
    rem_size: Pixels,
    scale_bits: u32,
    layout: LayoutStyle,
    native: taffy::Style,
    fingerprint: u64,
}

/// Native measurement identity. Text already computed its geometry and paint
/// keys; do not hash them again or conflate a fresh mutable text state with an
/// adopted state. The previous closure owns the state behind the pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SizeMeasurement {
    Intrinsic(Size<Pixels>),
    List(Size<Pixels>),
}

impl SizeMeasurement {
    fn measure(self, known: Size<Option<Pixels>>, available: Size<AvailableSpace>) -> Size<Pixels> {
        match self {
            Self::Intrinsic(size) => size,
            Self::List(content) => size(
                known.width.unwrap_or(match available.width {
                    AvailableSpace::Definite(width) => width,
                    _ => content.width,
                }),
                match available.height {
                    AvailableSpace::Definite(height) => content.height.min(height),
                    _ => content.height,
                },
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MeasurementKey {
    Size(SizeMeasurement),
    Text {
        state: usize,
        geometry: u64,
        paint: u64,
        geometry_only: bool,
    },
}

impl MeasurementKey {
    pub(crate) fn compatible(self, previous: Self) -> bool {
        match (self, previous) {
            (
                Self::Text { state, geometry, paint, geometry_only },
                Self::Text { state: old_state, geometry: old_geometry, paint: old_paint, geometry_only: old_geometry_only },
            ) => state == old_state && geometry == old_geometry
                && (paint == old_paint || (geometry_only && old_geometry_only)),
            _ => self == previous,
        }
    }

    fn adopts(self, previous: Self) -> bool {
        matches!(self, Self::Size(_) | Self::Text { geometry_only: true, .. }) && self == previous
    }
}

pub(super) struct NodeContext {
    measure: Option<NodeMeasureFn>,
    // Plain text belongs to its native layout slot, not the frame state map.
    plain_text: Option<crate::TextLayout>,
    key: Option<MeasurementKey>,
    replayable: bool,
    measure_log: MeasureLog,
    replay_pending: bool,
}

impl NodeContext {
    fn measure(&mut self, known: Size<Option<Pixels>>, available: Size<AvailableSpace>, scale: f32, window: &mut Window, cx: &mut App) -> Size<Pixels> {
        if let Some(MeasurementKey::Size(input)) = self.key {
            return input.measure(known, available).map(|axis| Pixels((axis.0.max(0.0) * scale).ceil() / scale));
        }
        (self.measure.as_mut().expect("dynamic measurement callback"))(known, available, window, cx)
    }
}

// Keep the new callback on the stack until native adoption has failed.
struct MeasureRequest<F> {
    key: Option<MeasurementKey>,
    replayable: bool,
    state_ready: bool,
    build: F,
}

impl<F: FnOnce() -> Option<NodeMeasureFn>> MeasureRequest<F> {
    fn into_context(self) -> NodeContext {
        NodeContext {
            measure: (self.build)(),
            plain_text: None,
            key: self.key,
            replayable: self.replayable,
            measure_log: MeasureLog::Empty,
            replay_pending: false,
        }
    }
}

fn boxed_measure(
    measure: impl FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels> + 'static,
) -> NodeMeasureFn {
    let measure = Box::new(measure) as Box<MeasureFn>;
    #[cfg(feature = "stacker")]
    let measure = StackSafe::new(measure);
    measure
}

// Adapted from longbridge/gpui-fast fast/layout.rs (Apache-2.0).
// Copyright 2022-2025 Zed Industries, Inc.; Copyright 2026 Longbridge.
// Keep one entry inline; allocate only when Taffy actually asks for more
// constraints. A conflicting result or overflow cannot vouch for its cache.
type MeasureEntry = (Size<Option<Pixels>>, Size<AvailableSpace>, Size<Pixels>);
const MEASURE_LOG_CAPACITY: usize = 16;

#[derive(Default)]
enum MeasureLog {
    #[default]
    Empty,
    Single(MeasureEntry),
    Multiple(Vec<MeasureEntry>),
    Invalid,
}

impl MeasureLog {
    fn entries(&self) -> &[MeasureEntry] {
        match self {
            Self::Single(entry) => std::slice::from_ref(entry),
            Self::Multiple(entries) => entries,
            _ => &[],
        }
    }

    fn record(&mut self, known: Size<Option<Pixels>>, available: Size<AvailableSpace>, measured: Size<Pixels>) {
        let entry = (known, available, measured);
        match self {
            Self::Empty => *self = Self::Single(entry),
            Self::Single(old) => {
                if old.0 == known && old.1 == available {
                    if old.2 != measured {
                        *self = Self::Invalid;
                    }
                } else {
                    *self = Self::Multiple(vec![*old, entry]);
                }
            }
            Self::Multiple(entries) => {
                if let Some(index) = entries.iter().position(|old| old.0 == known && old.1 == available) {
                    if entries[index].2 != measured {
                        *self = Self::Invalid;
                        return;
                    }
                    entries.remove(index);
                } else if entries.len() == MEASURE_LOG_CAPACITY {
                    *self = Self::Invalid;
                    return;
                }
                entries.push(entry);
            }
            Self::Invalid => {}
        }
    }
}

pub(super) const EXPECT_MESSAGE: &str =
    "we should avoid taffy layout errors by construction if possible";

// Adapt gpui-fast #169: bound offscreen native layout retention by both age
// and node count. Bound spare nodes relative to the current mounted
// working set as well, so small lists do not accumulate thousands of nodes.
const ROW_NODE_LINGER_FRAMES: usize = 240;
const ROW_NODE_LINGER_MAX: usize = 8192;
const ROW_NODE_LINGER_MIN: usize = 256;

/// A retained slot and its generational Taffy identity. Unlike a hash-only
/// key, a claim cannot silently bind to a replacement node after eviction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub(crate) struct LayoutClaim {
    key: u64,
    pub(crate) id: LayoutId,
}

/// Candidate-local handle, consumed before the splice enters Prepaint.
#[derive(Clone, Copy)]
pub(crate) struct NativePlacementProof(usize);

struct PlacementWatch {
    clean: Vec<(LayoutId, Option<LayoutId>)>,
    gap_parents: smallvec::SmallVec<[(LayoutId, Option<LayoutId>); 4]>,
    transient_count: usize,
    mutation_epoch: usize,
}

/// Sparse frame-local journal. Only actual native layout changes enter the
/// log, once per node per compute. Multiple candidate proofs share the log.
#[derive(Default)]
struct NativeLayoutMutationLog {
    epoch: usize,
    heads: FxHashMap<LayoutId, usize>,
    entries: Vec<NativeLayoutMutation>,
}

struct NativeLayoutMutation {
    epoch: usize,
    previous: taffy::tree::Layout,
    // usize::MAX is the end of the per-node chain.
    older: usize,
}

impl NativeLayoutMutationLog {
    const END: usize = usize::MAX;
    fn begin_compute(&mut self) {
        self.epoch += 1;
    }

    fn record(&mut self, id: LayoutId, previous: &taffy::tree::Layout) {
        let head = self.heads.entry(id).or_insert(Self::END);
        if *head != Self::END && self.entries[*head].epoch == self.epoch {
            return;
        }
        let index = self.entries.len();
        self.entries.push(NativeLayoutMutation { epoch: self.epoch, previous: *previous, older: *head });
        *head = index;
    }

    fn previous_since(&self, id: LayoutId, epoch: usize) -> Option<&taffy::tree::Layout> {
        if epoch == self.epoch { return None; }
        let mut cursor = self.heads.get(&id).copied()?;
        let mut previous = None;
        while cursor != Self::END {
            let entry = &self.entries[cursor];
            if entry.epoch <= epoch { break; }
            previous = Some(&entry.previous);
            cursor = entry.older;
        }
        previous
    }

    fn clear(&mut self) {
        self.epoch = 0;
        let count = self.entries.len();
        self.entries.clear();
        self.heads.clear();
        let target = count.max(64);
        if self.entries.capacity() > target.saturating_mul(4) {
            self.entries.shrink_to(target);
        }
        if self.heads.capacity() > target.saturating_mul(4) {
            self.heads.shrink_to(target);
        }
    }
}

struct PersistentLayoutNode {
    id: LayoutId,
    /// Structural equality guards key collisions; no frame metadata lookup.
    element: GlobalElementId,
    measured: bool,
    claimed_in_frame: u64,
    lingers: bool,
    offscreen: bool,
    /// Fingerprint of the layout style this node currently holds.
    ///
    /// An unchanged style is the common case, and converting a style to Taffy's
    /// representation is not free. Keeping the fingerprint lets an unchanged request skip
    /// that conversion and the full-struct comparison entirely.
    style_fingerprint: Option<u64>,
}

fn is_list_row(path: &[ElementId]) -> bool {
    path.iter().any(|segment| {
        matches!(segment, ElementId::NamedInteger(name, _)
            if matches!(name.as_str(), "__gpui_retained_list_item" | "__gpui_retained_uniform_row"))
    })
}

// Keep the youngest complete age buckets. Dropping a whole tied bucket avoids
// sorting or depending on hash-map iteration order when the budget is exceeded.
fn row_linger_budget(mounted: usize) -> usize {
    mounted.clamp(ROW_NODE_LINGER_MIN, ROW_NODE_LINGER_MAX)
}

fn row_linger_limit(by_age: &[usize; ROW_NODE_LINGER_FRAMES], budget: usize) -> usize {
    let mut count = 0usize;
    for (age, nodes) in by_age.iter().enumerate().skip(1) {
        count = count.saturating_add(*nodes);
        if count > budget {
            return age;
        }
    }
    ROW_NODE_LINGER_FRAMES
}

pub struct TaffyLayoutEngine {
    pub(super) taffy: TaffyTree<NodeContext>,
    persistent_nodes: FxHashMap<u64, PersistentLayoutNode>,
    /// Nodes whose native style was rewritten by a percentage-passthrough parent.
    ///
    /// Their stored fingerprint describes the style they asked for, not what the node holds,
    /// so it must not be trusted until the next request compares and rewrites. Kept as a set
    /// so the passthrough path does not have to search the node map.
    stale_style_fingerprints: FxHashSet<LayoutId>,
    anonymous_nodes: Vec<LayoutId>,
    persistent_layout_reuses: usize,
    request_frame: u64,
    claimed_this_frame: usize,
    claimed_key_log: Vec<Option<LayoutClaim>>,
    committed_claim_log: Vec<Option<LayoutClaim>>,
    claim_relocations: SpanRelocations,
    splice_placements: Vec<Option<PlacementWatch>>,
    layout_mutations: NativeLayoutMutationLog,
    pending_replays: Vec<LayoutId>,
    layout_changes: u64,
    remeasures: u64,
    robbed_parents: FxHashSet<LayoutId>,
    default_measured_style: Option<DefaultMeasuredStyle>,
    pub(super) absolute_layout_bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    unrounded_layout_origins: FxHashMap<LayoutId, (f32, f32)>,
    pub(super) computed_layouts: FxHashSet<LayoutId>,
    node_layout_metadata: FxHashMap<LayoutId, NodeLayoutMetadata>,
    // Taffy stores device-pixel geometry; equal logical constraints at a new
    // DPI must not reuse the previous physical layout.
    root_constraints: FxHashMap<LayoutId, (Size<AvailableSpaceKey>, u32)>,
    layout_bounds_scratch_space: Vec<LayoutId>,
    pub(super) nodes_requested: usize,
    pub(super) measured_nodes_requested: usize,
    pub(super) roots_computed: usize,
    pub(super) bounds_cache_hits: usize,
    pub(super) bounds_cache_misses: usize,
    pub(super) layout_cache_hits: usize,
    pub(super) layout_cache_misses: usize,
    pub(super) layout_cache_reused_roots: usize,
    layout_time: Duration,
}

fn promote_percentage_axis(wrapper: &mut Length, child: &mut Length) -> bool {
    if !matches!(wrapper, Length::Auto) {
        return false;
    }

    let Length::Definite(DefiniteLength::Fraction(fraction)) = *child else {
        return false;
    };
    if !fraction.is_finite() || fraction < 0.0 {
        return false;
    }

    *wrapper = relative(fraction).into();
    *child = relative(1.0).into();
    true
}

impl TaffyLayoutEngine {
    const MIN_RETAINED_CAPACITY: usize = 32;

    pub fn new() -> Self {
        // Taffy's rounded sizes use cumulative coordinates, while its rounded locations remain
        // parent-relative. GPUI needs both values from the same absolute coordinate system.
        let mut taffy = TaffyTree::new();
        taffy.disable_rounding();
        TaffyLayoutEngine {
            taffy,
            persistent_nodes: FxHashMap::default(),
            stale_style_fingerprints: FxHashSet::default(),
            anonymous_nodes: Vec::new(),
            persistent_layout_reuses: 0,
            request_frame: 1,
            claimed_this_frame: 0,
            claimed_key_log: Vec::new(),
            committed_claim_log: Vec::new(),
            claim_relocations: SpanRelocations::default(),
            splice_placements: Vec::new(),
            layout_mutations: NativeLayoutMutationLog::default(),
            pending_replays: Vec::new(),
            layout_changes: 0,
            remeasures: 0,
            robbed_parents: FxHashSet::default(),
            default_measured_style: None,
            absolute_layout_bounds: FxHashMap::default(),
            unrounded_layout_origins: FxHashMap::default(),
            computed_layouts: FxHashSet::default(),
            node_layout_metadata: FxHashMap::default(),
            root_constraints: FxHashMap::default(),
            layout_bounds_scratch_space: Vec::new(),
            nodes_requested: 0,
            measured_nodes_requested: 0,
            roots_computed: 0,
            bounds_cache_hits: 0,
            bounds_cache_misses: 0,
            layout_cache_hits: 0,
            layout_cache_misses: 0,
            layout_cache_reused_roots: 0,
            layout_time: Duration::ZERO,
        }
    }

    /// Fast's native claim-based release: no query of the rendered element
    /// tree. Stable frames with all slots claimed do not sweep the map.
    fn release_unclaimed_nodes(&mut self) {
        if self.persistent_nodes.len() == self.claimed_this_frame {
            return;
        }
        let frame = self.request_frame;
        let mut by_age = [0usize; ROW_NODE_LINGER_FRAMES];
        for node in self.persistent_nodes.values() {
            let age = frame.wrapping_sub(node.claimed_in_frame);
            if node.lingers && age > 0 && age < ROW_NODE_LINGER_FRAMES as u64 {
                by_age[age as usize] += 1;
            }
        }
        let limit = row_linger_limit(&by_age, row_linger_budget(self.claimed_this_frame));
        let mut removed = Vec::new();
        let taffy = &mut self.taffy;
        self.persistent_nodes.retain(|_, node| {
            if node.claimed_in_frame == frame {
                return true;
            }
            let age = frame.wrapping_sub(node.claimed_in_frame);
            if !node.lingers || age >= limit as u64 {
                removed.push(node.id);
                return false;
            }
            // Offscreen row caches must not pin a frame-local measuring closure.
            // Intrinsic text keeps only its size, not an offscreen shaped-line state.
            if let Some(context) = taffy.get_node_context_mut(node.id.into()) {
                if matches!(context.key, Some(MeasurementKey::Size(_))) {
                    context.plain_text = None;
                }
            }
            if node.measured && taffy.get_node_context_mut(node.id.into())
                .is_some_and(|context| context.key.is_none())
            {
                taffy.set_node_context(node.id.into(), None).expect(EXPECT_MESSAGE);
                taffy.mark_dirty(node.id.into()).expect(EXPECT_MESSAGE);
                node.measured = false;
            }
            node.offscreen = true;
            true
        });
        for id in removed {
            self.remove_native_node(id);
        }
        let target = Self::MIN_RETAINED_CAPACITY.max(self.persistent_nodes.len());
        if self.persistent_nodes.capacity() > target.saturating_mul(4) {
            self.persistent_nodes.shrink_to(target);
        }
    }

    fn remove_native_node(&mut self, id: LayoutId) {
        if let Some(parent) = self.taffy.parent(id.into()) {
            self.robbed_parents.insert(parent.into());
            self.taffy.mark_dirty(parent).expect(EXPECT_MESSAGE);
        }
        self.stale_style_fingerprints.remove(&id);
        self.robbed_parents.remove(&id);
        self.taffy.remove(id.into()).expect(EXPECT_MESSAGE);
        self.root_constraints.remove(&id);
        self.layout_changes = self.layout_changes.wrapping_add(1);
    }

    /// An aborted candidate may have overwritten native styles, child edges
    /// and measurement closures. Do not publish that speculative state as
    /// committed. Discard native nodes (preserving Taffy's generational IDs)
    /// and let the next request rebuild them; the committed paint stays intact.
    pub(crate) fn abort_frame(&mut self) {
        let ids: Vec<_> = self.persistent_nodes.drain().map(|(_, node)| node.id).collect();
        for id in ids {
            self.remove_native_node(id);
        }
        self.claimed_this_frame = 0;
        self.clear();
        self.committed_claim_log.clear();
    }

    pub fn clear(&mut self) {
        // Observe the frame that just finished before clearing it. This is the real active layout
        // working set and lets a large-to-small page transition return excess buckets without
        // disturbing stable large pages.
        let absolute_layout_count = self.absolute_layout_bounds.len();
        let unrounded_origin_count = self.unrounded_layout_origins.len();
        let computed_layout_count = self.computed_layouts.len();
        let metadata_count = self.node_layout_metadata.len();
        let node_working_set = self.nodes_requested;

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI persistent Taffy nodes: reused={} requested={} live={} claimed={} layout_changes={} remeasures={}",
                self.persistent_layout_reuses,
                self.nodes_requested,
                self.persistent_nodes.len(),
                self.claimed_this_frame,
                self.layout_changes,
                self.remeasures,
            );
        }
        self.persistent_layout_reuses = 0;
        std::mem::swap(&mut self.claimed_key_log, &mut self.committed_claim_log);
        self.claimed_key_log.clear();
        self.claim_relocations.clear();
        self.splice_placements.clear();
        self.layout_mutations.clear();
        for id in self.pending_replays.drain(..) {
            if let Some(context) = self.taffy.get_node_context_mut(id.into()) {
                context.replay_pending = false;
                context.measure_log = MeasureLog::Empty;
                self.taffy.mark_dirty(id.into()).expect(EXPECT_MESSAGE);
            }
        }
        let mut transient = std::mem::take(&mut self.anonymous_nodes);
        for id in transient.drain(..) {
            self.remove_native_node(id);
        }
        self.anonymous_nodes = transient;
        self.release_unclaimed_nodes();
        self.claimed_this_frame = 0;
        self.request_frame = self.request_frame.wrapping_add(1);
        self.absolute_layout_bounds.clear();
        self.unrounded_layout_origins.clear();
        self.computed_layouts.clear();
        self.node_layout_metadata.clear();

        macro_rules! trim_to_working_set {
            ($collection:expr, $count:expr) => {{
                let collection = &mut $collection;
                let target = Self::MIN_RETAINED_CAPACITY.max($count);
                if collection.capacity() > target.saturating_mul(4) {
                    collection.shrink_to(target);
                }
            }};
        }
        trim_to_working_set!(self.absolute_layout_bounds, absolute_layout_count);
        trim_to_working_set!(self.unrounded_layout_origins, unrounded_origin_count);
        trim_to_working_set!(self.computed_layouts, computed_layout_count);
        trim_to_working_set!(self.node_layout_metadata, metadata_count);
        trim_to_working_set!(self.root_constraints, self.persistent_nodes.len());
        trim_to_working_set!(self.claimed_key_log, self.committed_claim_log.len());
        let claim_count = self.committed_claim_log.len();
        trim_to_working_set!(self.committed_claim_log, claim_count);
        trim_to_working_set!(self.layout_bounds_scratch_space, node_working_set);

        self.nodes_requested = 0;
        self.measured_nodes_requested = 0;
        self.roots_computed = 0;
        self.bounds_cache_hits = 0;
        self.bounds_cache_misses = 0;
        self.layout_cache_hits = 0;
        self.layout_cache_misses = 0;
        self.layout_cache_reused_roots = 0;
        self.layout_time = Duration::ZERO;
    }

    pub(crate) fn trim_retained_capacity(&mut self, level: GpuiMemoryTrimLevel) {
        let floor = match level {
            GpuiMemoryTrimLevel::Light => Self::MIN_RETAINED_CAPACITY,
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive => 0,
        };
        self.absolute_layout_bounds
            .shrink_to(floor.max(self.absolute_layout_bounds.len()));
        self.unrounded_layout_origins
            .shrink_to(floor.max(self.unrounded_layout_origins.len()));
        self.computed_layouts
            .shrink_to(floor.max(self.computed_layouts.len()));
        self.node_layout_metadata
            .shrink_to(floor.max(self.node_layout_metadata.len()));
        self.layout_bounds_scratch_space
            .shrink_to(floor.max(self.layout_bounds_scratch_space.len()));
        self.layout_mutations.entries.shrink_to(floor.max(self.layout_mutations.entries.len()));
        self.layout_mutations.heads.shrink_to(floor.max(self.layout_mutations.heads.len()));
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.splice_placements.shrink_to_fit();
            self.anonymous_nodes.shrink_to_fit();
            self.claimed_key_log.shrink_to_fit();
            self.committed_claim_log.shrink_to_fit();
            self.pending_replays.shrink_to_fit();
        }
        if matches!(level, GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive) {
            let taffy = &mut self.taffy;
            let root_constraints = &mut self.root_constraints;
            let stale_styles = &mut self.stale_style_fingerprints;
            let robbed_parents = &mut self.robbed_parents;
            let layout_changes = &mut self.layout_changes;
            self.persistent_nodes.retain(|_, node| {
                if node.offscreen {
                    taffy.remove(node.id.into()).expect(EXPECT_MESSAGE);
                    root_constraints.remove(&node.id);
                    stale_styles.remove(&node.id);
                    robbed_parents.remove(&node.id);
                    *layout_changes = layout_changes.wrapping_add(1);
                    false
                } else {
                    true
                }
            });
        }
        self.root_constraints
            .shrink_to(floor.max(self.root_constraints.len()));
        self.persistent_nodes
            .shrink_to(floor.max(self.persistent_nodes.len()));
    }

    // Nested recording and all-or-nothing claim transaction adapted from
    // longbridge/gpui-fast 1100a214532bdeac0282acd1c5c6d2062d8ab1a7, fast/layout.rs.
    // Copyright 2022 - 2025 Zed Industries, Inc. Copyright 2026 Longbridge.
    // SPDX-License-Identifier: Apache-2.0
    /// Every native claim is recorded once. Nested views retain ranges, not arrays.
    pub(crate) fn claim_log_len(&self) -> usize {
        self.claimed_key_log.len()
    }

    /// Validate every committed interval before taking any ownership.
    /// Preserve tombstones and order for normal retained-span rebasing.
    pub(crate) fn try_keep_retained(
        &mut self,
        spans: &LayoutClaimBlocks,
        root: LayoutId,
    ) -> Option<LayoutClaimBlocks> {
        let start = self.claimed_key_log.len();
        let first_len: usize = spans.request_blocks().iter().map(Range::len).sum();
        let intervals: smallvec::SmallVec<[Range<usize>; 4]> = spans.iter().cloned().collect();
        let kept = self.try_keep_intervals(&intervals, root)?;
        let middle = start + first_len;
        let mut result = LayoutClaimBlocks::request(start..middle);
        result.push_prepaint(middle..kept.end);
        Some(result)
    }

    fn claims_valid(&self, claims: &[Option<LayoutClaim>]) -> bool {
        claims.iter().flatten().all(|claim| self.persistent_nodes.get(&claim.key).is_some_and(|node| {
            node.id == claim.id && node.claimed_in_frame != self.request_frame
                && !node.offscreen && !self.robbed_parents.contains(&node.id)
        }))
    }

    fn try_keep_intervals(&mut self, intervals: &[Range<usize>], root: LayoutId) -> Option<Range<usize>> {
        // Reject overlapping blocks before claiming any nodes.
        let mut ordered: smallvec::SmallVec<[&Range<usize>; 4]> = intervals.iter().collect();
        ordered.sort_unstable_by_key(|range| range.start);
        if ordered.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return None;
        }
        let mut has_root = false;
        for interval in intervals {
            let claims = self.committed_claim_log.get(interval.clone())?;
            if !self.claims_valid(claims) {
                return None;
            }
            has_root |= claims.iter().flatten().any(|claim| claim.id == root);
        }
        if !has_root {
            return None;
        }
        let start = self.claimed_key_log.len();
        for interval in intervals {
            let claims = &self.committed_claim_log[interval.clone()];
            for claim in claims.iter().flatten() {
                let node = self.persistent_nodes.get_mut(&claim.key).expect("validated layout claim");
                if node.claimed_in_frame != self.request_frame {
                    node.claimed_in_frame = self.request_frame;
                    self.claimed_this_frame += 1;
                }
            }
            self.claimed_key_log.extend_from_slice(claims);
        }
        Some(start..self.claimed_key_log.len())
    }

    /// A gap must be a strict descendant of the anchor's native layout root.
    /// Lazy layout roots and roots shared with a view wrapper cannot be replaced
    /// without executing the ancestor's layout lifecycle.
    pub(crate) fn layout_reaches(&self, root: LayoutId, child: LayoutId) -> bool {
        let mut node = self.taffy.parent(child.into());
        while let Some(parent) = node {
            if parent == root.into() {
                return true;
            }
            node = self.taffy.parent(parent);
        }
        false
    }

    /// Record topology and the current native mutation epoch at admission.
    /// Geometry is journaled only at actual changed native layout writes.
    pub(crate) fn capture_splice_placement(
        &mut self, kept: &LayoutClaimBlocks, gaps: &[(LayoutClaimBlocks, LayoutId)],
    ) -> Option<NativePlacementProof> {
        if !self.pending_replays.is_empty() {
            return None;
        }
        let mut clean = Vec::with_capacity(kept.iter().map(|span| span.len()).sum());
        for span in kept.iter() {
            for claim in self.claimed_key_log.get(span.clone())?.iter().flatten() {
                let id = claim.id;
                if self.taffy.dirty(id.into()).ok()? || self.stale_style_fingerprints.contains(&id) {
                    return None;
                }
                clean.push((id, self.taffy.parent(id.into()).map(LayoutId::from)));
            }
        }
        let gap_parents = gaps.iter().map(|(_, id)| {
            (*id, self.taffy.parent((*id).into()).map(LayoutId::from))
        }).collect();
        let watch = PlacementWatch {
            clean, gap_parents, transient_count: self.anonymous_nodes.len(),
            mutation_epoch: self.layout_mutations.epoch,
        };
        let index = self.splice_placements.len();
        self.splice_placements.push(Some(watch));
        Some(NativePlacementProof(index))
    }

    fn placement_topology_matches(&self, watch: &PlacementWatch) -> bool {
        self.anonymous_nodes.len() == watch.transient_count
            && watch.clean.iter().all(|(id, parent)| {
                !self.robbed_parents.contains(id)
                    && !self.stale_style_fingerprints.contains(id)
                    && self.taffy.parent((*id).into()).map(LayoutId::from) == *parent
            })
            && watch.gap_parents.iter().all(|(id, parent)| {
                self.taffy.parent((*id).into()).map(LayoutId::from) == *parent
            })
    }

    pub(crate) fn splice_topology_matches(&self, proof: NativePlacementProof) -> bool {
        self.splice_placements.get(proof.0).and_then(Option::as_ref)
            .is_some_and(|watch| self.placement_topology_matches(watch))
    }

    pub(crate) fn discard_splice_placement(&mut self, proof: NativePlacementProof) {
        if let Some(watch) = self.splice_placements.get_mut(proof.0) {
            *watch = None;
        }
    }

    /// Consume the watch once the containing root is computed. A node that
    /// was changed and subsequently restored still has identical placement.
    /// Compare full native Layout values, not just bounds.
    pub(crate) fn finish_splice_placement(&mut self, proof: NativePlacementProof) -> bool {
        let Some(watch) = self.splice_placements.get_mut(proof.0).and_then(Option::take) else {
            return false;
        };
        self.pending_replays.is_empty()
            && self.placement_topology_matches(&watch)
            && watch.clean.iter().all(|(id, _)| {
                self.taffy.dirty((*id).into()).is_ok_and(|dirty| !dirty)
                    && self.layout_mutations.previous_since(*id, watch.mutation_epoch)
                        .is_none_or(|previous| self.taffy.layout((*id).into()).is_ok_and(|current| current == previous))
            })
    }

    #[cfg(test)]
    fn splice_layout_stamp(&self) -> (u64, u64, usize, usize) {
        (self.layout_changes, self.remeasures, self.anonymous_nodes.len(), self.pending_replays.len())
    }

    /// Claim the complement of disjoint dirty blocks and publish the exact
    /// source-to-candidate map for clean records and intermediate ancestors.
    pub(crate) fn try_keep_around_gaps(
        &mut self,
        anchor: &LayoutClaimBlocks,
        gaps: &[(LayoutClaimBlocks, LayoutId)],
        root: LayoutId,
    ) -> Option<(LayoutClaimBlocks, ClaimBlockMap)> {
        if !anchor.prepaint_blocks().is_empty() || gaps.is_empty() {
            return None;
        }
        let mut excluded = smallvec::SmallVec::<[Range<usize>; 4]>::new();
        for (gap, gap_root) in gaps {
            if !gap.prepaint_blocks().is_empty() || gap.request_blocks().is_empty()
                || !self.layout_reaches(root, *gap_root)
            {
                return None;
            }
            let mut has_root = false;
            for span in gap.request_blocks() {
                let old_gap = self.committed_claim_log.get(span.clone())?;
                if !self.claims_valid(old_gap) { return None; }
                has_root |= old_gap.iter().flatten().any(|claim| claim.id == *gap_root);
                excluded.push(span.clone());
            }
            if !has_root { return None; }
        }
        excluded.sort_unstable_by_key(|range| range.start);
        if excluded.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return None;
        }
        let mut anchors: smallvec::SmallVec<[Range<usize>; 4]> = anchor.request_blocks().iter().cloned().collect();
        anchors.sort_unstable_by_key(|range| range.start);
        if anchors.windows(2).any(|pair| pair[0].end > pair[1].start) { return None; }
        let mut kept = smallvec::SmallVec::<[Range<usize>; 5]>::new();
        let mut gaps = excluded.iter().peekable();
        for block in anchors {
            let mut cursor = block.start;
            while let Some(gap) = gaps.peek() {
                if gap.start >= block.end { break; }
                if gap.start < cursor || gap.end > block.end { return None; }
                if cursor < gap.start { kept.push(cursor..gap.start); }
                cursor = gap.end;
                gaps.next();
            }
            if cursor < block.end { kept.push(cursor..block.end); }
        }
        if gaps.next().is_some() { return None; }
        let mut map = ClaimBlockMap::default();
        let mut target = self.claimed_key_log.len();
        for span in &kept {
            map.record(span.clone(), target);
            target += span.len();
        }
        let kept = self.try_keep_intervals(&kept, root)?;
        Some((LayoutClaimBlocks::request(kept), map))
    }

    /// Adopt every prebuilt request block atomically. Old slots become
    /// tombstones; nested lifecycle states resolve their original blocks.
    pub(crate) fn adopt_claim_blocks(
        &mut self, blocks: &LayoutClaimBlocks, root: LayoutId,
    ) -> Option<LayoutClaimBlocks> {
        if !blocks.prepaint_blocks().is_empty() { return None; }
        let blocks = self.resolve_claim_blocks(blocks);
        let mut ordered: smallvec::SmallVec<[&Range<usize>; 4]> = blocks.iter().collect();
        ordered.sort_unstable_by_key(|range| range.start);
        if ordered.windows(2).any(|pair| pair[0].end > pair[1].start) { return None; }
        let frame = self.request_frame;
        let mut has_root = false;
        for span in blocks.iter() {
            let source = self.claimed_key_log.get(span.clone())?;
            has_root |= source.iter().flatten().any(|claim| claim.id == root);
            if !source.iter().flatten().all(|claim| self.persistent_nodes.get(&claim.key).is_some_and(|node| {
                node.id == claim.id && node.claimed_in_frame == frame && !node.offscreen
            })) { return None; }
        }
        if !has_root { return None; }
        let start = self.claimed_key_log.len();
        self.claimed_key_log.reserve(blocks.iter().map(Range::len).sum());
        for span in blocks.iter() {
            let target = self.claimed_key_log.len();
            for index in span.clone() {
                let entry = self.claimed_key_log[index].take();
                self.claimed_key_log.push(entry);
            }
            self.claim_relocations.record(span.clone(), target);
        }
        Some(LayoutClaimBlocks::request(start..self.claimed_key_log.len()))
    }

    pub(crate) fn resolve_claim_blocks(&self, spans: &LayoutClaimBlocks) -> LayoutClaimBlocks {
        spans.resolve(&self.claim_relocations)
    }

    /// Cancel exactly the speculative claims made by admission, without
    /// scanning unrelated nodes or allocating a temporary set.
    pub(crate) fn release_kept(&mut self, spans: &LayoutClaimBlocks) {
        let frame = self.request_frame;
        for span in self.resolve_claim_blocks(spans).iter().cloned() {
            for entry in &mut self.claimed_key_log[span] {
                let Some(claim) = entry.take() else { continue; };
                if let Some(node) = self.persistent_nodes.get_mut(&claim.key)
                    && node.id == claim.id && node.claimed_in_frame == frame
                {
                    node.claimed_in_frame = frame.wrapping_sub(1);
                    self.claimed_this_frame -= 1;
                }
            }
        }
    }

    /// Reconcile the retained Element's native layout nodes. Only genuinely
    /// changed styles/child edges/measure inputs dirty Taffy's cached layouts.
    fn request_persistent_node(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        children: &[LayoutId],
        measure: Option<MeasureRequest<impl FnOnce() -> Option<NodeMeasureFn>>>,
        style_fingerprint: Option<u64>,
        build_style: impl FnOnce() -> taffy::Style,
    ) -> LayoutId {
        let _work = crate::window::element_work::begin_named(
            "layout_node_reconcile",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        // Keep the lazy conversion monomorphized. Boxing this closure allocated
        // for every request, including the fingerprint-hit path that never calls it.
        let Some((key, element)) = owner else {
            let style = build_style();
            let id = self.create_taffy_node(style, children, measure.map(MeasureRequest::into_context));
            self.anonymous_nodes.push(id);
            return id;
        };
        // As in Fast's claim path, resolve the native node with one map probe.
        // Unlike a hash-only identity, keep the full collision check. A collided
        // slot is transient for this request; do not overwrite or leak its owner.
        if let Some(node) = self.persistent_nodes.get_mut(&key) {
            if node.element != element || node.claimed_in_frame == self.request_frame {
                let id = self.create_taffy_node(build_style(), children, measure.map(MeasureRequest::into_context));
                self.anonymous_nodes.push(id);
                return id;
            }
            // A claimed row is active and no longer eligible for idle trim.
            node.offscreen = false;
            node.claimed_in_frame = self.request_frame;
            self.claimed_this_frame += 1;
            let id = node.id;
            self.claimed_key_log.push(Some(LayoutClaim { key, id }));
            self.robbed_parents.remove(&id);
            // Compare the actual native inputs. GPUI interaction/scroll flags
            // can change without changing Taffy's layout style; they must not
            // clear native measurement caches. This also detects a child style
            // rewritten by its previous percentage-passthrough parent.
            //
            // A matching fingerprint means the requested layout style is identical to the
            // one the node already holds, so the style is never converted to Taffy's
            // representation and the full-struct compare is skipped.
            let style_is_stale = self.stale_style_fingerprints.remove(&id);
            let changed_style = if !style_is_stale
                && style_fingerprint.is_some()
                && node.style_fingerprint == style_fingerprint
            {
                None
            } else {
                let style = build_style();
                (self.taffy.style(id.into()).expect(EXPECT_MESSAGE) != &style)
                    .then_some(style)
            };
            let requires_style = changed_style.is_some();
            let same_style = !style_is_stale && node.style_fingerprint == style_fingerprint;
            node.style_fingerprint = style_fingerprint;
            let is_measured = measure.is_some();
            if node.measured && !is_measured {
                self.taffy
                    .set_node_context(id.into(), None)
                    .expect(EXPECT_MESSAGE);
                self.taffy.mark_dirty(id.into()).expect(EXPECT_MESSAGE);
                self.layout_changes = self.layout_changes.wrapping_add(1);
            }
            // Taffy 0.14 exposes allocation-free child traversal. The old
            // children() call materialized a Vec on every stable node request,
            // even when nothing changed in this subtree.
            let child_order_unchanged = self.taffy.child_count(id.into()) == children.len()
                && children.iter().enumerate().all(|(index, child)| {
                    self.taffy.get_child_id(id.into(), index) == (*child).into()
                });
            if !child_order_unchanged {
                if Self::note_reparented_children(&mut self.taffy, &mut self.robbed_parents,
                    Some(id), children) {
                    self.absolute_layout_bounds.clear();
                    self.unrounded_layout_origins.clear();
                }
                self.layout_changes = self.layout_changes.wrapping_add(1);
                self.taffy
                    .set_children(id.into(), LayoutId::to_taffy_slice(children))
                    .expect(EXPECT_MESSAGE);
            }
            if let Some(style) = changed_style {
                self.taffy.set_style(id.into(), style).expect(EXPECT_MESSAGE);
                self.layout_changes = self.layout_changes.wrapping_add(1);
            }
            if let Some(request) = measure {
                if let Some(old_context) = self.taffy.get_node_context_mut(id.into()) {
                    let same_inputs = same_style && !requires_style && child_order_unchanged
                        && !old_context.replay_pending;
                    let keys = request.key.zip(old_context.key);
                    let reusable = same_inputs && request.state_ready
                        && keys.is_some_and(|(new, old)| new.compatible(old));
                    if reusable && keys.is_some_and(|(new, old)| new.adopts(old)) {
                        // Same live text state and unchanged non-truncated inputs: the
                        // existing closure and native cache can both be adopted.
                        self.persistent_layout_reuses += 1;
                        return id;
                    }
                    let replay = !reusable && same_inputs
                        && request.replayable && old_context.replayable
                        && !old_context.measure_log.entries().is_empty();
                    let mut context = request.into_context();
                    if reusable || replay {
                        context.measure_log = std::mem::take(&mut old_context.measure_log);
                    }
                    if replay {
                        context.replay_pending = true;
                        self.pending_replays.push(id);
                    }
                    *old_context = context;
                    if !reusable && !replay {
                        self.taffy.mark_dirty(id.into()).expect(EXPECT_MESSAGE);
                        self.layout_changes = self.layout_changes.wrapping_add(1);
                        self.remeasures = self.remeasures.wrapping_add(1);
                    }
                } else {
                    self.taffy
                        .set_node_context(id.into(), Some(request.into_context()))
                        .expect(EXPECT_MESSAGE);
                    self.taffy.mark_dirty(id.into()).expect(EXPECT_MESSAGE);
                    self.layout_changes = self.layout_changes.wrapping_add(1);
                    self.remeasures = self.remeasures.wrapping_add(1);
                }
            }
            node.measured = is_measured;
            self.persistent_layout_reuses += 1;
            id
        } else {
            let is_measured = measure.is_some();
            let id = self.create_taffy_node(build_style(), children, measure.map(MeasureRequest::into_context));
            self.claimed_this_frame += 1;
            self.claimed_key_log.push(Some(LayoutClaim { key, id }));
            self.persistent_nodes.insert(
                key,
                PersistentLayoutNode {
                    id,
                    lingers: is_list_row(&element.0),
                    element,
                    measured: is_measured,
                    claimed_in_frame: self.request_frame,
                    offscreen: false,
                    style_fingerprint,
                },
            );
            id
        }
    }

    // Based on Fast's set_children / robbed_parents mechanism
    // (longbridge/gpui-fast 1100a214532bdeac0282acd1c5c6d2062d8ab1a7, Apache-2.0).
    // Copyright 2022 - 2025 Zed Industries, Inc. Copyright 2026 Longbridge.
    fn note_reparented_children(
        taffy: &mut TaffyTree<NodeContext>,
        robbed_parents: &mut FxHashSet<LayoutId>,
        new_parent: Option<LayoutId>,
        children: &[LayoutId],
    ) -> bool {
        let mut changed = false;
        for child in children {
            if let Some(parent) = taffy.parent((*child).into())
                && Some(LayoutId::from(parent)) != new_parent
            {
                changed = true;
                robbed_parents.insert(parent.into());
                taffy.mark_dirty(parent).expect(EXPECT_MESSAGE);
            }
        }
        changed
    }

    fn create_taffy_node(
        &mut self,
        style: taffy::Style,
        children: &[LayoutId],
        measure: Option<NodeContext>,
    ) -> LayoutId {
        self.layout_changes = self.layout_changes.wrapping_add(1);
        if measure.is_some() {
            self.remeasures = self.remeasures.wrapping_add(1);
        }
        if Self::note_reparented_children(&mut self.taffy, &mut self.robbed_parents,
            None, children) {
            self.absolute_layout_bounds.clear();
            self.unrounded_layout_origins.clear();
        }
        match measure {
            Some(context) => self
                .taffy
                .new_leaf_with_context(style, context)
                .expect(EXPECT_MESSAGE)
                .into(),
            None if children.is_empty() => self.taffy.new_leaf(style).expect(EXPECT_MESSAGE).into(),
            None => self
                .taffy
                .new_with_children(style, LayoutId::to_taffy_slice(children))
                .expect(EXPECT_MESSAGE)
                .into(),
        }
    }

    pub fn request_layout(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        let _work = crate::window::element_work::begin_named(
            "layout_style_and_fingerprint",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        let mut layout_style = LayoutStyle::from(&style);
        self.normalize_percentage_passthrough(&mut layout_style, children);
        let mut style_hasher = FxHasher::default();
        self.hash_layout_style(&layout_style, rem_size, scale_factor, &mut style_hasher);
        let style_fingerprint = style_hasher.finish();
        let pending_style = layout_style.clone();
        let layout_id = self.request_persistent_node(
            owner,
            children,
            None::<MeasureRequest<fn() -> Option<NodeMeasureFn>>>,
            Some(style_fingerprint),
            move || pending_style.to_taffy(rem_size, scale_factor),
        );
        self.record_layout_metadata(layout_id, layout_style, rem_size, scale_factor);
        layout_id
    }

    pub fn request_measured_layout(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        measure: impl FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels>
        + 'static,
    ) -> LayoutId {
        self.request_measured_node(owner, style, rem_size, scale_factor, MeasureRequest {
            key: None, replayable: false, state_ready: true,
            build: move || Some(boxed_measure(measure)),
        })
    }

    /// Obtain the previous plain text state only for an unclaimed matching slot.
    /// A duplicate owner or a hash collision must never alias mutable text.
    pub(crate) fn previous_plain_text(&mut self, owner: &(u64, GlobalElementId)) -> Option<crate::TextLayout> {
        let node = self.persistent_nodes.get(&owner.0)?;
        if node.claimed_in_frame == self.request_frame || node.element != owner.1 {
            return None;
        }
        self.taffy.get_node_context_mut(node.id.into())?.plain_text.clone()
    }

    /// Attach state to the actual node returned by the request (including
    /// transient collision nodes). Replayed views retain their native claims.
    pub(crate) fn set_plain_text(&mut self, id: LayoutId, state: crate::TextLayout) {
        self.taffy.get_node_context_mut(id.into())
            .expect("plain text has a native measurement context")
            .plain_text = Some(state);
    }

    pub(crate) fn request_size_layout(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        input: SizeMeasurement,
    ) -> LayoutId {
        self.request_measured_node(owner, style, rem_size, scale_factor, MeasureRequest {
            key: Some(MeasurementKey::Size(input)), replayable: false, state_ready: true,
            build: || None,
        })
    }

    fn request_measured_node(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        request: MeasureRequest<impl FnOnce() -> Option<NodeMeasureFn>>,
    ) -> LayoutId {
        let _work = crate::window::element_work::begin_named(
            "layout_measured_node_request",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        self.measured_nodes_requested = self.measured_nodes_requested.saturating_add(1);
        let layout_style = LayoutStyle::from(&style);
        let mut style_hasher = FxHasher::default();
        self.hash_layout_style(&layout_style, rem_size, scale_factor, &mut style_hasher);
        let style_fingerprint = style_hasher.finish();
        let pending_style = layout_style.clone();
        let layout_id = self.request_persistent_node(
            owner,
            &[],
            Some(request),
            Some(style_fingerprint),
            move || pending_style.to_taffy(rem_size, scale_factor),
        );
        self.record_layout_metadata(layout_id, layout_style, rem_size, scale_factor);
        layout_id
    }

    pub(crate) fn request_default_measured_layout(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        rem_size: Pixels,
        scale_factor: f32,
        key: Option<MeasurementKey>,
        replayable: bool,
        state_ready: bool,
        measure: impl FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels> + 'static,
    ) -> LayoutId {
        if self.default_measured_style.as_ref().is_none_or(|cached| {
            cached.rem_size.0.to_bits() != rem_size.0.to_bits()
                || cached.scale_bits != scale_factor.to_bits()
        }) {
            let layout = LayoutStyle::from(&Style::default());
            let mut hasher = FxHasher::default();
            self.hash_layout_style(&layout, rem_size, scale_factor, &mut hasher);
            self.default_measured_style = Some(DefaultMeasuredStyle {
                rem_size,
                scale_bits: scale_factor.to_bits(),
                native: layout.to_taffy(rem_size, scale_factor),
                layout,
                fingerprint: hasher.finish(),
            });
        }
        let cached = self.default_measured_style.as_ref().expect("default measured style");
        self.request_native_measured_node(
            owner, cached.layout.clone(), cached.native.clone(), cached.fingerprint,
            rem_size, scale_factor, key, replayable, state_ready, measure,
        )
    }

    fn request_native_measured_node(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        layout_style: LayoutStyle,
        taffy_style: taffy::Style,
        style_fingerprint: u64,
        rem_size: Pixels,
        scale_factor: f32,
        key: Option<MeasurementKey>,
        replayable: bool,
        state_ready: bool,
        measure: impl FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels> + 'static,
    ) -> LayoutId {
        let _work = crate::window::element_work::begin_named(
            "layout_impure_node_request",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        self.measured_nodes_requested = self.measured_nodes_requested.saturating_add(1);
        let layout_id = self.request_persistent_node(
            owner,
            &[],
            Some(MeasureRequest {
                key,
                replayable,
                state_ready,
                build: move || Some(boxed_measure(measure)),
            }),
            Some(style_fingerprint),
            move || taffy_style,
        );
        self.record_layout_metadata(layout_id, layout_style, rem_size, scale_factor);
        layout_id
    }

    fn replay_pending_measures(&mut self, window: &mut Window, cx: &mut App) {
        let mut pending = std::mem::take(&mut self.pending_replays);
        for id in pending.drain(..) {
            let Some(context) = self.taffy.get_node_context_mut(id.into()) else { continue; };
            if !std::mem::take(&mut context.replay_pending) {
                continue;
            }
            let log = std::mem::take(&mut context.measure_log);
            let scale = window.scale_factor();
            let stands = !log.entries().is_empty() && log.entries().iter().all(|&(known, available, previous)| {
                context.measure(known, available, scale, window, cx) == previous
            });
            if stands {
                context.measure_log = log;
            } else {
                self.taffy.mark_dirty(id.into()).expect(EXPECT_MESSAGE);
                self.layout_changes = self.layout_changes.wrapping_add(1);
                self.remeasures = self.remeasures.wrapping_add(1);
            }
        }
        self.pending_replays = pending;
    }

    /// Settle text at its final native content width, not a cached sizing probe.
    /// This prepares paint state only; it must not record a new Taffy measurement.
    pub(crate) fn settle_text_measure(&mut self, id: LayoutId, window: &mut Window, cx: &mut App) {
        let scale = window.scale_factor();
        let Ok(layout) = self.taffy.layout(id.into()) else { return; };
        let content = size(
            Pixels((layout.size.width - layout.padding.left - layout.padding.right
                - layout.border.left - layout.border.right).max(0.) / scale),
            Pixels((layout.size.height - layout.padding.top - layout.padding.bottom
                - layout.border.top - layout.border.bottom).max(0.) / scale),
        );
        let Some(context) = self.taffy.get_node_context_mut(id.into()) else { return; };
        if context.replayable {
            context.measure(content.map(Some), content.map(AvailableSpace::Definite), scale, window, cx);
        }
    }

    #[cfg_attr(feature = "stacker", stacksafe::stacksafe)]
    pub fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _work = crate::window::element_work::begin_named(
            "layout_compute_setup_and_finish",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        let started_at = Instant::now();
        self.roots_computed = self.roots_computed.saturating_add(1);
        self.replay_pending_measures(window, cx);
        let scale_factor = window.scale_factor();
        let constraints = (
            size(
                AvailableSpaceKey::from(available_space.width),
                AvailableSpaceKey::from(available_space.height),
            ),
            scale_factor.to_bits(),
        );
        let parent = self.taffy.parent(id.into());
        let reusable = parent.is_none()
            && !self.robbed_parents.contains(&id)
            && self.root_constraints.get(&id) == Some(&constraints)
            && !self.taffy.dirty(id.into()).expect(EXPECT_MESSAGE);
        if reusable {
            self.computed_layouts.insert(id);
            self.layout_cache_hits = self.layout_cache_hits.saturating_add(1);
            self.layout_cache_reused_roots = self.layout_cache_reused_roots.saturating_add(1);
            self.layout_time = self.layout_time.saturating_add(started_at.elapsed());
            return;
        }
        self.layout_cache_misses = self.layout_cache_misses.saturating_add(1);
        self.root_constraints.insert(id, constraints);

        // Leaving this here until we have a better instrumentation approach.
        // println!("Laying out {} children", self.count_all_children(id)?);
        // println!("Max layout depth: {}", self.max_depth(0, id)?);

        // Output the edges (branches) of the tree in Mermaid format for visualization.
        // println!("Edges:");
        // for (a, b) in self.edges(id)? {
        //     println!("N{} --> N{}", u64::from(a), u64::from(b));
        // }
        //

        self.computed_layouts.insert(id);
        // Descendants may already have been measured as standalone roots in
        // this frame. Parent placement changes their absolute coordinates even
        // when this is the parent's first computation.
        self.invalidate_subtree_bounds(id);

        let transform = |v: AvailableSpace| match v {
            AvailableSpace::Definite(pixels) => {
                AvailableSpace::Definite(Pixels(pixels.0 * scale_factor))
            }
            AvailableSpace::MinContent => AvailableSpace::MinContent,
            AvailableSpace::MaxContent => AvailableSpace::MaxContent,
        };
        let available_space = size(
            transform(available_space.width),
            transform(available_space.height),
        );

        let taffy_work = crate::window::element_work::begin_named(
            "taffy_compute",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        let observe = self.splice_placements.iter().any(Option::is_some);
        let mutations = &mut self.layout_mutations;
        if observe { mutations.begin_compute(); }
        let mut record_mutation = |node, previous: &taffy::tree::Layout| {
            mutations.record(LayoutId::from(node), previous);
        };
        let observer: Option<&mut dyn FnMut(taffy::NodeId, &taffy::tree::Layout)> =
            if observe { Some(&mut record_mutation) } else { None };
        self.taffy
            .compute_layout_with_measure_and_observer(
                id.into(),
                available_space.into(),
                |inputs, _node_id, node_context, style| {
                    taffy::compute_leaf_layout(
                        inputs,
                        style,
                        |_, _| 0.0,
                        |known_dimensions, available_space| {
                            let Some(node_context) = node_context else {
                                return taffy::geometry::Size::ZERO;
                            };
                            let _work = crate::window::element_work::begin_named(
                                "layout_measure_callback",
                                crate::window::element_work::ElementWorkPhase::Layout,
                            );

                            let known_dimensions = Size {
                                width: known_dimensions.width.map(|e| Pixels(e / scale_factor)),
                                height: known_dimensions.height.map(|e| Pixels(e / scale_factor)),
                            };

                            let available_space: Size<AvailableSpace> = available_space.into();
                            let untransform = |ev: AvailableSpace| match ev {
                                AvailableSpace::Definite(pixels) => {
                                    AvailableSpace::Definite(Pixels(pixels.0 / scale_factor))
                                }
                                AvailableSpace::MinContent => AvailableSpace::MinContent,
                                AvailableSpace::MaxContent => AvailableSpace::MaxContent,
                            };
                            let available_space = size(
                                untransform(available_space.width),
                                untransform(available_space.height),
                            );

                            let measured = node_context.measure(
                                known_dimensions,
                                available_space,
                                scale_factor,
                                window,
                                cx,
                            );
                            if node_context.replayable {
                                node_context.measure_log.record(known_dimensions, available_space, measured);
                            }
                            size(
                                measured.width.0 * scale_factor,
                                measured.height.0 * scale_factor,
                            )
                            .into()
                        },
                    )
                },
                observer,
            )
            .expect(EXPECT_MESSAGE);
        drop(taffy_work);
        if let Some(parent) = parent {
            // A standalone computation overwrites this child's parent-relative
            // placement. Invalidate only ancestor layout caches so the next
            // enclosing computation restores placement from cached measures.
            self.taffy.mark_dirty(parent).expect(EXPECT_MESSAGE);
        }
        self.layout_time = self.layout_time.saturating_add(started_at.elapsed());
    }

    fn invalidate_subtree_bounds(&mut self, id: LayoutId) {
        if self.absolute_layout_bounds.is_empty() && self.unrounded_layout_origins.is_empty() {
            return;
        }
        let _work = crate::window::element_work::begin_named(
            "layout_bounds_invalidation",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        let stack = &mut self.layout_bounds_scratch_space;
        stack.clear();
        stack.push(id);
        while let Some(id) = stack.pop() {
            self.absolute_layout_bounds.remove(&id);
            self.unrounded_layout_origins.remove(&id);
            stack.extend(self.taffy.child_ids(id.into()).map(LayoutId::from));
        }
    }

    pub(crate) fn frame_layout_time(&self) -> Duration {
        self.layout_time
    }

    pub fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        if let Some(layout) = self.absolute_layout_bounds.get(&id).cloned() {
            self.bounds_cache_hits = self.bounds_cache_hits.saturating_add(1);
            return layout;
        }
        self.bounds_cache_misses = self.bounds_cache_misses.saturating_add(1);

        let layout = *self.taffy.layout(id.into()).expect(EXPECT_MESSAGE);
        let (origin_x, origin_y) = self.unrounded_layout_origin(id);
        let right = origin_x + layout.size.width;
        let bottom = origin_y + layout.size.height;
        let rounded_origin_x = origin_x.round();
        let rounded_origin_y = origin_y.round();
        let bounds = Bounds {
            origin: point(
                Pixels(rounded_origin_x / scale_factor),
                Pixels(rounded_origin_y / scale_factor),
            ),
            size: size(
                Pixels((right.round() - rounded_origin_x) / scale_factor),
                Pixels((bottom.round() - rounded_origin_y) / scale_factor),
            ),
        };
        self.absolute_layout_bounds.insert(id, bounds);

        bounds
    }

    fn unrounded_layout_origin(&mut self, id: LayoutId) -> (f32, f32) {
        if let Some(origin) = self.unrounded_layout_origins.get(&id).copied() {
            return origin;
        }

        let layout = self.taffy.layout(id.into()).expect(EXPECT_MESSAGE);
        let mut origin = (layout.location.x, layout.location.y);
        if let Some(parent_id) = self.taffy.parent(id.0) {
            let parent_origin = self.unrounded_layout_origin(parent_id.into());
            origin.0 += parent_origin.0;
            origin.1 += parent_origin.1;
        }
        self.unrounded_layout_origins.insert(id, origin);
        origin
    }

    /// Resolve the one safe and common percentage cycle produced by paint-only wrappers:
    /// an auto-sized wrapper with one percentage-sized child.
    ///
    /// Example: `center -> animation wrapper(auto) -> modal(width: 80%)`. The wrapper's width
    /// depends on the modal while the modal's percentage depends on the wrapper. For an explicitly
    /// marked passthrough wrapper, forwarding `80%` to the wrapper and making the child `100%` is
    /// layout-equivalent, gives Taffy a definite containing block, and preserves centering.
    #[inline]
    fn record_layout_metadata(
        &mut self,
        id: LayoutId,
        style: LayoutStyle,
        rem_size: Pixels,
        scale_factor: f32,
    ) {
        // The full logical style is not needed to compute a native layout or
        // after the request. Only percentage-passthrough parents consume it.
        // Most text and fixed-size
        // nodes therefore need no second style copy in a frame-local map.
        let percentage_axis = |length: Length| {
            matches!(length, Length::Definite(DefiniteLength::Fraction(fraction))
                if fraction.is_finite() && fraction >= 0.0)
        };
        if percentage_axis(style.size.width) || percentage_axis(style.size.height)
        {
            self.node_layout_metadata.insert(id, NodeLayoutMetadata {
                style, rem_size, scale_factor,
            });
        } else if !self.node_layout_metadata.is_empty() {
            // A persistent slot can be requested again in the same frame.
            self.node_layout_metadata.remove(&id);
        }
    }

    fn normalize_percentage_passthrough(
        &mut self,
        wrapper: &mut LayoutStyle,
        children: &[LayoutId],
    ) {
        if !wrapper.percentage_passthrough || children.len() != 1 {
            return;
        }

        let child_id = children[0];
        let Some(mut child) = self.node_layout_metadata.get(&child_id).cloned() else {
            return;
        };

        let mut changed = false;
        changed |= promote_percentage_axis(&mut wrapper.size.width, &mut child.style.size.width);
        changed |= promote_percentage_axis(&mut wrapper.size.height, &mut child.style.size.height);

        if !changed {
            return;
        }

        let taffy_style = child.style.to_taffy(child.rem_size, child.scale_factor);
        if self.taffy.style(child_id.into()).expect(EXPECT_MESSAGE) != &taffy_style {
            self.taffy
                .set_style(child_id.into(), taffy_style)
                .expect(EXPECT_MESSAGE);
            self.layout_changes = self.layout_changes.wrapping_add(1);
        }
        // This rewrote the child's native style behind its back. The child's stored
        // fingerprint still describes the style it asked for, so it must not be trusted to
        // mean "the node already holds this": marking it stale forces the next request to
        // compare, and to restore the child's own style once the passthrough parent is gone.
        self.stale_style_fingerprints.insert(child_id);
        self.node_layout_metadata.insert(child_id, child);
    }

    fn hash_layout_style(
        &self,
        style: &LayoutStyle,
        rem_size: Pixels,
        scale_factor: f32,
        hasher: &mut FxHasher,
    ) {
        hash_display(style.display, hasher);
        hash_overflow_point(&style.overflow, hasher);
        hash_absolute_length(style.scrollbar_width, hasher);
        style.allow_concurrent_scroll.hash(hasher);
        style.restrict_scroll_to_axis.hash(hasher);
        hash_position(style.position, hasher);
        hash_edges_length(&style.inset, hasher);
        hash_size_length(&style.size, hasher);
        hash_size_length(&style.min_size, hasher);
        hash_size_length(&style.max_size, hasher);
        style.aspect_ratio.map(f32::to_bits).hash(hasher);
        style.percentage_passthrough.hash(hasher);
        hash_edges_length(&style.margin, hasher);
        hash_edges_definite_length(&style.padding, hasher);
        hash_edges_absolute_length(&style.border_widths, hasher);
        hash_optional_align_items(style.align_items, hasher);
        hash_optional_align_items(style.align_self, hasher);
        hash_optional_align_content(style.align_content, hasher);
        hash_optional_align_content(style.justify_content, hasher);
        hash_size_definite_length(&style.gap, hasher);
        hash_flex_direction(style.flex_direction, hasher);
        hash_flex_wrap(style.flex_wrap, hasher);
        hash_length(style.flex_basis, hasher);
        style.flex_grow.to_bits().hash(hasher);
        style.flex_shrink.to_bits().hash(hasher);
        style.grid_cols.hash(hasher);
        style.grid_rows.hash(hasher);
        style
            .grid_location
            .as_ref()
            .map(hash_grid_location)
            .hash(hasher);
        rem_size.hash(hasher);
        scale_factor.to_bits().hash(hasher);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row_id(index: usize) -> GlobalElementId {
        GlobalElementId::from_path(&[
            ElementId::from("list"),
            ElementId::named_usize("__gpui_retained_uniform_row", index),
            ElementId::from("leaf"),
        ])
    }

    fn list_id() -> GlobalElementId {
        GlobalElementId::from_path(&[ElementId::from("list")])
    }

    /// The owner pair the engine now takes: an integer path hash plus the full identifier.
    fn owner(id: &GlobalElementId) -> (u64, GlobalElementId) {
        let mut hasher = collections::FxHasher::default();
        std::hash::Hash::hash(id.0.as_ref(), &mut hasher);
        (std::hash::Hasher::finish(&hasher), id.clone())
    }

    #[test]
    fn measure_log_keeps_distinct_constraints_and_rejects_conflicts() {
        let mut log = MeasureLog::default();
        let known = Size::default();
        let available = crate::size(AvailableSpace::MinContent, AvailableSpace::MinContent);
        let measured = crate::size(crate::px(20.), crate::px(10.));
        log.record(known, available, measured);
        log.record(known, available, measured);
        assert_eq!(log.entries().len(), 1);
        let second = crate::size(AvailableSpace::Definite(crate::px(30.)), AvailableSpace::MinContent);
        log.record(known, second, measured);
        assert_eq!(log.entries().len(), 2);
        log.record(known, available, measured);
        assert_eq!(log.entries()[1].1, available);
        log.record(known, available, crate::size(crate::px(20.), crate::px(12.)));
        assert!(matches!(log, MeasureLog::Invalid));
    }

    #[test]
    fn subtree_claims_nest_and_rollback_atomically() {
        let mut engine = TaffyLayoutEngine::new();
        let root = list_id();
        let row = row_id(2);
        let first = engine.request_layout(Some(owner(&root)), Style::default(), crate::px(16.), 1., &[]);
        let start = engine.claim_log_len();
        engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        let end = engine.claim_log_len();
        let mut claims = LayoutClaimBlocks::request(0..start);
        claims.push_prepaint(start..end);
        assert_eq!(end, 2);
        engine.clear();

        assert!(engine.try_keep_retained(&LayoutClaimBlocks::request(0..end+1), first).is_none());
        assert_eq!(engine.claimed_this_frame, 0);
        let kept = engine.try_keep_retained(&claims, first).unwrap();
        assert!(engine.try_keep_retained(&claims, first).is_none());
        assert_eq!(engine.claimed_this_frame, 2);
        engine.release_kept(&kept);
        assert_eq!(engine.claimed_this_frame, 0);
        assert!(engine.claimed_key_log.iter().all(Option::is_none));
        assert_eq!(engine.request_layout(Some(owner(&root)), Style::default(), crate::px(16.), 1., &[]), first);
        assert!(engine.try_keep_retained(&claims, first).is_none());
        assert_eq!(engine.claimed_this_frame, 1);
    }

    #[test]
    fn gap_claim_keeps_only_native_prefix_and_suffix() {
        let mut engine = TaffyLayoutEngine::new();
        let gap_owner = row_id(2);
        let root_owner = list_id();
        let gap = engine.request_layout(Some(owner(&gap_owner)), Style::default(), crate::px(16.), 1., &[]);
        let root = engine.request_layout(Some(owner(&root_owner)), Style::default(), crate::px(16.), 1., &[gap]);
        engine.clear();
        let anchor = LayoutClaimBlocks::request(0..2);
        let gap_span = LayoutClaimBlocks::request(0..1);
        let stamp = engine.splice_layout_stamp();
        let (kept, _) = engine.try_keep_around_gaps(&anchor, &[(gap_span, gap)], root).unwrap();
        assert_eq!(engine.claimed_this_frame, 1);
        assert_eq!(engine.request_layout(Some(owner(&gap_owner)), Style::default(), crate::px(16.), 1., &[]), gap);
        assert_eq!(engine.splice_layout_stamp(), stamp);
        engine.release_kept(&kept);
        assert_eq!(engine.claimed_this_frame, 1);
        assert_eq!(engine.request_layout(Some(owner(&root_owner)), Style::default(), crate::px(16.), 1., &[gap]), root);
    }

    #[test]
    fn subtree_claim_refuses_robbed_layout_nodes() {
        let mut engine = TaffyLayoutEngine::new();
        let root = list_id();
        let start = engine.claim_log_len();
        let id = engine.request_layout(Some(owner(&root)), Style::default(), crate::px(16.), 1., &[]);
        let claims = LayoutClaimBlocks::request(start..engine.claim_log_len());
        engine.clear();
        engine.robbed_parents.insert(id);
        assert!(engine.try_keep_retained(&claims, id).is_none());
        assert_eq!(engine.claimed_this_frame, 0);
    }

    #[test]
    fn claims_release_unused_slots_without_mount_metadata() {
        let mut engine = TaffyLayoutEngine::new();
        let owner_id = list_id();
        let request = |engine: &mut TaffyLayoutEngine, slot| {
            let (path, id) = owner(&owner_id);
            engine.request_layout(Some((crate::layout::layout_node_key(path, slot), id)),
                Style::default(), crate::px(16.), 1., &[])
        };
        let first = request(&mut engine, 0);
        let second = request(&mut engine, 1);
        assert_eq!(engine.claimed_this_frame, 2);
        engine.clear();
        let changes = engine.layout_changes;
        assert_eq!(request(&mut engine, 0), first);
        assert_eq!(engine.layout_changes, changes);
        engine.clear();
        assert_eq!(engine.taffy.total_node_count(), 1);
        assert!(!engine.persistent_nodes.values().any(|node| node.id == second));
    }

    #[test]
    fn reparenting_marks_previous_parent_robbed() {
        let mut engine = TaffyLayoutEngine::new();
        let child = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[]);
        let old = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[child]);
        let ancestor = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[old]);
        let next = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[child]);
        assert_eq!(engine.taffy.parent(child.into()), Some(next.into()));
        assert!(engine.robbed_parents.contains(&old));
        assert!(engine.taffy.dirty(old.into()).unwrap());
        assert!(engine.taffy.dirty(ancestor.into()).unwrap());
        engine.clear();
        assert!(engine.robbed_parents.is_empty());
    }

    #[test]
    fn offscreen_rows_reuse_layout_and_expire_on_trim() {
        let mut engine = TaffyLayoutEngine::new();
        let row = row_id(7);
        let first = engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        engine.clear();
        engine.clear();
        assert_eq!(engine.taffy.total_node_count(), 1);
        let again = engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        assert_eq!(again, first);
        engine.clear();
        engine.clear();
        engine.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);
        assert_eq!(engine.taffy.total_node_count(), 0);
    }

    #[test]
    fn offscreen_rows_expire_and_abort_discards_speculative_nodes() {
        let mut engine = TaffyLayoutEngine::new();
        let row = row_id(1);
        engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        engine.clear();
        for _ in 0..ROW_NODE_LINGER_FRAMES {
            engine.clear();
        }
        assert_eq!(engine.taffy.total_node_count(), 0);
        let start = engine.claim_log_len();
        let id = engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        let claims = LayoutClaimBlocks::request(start..engine.claim_log_len());
        engine.abort_frame();
        assert_eq!(engine.taffy.total_node_count(), 0);
        assert!(engine.try_keep_retained(&claims, id).is_none());
        let new = engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        assert_ne!(id, new);
    }

    #[test]
    fn offscreen_budget_keeps_youngest_age_buckets() {
        let mut by_age = [0; ROW_NODE_LINGER_FRAMES];
        by_age[1] = 4000;
        by_age[2] = 4000;
        assert_eq!(row_linger_limit(&by_age, ROW_NODE_LINGER_MAX), ROW_NODE_LINGER_FRAMES);
        by_age[3] = 500;
        assert_eq!(row_linger_limit(&by_age, ROW_NODE_LINGER_MAX), 3);
        by_age[1] = ROW_NODE_LINGER_MAX + 1;
        assert_eq!(row_linger_limit(&by_age, ROW_NODE_LINGER_MAX), 1);
    }

    #[test]
    fn row_linger_budget_scales_with_mounted_working_set() {
        assert_eq!(row_linger_budget(0), ROW_NODE_LINGER_MIN);
        assert_eq!(row_linger_budget(64), ROW_NODE_LINGER_MIN);
        assert_eq!(row_linger_budget(1024), 1024);
        assert_eq!(row_linger_budget(100_000), ROW_NODE_LINGER_MAX);
        let mut by_age = [0usize; ROW_NODE_LINGER_FRAMES];
        by_age[1] = 128;
        by_age[2] = 128;
        by_age[3] = 1;
        assert_eq!(row_linger_limit(&by_age, row_linger_budget(32)), 3);
    }

    #[test]
    fn frame_clear_releases_excess_layout_working_set_capacity() {
        let mut engine = TaffyLayoutEngine::new();
        engine.absolute_layout_bounds.reserve(4096);
        engine.root_constraints.reserve(4096);
        let bounds_capacity = engine.absolute_layout_bounds.capacity();
        let roots_capacity = engine.root_constraints.capacity();

        engine.clear();

        assert!(engine.absolute_layout_bounds.capacity() < bounds_capacity);
        assert!(engine.root_constraints.capacity() < roots_capacity);
    }

    #[test]
    fn aggressive_trim_releases_layout_high_watermark_capacity() {
        let mut engine = TaffyLayoutEngine::new();
        engine.root_constraints.reserve(4096);
        engine.absolute_layout_bounds.reserve(4096);
        let roots_capacity = engine.root_constraints.capacity();
        let bounds_capacity = engine.absolute_layout_bounds.capacity();

        engine.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);

        assert!(engine.root_constraints.capacity() < roots_capacity);
        assert!(engine.absolute_layout_bounds.capacity() < bounds_capacity);
    }
}
