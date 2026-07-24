use crate::{
    App, Bounds, DefiniteLength, LayoutStyle, Length, Pixels, Size, Style, Window, point, relative,
    size,
};
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

#[derive(Clone)]
struct NodeLayoutMetadata {
    style: LayoutStyle,
    rem_size: Pixels,
    scale_factor: f32,
}

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
    unrounded_layout_origins: FxHashMap<LayoutId, (f32, f32)>,
    pub(super) computed_layouts: FxHashSet<LayoutId>,
    pub(super) node_fingerprints: FxHashMap<LayoutId, Option<u64>>,
    node_layout_metadata: FxHashMap<LayoutId, NodeLayoutMetadata>,
    pub(super) measured_subtrees: FxHashSet<LayoutId>,
    pub(super) previous_layout_roots: FxHashMap<LayoutRootCacheKey, Vec<RetainedLayoutNode>>,
    pub(super) computed_root_keys: Vec<(LayoutRootCacheKey, LayoutId)>,
    pub(super) subtree_scratch: Vec<LayoutId>,
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
    pub fn new() -> Self {
        // Taffy's rounded sizes use cumulative coordinates, while its rounded locations remain
        // parent-relative. GPUI needs both values from the same absolute coordinate system.
        let mut taffy = TaffyTree::new();
        taffy.disable_rounding();
        TaffyLayoutEngine {
            taffy,
            absolute_layout_bounds: FxHashMap::default(),
            unrounded_layout_origins: FxHashMap::default(),
            computed_layouts: FxHashSet::default(),
            node_fingerprints: FxHashMap::default(),
            node_layout_metadata: FxHashMap::default(),
            measured_subtrees: FxHashSet::default(),
            previous_layout_roots: FxHashMap::default(),
            computed_root_keys: Vec::new(),
            subtree_scratch: Vec::new(),
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
        self.unrounded_layout_origins.clear();
        self.computed_layouts.clear();
        self.node_fingerprints.clear();
        self.node_layout_metadata.clear();
        self.measured_subtrees.clear();
        self.computed_root_keys.clear();
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
        let mut layout_style = LayoutStyle::from(&style);
        self.normalize_percentage_passthrough(&mut layout_style, children);
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
        self.node_layout_metadata.insert(
            layout_id,
            NodeLayoutMetadata {
                style: layout_style,
                rem_size,
                scale_factor,
            },
        );
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
        self.node_layout_metadata.insert(
            layout_id,
            NodeLayoutMetadata {
                style: layout_style,
                rem_size,
                scale_factor,
            },
        );
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
        self.node_layout_metadata.insert(
            layout_id,
            NodeLayoutMetadata {
                style: layout_style,
                rem_size,
                scale_factor,
            },
        );
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
            if self.try_retain_layout(id, &root_key, window, cx) {
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
                self.unrounded_layout_origins.remove(&id);
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

    pub fn layout_bounds(&mut self, id: LayoutId, scale_factor: f32) -> Bounds<Pixels> {
        if let Some(layout) = self.absolute_layout_bounds.get(&id).cloned() {
            self.bounds_cache_hits = self.bounds_cache_hits.saturating_add(1);
            return layout;
        }
        self.bounds_cache_misses = self.bounds_cache_misses.saturating_add(1);

        let layout = self.taffy.layout(id.into()).expect(EXPECT_MESSAGE).clone();
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
        self.taffy
            .set_style(child_id.into(), taffy_style)
            .expect(EXPECT_MESSAGE);
        self.node_layout_metadata.insert(child_id, child);
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
