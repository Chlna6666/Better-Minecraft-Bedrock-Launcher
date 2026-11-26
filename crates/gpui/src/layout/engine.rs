use crate::{
    App, Bounds, DefiniteLength, ElementId, GlobalElementId, GpuiMemoryTrimLevel, LayoutStyle, Length, Pixels,
    Size, Style, Window, point, relative, size,
};
use collections::{FxHashMap, FxHashSet, FxHasher};
use std::{
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};
use taffy::{TaffyTree, TraversePartialTree as _};

use super::{
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

struct LayoutRootDiagnosticNode {
    style_fingerprint: u64,
    subtree_fingerprint: Option<u64>,
    child_count: usize,
}

pub(super) struct NodeContext {
    pub(super) measure: NodeMeasureFn,
    pub(super) is_pure: bool,
    pub(super) last_measure_input: Option<(Size<Option<Pixels>>, Size<AvailableSpace>)>,
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

struct PersistentLayoutNode {
    id: LayoutId,
    /// Structural equality guards key collisions; no frame metadata lookup.
    element: GlobalElementId,
    measure_fingerprint: Option<u64>,
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
    open_key_recordings: usize,
    layout_changes: u64,
    remeasures: u64,
    robbed_parents: FxHashSet<LayoutId>,
    default_measured_style: Option<DefaultMeasuredStyle>,
    pub(super) absolute_layout_bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    unrounded_layout_origins: FxHashMap<LayoutId, (f32, f32)>,
    pub(super) computed_layouts: FxHashSet<LayoutId>,
    pub(super) node_fingerprints: FxHashMap<LayoutId, Option<u64>>,
    node_layout_metadata: FxHashMap<LayoutId, NodeLayoutMetadata>,
    // Taffy stores device-pixel geometry; equal logical constraints at a new
    // DPI must not reuse the previous physical layout.
    root_constraints: FxHashMap<LayoutId, (Size<AvailableSpaceKey>, u32)>,
    pub(super) subtree_scratch: Vec<LayoutId>,
    layout_bounds_scratch_space: Vec<LayoutId>,
    root_diagnostic_ids: FxHashMap<LayoutId, GlobalElementId>,
    previous_root_diagnostics: FxHashMap<GlobalElementId, Vec<LayoutRootDiagnosticNode>>,
    current_root_diagnostics: FxHashMap<GlobalElementId, Vec<LayoutRootDiagnosticNode>>,
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
            open_key_recordings: 0,
            layout_changes: 0,
            remeasures: 0,
            robbed_parents: FxHashSet::default(),
            default_measured_style: None,
            absolute_layout_bounds: FxHashMap::default(),
            unrounded_layout_origins: FxHashMap::default(),
            computed_layouts: FxHashSet::default(),
            node_fingerprints: FxHashMap::default(),
            node_layout_metadata: FxHashMap::default(),
            root_constraints: FxHashMap::default(),
            subtree_scratch: Vec::new(),
            layout_bounds_scratch_space: Vec::new(),
            root_diagnostic_ids: FxHashMap::default(),
            previous_root_diagnostics: FxHashMap::default(),
            current_root_diagnostics: FxHashMap::default(),
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
            if node.measured && taffy.get_node_context_mut(node.id.into())
                .is_some_and(|context| !context.is_pure)
            {
                taffy.set_node_context(node.id.into(), None).expect(EXPECT_MESSAGE);
                taffy.mark_dirty(node.id.into()).expect(EXPECT_MESSAGE);
                node.measured = false;
                node.measure_fingerprint = None;
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
    }

    pub fn clear(&mut self) {
        // Observe the frame that just finished before clearing it. This is the real active layout
        // working set and lets a large-to-small page transition return excess buckets without
        // disturbing stable large pages.
        let absolute_layout_count = self.absolute_layout_bounds.len();
        let unrounded_origin_count = self.unrounded_layout_origins.len();
        let computed_layout_count = self.computed_layouts.len();
        let fingerprint_count = self.node_fingerprints.len();
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
        debug_assert_eq!(self.open_key_recordings, 0);
        self.open_key_recordings = 0;
        self.claimed_key_log.clear();
        std::mem::swap(
            &mut self.previous_root_diagnostics,
            &mut self.current_root_diagnostics,
        );
        self.current_root_diagnostics.clear();
        self.root_diagnostic_ids.clear();
        if !log::log_enabled!(log::Level::Trace) {
            self.previous_root_diagnostics = FxHashMap::default();
            self.current_root_diagnostics = FxHashMap::default();
            self.root_diagnostic_ids = FxHashMap::default();
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
        self.node_fingerprints.clear();
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
        trim_to_working_set!(self.node_fingerprints, fingerprint_count);
        trim_to_working_set!(self.node_layout_metadata, metadata_count);
        trim_to_working_set!(self.root_constraints, self.persistent_nodes.len());
        trim_to_working_set!(self.claimed_key_log, node_working_set);
        trim_to_working_set!(self.subtree_scratch, node_working_set);
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

    pub(crate) fn register_root_diagnostic_id(&mut self, id: LayoutId, identity: &GlobalElementId) {
        if log::log_enabled!(log::Level::Trace) {
            self.root_diagnostic_ids.insert(id, identity.clone());
        }
    }

    fn record_root_diagnostic(&mut self, id: LayoutId, cache_hit: bool) {
        let Some(identity) = self.root_diagnostic_ids.get(&id).cloned() else {
            return;
        };
        let mut nodes = std::mem::take(&mut self.subtree_scratch);
        nodes.clear();
        self.collect_subtree_nodes_into(id, &mut nodes);
        let mut snapshot = Vec::with_capacity(nodes.len());
        for node_id in &nodes {
            let Some(metadata) = self.node_layout_metadata.get(node_id) else {
                // Trace logging can be enabled after the request or a retained
                // child may not have been requested in this frame.
                nodes.clear();
                self.subtree_scratch = nodes;
                return;
            };
            let mut hasher = FxHasher::default();
            self.hash_layout_style(
                &metadata.style,
                metadata.rem_size,
                metadata.scale_factor,
                &mut hasher,
            );
            snapshot.push(LayoutRootDiagnosticNode {
                style_fingerprint: hasher.finish(),
                subtree_fingerprint: self.node_fingerprints[node_id],
                child_count: self.taffy.child_count((*node_id).into()),
            });
        }
        if !cache_hit {
            if let Some(previous) = self.previous_root_diagnostics.get(&identity) {
                let divergence =
                    previous
                        .iter()
                        .zip(&snapshot)
                        .enumerate()
                        .find_map(|(index, (old, new))| {
                            if old.child_count != new.child_count {
                                Some((index, "children"))
                            } else if old.style_fingerprint != new.style_fingerprint {
                                Some((index, "style"))
                            } else if old.child_count == 0
                                && old.subtree_fingerprint != new.subtree_fingerprint
                            {
                                Some((index, "measured_content"))
                            } else {
                                None
                            }
                        });
                let divergence = divergence.or_else(|| {
                    (previous.len() != snapshot.len())
                        .then_some((previous.len().min(snapshot.len()), "node_count"))
                });
                if let Some((index, reason)) = divergence {
                    log::trace!(
                        "gpui layout root cache divergence: root={} preorder_node={} reason={} old_nodes={} new_nodes={}",
                        identity,
                        index,
                        reason,
                        previous.len(),
                        snapshot.len(),
                    );
                } else {
                    log::trace!(
                        "gpui layout root cache miss without local divergence: root={} nodes={} (constraints or unavailable fingerprint)",
                        identity,
                        snapshot.len(),
                    );
                }
            } else {
                log::trace!(
                    "gpui layout root cache miss without prior root: root={} nodes={}",
                    identity,
                    snapshot.len(),
                );
            }
        }
        self.current_root_diagnostics.insert(identity, snapshot);
        nodes.clear();
        self.subtree_scratch = nodes;
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
        self.node_fingerprints
            .shrink_to(floor.max(self.node_fingerprints.len()));
        self.node_layout_metadata
            .shrink_to(floor.max(self.node_layout_metadata.len()));
        self.subtree_scratch
            .shrink_to(floor.max(self.subtree_scratch.len()));
        self.layout_bounds_scratch_space
            .shrink_to(floor.max(self.layout_bounds_scratch_space.len()));
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.previous_root_diagnostics.clear();
            self.current_root_diagnostics.clear();
        }
        self.root_diagnostic_ids
            .shrink_to(floor.max(self.root_diagnostic_ids.len()));
        self.previous_root_diagnostics
            .shrink_to(floor.max(self.previous_root_diagnostics.len()));
        self.current_root_diagnostics
            .shrink_to(floor.max(self.current_root_diagnostics.len()));
        if matches!(level, GpuiMemoryTrimLevel::Aggressive) {
            self.anonymous_nodes.shrink_to_fit();
            self.claimed_key_log.shrink_to_fit();
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
    pub(crate) fn record_claimed_keys(&mut self) -> usize {
        self.open_key_recordings += 1;
        self.claimed_key_log.len()
    }

    /// Ends the recording started at `start`, returning the keys it saw.
    pub(crate) fn finish_recording_claimed_keys(&mut self, start: usize) -> Vec<LayoutClaim> {
        let keys = self.claimed_key_log[start..].iter().flatten().copied().collect();
        self.open_key_recordings -= 1;
        if self.open_key_recordings == 0 {
            self.claimed_key_log.clear();
        }
        keys
    }


    /// Every node must still have the recorded identity and be unclaimed.
    /// Validate the complete slice before modifying any request stamps.
    pub(crate) fn try_keep_retained(&mut self, claims: &[LayoutClaim]) -> bool {
        let frame = self.request_frame;
        let all_there = claims.iter().all(|claim| {
            self.persistent_nodes.get(&claim.key).is_some_and(|node| {
                node.id == claim.id
                    && node.claimed_in_frame != frame
                    && !node.offscreen
                    && !self.robbed_parents.contains(&node.id)
            })
        });
        if !all_there {
            return false;
        }
        for claim in claims {
            let node = self.persistent_nodes.get_mut(&claim.key)
                .expect("validated retained layout claim");
            // Recordings produce unique claims; this also handles duplicate
            // input defensively without inflating the live-node count.
            if node.claimed_in_frame != frame {
                node.claimed_in_frame = frame;
                self.claimed_this_frame += 1;
                if self.open_key_recordings > 0 {
                    self.claimed_key_log.push(Some(*claim));
                }
            }
        }
        true
    }

    /// Undo a just-acquired transaction before rebuilding the subtree.
    /// Released entries become tombstones: nested recording offsets stay stable.
    pub(crate) fn release_kept(&mut self, claims: &[LayoutClaim]) {
        let frame = self.request_frame;
        for claim in claims {
            if let Some(node) = self.persistent_nodes.get_mut(&claim.key)
                && node.id == claim.id && node.claimed_in_frame == frame
            {
                node.claimed_in_frame = frame.wrapping_sub(1);
                self.claimed_this_frame -= 1;
            }
        }
        if self.open_key_recordings > 0 {
            let released: FxHashSet<_> = claims.iter().copied().collect();
            for entry in &mut self.claimed_key_log {
                if entry.is_some_and(|claim| released.contains(&claim)) {
                    *entry = None;
                }
            }
        }
    }

    pub(crate) fn claim_is_current(&self, claim: &LayoutClaim) -> bool {
        self.persistent_nodes.get(&claim.key).is_some_and(|node| {
            node.id == claim.id && node.claimed_in_frame == self.request_frame
        })
    }

    /// Reconcile the retained Element's native layout nodes. Only genuinely
    /// changed styles/child edges/measure inputs dirty Taffy's cached layouts.
    fn request_persistent_node(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        children: &[LayoutId],
        measure: Option<NodeContext>,
        measure_fingerprint: Option<u64>,
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
            let id = self.create_taffy_node(style, children, measure);
            self.anonymous_nodes.push(id);
            return id;
        };
        // As in Fast's claim path, resolve the native node with one map probe.
        // Unlike a hash-only identity, keep the full collision check. A collided
        // slot is transient for this request; do not overwrite or leak its owner.
        if let Some(node) = self.persistent_nodes.get_mut(&key) {
            if node.element != element || node.claimed_in_frame == self.request_frame {
                let id = self.create_taffy_node(build_style(), children, measure);
                self.anonymous_nodes.push(id);
                return id;
            }
            // A claimed row is active and no longer eligible for idle trim.
            node.offscreen = false;
            node.claimed_in_frame = self.request_frame;
            self.claimed_this_frame += 1;
            let id = node.id;
            if self.open_key_recordings > 0 {
                self.claimed_key_log.push(Some(LayoutClaim { key, id }));
            }
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
                    &mut self.node_fingerprints, Some(id), children) {
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
            if let Some(mut context) = measure {
                // A pure measure declares that its result depends only on the fingerprint, so
                // an equal fingerprint with an unchanged style means the node can keep Taffy's
                // cache. An impure measure may close over mutable state the fingerprint does
                // not cover, so it keeps being marked dirty every frame; its own cache is what
                // keeps that cheap.
                let unchanged_pure = node.measured
                    && context.is_pure
                    && node.measure_fingerprint == measure_fingerprint
                    && !requires_style;
                if let Some(old_context) = self.taffy.get_node_context_mut(id.into()) {
                    let reusable = unchanged_pure && old_context.is_pure;
                    if reusable {
                        context.last_measure_input = old_context.last_measure_input;
                    }
                    *old_context = context;
                    if !reusable {
                        self.taffy.mark_dirty(id.into()).expect(EXPECT_MESSAGE);
                        self.layout_changes = self.layout_changes.wrapping_add(1);
                        self.remeasures = self.remeasures.wrapping_add(1);
                    }
                } else {
                    self.taffy
                        .set_node_context(id.into(), Some(context))
                        .expect(EXPECT_MESSAGE);
                    self.taffy.mark_dirty(id.into()).expect(EXPECT_MESSAGE);
                    self.layout_changes = self.layout_changes.wrapping_add(1);
                    self.remeasures = self.remeasures.wrapping_add(1);
                }
            }
            node.measure_fingerprint = measure_fingerprint;
            node.measured = is_measured;
            self.persistent_layout_reuses += 1;
            id
        } else {
            let is_measured = measure.is_some();
            let id = self.create_taffy_node(build_style(), children, measure);
            self.claimed_this_frame += 1;
            if self.open_key_recordings > 0 {
                self.claimed_key_log.push(Some(LayoutClaim { key, id }));
            }
            self.persistent_nodes.insert(
                key,
                PersistentLayoutNode {
                    id,
                    lingers: is_list_row(&element.0),
                    element,
                    measure_fingerprint,
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
    // Invalidate already-computed recursive proofs too: BMCBL still uses
    // these for replay until Fast's subtree claim transaction takes over.
    fn note_reparented_children(
        taffy: &mut TaffyTree<NodeContext>,
        robbed_parents: &mut FxHashSet<LayoutId>,
        fingerprints: &mut FxHashMap<LayoutId, Option<u64>>,
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
                let mut ancestor = Some(parent);
                while let Some(id) = ancestor {
                    fingerprints.remove(&LayoutId::from(id));
                    ancestor = taffy.parent(id);
                }
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
            &mut self.node_fingerprints, None, children) {
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
        let fingerprint = self.layout_fingerprint(style_fingerprint, children);
        let pending_style = layout_style.clone();
        let layout_id = self.request_persistent_node(
            owner,
            children,
            None,
            None,
            Some(style_fingerprint),
            move || pending_style.to_taffy(rem_size, scale_factor),
        );
        self.node_fingerprints.insert(layout_id, fingerprint);
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
        self.request_fingerprinted_layout(owner, style, rem_size, scale_factor, None, measure)
    }

    pub fn request_fingerprinted_layout(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        fingerprint_seed: Option<u64>,
        measure: impl FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels>
        + 'static,
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
        let fingerprint = fingerprint_seed.map(|seed| {
            let mut hasher = FxHasher::default();
            style_fingerprint.hash(&mut hasher);
            seed.hash(&mut hasher);
            hasher.finish()
        });
        let taffy_style = layout_style.to_taffy(rem_size, scale_factor);
        let measure = Box::new(measure) as Box<MeasureFn>;
        #[cfg(feature = "stacker")]
        let measure = StackSafe::new(measure);
        let layout_id = self.request_persistent_node(
            owner,
            &[],
            Some(NodeContext {
                measure,
                is_pure: fingerprint_seed.is_some(),
                last_measure_input: None,
            }),
            fingerprint_seed,
            Some(style_fingerprint),
            move || taffy_style,
        );
        self.node_fingerprints.insert(layout_id, fingerprint);
        self.record_layout_metadata(layout_id, layout_style, rem_size, scale_factor);
        layout_id
    }

    pub fn request_impure_measured_layout(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        fingerprint_seed: u64,
        measure: impl FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels>
        + 'static,
    ) -> LayoutId {
        let layout_style = LayoutStyle::from(&style);
        let mut style_hasher = FxHasher::default();
        self.hash_layout_style(&layout_style, rem_size, scale_factor, &mut style_hasher);
        let style_fingerprint = style_hasher.finish();
        let native = layout_style.to_taffy(rem_size, scale_factor);
        self.request_impure_measured_node(
            owner, layout_style, native, style_fingerprint, rem_size,
            scale_factor, fingerprint_seed, measure,
        )
    }

    pub(crate) fn request_default_measured_layout(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        rem_size: Pixels,
        scale_factor: f32,
        fingerprint_seed: u64,
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
        self.request_impure_measured_node(
            owner, cached.layout.clone(), cached.native.clone(), cached.fingerprint,
            rem_size, scale_factor, fingerprint_seed, measure,
        )
    }

    fn request_impure_measured_node(
        &mut self,
        owner: Option<(u64, GlobalElementId)>,
        layout_style: LayoutStyle,
        taffy_style: taffy::Style,
        style_fingerprint: u64,
        rem_size: Pixels,
        scale_factor: f32,
        fingerprint_seed: u64,
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
        let mut hasher = FxHasher::default();
        style_fingerprint.hash(&mut hasher);
        fingerprint_seed.hash(&mut hasher);
        let fingerprint = hasher.finish();
        let measure = Box::new(measure) as Box<MeasureFn>;
        #[cfg(feature = "stacker")]
        let measure = StackSafe::new(measure);
        let layout_id = self.request_persistent_node(
            owner,
            &[],
            Some(NodeContext {
                measure,
                is_pure: false,
                last_measure_input: None,
            }),
            Some(fingerprint_seed),
            Some(style_fingerprint),
            move || taffy_style,
        );
        self.node_fingerprints.insert(layout_id, Some(fingerprint));
        self.record_layout_metadata(layout_id, layout_style, rem_size, scale_factor);
        layout_id
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
        self.record_root_diagnostic(id, reusable);
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
        self.taffy
            .compute_layout_with_measure(
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

                            node_context.last_measure_input =
                                Some((known_dimensions, available_space));
                            let measured = (node_context.measure)(
                                known_dimensions,
                                available_space,
                                window,
                                cx,
                            );
                            size(
                                measured.width.0 * scale_factor,
                                measured.height.0 * scale_factor,
                            )
                            .into()
                        },
                    )
                },
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
        // fingerprint after the request. Only percentage-passthrough parents
        // and opt-in root diagnostics consume it. Most text and fixed-size
        // nodes therefore need no second style copy in a frame-local map.
        let percentage_axis = |length: Length| {
            matches!(length, Length::Definite(DefiniteLength::Fraction(fraction))
                if fraction.is_finite() && fraction >= 0.0)
        };
        if log::log_enabled!(log::Level::Trace)
            || percentage_axis(style.size.width)
            || percentage_axis(style.size.height)
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

    fn layout_fingerprint(&self, style_fingerprint: u64, children: &[LayoutId]) -> Option<u64> {
        let mut hasher = FxHasher::default();
        style_fingerprint.hash(&mut hasher);
        self.hash_layout_children(children, &mut hasher)?;
        Some(hasher.finish())
    }

    fn hash_layout_children(&self, children: &[LayoutId], hasher: &mut FxHasher) -> Option<()> {
        children.len().hash(hasher);
        for child in children {
            self.node_fingerprints
                .get(child)
                .copied()
                .flatten()?
                .hash(hasher);
        }
        Some(())
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
    fn subtree_claims_nest_and_rollback_atomically() {
        let mut engine = TaffyLayoutEngine::new();
        let root = list_id();
        let row = row_id(2);
        let outer = engine.record_claimed_keys();
        let first = engine.request_layout(Some(owner(&root)), Style::default(), crate::px(16.), 1., &[]);
        let inner = engine.record_claimed_keys();
        engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        let inner_claims = engine.finish_recording_claimed_keys(inner);
        let claims = engine.finish_recording_claimed_keys(outer);
        assert_eq!(inner_claims.len(), 1);
        assert_eq!(claims.len(), 2);
        assert_eq!(claims[1], inner_claims[0]);
        engine.clear();

        let recording = engine.record_claimed_keys();
        let mut invalid = claims.clone();
        invalid[1].id = first;
        assert!(!engine.try_keep_retained(&invalid));
        assert_eq!(engine.claimed_this_frame, 0);
        assert!(engine.try_keep_retained(&claims));
        assert!(!engine.try_keep_retained(&claims));
        assert_eq!(engine.claimed_this_frame, 2);
        engine.release_kept(&claims);
        assert_eq!(engine.claimed_this_frame, 0);
        assert!(engine.finish_recording_claimed_keys(recording).is_empty());
        assert_eq!(engine.request_layout(Some(owner(&root)), Style::default(), crate::px(16.), 1., &[]), first);
        assert!(!engine.try_keep_retained(&claims));
        assert_eq!(engine.claimed_this_frame, 1);
    }

    #[test]
    fn subtree_claim_refuses_robbed_layout_nodes() {
        let mut engine = TaffyLayoutEngine::new();
        let root = list_id();
        let recording = engine.record_claimed_keys();
        let id = engine.request_layout(Some(owner(&root)), Style::default(), crate::px(16.), 1., &[]);
        let claims = engine.finish_recording_claimed_keys(recording);
        engine.clear();
        engine.robbed_parents.insert(id);
        assert!(!engine.try_keep_retained(&claims));
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
    fn reparenting_invalidates_previous_parent_and_ancestor_proofs() {
        let mut engine = TaffyLayoutEngine::new();
        let child = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[]);
        let old = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[child]);
        let ancestor = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[old]);
        assert!(engine.node_fingerprints.get(&old).copied().flatten().is_some());
        assert!(engine.node_fingerprints.get(&ancestor).copied().flatten().is_some());
        let next = engine.request_layout(None, Style::default(), crate::px(16.), 1., &[child]);
        assert_eq!(engine.taffy.parent(child.into()), Some(next.into()));
        assert!(engine.robbed_parents.contains(&old));
        assert!(!engine.node_fingerprints.contains_key(&old));
        assert!(!engine.node_fingerprints.contains_key(&ancestor));
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
        let start = engine.record_claimed_keys();
        let id = engine.request_layout(Some(owner(&row)), Style::default(), crate::px(16.), 1., &[]);
        let claims = engine.finish_recording_claimed_keys(start);
        engine.abort_frame();
        assert_eq!(engine.taffy.total_node_count(), 0);
        assert!(!engine.try_keep_retained(&claims));
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
        engine.node_fingerprints.reserve(4096);
        engine.root_constraints.reserve(4096);
        let bounds_capacity = engine.absolute_layout_bounds.capacity();
        let fingerprints_capacity = engine.node_fingerprints.capacity();
        let roots_capacity = engine.root_constraints.capacity();

        engine.clear();

        assert!(engine.absolute_layout_bounds.capacity() < bounds_capacity);
        assert!(engine.node_fingerprints.capacity() < fingerprints_capacity);
        assert!(engine.root_constraints.capacity() < roots_capacity);
    }

    #[test]
    fn aggressive_trim_releases_layout_high_watermark_capacity() {
        let mut engine = TaffyLayoutEngine::new();
        engine.root_constraints.reserve(4096);
        engine.subtree_scratch.reserve(4096);
        engine.absolute_layout_bounds.reserve(4096);
        let roots_capacity = engine.root_constraints.capacity();
        let scratch_capacity = engine.subtree_scratch.capacity();
        let bounds_capacity = engine.absolute_layout_bounds.capacity();

        engine.trim_retained_capacity(GpuiMemoryTrimLevel::Aggressive);

        assert!(engine.root_constraints.capacity() < roots_capacity);
        assert!(engine.subtree_scratch.capacity() < scratch_capacity);
        assert!(engine.absolute_layout_bounds.capacity() < bounds_capacity);
    }
}
