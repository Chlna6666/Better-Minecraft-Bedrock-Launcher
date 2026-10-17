use super::*;
use std::hash::Hasher;

/// Closed, ordered static prefix proven during admission; counts exclude clipped instances.
pub(in crate::platform::nova) struct StaticPrefix {
    pub(in crate::platform::nova) bounds: crate::Bounds<crate::ScaledPixels>,
    pub(in crate::platform::nova) content_mask: crate::ContentMask<crate::ScaledPixels>,
    pub(in crate::platform::nova) quads: usize,
    pub(in crate::platform::nova) shadows: usize,
}

impl FrameUpload {
    /// Wrap the existing packed static prefix in a zero-radius GPU color
    /// target, without replaying or re-encoding an independent foreground Scene.
    ///
    /// The original Scene is finalized: Quad/Shadow streams are authoritative.
    /// FilterRegistry compares this source's packed content fingerprint; no
    /// Background or Foreground Scene is retained for automatic capture.
    /// Reject any unsorted or unsupported batch topology before mutation.
    pub(in crate::platform::nova) fn capture_encoded_static_prefix(
        &mut self,
        prefix: &StaticPrefix,
        drawable_size: DrawableSize,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        self.capture_encoded_static_prefix_for_target_size(
            prefix,
            drawable_size,
            summary,
        )
    }

    pub(in crate::platform::nova) fn capture_encoded_static_prefix_for_target_size(
        &mut self,
        prefix: &StaticPrefix,
        target_size: DrawableSize,
        summary: &mut FrameUploadSummary,
    ) -> bool {
        if self.root_color.is_some()
            || self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES >= MAX_BACKDROP_BLURS
            || self.batches.is_empty()
        {
            return false;
        }

        // Consume the admission pass's visible counts; never rescan Scene arrays.
        let StaticPrefix {
            bounds,
            content_mask,
            quads: static_quads,
            shadows: static_shadows,
        } = *prefix;
        if static_quads + static_shadows == 0 {
            return false;
        }

        // Find the first dynamic packed instance; a mixed Quad/Shadow batch
        // may need ONE split. All later batches must contain no static part.
        let mut boundary = None::<(usize, usize)>;
        let mut saw_static = false;
        let mut saw_dynamic = false;
        for (index, batch) in self.batches.iter().enumerate() {
            let (first, count, limit) = match *batch {
                UploadedBatch::Quads { first, count }
                | UploadedBatch::SolidQuads { first, count } => {
                    (first as usize, count as usize, static_quads)
                }
                UploadedBatch::Shadows { first, count } => {
                    (first as usize, count as usize, static_shadows)
                }
                _ => (0, 1, 0),
            };
            let static_count = limit.saturating_sub(first).min(count);
            if static_count != 0 && saw_dynamic {
                return false;
            }
            saw_static |= static_count != 0;
            if static_count < count {
                saw_dynamic = true;
                if boundary.is_none() {
                    boundary = Some((index, static_count));
                }
            }
        }
        if !saw_static || !saw_dynamic {
            return false;
        }
        let Some((mut end_index, retained_count)) = boundary else {
            return false;
        };
        // Fingerprint the actual ordered packed source, not just raw Scene
        // revision or unordered Quad/Shadow arrays. Clip-degenerate instances
        // have already been removed and no temporary flattened buffer is made.
        // Preflight all ranges before mutating batches so rejection is atomic.
        let Some((content_hash, content_bytes)) =
            self.static_prefix_fingerprint(end_index, retained_count)
        else {
            return false;
        };
        if content_bytes == 0 {
            return false;
        }

        if retained_count != 0 {
            // A single prepared run may straddle the static/dynamic boundary.
            // Preserve its original instance order and exact buffer offsets.
            let old = self.batches[end_index];
            let retained_count = retained_count as u32;
            let (prefix, suffix) = match old {
                UploadedBatch::Quads { first, count } => (
                    UploadedBatch::Quads {
                        first,
                        count: retained_count,
                    },
                    UploadedBatch::Quads {
                        first: first + retained_count,
                        count: count - retained_count,
                    },
                ),
                UploadedBatch::SolidQuads { first, count } => (
                    UploadedBatch::SolidQuads {
                        first,
                        count: retained_count,
                    },
                    UploadedBatch::SolidQuads {
                        first: first + retained_count,
                        count: count - retained_count,
                    },
                ),
                UploadedBatch::Shadows { first, count } => (
                    UploadedBatch::Shadows {
                        first,
                        count: retained_count,
                    },
                    UploadedBatch::Shadows {
                        first: first + retained_count,
                        count: count - retained_count,
                    },
                ),
                _ => unreachable!("only Quad/Shadow batches can straddle the capture boundary"),
            };
            self.batches[end_index] = prefix;
            self.batches.insert(end_index + 1, suffix);
            // Bounds from a combined animated run are not independently
            // cullable. Keep the split conservative until each component has
            // a proven isolated visibility region.
            self.batch_visual_bounds[end_index] = None;
            self.batch_visual_bounds.insert(end_index + 1, None);
            end_index += 1;
        }

        let index = (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
        write_paint_blur_fields_for_target_size(
            &mut self.backdrop_blurs,
            0,
            bounds,
            content_mask,
            crate::ScaledPixels(0.0),
            1.0,
            target_size,
        );
        self.element_blur_inputs.push((
            index,
            FilterSource::AutoStatic {
                bounds,
                content_mask,
                content_hash,
                content_bytes,
            },
        ));
        self.batches.insert(0, UploadedBatch::BeginBlur { index });
        self.batch_visual_bounds.insert(0, None);
        // The leading BeginBlur shifts the first foreground batch by one.
        self.batches
            .insert(end_index + 1, UploadedBatch::EndBlur { index });
        self.batches
            .insert(end_index + 2, UploadedBatch::CompositeBlur { index });
        self.batch_visual_bounds.insert(end_index + 1, None);
        self.batch_visual_bounds.insert(end_index + 2, None);
        summary.backdrop_blur_count = summary.backdrop_blur_count.saturating_add(1);
        self.refresh_backdrop_blur_configs();
        self.rebuild_backdrop_blur_passes();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quads() -> FrameUpload {
        let bytes: Vec<_> = (0..4 * PACKED_QUAD_BYTES)
            .map(|index| (index / PACKED_QUAD_BYTES) as u8)
            .collect();
        FrameUpload {
            quads: bytes.into(),
            batches: vec![UploadedBatch::SolidQuads { first: 0, count: 4 }],
            ..Default::default()
        }
    }

    #[test]
    fn chunk_and_batch_segmentation_preserve_source_fingerprint() {
        let mut upload = quads();
        let expected = upload.static_prefix_fingerprint(0, 4).unwrap();
        let flat = upload.quads.to_vec();
        upload.quads.clear();
        upload
            .quads
            .append_shared(Arc::new(flat[..PACKED_QUAD_BYTES].to_vec()), 7);
        upload.quads.append_shared(
            Arc::new(flat[PACKED_QUAD_BYTES..3 * PACKED_QUAD_BYTES].to_vec()),
            8,
        );
        upload
            .quads
            .write(|bytes| bytes.extend_from_slice(&flat[3 * PACKED_QUAD_BYTES..]));
        upload.batches = vec![
            UploadedBatch::SolidQuads { first: 0, count: 1 },
            UploadedBatch::SolidQuads { first: 1, count: 2 },
            UploadedBatch::SolidQuads { first: 3, count: 1 },
        ];
        assert_eq!(upload.static_prefix_fingerprint(2, 1), Some(expected));
        assert_ne!(upload.static_prefix_fingerprint(2, 0), Some(expected));
        upload.batches[1] = UploadedBatch::Quads { first: 1, count: 2 };
        assert_ne!(upload.static_prefix_fingerprint(2, 1), Some(expected));
    }

    #[test]
    fn source_bytes_order_and_draw_state_invalidate_fingerprint() {
        let mut upload = quads();
        let expected = upload.static_prefix_fingerprint(0, 4).unwrap();
        upload.quads.slice_mut(0..1)[0] = 9;
        assert_ne!(upload.static_prefix_fingerprint(0, 4), Some(expected));
        upload.quads.slice_mut(0..1)[0] = 0;
        upload.batches = vec![
            UploadedBatch::SolidQuads { first: 2, count: 2 },
            UploadedBatch::SolidQuads { first: 0, count: 2 },
        ];
        assert_ne!(upload.static_prefix_fingerprint(1, 2), Some(expected));
        upload.batches = vec![UploadedBatch::Shadows { first: 0, count: 4 }];
        upload.shadows.resize(4 * PACKED_SHADOW_BYTES, 0);
        assert_ne!(upload.static_prefix_fingerprint(0, 4), Some(expected));
    }

    #[test]
    fn malformed_or_unsupported_source_never_produces_fingerprint() {
        let mut upload = quads();
        assert!(upload.static_prefix_fingerprint(0, 5).is_none());
        assert!(upload.static_prefix_fingerprint(1, 4).is_none());
        upload.batches[0] = UploadedBatch::BeginBlur { index: 0 };
        assert!(upload.static_prefix_fingerprint(0, 1).is_none());
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PrefixKind {
    Quad,
    SolidQuad,
    Shadow,
}

struct PrefixRun {
    kind: PrefixKind,
    first: usize,
    count: usize,
}

impl PrefixRun {
    fn from_batch(batch: UploadedBatch, count: usize) -> Option<Self> {
        let (kind, first) = match batch {
            UploadedBatch::Quads { first, .. } => (PrefixKind::Quad, first),
            UploadedBatch::SolidQuads { first, .. } => (PrefixKind::SolidQuad, first),
            UploadedBatch::Shadows { first, .. } => (PrefixKind::Shadow, first),
            _ => return None,
        };
        Some(Self {
            kind,
            first: first as usize,
            count,
        })
    }
}

impl FrameUpload {
    /// Fingerprint logical draw-state runs, independently of CPU Chunk/Batch segmentation.
    /// Do not treat a change of backing allocation as a change of Filter Source pixels.
    fn static_prefix_fingerprint(&self, end: usize, tail_count: usize) -> Option<(u64, usize)> {
        let mut fingerprint = collections::FxHasher::default();
        let mut content_bytes = 0usize;
        let mut pending: Option<PrefixRun> = None;
        for (position, batch) in self.batches.get(..=end)?.iter().copied().enumerate() {
            let count = if position == end {
                tail_count
            } else {
                match batch {
                    UploadedBatch::Quads { count, .. }
                    | UploadedBatch::SolidQuads { count, .. }
                    | UploadedBatch::Shadows { count, .. } => count as usize,
                    _ => return None,
                }
            };
            if count == 0 {
                continue;
            }
            let run = PrefixRun::from_batch(batch, count)?;
            if let Some(previous) = pending.as_mut()
                && previous.kind == run.kind
                && previous.first.checked_add(previous.count)? == run.first
            {
                previous.count = previous.count.checked_add(run.count)?;
                continue;
            }
            if let Some(previous) = pending.replace(run) {
                content_bytes =
                    content_bytes.checked_add(self.hash_prefix_run(previous, &mut fingerprint)?)?;
            }
        }
        if let Some(run) = pending {
            content_bytes =
                content_bytes.checked_add(self.hash_prefix_run(run, &mut fingerprint)?)?;
        }
        Some((fingerprint.finish(), content_bytes))
    }

    fn hash_prefix_run(
        &self,
        run: PrefixRun,
        fingerprint: &mut collections::FxHasher,
    ) -> Option<usize> {
        let record_bytes = if run.kind == PrefixKind::Shadow {
            PACKED_SHADOW_BYTES
        } else {
            PACKED_QUAD_BYTES
        };
        let start = run.first.checked_mul(record_bytes)?;
        let size = run.count.checked_mul(record_bytes)?;
        let end = start.checked_add(size)?;
        fingerprint.write_u8(match run.kind {
            PrefixKind::Quad => 0,
            PrefixKind::SolidQuad => 1,
            PrefixKind::Shadow => 2,
        });
        fingerprint.write_usize(run.count);
        if run.kind == PrefixKind::Shadow {
            fingerprint.write(self.shadows.get(start..end)?);
        } else {
            if end > self.quads.len() {
                return None;
            }
            // FxHasher's write boundaries affect its result. Use fixed record
            // boundaries across both owned gaps and shared Chunk allocations.
            for (_, bytes) in self.quads.slices(start..end) {
                let records = bytes.chunks_exact(PACKED_QUAD_BYTES);
                if !records.remainder().is_empty() {
                    return None;
                }
                for record in records {
                    fingerprint.write(record);
                }
            }
        }
        Some(size)
    }
}
