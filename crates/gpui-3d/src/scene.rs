use crate::{
    AnimationError, AnimationScratch, Light, Material, Mesh, Transform, TransformTrack, Vec2,
};
use std::{borrow::Cow, collections::HashMap, fmt, sync::Arc, time::Duration};

mod bounds;

/// Generational identity for a scene node.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

impl NodeId {
    /// Slot index used for compact scene storage.
    pub fn index(self) -> u32 {
        self.index
    }

    /// Generation used to reject stale node handles.
    pub fn generation(self) -> u32 {
        self.generation
    }
}

/// Scene hierarchy or handle error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SceneError {
    /// The node handle no longer identifies a live node.
    StaleNode,
    /// Reparenting would make a node its own ancestor.
    HierarchyCycle,
    /// The graph has exhausted its 32-bit node index space.
    TooManyNodes,
    /// A world transform or transformed mesh bound is not finite.
    InvalidTransform,
}

impl fmt::Display for SceneError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StaleNode => "scene node handle is stale",
            Self::HierarchyCycle => "scene hierarchy cannot contain a cycle",
            Self::TooManyNodes => "scene graph exceeds the 32-bit node index space",
            Self::InvalidTransform => "scene world transform produces non-finite bounds",
        })
    }
}

impl std::error::Error for SceneError {}

/// Invalid raster adjustment for a scene node.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeError {
    /// A pixel offset contains a non-finite component.
    InvalidPixelOffset,
    /// A depth bias is not finite.
    InvalidDepthBias,
}

impl fmt::Display for NodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidPixelOffset => "node pixel offset must be finite",
            Self::InvalidDepthBias => "node depth bias must be finite",
        })
    }
}

impl std::error::Error for NodeError {}

/// Transformable scene node with optional geometry, material, or light.
#[derive(Clone, Debug, Default)]
#[must_use]
pub struct Node {
    /// Local transform relative to the parent.
    pub transform: Transform,
    /// Optional shared geometry.
    pub mesh: Option<Arc<Mesh>>,
    /// Materials indexed by [`crate::MeshPart::material_slot`].
    pub materials: Vec<Arc<Material>>,
    /// Optional light attached to this node.
    pub light: Option<Light>,
    pixel_offset: Vec2,
    depth_bias: f32,
}

impl Node {
    /// Creates an empty identity node.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches immutable mesh geometry.
    pub fn with_mesh(mut self, mesh: Arc<Mesh>) -> Self {
        self.mesh = Some(mesh);
        self
    }

    /// Attaches materials indexed by mesh-part material slots.
    pub fn with_materials(mut self, materials: impl IntoIterator<Item = Arc<Material>>) -> Self {
        self.materials = materials.into_iter().collect();
        self
    }

    /// Attaches a light.
    pub fn with_light(mut self, light: Light) -> Self {
        self.light = Some(light);
        self
    }

    /// Sets this node's local transform.
    pub fn with_transform(mut self, transform: Transform) -> Self {
        self.transform = transform;
        self
    }

    /// Offsets rasterized pixels after camera projection.
    ///
    /// The offset is measured in render-target pixels: positive X moves right and positive Y moves
    /// down. It affects rendering only; world bounds and CPU ray queries remain unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvalidPixelOffset`] if either component is not finite.
    pub fn with_pixel_offset(mut self, offset: Vec2) -> Result<Self, NodeError> {
        if !offset.x.is_finite() || !offset.y.is_finite() {
            return Err(NodeError::InvalidPixelOffset);
        }
        self.pixel_offset = offset;
        Ok(self)
    }

    /// Biases the rasterized depth after projection.
    ///
    /// Values use the camera's zero-to-one normalized depth range. Positive values move toward
    /// the near plane; negative values move toward the far plane. This affects depth testing only
    /// and does not change world bounds, draw ordering, or CPU ray queries.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvalidDepthBias`] when `bias` is not finite.
    pub fn with_depth_bias(mut self, bias: f32) -> Result<Self, NodeError> {
        if !bias.is_finite() {
            return Err(NodeError::InvalidDepthBias);
        }
        self.depth_bias = bias;
        Ok(self)
    }

    pub(crate) const fn pixel_offset(&self) -> Vec2 {
        self.pixel_offset
    }

    pub(crate) const fn depth_bias(&self) -> f32 {
        self.depth_bias
    }
}

#[derive(Debug)]
struct NodeSlot {
    generation: u32,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    node: Option<Node>,
}

/// Mutable 3D scene graph with generational handles and shared immutable assets.
#[derive(Debug, Default)]
pub struct Scene {
    nodes: Vec<NodeSlot>,
    free_slots: Vec<u32>,
    roots: Vec<NodeId>,
    live_nodes: usize,
}

/// Read-only scene view with transform tracks sampled at one absolute clip time.
///
/// The source graph and its authored node transforms remain unchanged. Rendering preparation and
/// ray queries can consume this same evaluated view so they observe the same pose.
pub struct EvaluatedScene<'a> {
    pub(crate) source: &'a Scene,
    pub(crate) transforms: Cow<'a, HashMap<NodeId, Transform>>,
}

impl EvaluatedScene<'_> {
    /// The authored source graph.
    pub fn source(&self) -> &Scene {
        self.source
    }

    /// Sampled local transform for a live node, or its authored transform when untracked.
    pub fn local_transform(&self, id: NodeId) -> Option<Transform> {
        self.transforms
            .get(&id)
            .copied()
            .or_else(|| self.source.node(id).map(|node| node.transform))
    }
}

impl Scene {
    /// Creates an empty scene.
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluates transform tracks without mutating authored scene data.
    ///
    /// Each node may have at most one track. Missing channels preserve that node's authored local
    /// transform. The returned snapshot is suitable for both [`crate::PreparedScene`] and
    /// [`EvaluatedScene::raycast`](crate::EvaluatedScene::raycast).
    ///
    /// # Errors
    ///
    /// Returns [`AnimationError::StaleNode`] for a removed handle,
    /// [`AnimationError::DuplicateNodeTrack`] for repeated node targets, or a track sampling error
    /// for invalid key data or an invalid authored transform.
    pub fn evaluate(
        &self,
        tracks: &[TransformTrack],
        time: Duration,
    ) -> Result<EvaluatedScene<'_>, AnimationError> {
        let mut scratch = AnimationScratch::new();
        self.sample_transforms(tracks, time, &mut scratch.transforms)?;
        Ok(EvaluatedScene {
            source: self,
            transforms: Cow::Owned(scratch.transforms),
        })
    }

    /// Evaluates transform tracks using caller-owned storage retained across samples.
    ///
    /// The returned view borrows both this scene and `scratch`; drop it before reusing the
    /// scratch value. This avoids allocating a transform map for each sample.
    ///
    /// # Errors
    ///
    /// Returns the same track and authored-transform errors as [`Scene::evaluate`].
    pub fn evaluate_with<'a>(
        &'a self,
        tracks: &[TransformTrack],
        time: Duration,
        scratch: &'a mut AnimationScratch,
    ) -> Result<EvaluatedScene<'a>, AnimationError> {
        self.sample_transforms(tracks, time, &mut scratch.transforms)?;
        Ok(EvaluatedScene {
            source: self,
            transforms: Cow::Borrowed(&scratch.transforms),
        })
    }

    fn sample_transforms(
        &self,
        tracks: &[TransformTrack],
        time: Duration,
        transforms: &mut HashMap<NodeId, Transform>,
    ) -> Result<(), AnimationError> {
        transforms.clear();
        transforms.reserve(tracks.len());
        for track in tracks {
            if transforms.contains_key(&track.node()) {
                return Err(AnimationError::DuplicateNodeTrack);
            }
            let Some(node) = self.node(track.node()) else {
                return Err(AnimationError::StaleNode);
            };
            transforms.insert(track.node(), track.sample(node.transform, time)?);
        }
        Ok(())
    }

    /// Inserts a node below `parent`, or at the scene root when `parent` is `None`.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::StaleNode`] when the requested parent is no longer live, or
    /// [`SceneError::TooManyNodes`] when the 32-bit slot space is exhausted.
    pub fn insert(&mut self, parent: Option<NodeId>, node: Node) -> Result<NodeId, SceneError> {
        if parent.is_some_and(|parent| self.node_slot(parent).is_none()) {
            return Err(SceneError::StaleNode);
        }
        let id = if let Some(index) = self.free_slots.pop() {
            let slot = &mut self.nodes[index as usize];
            slot.parent = parent;
            slot.children.clear();
            slot.node = Some(node);
            NodeId {
                index,
                generation: slot.generation,
            }
        } else {
            let index = u32::try_from(self.nodes.len()).map_err(|_| SceneError::TooManyNodes)?;
            self.nodes.push(NodeSlot {
                generation: 0,
                parent,
                children: Vec::new(),
                node: Some(node),
            });
            NodeId {
                index,
                generation: 0,
            }
        };
        if let Some(parent) = parent {
            self.node_slot_mut(parent)
                .ok_or(SceneError::StaleNode)?
                .children
                .push(id);
        } else {
            self.roots.push(id);
        }
        self.live_nodes += 1;
        Ok(id)
    }

    /// Returns a node by its generational handle.
    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.node_slot(id)?.node.as_ref()
    }

    /// Returns a mutable node by its generational handle.
    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.node_slot_mut(id)?.node.as_mut()
    }

    /// Returns a node's parent.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node_slot(id)?.parent
    }

    /// Returns the children of a node.
    pub fn children(&self, id: NodeId) -> Option<&[NodeId]> {
        Some(&self.node_slot(id)?.children)
    }

    /// Reparents a node while preserving its local transform.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::StaleNode`] for a stale source or parent, or
    /// [`SceneError::HierarchyCycle`] if the new parent is inside the moved subtree.
    pub fn reparent(&mut self, id: NodeId, parent: Option<NodeId>) -> Result<(), SceneError> {
        if self.node_slot(id).is_none()
            || parent.is_some_and(|parent| self.node_slot(parent).is_none())
        {
            return Err(SceneError::StaleNode);
        }
        let mut ancestor = parent;
        while let Some(candidate) = ancestor {
            if candidate == id {
                return Err(SceneError::HierarchyCycle);
            }
            ancestor = self.node_slot(candidate).and_then(|slot| slot.parent);
        }
        let old_parent = self.node_slot(id).ok_or(SceneError::StaleNode)?.parent;
        if old_parent == parent {
            return Ok(());
        }
        if let Some(old_parent) = old_parent {
            self.node_slot_mut(old_parent)
                .ok_or(SceneError::StaleNode)?
                .children
                .retain(|child| *child != id);
        } else {
            self.roots.retain(|root| *root != id);
        }
        self.node_slot_mut(id).ok_or(SceneError::StaleNode)?.parent = parent;
        if let Some(parent) = parent {
            self.node_slot_mut(parent)
                .ok_or(SceneError::StaleNode)?
                .children
                .push(id);
        } else {
            self.roots.push(id);
        }
        Ok(())
    }

    /// Removes a node and all descendants, invalidating their handles.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::StaleNode`] when `id` is not live.
    pub fn remove(&mut self, id: NodeId) -> Result<(), SceneError> {
        let parent = self.node_slot(id).ok_or(SceneError::StaleNode)?.parent;
        if let Some(parent) = parent {
            self.node_slot_mut(parent)
                .ok_or(SceneError::StaleNode)?
                .children
                .retain(|child| *child != id);
        } else {
            self.roots.retain(|root| *root != id);
        }
        let mut pending = vec![id];
        while let Some(current) = pending.pop() {
            let next_generation = {
                let Some(slot) = self.node_slot_mut(current) else {
                    continue;
                };
                pending.append(&mut slot.children);
                slot.node = None;
                slot.parent = None;
                slot.generation.checked_add(1)
            };
            self.live_nodes -= 1;
            if let Some(generation) = next_generation {
                self.nodes[current.index as usize].generation = generation;
                self.free_slots.push(current.index);
            }
        }
        Ok(())
    }

    /// Number of live nodes.
    pub fn len(&self) -> usize {
        self.live_nodes
    }

    /// Whether the scene graph contains no nodes.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    fn node_slot(&self, id: NodeId) -> Option<&NodeSlot> {
        self.nodes
            .get(id.index as usize)
            .filter(|slot| slot.generation == id.generation && slot.node.is_some())
    }

    fn node_slot_mut(&mut self, id: NodeId) -> Option<&mut NodeSlot> {
        self.nodes
            .get_mut(id.index as usize)
            .filter(|slot| slot.generation == id.generation && slot.node.is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reparent_rejects_cycles_and_remove_invalidates_handles() {
        let mut scene = Scene::new();
        let root = scene.insert(None, Node::new()).unwrap();
        let child = scene.insert(Some(root), Node::new()).unwrap();
        assert_eq!(scene.len(), 2);
        assert_eq!(
            scene.reparent(root, Some(child)),
            Err(SceneError::HierarchyCycle)
        );
        scene.remove(root).unwrap();
        assert!(scene.is_empty());
        assert!(scene.node(root).is_none());
        assert!(scene.node(child).is_none());
        let replacement = scene.insert(None, Node::new()).unwrap();
        let stale = if replacement.index() == root.index() {
            root
        } else {
            child
        };
        assert!(replacement.index() == root.index() || replacement.index() == child.index());
        assert_ne!(replacement.generation(), stale.generation());
    }

    #[test]
    fn node_raster_adjustments_reject_non_finite_values() {
        assert_eq!(
            Node::new()
                .with_pixel_offset(crate::Vec2::new(f32::NAN, 0.0))
                .unwrap_err(),
            NodeError::InvalidPixelOffset
        );
        assert_eq!(
            Node::new().with_depth_bias(f32::INFINITY).unwrap_err(),
            NodeError::InvalidDepthBias
        );
    }

    #[test]
    fn evaluated_transforms_preserve_the_authored_scene_and_reject_stale_or_duplicate_tracks() {
        use crate::{Keyframe, Vec3, Vec3Track};
        use std::time::Duration;

        let mut scene = Scene::new();
        let node = scene.insert(None, Node::new()).unwrap();
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([
                Keyframe::new(Duration::ZERO, Vec3::ZERO),
                Keyframe::new(Duration::from_secs(1), Vec3::X),
            ])
            .unwrap(),
        );
        let evaluated = scene
            .evaluate(&[track.clone()], Duration::from_millis(500))
            .unwrap();
        assert_eq!(
            evaluated.local_transform(node).unwrap().translation,
            Vec3::X * 0.5
        );
        assert_eq!(scene.node(node).unwrap().transform.translation, Vec3::ZERO);
        assert_eq!(
            scene
                .evaluate(&[track.clone(), track], Duration::ZERO)
                .err(),
            Some(AnimationError::DuplicateNodeTrack)
        );

        scene.remove(node).unwrap();
        assert_eq!(
            scene
                .evaluate(&[TransformTrack::new(node)], Duration::ZERO)
                .err(),
            Some(AnimationError::StaleNode)
        );
    }

    #[test]
    fn evaluate_with_reuses_transform_storage_between_samples() {
        use crate::{Keyframe, Vec3, Vec3Track};
        use std::time::Duration;

        let mut scene = Scene::new();
        let node = scene.insert(None, Node::new()).unwrap();
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([
                Keyframe::new(Duration::ZERO, Vec3::ZERO),
                Keyframe::new(Duration::from_secs(1), Vec3::X),
            ])
            .unwrap(),
        );
        let mut scratch = AnimationScratch::new();
        let first_capacity = {
            let evaluated = scene
                .evaluate_with(&[track.clone()], Duration::ZERO, &mut scratch)
                .unwrap();
            assert_eq!(
                evaluated.local_transform(node).unwrap().translation,
                Vec3::ZERO
            );
            evaluated.transforms.capacity()
        };
        let second_capacity = {
            let evaluated = scene
                .evaluate_with(&[track], Duration::from_secs(1), &mut scratch)
                .unwrap();
            assert_eq!(
                evaluated.local_transform(node).unwrap().translation,
                Vec3::X
            );
            evaluated.transforms.capacity()
        };

        assert_eq!(first_capacity, second_capacity);
        scratch.trim();
        assert_eq!(scratch.transforms.capacity(), 0);
    }
}
