//! One frame owns the exact writes used for both damage planning and upload.
use super::*;

/// Owns one frame's queued atlas writes through damage planning and native
/// upload. Dropping before consumption restores the batch in producer order.
pub(in crate::platform::nova) struct AtlasUpload<'a> {
    atlas: &'a NovaAtlas,
    batch: Option<AtlasUploadBatch>,
}

impl<'a> AtlasUpload<'a> {
    /// Move queued pixels and their metadata under one lock. Later producer
    /// writes remain queued for the next frame; dropping unsubmitted work
    /// restores this batch before those newer writes.
    pub(in crate::platform::nova) fn new(atlas: &'a NovaAtlas) -> Self {
        Self {
            atlas,
            batch: Some(atlas.take_pending_uploads()),
        }
    }

    pub(super) fn batch(&self) -> &AtlasUploadBatch {
        self.batch
            .as_ref()
            .expect("atlas upload is owned until consumed")
    }

    pub(in crate::platform::nova) fn content_generation(&self) -> u64 {
        self.batch().content_generation
    }

    pub(in crate::platform::nova) fn texture_infos(&self) -> &[NovaAtlasTextureInfo] {
        self.batch().texture_infos()
    }

    /// Consume the planned batch once. The existing upload path restores all
    /// failed writes; success recycles its staging allocation without cloning.
    ///
    /// # Errors
    /// Propagates invalid pixel ranges, texture resolution and backend upload
    /// failures. Partial native writes are retried before newer producer writes.
    pub(in crate::platform::nova) fn upload(
        mut self,
        resolve_texture: impl FnMut(AtlasTextureId) -> Result<TextureId>,
        upload: impl FnMut(&[TextureWrite<'_>]) -> Result<()>,
    ) -> Result<AtlasUploadStats> {
        let batch = self.batch.take().expect("atlas upload is consumed once");
        self.atlas
            .upload_taken_rgba_pixels(batch, resolve_texture, upload)
    }
}

impl Drop for AtlasUpload<'_> {
    fn drop(&mut self) {
        if let Some(batch) = self.batch.take() {
            self.atlas.restore_upload_batch(batch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(atlas: &NovaAtlas, value: u8, x: i32) {
        assert!(
            atlas
                .state
                .lock()
                .expect("test atlas mutex")
                .enqueue_tile_upload(
                    AtlasTextureId {
                        index: 0,
                        kind: AtlasTextureKind::Rgba
                    },
                    AtlasTextureKind::Rgba,
                    crate::point(DevicePixels(x), DevicePixels(10)),
                    crate::size(DevicePixels(1), DevicePixels(1)),
                    &[value; 4],
                    0,
                )
        );
    }

    fn upload_values(batch: AtlasUpload<'_>) -> Vec<u8> {
        let mut values = Vec::new();
        batch
            .upload(
                |_| Ok(TextureId::from_parts(0, 0)),
                |writes| {
                    values.extend(writes.iter().map(|write| write.data[0]));
                    Ok(())
                },
            )
            .expect("upload test snapshot");
        values
    }

    #[test]
    fn adopted_pages_preserve_allocation_and_retry_order() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        let texture = AtlasTextureId { index: 0, kind: AtlasTextureKind::Rgba };
        let page = vec![7; 36];
        let pointer = page.as_ptr();
        {
            let mut state = atlas.state.lock().expect("atlas");
            let index = state.upload_bytes.adopt(page);
            assert!(state.enqueue_encoded_tile_upload(
                texture, crate::point(DevicePixels(10), DevicePixels(10)),
                crate::size(DevicePixels(1), DevicePixels(1)), index, 0..36,
            ));
        }
        let batch = AtlasUpload::new(&atlas);
        queue(&atlas, 9, 10);
        assert!(batch.upload(
            |_| Ok(TextureId::from_parts(0, 0)),
            |writes| {
                assert_eq!(writes[0].data.as_ptr(), pointer);
                anyhow::bail!("retry")
            },
        ).is_err());
        let restored = AtlasUpload::new(&atlas);
        restored.upload(
            |_| Ok(TextureId::from_parts(0, 0)),
            |writes| {
                assert_eq!(writes.len(), 2);
                assert_eq!(writes[0].data.as_ptr(), pointer);
                assert_eq!(writes[0].data, &[7; 36]);
                assert_eq!(writes[1].data, &[9; 4]);
                Ok(())
            },
        ).expect("retry retains page ownership");
    }

    #[test]
    fn adjacent_rows_share_one_native_write_without_copying() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        let texture = AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Rgba,
        };
        {
            let mut state = atlas.state.lock().expect("test atlas mutex");
            for (y, value) in [(10, 7), (12, 9)] {
                assert!(state.enqueue_tile_upload(
                    texture,
                    AtlasTextureKind::Rgba,
                    crate::point(DevicePixels(5), DevicePixels(y)),
                    crate::size(DevicePixels(3), DevicePixels(2)),
                    &[value; 24],
                    0,
                ));
            }
        }
        let batch = AtlasUpload::new(&atlas);
        let original_ptr = batch.batch().bytes.pages[0].as_ptr();
        let stats = batch
            .upload(
                |_| Ok(TextureId::from_parts(0, 0)),
                |writes| {
                    assert_eq!(writes.len(), 1);
                    assert_eq!(writes[0].descriptor.origin, Origin2d { x: 5, y: 10 });
                    assert_eq!(writes[0].descriptor.size, Extent2d::new(3, 4).unwrap());
                    assert_eq!(writes[0].data.as_ptr(), original_ptr);
                    assert_eq!(&writes[0].data[..24], &[7; 24]);
                    assert_eq!(&writes[0].data[24..], &[9; 24]);
                    Ok(())
                },
            )
            .expect("merged atlas upload");
        assert_eq!(stats.upload_count, 1);
        assert_eq!(stats.uploaded_bytes, 48);
    }

    #[test]
    fn adjacent_single_row_uploads_merge_but_gaps_do_not() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        queue(&atlas, 1, 10);
        queue(&atlas, 2, 11);
        queue(&atlas, 3, 13);
        let stats = AtlasUpload::new(&atlas)
            .upload(
                |_| Ok(TextureId::from_parts(0, 0)),
                |writes| {
                    assert_eq!(writes.len(), 2);
                    assert_eq!(writes[0].data, &[1, 1, 1, 1, 2, 2, 2, 2]);
                    assert_eq!(writes[1].data, &[3; 4]);
                    Ok(())
                },
            )
            .expect("merged atlas row");
        assert_eq!(stats.upload_count, 2);
        assert_eq!(stats.uploaded_bytes, 12);
    }

    #[test]
    fn failed_merged_upload_restores_original_descriptors() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        queue(&atlas, 1, 10);
        queue(&atlas, 2, 11);
        let batch = AtlasUpload::new(&atlas);
        queue(&atlas, 3, 10);
        assert!(batch.upload(
            |_| Ok(TextureId::from_parts(0, 0)),
            |writes| {
                assert_eq!(writes.len(), 1);
                anyhow::bail!("injected upload failure")
            },
        ).is_err());
        assert_eq!(atlas.pending_upload_count_for_test(), 3);
        AtlasUpload::new(&atlas).upload(
            |_| Ok(TextureId::from_parts(0, 0)),
            |writes| {
                assert_eq!(writes.len(), 2);
                assert_eq!(writes[0].data, &[1, 1, 1, 1, 2, 2, 2, 2]);
                assert_eq!(writes[1].data, &[3; 4]);
                Ok(())
            },
        ).expect("retry preserves newer writes");
    }

    #[test]
    fn snapshot_upload_excludes_later_writes_and_their_generation() {
        let atlas = NovaAtlas::new();
        atlas.clear_pending_uploads_for_test();
        queue(&atlas, 1, 10);
        let batch = AtlasUpload::new(&atlas);
        let generation = batch.content_generation();
        let texture_id = AtlasTextureId {
            index: 0,
            kind: AtlasTextureKind::Rgba,
        };
        let sources = FxHashSet::from_iter([texture_id]);
        let tiles = FxHashMap::from_iter([(
            texture_id,
            vec![AtlasTile {
                texture_id,
                tile_id: crate::TileId(1),
                padding: 0,
                bounds: crate::bounds(
                    crate::point(DevicePixels(50), DevicePixels(10)),
                    crate::size(DevicePixels(1), DevicePixels(1)),
                ),
            }],
        )]);
        assert!(!batch.touches_source_tiles(&sources, &tiles));
        queue(&atlas, 2, 50);
        assert!(!batch.touches_source_tiles(&sources, &tiles));
        assert_eq!(batch.content_generation(), generation);
        assert_eq!(upload_values(batch), [1]);
        let next = AtlasUpload::new(&atlas);
        assert_ne!(next.content_generation(), generation);
        assert!(next.touches_source_tiles(&sources, &tiles));
        assert_eq!(upload_values(next), [2]);
    }

    #[test]
    fn abandoned_and_failed_uploads_restore_before_newer_writes() {
        for failed_upload in [false, true] {
            let atlas = NovaAtlas::new();
            atlas.clear_pending_uploads_for_test();
            queue(&atlas, 1, 10);
            let batch = AtlasUpload::new(&atlas);
            queue(&atlas, 2, 10);
            if failed_upload {
                assert!(
                    batch
                        .upload(
                            |_| Ok(TextureId::from_parts(0, 0)),
                            |_| anyhow::bail!("injected upload failure"),
                        )
                        .is_err()
                );
            } else {
                drop(batch);
            }
            assert_eq!(
                upload_values(AtlasUpload::new(&atlas)),
                [1, 2],
                "retry must finish with the newest producer value"
            );
        }
    }
}
