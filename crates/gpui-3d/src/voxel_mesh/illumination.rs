//! Immutable cell light fields shared by compute rendering and CPU fallback.
use super::*;

#[derive(Debug, PartialEq)]
pub(crate) struct Lighting {
    pub(crate) levels: Box<[u8]>,
    pub(crate) ambient: f32,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn immutable_light_fields_match_cpu_sky_and_block_channels() {
        for level in [0, 1, 7, 15] {
            for shift in [0, 4] {
                let source = crate::voxel_mesh::tests::source();
                let source = source.with_lighting(vec![level << shift; 36], 0.2).unwrap();
                let expected = 0.2 + 0.8 * (f32::from(level) / 15.).powi(2);
                let expected = (expected * 255.).round() / 255.;
                assert!((source.fallback().vertices()[0].color[0] - expected).abs() < 0.00001);
            }
        }
        assert!(
            crate::voxel_mesh::tests::source()
                .with_lighting(vec![0; 35], 0.)
                .is_err()
        );
        assert!(
            crate::voxel_mesh::tests::source()
                .with_lighting(vec![0; 36], f32::NAN)
                .is_err()
        );
        let mesh = |level| {
            Mesh::from_gpu_voxels(
                crate::voxel_mesh::tests::source()
                    .with_lighting(vec![level; 36], 0.2)
                    .unwrap(),
            )
        };
        assert!(mesh(7).same_voxel_quad_geometry(&mesh(7)));
        assert!(!mesh(7).same_voxel_quad_geometry(&mesh(0)));
    }
}

impl VoxelMeshSource {
    /// Attaches sky (high nibble) and block (low nibble) light for the padded cell grid.
    /// Ambient is the minimum brightness in 0..=1; this is independent of corner AO.
    /// # Errors
    /// Rejects mismatched fields or invalid ambient values.
    pub fn with_lighting(mut self, levels: Vec<u8>, ambient: f32) -> Result<Self, MeshError> {
        if levels.len() != self.size.into_iter().map(|n| n as usize).product::<usize>()
            || !ambient.is_finite()
            || !(0.0..=1.0).contains(&ambient)
        {
            return Err(MeshError::InvalidParts);
        }
        self.lighting = Some(Lighting {
            levels: levels.into_boxed_slice(),
            ambient,
        });
        self.fallback = OnceLock::new();
        Ok(self)
    }

    pub(super) fn face_light(&self, face: &VoxelFaceTemplate, position: [u32; 3]) -> f32 {
        let Some(light) = &self.lighting else {
            return 1.;
        };
        let index =
            ((position[2] * self.size[1] + position[1]) * self.size[0] + position[0]) as usize;
        let own = light.levels[index];
        let neighbor = i64::try_from(index).unwrap_or(0) + i64::from(self.neighbour_delta(face));
        let next = usize::try_from(neighbor)
            .ok()
            .and_then(|i| light.levels.get(i))
            .copied()
            .unwrap_or(0);
        let level = (own & 15).max(own >> 4).max(next & 15).max(next >> 4);
        let value = f32::from(level) / 15.;
        light.ambient + (1. - light.ambient) * value * value
    }
}
