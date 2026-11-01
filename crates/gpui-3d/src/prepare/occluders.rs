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
                (transformed.min.x < transformed.max.x
                    && transformed.min.y < transformed.max.y
                    && transformed.min.z < transformed.max.z)
                    .then_some(transformed)
                    .filter(|b| Aabb::new(b.min, b.max).is_some())
                    .map(|bounds| crate::Occluder {
                        bounds,
                        faces: occluder.faces & 63,
                    })
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
