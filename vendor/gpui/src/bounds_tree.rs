use crate::{Bounds, Half};
use std::{
    cmp,
    fmt::Debug,
    ops::{Add, Sub},
    ptr::NonNull,
};

const MAX_CHILDREN: usize = 12;

#[derive(Debug)]
pub(crate) struct BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    nodes: Vec<Node<U>>,
    root: Option<usize>,
    max_leaf: Option<usize>,
    insert_path: Vec<usize>,
    search_stack: Vec<NonNull<Node<U>>>,
}

#[derive(Debug, Clone)]
struct Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    bounds: Bounds<U>,
    max_order: u32,
    kind: NodeKind,
}

#[derive(Debug, Clone)]
enum NodeKind {
    Leaf { order: u32 },
    Internal { children: NodeChildren },
}

#[derive(Debug, Clone)]
struct NodeChildren {
    indices: [usize; MAX_CHILDREN],
    len: u8,
}

impl NodeChildren {
    fn new() -> Self {
        Self {
            indices: [0; MAX_CHILDREN],
            len: 0,
        }
    }

    fn push(&mut self, index: usize) {
        debug_assert!((self.len as usize) < MAX_CHILDREN);
        self.indices[self.len as usize] = index;
        self.len += 1;
    }

    fn len(&self) -> usize {
        self.len as usize
    }

    fn as_slice(&self) -> &[usize] {
        &self.indices[..self.len as usize]
    }
}

impl<U> BoundsTree<U>
where
    U: Clone
        + Debug
        + PartialEq
        + PartialOrd
        + Add<U, Output = U>
        + Sub<Output = U>
        + Half
        + Default,
{
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.root = None;
        self.max_leaf = None;
        self.insert_path.clear();
        self.search_stack.clear();
    }

    pub fn insert(&mut self, new_bounds: Bounds<U>) -> u32 {
        let max_intersecting = self.find_max_ordering(&new_bounds);
        let ordering = max_intersecting + 1;
        let new_leaf_index = self.insert_leaf(new_bounds, ordering);

        self.max_leaf = match self.max_leaf {
            None => Some(new_leaf_index),
            Some(old_index) if self.nodes[old_index].max_order < ordering => Some(new_leaf_index),
            existing => existing,
        };

        ordering
    }

    fn find_max_ordering(&mut self, query: &Bounds<U>) -> u32 {
        let Some(root_index) = self.root else {
            return 0;
        };

        if let Some(max_index) = self.max_leaf {
            let max_node = &self.nodes[max_index];
            if query.intersects(&max_node.bounds) {
                return max_node.max_order;
            }
        }

        self.search_stack.clear();
        self.search_stack
            .push(NonNull::from(&self.nodes[root_index]));

        let mut max_found = 0u32;
        while let Some(node) = self.search_stack.pop() {
            // SAFETY: The stack only contains pointers into `self.nodes`, and this method does
            // not mutate or reallocate `self.nodes` while those pointers are live.
            let node = unsafe { node.as_ref() };

            if node.max_order <= max_found || !query.intersects(&node.bounds) {
                continue;
            }

            match &node.kind {
                NodeKind::Leaf { order } => {
                    max_found = cmp::max(max_found, *order);
                }
                NodeKind::Internal { children } => {
                    self.search_stack.extend(
                        children
                            .as_slice()
                            .iter()
                            .map(|&child_index| &self.nodes[child_index])
                            .filter(|node| node.max_order > max_found)
                            .map(NonNull::from),
                    );
                }
            }
        }

        max_found
    }

    fn insert_leaf(&mut self, bounds: Bounds<U>, order: u32) -> usize {
        let new_leaf_index = self.nodes.len();
        self.nodes.push(Node {
            bounds: bounds.clone(),
            max_order: order,
            kind: NodeKind::Leaf { order },
        });

        let Some(root_index) = self.root else {
            self.root = Some(new_leaf_index);
            return new_leaf_index;
        };

        if matches!(self.nodes[root_index].kind, NodeKind::Leaf { .. }) {
            let root_bounds = self.nodes[root_index].bounds.clone();
            let root_order = self.nodes[root_index].max_order;

            let mut children = NodeChildren::new();
            if order > root_order {
                children.push(root_index);
                children.push(new_leaf_index);
            } else {
                children.push(new_leaf_index);
                children.push(root_index);
            }

            let new_root_index = self.nodes.len();
            self.nodes.push(Node {
                bounds: root_bounds.union(&bounds),
                max_order: cmp::max(root_order, order),
                kind: NodeKind::Internal { children },
            });
            self.root = Some(new_root_index);
            return new_leaf_index;
        }

        self.insert_path.clear();
        let mut current_index = root_index;

        loop {
            let current = &self.nodes[current_index];
            let NodeKind::Internal { children } = &current.kind else {
                unreachable!("Should only traverse internal nodes");
            };

            self.insert_path.push(current_index);

            let mut best_child_index = children.as_slice()[0];
            let mut best_child_position = 0;
            let mut best_cost = bounds
                .union(&self.nodes[best_child_index].bounds)
                .half_perimeter();

            for (position, &child_index) in children.as_slice().iter().enumerate().skip(1) {
                let cost = bounds
                    .union(&self.nodes[child_index].bounds)
                    .half_perimeter();
                if cost < best_cost {
                    best_cost = cost;
                    best_child_index = child_index;
                    best_child_position = position;
                }
            }

            if matches!(self.nodes[best_child_index].kind, NodeKind::Leaf { .. }) {
                if children.len() < MAX_CHILDREN {
                    let node = &mut self.nodes[current_index];
                    if let NodeKind::Internal { children } = &mut node.kind {
                        children.push(new_leaf_index);
                        if order <= node.max_order {
                            let last = children.len() - 1;
                            children.indices.swap(last - 1, last);
                        }
                    }

                    node.bounds = node.bounds.union(&bounds);
                    node.max_order = cmp::max(node.max_order, order);
                    break;
                }

                let sibling_bounds = self.nodes[best_child_index].bounds.clone();
                let sibling_order = self.nodes[best_child_index].max_order;

                let mut new_children = NodeChildren::new();
                if order > sibling_order {
                    new_children.push(best_child_index);
                    new_children.push(new_leaf_index);
                } else {
                    new_children.push(new_leaf_index);
                    new_children.push(best_child_index);
                }

                let new_internal_index = self.nodes.len();
                let new_internal_max = cmp::max(sibling_order, order);
                self.nodes.push(Node {
                    bounds: sibling_bounds.union(&bounds),
                    max_order: new_internal_max,
                    kind: NodeKind::Internal {
                        children: new_children,
                    },
                });

                let parent = &mut self.nodes[current_index];
                if let NodeKind::Internal { children } = &mut parent.kind {
                    let children_len = children.len();
                    children.indices[best_child_position] = new_internal_index;

                    if new_internal_max > parent.max_order {
                        children.indices.swap(best_child_position, children_len - 1);
                    }
                }
                break;
            }

            current_index = best_child_index;
        }

        let mut updated_child_index = None;
        for &node_index in self.insert_path.iter().rev() {
            let node = &mut self.nodes[node_index];
            node.bounds = node.bounds.union(&bounds);

            if node.max_order < order {
                node.max_order = order;

                if let Some(child_index) = updated_child_index {
                    if let NodeKind::Internal { children } = &mut node.kind
                        && let Some(position) = children
                            .as_slice()
                            .iter()
                            .position(|&child| child == child_index)
                    {
                        let last = children.len() - 1;
                        if position != last {
                            children.indices.swap(position, last);
                        }
                    }
                }
            }

            updated_child_index = Some(node_index);
        }

        new_leaf_index
    }
}

impl<U> Default for BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn default() -> Self {
        Self {
            nodes: Vec::new(),
            root: None,
            max_leaf: None,
            insert_path: Vec::new(),
            search_stack: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bounds, Point, Size};
    use rand::{RngExt as _, SeedableRng};

    #[test]
    fn test_insert() {
        let mut tree = BoundsTree::<f32>::default();
        let bounds1 = Bounds {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds2 = Bounds {
            origin: Point { x: 5.0, y: 5.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds3 = Bounds {
            origin: Point { x: 10.0, y: 10.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };

        assert_eq!(tree.insert(bounds1), 1);
        assert_eq!(tree.insert(bounds2), 2);
        assert_eq!(tree.insert(bounds3), 3);

        let bounds4 = Bounds {
            origin: Point { x: 20.0, y: 20.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds5 = Bounds {
            origin: Point { x: 40.0, y: 40.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        let bounds6 = Bounds {
            origin: Point { x: 25.0, y: 25.0 },
            size: Size {
                width: 10.0,
                height: 10.0,
            },
        };
        assert_eq!(tree.insert(bounds4), 1);
        assert_eq!(tree.insert(bounds5), 1);
        assert_eq!(tree.insert(bounds6), 2);
    }

    #[test]
    fn full_window_background_keeps_later_overlays_above_it() {
        let mut tree = BoundsTree::<f32>::default();
        let background = Bounds {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 1000.0,
                height: 600.0,
            },
        };
        assert_eq!(tree.insert(background), 1);

        for index in 0..96 {
            let x = (index % 12) as f32 * 72.0;
            let y = (index / 12) as f32 * 48.0;
            let bounds = Bounds {
                origin: Point { x, y },
                size: Size {
                    width: 24.0,
                    height: 20.0,
                },
            };
            assert!(tree.insert(bounds) > 1);
        }

        let topbar = Bounds {
            origin: Point { x: 0.0, y: 0.0 },
            size: Size {
                width: 1000.0,
                height: 72.0,
            },
        };
        let launch_button = Bounds {
            origin: Point { x: 650.0, y: 500.0 },
            size: Size {
                width: 280.0,
                height: 72.0,
            },
        };

        assert!(tree.insert(topbar) > 1);
        assert!(tree.insert(launch_button) > 1);
    }

    #[test]
    fn test_random_iterations() {
        let max_bounds = 100;
        for seed in 1..=1000 {
            let mut tree = BoundsTree::default();
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u64);
            let mut expected_quads: Vec<(Bounds<f32>, u32)> = Vec::new();

            let num_bounds = rng.random_range(1..=max_bounds);
            for _ in 0..num_bounds {
                let min_x: f32 = rng.random_range(-100.0..100.0);
                let min_y: f32 = rng.random_range(-100.0..100.0);
                let width: f32 = rng.random_range(0.0..50.0);
                let height: f32 = rng.random_range(0.0..50.0);
                let bounds = Bounds {
                    origin: Point { x: min_x, y: min_y },
                    size: Size { width, height },
                };

                let expected_ordering = expected_quads
                    .iter()
                    .filter_map(|quad| quad.0.intersects(&bounds).then_some(quad.1))
                    .max()
                    .unwrap_or(0)
                    + 1;
                expected_quads.push((bounds, expected_ordering));

                let actual_ordering = tree.insert(bounds);
                assert_eq!(actual_ordering, expected_ordering);
            }
        }
    }
}
