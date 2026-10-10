use super::*;

impl StaticUploadSignature {
    fn stream_mut(&mut self, stream: BufferStream) -> &mut StaticStreamToken {
        match stream {
            BufferStream::TextRaster => &mut self.text_raster,
            BufferStream::Quad => &mut self.quad,
            BufferStream::Shadow => &mut self.shadow,
            BufferStream::PathVertices => &mut self.path_rasterization_vertex,
            BufferStream::PathSprites => &mut self.path_sprite,
            BufferStream::MonoSprites => &mut self.mono_sprite,
            BufferStream::PolySprites => &mut self.poly_sprite,
            BufferStream::Underlines => &mut self.underline,
            BufferStream::BlurPasses => &mut self.backdrop_blur_pass,
            BufferStream::BlurRecords => &mut self.backdrop_blur,
        }
    }
}

fn invalid_token() -> StaticStreamToken {
    StaticStreamToken {
        content: BufferContentToken {
            byte_len: usize::MAX,
            byte_hash: u64::MAX,
        },
        animation_topology: BufferContentToken::default(),
    }
}

impl RetainedUpload {
    pub(in crate::platform::nova::renderer) fn update_shareability(
        &mut self,
        upload: &FrameUpload,
    ) {
        for (index, stream) in BufferStream::ALL.into_iter().enumerate() {
            let kind = match stream {
                BufferStream::Quad => Some(AnimatedPrimitiveKind::Quad),
                BufferStream::Shadow => Some(AnimatedPrimitiveKind::Shadow),
                BufferStream::MonoSprites => Some(AnimatedPrimitiveKind::MonochromeSprite),
                BufferStream::PolySprites => Some(AnimatedPrimitiveKind::PolychromeSprite),
                BufferStream::BlurRecords => Some(AnimatedPrimitiveKind::BackdropBlur),
                _ => None,
            };
            self.current_shareable[index] = !upload
                .animated_primitives
                .iter()
                .any(|primitive| Some(primitive.kind) == kind)
                && !(stream == BufferStream::BlurPasses && upload.has_animated_backdrop_blurs());
        }
    }

    pub(in crate::platform::nova::renderer) fn stream_shareable(
        &self,
        stream: BufferStream,
    ) -> bool {
        let index = BufferStream::ALL
            .iter()
            .position(|candidate| *candidate == stream)
            .expect("known static stream");
        self.current_shareable[index]
    }

    pub(in crate::platform::nova::renderer) fn stream_state(
        &self,
        slot: usize,
        stream: BufferStream,
    ) -> (bool, bool, bool) {
        let index = BufferStream::ALL
            .iter()
            .position(|candidate| *candidate == stream)
            .expect("known static stream");
        let Some(mut current) = self.static_signature else {
            return (false, false, true);
        };
        let previous = self.uploaded_slots.get(slot).copied().flatten();
        let matches = previous
            .is_some_and(|mut previous| previous.stream_mut(stream) == current.stream_mut(stream));
        let reusable = matches
            && self.current_shareable[index]
            && self
                .uploaded_shareable
                .get(slot)
                .is_some_and(|streams| streams[index]);
        (reusable, previous.is_some(), !matches)
    }

    pub(in crate::platform::nova::renderer) fn invalidate_stream(
        &mut self,
        slot: usize,
        stream: BufferStream,
    ) {
        if let Some(Some(signature)) = self.uploaded_slots.get_mut(slot) {
            *signature.stream_mut(stream) = invalid_token();
        }
        if stream == BufferStream::Quad {
            if let Some(layout) = self.uploaded_quad_layouts.get_mut(slot) {
                *layout = None;
            }
        }
    }

    pub(in crate::platform::nova::renderer) fn inherit_stream(
        &mut self,
        slot: usize,
        source: usize,
        stream: BufferStream,
    ) {
        let Some(mut signature) = self.static_signature else {
            return;
        };
        let mut previous = self.uploaded_slots[slot].unwrap_or_else(|| {
            let mut signature = StaticUploadSignature::default();
            signature.global = invalid_token();
            for stream in BufferStream::ALL {
                *signature.stream_mut(stream) = invalid_token();
            }
            signature
        });
        *previous.stream_mut(stream) = *signature.stream_mut(stream);
        self.uploaded_slots[slot] = Some(previous);
        let index = BufferStream::ALL
            .iter()
            .position(|candidate| *candidate == stream)
            .expect("known static stream");
        self.uploaded_shareable[slot][index] = true;
        if stream == BufferStream::Quad {
            self.uploaded_quad_layouts[slot] = self.uploaded_quad_layouts[source].clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resident(upload: &FrameUpload) -> RetainedUpload {
        let mut retained = RetainedUpload::default();
        retained.replace(
            UploadKey {
                scene_revision: 1,
                atlas_placement_generation: 0,
                size: DrawableSize {
                    width: 16,
                    height: 16,
                },
                premultiplied_alpha: false,
                blur_quality: BackdropBlurQuality::Full,
            },
            FrameUploadSummary::default(),
            2,
            StaticUploadSignature::from_frame_upload(upload).0,
            QuadResidentLayout::from_upload(upload),
        );
        retained.update_shareability(upload);
        retained.mark_uploaded(0);
        retained
    }

    #[test]
    fn inheriting_static_streams_skips_upload_but_keeps_globals_slot_local() {
        let upload = FrameUpload::default();
        let mut retained = resident(&upload);
        for stream in BufferStream::ALL {
            assert!(retained.stream_state(0, stream).0);
            retained.inherit_stream(1, 0, stream);
        }
        let dirty = retained.static_upload_mask(1);
        assert!(dirty.global);
        assert_eq!(dirty.count(), 1);
        assert!(matches!(
            retained.quad_upload_plan(1, 0),
            QuadUploadPlan::None
        ));
    }

    #[test]
    fn new_quad_version_requires_full_upload_and_does_not_poison_other_streams() {
        let upload = FrameUpload::default();
        let mut retained = resident(&upload);
        retained.inherit_stream(1, 0, BufferStream::Quad);
        retained.invalidate_stream(1, BufferStream::Quad);
        assert!(retained.static_upload_mask(1).quad);
        assert!(!retained.static_upload_mask(0).quad);
        assert!(matches!(
            retained.quad_upload_plan(1, PACKED_QUAD_BYTES),
            QuadUploadPlan::Full
        ));
    }

    #[test]
    fn cpu_patch_history_is_not_shareable_until_static_contents_are_replaced() {
        let mut retained = resident(&FrameUpload::default());
        let index = BufferStream::ALL
            .iter()
            .position(|stream| *stream == BufferStream::BlurPasses)
            .unwrap();
        retained.current_shareable[index] = false;
        retained.mark_uploaded(0);
        assert!(!retained.stream_state(0, BufferStream::BlurPasses).0);
        retained.current_shareable[index] = true;
        retained.mark_uploaded(0);
        assert!(
            !retained.stream_state(0, BufferStream::BlurPasses).0,
            "no patch this frame does not restore base bytes"
        );
        retained.invalidate_stream(0, BufferStream::BlurPasses);
        retained.mark_uploaded(0);
        assert!(retained.stream_state(0, BufferStream::BlurPasses).0);
    }

    #[test]
    fn dense_animation_slot_changes_invalidate_the_animated_gap() {
        fn encoded(animate_underline: bool) -> FrameUpload {
            let bounds = crate::bounds(
                crate::point(crate::px(0.0), crate::px(0.0)),
                crate::size(crate::px(16.0), crate::px(16.0)),
            )
            .scale(1.0);
            let mask = crate::ContentMask {
                bounds,
                corner_bounds: bounds,
                ..Default::default()
            };
            let mut scene = crate::Scene::default();
            let start = scene.len();
            for _ in 0..32 {
                scene.insert_primitive(Quad {
                    bounds,
                    content_mask: mask,
                    background: crate::white().into(),
                    ..Default::default()
                });
            }
            scene.record_retained_chunk(
                crate::GlobalElementId::from_path(&["static-chunk".into()]),
                1,
                start..scene.len(),
            );
            scene.insert_primitive(Quad {
                bounds,
                content_mask: mask,
                background: crate::white().into(),
                animation_id: Some(crate::SceneAnimationId(2)),
                ..Default::default()
            });
            scene.insert_primitive(Underline {
                order: 0,
                pad: 0,
                animation_id: animate_underline.then_some(crate::SceneAnimationId(1)),
                bounds,
                content_mask: mask,
                color: crate::white().into(),
                thickness: crate::ScaledPixels(1.0),
                wavy: 0,
            });
            scene.finish();
            let mut upload = FrameUpload::default();
            let mut summary = upload.encode(
                &scene,
                &[],
                DrawableSize {
                    width: 16,
                    height: 16,
                },
                &RenderingParameters::from_env(),
                false,
                BackdropBlurQuality::Full,
            );
            upload.register_element_blur_animations(&scene, &mut summary);
            let mut sharing = RetainedUpload::default();
            sharing.update_shareability(&upload);
            assert!(
                !sharing.stream_shareable(BufferStream::Quad),
                "CPU-patched records cannot share a stream"
            );
            upload.promote_gpu_indexed_animations();
            sharing.update_shareability(&upload);
            assert!(
                sharing.stream_shareable(BufferStream::Quad),
                "GPU-indexed records remain immutable"
            );
            upload
        }
        let before = encoded(true);
        let after = encoded(false);
        assert_eq!(before.resident_quad_spans.len(), 1);
        assert_eq!(after.resident_quad_spans.len(), 1);
        assert_eq!(before.resident_quad_spans[0], after.resident_quad_spans[0]);
        let gap = 32 * PACKED_QUAD_BYTES;
        assert_eq!(
            u32::from_ne_bytes(before.quads.slice(gap..gap + 4).try_into().unwrap()),
            2
        );
        assert_eq!(
            u32::from_ne_bytes(after.quads.slice(gap..gap + 4).try_into().unwrap()),
            1
        );
        let mut retained = resident(&before);
        retained.static_signature = Some(StaticUploadSignature::from_frame_upload(&after).0);
        retained.current_quad_layout = QuadResidentLayout::from_upload(&after);
        retained.update_shareability(&after);
        assert!(
            !retained.stream_state(0, BufferStream::Quad).0,
            "different GPU slots must not alias"
        );
        assert!(retained.static_upload_mask(0).quad);
        assert_eq!(
            retained.quad_upload_plan(0, after.quads.len()),
            QuadUploadPlan::Ranges(vec![gap..after.quads.len()]),
            "only the animated gap needs uploading into an owned resident buffer"
        );
        retained.invalidate_stream(0, BufferStream::Quad);
        assert_eq!(
            retained.quad_upload_plan(0, after.quads.len()),
            QuadUploadPlan::Full,
            "a new COW version must initialize static gaps too"
        );
    }
}
