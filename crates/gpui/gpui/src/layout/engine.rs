use crate::{
    App, Bounds, GpuiMemoryTrimLevel, Length, Pixels,
    Size, Style, Window, point, relative, size,
};
use collections::{FxHashMap, FxHashSet, FxHasher};
use std::{
    hash::{Hash, Hasher},
    ops::Range,
    time::{Duration, Instant},
};
use slotmap::{DefaultKey, KeyData, SecondaryMap};
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
    layout_bounds::LayoutIdMap,
};

#[cfg(feature = "stacker")]
type StackSafe<T> = stacksafe::StackSafe<T>;
#[cfg(not(feature = "stacker"))]
type StackSafe<T> = T;

type MeasureFn =
    dyn FnMut(Size<Option<Pixels>>, Size<AvailableSpace>, &mut Window, &mut App) -> Size<Pixels>;
pub(super) type NodeMeasureFn = StackSafe<Box<MeasureFn>>;

// The common text leaf always requests the default layout style. Its
// native style and fingerprint depend only on rem size and device scale.
struct DefaultMeasuredStyle {
    rem_size: Pixels,
    scale_bits: u32,
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

/// GPUI Fast's held native layouts (fast/layout.rs, retained_layouts and
/// layouts_unchanged), with the claim and parent needed by the current
/// multi-slot retained layout representation.
pub(crate) type HeldLayout = (LayoutClaim, taffy::tree::Layout, Option<LayoutId>);

/// A cache invalidation is narrower than a placement proof: only location
/// changes affect descendant origins, and only size/location affect bounds.
struct BoundsMutation {
    id: LayoutId,
    moved: bool,
}

struct PersistentLayoutNode {
    id: LayoutId,
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

/// Temporary detached-row transaction, allocated only while entering rows
/// are built. Existing native layout IDs are never copied into a second tree.
struct DetachedBoundary {
    retired: Vec<Range<usize>>,
    created: FxHashSet<LayoutId>,
    created_keys: Vec<u64>,
    // A colliding key is diverted to a temporary node; an external native
    // write requires full layout recovery rather than pretending to undo it.
    rejected: bool,
    touched_existing: bool,
}

pub struct TaffyLayoutEngine {
    pub(super) taffy: TaffyTree<()>,
    // Zed GPUI #67223: measurement ownership uses the native generational node key.
    // Index directly by the layout node rather than Taffy's separate context key.
    contexts: SecondaryMap<DefaultKey, NodeContext>,
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
    detached_boundary: Option<DetachedBoundary>,
    // The committed claim log has stable coordinates between ordinary
    // draws. A successful append-only boundary preserves its proof.
    detached_claim_proof: Option<(u64, u64)>,
    claim_relocations: SpanRelocations,
    pending_replays: Vec<LayoutId>,
    layout_changes: u64,
    remeasures: u64,
    robbed_parents: FxHashSet<LayoutId>,
    default_measured_style: Option<DefaultMeasuredStyle>,
    pub(super) absolute_layout_bounds: LayoutIdMap<Bounds<Pixels>>,
    unrounded_layout_origins: LayoutIdMap<(f32, f32)>,
    // Rounded logical bounds depend on the window's device scale.
    bounds_scale_bits: Option<u32>,
    pub(super) computed_layouts: FxHashSet<LayoutId>,
    // Taffy stores device-pixel geometry; equal logical constraints at a new
    // DPI must not reuse the previous physical layout.
    root_constraints: FxHashMap<LayoutId, (Size<AvailableSpaceKey>, u32)>,
    layout_bounds_scratch_space: Vec<LayoutId>,
    bounds_mutations: Vec<BoundsMutation>,
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

fn promote_percentage_axis(wrapper: &mut Length, child: taffy::style::Dimension) -> bool {
    if !matches!(wrapper, Length::Auto) {
        return false;
    }
    let taffy::style::ExpandedDimension::Percent(fraction) = child.expand() else {
        return false;
    };
    if !fraction.is_finite() || fraction < 0.0 {
        return false;
    }
    *wrapper = relative(fraction).into();
    true
}

// Adapted from Zed GPUI 79af38b26c7571fbbdf84b8dd1f19d4426e58df7 (Apache-2.0).
// Copyright 2022-2025 Zed Industries, Inc.
#[inline]
fn node_key(id: taffy::NodeId) -> DefaultKey {
    KeyData::from_ffi(id.into()).into()
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
            contexts: SecondaryMap::new(),
            persistent_nodes: FxHashMap::default(),
            stale_style_fingerprints: FxHashSet::default(),
            anonymous_nodes: Vec::new(),
            persistent_layout_reuses: 0,
            request_frame: 1,
            claimed_this_frame: 0,
            claimed_key_log: Vec::new(),
            committed_claim_log: Vec::new(),
            detached_boundary: None,
            detached_claim_proof: None,
            claim_relocations: SpanRelocations::default(),
            pending_replays: Vec::new(),
            layout_changes: 0,
            remeasures: 0,
            robbed_parents: FxHashSet::default(),
            default_measured_style: None,
            absolute_layout_bounds: LayoutIdMap::default(),
            unrounded_layout_origins: LayoutIdMap::default(),
            bounds_scale_bits: None,
            computed_layouts: FxHashSet::default(),
            root_constraints: FxHashMap::default(),
            layout_bounds_scratch_space: Vec::new(),
            bounds_mutations: Vec::new(),
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
        let contexts = &mut self.contexts;
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
            if let Some(context) = contexts.get_mut(node_key(node.id.into())) {
                if matches!(context.key, Some(MeasurementKey::Size(_))) {
                    context.plain_text = None;
                }
            }
            if node.measured && contexts.get_mut(node_key(node.id.into()))
                .is_some_and(|context| context.key.is_none())
            {
                contexts.remove(node_key(node.id.into()));
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
        // Removing a parent can detach surviving children. Revoke the
        // old origin proof before Taffy changes their parent links.
        Self::invalidate_origin_subtree(
            &self.taffy, &mut self.unrounded_layout_origins,
            &mut self.absolute_layout_bounds, &mut self.layout_bounds_scratch_space, id,
        );
        self.contexts.remove(node_key(id.into()));
        self.taffy.remove(id.into()).expect(EXPECT_MESSAGE);
        self.root_constraints.remove(&id);
        self.layout_changes = self.layout_changes.wrapping_add(1);
    }

    /// An aborted candidate may have overwritten native styles, child edges
    /// and measurement closures. Do not publish that speculative state as
    /// committed. Discard native nodes (preserving Taffy's generational IDs)
    /// and let the next request rebuild them; the committed paint stays intact.
    pub(crate) fn abort_frame(&mut self) {
        self.detached_boundary = None;
        let ids: Vec<_> = self.persistent_nodes.drain().map(|(_, node)| node.id).collect();
        for id in ids {
            self.remove_native_node(id);
        }
        self.claimed_this_frame = 0;
        // An aborted candidate can have performed speculative native writes.
        self.absolute_layout_bounds.clear();
        self.unrounded_layout_origins.clear();
        self.bounds_scale_bits = None;
        self.clear();
        self.committed_claim_log.clear();
    }

    pub fn clear(&mut self) {
        debug_assert!(self.detached_boundary.is_none(), "unfinished detached boundary");
        self.detached_claim_proof = None;
        // Observe the frame that just finished before clearing it. This is the real active layout
        // working set and lets a large-to-small page transition return excess buckets without
        // disturbing stable large pages.
        let computed_layout_count = self.computed_layouts.len();
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
        for id in self.pending_replays.drain(..) {
            if let Some(context) = self.contexts.get_mut(node_key(id.into())) {
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
        // Native placement writes and topology edits revoke affected entries
        // when they happen. Surviving nodes keep their device-space origins
        // and logical bounds across the frame boundary.
        self.computed_layouts.clear();

        macro_rules! trim_to_working_set {
            ($collection:expr, $count:expr) => {{
                let collection = &mut $collection;
                let target = Self::MIN_RETAINED_CAPACITY.max($count);
                if collection.capacity() > target.saturating_mul(4) {
                    collection.shrink_to(target);
                }
            }};
        }
        self.absolute_layout_bounds.trim_empty(Self::MIN_RETAINED_CAPACITY);
        self.unrounded_layout_origins.trim_empty(Self::MIN_RETAINED_CAPACITY);
        trim_to_working_set!(self.computed_layouts, computed_layout_count);
        trim_to_working_set!(self.root_constraints, self.persistent_nodes.len());
        trim_to_working_set!(self.claimed_key_log, self.committed_claim_log.len());
        let claim_count = self.committed_claim_log.len();
        trim_to_working_set!(self.committed_claim_log, claim_count);
        trim_to_working_set!(self.layout_bounds_scratch_space, node_working_set);
        trim_to_working_set!(self.bounds_mutations, node_working_set);

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
        // Idle trimming cannot retire native slots while a boundary is open.
        if self.detached_boundary.is_some() { return; }
        let floor = match level {
            GpuiMemoryTrimLevel::Light => Self::MIN_RETAINED_CAPACITY,
            GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive => 0,
        };
        self.computed_layouts
            .shrink_to(floor.max(self.computed_layouts.len()));
        self.layout_bounds_scratch_space
            .shrink_to(floor.max(self.layout_bounds_scratch_space.len()));
        self.bounds_mutations.shrink_to(floor.max(self.bounds_mutations.len()));
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.anonymous_nodes.shrink_to_fit();
            self.claimed_key_log.shrink_to_fit();
            self.committed_claim_log.shrink_to_fit();
            self.pending_replays.shrink_to_fit();
        }
        if matches!(level, GpuiMemoryTrimLevel::Moderate | GpuiMemoryTrimLevel::Aggressive)
        {
            let taffy = &mut self.taffy;
            let contexts = &mut self.contexts;
            let root_constraints = &mut self.root_constraints;
            let stale_styles = &mut self.stale_style_fingerprints;
            let robbed_parents = &mut self.robbed_parents;
            let layout_changes = &mut self.layout_changes;
            let origins = &mut self.unrounded_layout_origins;
            let bounds = &mut self.absolute_layout_bounds;
            let stack = &mut self.layout_bounds_scratch_space;
            self.persistent_nodes.retain(|_, node| {
                if node.offscreen {
                    // Memory pressure may detach a surviving child just like
                    // normal retirement. Invalidate before removing the node.
                    Self::invalidate_origin_subtree(taffy, origins, bounds, stack, node.id);
                    if let Some(parent) = taffy.parent(node.id.into()) {
                        robbed_parents.insert(parent.into());
                        taffy.mark_dirty(parent).expect(EXPECT_MESSAGE);
                    }
                    contexts.remove(node_key(node.id.into()));
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
        // Retire dead high slots after offscreen node removal. Never move
        // a live entry to a different slot or scan these tables every frame.
        if matches!(level, GpuiMemoryTrimLevel::Light) {
            self.absolute_layout_bounds.trim_empty(floor);
            self.unrounded_layout_origins.trim_empty(floor);
        } else {
            self.absolute_layout_bounds.trim_dead_tail(floor);
            self.unrounded_layout_origins.trim_dead_tail(floor);
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

    /// Start from an idle native engine. Existing claim offsets stay stable:
    /// retired intervals will become tombstones and fresh claims are appended
    /// only on commit. Return the offset for new row claim blocks.
    #[allow(dead_code)]
    pub(crate) fn begin_detached_boundary(
        &mut self, retired: &[(LayoutId, LayoutClaimBlocks)],
    ) -> Option<usize> {
        if self.detached_boundary.is_some() || !self.claimed_key_log.is_empty()
            || self.claimed_this_frame != 0 || !self.anonymous_nodes.is_empty()
            || !self.pending_replays.is_empty()
            // Bound tombstone accumulation; normal full traversal compacts it.
            || self.committed_claim_log.len() > self.persistent_nodes.len().max(32).saturating_mul(2)
        {
            return None;
        }
        // Validate the committed domain once per ordinary frame or
        // external native mutation, not once per scroll boundary.
        let proof = (self.request_frame, self.layout_changes);
        if self.detached_claim_proof != Some(proof) {
            if !self.claims_valid(&self.committed_claim_log) { return None; }
            self.detached_claim_proof = Some(proof);
        }
        let mut ranges = Vec::with_capacity(retired.len());
        for (root, blocks) in retired {
            if !blocks.request_blocks().is_empty() || blocks.prepaint_blocks().len() != 1 {
                return None;
            }
            let range = blocks.prepaint_blocks()[0].clone();
            let claims = self.committed_claim_log.get(range.clone())?;
            if claims.is_empty() || !self.claims_valid(claims) { return None; }
            let mut roots = 0usize;
            let mut edges = 0usize;
            for claim in claims {
                let claim = claim.as_ref()?;
                let node = self.persistent_nodes.get(&claim.key)?;
                if !node.lingers || self.stale_style_fingerprints.contains(&claim.id) { return None; }
                if claim.id == *root {
                    if self.layout_parent(*root).is_some() { return None; }
                    roots += 1;
                } else if !self.layout_reaches(*root, claim.id) {
                    return None;
                }
                edges = edges.checked_add(self.taffy.child_count(claim.id.into()))?;
            }
            if roots != 1 || edges != claims.len() - 1 { return None; }
            ranges.push(range);
        }
        ranges.sort_unstable_by_key(|range| range.start);
        if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) { return None; }
        let offset = self.committed_claim_log.len();
        self.detached_boundary = Some(DetachedBoundary {
            retired: ranges, created: FxHashSet::default(), created_keys: Vec::new(),
            rejected: false, touched_existing: false,
        });
        Some(offset)
    }

    /// Detect writes to pre-existing native state before executing them.
    /// The ordinary path is one predictable None check.
    fn observe_boundary_write(&mut self, id: LayoutId) {
        if let Some(boundary) = self.detached_boundary.as_mut()
            && !boundary.created.contains(&id)
        {
            boundary.rejected = true;
            boundary.touched_existing = true;
        }
    }

    /// Cancel an append-only candidate without destroying committed nodes.
    /// False means full native recovery was necessary; the caller must
    /// invalidate retained layout admission and rebuild before publication.
    #[allow(dead_code)]
    pub(crate) fn cancel_detached_boundary(&mut self) -> bool {
        let Some(boundary) = self.detached_boundary.take() else { return false; };
        self.detached_claim_proof = None;
        if boundary.touched_existing {
            self.abort_frame();
            return false;
        }
        for key in boundary.created_keys {
            self.persistent_nodes.remove(&key);
        }
        for id in boundary.created {
            self.computed_layouts.remove(&id);
            self.remove_native_node(id);
        }
        self.anonymous_nodes.clear();
        self.claimed_key_log.clear();
        self.claimed_this_frame = 0;
        self.pending_replays.clear();
        self.claim_relocations.clear();
        self.bounds_mutations.clear();
        true
    }

    /// Project an enclosing View's layout ownership into the candidate's
    /// stable coordinates. The caller must certify it is an ancestor of
    /// this single boundary domain, including for insertion-only edits.
    /// Unrelated View records keep their original claim coordinates.
    #[allow(dead_code)]
    pub(crate) fn detached_boundary_ancestor_claims(
        &self, old: &LayoutClaimBlocks,
    ) -> Option<LayoutClaimBlocks> {
        let boundary = self.detached_boundary.as_ref()?;
        let old_len = self.committed_claim_log.len();
        if boundary.rejected || old.iter().any(|span| span.start > span.end || span.end > old_len) {
            return None;
        }
        let mut retired = boundary.retired.clone();
        // The row domain belongs to prepaint; an overlapping request
        // block would mean the caller chose an invalid enclosing owner.
        if old.request_blocks().iter().any(|request| retired.iter().any(|span| {
            request.start < span.end && span.start < request.end
        })) {
            return None;
        }
        let mut kept = smallvec::SmallVec::<[Range<usize>; 5]>::new();
        Self::subtract_claim_blocks(old.prepaint_blocks(), &mut retired, &mut kept)?;
        let mut result = LayoutClaimBlocks::default();
        for span in old.request_blocks() { result.push_request(span.clone()); }
        for span in kept { result.push_prepaint(span); }
        result.push_prepaint(old_len..old_len.checked_add(self.claimed_key_log.len())?);
        result.normalize();
        Some(result)
    }

    /// Resolve one freshly materialized row's local claim coordinates
    /// into the append-only committed log without touching the nodes.
    #[allow(dead_code)]
    pub(crate) fn detached_boundary_row_claims(
        &self, root: LayoutId, local: &LayoutClaimBlocks,
    ) -> Option<LayoutClaimBlocks> {
        let boundary = self.detached_boundary.as_ref()?;
        if boundary.rejected || !local.request_blocks().is_empty() || local.prepaint_blocks().len() != 1 {
            return None;
        }
        let span = &local.prepaint_blocks()[0];
        self.capture_detached_row_claims(span.clone(), root)?;
        let offset = self.committed_claim_log.len();
        Some(LayoutClaimBlocks::prepaint(offset.checked_add(span.start)?..offset.checked_add(span.end)?))
    }

    /// Final native adoption, after all other Frame candidates are validated.
    /// Existing indices are unchanged, retired slots are tombstoned, and
    /// new row claims use begin_detached_boundary's returned offset.
    /// A rejected candidate remains open for cancel_detached_boundary.
    #[allow(dead_code)]
    pub(crate) fn prepare_detached_boundary_commit(&mut self) -> Option<()> {
        let boundary = self.detached_boundary.as_ref()?;
        if boundary.rejected || !self.anonymous_nodes.is_empty()
            || !self.pending_replays.is_empty()
            || self.claimed_this_frame != self.claimed_key_log.len()
            || boundary.created.len() != self.claimed_key_log.len()
            || boundary.created_keys.len() != self.claimed_key_log.len()
            || !self.claimed_key_log.iter().all(|entry| entry.as_ref().is_some_and(|claim| {
                boundary.created.contains(&claim.id)
                    && self.persistent_nodes.get(&claim.key).is_some_and(|node| {
                        node.id == claim.id && node.claimed_in_frame == self.request_frame
                    })
            }))
        {
            return None;
        }
        // Complete the only fallible allocation before updating any epoch.
        self.committed_claim_log.try_reserve(self.claimed_key_log.len()).ok()?;
        Some(())
    }

    pub(crate) fn commit_detached_boundary(&mut self) -> Option<()> {
        self.prepare_detached_boundary_commit()?;
        let boundary = self.detached_boundary.take().expect("validated boundary");
        // Keep the ordinary traversal epoch unchanged. Its next request
        // must still be able to claim every surviving node. Only entered
        // nodes need their current-frame claim stamp converted to the
        // previously committed epoch.
        for claim in self.claimed_key_log.iter().flatten() {
            self.persistent_nodes.get_mut(&claim.key).expect("validated entered claim")
                .claimed_in_frame = self.request_frame.wrapping_sub(1);
        }
        // The admission proof established complete detached subtrees.
        // Retire them immediately instead of scanning the entire native
        // map to age offscreen nodes. A reverse scroll can recreate the
        // same key without colliding with an offscreen lingering slot.
        for range in boundary.retired {
            for index in range {
                let claim = self.committed_claim_log[index].take().expect("validated retired claim");
                self.persistent_nodes.remove(&claim.key).expect("validated retired node");
                self.remove_native_node(claim.id);
            }
        }
        self.committed_claim_log.append(&mut self.claimed_key_log);
        self.claimed_this_frame = 0;
        self.claim_relocations.clear();
        self.computed_layouts.clear();
        self.bounds_mutations.clear();
        self.persistent_layout_reuses = 0;
        self.nodes_requested = 0;
        self.measured_nodes_requested = 0;
        self.roots_computed = 0;
        self.bounds_cache_hits = 0;
        self.bounds_cache_misses = 0;
        self.layout_cache_hits = 0;
        self.layout_cache_misses = 0;
        self.layout_cache_reused_roots = 0;
        self.layout_time = Duration::ZERO;
        self.detached_claim_proof = Some((self.request_frame, self.layout_changes));
        Some(())
    }

    /// A row layout is a detached native subtree. Record its actual claim
    /// range only if every node belongs to that subtree and every child is
    /// represented. Transient nodes or external child edges disqualify it.
    pub(crate) fn capture_detached_row_claims(
        &self, range: Range<usize>, root: LayoutId,
    ) -> Option<LayoutClaimBlocks> {
        if range.is_empty() || self.layout_parent(root).is_some() { return None; }
        let records = self.claimed_key_log.get(range.clone())?;
        let mut roots = 0;
        let mut edges = 0usize;
        for record in records {
            let claim = record.as_ref()?;
            let node = self.persistent_nodes.get(&claim.key)?;
            if node.id != claim.id || node.claimed_in_frame != self.request_frame
                || !node.lingers || node.offscreen
                || self.robbed_parents.contains(&claim.id)
                || self.stale_style_fingerprints.contains(&claim.id)
            {
                return None;
            }
            if claim.id == root {
                roots += 1;
            } else if !self.layout_reaches(root, claim.id) {
                return None;
            }
            edges = edges.checked_add(self.taffy.child_count(claim.id.into()))?;
        }
        if roots != 1 || edges != records.len() - 1 { return None; }
        // Lazy rows request layout during the enclosing view's prepaint.
        Some(LayoutClaimBlocks::prepaint(range))
    }

    /// Native request order is children-before-parent. Avoid asking the
    /// Element to request layout a second time just to recover its root.
    pub(crate) fn capture_detached_row_range(
        &self, range: Range<usize>,
    ) -> Option<(LayoutId, LayoutClaimBlocks)> {
        if range.is_empty() { return None; }
        let root = self.claimed_key_log.get(range.end.checked_sub(1)?)?.as_ref()?.id;
        let claims = self.capture_detached_row_claims(range, root)?;
        Some((root, claims))
    }

    /// Use the existing all-or-nothing native claim mechanism. A row may
    /// already have been claimed by another retained owner; in that case
    /// do not publish a second claim proof.
    pub(crate) fn try_keep_detached_row_claims(
        &mut self, spans: &LayoutClaimBlocks, root: LayoutId,
    ) -> Option<LayoutClaimBlocks> {
        if !spans.request_blocks().is_empty() || spans.prepaint_blocks().is_empty() {
            return None;
        }
        // A previous paint can outlive a discarded native layout engine
        // generation. Check the authoritative persistent slot before
        // accessing Taffy's generational node ID.
        let mut root_live = false;
        for span in spans.prepaint_blocks() {
            let claims = self.committed_claim_log.get(span.clone())?;
            root_live |= claims.iter().flatten().any(|claim| {
                claim.id == root && self.persistent_nodes.get(&claim.key)
                    .is_some_and(|node| node.id == root && !node.offscreen)
            });
        }
        if !root_live || self.layout_parent(root).is_some() { return None; }
        self.try_keep_retained(spans, root)
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
        if self.detached_boundary.is_some() { return None; }
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

    /// GPUI Fast retains the actual native layouts before rebuilding a gap
    /// and checks them after layout, instead of intercepting Taffy's writes.
    /// Source: longbridge/gpui-fast fast/layout.rs at
    /// 1100a214532bdeac0282acd1c5c6d2062d8ab1a7 (Apache-2.0).
    pub(crate) fn retained_layouts(&self, kept: &LayoutClaimBlocks) -> Option<Vec<HeldLayout>> {
        if !self.pending_replays.is_empty() {
            return None;
        }
        let mut held = Vec::with_capacity(kept.iter().map(Range::len).sum());
        for span in kept.iter() {
            for claim in self.claimed_key_log.get(span.clone())?.iter().flatten() {
                let id = claim.id;
                if self.taffy.dirty(id.into()).ok()? || self.stale_style_fingerprints.contains(&id) {
                    return None;
                }
                held.push((*claim, *self.taffy.layout(id.into()).ok()?, self.layout_parent(id)));
            }
        }
        Some(held)
    }

    pub(crate) fn layout_parent(&self, id: LayoutId) -> Option<LayoutId> {
        self.taffy.parent(id.into()).map(LayoutId::from)
    }

    pub(crate) fn transient_count(&self) -> usize {
        self.anonymous_nodes.len()
    }

    // Multi-slot claim validation is still required until all retained
    // records use Fast's native key lists. No global mutation journal.
    pub(crate) fn retained_topology_matches(&self, held: &[HeldLayout]) -> bool {
        held.iter().all(|(claim, _, parent)| {
            let id = claim.id;
            !self.robbed_parents.contains(&id)
                && !self.stale_style_fingerprints.contains(&id)
                && self.persistent_nodes.get(&claim.key).is_some_and(|node| {
                    node.id == id && node.claimed_in_frame == self.request_frame
                })
                && self.layout_parent(id) == *parent
        })
    }

    /// Same final native-layout comparison as Fast's layouts_unchanged.
    pub(crate) fn layouts_unchanged(&self, held: &[HeldLayout]) -> bool {
        self.pending_replays.is_empty()
            && self.retained_topology_matches(held)
            && held.iter().all(|(claim, layout, _)| {
                self.taffy.dirty(claim.id.into()).is_ok_and(|dirty| !dirty)
                    && self.taffy.layout(claim.id.into()).is_ok_and(|now| now == layout)
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
        if gaps.is_empty() {
            return None;
        }
        let mut excluded_request = smallvec::SmallVec::<[Range<usize>; 4]>::new();
        let mut excluded_prepaint = smallvec::SmallVec::<[Range<usize>; 4]>::new();
        for (gap, gap_root) in gaps {
            // Only a root already allocated by the enclosing RequestLayout
            // can be rebuilt early. Its lazy prepaint descendants may be
            // reclaimed later in their original lifecycle.
            if gap.request_blocks().is_empty() || !anchor.covers(gap)
                || !self.layout_reaches(root, *gap_root)
            {
                return None;
            }
            let mut has_root = false;
            for span in gap.request_blocks() {
                let old_gap = self.committed_claim_log.get(span.clone())?;
                if !self.claims_valid(old_gap) { return None; }
                has_root |= old_gap.iter().flatten().any(|claim| claim.id == *gap_root);
                excluded_request.push(span.clone());
            }
            if !has_root { return None; }
            for span in gap.prepaint_blocks() {
                if !self.claims_valid(self.committed_claim_log.get(span.clone())?) {
                    return None;
                }
                excluded_prepaint.push(span.clone());
            }
        }
        // Each phase independently subtracts its dirty blocks. The source
        // ranges may be interleaved physically after previous splices.
        let mut kept = smallvec::SmallVec::<[Range<usize>; 5]>::new();
        Self::subtract_claim_blocks(anchor.request_blocks(), &mut excluded_request, &mut kept)?;
        let request_len: usize = kept.iter().map(Range::len).sum();
        Self::subtract_claim_blocks(anchor.prepaint_blocks(), &mut excluded_prepaint, &mut kept)?;
        let start = self.claimed_key_log.len();
        let mut map = ClaimBlockMap::default();
        let mut target = start;
        for span in &kept {
            map.record(span.clone(), target);
            target += span.len();
        }
        map.sort_by_source();
        let kept = self.try_keep_intervals(&kept, root)?;
        let middle = start + request_len;
        let mut blocks = LayoutClaimBlocks::request(start..middle);
        blocks.push_prepaint(middle..kept.end);
        Some((blocks, map))
    }

    fn subtract_claim_blocks(
        anchors: &[Range<usize>], excluded: &mut [Range<usize>],
        kept: &mut smallvec::SmallVec<[Range<usize>; 5]>,
    ) -> Option<()> {
        excluded.sort_unstable_by_key(|range| range.start);
        if excluded.windows(2).any(|pair| pair[0].end > pair[1].start)
            || anchors.windows(2).any(|pair| pair[0].end > pair[1].start)
        {
            return None;
        }
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
        gaps.next().is_none().then_some(())
    }

    /// Adopt every prebuilt request block atomically. Old slots become
    /// tombstones; nested lifecycle states resolve their original blocks.
    pub(crate) fn adopt_claim_blocks(
        &mut self, blocks: &LayoutClaimBlocks, root: LayoutId,
    ) -> Option<LayoutClaimBlocks> {
        if self.detached_boundary.is_some() || !blocks.prepaint_blocks().is_empty() { return None; }
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

    pub(crate) fn resolve_claim_blocks_in_place(&self, spans: &mut LayoutClaimBlocks) {
        spans.resolve_in_place(&self.claim_relocations);
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
        owner: Option<u64>,
        lingers: bool,
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
        // Never overwrite a committed slot (including lingering offscreen
        // slots) during the append-only transaction. Materialize a transient
        // instead and reject the candidate after normal lifecycle execution.
        let owner = if let Some(boundary) = self.detached_boundary.as_mut() {
            if owner.is_some_and(|key| self.persistent_nodes.contains_key(&key)) {
                boundary.rejected = true;
                None
            } else { owner }
        } else { owner };
        let Some(key) = owner else {
            let style = build_style();
            let id = self.create_taffy_node(style, children, measure.map(MeasureRequest::into_context));
            self.anonymous_nodes.push(id);
            return id;
        };
        // Fast's integer-key claim: duplicate keys in the same frame use a
        // transient node rather than sharing mutable native layout state.
        if let Some(node) = self.persistent_nodes.get_mut(&key) {
            if node.claimed_in_frame == self.request_frame {
                let id = self.create_taffy_node(build_style(), children, measure.map(MeasureRequest::into_context));
                self.anonymous_nodes.push(id);
                return id;
            }
            // A claimed row is active and no longer eligible for idle trim.
            node.offscreen = false;
            node.lingers |= lingers;
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
                // Same invariant as GPUI Fast: a fingerprint hit must mean
                // exactly the native style Taffy holds. Debug builds catch
                // missing fields in the hash without slowing release builds.
                #[cfg(debug_assertions)]
                debug_assert_eq!(
                    self.taffy.style(id.into()).expect(EXPECT_MESSAGE),
                    &build_style(),
                    "layout fingerprint omitted a native style input",
                );
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
                self.contexts.remove(node_key(id.into()));
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
                Self::invalidate_changed_child_origins(
                    &self.taffy, &mut self.unrounded_layout_origins,
                    &mut self.absolute_layout_bounds, &mut self.layout_bounds_scratch_space,
                    Some(id), children,
                );
                Self::detach_reparented_children(&mut self.taffy, &mut self.robbed_parents,
                    Some(id), children);
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
                if let Some(old_context) = self.contexts.get_mut(node_key(id.into())) {
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
                    self.contexts.insert(node_key(id.into()), request.into_context());
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
            if let Some(boundary) = self.detached_boundary.as_mut() {
                boundary.created_keys.push(key);
            }
            self.persistent_nodes.insert(
                key,
                PersistentLayoutNode {
                    id,
                    lingers,
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
    fn detach_reparented_children(
        taffy: &mut TaffyTree<()>,
        robbed_parents: &mut FxHashSet<LayoutId>,
        new_parent: Option<LayoutId>,
        children: &[LayoutId],
    ) {
        for child in children {
            let child = (*child).into();
            if let Some(parent) = taffy.parent(child)
                && Some(LayoutId::from(parent)) != new_parent
            {
                robbed_parents.insert(parent.into());
                // Fast uses set_children for adoption because new_with_children
                // leaves the previous parent's child entry intact.
                taffy.remove_child(parent, child).expect(EXPECT_MESSAGE);
            }
        }
    }

    fn create_taffy_node(
        &mut self,
        style: taffy::Style,
        children: &[LayoutId],
        measure: Option<NodeContext>,
    ) -> LayoutId {
        if self.detached_boundary.is_some() {
            for &child in children {
                self.observe_boundary_write(child);
            }
        }
        self.layout_changes = self.layout_changes.wrapping_add(1);
        if measure.is_some() {
            self.remeasures = self.remeasures.wrapping_add(1);
        }
        Self::invalidate_changed_child_origins(
            &self.taffy, &mut self.unrounded_layout_origins,
            &mut self.absolute_layout_bounds, &mut self.layout_bounds_scratch_space,
            None, children,
        );
        Self::detach_reparented_children(&mut self.taffy, &mut self.robbed_parents,
            None, children);
        let id: LayoutId = match measure {
            Some(context) => {
                let id = self.taffy.new_leaf(style).expect(EXPECT_MESSAGE);
                self.contexts.insert(node_key(id), context);
                id.into()
            }
            None if children.is_empty() => self.taffy.new_leaf(style).expect(EXPECT_MESSAGE).into(),
            None => self
                .taffy
                .new_with_children(style, LayoutId::to_taffy_slice(children))
                .expect(EXPECT_MESSAGE)
                .into(),
        };
        if let Some(boundary) = self.detached_boundary.as_mut() {
            boundary.created.insert(id);
        }
        id
    }

    pub fn request_layout(
        &mut self,
        owner: Option<u64>,
        lingers: bool,
        style: impl std::borrow::Borrow<Style>,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        let _work = crate::window::element_work::begin_named(
            "layout_style_and_fingerprint",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        let mut style = std::borrow::Cow::Borrowed(style.borrow());
        if style.percentage_passthrough && children.len() == 1 {
            self.normalize_percentage_passthrough(style.to_mut(), children);
        }
        let mut style_hasher = FxHasher::default();
        self.hash_layout_style(&style, rem_size, scale_factor, &mut style_hasher);
        let style_fingerprint = style_hasher.finish();
        let layout_id = self.request_persistent_node(
            owner,
            lingers,
            children,
            None::<MeasureRequest<fn() -> Option<NodeMeasureFn>>>,
            Some(style_fingerprint),
            || style.to_taffy(rem_size, scale_factor),
        );
        layout_id
    }

    pub fn request_measured_layout(
        &mut self,
        owner: Option<u64>,
        lingers: bool,
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
        self.request_measured_node(owner, lingers, style, rem_size, scale_factor, MeasureRequest {
            key: None, replayable: false, state_ready: true,
            build: move || Some(boxed_measure(measure)),
        })
    }

    /// Obtain the previous plain text state only for an unclaimed matching slot.
    /// A slot already claimed in this frame cannot lend its mutable text.
    pub(crate) fn previous_plain_text(&mut self, owner: &u64) -> Option<crate::TextLayout> {
        if self.detached_boundary.is_some() { return None; }
        let node = self.persistent_nodes.get(owner)?;
        if node.claimed_in_frame == self.request_frame {
            return None;
        }
        self.contexts.get_mut(node_key(node.id.into()))?.plain_text.clone()
    }

    /// Attach state to the actual node returned by the request (including
    /// transient collision nodes). Replayed views retain their native claims.
    pub(crate) fn set_plain_text(&mut self, id: LayoutId, state: crate::TextLayout) {
        self.observe_boundary_write(id);
        self.contexts.get_mut(node_key(id.into()))
            .expect("plain text has a native measurement context")
            .plain_text = Some(state);
    }

    pub(crate) fn request_size_layout(
        &mut self,
        owner: Option<u64>,
        lingers: bool,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        input: SizeMeasurement,
    ) -> LayoutId {
        self.request_measured_node(owner, lingers, style, rem_size, scale_factor, MeasureRequest {
            key: Some(MeasurementKey::Size(input)), replayable: false, state_ready: true,
            build: || None,
        })
    }

    fn request_measured_node(
        &mut self,
        owner: Option<u64>,
        lingers: bool,
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
        let mut style_hasher = FxHasher::default();
        self.hash_layout_style(&style, rem_size, scale_factor, &mut style_hasher);
        let style_fingerprint = style_hasher.finish();
        let layout_id = self.request_persistent_node(
            owner,
            lingers,
            &[],
            Some(request),
            Some(style_fingerprint),
            || style.to_taffy(rem_size, scale_factor),
        );
        layout_id
    }

    pub(crate) fn request_default_measured_layout(
        &mut self,
        owner: Option<u64>,
        lingers: bool,
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
            let layout = Style::default();
            let mut hasher = FxHasher::default();
            self.hash_layout_style(&layout, rem_size, scale_factor, &mut hasher);
            self.default_measured_style = Some(DefaultMeasuredStyle {
                rem_size,
                scale_bits: scale_factor.to_bits(),
                native: layout.to_taffy(rem_size, scale_factor),
                fingerprint: hasher.finish(),
            });
        }
        let cached = self.default_measured_style.as_ref().expect("default measured style");
        self.request_native_measured_node(
            owner, lingers, cached.native.clone(), cached.fingerprint,
            key, replayable, state_ready, measure,
        )
    }

    fn request_native_measured_node(
        &mut self,
        owner: Option<u64>,
        lingers: bool,
        taffy_style: taffy::Style,
        style_fingerprint: u64,
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
            lingers,
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
        layout_id
    }

    fn replay_pending_measures(&mut self, window: &mut Window, cx: &mut App) {
        let mut pending = std::mem::take(&mut self.pending_replays);
        for id in pending.drain(..) {
            let Some(context) = self.contexts.get_mut(node_key(id.into())) else { continue; };
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
        self.observe_boundary_write(id);
        let scale = window.scale_factor();
        let Ok(layout) = self.taffy.layout(id.into()) else { return; };
        let content = size(
            Pixels((layout.size.width - layout.padding.left - layout.padding.right
                - layout.border.left - layout.border.right).max(0.) / scale),
            Pixels((layout.size.height - layout.padding.top - layout.padding.bottom
                - layout.border.top - layout.border.bottom).max(0.) / scale),
        );
        let Some(context) = self.contexts.get_mut(node_key(id.into())) else { return; };
        if context.replayable {
            context.measure(content.map(Some), content.map(AvailableSpace::Definite), scale, window, cx);
        }
    }

    /// Rebuild a retained subtree in its already allocated parent slot.
    /// Adapted from GPUI Fast's relayout_in_place (fd15f82), Apache-2.0.
    /// A paint-only rebuild need not invoke Taffy at all.
    pub(crate) fn relayout_retained_root(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) -> bool {
        self.observe_boundary_write(id);
        self.replay_pending_measures(window, cx);
        if !self.taffy.dirty(id.into()).expect(EXPECT_MESSAGE)
            && !self.robbed_parents.contains(&id)
        {
            return false;
        }

        let placement = *self.taffy.unrounded_layout(id.into());
        let original = self.taffy.style(id.into()).expect(EXPECT_MESSAGE).clone();
        let mut held = original.clone();
        // Retain the actual device-pixel border box, including a size assigned
        // by flex/grid stretch rather than the subtree's intrinsic size.
        held.size = taffy::geometry::Size {
            width: taffy::style::Dimension::length(placement.size.width),
            height: taffy::style::Dimension::length(placement.size.height),
        };
        held.min_size = taffy::geometry::Size {
            width: taffy::style::LengthPercentageAuto::length(placement.size.width),
            height: taffy::style::LengthPercentageAuto::length(placement.size.height),
        };
        held.max_size = held.min_size;
        held.box_sizing = taffy::style::BoxSizing::BorderBox;
        self.taffy.set_style(id.into(), held).expect(EXPECT_MESSAGE);
        self.compute_layout(id, available_space, window, cx);
        self.taffy.set_style(id.into(), original).expect(EXPECT_MESSAGE);
        self.taffy.restore_unrounded_layout_placement(id.into(), &placement);
        self.bounds_mutations.push(BoundsMutation { id, moved: true });
        self.invalidate_changed_bounds();
        true
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
        self.observe_boundary_write(id);
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
        // A standalone root may already have cached bounds. Observe actual
        // native writes below instead of invalidating every descendant here.

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
        let observe_bounds = !self.unrounded_layout_origins.is_empty()
            || !self.absolute_layout_bounds.is_empty();
        let origins = &self.unrounded_layout_origins;
        let bounds = &self.absolute_layout_bounds;
        let changed_bounds = &mut self.bounds_mutations;
        changed_bounds.clear();
        let mut record_mutation = |node, previous: &taffy::tree::Layout, current: &taffy::tree::Layout| {
            let id = LayoutId::from(node);
            if observe_bounds {
                let moved = previous.location != current.location;
                // A cached descendant implies a cached origin for every ancestor.
                // No cached origin means there is no cached subtree to invalidate.
                if (moved && origins.get(&id).is_some())
                    || (previous.size != current.size && bounds.get(&id).is_some())
                {
                    changed_bounds.push(BoundsMutation { id, moved });
                }
            }
        };
        let observer: Option<&mut dyn FnMut(taffy::NodeId, &taffy::tree::Layout, &taffy::tree::Layout)> =
            if observe_bounds { Some(&mut record_mutation) } else { None };
        let contexts = &mut self.contexts;
        self.taffy
            .compute_layout_with_measure_and_observer(
                id.into(),
                available_space.into(),
                |inputs, node_id, _, style| {
                    let node_context = contexts.get_mut(node_key(node_id));
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
        self.invalidate_changed_bounds();
        if let Some(parent) = parent {
            // A standalone computation overwrites this child's parent-relative
            // placement. Invalidate only ancestor layout caches so the next
            // enclosing computation restores placement from cached measures.
            self.taffy.mark_dirty(parent).expect(EXPECT_MESSAGE);
        }
        self.layout_time = self.layout_time.saturating_add(started_at.elapsed());
    }

    /// A cached descendant implies a cached origin for every ancestor.
    /// Removing the first cached origin also deduplicates overlapping roots.
    /// Call before structural edits, while the old descendant links exist.
    fn invalidate_origin_subtree(
        taffy: &TaffyTree<()>,
        origins: &mut LayoutIdMap<(f32, f32)>,
        bounds: &mut LayoutIdMap<Bounds<Pixels>>,
        stack: &mut Vec<LayoutId>,
        root: LayoutId,
    ) {
        bounds.remove(&root);
        if origins.remove(&root).is_none() {
            return;
        }
        stack.clear();
        stack.extend(taffy.child_ids(root.into()).map(LayoutId::from));
        while let Some(id) = stack.pop() {
            if origins.remove(&id).is_none() {
                continue;
            }
            bounds.remove(&id);
            stack.extend(taffy.child_ids(id.into()).map(LayoutId::from));
        }
    }

    /// Child order alone does not change an already computed absolute
    /// origin. Only changed parent links revoke it immediately; subsequent
    /// native layout writes invalidate any actual position/size changes.
    fn invalidate_changed_child_origins(
        taffy: &TaffyTree<()>,
        origins: &mut LayoutIdMap<(f32, f32)>,
        bounds: &mut LayoutIdMap<Bounds<Pixels>>,
        stack: &mut Vec<LayoutId>,
        parent: Option<LayoutId>,
        children: &[LayoutId],
    ) {
        if origins.is_empty() && bounds.is_empty() {
            return;
        }

        if let Some(parent) = parent {
            // Large sibling sets use membership rather than quadratic
            // comparisons. Small lists avoid a temporary hash allocation.
            let membership = (children.len() > 32)
                .then(|| children.iter().copied().collect::<FxHashSet<_>>());
            for child in taffy.child_ids(parent.into()).map(LayoutId::from) {
                let retained = membership.as_ref().map_or_else(
                    || children.contains(&child), |set| set.contains(&child),
                );
                if !retained {
                    Self::invalidate_origin_subtree(taffy, origins, bounds, stack, child);
                }
            }
        }

        for &child in children {
            // A new parent does not have an ID yet, including when the
            // child was previously a standalone root.
            if parent.is_none() || taffy.parent(child.into()).map(LayoutId::from) != parent {
                Self::invalidate_origin_subtree(taffy, origins, bounds, stack, child);
            }
        }
    }

    fn invalidate_changed_bounds(&mut self) {
        if self.bounds_mutations.is_empty() { return; }
        let _work = crate::window::element_work::begin_named(
            "layout_bounds_invalidation",
            crate::window::element_work::ElementWorkPhase::Layout,
        );
        // Native writes can repeat during measurement/placement. Removing
        // origins as we visit them also deduplicates overlapping dirty roots.
        for mutation in self.bounds_mutations.drain(..) {
            if mutation.moved {
                Self::invalidate_origin_subtree(
                    &self.taffy, &mut self.unrounded_layout_origins,
                    &mut self.absolute_layout_bounds, &mut self.layout_bounds_scratch_space,
                    mutation.id,
                );
            } else {
                self.absolute_layout_bounds.remove(&mutation.id);
            }
        }
    }

    pub(crate) fn frame_layout_time(&self) -> Duration {
        self.layout_time
    }

    pub fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        let scale_bits = scale_factor.to_bits();
        if self.bounds_scale_bits != Some(scale_bits) {
            // Native origins are device-space values. Their validity is
            // independent of the logical conversion; native geometry
            // changes at the new DPI still flow through the observer.
            self.absolute_layout_bounds.clear();
            self.bounds_scale_bits = Some(scale_bits);
        }
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

    /// Forward percentage sizing through an explicitly marked auto-sized
    /// wrapper. The child style is already owned by Taffy: inspect its native
    /// dimensions directly instead of recording a second logical style per node.
    fn normalize_percentage_passthrough(
        &mut self,
        wrapper: &mut Style,
        children: &[LayoutId],
    ) {
        let child_id = children[0];
        let child = self.taffy.style(child_id.into()).expect(EXPECT_MESSAGE);
        let width = promote_percentage_axis(&mut wrapper.size.width, child.size.width);
        let height = promote_percentage_axis(&mut wrapper.size.height, child.size.height);
        if !width && !height {
            return;
        }

        let mut native = child.clone();
        if width {
            native.size.width = taffy::style::Dimension::percent(1.0);
        }
        if height {
            native.size.height = taffy::style::Dimension::percent(1.0);
        }
        if child != &native {
            self.observe_boundary_write(child_id);
            self.taffy.set_style(child_id.into(), native).expect(EXPECT_MESSAGE);
            self.layout_changes = self.layout_changes.wrapping_add(1);
            // Rewritten child styles must be restored on their next request;
            // never allow a retained claim to bypass that restoration.
            self.stale_style_fingerprints.insert(child_id);
        }
    }

    fn hash_layout_style(
        &self,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        hasher: &mut FxHasher,
    ) {
        hash_display(style.display, hasher);
        hash_overflow_point(&style.overflow, hasher);
        hash_absolute_length(style.scrollbar_width, hasher);
        hash_position(style.position, hasher);
        hash_edges_length(&style.inset, hasher);
        hash_size_length(&style.size, hasher);
        hash_size_length(&style.min_size, hasher);
        hash_size_length(&style.max_size, hasher);
        style.aspect_ratio.map(f32::to_bits).hash(hasher);
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
    use crate::{ElementId, GlobalElementId};

    fn row_id(index: usize) -> GlobalElementId {
        GlobalElementId::from_path(&[
            ElementId::from("list"),
            ElementId::named_usize("gpui::list_item", index),
            ElementId::from("leaf"),
        ])
    }

    fn list_id() -> GlobalElementId {
        GlobalElementId::from_path(&[ElementId::from("list")])
    }

    fn owner(id: &GlobalElementId) -> u64 {
        crate::fast::layout_key::key_for_path(id.0.as_ref())
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
    fn detached_boundary_commit_preserves_old_claim_coordinates_and_native_ids() {
        let mut engine = TaffyLayoutEngine::new();
        let list = list_id();
        let exit = row_id(1);
        let keep = row_id(2);
        let enter = row_id(3);
        let root = engine.request_layout(Some(owner(&list)), false, Style::default(), crate::px(16.), 1., &[]);
        let leaving = engine.request_layout(Some(owner(&exit)), true, Style::default(), crate::px(16.), 1., &[]);
        let kept = engine.request_layout(Some(owner(&keep)), true, Style::default(), crate::px(16.), 1., &[]);
        engine.clear();
        let before = engine.committed_claim_log.clone();
        let retired = LayoutClaimBlocks::prepaint(1..2);
        assert!(engine.begin_detached_boundary(&[(leaving, retired.clone()), (leaving, retired.clone())]).is_none());
        assert_eq!(engine.begin_detached_boundary(&[(leaving, retired)]), Some(3));
        let entering = engine.request_layout(Some(owner(&enter)), true, Style::default(), crate::px(16.), 1., &[]);
        assert_eq!(engine.committed_claim_log, before);
        let (captured_root, local) = engine.capture_detached_row_range(0..1).unwrap();
        assert_eq!(captured_root, entering);
        let entering_claims = engine.detached_boundary_row_claims(entering, &local).unwrap();
        assert_eq!(entering_claims.prepaint_blocks(), &[3..4]);
        let mut enclosing = LayoutClaimBlocks::request(0..1);
        enclosing.push_prepaint(1..3);
        let projected = engine.detached_boundary_ancestor_claims(&enclosing).unwrap();
        assert_eq!(projected.request_blocks(), &[0..1]);
        assert_eq!(projected.prepaint_blocks(), &[2..4]);
        assert!(projected.covers(&entering_claims));
        assert!(engine.detached_boundary_ancestor_claims(&LayoutClaimBlocks::prepaint(2..3)).is_none());
        engine.commit_detached_boundary().unwrap();
        assert_eq!(engine.committed_claim_log.len(), 4);
        assert_eq!(engine.committed_claim_log[0], before[0]);
        assert!(engine.committed_claim_log[1].is_none());
        assert_eq!(engine.committed_claim_log[2], before[2]);
        assert_eq!(engine.committed_claim_log[3].unwrap().id, entering);
        assert_eq!(engine.persistent_nodes[&owner(&list)].id, root);
        assert_eq!(engine.persistent_nodes[&owner(&keep)].id, kept);
        assert!(!engine.persistent_nodes.contains_key(&owner(&exit)));
        assert_eq!(engine.claimed_this_frame, 0);
        let epoch = engine.request_frame;
        let kept_stamp = engine.persistent_nodes[&owner(&keep)].claimed_in_frame;
        // Reverse scroll must be able to recreate the retired key while
        // retaining both the enclosing layout and the other row.
        assert_eq!(engine.begin_detached_boundary(&[(entering, entering_claims)]), Some(4));
        let returning = engine.request_layout(Some(owner(&exit)), true, Style::default(), crate::px(16.), 1., &[]);
        assert_ne!(returning, leaving);
        engine.commit_detached_boundary().unwrap();
        assert_eq!(engine.request_frame, epoch);
        assert_eq!(engine.persistent_nodes[&owner(&keep)].claimed_in_frame, kept_stamp);
        assert!(!engine.persistent_nodes.contains_key(&owner(&enter)));
        assert_eq!(engine.detached_claim_proof, Some((epoch, engine.layout_changes)));
        assert!(engine.committed_claim_log[3].is_none());
        assert_eq!(engine.committed_claim_log[4].unwrap().id, returning);
        assert!(engine.try_keep_retained(&LayoutClaimBlocks::request(0..1), root).is_some());
        assert!(engine.try_keep_detached_row_claims(&LayoutClaimBlocks::prepaint(2..3), kept).is_some());
        assert!(engine.try_keep_detached_row_claims(&LayoutClaimBlocks::prepaint(4..5), returning).is_some());
    }

    #[test]
    fn detached_boundary_cancel_removes_only_new_nodes_and_rejects_colliding_keys() {
        let mut engine = TaffyLayoutEngine::new();
        let key = row_id(1);
        let root = engine.request_layout(Some(owner(&key)), true, Style::default(), crate::px(16.), 1., &[]);
        engine.clear();
        let before = engine.committed_claim_log.clone();
        let epoch = engine.request_frame;
        assert_eq!(engine.begin_detached_boundary(&[]), Some(1));
        let child = engine.request_layout(Some(owner(&row_id(2))), true, Style::default(), crate::px(16.), 1., &[]);
        let parent = engine.request_layout(Some(owner(&row_id(3))), true, Style::default(), crate::px(16.), 1., &[child]);
        let duplicate = engine.request_layout(Some(owner(&key)), true, Style::default(), crate::px(16.), 1., &[]);
        assert_ne!(duplicate, root);
        assert_ne!(parent, root);
        assert!(engine.commit_detached_boundary().is_none());
        assert!(engine.cancel_detached_boundary());
        assert_eq!(engine.request_frame, epoch);
        assert_eq!(engine.committed_claim_log, before);
        assert_eq!(engine.taffy.total_node_count(), 1);
        assert_eq!(engine.persistent_nodes[&owner(&key)].id, root);
        assert!(engine.try_keep_detached_row_claims(&LayoutClaimBlocks::prepaint(0..1), root).is_some());
    }

    #[test]
    fn detached_boundary_external_edge_forces_native_recovery() {
        let mut engine = TaffyLayoutEngine::new();
        let child = engine.request_layout(Some(owner(&row_id(1))), true, Style::default(), crate::px(16.), 1., &[]);
        let root = engine.request_layout(Some(owner(&row_id(2))), true, Style::default(), crate::px(16.), 1., &[child]);
        engine.clear();
        assert_eq!(engine.begin_detached_boundary(&[]), Some(2));
        engine.request_layout(Some(owner(&row_id(3))), true, Style::default(), crate::px(16.), 1., &[child]);
        assert!(engine.commit_detached_boundary().is_none());
        assert!(!engine.cancel_detached_boundary());
        assert_eq!(engine.taffy.total_node_count(), 0);
        assert!(engine.try_keep_detached_row_claims(&LayoutClaimBlocks::prepaint(0..2), root).is_none());
    }

    #[test]
    fn detached_row_claims_keep_native_subtree_without_claiming_unrelated_nodes() {
        let mut engine = TaffyLayoutEngine::new();
        let child_key = row_id(1);
        let root_key = row_id(2);
        let unrelated_key = list_id();
        let child = engine.request_layout(Some(owner(&child_key)), true, Style::default(), crate::px(16.), 1., &[]);
        let root = engine.request_layout(Some(owner(&root_key)), true, Style::default(), crate::px(16.), 1., &[child]);
        let unrelated = engine.request_layout(Some(owner(&unrelated_key)), false, Style::default(), crate::px(16.), 1., &[]);
        let claims = engine.capture_detached_row_claims(0..2, root).unwrap();
        assert!(engine.capture_detached_row_claims(1..2, root).is_none());
        assert!(engine.capture_detached_row_claims(0..3, root).is_none());
        assert!(engine.capture_detached_row_claims(0..1, child).is_none());
        assert!(engine.capture_detached_row_claims(2..3, unrelated).is_none());
        engine.clear();
        let kept = engine.try_keep_detached_row_claims(&claims, root).unwrap();
        assert_eq!(engine.claimed_this_frame, 2);
        assert_eq!(kept.prepaint_blocks(), &[0..2]);
        assert!(engine.try_keep_detached_row_claims(&claims, root).is_none());
        engine.release_kept(&kept);
        assert_eq!(engine.claimed_this_frame, 0);
        engine.abort_frame();
        assert!(engine.try_keep_detached_row_claims(&claims, root).is_none());
    }

    #[test]
    fn subtree_claims_nest_and_rollback_atomically() {
        let mut engine = TaffyLayoutEngine::new();
        let root = list_id();
        let row = row_id(2);
        let first = engine.request_layout(Some(owner(&root)), false, Style::default(), crate::px(16.), 1., &[]);
        let start = engine.claim_log_len();
        engine.request_layout(Some(owner(&row)), false, Style::default(), crate::px(16.), 1., &[]);
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
        assert_eq!(engine.request_layout(Some(owner(&root)), false, Style::default(), crate::px(16.), 1., &[]), first);
        assert!(engine.try_keep_retained(&claims, first).is_none());
        assert_eq!(engine.claimed_this_frame, 1);
    }

    #[test]
    fn gap_claim_keeps_only_native_prefix_and_suffix() {
        let mut engine = TaffyLayoutEngine::new();
        let gap_owner = row_id(2);
        let root_owner = list_id();
        let gap = engine.request_layout(Some(owner(&gap_owner)), false, Style::default(), crate::px(16.), 1., &[]);
        let root = engine.request_layout(Some(owner(&root_owner)), false, Style::default(), crate::px(16.), 1., &[gap]);
        engine.clear();
        let anchor = LayoutClaimBlocks::request(0..2);
        let gap_span = LayoutClaimBlocks::request(0..1);
        let stamp = engine.splice_layout_stamp();
        let (kept, _) = engine.try_keep_around_gaps(&anchor, &[(gap_span, gap)], root).unwrap();
        assert_eq!(engine.claimed_this_frame, 1);
        assert_eq!(engine.request_layout(Some(owner(&gap_owner)), false, Style::default(), crate::px(16.), 1., &[]), gap);
        assert_eq!(engine.splice_layout_stamp(), stamp);
        engine.release_kept(&kept);
        assert_eq!(engine.claimed_this_frame, 1);
        assert_eq!(engine.request_layout(Some(owner(&root_owner)), false, Style::default(), crate::px(16.), 1., &[gap]), root);
    }

    #[test]
    fn subtree_claim_refuses_robbed_layout_nodes() {
        let mut engine = TaffyLayoutEngine::new();
        let root = list_id();
        let start = engine.claim_log_len();
        let id = engine.request_layout(Some(owner(&root)), false, Style::default(), crate::px(16.), 1., &[]);
        let claims = LayoutClaimBlocks::request(start..engine.claim_log_len());
        engine.clear();
        engine.robbed_parents.insert(id);
        assert!(engine.try_keep_retained(&claims, id).is_none());
        assert_eq!(engine.claimed_this_frame, 0);
    }

    #[test]
    fn interaction_flags_do_not_change_native_style_fingerprint() {
        let engine = TaffyLayoutEngine::new();
        let original = Style::default();
        let mut changed = original.clone();
        changed.allow_concurrent_scroll = !original.allow_concurrent_scroll;
        changed.restrict_scroll_to_axis = !original.restrict_scroll_to_axis;
        changed.percentage_passthrough = !original.percentage_passthrough;
        let fingerprint = |style: &Style| {
            let mut hasher = FxHasher::default();
            engine.hash_layout_style(style, crate::px(16.), 1., &mut hasher);
            hasher.finish()
        };
        assert_eq!(fingerprint(&original), fingerprint(&changed));
        assert_eq!(original.to_taffy(crate::px(16.), 1.), changed.to_taffy(crate::px(16.), 1.));
    }

    #[test]
    fn measurement_context_follows_native_node_lifetime() {
        let mut engine = TaffyLayoutEngine::new();
        let key = owner(&list_id());
        let measured = engine.request_size_layout(
            Some(key), false, Style::default(), crate::px(16.), 1.,
            SizeMeasurement::Intrinsic(crate::size(crate::px(20.), crate::px(10.))),
        );
        assert!(engine.contexts.get(node_key(measured.into())).is_some());
        engine.clear();

        let plain = engine.request_layout(
            Some(key), false, Style::default(), crate::px(16.), 1., &[],
        );
        assert_eq!(plain, measured);
        assert!(engine.contexts.get(node_key(plain.into())).is_none());
        engine.clear();

        let measured = engine.request_size_layout(
            Some(key), false, Style::default(), crate::px(16.), 1.,
            SizeMeasurement::Intrinsic(crate::size(crate::px(30.), crate::px(10.))),
        );
        assert_eq!(plain, measured);
        assert!(engine.contexts.get(node_key(measured.into())).is_some());
        engine.clear();
        engine.clear();
        assert!(engine.contexts.get(node_key(measured.into())).is_none());
        assert!(engine.contexts.is_empty());
    }

    #[test]
    fn duplicate_fast_layout_keys_use_a_transient_node() {
        let mut engine = TaffyLayoutEngine::new();
        let key = owner(&list_id());
        let first = engine.request_layout(
            Some(key), false, Style::default(), crate::px(16.), 1., &[],
        );
        let duplicate = engine.request_layout(
            Some(key), false, Style::default(), crate::px(16.), 1., &[],
        );
        assert_ne!(first, duplicate);
        assert_eq!(engine.persistent_nodes.len(), 1);
        assert_eq!(engine.transient_count(), 1);
        engine.clear();
        assert_eq!(engine.taffy.total_node_count(), 1);
    }

    #[test]
    fn reparenting_marks_previous_parent_robbed() {
        let mut engine = TaffyLayoutEngine::new();
        let child = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[]);
        let old = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[child]);
        let ancestor = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[old]);
        let next = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[child]);
        assert_eq!(engine.taffy.parent(child.into()), Some(next.into()));
        assert_eq!(engine.taffy.child_count(old.into()), 0);
        assert!(engine.robbed_parents.contains(&old));
        assert!(engine.taffy.dirty(old.into()).unwrap());
        assert!(engine.taffy.dirty(ancestor.into()).unwrap());
        engine.clear();
        assert!(engine.robbed_parents.is_empty());
    }

    #[test]
    fn reparenting_and_removal_preserve_unrelated_absolute_bounds() {
        let mut engine = TaffyLayoutEngine::new();
        let leaf = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[]);
        let branch = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[leaf]);
        let sibling = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[]);
        let old_parent = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[branch, sibling]);

        // Seed real cache entries through the normal lookup (the native
        // uncomputed layouts have their initial geometry).
        engine.layout_bounds(leaf, 1.);
        let sibling_bounds = engine.layout_bounds(sibling, 1.);
        assert!(engine.unrounded_layout_origins.get(&old_parent).is_some());
        let new_parent = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[branch]);
        assert!(engine.unrounded_layout_origins.get(&branch).is_none());
        assert!(engine.absolute_layout_bounds.get(&leaf).is_none());
        assert_eq!(engine.absolute_layout_bounds.get(&sibling), Some(&sibling_bounds));

        engine.layout_bounds(leaf, 1.);
        engine.remove_native_node(new_parent);
        assert!(engine.unrounded_layout_origins.get(&branch).is_none());
        assert!(engine.absolute_layout_bounds.get(&leaf).is_none());
        assert_eq!(engine.absolute_layout_bounds.get(&sibling), Some(&sibling_bounds));
    }

    #[test]
    fn child_order_change_keeps_origins_until_native_layout_changes() {
        let mut engine = TaffyLayoutEngine::new();
        let first = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[]);
        let second = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[]);
        let parent = engine.request_layout(None, false, Style::default(), crate::px(16.), 1., &[first, second]);
        engine.layout_bounds(first, 1.);
        engine.layout_bounds(second, 1.);
        TaffyLayoutEngine::invalidate_changed_child_origins(
            &engine.taffy, &mut engine.unrounded_layout_origins,
            &mut engine.absolute_layout_bounds, &mut engine.layout_bounds_scratch_space,
            Some(parent), &[second, first],
        );
        assert!(engine.absolute_layout_bounds.get(&first).is_some());
        assert!(engine.absolute_layout_bounds.get(&second).is_some());
        TaffyLayoutEngine::invalidate_changed_child_origins(
            &engine.taffy, &mut engine.unrounded_layout_origins,
            &mut engine.absolute_layout_bounds, &mut engine.layout_bounds_scratch_space,
            Some(parent), &[second],
        );
        assert!(engine.absolute_layout_bounds.get(&first).is_none());
        assert!(engine.absolute_layout_bounds.get(&second).is_some());
    }

    #[test]
    fn pressure_retirement_invalidates_detached_child_not_clean_sibling() {
        let mut engine = TaffyLayoutEngine::new();
        let leaf_key = owner(&row_id(1));
        let branch_key = owner(&row_id(2));
        let sibling_key = owner(&row_id(3));
        let root_key = owner(&list_id());
        let leaf = engine.request_layout(Some(leaf_key), true, Style::default(), crate::px(16.), 1., &[]);
        let branch = engine.request_layout(Some(branch_key), true, Style::default(), crate::px(16.), 1., &[leaf]);
        let sibling = engine.request_layout(Some(sibling_key), true, Style::default(), crate::px(16.), 1., &[]);
        let root = engine.request_layout(Some(root_key), false, Style::default(), crate::px(16.), 1., &[branch, sibling]);
        engine.layout_bounds(leaf, 1.);
        let sibling_bounds = engine.layout_bounds(sibling, 1.);
        engine.persistent_nodes.get_mut(&branch_key).unwrap().offscreen = true;

        engine.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);

        assert!(!engine.persistent_nodes.contains_key(&branch_key));
        assert!(engine.persistent_nodes.contains_key(&leaf_key));
        assert!(engine.unrounded_layout_origins.get(&leaf).is_none());
        assert!(engine.absolute_layout_bounds.get(&leaf).is_none());
        assert_eq!(engine.absolute_layout_bounds.get(&sibling), Some(&sibling_bounds));
        assert!(engine.robbed_parents.contains(&root));
    }

    #[test]
    fn offscreen_rows_reuse_layout_and_expire_on_trim() {
        let mut engine = TaffyLayoutEngine::new();
        let row = row_id(7);
        let first = engine.request_layout(Some(owner(&row)), true, Style::default(), crate::px(16.), 1., &[]);
        engine.clear();
        engine.clear();
        assert_eq!(engine.taffy.total_node_count(), 1);
        let again = engine.request_layout(Some(owner(&row)), true, Style::default(), crate::px(16.), 1., &[]);
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
        engine.request_layout(Some(owner(&row)), true, Style::default(), crate::px(16.), 1., &[]);
        engine.clear();
        for _ in 0..ROW_NODE_LINGER_FRAMES {
            engine.clear();
        }
        assert_eq!(engine.taffy.total_node_count(), 0);
        let start = engine.claim_log_len();
        let id = engine.request_layout(Some(owner(&row)), true, Style::default(), crate::px(16.), 1., &[]);
        let claims = LayoutClaimBlocks::request(start..engine.claim_log_len());
        engine.abort_frame();
        assert_eq!(engine.taffy.total_node_count(), 0);
        assert!(engine.try_keep_retained(&claims, id).is_none());
        let new = engine.request_layout(Some(owner(&row)), true, Style::default(), crate::px(16.), 1., &[]);
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
