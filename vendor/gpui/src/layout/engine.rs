use crate::{App, Bounds, LayoutStyle, Pixels, Size, Style, Window, point, size};
use collections::{FxHashMap, FxHashSet, FxHasher};
use smallvec::SmallVec;
use stacksafe::{StackSafe, stacksafe};
use std::hash::{Hash, Hasher};
use taffy::TaffyTree;

use super::{
    convert::ToTaffy,
    fingerprint::{
        hash_absolute_length, hash_display, hash_edges_absolute_length, hash_edges_definite_length,
        hash_edges_length, hash_flex_direction, hash_flex_wrap, hash_grid_location, hash_length,
        hash_optional_align_content, hash_optional_align_items, hash_overflow_point, hash_position,
        hash_size_definite_length, hash_size_length,
    },
    metrics::{AvailableSpace, LayoutId, LayoutRootCacheKey, RetainedLayoutNode},
};

pub(super) type NodeMeasureFn = StackSafe<
    Box<
        dyn FnMut(
            Size<Option<Pixels>>,
            Size<AvailableSpace>,
            &mut Window,
            &mut App,
        ) -> Size<Pixels>,
    >,
>;

pub(super) struct NodeContext {
    pub(super) measure: NodeMeasureFn,
    pub(super) is_pure: bool,
    pub(super) last_measure_input: Option<(Size<Option<Pixels>>, Size<AvailableSpace>)>,
}

pub(super) const EXPECT_MESSAGE: &str =
    "we should avoid taffy layout errors by construction if possible";

pub struct TaffyLayoutEngine {
    pub(super) taffy: TaffyTree<NodeContext>,
    pub(super) absolute_layout_bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    pub(super) computed_layouts: FxHashSet<LayoutId>,
    pub(super) node_fingerprints: FxHashMap<LayoutId, Option<u64>>,
    pub(super) measured_subtrees: FxHashSet<LayoutId>,
    pub(super) previous_layout_roots: FxHashMap<LayoutRootCacheKey, Vec<RetainedLayoutNode>>,
    pub(super) computed_root_keys: Vec<(LayoutRootCacheKey, LayoutId)>,
    pub(super) retained_layout_bounds: FxHashMap<LayoutId, Bounds<Pixels>>,
    pub(super) nodes_requested: usize,
    pub(super) measured_nodes_requested: usize,
    pub(super) roots_computed: usize,
    pub(super) bounds_cache_hits: usize,
    pub(super) bounds_cache_misses: usize,
    pub(super) layout_cache_hits: usize,
    pub(super) layout_cache_misses: usize,
    pub(super) layout_cache_reused_roots: usize,
    pub(super) layout_cache_saved_roots: usize,
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
            previous_layout_roots: FxHashMap::default(),
            computed_root_keys: Vec::new(),
            retained_layout_bounds: FxHashMap::default(),
            nodes_requested: 0,
            measured_nodes_requested: 0,
            roots_computed: 0,
            bounds_cache_hits: 0,
            bounds_cache_misses: 0,
            layout_cache_hits: 0,
            layout_cache_misses: 0,
            layout_cache_reused_roots: 0,
            layout_cache_saved_roots: 0,
        }
    }

    pub fn clear(&mut self) {
        self.save_retained_layout_roots();
        self.taffy.clear();
        self.absolute_layout_bounds.clear();
        self.computed_layouts.clear();
        self.node_fingerprints.clear();
        self.measured_subtrees.clear();
        self.computed_root_keys.clear();
        self.retained_layout_bounds.clear();
        self.nodes_requested = 0;
        self.measured_nodes_requested = 0;
        self.roots_computed = 0;
        self.bounds_cache_hits = 0;
        self.bounds_cache_misses = 0;
        self.layout_cache_hits = 0;
        self.layout_cache_misses = 0;
        self.layout_cache_reused_roots = 0;
        self.layout_cache_saved_roots = 0;
    }

    pub fn request_layout(
        &mut self,
        style: Style,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> LayoutId {
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        let layout_style = LayoutStyle::from(&style);
        let fingerprint = self.layout_fingerprint(&layout_style, rem_size, scale_factor, children);
        let taffy_style = layout_style.to_taffy(rem_size, scale_factor);

        let layout_id = if children.is_empty() {
            self.taffy
                .new_leaf(taffy_style)
                .expect(EXPECT_MESSAGE)
                .into()
        } else {
            self.taffy
                // This is safe because LayoutId is repr(transparent) to taffy::tree::NodeId.
                .new_with_children(taffy_style, LayoutId::to_taffy_slice(children))
                .expect(EXPECT_MESSAGE)
                .into()
        };
        self.node_fingerprints.insert(layout_id, fingerprint);
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
        let layout_style = LayoutStyle::from(&style);
        let fingerprint = fingerprint_seed.map(|seed| {
            let mut hasher = FxHasher::default();
            self.hash_layout_style(&layout_style, rem_size, scale_factor, &mut hasher);
            seed.hash(&mut hasher);
            hasher.finish()
        });
        let taffy_style = layout_style.to_taffy(rem_size, scale_factor);

        let layout_id = self
            .taffy
            .new_leaf_with_context(
                taffy_style,
                NodeContext {
                    measure: StackSafe::new(Box::new(measure)),
                    is_pure: fingerprint_seed.is_some(),
                    last_measure_input: None,
                },
            )
            .expect(EXPECT_MESSAGE)
            .into();
        self.node_fingerprints.insert(layout_id, fingerprint);
        self.measured_subtrees.insert(layout_id);
        layout_id
    }

    pub fn request_impure_measured_layout_with_fingerprint(
        &mut self,
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
        self.nodes_requested = self.nodes_requested.saturating_add(1);
        self.measured_nodes_requested = self.measured_nodes_requested.saturating_add(1);
        let layout_style = LayoutStyle::from(&style);
        let mut hasher = FxHasher::default();
        self.hash_layout_style(&layout_style, rem_size, scale_factor, &mut hasher);
        fingerprint_seed.hash(&mut hasher);
        let fingerprint = hasher.finish();
        let taffy_style = layout_style.to_taffy(rem_size, scale_factor);

        let layout_id = self
            .taffy
            .new_leaf_with_context(
                taffy_style,
                NodeContext {
                    measure: StackSafe::new(Box::new(measure)),
                    is_pure: false,
                    last_measure_input: None,
                },
            )
            .expect(EXPECT_MESSAGE)
            .into();
        self.node_fingerprints.insert(layout_id, Some(fingerprint));
        self.measured_subtrees.insert(layout_id);
        layout_id
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
                self.replay_impure_measurements(id, window, cx);
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
        // for (a, b) in self.edges(id)? {
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

                    node_context.last_measure_input = Some((known_dimensions, available_space));
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

    fn replay_impure_measurements(&mut self, root_id: LayoutId, window: &mut Window, cx: &mut App) {
        for layout_id in self.subtree_nodes(root_id) {
            let Some(node_context) = self.taffy.get_node_context_mut(layout_id.into()) else {
                continue;
            };
            if node_context.is_pure {
                continue;
            }
            let Some((known_dimensions, available_space)) = node_context.last_measure_input else {
                continue;
            };
            (node_context.measure)(known_dimensions, available_space, window, cx);
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

    fn layout_fingerprint(
        &self,
        style: &LayoutStyle,
        rem_size: Pixels,
        scale_factor: f32,
        children: &[LayoutId],
    ) -> Option<u64> {
        let mut hasher = FxHasher::default();
        self.hash_layout_style(style, rem_size, scale_factor, &mut hasher);
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
