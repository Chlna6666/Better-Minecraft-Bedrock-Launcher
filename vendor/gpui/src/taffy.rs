use crate::{
    AbsoluteLength, App, Bounds, DefiniteLength, Edges, LayoutFrameMetrics, Length, Pixels, Point,
    RenderFingerprint, Size, Style, Window, point, size,
};
use collections::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use stacksafe::{StackSafe, stacksafe};
use std::{
    collections::VecDeque,
    fmt::Debug,
    hash::Hash,
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind},
};
use taffy::{
    TaffyTree, TraversePartialTree as _,
    geometry::{Point as TaffyPoint, Rect as TaffyRect, Size as TaffySize},
    style::AvailableSpace as TaffyAvailableSpace,
    tree::NodeId,
};

type NodeMeasureFn = StackSafe<
    Box<
        dyn FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels>,
    >,
>;

struct NodeContext {
    measure: NodeMeasureFn,
}
pub struct TaffyLayoutEngine {
    taffy: TaffyTree<NodeContext>,
    absolute_layout_bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    computed_layouts: FxHashSet<LayoutId>,
    node_fingerprints: FxHashMap<LayoutId, Option<u64>>,
    measured_subtrees: FxHashSet<LayoutId>,
    previous_fingerprint_nodes: FxHashMap<LayoutNodeCacheKey, VecDeque<LayoutId>>,
    current_fingerprint_nodes: FxHashMap<LayoutNodeCacheKey, VecDeque<LayoutId>>,
    nodes_retained_this_frame: FxHashSet<LayoutId>,
    live_layout_nodes: FxHashSet<LayoutId>,
    previous_layout_roots: FxHashMap<LayoutRootCacheKey, Vec<RetainedLayoutNode>>,
    computed_root_keys: Vec<(LayoutRootCacheKey, LayoutId)>,
    retained_layout_bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    nodes_requested: usize,
    measured_nodes_requested: usize,
    persistent_node_reuses: usize,
    persistent_node_creations: usize,
    persistent_node_removals: usize,
    roots_computed: usize,
    bounds_cache_hits: usize,
    bounds_cache_misses: usize,
    layout_cache_hits: usize,
    layout_cache_misses: usize,
    layout_cache_reused_roots: usize,
    layout_cache_saved_roots: usize,
}

const EXPECT_MESSAGE: &str = "we should avoid taffy layout errors by construction if possible";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct LayoutRootCacheKey {
    root_fingerprint: u64,
    available_space: Size<AvailableSpaceKey>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
enum AvailableSpaceKey {
    Definite(u32),
    #[default]
    MinContent,
    MaxContent,
}

#[derive(Clone, Copy, Debug)]
struct RetainedLayoutNode {
    fingerprint: u64,
    bounds: Bounds<Pixels>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
struct LayoutNodeCacheKey {
    fingerprint: u64,
    measured: bool,
}

impl TaffyLayoutEngine {
    pub fn new() -> Self {
        let mut taffy = TaffyTree::new();
        taffy.enable_rounding();
        TaffyLayoutEngine {
            taffy,
            absolute_layout_bounds: FxHashMap::default(),
            computed_layouts: FxHashSet::default(),
            node_fingerprints: FxHashMap::default(),
            measured_subtrees: FxHashSet::default(),
            previous_fingerprint_nodes: FxHashMap::default(),
            current_fingerprint_nodes: FxHashMap::default(),
            nodes_retained_this_frame: FxHashSet::default(),
            live_layout_nodes: FxHashSet::default(),
            previous_layout_roots: FxHashMap::default(),
            computed_root_keys: Vec::new(),
            retained_layout_bounds: FxHashMap::default(),
            nodes_requested: 0,
            measured_nodes_requested: 0,
            persistent_node_reuses: 0,
            persistent_node_creations: 0,
            persistent_node_removals: 0,
            roots_computed: 0,
            bounds_cache_hits: 0,
            bounds_cache_misses: 0,
            layout_cache_hits: 0,
            layout_cache_misses: 0,
            layout_cache_reused_roots: 0,
            layout_cache_saved_roots: 0,
        }
    }

    pub fn finish_frame(&mut self) -> LayoutFrameMetrics {
        self.save_retained_layout_roots();
        self.reclaim_unused_persistent_nodes();
        self.detach_current_frame_nodes();
        let metrics = self.frame_metrics();
        self.previous_fingerprint_nodes = self.reusable_current_fingerprint_nodes();
        self.current_fingerprint_nodes.clear();
        self.absolute_layout_bounds.clear();
        self.computed_layouts.clear();
        self.node_fingerprints.clear();
        self.measured_subtrees.clear();
        self.nodes_retained_this_frame.clear();
        self.computed_root_keys.clear();
        self.retained_layout_bounds.clear();
        self.nodes_requested = 0;
        self.measured_nodes_requested = 0;
        self.persistent_node_reuses = 0;
        self.persistent_node_creations = 0;
        self.persistent_node_removals = 0;
        self.roots_computed = 0;
        self.bounds_cache_hits = 0;
        self.bounds_cache_misses = 0;
        self.layout_cache_hits = 0;
        self.layout_cache_misses = 0;
        self.layout_cache_reused_roots = 0;
        self.layout_cache_saved_roots = 0;
        metrics
    }

    pub fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        let fingerprint = self.layout_fingerprint(&style, rem_size, scale_factor, children);
        let taffy_style = style.to_taffy(rem_size, scale_factor);

        let layout_id = if let Some(layout_id) = self.take_retained_node(fingerprint, false) {
            self.update_retained_node(layout_id, taffy_style, children);
            self.persistent_node_reuses = self.persistent_node_reuses.saturating_add(1);
            layout_id
        } else if children.is_empty() {
            self.persistent_node_creations = self.persistent_node_creations.saturating_add(1);
            self.taffy
                .new_leaf(taffy_style)
                .expect(EXPECT_MESSAGE)
                .into()
        } else {
            self.persistent_node_creations = self.persistent_node_creations.saturating_add(1);
            self.taffy
                // This is safe because LayoutId is repr(transparent) to taffy::tree::NodeId.
                .new_with_children(taffy_style, LayoutId::to_taffy_slice(children))
                .expect(EXPECT_MESSAGE)
                .into()
        };
        self.track_current_node(layout_id, fingerprint, false);
        self.node_fingerprints.insert(layout_id, fingerprint);
        self.live_layout_nodes.insert(layout_id);
        if children
            .iter()
            .any(|child| self.measured_subtrees.contains(child))
        {
            self.measured_subtrees.insert(layout_id);
        }
        layout_id
    }

    pub fn request_measured_layout(
        &mut self,
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
        self.request_measured_layout_with_fingerprint(style, rem_size, scale_factor, None, measure)
    }

    pub fn request_measured_layout_with_fingerprint(
        &mut self,
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
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        self.measured_nodes_requested = self.measured_nodes_requested.saturating_add(1);
        let fingerprint =
            self.measured_layout_fingerprint(&style, rem_size, scale_factor, fingerprint_seed);
        let taffy_style = style.to_taffy(rem_size, scale_factor);

        let node_context = NodeContext {
            measure: StackSafe::new(Box::new(measure)),
        };
        self.persistent_node_creations = self.persistent_node_creations.saturating_add(1);
        let layout_id = self
            .taffy
            .new_leaf_with_context(taffy_style, node_context)
            .expect(EXPECT_MESSAGE)
            .into();
        self.track_current_node(layout_id, fingerprint, true);
        self.node_fingerprints.insert(layout_id, fingerprint);
        self.measured_subtrees.insert(layout_id);
        self.live_layout_nodes.insert(layout_id);
        layout_id
    }

    pub fn request_pure_measured_layout_with_fingerprint(
        &mut self,
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
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        self.measured_nodes_requested = self.measured_nodes_requested.saturating_add(1);
        let fingerprint =
            self.measured_layout_fingerprint(&style, rem_size, scale_factor, fingerprint_seed);
        let taffy_style = style.to_taffy(rem_size, scale_factor);
        let layout_id = if let Some(layout_id) = self.take_retained_node(fingerprint, true) {
            self.persistent_node_reuses = self.persistent_node_reuses.saturating_add(1);
            layout_id
        } else {
            self.persistent_node_creations = self.persistent_node_creations.saturating_add(1);
            let node_context = NodeContext {
                measure: StackSafe::new(Box::new(measure)),
            };
            self.taffy
                .new_leaf_with_context(taffy_style, node_context)
                .expect(EXPECT_MESSAGE)
                .into()
        };
        self.track_current_node(layout_id, fingerprint, true);
        self.node_fingerprints.insert(layout_id, fingerprint);
        self.measured_subtrees.insert(layout_id);
        self.live_layout_nodes.insert(layout_id);
        layout_id
    }

    fn measured_layout_fingerprint(
        &self,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        fingerprint_seed: Option<u64>,
    ) -> Option<u64> {
        fingerprint_seed.map(|seed| {
            let mut hasher = RenderFingerprint::new();
            self.hash_layout_style(style, rem_size, scale_factor, &mut hasher);
            seed.hash(&mut hasher);
            hasher.value()
        })
    }

    // Used to understand performance
    #[allow(dead_code)]
    fn count_all_children(&self, parent: LayoutId) -> anyhow::Result<u32> {
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
    fn max_depth(&self, depth: u32, parent: LayoutId) -> anyhow::Result<u32> {
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
    fn get_edges(&self, parent: LayoutId) -> anyhow::Result<Vec<(LayoutId, LayoutId)>> {
        let mut edges = Vec::new();

        for child in self.taffy.children(parent.0)? {
            edges.push((parent, LayoutId(child)));

            edges.extend(self.get_edges(LayoutId(child))?);
        }

        Ok(edges)
    }

    #[stacksafe]
    pub fn compute_layout(
        &mut self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.roots_computed = self.roots_computed.saturating_add(1);
        let root_key = self.root_cache_key(id, available_space);
        if let Some(root_key) = root_key {
            if self.try_retain_layout(id, &root_key) {
                self.computed_layouts.insert(id);
                self.computed_root_keys.push((root_key, id));
                self.layout_cache_hits = self.layout_cache_hits.saturating_add(1);
                self.layout_cache_reused_roots = self.layout_cache_reused_roots.saturating_add(1);
                return;
            }
            self.layout_cache_misses = self.layout_cache_misses.saturating_add(1);
        } else {
            self.layout_cache_misses = self.layout_cache_misses.saturating_add(1);
        }
        // Leaving this here until we have a better instrumentation approach.
        // println!("Laying out {} children", self.count_all_children(id)?);
        // println!("Max layout depth: {}", self.max_depth(0, id)?);

        // Output the edges (branches) of the tree in Mermaid format for visualization.
        // println!("Edges:");
        // for (a, b) in self.get_edges(id)? {
        //     println!("N{} --> N{}", u64::from(a), u64::from(b));
        // }
        //

        if !self.computed_layouts.insert(id) {
            let mut stack = SmallVec::<[LayoutId; 64]>::new();
            stack.push(id);
            while let Some(id) = stack.pop() {
                self.absolute_layout_bounds.remove(&id);
                stack.extend(
                    self.taffy
                        .children(id.into())
                        .expect(EXPECT_MESSAGE)
                        .into_iter()
                        .map(Into::into),
                );
            }
        }

        let scale_factor = window.scale_factor();

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

        self.taffy
            .compute_layout_with_measure(
                id.into(),
                available_space.into(),
                |known_dimensions, available_space, _id, node_context, _style| {
                    let Some(node_context) = node_context else {
                        return taffy::geometry::Size::default();
                    };

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

                    let a: Size<Pixels> =
                        (node_context.measure)(known_dimensions, available_space, window, cx);
                    size(a.width.0 * scale_factor, a.height.0 * scale_factor).into()
                },
            )
            .expect(EXPECT_MESSAGE);
        if let Some(root_key) = root_key {
            self.computed_root_keys.push((root_key, id));
        }
    }

    pub fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        if let Some(layout) = self.retained_layout_bounds.get(&id).cloned() {
            self.absolute_layout_bounds.insert(id, layout);
            self.bounds_cache_hits = self.bounds_cache_hits.saturating_add(1);
            return layout;
        }
        if let Some(layout) = self.absolute_layout_bounds.get(&id).cloned() {
            self.bounds_cache_hits = self.bounds_cache_hits.saturating_add(1);
            return layout;
        }
        self.bounds_cache_misses = self.bounds_cache_misses.saturating_add(1);

        let layout = self.taffy.layout(id.into()).expect(EXPECT_MESSAGE);
        let mut bounds = Bounds {
            origin: point(
                Pixels(layout.location.x / scale_factor),
                Pixels(layout.location.y / scale_factor),
            ),
            size: size(
                Pixels(layout.size.width / scale_factor),
                Pixels(layout.size.height / scale_factor),
            ),
        };

        if let Some(parent_id) = self.taffy.parent(id.0) {
            let parent_bounds = self.layout_bounds(parent_id.into(), scale_factor);
            bounds.origin += parent_bounds.origin;
        }
        self.absolute_layout_bounds.insert(id, bounds);

        bounds
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
            persistent_node_reuses: self.persistent_node_reuses,
            persistent_node_creations: self.persistent_node_creations,
            persistent_node_removals: self.persistent_node_removals,
        }
    }

    pub fn layout_cache_metrics(&self) -> (usize, usize) {
        (self.layout_cache_hits, self.layout_cache_misses)
    }

    #[cfg(test)]
    fn retained_node_count(&self) -> usize {
        self.taffy.total_node_count()
    }

    fn take_retained_node(&mut self, fingerprint: Option<u64>, measured: bool) -> Option<LayoutId> {
        let key = LayoutNodeCacheKey {
            fingerprint: fingerprint?,
            measured,
        };
        loop {
            let layout_id = {
                let retained_nodes = self.previous_fingerprint_nodes.get(&key)?;
                retained_nodes.front().copied()?
            };
            if self.nodes_retained_this_frame.contains(&layout_id) {
                if let Some(retained_nodes) = self.previous_fingerprint_nodes.get_mut(&key) {
                    let _ = retained_nodes.pop_front();
                }
                continue;
            }
            if !self.retained_node_is_leaf(layout_id) {
                if let Some(retained_nodes) = self.previous_fingerprint_nodes.get_mut(&key) {
                    let _ = retained_nodes.pop_front();
                }
                self.remove_persistent_node(layout_id);
                continue;
            }
            if self.retained_node_has_parent(layout_id) {
                if let Some(retained_nodes) = self.previous_fingerprint_nodes.get_mut(&key) {
                    let _ = retained_nodes.pop_front();
                }
                self.remove_persistent_node(layout_id);
                continue;
            }
            let layout_id = self.previous_fingerprint_nodes.get_mut(&key)?.pop_front()?;
            if self.nodes_retained_this_frame.insert(layout_id) {
                return Some(layout_id);
            }
        }
    }

    fn track_current_node(
        &mut self,
        layout_id: LayoutId,
        fingerprint: Option<u64>,
        measured: bool,
    ) {
        if let Some(fingerprint) = fingerprint {
            let key = LayoutNodeCacheKey {
                fingerprint,
                measured,
            };
            self.current_fingerprint_nodes
                .entry(key)
                .or_default()
                .push_back(layout_id);
        }
    }

    fn update_retained_node(
        &mut self,
        layout_id: LayoutId,
        style: taffy::style::Style,
        children: &[LayoutId],
    ) {
        if self
            .taffy
            .style(layout_id.into())
            .is_ok_and(|current_style| current_style != &style)
        {
            self.taffy
                .set_style(layout_id.into(), style)
                .expect(EXPECT_MESSAGE);
        }

        let children_changed = self
            .taffy
            .children(layout_id.into())
            .map(|current_children| current_children != LayoutId::to_taffy_slice(children))
            .unwrap_or(true);
        if children_changed {
            self.taffy
                .set_children(layout_id.into(), LayoutId::to_taffy_slice(children))
                .expect(EXPECT_MESSAGE);
        }
    }

    fn reclaim_unused_persistent_nodes(&mut self) {
        let retained_nodes = self
            .current_fingerprint_nodes
            .values()
            .flatten()
            .copied()
            .collect::<FxHashSet<_>>();
        let mut unused_nodes = self
            .previous_fingerprint_nodes
            .values()
            .flatten()
            .copied()
            .filter(|layout_id| !retained_nodes.contains(layout_id))
            .collect::<Vec<_>>();
        unused_nodes
            .sort_unstable_by_key(|layout_id| std::cmp::Reverse(self.node_depth(*layout_id)));
        for layout_id in unused_nodes {
            self.remove_persistent_node(layout_id);
        }
    }

    fn detach_current_frame_nodes(&mut self) {
        let mut current_nodes = self
            .current_fingerprint_nodes
            .values()
            .flatten()
            .copied()
            .collect::<Vec<_>>();
        current_nodes.sort_unstable_by_key(|node| self.node_depth(*node));
        for layout_id in current_nodes {
            if !self.live_layout_nodes.contains(&layout_id) {
                continue;
            }
            let has_children = catch_unwind(AssertUnwindSafe(|| {
                self.taffy
                    .children(layout_id.into())
                    .expect(EXPECT_MESSAGE)
                    .is_empty()
            }))
            .ok()
            .is_some_and(|is_empty| !is_empty);
            if !has_children {
                continue;
            }
            let _ = catch_unwind(AssertUnwindSafe(|| {
                self.taffy
                    .set_children(layout_id.into(), &[])
                    .expect(EXPECT_MESSAGE);
            }));
        }
    }

    fn reusable_current_fingerprint_nodes(
        &self,
    ) -> FxHashMap<LayoutNodeCacheKey, VecDeque<LayoutId>> {
        let mut reusable_nodes = FxHashMap::default();
        for (key, nodes) in &self.current_fingerprint_nodes {
            for layout_id in nodes {
                if self.live_layout_nodes.contains(layout_id)
                    && self.retained_node_is_leaf(*layout_id)
                    && !self.retained_node_has_parent(*layout_id)
                {
                    reusable_nodes
                        .entry(*key)
                        .or_insert_with(VecDeque::new)
                        .push_back(*layout_id);
                }
            }
        }
        reusable_nodes
    }

    fn remove_persistent_node(&mut self, layout_id: LayoutId) {
        if !self.live_layout_nodes.remove(&layout_id) {
            return;
        }
        let removed = catch_unwind(AssertUnwindSafe(|| self.taffy.remove(layout_id.into()))).ok();
        self.node_fingerprints.remove(&layout_id);
        self.measured_subtrees.remove(&layout_id);
        if removed.is_some_and(|result| result.is_ok()) {
            self.persistent_node_removals = self.persistent_node_removals.saturating_add(1);
        }
    }

    fn retained_node_is_leaf(&self, layout_id: LayoutId) -> bool {
        let Ok(children) = catch_unwind(AssertUnwindSafe(|| self.taffy.children(layout_id.into())))
        else {
            return false;
        };
        matches!(children, Ok(children) if children.is_empty())
    }

    fn retained_node_has_parent(&self, layout_id: LayoutId) -> bool {
        catch_unwind(AssertUnwindSafe(|| self.taffy.parent(layout_id.0)))
            .ok()
            .flatten()
            .is_some()
    }

    fn node_depth(&self, mut layout_id: LayoutId) -> usize {
        let mut depth = 0;
        while let Some(parent_id) =
            catch_unwind(AssertUnwindSafe(|| self.taffy.parent(layout_id.0)))
                .ok()
                .flatten()
        {
            depth += 1;
            layout_id = parent_id.into();
        }
        depth
    }

    fn layout_fingerprint(
        &self,
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> Option<u64> {
        let mut hasher = RenderFingerprint::new();
        self.hash_layout_style(style, rem_size, scale_factor, &mut hasher);
        self.hash_layout_children(children, &mut hasher)?;
        Some(hasher.value())
    }

    fn hash_layout_children(
        &self,
        children: &[LayoutId],
        hasher: &mut RenderFingerprint,
    ) -> Option<()> {
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
        style: &Style,
        rem_size: Pixels,
        scale_factor: f32,
        hasher: &mut RenderFingerprint,
    ) {
        hash_display(style.display, hasher);
        hash_visibility(style.visibility, hasher);
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

    fn root_cache_key(
        &self,
        id: LayoutId,
        available_space: Size<AvailableSpace>,
    ) -> Option<LayoutRootCacheKey> {
        if self.measured_subtrees.contains(&id) {
            return None;
        }
        Some(LayoutRootCacheKey {
            root_fingerprint: self.node_fingerprints.get(&id).copied().flatten()?,
            available_space: size(
                AvailableSpaceKey::from(available_space.width),
                AvailableSpaceKey::from(available_space.height),
            ),
        })
    }

    fn try_retain_layout(&mut self, id: LayoutId, root_key: &LayoutRootCacheKey) -> bool {
        let Some(cached_nodes) = self.previous_layout_roots.get(root_key).cloned() else {
            return false;
        };
        let current_nodes = self.subtree_nodes(id);
        if current_nodes.len() != cached_nodes.len() {
            return false;
        }
        for (current_id, cached_node) in current_nodes.iter().zip(cached_nodes.iter()) {
            let current_fingerprint = self.node_fingerprints.get(current_id).copied().flatten();
            if current_fingerprint != Some(cached_node.fingerprint) {
                return false;
            }
        }
        let reused_node_count = current_nodes.len();
        for (current_id, cached_node) in current_nodes.into_iter().zip(cached_nodes) {
            self.retained_layout_bounds
                .insert(current_id, cached_node.bounds);
        }
        self.layout_cache_saved_roots = self
            .layout_cache_saved_roots
            .saturating_add(reused_node_count.saturating_sub(1));
        true
    }

    fn save_retained_layout_roots(&mut self) {
        self.previous_layout_roots.clear();
        let computed_root_keys = std::mem::take(&mut self.computed_root_keys);
        for (root_key, root_id) in computed_root_keys {
            if let Some(nodes) = self.retained_layout_nodes(root_id) {
                if let Some(previous_nodes) = self.previous_layout_roots.get_mut(&root_key) {
                    *previous_nodes = nodes;
                } else {
                    self.previous_layout_roots.insert(root_key, nodes);
                }
            }
        }
    }

    fn retained_layout_nodes(&self, root_id: LayoutId) -> Option<Vec<RetainedLayoutNode>> {
        self.subtree_nodes(root_id)
            .into_iter()
            .map(|id| {
                Some(RetainedLayoutNode {
                    fingerprint: self.node_fingerprints.get(&id).copied().flatten()?,
                    bounds: self.absolute_layout_bounds.get(&id).copied()?,
                })
            })
            .collect()
    }

    fn subtree_nodes(&self, root_id: LayoutId) -> Vec<LayoutId> {
        let mut nodes = Vec::new();
        let mut stack = SmallVec::<[LayoutId; 64]>::new();
        stack.push(root_id);
        while let Some(id) = stack.pop() {
            nodes.push(id);
            let children = self.taffy.children(id.into()).expect(EXPECT_MESSAGE);
            stack.extend(children.into_iter().rev().map(Into::into));
        }
        nodes
    }
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
pub struct LayoutId(NodeId);

impl LayoutId {
    fn to_taffy_slice(node_ids: &[Self]) -> &[taffy::NodeId] {
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

trait ToTaffy<Output> {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> Output;
}

impl ToTaffy<taffy::style::Style> for Style {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Style {
        use taffy::style_helpers::{fr, length, minmax, repeat};

        fn to_grid_line(
            placement: &Range<crate::GridPlacement>,
        ) -> taffy::Line<taffy::GridPlacement> {
            taffy::Line {
                start: placement.start.into(),
                end: placement.end.into(),
            }
        }

        fn to_grid_repeat<T: taffy::style::CheapCloneStr>(
            unit: &Option<u16>,
        ) -> Vec<taffy::GridTemplateComponent<T>> {
            // grid-template-columns: repeat(<number>, minmax(0, 1fr));
            unit.map(|count| vec![repeat(count, vec![minmax(length(0.0), fr(1.0))])])
                .unwrap_or_default()
        }

        taffy::style::Style {
            display: self.display.into(),
            overflow: self.overflow.into(),
            scrollbar_width: self.scrollbar_width.to_taffy(rem_size, scale_factor),
            position: self.position.into(),
            inset: self.inset.to_taffy(rem_size, scale_factor),
            size: self.size.to_taffy(rem_size, scale_factor),
            min_size: self.min_size.to_taffy(rem_size, scale_factor),
            max_size: self.max_size.to_taffy(rem_size, scale_factor),
            aspect_ratio: self.aspect_ratio,
            margin: self.margin.to_taffy(rem_size, scale_factor),
            padding: self.padding.to_taffy(rem_size, scale_factor),
            border: self.border_widths.to_taffy(rem_size, scale_factor),
            align_items: self.align_items.map(|x| x.into()),
            align_self: self.align_self.map(|x| x.into()),
            align_content: self.align_content.map(|x| x.into()),
            justify_content: self.justify_content.map(|x| x.into()),
            gap: self.gap.to_taffy(rem_size, scale_factor),
            flex_direction: self.flex_direction.into(),
            flex_wrap: self.flex_wrap.into(),
            flex_basis: self.flex_basis.to_taffy(rem_size, scale_factor),
            flex_grow: self.flex_grow,
            flex_shrink: self.flex_shrink,
            grid_template_rows: to_grid_repeat(&self.grid_rows),
            grid_template_columns: to_grid_repeat(&self.grid_cols),
            grid_row: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.row))
                .unwrap_or_default(),
            grid_column: self
                .grid_location
                .as_ref()
                .map(|location| to_grid_line(&location.column))
                .unwrap_or_default(),
            ..Default::default()
        }
    }
}

impl ToTaffy<f32> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> f32 {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                pixels * scale_factor
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                pixels * scale_factor
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for Length {
    fn to_taffy(
        &self,
        rem_size: Pixels,
        scale_factor: f32,
    ) -> taffy::prelude::LengthPercentageAuto {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::LengthPercentageAuto::auto(),
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for Length {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::prelude::Dimension {
        match self {
            Length::Definite(length) => length.to_taffy(rem_size, scale_factor),
            Length::Auto => taffy::prelude::Dimension::auto(),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentage::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentage::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentageAuto> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentageAuto {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    let pixels: f32 = (*rems * rem_size).into();
                    taffy::style::LengthPercentageAuto::length(pixels * scale_factor)
                }
            },
            DefiniteLength::Fraction(fraction) => {
                taffy::style::LengthPercentageAuto::percent(*fraction)
            }
        }
    }
}

impl ToTaffy<taffy::style::Dimension> for DefiniteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::Dimension {
        match self {
            DefiniteLength::Absolute(length) => match length {
                AbsoluteLength::Pixels(pixels) => {
                    let pixels: f32 = pixels.into();
                    taffy::style::Dimension::length(pixels * scale_factor)
                }
                AbsoluteLength::Rems(rems) => {
                    taffy::style::Dimension::length((*rems * rem_size * scale_factor).into())
                }
            },
            DefiniteLength::Fraction(fraction) => taffy::style::Dimension::percent(*fraction),
        }
    }
}

impl ToTaffy<taffy::style::LengthPercentage> for AbsoluteLength {
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> taffy::style::LengthPercentage {
        match self {
            AbsoluteLength::Pixels(pixels) => {
                let pixels: f32 = pixels.into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
            AbsoluteLength::Rems(rems) => {
                let pixels: f32 = (*rems * rem_size).into();
                taffy::style::LengthPercentage::length(pixels * scale_factor)
            }
        }
    }
}

impl<T, T2> From<TaffyPoint<T>> for Point<T2>
where
    T: Into<T2>,
    T2: Clone + Debug + Default + PartialEq,
{
    fn from(point: TaffyPoint<T>) -> Point<T2> {
        Point {
            x: point.x.into(),
            y: point.y.into(),
        }
    }
}

impl<T, T2> From<Point<T>> for TaffyPoint<T2>
where
    T: Into<T2> + Clone + Debug + Default + PartialEq,
{
    fn from(val: Point<T>) -> Self {
        TaffyPoint {
            x: val.x.into(),
            y: val.y.into(),
        }
    }
}

impl<T, U> ToTaffy<TaffySize<U>> for Size<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffySize<U> {
        TaffySize {
            width: self.width.to_taffy(rem_size, scale_factor),
            height: self.height.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> ToTaffy<TaffyRect<U>> for Edges<T>
where
    T: ToTaffy<U> + Clone + Debug + Default + PartialEq,
{
    fn to_taffy(&self, rem_size: Pixels, scale_factor: f32) -> TaffyRect<U> {
        TaffyRect {
            top: self.top.to_taffy(rem_size, scale_factor),
            right: self.right.to_taffy(rem_size, scale_factor),
            bottom: self.bottom.to_taffy(rem_size, scale_factor),
            left: self.left.to_taffy(rem_size, scale_factor),
        }
    }
}

impl<T, U> From<TaffySize<T>> for Size<U>
where
    T: Into<U>,
    U: Clone + Debug + Default + PartialEq,
{
    fn from(taffy_size: TaffySize<T>) -> Self {
        Size {
            width: taffy_size.width.into(),
            height: taffy_size.height.into(),
        }
    }
}

impl<T, U> From<Size<T>> for TaffySize<U>
where
    T: Into<U> + Clone + Debug + Default + PartialEq,
{
    fn from(size: Size<T>) -> Self {
        TaffySize {
            width: size.width.into(),
            height: size.height.into(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppContext as _, TestAppContext, WindowOptions, px};

    #[gpui::test]
    fn retained_layout_cache_reuses_clean_style_tree(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let (first_hits, first_misses) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(child, 1.0);
                engine.layout_bounds(root, 1.0);
                let metrics = engine.layout_cache_metrics();
                engine.finish_frame();
                metrics
            })
            .unwrap();

        assert_eq!(first_hits, 0);
        assert_eq!(first_misses, 1);

        let (second_hits, second_misses) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(child, 1.0);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_cache_metrics()
            })
            .unwrap();

        assert_eq!(second_hits, 1);
        assert_eq!(second_misses, 0);
    }

    #[gpui::test]
    fn persistent_layout_tree_reuses_nodes_across_frames(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let (first_child, first_root, second_child, second_root, second_metrics) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let first_child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let first_root =
                    engine.request_layout(Style::default(), px(16.), 1.0, &[first_child]);
                engine.compute_layout(first_root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(first_root, 1.0);
                engine.finish_frame();

                let second_child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let second_root =
                    engine.request_layout(Style::default(), px(16.), 1.0, &[second_child]);
                engine.compute_layout(second_root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(second_root, 1.0);
                let second_metrics = engine.finish_frame();
                (
                    first_child,
                    first_root,
                    second_child,
                    second_root,
                    second_metrics,
                )
            })
            .unwrap();

        assert_eq!(first_child, second_child);
        assert_eq!(first_root, second_root);
        assert_eq!(second_metrics.persistent_node_reuses, 2);
        assert_eq!(second_metrics.persistent_node_creations, 0);
    }

    #[gpui::test]
    fn persistent_layout_tree_does_not_accumulate_nodes_across_frames(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let retained_node_count = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                for _ in 0..8 {
                    let first_child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                    let second_child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                    let root = engine.request_layout(
                        Style::default(),
                        px(16.),
                        1.0,
                        &[first_child, second_child],
                    );
                    engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                    engine.layout_bounds(root, 1.0);
                    engine.finish_frame();
                }
                engine.retained_node_count()
            })
            .unwrap();

        assert_eq!(retained_node_count, 3);
    }

    #[gpui::test]
    fn persistent_layout_tree_reuses_stable_subtrees_when_sibling_changes(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let (stable_first, stable_second, changed_first, changed_second, metrics) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let stable_first = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let changed_first = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let root = engine.request_layout(
                    Style::default(),
                    px(16.),
                    1.0,
                    &[stable_first, changed_first],
                );
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let stable_second = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let mut changed_style = Style::default();
                changed_style.size.width = Length::Definite(DefiniteLength::from(px(42.)));
                let changed_second = engine.request_layout(changed_style, px(16.), 1.0, &[]);
                let root = engine.request_layout(
                    Style::default(),
                    px(16.),
                    1.0,
                    &[stable_second, changed_second],
                );
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(root, 1.0);
                let metrics = engine.finish_frame();
                (
                    stable_first,
                    stable_second,
                    changed_first,
                    changed_second,
                    metrics,
                )
            })
            .unwrap();

        assert_eq!(stable_first, stable_second);
        assert_ne!(changed_first, changed_second);
        assert!(metrics.persistent_node_reuses >= 1);
        assert!(metrics.persistent_node_creations >= 1);
        assert!(metrics.persistent_node_removals >= 1);
    }

    #[gpui::test]
    fn retained_layout_cache_misses_when_available_space_changes(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let (hits, misses) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                engine.compute_layout(
                    root,
                    size(
                        AvailableSpace::Definite(px(100.)),
                        AvailableSpace::MinContent,
                    ),
                    window,
                    cx,
                );
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                engine.compute_layout(
                    root,
                    size(
                        AvailableSpace::Definite(px(200.)),
                        AvailableSpace::MinContent,
                    ),
                    window,
                    cx,
                );
                engine.layout_cache_metrics()
            })
            .unwrap();

        assert_eq!(hits, 0);
        assert_eq!(misses, 1);
    }

    #[gpui::test]
    fn retained_layout_cache_ignores_paint_only_style_changes(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let (hits, misses) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let mut style = Style::default();
                style.background = Some(crate::red().into());
                let root = engine.request_layout(style.clone(), px(16.), 1.0, &[]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                style.background = Some(crate::blue().into());
                let root = engine.request_layout(style, px(16.), 1.0, &[]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_cache_metrics()
            })
            .unwrap();

        assert_eq!(hits, 1);
        assert_eq!(misses, 0);
    }

    #[gpui::test]
    fn retained_layout_cache_misses_when_measured_fingerprint_changes(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let (hits, misses, second_bounds) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let child = engine.request_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(1),
                    |_, _, _, _| size(px(10.), px(10.)),
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(child, 1.0);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let child = engine.request_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(2),
                    |_, _, _, _| size(px(10.), px(40.)),
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                let second_bounds = engine.layout_bounds(child, 1.0);
                let (hits, misses) = engine.layout_cache_metrics();
                (hits, misses, second_bounds)
            })
            .unwrap();

        assert_eq!(hits, 0);
        assert_eq!(misses, 1);
        assert!(second_bounds.size.height > px(10.));
    }

    #[gpui::test]
    fn retained_layout_cache_skips_measured_subtrees(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let (hits, misses) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let child = engine.request_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(11),
                    |_, _, _, _| size(px(20.), px(12.)),
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(child, 1.0);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let child = engine.request_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(11),
                    |_, _, _, _| size(px(20.), px(24.)),
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_cache_metrics()
            })
            .unwrap();

        assert_eq!((hits, misses), (0, 1));
    }

    #[gpui::test]
    fn retained_measured_node_with_stable_fingerprint_remeasures_for_effects(
        cx: &mut TestAppContext,
    ) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let measure_count = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let count_after_second_frame = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let child = engine.request_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(11),
                    {
                        let measure_count = measure_count.clone();
                        move |_, _, _, _| {
                            measure_count.set(measure_count.get().saturating_add(1));
                            size(px(20.), px(12.))
                        }
                    },
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(child, 1.0);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let child = engine.request_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(11),
                    {
                        let measure_count = measure_count.clone();
                        move |_, _, _, _| {
                            measure_count.set(measure_count.get().saturating_add(1));
                            size(px(20.), px(12.))
                        }
                    },
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                measure_count.get()
            })
            .unwrap();

        assert_eq!(count_after_second_frame, 2);
    }

    #[gpui::test]
    fn retained_pure_measured_node_with_stable_fingerprint_does_not_remeasure(
        cx: &mut TestAppContext,
    ) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let measure_count = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let count_after_second_frame = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let child = engine.request_pure_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(11),
                    {
                        let measure_count = measure_count.clone();
                        move |_, _, _, _| {
                            measure_count.set(measure_count.get().saturating_add(1));
                            size(px(20.), px(12.))
                        }
                    },
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(child, 1.0);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let child = engine.request_pure_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(11),
                    {
                        let measure_count = measure_count.clone();
                        move |_, _, _, _| {
                            measure_count.set(measure_count.get().saturating_add(1));
                            size(px(20.), px(12.))
                        }
                    },
                );
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                measure_count.get()
            })
            .unwrap();

        assert_eq!(count_after_second_frame, 1);
    }

    #[gpui::test]
    fn stable_pure_measured_sibling_is_not_remeasured_when_other_sibling_changes(
        cx: &mut TestAppContext,
    ) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let stable_measure_count = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let changed_measure_count = std::rc::Rc::new(std::cell::Cell::new(0usize));
        let (stable_first, stable_second, changed_first, changed_second, counts, metrics) = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let stable_first = engine.request_pure_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(101),
                    {
                        let stable_measure_count = stable_measure_count.clone();
                        move |_, _, _, _| {
                            stable_measure_count.set(stable_measure_count.get().saturating_add(1));
                            size(px(20.), px(12.))
                        }
                    },
                );
                let changed_first = engine.request_pure_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(201),
                    {
                        let changed_measure_count = changed_measure_count.clone();
                        move |_, _, _, _| {
                            changed_measure_count
                                .set(changed_measure_count.get().saturating_add(1));
                            size(px(10.), px(10.))
                        }
                    },
                );
                let root = engine.request_layout(
                    Style::default(),
                    px(16.),
                    1.0,
                    &[stable_first, changed_first],
                );
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();
                let changed_count_after_first_frame = changed_measure_count.get();

                let stable_second = engine.request_pure_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(101),
                    {
                        let stable_measure_count = stable_measure_count.clone();
                        move |_, _, _, _| {
                            stable_measure_count.set(stable_measure_count.get().saturating_add(1));
                            size(px(20.), px(12.))
                        }
                    },
                );
                let changed_second = engine.request_pure_measured_layout_with_fingerprint(
                    Style::default(),
                    px(16.),
                    1.0,
                    Some(202),
                    {
                        let changed_measure_count = changed_measure_count.clone();
                        move |_, _, _, _| {
                            changed_measure_count
                                .set(changed_measure_count.get().saturating_add(1));
                            size(px(10.), px(30.))
                        }
                    },
                );
                let root = engine.request_layout(
                    Style::default(),
                    px(16.),
                    1.0,
                    &[stable_second, changed_second],
                );
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(root, 1.0);
                let metrics = engine.finish_frame();
                (
                    stable_first,
                    stable_second,
                    changed_first,
                    changed_second,
                    (
                        stable_measure_count.get(),
                        changed_count_after_first_frame,
                        changed_measure_count.get(),
                    ),
                    metrics,
                )
            })
            .unwrap();

        assert_eq!(stable_first, stable_second);
        assert_ne!(changed_first, changed_second);
        assert_eq!(counts.0, 1);
        assert!(counts.2 > counts.1);
        assert!(metrics.persistent_node_reuses >= 1);
    }

    #[gpui::test]
    fn reclaiming_unused_parent_and_child_nodes_does_not_double_remove(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(WindowOptions::default(), |_, cx| cx.new(|_| crate::Empty))
                .unwrap()
        });

        let removed = window
            .update(cx, |_, window, cx| {
                let mut engine = TaffyLayoutEngine::new();
                let child = engine.request_layout(Style::default(), px(16.), 1.0, &[]);
                let root = engine.request_layout(Style::default(), px(16.), 1.0, &[child]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.layout_bounds(child, 1.0);
                engine.layout_bounds(root, 1.0);
                engine.finish_frame();

                let mut replacement_style = Style::default();
                replacement_style.margin.left = Length::Definite(px(1.).into());
                let root = engine.request_layout(replacement_style, px(16.), 1.0, &[]);
                engine.compute_layout(root, AvailableSpace::min_size(), window, cx);
                engine.finish_frame().persistent_node_removals
            })
            .unwrap();

        assert_eq!(removed, 2);
    }
}

fn hash_grid_location(location: &crate::GridLocation) -> u64 {
    let mut hasher = RenderFingerprint::new();
    hash_grid_placement_range(&location.row, &mut hasher);
    hash_grid_placement_range(&location.column, &mut hasher);
    hasher.value()
}

fn hash_grid_placement_range(range: &Range<crate::GridPlacement>, hasher: &mut RenderFingerprint) {
    hash_grid_placement(range.start, hasher);
    hash_grid_placement(range.end, hasher);
}

fn hash_display(display: crate::Display, hasher: &mut RenderFingerprint) {
    std::mem::discriminant(&display).hash(hasher);
}

fn hash_visibility(visibility: crate::Visibility, hasher: &mut RenderFingerprint) {
    std::mem::discriminant(&visibility).hash(hasher);
}

fn hash_position(position: crate::Position, hasher: &mut RenderFingerprint) {
    std::mem::discriminant(&position).hash(hasher);
}

fn hash_overflow_point(point: &Point<crate::Overflow>, hasher: &mut RenderFingerprint) {
    hash_overflow_value(point.x, hasher);
    hash_overflow_value(point.y, hasher);
}

fn hash_absolute_length(length: crate::AbsoluteLength, hasher: &mut RenderFingerprint) {
    match length {
        crate::AbsoluteLength::Pixels(pixels) => {
            0u8.hash(hasher);
            pixels.0.to_bits().hash(hasher);
        }
        crate::AbsoluteLength::Rems(rems) => {
            1u8.hash(hasher);
            rems.0.to_bits().hash(hasher);
        }
    }
}

fn hash_length(length: crate::Length, hasher: &mut RenderFingerprint) {
    match length {
        crate::Length::Definite(definite) => {
            0u8.hash(hasher);
            hash_definite_length(definite, hasher);
        }
        crate::Length::Auto => {
            1u8.hash(hasher);
        }
    }
}

fn hash_overflow_value(overflow: crate::Overflow, hasher: &mut RenderFingerprint) {
    std::mem::discriminant(&overflow).hash(hasher);
}

fn hash_definite_length(length: crate::DefiniteLength, hasher: &mut RenderFingerprint) {
    match length {
        crate::DefiniteLength::Absolute(absolute) => {
            0u8.hash(hasher);
            hash_absolute_length(absolute, hasher);
        }
        crate::DefiniteLength::Fraction(fraction) => {
            1u8.hash(hasher);
            fraction.to_bits().hash(hasher);
        }
    }
}

fn hash_size_length(size: &Size<crate::Length>, hasher: &mut RenderFingerprint) {
    hash_length(size.width, hasher);
    hash_length(size.height, hasher);
}

fn hash_size_definite_length(size: &Size<crate::DefiniteLength>, hasher: &mut RenderFingerprint) {
    hash_definite_length(size.width, hasher);
    hash_definite_length(size.height, hasher);
}

fn hash_edges_length(edges: &Edges<crate::Length>, hasher: &mut RenderFingerprint) {
    hash_length(edges.top, hasher);
    hash_length(edges.right, hasher);
    hash_length(edges.bottom, hasher);
    hash_length(edges.left, hasher);
}

fn hash_edges_definite_length(
    edges: &Edges<crate::DefiniteLength>,
    hasher: &mut RenderFingerprint,
) {
    hash_definite_length(edges.top, hasher);
    hash_definite_length(edges.right, hasher);
    hash_definite_length(edges.bottom, hasher);
    hash_definite_length(edges.left, hasher);
}

fn hash_edges_absolute_length(
    edges: &Edges<crate::AbsoluteLength>,
    hasher: &mut RenderFingerprint,
) {
    hash_absolute_length(edges.top, hasher);
    hash_absolute_length(edges.right, hasher);
    hash_absolute_length(edges.bottom, hasher);
    hash_absolute_length(edges.left, hasher);
}

fn hash_grid_placement(placement: crate::GridPlacement, hasher: &mut RenderFingerprint) {
    match placement {
        crate::GridPlacement::Line(index) => {
            0u8.hash(hasher);
            index.hash(hasher);
        }
        crate::GridPlacement::Span(span) => {
            1u8.hash(hasher);
            span.hash(hasher);
        }
        crate::GridPlacement::Auto => {
            2u8.hash(hasher);
        }
    }
}

fn hash_flex_direction(direction: crate::FlexDirection, hasher: &mut RenderFingerprint) {
    std::mem::discriminant(&direction).hash(hasher);
}

fn hash_flex_wrap(wrap: crate::FlexWrap, hasher: &mut RenderFingerprint) {
    std::mem::discriminant(&wrap).hash(hasher);
}

fn hash_optional_align_items(value: Option<crate::AlignItems>, hasher: &mut RenderFingerprint) {
    match value {
        Some(value) => {
            1u8.hash(hasher);
            std::mem::discriminant(&value).hash(hasher);
        }
        None => {
            0u8.hash(hasher);
        }
    }
}

fn hash_optional_align_content(value: Option<crate::AlignContent>, hasher: &mut RenderFingerprint) {
    match value {
        Some(value) => {
            1u8.hash(hasher);
            std::mem::discriminant(&value).hash(hasher);
        }
        None => {
            0u8.hash(hasher);
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
