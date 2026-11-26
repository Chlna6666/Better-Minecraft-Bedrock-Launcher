use crate::{Bounds, Half};
use std::{
    cmp,
    fmt::Debug,
    ops::{Add, Sub},
};

#[derive(Debug)]
pub(crate) struct BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    root: Option<usize>,
    nodes: Vec<Node<U>>,
    stack: Vec<usize>,
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
        self.root = None;
        self.nodes.clear();
        self.stack.clear();
    }

    pub(crate) fn retained_capacity(&self) -> usize {
        self.nodes.capacity() + self.stack.capacity()
    }

    pub(crate) fn trim_for_reuse(&mut self, current: &Self, floor: usize, multiplier: usize) {
        let multiplier = multiplier.max(1);
        let node_target = floor.max(current.nodes.len());
        if self.nodes.capacity() > node_target.saturating_mul(multiplier) {
            self.nodes.shrink_to(node_target);
        }
        let stack_target = floor.min(8).max(current.stack.len());
        if self.stack.capacity() > stack_target.saturating_mul(multiplier) {
            self.stack.shrink_to(stack_target);
        }
    }

    pub fn insert(&mut self, new_bounds: Bounds<U>) -> u32 {
        self.insert_internal(new_bounds, None)
    }

    pub(crate) fn insert_ordered(&mut self, new_bounds: Bounds<U>, order: u32) -> u32 {
        self.insert_internal(new_bounds, Some(order))
    }

    /// Import already proven orders as a balanced subtree. Future order
    /// queries depend on leaf bounds and maximum orders, not insertion shape.
    /// Only the subtree root needs to descend through the existing tree.
    pub(crate) fn extend_ordered(
        &mut self,
        count: usize,
        mut leaves: impl Iterator<Item = (Bounds<U>, u32)>,
    ) {
        if count == 0 {
            return;
        }
        if count == 1 {
            let (bounds, order) = leaves.next().expect("qualified ordering leaf");
            self.insert_ordered(bounds, order);
            return;
        }
        self.nodes.reserve(count.saturating_mul(2));
        let subtree = self.build_ordered_subtree(count, &mut leaves);
        debug_assert!(leaves.next().is_none());
        let Some(mut index) = self.root else {
            self.root = Some(subtree);
            return;
        };
        let bounds = self.nodes[subtree].bounds().clone();
        let order = self.nodes[subtree].max_ordering();
        while let Node::Internal { left, right, bounds: node_bounds, .. } = &mut self.nodes[index] {
            let left = *left;
            let right = *right;
            *node_bounds = node_bounds.union(&bounds);
            self.stack.push(index);
            let left_cost = bounds.union(self.nodes[left].bounds()).half_perimeter();
            let right_cost = bounds.union(self.nodes[right].bounds()).half_perimeter();
            index = if left_cost < right_cost { left } else { right };
        }
        self.attach_subtree(index, subtree, order);
    }

    fn build_ordered_subtree(
        &mut self,
        count: usize,
        leaves: &mut impl Iterator<Item = (Bounds<U>, u32)>,
    ) -> usize {
        if count == 1 {
            let (bounds, order) = leaves.next().expect("qualified ordering leaf");
            return self.push_leaf(bounds, order);
        }
        let left_count = count / 2;
        let left = self.build_ordered_subtree(left_count, leaves);
        let right = self.build_ordered_subtree(count - left_count, leaves);
        self.push_internal(left, right)
    }

    fn insert_internal(&mut self, new_bounds: Bounds<U>, retained_order: Option<u32>) -> u32 {
        // If the tree is empty, make the root the new leaf.
        if self.root.is_none() {
            let order = retained_order.unwrap_or(1);
            let new_node = self.push_leaf(new_bounds, order);
            self.root = Some(new_node);
            return order;
        }

        // Search for the best place to add the new leaf based on heuristics.
        let mut max_intersecting_ordering = 0;
        let mut index = self.root.unwrap();
        while let Node::Internal {
            left,
            right,
            bounds: node_bounds,
            ..
        } = &mut self.nodes[index]
        {
            let left = *left;
            let right = *right;
            *node_bounds = node_bounds.union(&new_bounds);
            self.stack.push(index);

            // Descend to the best-fit child, based on which one would increase
            // the surface area the least. This attempts to keep the tree balanced
            // in terms of surface area. If there is an intersection with the other child,
            // add its keys to the intersections vector.
            let left_cost = new_bounds.union(self.nodes[left].bounds()).half_perimeter();
            let right_cost = new_bounds
                .union(self.nodes[right].bounds())
                .half_perimeter();
            if left_cost < right_cost {
                if retained_order.is_none() {
                    max_intersecting_ordering =
                        self.find_max_ordering(right, &new_bounds, max_intersecting_ordering);
                }
                index = left;
            } else {
                if retained_order.is_none() {
                    max_intersecting_ordering =
                        self.find_max_ordering(left, &new_bounds, max_intersecting_ordering);
                }
                index = right;
            }
        }

        // We've found a leaf ('index' now refers to a leaf node).
        // We'll insert a new parent node above the leaf and attach our new leaf to it.
        let sibling = index;

        // Check for collision with the located leaf node
        let Node::Leaf {
            bounds: sibling_bounds,
            order: sibling_ordering,
            ..
        } = &self.nodes[index]
        else {
            unreachable!();
        };
        if retained_order.is_none() && sibling_bounds.intersects(&new_bounds) {
            max_intersecting_ordering = cmp::max(max_intersecting_ordering, *sibling_ordering);
        }

        let ordering = retained_order.unwrap_or(max_intersecting_ordering + 1);
        let new_node = self.push_leaf(new_bounds, ordering);
        self.attach_subtree(sibling, new_node, ordering);
        ordering
    }

    fn attach_subtree(&mut self, sibling: usize, subtree: usize, ordering: u32) {
        let new_parent = self.push_internal(sibling, subtree);

        // If there was an old parent, we need to update its children indices.
        if let Some(old_parent) = self.stack.last().copied() {
            let Node::Internal { left, right, .. } = &mut self.nodes[old_parent] else {
                unreachable!();
            };

            if *left == sibling {
                *left = new_parent;
            } else {
                *right = new_parent;
            }
        } else {
            // If the old parent was the root, the new parent is the new root.
            self.root = Some(new_parent);
        }

        for node_index in self.stack.drain(..).rev() {
            let Node::Internal {
                max_order: max_ordering,
                ..
            } = &mut self.nodes[node_index]
            else {
                unreachable!()
            };
            if *max_ordering >= ordering {
                break;
            }
            *max_ordering = ordering;
        }
    }

    fn find_max_ordering(&self, index: usize, bounds: &Bounds<U>, mut max_ordering: u32) -> u32 {
        match &self.nodes[index] {
            Node::Leaf {
                bounds: node_bounds,
                order: ordering,
                ..
            } => {
                if bounds.intersects(node_bounds) {
                    max_ordering = cmp::max(*ordering, max_ordering);
                }
            }
            Node::Internal {
                left,
                right,
                bounds: node_bounds,
                max_order: node_max_ordering,
                ..
            } => {
                if bounds.intersects(node_bounds) && max_ordering < *node_max_ordering {
                    let left_max_ordering = self.nodes[*left].max_ordering();
                    let right_max_ordering = self.nodes[*right].max_ordering();
                    if left_max_ordering > right_max_ordering {
                        max_ordering = self.find_max_ordering(*left, bounds, max_ordering);
                        max_ordering = self.find_max_ordering(*right, bounds, max_ordering);
                    } else {
                        max_ordering = self.find_max_ordering(*right, bounds, max_ordering);
                        max_ordering = self.find_max_ordering(*left, bounds, max_ordering);
                    }
                }
            }
        }
        max_ordering
    }

    fn push_leaf(&mut self, bounds: Bounds<U>, order: u32) -> usize {
        self.nodes.push(Node::Leaf { bounds, order });
        self.nodes.len() - 1
    }

    fn push_internal(&mut self, left: usize, right: usize) -> usize {
        let left_node = &self.nodes[left];
        let right_node = &self.nodes[right];
        let new_bounds = left_node.bounds().union(right_node.bounds());
        let max_ordering = cmp::max(left_node.max_ordering(), right_node.max_ordering());
        self.nodes.push(Node::Internal {
            bounds: new_bounds,
            left,
            right,
            max_order: max_ordering,
        });
        self.nodes.len() - 1
    }
}

impl<U> Default for BoundsTree<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn default() -> Self {
        BoundsTree {
            root: None,
            nodes: Vec::new(),
            stack: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
enum Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    Leaf {
        bounds: Bounds<U>,
        order: u32,
    },
    Internal {
        left: usize,
        right: usize,
        bounds: Bounds<U>,
        max_order: u32,
    },
}

impl<U> Node<U>
where
    U: Clone + Debug + Default + PartialEq,
{
    fn bounds(&self) -> &Bounds<U> {
        match self {
            Node::Leaf { bounds, .. } => bounds,
            Node::Internal { bounds, .. } => bounds,
        }
    }

    fn max_ordering(&self) -> u32 {
        match self {
            Node::Leaf {
                order: ordering, ..
            } => *ordering,
            Node::Internal {
                max_order: max_ordering,
                ..
            } => *max_ordering,
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

        // Insert the bounds into the tree and verify the order is correct
        assert_eq!(tree.insert(bounds1), 1);
        assert_eq!(tree.insert(bounds2), 2);
        assert_eq!(tree.insert(bounds3), 3);

        // Insert non-overlapping bounds and verify they can reuse orders
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
        assert_eq!(tree.insert(bounds4), 1); // bounds4 does not overlap with bounds1, bounds2, or bounds3
        assert_eq!(tree.insert(bounds5), 1); // bounds5 does not overlap with any other bounds
        assert_eq!(tree.insert(bounds6), 2); // bounds6 overlaps with bounds4, so it should have a different order
    }

    #[test]
    fn test_random_iterations() {
        let max_bounds = 100;
        for seed in 1..=1000 {
            // let seed = 44;
            let mut tree = BoundsTree::default();
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed as u64);
            let mut expected_quads: Vec<(Bounds<f32>, u32)> = Vec::new();

            // Insert a random number of random AABBs into the tree.
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

                // Insert the AABB into the tree and collect intersections.
                let actual_ordering = tree.insert(bounds);
                assert_eq!(actual_ordering, expected_ordering);
            }
        }
    }

    #[test]
    fn ordered_subtrees_preserve_future_intersection_orders() {
        for seed in 0..100 {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let mut tree = BoundsTree::default();
            let mut reference = Vec::<(Bounds<f32>, u32)>::new();
            let mut random_bounds = || Bounds::new(
                Point { x: rng.random_range(-100.0..100.0), y: rng.random_range(-100.0..100.0) },
                Size { width: rng.random_range(1.0..40.0), height: rng.random_range(1.0..40.0) },
            );
            for count in [0, 1, 2, 7, 32, 3, 64, 11] {
                let mut batch = Vec::new();
                for _ in 0..count {
                    let bounds = random_bounds();
                    let order = reference.iter()
                        .filter(|(old, _)| old.intersects(&bounds))
                        .map(|(_, order)| *order)
                        .max().unwrap_or(0) + 1;
                    reference.push((bounds, order));
                    batch.push((bounds, order));
                }
                tree.extend_ordered(batch.len(), batch.into_iter());
                for _ in 0..8 {
                    let bounds = random_bounds();
                    let expected = reference.iter()
                        .filter(|(old, _)| old.intersects(&bounds))
                        .map(|(_, order)| *order)
                        .max().unwrap_or(0) + 1;
                    assert_eq!(tree.insert(bounds), expected, "seed={seed}");
                    reference.push((bounds, expected));
                }
            }
        }
    }

    #[test]
    fn retained_orders_preserve_future_insert_order() {
        let bounds = [
            Bounds::new(
                Point { x: 0.0, y: 0.0 },
                Size {
                    width: 12.0,
                    height: 12.0,
                },
            ),
            Bounds::new(
                Point { x: 4.0, y: 4.0 },
                Size {
                    width: 12.0,
                    height: 12.0,
                },
            ),
            Bounds::new(
                Point { x: 30.0, y: 30.0 },
                Size {
                    width: 5.0,
                    height: 5.0,
                },
            ),
        ];
        let following = Bounds::new(
            Point { x: 8.0, y: 8.0 },
            Size {
                width: 12.0,
                height: 12.0,
            },
        );

        let mut original = BoundsTree::default();
        let orders = bounds.map(|bounds| original.insert(bounds));
        let expected_following_order = original.insert(following);

        let mut retained = BoundsTree::default();
        for (bounds, order) in bounds.into_iter().zip(orders) {
            retained.insert_ordered(bounds, order);
        }

        assert_eq!(retained.insert(following), expected_following_order);
    }
}
