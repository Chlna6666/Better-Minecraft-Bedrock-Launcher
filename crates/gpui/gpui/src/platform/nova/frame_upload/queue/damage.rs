//! Atlas-space dependencies of retained filter sources; never copies pixel payloads.
use super::*;
use smallvec::SmallVec;

type UploadPageIndex<'a> = FxHashMap<AtlasTextureId, SmallVec<[&'a PendingAtlasUpload; 8]>>;

// Small batches are cheaper to scan directly. Large multi-layer frames
// index the immutable upload snapshot once, without copying pixel data.
const INDEX_UPLOAD_THRESHOLD: usize = 16;

fn index_upload_pages(uploads: &[PendingAtlasUpload]) -> UploadPageIndex<'_> {
    let mut pages = UploadPageIndex::default();
    for upload in uploads {
        pages.entry(upload.texture_id).or_default().push(upload);
    }
    pages
}

impl AtlasUpload<'_> {
    /// Checks the cached prefix dependencies of the shared backdrop source.
    /// Missing tile bookkeeping conservatively invalidates that source.
    pub(in crate::platform::nova) fn touches_source_tiles(
        &self,
        texture_ids: &FxHashSet<AtlasTextureId>,
        tiles: &FxHashMap<AtlasTextureId, Vec<AtlasTile>>,
    ) -> bool {
        if texture_ids.is_empty() {
            return false;
        }
        self.batch().uploads.iter().any(|upload| {
            if !texture_ids.contains(&upload.texture_id) {
                return false;
            }
            let Some(used_tiles) = tiles.get(&upload.texture_id) else {
                return true;
            };
            used_tiles.is_empty()
                || used_tiles.iter().any(|tile| {
                    tile.texture_id != upload.texture_id || atlas_upload_touches_tile(upload, tile)
                })
        })
    }

    /// Checks every isolated source against this frame's immutable writes.
    /// Packed ranges include nested child sources, so affected ancestors are
    /// invalidated too. A sibling on the same atlas page is not a dependency.
    pub(in crate::platform::nova) fn touches_layers(
        &self,
        frame: &FrameUpload,
    ) -> SmallVec<[u32; 4]> {
        let uploads = &self.batch().uploads;
        if uploads.is_empty() {
            return SmallVec::new();
        }
        let ranges = frame.blur_content_ranges();
        let pages = (uploads.len() >= INDEX_UPLOAD_THRESHOLD && ranges.len() > 1)
            .then(|| index_upload_pages(uploads));
        ranges
            .iter()
            .filter_map(|range| {
                let batches = frame.batches.get(range.content_start..range.content_end);
                let touched = batches.is_none_or(|batches| {
                    batches
                        .iter()
                        .any(|batch| batch_uploads_overlap(frame, batch, uploads, pages.as_ref()))
                });
                touched.then_some(range.index)
            })
            .collect()
    }
}

fn batch_uploads_overlap(
    frame: &FrameUpload,
    batch: &UploadedBatch,
    uploads: &[PendingAtlasUpload],
    pages: Option<&UploadPageIndex<'_>>,
) -> bool {
    let (texture_id, first, count, tiles) = match *batch {
        UploadedBatch::MonoSprites {
            texture_id,
            first,
            count,
        } => (texture_id, first, count, &frame.mono_atlas_tiles),
        UploadedBatch::PolySprites {
            texture_id,
            first,
            count,
            ..
        } => (texture_id, first, count, &frame.poly_atlas_tiles),
        _ => return false,
    };
    let tiles = tiles.get(first as usize..(first as usize).saturating_add(count as usize));
    let overlaps = |upload: &PendingAtlasUpload| {
        tiles.is_none_or(|tiles| {
            tiles.iter().any(|tile| {
                tile.texture_id != texture_id || atlas_upload_touches_tile(upload, tile)
            })
        })
    };
    if let Some(pages) = pages {
        return pages
            .get(&texture_id)
            .is_some_and(|writes| writes.iter().any(|upload| overlaps(upload)));
    }
    uploads
        .iter()
        .filter(|upload| upload.texture_id == texture_id)
        .any(overlaps)
}

/// Texture-coordinate overlap, including padding read by bilinear sampling.
pub(super) fn atlas_upload_touches_tile(upload: &PendingAtlasUpload, tile: &AtlasTile) -> bool {
    let bounds = tile.bounds;
    let left = i64::from(bounds.origin.x.0) - i64::from(tile.padding);
    let top = i64::from(bounds.origin.y.0) - i64::from(tile.padding);
    let right =
        i64::from(bounds.origin.x.0) + i64::from(bounds.size.width.0) + i64::from(tile.padding);
    let bottom =
        i64::from(bounds.origin.y.0) + i64::from(bounds.size.height.0) + i64::from(tile.padding);
    if left >= right || top >= bottom {
        return true; // Unknown or degenerate placement requires conservative refresh.
    }
    let upload_left = i64::from(upload.origin.x);
    let upload_top = i64::from(upload.origin.y);
    let upload_right = upload_left + i64::from(upload.size.width());
    let upload_bottom = upload_top + i64::from(upload.size.height());
    upload_left < right && upload_right > left && upload_top < bottom && upload_bottom > top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_sprite_dependencies_use_tiles_and_fail_closed_for_missing_metadata() {
        for monochrome in [false, true] {
            let atlas = NovaAtlas::new();
            atlas.clear_pending_uploads_for_test();
            let texture_id = AtlasTextureId {
                index: 9,
                kind: AtlasTextureKind::Rgba,
            };
            atlas
                .state
                .lock()
                .expect("test atlas mutex")
                .pending_uploads
                .push(PendingAtlasUpload {
                    texture_id,
                    origin: Origin2d { x: 10, y: 10 },
                    size: Extent2d::new(1, 1).expect("test extent"),
                    bytes_per_row: 4,
                    offset: 0,
                    len: 0,
                });
            let batch = AtlasUpload::new(&atlas);
            let mut frame = FrameUpload::default();
            let tiles = if monochrome {
                &mut frame.mono_atlas_tiles
            } else {
                &mut frame.poly_atlas_tiles
            };
            for (number, x) in [10, 50].into_iter().enumerate() {
                tiles.push(AtlasTile {
                    texture_id,
                    tile_id: crate::TileId(number as u32),
                    padding: 1,
                    bounds: crate::bounds(
                        crate::point(DevicePixels(x), DevicePixels(10)),
                        crate::size(DevicePixels(1), DevicePixels(1)),
                    ),
                });
                frame.batches.push(if monochrome {
                    UploadedBatch::MonoSprites {
                        texture_id,
                        first: number as u32,
                        count: 1,
                    }
                } else {
                    UploadedBatch::PolySprites {
                        texture_id,
                        first: number as u32,
                        count: 1,
                        color_only: false,
                    }
                });
                frame.blur_content_ranges_cache.push(ColorContentRange {
                    index: number as u32,
                    depth: 0,
                    content_start: number,
                    content_end: number + 1,
                });
            }
            assert_eq!(batch.touches_layers(&frame).as_slice(), &[0]);
            // The indexed path must agree with the linear oracle, including
            // unrelated atlas pages and multiple writes on the sampled page.
            let mut uploads = batch.batch().uploads.clone();
            for index in 0..32 {
                let mut unrelated = uploads[0];
                unrelated.texture_id.index = 100 + index;
                uploads.push(unrelated);
            }
            uploads.push(uploads[0]);
            let pages = index_upload_pages(&uploads);
            for draw in &frame.batches {
                assert_eq!(
                    batch_uploads_overlap(&frame, draw, &uploads, None),
                    batch_uploads_overlap(&frame, draw, &uploads, Some(&pages)),
                );
            }
            frame.mono_atlas_tiles.clear();
            frame.poly_atlas_tiles.clear();
            assert_eq!(
                batch.touches_layers(&frame).as_slice(),
                &[0, 1],
                "missing sprite bookkeeping must not authorize retained pixels"
            );
        }
    }
}
