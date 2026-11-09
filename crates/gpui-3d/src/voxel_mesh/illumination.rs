//! Immutable cell light fields shared by compute rendering and CPU fallback.
use super::*;

#[derive(Debug, PartialEq)]
pub(crate) struct Lighting {
    pub(crate) levels: Box<[u8]>,
    pub(crate) ambient: f32,
    /// Entire padded grid has identical sky/block light values, so GPU face
    /// merging cannot erase a per-voxel light gradient.
    pub(crate) uniform: bool,
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
        assert!(crate::voxel_mesh::tests::source()
            .with_lighting(vec![7; 36], 0.2).unwrap().lighting.as_ref().unwrap().uniform);
        let mut gradient = vec![7; 36];
        gradient[18] = 6;
        assert!(!crate::voxel_mesh::tests::source()
            .with_lighting(gradient, 0.2).unwrap().lighting.as_ref().unwrap().uniform);
        assert!(!mesh(7).same_voxel_quad_geometry(&mesh(0)));
    }
}

impl VoxelMeshSource {
    pub(super) fn static_light(&self, face: &VoxelFaceTemplate) -> f32 {
        let Some(light) = &self.lighting else {
            return 1.;
        };
        let center = face
            .vertices
            .iter()
            .fold(Vec3::ZERO, |sum, v| sum + v.position)
            * 0.25
            - self.origin;
        let normal = Vec3::new(
            face.neighbour[0] as f32,
            face.neighbour[1] as f32,
            face.neighbour[2] as f32,
        ) * 0.001;
        let level = [center - normal, center + normal]
            .into_iter()
            .filter_map(|p| {
                let q = [p.x.floor() as i32, p.y.floor() as i32, p.z.floor() as i32];
                if q.into_iter()
                    .zip(self.size)
                    .any(|(p, n)| p < 0 || p >= n as i32)
                {
                    return None;
                }
                light
                    .levels
                    .get(
                        ((q[2] as u32 * self.size[1] + q[1] as u32) * self.size[0] + q[0] as u32)
                            as usize,
                    )
                    .copied()
            })
            .map(|n| (n & 15).max(n >> 4))
            .max()
            .unwrap_or(0);
        light.ambient + (1. - light.ambient) * (f32::from(level) / 15.).powi(2)
    }
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
        let uniform = levels.first().is_some_and(|first| levels.iter().all(|value| value == first));
        self.lighting = Some(Lighting {
            levels: levels.into_boxed_slice(),
            ambient,
            uniform,
        });
        self.fallback = OnceLock::new();
        Ok(self)
    }

    pub(super) fn face_light(
        &self,
        face: &VoxelFaceTemplate,
        position: [u32; 3],
        height: f32,
    ) -> [f32; 4] {
        let Some(light) = &self.lighting else {
            return [1.; 4];
        };
        // Smooth lighting: average the four front-plane cells around each corner so
        // adjacent faces interpolate instead of stepping one level per block. This
        // mirrors `corner_cell_light` in the voxel vertex shader.
        let normal = face.neighbour;
        let cell = [
            i64::from(position[0]),
            i64::from(position[1]),
            i64::from(position[2]),
        ];
        std::array::from_fn(|corner| {
            let vertex = face.vertices[corner];
            let local = [
                vertex.position.x,
                vertex.position.y * height,
                vertex.position.z,
            ];
            let tangent = |axis: usize| {
                let mut step = [0i32; 3];
                step[axis] = if local[axis] >= 0.5 { 1 } else { -1 };
                step
            };
            let (a, b) = match normal {
                [1 | -1, 0, 0] => (tangent(1), tangent(2)),
                [0, 1 | -1, 0] => (tangent(0), tangent(2)),
                _ => (tangent(0), tangent(1)),
            };
            let offsets = [
                normal,
                [normal[0] + a[0], normal[1] + a[1], normal[2] + a[2]],
                [normal[0] + b[0], normal[1] + b[1], normal[2] + b[2]],
                [
                    normal[0] + a[0] + b[0],
                    normal[1] + a[1] + b[1],
                    normal[2] + a[2] + b[2],
                ],
            ];
            let mut block = 0.;
            let mut sky = 0.;
            let mut count = 0.;
            for offset in offsets {
                let p = [
                    cell[0] + i64::from(offset[0]),
                    cell[1] + i64::from(offset[1]),
                    cell[2] + i64::from(offset[2]),
                ];
                if (0..3).any(|axis| p[axis] < 0 || p[axis] >= i64::from(self.size[axis])) {
                    continue;
                }
                let packed = light.levels[((p[2] * i64::from(self.size[1]) + p[1])
                    * i64::from(self.size[0])
                    + p[0]) as usize];
                block += f32::from(packed & 15);
                sky += f32::from(packed >> 4);
                count += 1.;
            }
            let value = if count == 0. {
                let own = light.levels[((cell[2] * i64::from(self.size[1]) + cell[1])
                    * i64::from(self.size[0])
                    + cell[0]) as usize];
                f32::from((own & 15).max(own >> 4)) / 15.
            } else {
                (block / count).max(sky / count) / 15.
            };
            light.ambient + (1. - light.ambient) * value * value
        })
    }
}

#[cfg(test)]
mod static_tests {
    use super::*;
    #[test]
    fn static_faces_sample_the_same_sky_and_block_levels() {
        let face = super::super::tests::source().templates[0].clone();
        let mut colors = Vec::new();
        for level in [7, 0x70, 0xf0] {
            let source = VoxelMeshSource::new([3; 3], Vec3::ZERO, vec![0; 27], vec![])
                .unwrap()
                .with_static_faces(vec![face.clone()])
                .unwrap()
                .with_lighting(vec![level; 27], 0.2)
                .unwrap();
            colors.push(source.fallback().vertices()[0].color[0]);
        }
        assert_eq!(colors[0], colors[1]);
        assert!(colors[2] > colors[0]);
        assert_eq!(colors[2], 1.);
    }
}
