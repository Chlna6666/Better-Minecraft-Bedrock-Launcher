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
        // RenderingParameters are immutable for this renderer. Nested blur captures
        // have their own value arrays and keep the full encoder until flattened.
        let reusable = scene.revision != 0
            && self.retained_upload.key == Some(key)
            && !self.frame_upload.has_element_blurs();
        let mut summary = self.retained_upload.summary;
        if reusable {
            self.frame_upload
                .refresh_animation_values(scene, &mut summary);
        } else {
            summary = self.frame_upload.encode(
                scene,
                self.current_size,
                &self.rendering_parameters,
                key.premultiplied_alpha,
                blur_quality,
            );
            self.retained_upload
                .replace(key, summary, self.frame_resources.len());
        }
        self.frame_upload
            .sample_animated_primitives(self.current_size);
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
