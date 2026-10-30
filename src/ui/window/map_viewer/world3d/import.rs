//! Shared placement preview for imported structures in the independent editor.

use super::super::model::{PasteRotation, PasteTransform};
use super::super::preview_3d::Preview3dMesh;
use bedrock_render::ChunkPos;
use gpui_3d::{Quat, Transform, Vec3};
use std::f32::consts::{FRAC_PI_2, PI};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) enum World3dImportAction {
    StartAt(ChunkPos),
    MoveTo(ChunkPos, i32),
    Nudge(i32, i32),
    Raise(i32),
    Rotate(bool),
    MirrorX,
    MirrorZ,
    Confirm,
    Cancel,
}

#[derive(Clone)]
pub(super) struct World3dImportPreview {
    pub(super) mesh: Arc<Preview3dMesh>,
    pub(super) source_anchor: ChunkPos,
    pub(super) target_anchor: ChunkPos,
    pub(super) size: [i32; 2],
    pub(super) height: i32,
    pub(super) mesh_origin_y: i32,
    pub(super) origin_y: i32,
    pub(super) transform: PasteTransform,
    pub(super) is_structure: bool,
    pub(super) chunk_count: usize,
    pub(super) is_writing: bool,
}

impl World3dImportPreview {
    /// Match mcstructure placement: mirror within the structure, then rotate each
    /// chunk's local 0..15 coordinates around its 16-block boundary.
    pub(super) fn scene_transform(&self, render_origin: [i32; 3]) -> Transform {
        placement_transform(
            self.source_anchor,
            self.target_anchor,
            self.size,
            self.mesh_origin_y,
            self.origin_y,
            self.transform,
            render_origin,
        )
    }
}

fn placement_transform(
    source_anchor: ChunkPos,
    target_anchor: ChunkPos,
    size: [i32; 2],
    mesh_origin_y: i32,
    origin_y: i32,
    transform: PasteTransform,
    render_origin: [i32; 3],
) -> Transform {
    let source = Vec3::new(
        source_anchor.x.saturating_mul(16) as f32,
        0.0,
        source_anchor.z.saturating_mul(16) as f32,
    );
    let mirrored_extent = Vec3::new(
        if transform.mirror_x {
            size[0] as f32
        } else {
            0.0
        },
        0.0,
        if transform.mirror_z {
            size[1] as f32
        } else {
            0.0
        },
    );
    let scale = Vec3::new(
        if transform.mirror_x { -1.0 } else { 1.0 },
        1.0,
        if transform.mirror_z { -1.0 } else { 1.0 },
    );
    let (angle, local_offset) = match transform.rotation {
        PasteRotation::NoRotation => (0.0, Vec3::ZERO),
        PasteRotation::Clockwise90 => (-FRAC_PI_2, Vec3::new(16.0, 0.0, 0.0)),
        PasteRotation::Rotate180 => (PI, Vec3::new(16.0, 0.0, 16.0)),
        PasteRotation::CounterClockwise90 => (FRAC_PI_2, Vec3::new(0.0, 0.0, 16.0)),
    };
    let rotation = Quat::from_axis_angle(Vec3::Y, angle).unwrap_or(Quat::IDENTITY);
    let offset = rotation.rotate(Vec3::new(
        mirrored_extent.x - source.x * scale.x,
        0.0,
        mirrored_extent.z - source.z * scale.z,
    ));
    Transform {
        translation: Vec3::new(
            target_anchor
                .x
                .saturating_mul(16)
                .saturating_sub(render_origin[0]) as f32,
            origin_y
                .saturating_sub(mesh_origin_y)
                .saturating_sub(render_origin[1]) as f32,
            target_anchor
                .z
                .saturating_mul(16)
                .saturating_sub(render_origin[2]) as f32,
        ) + local_offset
            + offset,
        rotation,
        scale,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_render::Dimension;

    #[test]
    fn preview_centers_match_structure_and_chunk_placement() {
        let source = ChunkPos {
            x: -3,
            z: 5,
            dimension: Dimension::Overworld,
        };
        let target = ChunkPos {
            x: 7,
            z: -2,
            dimension: Dimension::Overworld,
        };
        let render_origin = [64, 0, -64];
        let cases: [([i32; 2], &[[i32; 2]], i32, i32); 2] = [
            ([25, 19], &[[0, 0], [24, 18], [16, 8], [3, 17]], 32, 70),
            ([16, 16], &[[-16, 0], [0, 0], [15, 15], [24, -8]], 0, 0),
        ];
        for (size, samples, mesh_y, origin_y) in cases {
            for rotation in [
                PasteRotation::NoRotation,
                PasteRotation::Clockwise90,
                PasteRotation::Rotate180,
                PasteRotation::CounterClockwise90,
            ] {
                for mirror_x in [false, true] {
                    for mirror_z in [false, true] {
                        let transform = PasteTransform {
                            rotation,
                            mirror_x,
                            mirror_z,
                        };
                        let matrix = placement_transform(
                            source,
                            target,
                            size,
                            mesh_y,
                            origin_y,
                            transform,
                            render_origin,
                        )
                        .matrix();
                        for &[x, z] in samples {
                            let mirrored_x = if mirror_x { size[0] - 1 - x } else { x };
                            let mirrored_z = if mirror_z { size[1] - 1 - z } else { z };
                            let (chunk_x, chunk_z) = rotation
                                .rotate_delta(mirrored_x.div_euclid(16), mirrored_z.div_euclid(16));
                            let (local_x, local_z) = match rotation {
                                PasteRotation::NoRotation => {
                                    (mirrored_x.rem_euclid(16), mirrored_z.rem_euclid(16))
                                }
                                PasteRotation::Clockwise90 => {
                                    (15 - mirrored_z.rem_euclid(16), mirrored_x.rem_euclid(16))
                                }
                                PasteRotation::Rotate180 => (
                                    15 - mirrored_x.rem_euclid(16),
                                    15 - mirrored_z.rem_euclid(16),
                                ),
                                PasteRotation::CounterClockwise90 => {
                                    (mirrored_z.rem_euclid(16), 15 - mirrored_x.rem_euclid(16))
                                }
                            };
                            let rendered = matrix.transform_point(Vec3::new(
                                (source.x * 16 + x) as f32 + 0.5,
                                mesh_y as f32 + 0.5,
                                (source.z * 16 + z) as f32 + 0.5,
                            ));
                            let expected = Vec3::new(
                                ((target.x + chunk_x) * 16 + local_x - render_origin[0]) as f32
                                    + 0.5,
                                origin_y as f32 + 0.5,
                                ((target.z + chunk_z) * 16 + local_z - render_origin[2]) as f32
                                    + 0.5,
                            );
                            assert!(
                                (rendered.x - expected.x).abs() < 0.001,
                                "{transform:?} x={x} z={z}"
                            );
                            assert!(
                                (rendered.y - expected.y).abs() < 0.001,
                                "{transform:?} x={x} z={z}"
                            );
                            assert!(
                                (rendered.z - expected.z).abs() < 0.001,
                                "{transform:?} x={x} z={z}"
                            );
                        }
                    }
                }
            }
        }
    }
}
