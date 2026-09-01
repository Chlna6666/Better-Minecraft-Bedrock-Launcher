use super::*;
use std::hash::Hasher;

#[derive(Clone, Copy, PartialEq, Eq)]
struct UploadKey {
    scene_revision: u64,
    size: DrawableSize,
    premultiplied_alpha: bool,
    blur_quality: BackdropBlurQuality,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct BufferContentToken {
    byte_len: usize,
    byte_hash: u64,
}

impl BufferContentToken {
    fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = collections::FxHasher::default();
        hasher.write(bytes);
        Self {
            byte_len: bytes.len(),
            byte_hash: hasher.finish(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StaticStreamToken {
    content: BufferContentToken,
    animation_topology: BufferContentToken,
}

impl StaticStreamToken {
    fn from_bytes(bytes: &[u8]) -> Self {
        Self {
            content: BufferContentToken::from_bytes(bytes),
            animation_topology: BufferContentToken::default(),
        }
    }

    fn with_animation_topology(bytes: &[u8], animation_topology: BufferContentToken) -> Self {
        Self {
            content: BufferContentToken::from_bytes(bytes),
            animation_topology,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct StaticUploadSignature {
    global: StaticStreamToken,
    text_raster: StaticStreamToken,
    quad: StaticStreamToken,
    shadow: StaticStreamToken,
    path_rasterization_vertex: StaticStreamToken,
    path_sprite: StaticStreamToken,
    mono_sprite: StaticStreamToken,
    poly_sprite: StaticStreamToken,
    underline: StaticStreamToken,
    backdrop_blur_pass: StaticStreamToken,
    backdrop_blur: StaticStreamToken,
    custom_mesh_3d_parameters: StaticStreamToken,
}

impl StaticUploadSignature {
    fn from_frame_upload(upload: &FrameUpload) -> Self {
        let animation_topology = AnimationTopologyTokens::from_frame_upload(upload);
        Self {
            global: StaticStreamToken::from_bytes(&upload.globals),
            text_raster: StaticStreamToken::from_bytes(&upload.text_raster_params),
            quad: StaticStreamToken::with_animation_topology(
                &upload.quads,
                animation_topology.quad,
            ),
            shadow: StaticStreamToken::with_animation_topology(
                &upload.shadows,
                animation_topology.shadow,
            ),
            path_rasterization_vertex: StaticStreamToken::from_bytes(
                &upload.path_rasterization_vertices,
            ),
            path_sprite: StaticStreamToken::from_bytes(&upload.path_sprites),
            mono_sprite: StaticStreamToken::with_animation_topology(
                &upload.mono_sprites,
                animation_topology.mono_sprite,
            ),
            poly_sprite: StaticStreamToken::with_animation_topology(
                &upload.poly_sprites,
                animation_topology.poly_sprite,
            ),
            underline: StaticStreamToken::from_bytes(&upload.underlines),
            backdrop_blur_pass: StaticStreamToken::with_animation_topology(
                &upload.backdrop_blur_passes,
                animation_topology.backdrop_blur,
            ),
            backdrop_blur: StaticStreamToken::with_animation_topology(
                &upload.backdrop_blurs,
                animation_topology.backdrop_blur,
            ),
            custom_mesh_3d_parameters: StaticStreamToken::from_bytes(
                &upload.custom_mesh_3d_parameters,
            ),
        }
    }

    fn diff(self, previous: Option<Self>) -> StaticUploadMask {
        let Some(previous) = previous else {
            return StaticUploadMask::all();
        };
        StaticUploadMask {
            global: self.global != previous.global,
            text_raster: self.text_raster != previous.text_raster,
            quad: self.quad != previous.quad,
            shadow: self.shadow != previous.shadow,
            path_rasterization_vertex: self.path_rasterization_vertex
                != previous.path_rasterization_vertex,
            path_sprite: self.path_sprite != previous.path_sprite,
            mono_sprite: self.mono_sprite != previous.mono_sprite,
            poly_sprite: self.poly_sprite != previous.poly_sprite,
            underline: self.underline != previous.underline,
            backdrop_blur_pass: self.backdrop_blur_pass != previous.backdrop_blur_pass,
            backdrop_blur: self.backdrop_blur != previous.backdrop_blur,
            custom_mesh_3d_parameters: self.custom_mesh_3d_parameters
                != previous.custom_mesh_3d_parameters,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AnimationTopologyTokens {
    quad: BufferContentToken,
    shadow: BufferContentToken,
    mono_sprite: BufferContentToken,
    poly_sprite: BufferContentToken,
    backdrop_blur: BufferContentToken,
}

impl AnimationTopologyTokens {
    fn from_frame_upload(upload: &FrameUpload) -> Self {
        let mut quad = collections::FxHasher::default();
        let mut shadow = collections::FxHasher::default();
        let mut mono_sprite = collections::FxHasher::default();
        let mut poly_sprite = collections::FxHasher::default();
        let mut backdrop_blur = collections::FxHasher::default();
        let mut quad_count = 0usize;
        let mut shadow_count = 0usize;
        let mut mono_sprite_count = 0usize;
        let mut poly_sprite_count = 0usize;
        let mut backdrop_blur_count = 0usize;

        for primitive in &upload.animated_primitives {
            let (hasher, count) = match primitive.kind {
                AnimatedPrimitiveKind::Quad => (&mut quad, &mut quad_count),
                AnimatedPrimitiveKind::Shadow => (&mut shadow, &mut shadow_count),
                AnimatedPrimitiveKind::MonochromeSprite => {
                    (&mut mono_sprite, &mut mono_sprite_count)
                }
                AnimatedPrimitiveKind::PolychromeSprite => {
                    (&mut poly_sprite, &mut poly_sprite_count)
                }
                AnimatedPrimitiveKind::BackdropBlur => {
                    (&mut backdrop_blur, &mut backdrop_blur_count)
                }
            };
            hasher.write_u32(primitive.index);
            hasher.write_usize(primitive.bytes.len());
            *count = count.saturating_add(1);
        }

        Self {
            quad: BufferContentToken {
                byte_len: quad_count,
                byte_hash: quad.finish(),
            },
            shadow: BufferContentToken {
                byte_len: shadow_count,
                byte_hash: shadow.finish(),
            },
            mono_sprite: BufferContentToken {
                byte_len: mono_sprite_count,
                byte_hash: mono_sprite.finish(),
            },
            poly_sprite: BufferContentToken {
                byte_len: poly_sprite_count,
                byte_hash: poly_sprite.finish(),
            },
            backdrop_blur: BufferContentToken {
                byte_len: backdrop_blur_count,
                byte_hash: backdrop_blur.finish(),
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct StaticUploadMask {
    pub(super) global: bool,
    pub(super) text_raster: bool,
    pub(super) quad: bool,
    pub(super) shadow: bool,
    pub(super) path_rasterization_vertex: bool,
    pub(super) path_sprite: bool,
    pub(super) mono_sprite: bool,
    pub(super) poly_sprite: bool,
    pub(super) underline: bool,
    pub(super) backdrop_blur_pass: bool,
    pub(super) backdrop_blur: bool,
    pub(super) custom_mesh_3d_parameters: bool,
}

impl StaticUploadMask {
    const fn all() -> Self {
        Self {
            global: true,
            text_raster: true,
            quad: true,
            shadow: true,
            path_rasterization_vertex: true,
            path_sprite: true,
            mono_sprite: true,
            poly_sprite: true,
            underline: true,
            backdrop_blur_pass: true,
            backdrop_blur: true,
            custom_mesh_3d_parameters: true,
        }
    }

    pub(super) const fn is_empty(self) -> bool {
        !(self.global
            || self.text_raster
            || self.quad
            || self.shadow
            || self.path_rasterization_vertex
            || self.path_sprite
            || self.mono_sprite
            || self.poly_sprite
            || self.underline
            || self.backdrop_blur_pass
            || self.backdrop_blur
            || self.custom_mesh_3d_parameters)
    }

    pub(super) fn count(self) -> usize {
        [
            self.global,
            self.text_raster,
            self.quad,
            self.shadow,
            self.path_rasterization_vertex,
            self.path_sprite,
            self.mono_sprite,
            self.poly_sprite,
            self.underline,
            self.backdrop_blur_pass,
            self.backdrop_blur,
            self.custom_mesh_3d_parameters,
        ]
        .into_iter()
        .filter(|dirty| *dirty)
        .count()
    }

    pub(super) fn covers_animated_kind(self, kind: AnimatedPrimitiveKind) -> bool {
        match kind {
            AnimatedPrimitiveKind::Quad => self.quad,
            AnimatedPrimitiveKind::Shadow => self.shadow,
            AnimatedPrimitiveKind::MonochromeSprite => self.mono_sprite,
            AnimatedPrimitiveKind::PolychromeSprite => self.poly_sprite,
            AnimatedPrimitiveKind::BackdropBlur => self.backdrop_blur,
        }
    }

    pub(super) fn mapped_upload_bytes(
        self,
        upload: &FrameUpload,
        has_backdrop_blurs: bool,
    ) -> usize {
        let mut bytes = 0usize;
        let mut add = |enabled: bool, len: usize| {
            if enabled {
                bytes = bytes.saturating_add(len);
            }
        };
        add(self.global, upload.globals.len());
        add(self.text_raster, upload.text_raster_params.len());
        add(self.quad, upload.quads.len());
        add(self.shadow, upload.shadows.len());
        add(
            self.path_rasterization_vertex,
            upload.path_rasterization_vertices.len(),
        );
        add(self.path_sprite, upload.path_sprites.len());
        add(self.mono_sprite, upload.mono_sprites.len());
        add(self.poly_sprite, upload.poly_sprites.len());
        add(self.underline, upload.underlines.len());
        if has_backdrop_blurs {
            add(
                self.backdrop_blur_pass,
                upload.backdrop_blur_passes.len(),
            );
            add(self.backdrop_blur, upload.backdrop_blurs.len());
        }
        add(
            self.custom_mesh_3d_parameters,
            upload.custom_mesh_3d_parameters.len(),
        );
        drop(add);

        for primitive in &upload.animated_primitives {
            if !self.covers_animated_kind(primitive.kind) {
                bytes = bytes.saturating_add(primitive.bytes.len());
            }
        }
        if upload.has_animated_backdrop_blurs() && !self.backdrop_blur_pass {
            bytes = bytes.saturating_add(upload.backdrop_blur_passes.len());
        }
        bytes
    }
}

#[derive(Default)]
pub(super) struct RetainedUpload {
    key: Option<UploadKey>,
    summary: FrameUploadSummary,
    static_signature: Option<StaticUploadSignature>,
    uploaded_slots: Vec<Option<StaticUploadSignature>>,
}

impl RetainedUpload {
    pub(super) fn static_upload_mask(&self, slot: usize) -> StaticUploadMask {
        let Some(current) = self.static_signature else {
            return StaticUploadMask::all();
        };
        let previous = self.uploaded_slots.get(slot).copied().flatten();
        current.diff(previous)
    }

    pub(super) fn needs_static_upload(&self, slot: usize) -> bool {
        !self.static_upload_mask(slot).is_empty()
    }

    pub(super) fn mark_uploaded(&mut self, slot: usize) {
        let Some(signature) = self.static_signature else {
            return;
        };
        if let Some(uploaded) = self.uploaded_slots.get_mut(slot) {
            *uploaded = Some(signature);
        }
    }

    fn replace(
        &mut self,
        key: UploadKey,
        summary: FrameUploadSummary,
        slots: usize,
        static_signature: StaticUploadSignature,
    ) {
        self.key = Some(key);
        self.summary = summary;
        self.static_signature = Some(static_signature);
        // Preserve per-slot resident generations. A new scene revision no longer poisons every
        // static stream in every frame resource; `static_upload_mask` diffs the new packed streams
        // against what each slot actually contains.
        self.uploaded_slots.resize(slots, None);
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
            // Capture static content before animation sampling mutates the packed primitive bytes.
            // Animation topology is part of the token so removing an animation forces one restoring
            // full-stream upload instead of leaving the last sampled primitive resident on the GPU.
            let static_signature = StaticUploadSignature::from_frame_upload(&self.frame_upload);
            self.retained_upload.replace(
                key,
                summary,
                self.frame_resources.len(),
                static_signature,
            );
        }
        // The animation sampler needs to know whether static scene pixels were retained. Only then
        // may composite-only blur animation suppress self damage; a rebuilt display list can contain
        // real source changes inside or before the same filter layer.
        self.frame_upload.retained_static_reused = reusable;
        self.frame_upload
            .sample_animated_primitives(self.current_size);

        if self.diagnostics.should_log_frame_details() {
            let static_uploads = self
                .retained_upload
                .static_upload_mask(self.current_frame_resource_index);
            log::warn!(
                "nova-gfx retained upload: scene_revision={} retained_reused={} static_slot_upload={} static_stream_uploads={} element_blurs={} animation_values={}",
                scene.revision,
                reusable,
                !static_uploads.is_empty(),
                static_uploads.count(),
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

    fn key(scene_revision: u64) -> UploadKey {
        UploadKey {
            scene_revision,
            size: DrawableSize {
                width: 640,
                height: 480,
            },
            premultiplied_alpha: false,
            blur_quality: BackdropBlurQuality::Full,
        }
    }

    #[test]
    fn static_upload_is_tracked_per_stream_and_per_slot() {
        let mut retained = RetainedUpload::default();
        let mut upload = FrameUpload::default();
        upload.quads.extend_from_slice(b"quad-a");
        upload.shadows.extend_from_slice(b"shadow-a");
        let first = StaticUploadSignature::from_frame_upload(&upload);

        retained.replace(key(1), FrameUploadSummary::default(), 3, first);
        retained.mark_uploaded(0);
        assert!(!retained.needs_static_upload(0));
        assert_eq!(retained.static_upload_mask(1).count(), 12);

        upload.quads.clear();
        upload.quads.extend_from_slice(b"quad-b");
        let second = StaticUploadSignature::from_frame_upload(&upload);
        retained.replace(key(2), FrameUploadSummary::default(), 3, second);

        let slot_zero = retained.static_upload_mask(0);
        assert!(slot_zero.quad);
        assert!(!slot_zero.shadow);
        assert_eq!(slot_zero.count(), 1);
        // A frame-resource slot that never received the previous static scene still requires all
        // streams, independent of which streams changed relative to another slot.
        assert_eq!(retained.static_upload_mask(1).count(), 12);

        retained.mark_uploaded(0);
        assert!(!retained.needs_static_upload(0));

        // Scene revisions are a CPU encode key, not a GPU residency generation. If packing the new
        // scene produces identical static streams, the slot stays resident and performs zero static
        // writes.
        retained.replace(key(3), FrameUploadSummary::default(), 3, second);
        assert!(!retained.needs_static_upload(0));
    }
}
