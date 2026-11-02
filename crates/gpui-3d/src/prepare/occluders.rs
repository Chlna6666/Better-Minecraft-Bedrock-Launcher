use super::*;

impl PreparedScene {
    pub(crate) fn occluder_bounds(
        &self,
        range: Option<Aabb>,
    ) -> impl Iterator<Item = crate::Occluder> + '_ {
        self.occluders
            .values()
            .filter(move |(origin, _)| match (range, origin) {
                (Some(range), Some(origin)) => {
                    origin.x >= range.min.x
                        && origin.x < range.max.x
                        && origin.y >= range.min.y
                        && origin.y < range.max.y
                        && origin.z >= range.min.z
                        && origin.z < range.max.z
                }
                _ => true,
            })
            .flat_map(|(_, bounds)| bounds.iter().copied())
    }

    pub(super) fn record_occluders(&mut self, id: NodeId, node: &Node, world: Mat4) {
        if node.occluders.is_empty()
            || node
                .mesh
                .as_ref()
                .is_none_or(|mesh| mesh.index_count() == 0)
            || !node.lod.is_empty()
            || node.pixel_offset() != Vec2::ZERO
            || node.depth_bias() != 0.0
        {
            return;
        }
        // Rotation, reflection and shear would require remapping the local face mask.
        let columns = world.columns();
        if columns[..3].iter().enumerate().any(|(axis, column)| {
            column[axis] <= 0.0 || (0..3).any(|other| other != axis && column[other] != 0.0)
        }) {
            return;
        }
        let bounds = node
            .occluders
            .iter()
            .filter_map(|occluder| {
                if occluder.faces & 63 == 0 {
                    return None;
                }
                let transformed = occluder.bounds.transformed(world);
                let extent = transformed.max - transformed.min;
                let components = [extent.x, extent.y, extent.z];
                let faces = (0..3).fold(0, |mask, axis| {
                    if components[(axis + 1) % 3] > 0. && components[(axis + 2) % 3] > 0. {
                        mask | (occluder.faces & (3 << (axis * 2)))
                    } else {
                        mask
                    }
                });
                (transformed.min.x <= transformed.max.x
                    && transformed.min.y <= transformed.max.y
                    && transformed.min.z <= transformed.max.z
                    && faces != 0)
                    .then_some(transformed)
                    .filter(|b| Aabb::new(b.min, b.max).is_some())
                    .map(|bounds| crate::Occluder { bounds, faces })
            })
            .collect::<Vec<_>>();
        self.occluders.insert(
            id,
            (
                node.cull_by_origin
                    .then(|| world.transform_point(Vec3::ZERO)),
                bounds.into(),
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn planar_occluders_survive_preparation_but_rotated_bounds_do_not() {
        let mut scene = Scene::new();
        let mut node = Node::new().with_mesh(Arc::new(Mesh::cube()));
        node.occluders = Arc::from([crate::Occluder {
            bounds: Aabb::new(Vec3::new(-0.5, 0.5, -0.5), Vec3::new(0.5, 0.5, 0.5)).unwrap(),
            faces: 8,
        }]);
        let id = scene.insert(None, node).unwrap();
        let mut prepared = PreparedScene::default();
        let camera = Camera::perspective(Vec3::new(0., 2., 3.), Vec3::ZERO, Vec3::Y, 1., 0.1, 100.);
        prepared.update(&scene, camera, 1.).unwrap();
        assert_eq!(prepared.occluder_bounds(None).count(), 1);
        scene.node_mut(id).unwrap().transform.rotation =
            crate::Quat::from_axis_angle(Vec3::Y, 0.5).unwrap();
        prepared.update(&scene, camera, 1.).unwrap();
        assert_eq!(prepared.occluder_bounds(None).count(), 0);
    }
}
