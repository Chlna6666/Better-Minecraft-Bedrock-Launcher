use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
struct UploadKey {
    scene_revision: u64,
    size: DrawableSize,
    premultiplied_alpha: bool,
    blur_quality: BackdropBlurQuality,
}

#[derive(Default)]
pub(super) struct RetainedUpload {
    key: Option<UploadKey>,
    summary: FrameUploadSummary,
    uploaded_slots: Vec<bool>,
}

impl RetainedUpload {
    pub(super) fn needs_static_upload(&self, slot: usize) -> bool {
        !self.uploaded_slots.get(slot).copied().unwrap_or(false)
    }

    pub(super) fn mark_uploaded(&mut self, slot: usize) {
        self.uploaded_slots[slot] = true;
    }

    fn replace(&mut self, key: UploadKey, summary: FrameUploadSummary, slots: usize) {
        self.key = Some(key);
        self.summary = summary;
        self.uploaded_slots.resize(slots, false);
        self.uploaded_slots.fill(false);
    }
}

impl NovaRenderer {
    pub(super) fn pack_scene(
        &mut self,
        scene: &crate::Scene,
        blur_quality: BackdropBlurQuality,
    ) -> FrameUploadSummary {
        crate::diagnostics::performance_metrics::reset_frame_upload_metrics();
        let started_at = Instant::now();
        let key = UploadKey {
            scene_revision: scene.revision,
            size: self.current_size,
            premultiplied_alpha: self.surface_alpha.outputs_premultiplied_alpha(),
            blur_quality,
        };
        // RenderingParameters are immutable for this renderer. Element-blur child scenes are
        // flattened into the same static upload, and retained-animation refresh now recursively
        // rebuilds their animation-value stream. Their presence therefore no longer invalidates
        // otherwise identical static primitive/batch data.
        let reusable = scene.revision != 0 && self.retained_upload.key == Some(key);
        let mut summary = self.retained_upload.summary;
        if reusable {
            self.frame_upload
                .refresh_retained_animation_values(scene, &mut summary);
        } else {
            summary = self.frame_upload.encode(
                scene,
                self.current_size,
                &self.rendering_parameters,
                key.premultiplied_alpha,
                blur_quality,
            );
            // Element blur composites are intentionally registered after recursive static encode.
            // Their child batches remain ordinary retained source geometry, while only the final
            // CompositeBlur record receives the promoted visual animation binding.
            self.frame_upload
                .register_element_blur_animations(scene, &mut summary);
            // BeginBlur/EndBlur topology is a pure function of the static flattened batch stream.
            // Parse it once here and reuse the retained slice throughout target planning, present
            // damage and draw-step construction instead of rebuilding temporary Vecs per consumer.
            self.frame_upload.refresh_blur_content_ranges();
            self.retained_upload
                .replace(key, summary, self.frame_resources.len());
        }
        // The animation sampler needs to know whether static scene pixels were retained. Only then
        // may composite-only blur animation suppress self damage; a rebuilt display list can contain
        // real source changes inside or before the same filter layer.
        self.frame_upload.retained_static_reused = reusable;
        self.frame_upload
            .sample_animated_primitives(self.current_size);

        if self.diagnostics.should_log_frame_details() {
            log::warn!(
                "nova-gfx retained upload: scene_revision={} retained_reused={} static_slot_upload={} element_blurs={} animation_values={}",
                scene.revision,
                reusable,
                self.retained_upload
                    .needs_static_upload(self.current_frame_resource_index),
                self.frame_upload.has_element_blurs(),
                summary.animation_value_count,
            );
        }

        crate::diagnostics::performance_metrics::record_scene_pack_time(started_at.elapsed());
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_upload_is_tracked_per_slot_and_invalidated_by_new_scene() {
        let mut retained = RetainedUpload::default();
        let key = UploadKey {
            scene_revision: 1,
            size: DrawableSize {
                width: 640,
                height: 480,
            },
            premultiplied_alpha: false,
            blur_quality: BackdropBlurQuality::Full,
        };
        retained.replace(key, FrameUploadSummary::default(), 3);
        retained.mark_uploaded(0);
        assert!(!retained.needs_static_upload(0));
        assert!(retained.needs_static_upload(1));
        // A slot with an interrupted upload is never marked as current.
        assert!(retained.needs_static_upload(2));
        retained.replace(
            UploadKey {
                scene_revision: 2,
                ..key
            },
            FrameUploadSummary::default(),
            3,
        );
        assert!(retained.needs_static_upload(0));
    }
}
