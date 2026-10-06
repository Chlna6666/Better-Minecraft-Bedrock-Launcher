use super::*;
use std::hash::Hasher;
use std::sync::Arc;

impl FrameUpload {
    pub(super) fn encode_paths(
        &mut self,
        paths: &[crate::Path<crate::ScaledPixels>],
        summary: &mut FrameUploadSummary,
    ) {
        self.encode_path_vertices(paths, summary);
        self.encode_path_sprites(paths, summary);
    }

    fn encode_path_vertices(
        &mut self,
        paths: &[crate::Path<crate::ScaledPixels>],
        summary: &mut FrameUploadSummary,
    ) {
        let first_vertex = (self.path_rasterization_vertices.len()
            / PACKED_PATH_RASTERIZATION_VERTEX_BYTES) as u32;
        let mut vertex_count = 0_u32;
        for path in paths {
            let Some(encoded) = self.encoded_path_rasterization(path) else {
                continue;
            };
            let remaining_vertices = MAX_PATH_VERTICES.saturating_sub(
                self.path_rasterization_vertices.len() / PACKED_PATH_RASTERIZATION_VERTEX_BYTES,
            );
            let encoded_vertex_count = encoded.vertex_count as usize;
            if encoded_vertex_count > remaining_vertices {
                break;
            }
            self.path_rasterization_vertices
                .extend_from_slice(&encoded.bytes);
            vertex_count = vertex_count.saturating_add(encoded.vertex_count);
        }
        if vertex_count > 0 {
            self.batches.push(UploadedBatch::PathRasterization {
                first_vertex,
                vertex_count,
            });
            summary.path_vertex_count = summary.path_vertex_count.saturating_add(vertex_count);
        }
    }

    fn encode_path_sprites(
        &mut self,
        paths: &[crate::Path<crate::ScaledPixels>],
        summary: &mut FrameUploadSummary,
    ) {
        let Some(first_path) = paths.first() else {
            return;
        };
        let first = (self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES) as u32;
        let mut count = 0_u32;
        if paths
            .last()
            .is_some_and(|path| path.order == first_path.order)
        {
            for path in paths {
                if self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES >= MAX_PATH_SPRITES {
                    break;
                }
                write_path_sprite(&mut self.path_sprites, &path.clipped_bounds());
                count = count.saturating_add(1);
            }
        } else {
            let mut bounds = first_path.clipped_bounds();
            for path in paths.iter().skip(1) {
                bounds = bounds.union(&path.clipped_bounds());
            }
            if self.path_sprites.len() / PACKED_PATH_SPRITE_BYTES < MAX_PATH_SPRITES {
                write_path_sprite(&mut self.path_sprites, &bounds);
                count = 1;
            }
        }
        if count > 0 {
            self.batches.push(UploadedBatch::Paths { first, count });
            summary.path_sprite_count = summary.path_sprite_count.saturating_add(count);
        }
    }

    fn encoded_path_rasterization(
        &mut self,
        path: &crate::Path<crate::ScaledPixels>,
    ) -> Option<PathRasterizationCacheEntry> {
        let vertex_count = u32::try_from(path.vertices.len()).ok()?;
        if vertex_count == 0 {
            return None;
        }

        let clipped_bounds = path.clipped_bounds();
        let content_mask = crate::ContentMask {
            bounds: clipped_bounds,
            corner_bounds: path.content_mask.corner_bounds,
            corner_radii: if clipped_bounds == path.content_mask.bounds {
                path.content_mask.corner_radii
            } else {
                Default::default()
            },
        };
        self.path_paint_key_scratch.clear();
        write_content_mask(&mut self.path_paint_key_scratch, &content_mask);
        write_background(&mut self.path_paint_key_scratch, &path.color);
        let paint_hash = fnv1a_bytes(&self.path_paint_key_scratch);

        let key = PathRasterizationCacheKey {
            path_id: path.cache_id,
            generation: path.geometry_generation,
            vertex_count: path.vertices.len(),
            geometry_hash: self.path_geometry_hash_for(path),
            paint_hash,
        };
        if let Some(entry) = self.path_rasterization_cache.get(&key) {
            self.path_rasterization_cache_hits =
                self.path_rasterization_cache_hits.saturating_add(1);
            return Some(entry.clone());
        }

        let encoded_bytes = path
            .vertices
            .len()
            .saturating_mul(PACKED_PATH_RASTERIZATION_VERTEX_BYTES);
        self.path_rasterization_encode_scratch.clear();
        if self.path_rasterization_encode_scratch.capacity() < encoded_bytes {
            self.path_rasterization_encode_scratch
                .reserve(encoded_bytes);
        }
        for vertex in &path.vertices {
            write_path_rasterization_vertex(
                &mut self.path_rasterization_encode_scratch,
                vertex,
                &path.color,
                &content_mask,
            );
        }
        debug_assert_eq!(self.path_rasterization_encode_scratch.len(), encoded_bytes);
        let entry = PathRasterizationCacheEntry {
            bytes: Arc::<[u8]>::from(self.path_rasterization_encode_scratch.as_slice()),
            vertex_count,
        };
        self.path_rasterization_encode_scratch.clear();
        self.path_rasterization_cache.insert(key, entry.clone());
        self.path_rasterization_cache_misses =
            self.path_rasterization_cache_misses.saturating_add(1);
        Some(entry)
    }

    fn path_geometry_hash_for(&mut self, path: &crate::Path<crate::ScaledPixels>) -> u64 {
        let (Some(first), Some(last)) = (path.vertices.first(), path.vertices.last()) else {
            return path_geometry_hash(&path.vertices);
        };
        let first_xy_bits = (
            first.xy_position.x.0.to_bits(),
            first.xy_position.y.0.to_bits(),
        );
        let last_xy_bits = (
            last.xy_position.x.0.to_bits(),
            last.xy_position.y.0.to_bits(),
        );
        if let Some(memo) = self.path_geometry_hash_memo.get(&path.cache_id)
            && memo.generation == path.geometry_generation
            && memo.vertex_count == path.vertices.len()
            && memo.first_xy_bits == first_xy_bits
            && memo.last_xy_bits == last_xy_bits
        {
            return memo.geometry_hash;
        }
        let geometry_hash = path_geometry_hash(&path.vertices);
        self.path_geometry_hash_memo.insert(
            path.cache_id,
            PathGeometryHashMemo {
                generation: path.geometry_generation,
                vertex_count: path.vertices.len(),
                first_xy_bits,
                last_xy_bits,
                geometry_hash,
            },
        );
        geometry_hash
    }
}

fn path_geometry_hash(vertices: &[crate::PathVertex_ScaledPixels]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for vertex in vertices {
        hash = fnv1a_u32(hash, vertex.xy_position.x.0.to_bits());
        hash = fnv1a_u32(hash, vertex.xy_position.y.0.to_bits());
        hash = fnv1a_u32(hash, vertex.st_position.x.to_bits());
        hash = fnv1a_u32(hash, vertex.st_position.y.to_bits());
    }
    hash
}

fn fnv1a_u32(hash: u64, value: u32) -> u64 {
    let hash = hash ^ u64::from(value);
    hash.wrapping_mul(0x0000_0100_0000_01b3)
}

fn fnv1a_bytes(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
