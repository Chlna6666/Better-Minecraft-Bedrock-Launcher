use super::{EvaluatedScene, Node, NodeId, Scene, SceneError};
use crate::{Aabb, Mat4, Mesh, Transform};

impl Scene {
    /// Returns world-space bounds for every non-empty mesh in the authored scene pose.
    ///
    /// Parent transforms are applied, and meshes are included regardless of camera visibility or
    /// material. Empty scenes and scenes without vertices return `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::InvalidTransform`] when a node transform or transformed mesh bounds
    /// are not finite, and [`SceneError::StaleNode`] if the scene hierarchy contains an invalid
    /// handle.
    pub fn bounds(&self) -> Result<Option<Aabb>, SceneError> {
        self.bounds_with(|_, node| node.transform)
    }

    fn bounds_with(
        &self,
        local_transform: impl Fn(NodeId, &Node) -> Transform,
    ) -> Result<Option<Aabb>, SceneError> {
        let mut stack = Vec::with_capacity(self.roots.len());
        stack.extend(self.roots.iter().copied().map(|id| (id, Mat4::IDENTITY)));
        let mut scene_bounds: Option<Aabb> = None;

        while let Some((id, parent_world)) = stack.pop() {
            let node = self.node(id).ok_or(SceneError::StaleNode)?;
            let world = parent_world * local_transform(id, node).matrix();
            if !world.is_finite() {
                return Err(SceneError::InvalidTransform);
            }

            if let Some(mesh_bounds) = node.mesh.as_deref().and_then(Mesh::bounds) {
                let transformed = mesh_bounds.transformed(world);
                let Some(transformed) = Aabb::new(transformed.min, transformed.max) else {
                    return Err(SceneError::InvalidTransform);
                };
                scene_bounds = Some(match scene_bounds {
                    Some(bounds) => {
                        let Some(bounds) = Aabb::new(
                            bounds.min.min(transformed.min),
                            bounds.max.max(transformed.max),
                        ) else {
                            return Err(SceneError::InvalidTransform);
                        };
                        bounds
                    }
                    None => transformed,
                });
            }

            let children = self.children(id).ok_or(SceneError::StaleNode)?;
            stack.extend(children.iter().copied().map(|child| (child, world)));
        }

        Ok(scene_bounds)
    }
}

impl EvaluatedScene<'_> {
    /// Returns world-space bounds using this snapshot's sampled node transforms.
    ///
    /// The result includes every non-empty mesh regardless of camera visibility or material. Empty
    /// scenes and scenes without vertices return `Ok(None)`.
    ///
    /// # Errors
    ///
    /// Returns [`SceneError::InvalidTransform`] when a sampled transform or transformed mesh bounds
    /// are not finite, and [`SceneError::StaleNode`] if the source hierarchy contains an invalid
    /// handle.
    pub fn bounds(&self) -> Result<Option<Aabb>, SceneError> {
        self.source
            .bounds_with(|id, node| self.local_transform(id).unwrap_or(node.transform))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Keyframe, Mesh, TransformTrack, Vec3, Vec3Track};
    use std::{sync::Arc, time::Duration};

    #[test]
    fn scene_bounds_include_parent_transforms_and_all_roots() {
        let mut scene = Scene::new();
        let root = scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_transform(Transform {
                        translation: Vec3::new(2.0, 0.0, 0.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();
        scene
            .insert(
                Some(root),
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_transform(Transform {
                        translation: Vec3::new(0.0, 3.0, 0.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_transform(Transform {
                        translation: Vec3::new(100.0, 0.0, 0.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();

        assert_eq!(
            scene.bounds().unwrap(),
            Aabb::new(Vec3::new(1.5, -0.5, -0.5), Vec3::new(100.5, 3.5, 0.5),)
        );
    }

    #[test]
    fn evaluated_scene_bounds_follow_the_sampled_pose() {
        let mut scene = Scene::new();
        let node = scene
            .insert(None, Node::new().with_mesh(Arc::new(Mesh::cube())))
            .unwrap();
        let track = TransformTrack::new(node).with_translation(
            Vec3Track::new([
                Keyframe::new(Duration::ZERO, Vec3::ZERO),
                Keyframe::new(Duration::from_secs(1), Vec3::new(4.0, 0.0, 0.0)),
            ])
            .unwrap(),
        );
        let tracks = [track];

        let evaluated = scene.evaluate(&tracks, Duration::from_secs(1)).unwrap();

        assert_eq!(
            evaluated.bounds().unwrap(),
            Aabb::new(Vec3::new(3.5, -0.5, -0.5), Vec3::new(4.5, 0.5, 0.5),)
        );
        assert_eq!(
            scene.bounds().unwrap(),
            Aabb::new(Vec3::new(-0.5, -0.5, -0.5), Vec3::new(0.5, 0.5, 0.5),)
        );
    }

    #[test]
    fn empty_scene_and_transform_only_nodes_have_no_bounds() {
        let mut scene = Scene::new();
        scene.insert(None, Node::new()).unwrap();

        assert_eq!(scene.bounds().unwrap(), None);
    }

    #[test]
    fn scene_bounds_reject_non_finite_world_transforms() {
        let mut scene = Scene::new();
        scene
            .insert(
                None,
                Node::new()
                    .with_mesh(Arc::new(Mesh::cube()))
                    .with_transform(Transform {
                        translation: Vec3::new(f32::NAN, 0.0, 0.0),
                        ..Transform::IDENTITY
                    }),
            )
            .unwrap();

        assert_eq!(scene.bounds(), Err(SceneError::InvalidTransform));
    }
}
