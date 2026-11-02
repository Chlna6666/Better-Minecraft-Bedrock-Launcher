//! Corner lighting excludes cutout volumes from the face-culling occupancy index.
use super::*;

impl Preview3dChunkBlocks {
    pub(super) fn refresh_ambient_exclusions(&mut self) {
        let mut materials = HashMap::default();
        // Keep exclusions for records removed by interior-face compaction: their
        // occupied cells remain in the culling index until the chunk is replaced.
        self.ao_exclusions
            .extend(self.opaque_blocks.iter().filter_map(|block| {
                let opaque = materials.entry(block.material.as_ref()).or_insert_with(|| {
                    let name = bedrock_block_model::obj_material_texture_name(&block.material);
                    bedrock_block_model::is_full_opaque_block(&name)
                });
                (!*opaque).then_some(block.key)
            }));
    }
}

impl Preview3dMeshBuilder {
    pub(in crate::ui::window::map_viewer) fn set_ambient_occlusion(&mut self, enabled: bool) {
        self.ambient_occlusion = enabled;
    }
    pub(super) fn stream_ambient_light(&self, block: BlockKey, normal: [i32; 3]) -> [f32; 4] {
        if !self.ambient_occlusion {
            return [1.; 4];
        }
        gpui_3d::voxel_ambient_light(normal, preview_3d_stream_face_corners(normal), |offset| {
            let neighbor = BlockKey {
                x: block.x + offset[0],
                y: block.y + offset[1],
                z: block.z + offset[2],
            };
            self.ambient_occludes_at(neighbor)
        })
    }

    pub(super) fn ambient_occludes_at(&self, block: BlockKey) -> bool {
        let key = ChunkKey::from_block(block);
        if let Some(chunk) = self.block_chunks.get(&key) {
            return chunk.occupied.contains(&block) && !chunk.ao_exclusions.contains(&block);
        }
        self.boundary_chunks.get(&key).is_some_and(|chunk| {
            let index = (block.y.rem_euclid(16) * 256
                + block.z.rem_euclid(16) * 16
                + block.x.rem_euclid(16)) as usize;
            chunk
                .classes
                .get(&block.y.div_euclid(16))
                .is_some_and(|cells| cells[index / 2] & (8 << ((index % 2) * 4)) != 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corner_ao_reads_diagonal_chunk_boundaries_and_excludes_leaves() {
        let pos = |x, z| ChunkPos {
            x,
            z,
            dimension: bedrock_render::Dimension::Overworld,
        };
        let bounds = SlimeChunkBounds {
            dimension: bedrock_render::Dimension::Overworld,
            min_chunk_x: 0,
            max_chunk_x: 1,
            min_chunk_z: 0,
            max_chunk_z: 1,
        };
        let mut builder = Preview3dMeshBuilder::new_stream(bounds);
        let target = BlockKey {
            x: 15,
            y: 63,
            z: 15,
        };
        let mut neighbor = Preview3dChunkBlocks::test_cubes(&[[16, 64, 16]]);
        builder.insert_stream_boundary(pos(1, 1), Arc::new(neighbor.boundary_snapshot()));
        assert!((builder.stream_ambient_light(target, [0, 1, 0])[2] - 0.8).abs() < 0.001);
        neighbor.opaque_blocks[0].material = Arc::from("minecraft_oak_leaves");
        builder.insert_stream_boundary(pos(1, 1), Arc::new(neighbor.boundary_snapshot()));
        assert_eq!(builder.stream_ambient_light(target, [0, 1, 0]), [1.0; 4]);
        builder.insert_stream_chunk(pos(1, 1), neighbor);
        assert_eq!(builder.stream_ambient_light(target, [0, 1, 0]), [1.0; 4]);
    }
}
