//! Conservative corner lighting for full cell-boundary faces.
use super::{VoxelFaceTemplate, VoxelMeshSource};

/// Computes voxel corner AO from full opaque neighboring cells.
///
/// `corners` are cell-relative. Only unit cell-boundary faces are shaded; arbitrary
/// detail geometry is left unchanged. `is_opaque` receives offsets from the emitting
/// cell, including diagonals. Missing neighbors should return false.
#[must_use]
pub fn voxel_ambient_light(
    normal: [i32; 3],
    corners: [[f32; 3]; 4],
    is_opaque: impl Fn([i32; 3]) -> bool,
) -> [f32; 4] {
    let axis = match normal {
        [1 | -1, 0, 0] => 0,
        [0, 1 | -1, 0] => 1,
        [0, 0, 1 | -1] => 2,
        _ => return [1.0; 4],
    };
    let plane = if normal[axis] > 0 { 1.0 } else { 0.0 };
    if corners
        .iter()
        .any(|p| p[axis] != plane || p.iter().any(|&c| c != 0.0 && c != 1.0))
    {
        return [1.0; 4];
    }
    let tangents = match axis {
        0 => [1, 2],
        1 => [0, 2],
        _ => [0, 1],
    };
    corners.map(|p| {
        let mut side_a = normal;
        let mut side_b = normal;
        side_a[tangents[0]] = if p[tangents[0]] == 0.0 { -1 } else { 1 };
        side_b[tangents[1]] = if p[tangents[1]] == 0.0 { -1 } else { 1 };
        let corner = std::array::from_fn(|a| side_a[a] + side_b[a] - normal[a]);
        let a = is_opaque(side_a);
        let b = is_opaque(side_b);
        let occupied = if a && b {
            3
        } else {
            u8::from(a) + u8::from(b) + u8::from(is_opaque(corner))
        };
        1.0 - f32::from(occupied) * 0.2
    })
}

impl VoxelMeshSource {
    /// Enables corner AO for full boundary faces. Zero disables AO (the default).
    /// `opaque_mask` selects opaque class bits for instances; ordinary cube cells
    /// treat nonzero cells as opaque. Static faces retain caller-baked lighting.
    #[must_use]
    pub fn with_ambient_occlusion(mut self, opaque_mask: u32) -> Self {
        self.ambient_occlusion = opaque_mask;
        self.fallback = std::sync::OnceLock::new();
        self
    }

    pub(super) fn face_ambient_light(
        &self,
        face: &VoxelFaceTemplate,
        position: [u32; 3],
        instance: bool,
    ) -> [f32; 4] {
        if self.ambient_occlusion == 0 {
            return [1.0; 4];
        }
        let corners = face
            .vertices
            .map(|v| [v.position.x, v.position.y, v.position.z]);
        voxel_ambient_light(face.neighbour, corners, |offset| {
            let p: [i64; 3] =
                std::array::from_fn(|a| i64::from(position[a]) + i64::from(offset[a]));
            if (0..3).any(|a| p[a] < 0 || p[a] >= i64::from(self.size[a])) {
                return false;
            }
            let index =
                ((p[2] * i64::from(self.size[1]) + p[1]) * i64::from(self.size[0]) + p[0]) as usize;
            if instance {
                self.instances
                    .as_ref()
                    .is_some_and(|data| data.classes.cell(index) & self.ambient_occlusion != 0)
            } else {
                self.cells.cell(index) != 0
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const TOP: [[f32; 3]; 4] = [[0., 1., 0.], [1., 1., 0.], [1., 1., 1.], [0., 1., 1.]];

    #[test]
    fn corners_use_side_and_diagonal_occupancy_without_darkening_alpha() {
        assert_eq!(voxel_ambient_light([0, 1, 0], TOP, |_| false), [1.; 4]);
        let values = voxel_ambient_light([0, 1, 0], TOP, |p| p == [-1, 1, 0] || p == [0, 1, -1]);
        assert!((values[0] - 0.4).abs() < 0.0001);
        assert_eq!(values[2], 1.0);
        assert_eq!(
            voxel_ambient_light([0, 1, 0], [[0.5; 3]; 4], |_| true),
            [1.; 4]
        );
    }
}
