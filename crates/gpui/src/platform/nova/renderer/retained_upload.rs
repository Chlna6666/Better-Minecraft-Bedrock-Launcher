use super::chunk_upload::{QuadResidentLayout, QuadUploadPlan};
use super::*;
use std::hash::Hasher;
use std::time::Duration;

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

    fn from_segmented_bytes(
        bytes: &[u8],
        retained_spans: &[RetainedResidentSpan],
    ) -> (Self, usize) {
        if retained_spans.is_empty() {
            return (Self::from_bytes(bytes), bytes.len());
        }

        let mut aggregate = collections::FxHasher::default();
        let mut cursor = 0usize;
        let mut hashed_bytes = 0usize;
        for span in retained_spans {
            if span.range.start < cursor || span.range.end > bytes.len() {
                return (Self::from_bytes(bytes), bytes.len());
            }
            if cursor < span.range.start {
                let dirty = &bytes[cursor..span.range.start];
                let token = Self::from_bytes(dirty);
                aggregate.write_u8(0);
                aggregate.write_usize(token.byte_len);
                aggregate.write_u64(token.byte_hash);
                hashed_bytes = hashed_bytes.saturating_add(dirty.len());
            }
            aggregate.write_u8(1);
            aggregate.write_usize(span.range.len());
            aggregate.write_u64(span.byte_hash);
            cursor = span.range.end;
        }
        if cursor < bytes.len() {
            let dirty = &bytes[cursor..];
            let token = Self::from_bytes(dirty);
            aggregate.write_u8(0);
            aggregate.write_usize(token.byte_len);
            aggregate.write_u64(token.byte_hash);
            hashed_bytes = hashed_bytes.saturating_add(dirty.len());
        }
        (
            Self {
                byte_len: bytes.len(),
                byte_hash: aggregate.finish(),
            },
            hashed_bytes,
        )
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

    fn with_content_and_animation_topology(
        content: BufferContentToken,
        animation_topology: BufferContentToken,
    ) -> Self {
        Self {
            content,
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
    fn from_frame_upload(upload: &FrameUpload) -> (Self, usize) {
        let (animation_topology, animation_topology_bytes) =
            AnimationTopologyTokens::from_frame_upload(upload);
        let (quad_content, quad_hashed_bytes) =
            BufferContentToken::from_segmented_bytes(&upload.quads, &upload.resident_quad_spans);
        let hashed_bytes = [
            upload.globals.len(),
            upload.text_raster_params.len(),
            upload.shadows.len(),
            upload.path_rasterization_vertices.len(),
            upload.path_sprites.len(),
            upload.mono_sprites.len(),
            upload.poly_sprites.len(),
            upload.underlines.len(),
            upload.backdrop_blur_passes.len(),
            upload.backdrop_blurs.len(),
            upload.custom_mesh_3d_parameters.len(),
            animation_topology_bytes,
            quad_hashed_bytes,
        ]
        .into_iter()
        .fold(0usize, usize::saturating_add);
        (
            Self {
                global: StaticStreamToken::from_bytes(&upload.globals),
                text_raster: StaticStreamToken::from_bytes(&upload.text_raster_params),
                quad: StaticStreamToken::with_content_and_animation_topology(
                    quad_content,
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
            },
            hashed_bytes,
        )
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
    fn from_frame_upload(upload: &FrameUpload) -> (Self, usize) {
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

        let hashed_bytes = upload.animated_primitives.len().saturating_mul(
            std::mem::size_of::<u32>().saturating_add(std::mem::size_of::<usize>()),
        );
        (
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
            },
            hashed_bytes,
        )
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
    const STREAM_COUNT: usize = 12;

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
            add(self.backdrop_blur_pass, upload.backdrop_blur_passes.len());
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
    current_quad_layout: QuadResidentLayout,
    uploaded_quad_layouts: Vec<Option<QuadResidentLayout>>,
}

impl RetainedUpload {
    pub(super) fn static_upload_mask(&self, slot: usize) -> StaticUploadMask {
        let Some(current) = self.static_signature else {
            return StaticUploadMask::all();
        };
        let previous = self.uploaded_slots.get(slot).copied().flatten();
        current.diff(previous)
    }

    #[cfg(test)]
    pub(super) fn needs_static_upload(&self, slot: usize) -> bool {
        !self.static_upload_mask(slot).is_empty()
    }

    pub(super) fn quad_upload_plan(&self, slot: usize, byte_len: usize) -> QuadUploadPlan {
        let stream_dirty = self.static_upload_mask(slot).quad;
        let previous = self
            .uploaded_quad_layouts
            .get(slot)
            .and_then(Option::as_ref);
        self.current_quad_layout
            .upload_plan(previous, byte_len, stream_dirty)
    }

    pub(super) fn mark_uploaded(&mut self, slot: usize) {
        let Some(signature) = self.static_signature else {
            return;
        };
        if let Some(uploaded) = self.uploaded_slots.get_mut(slot) {
            *uploaded = Some(signature);
        }
        if let Some(uploaded) = self.uploaded_quad_layouts.get_mut(slot) {
            *uploaded = Some(self.current_quad_layout.clone());
        }
    }

    fn replace(
        &mut self,
        key: UploadKey,
        summary: FrameUploadSummary,
        slots: usize,
        static_signature: StaticUploadSignature,
        quad_layout: QuadResidentLayout,
    ) {
        self.key = Some(key);
        self.summary = summary;
        self.static_signature = Some(static_signature);
        self.current_quad_layout = quad_layout;
        // Preserve per-slot resident generations. A new scene revision no longer poisons every
        // static stream in every frame resource; `static_upload_mask` diffs the new packed streams
        // against what each slot actually contains.
        self.uploaded_slots.resize(slots, None);
        self.uploaded_quad_layouts.resize(slots, None);
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
        let mut encode_time = Duration::ZERO;
        let mut signature_time = Duration::ZERO;
        let mut hashed_bytes = 0usize;
        if reusable {
            summary.retained_chunk_hits = 0;
            summary.retained_chunk_misses = 0;
            summary.retained_chunk_reused_bytes = 0;
            self.frame_upload
                .refresh_retained_animation_values(scene, &mut summary);
        } else {
            let encode_started_at = Instant::now();
            summary = self.frame_upload.encode(
                scene,
                self.current_size,
                &self.rendering_parameters,
                key.premultiplied_alpha,
                blur_quality,
            );
            encode_time = encode_started_at.elapsed();
            // Element blur composites are intentionally registered after recursive static encode.
            // Their child batches remain ordinary retained source geometry, while only the final
            // CompositeBlur record receives the promoted visual animation binding.
            self.frame_upload
                .register_element_blur_animations(scene, &mut summary);
            // BeginBlur/EndBlur topology is a pure function of the static flattened batch stream.
            // Parse it once here and reuse the retained slice throughout target planning, present
            // damage and draw-step construction instead of rebuilding temporary Vecs per consumer.
            self.frame_upload.refresh_blur_content_ranges();
            // Promote ordinary Quad/glyph/image animation before hashing static streams. The
            // otherwise-unused packed draw-order lane becomes a stable animation slot and promoted
            // records leave `animated_primitives`, eliminating per-glyph CPU serialization.
            self.frame_upload.promote_gpu_indexed_animations();
            // Capture static content after slot assignment but before CPU-driven Shadow/Blur
            // sampling mutates its packed primitive bytes.
            let signature_started_at = Instant::now();
            let (static_signature, signature_hashed_bytes) =
                StaticUploadSignature::from_frame_upload(&self.frame_upload);
            signature_time = signature_started_at.elapsed();
            hashed_bytes = signature_hashed_bytes;
            self.retained_upload.replace(
                key,
                summary,
                self.frame_resources.len(),
                static_signature,
                QuadResidentLayout::from_upload(&self.frame_upload),
            );
        }
        // GPU-indexed source animations are intentionally absent from `animated_primitives`.
        // During their active/previous-frame window, disable aggressive retained blur self-damage
        // suppression so a filtered backdrop cannot reuse stale source pixels.
        let gpu_indexed_animation_blocks_blur_reuse = self
            .frame_upload
            .gpu_indexed_animation_affects_blur_history();
        self.frame_upload.retained_static_reused =
            reusable && !gpu_indexed_animation_blocks_blur_reuse;
        self.frame_upload
            .sample_animated_primitives(self.current_size);

        let static_stream_misses = self
            .retained_upload
            .static_upload_mask(self.current_frame_resource_index)
            .count();
        crate::diagnostics::performance_metrics::record_nova_scene_prepare_metrics(
            encode_time,
            signature_time,
            hashed_bytes,
            summary.retained_chunk_hits,
            summary.retained_chunk_misses,
            summary.retained_chunk_reused_bytes,
            reusable,
            StaticUploadMask::STREAM_COUNT.saturating_sub(static_stream_misses),
            static_stream_misses,
        );

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

    fn resident_span(
        name: &'static str,
        generation: u64,
        range: std::ops::Range<usize>,
    ) -> RetainedResidentSpan {
        RetainedResidentSpan {
            id: RetainedChunkId::new(
                crate::GlobalElementId(smallvec::smallvec![name.into()]),
                generation,
            ),
            range,
            byte_hash: generation,
        }
    }

    #[test]
    fn static_upload_is_tracked_per_stream_and_per_slot() {
        let mut retained = RetainedUpload::default();
        let mut upload = FrameUpload::default();
        upload.quads.extend_from_slice(b"quad-a");
        upload.shadows.extend_from_slice(b"shadow-a");
        let (first, first_hashed_bytes) = StaticUploadSignature::from_frame_upload(&upload);
        assert_eq!(
            first_hashed_bytes,
            upload.quads.len() + upload.shadows.len()
        );

        retained.replace(
            key(1),
            FrameUploadSummary::default(),
            3,
            first,
            QuadResidentLayout::default(),
        );
        retained.mark_uploaded(0);
        assert!(!retained.needs_static_upload(0));
        assert_eq!(retained.static_upload_mask(1).count(), 12);

        upload.quads.clear();
        upload.quads.extend_from_slice(b"quad-b");
        let (second, second_hashed_bytes) = StaticUploadSignature::from_frame_upload(&upload);
        assert_eq!(
            second_hashed_bytes,
            upload.quads.len() + upload.shadows.len()
        );
        retained.replace(
            key(2),
            FrameUploadSummary::default(),
            3,
            second,
            QuadResidentLayout::default(),
        );

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
        retained.replace(
            key(3),
            FrameUploadSummary::default(),
            3,
            second,
            QuadResidentLayout::default(),
        );
        assert!(!retained.needs_static_upload(0));
    }

    #[test]
    fn retained_span_signature_hashes_only_dirty_bytes() {
        let mut upload = FrameUpload::default();
        upload.quads.extend(0_u8..96);
        let cached = &upload.quads[32..64];
        let mut hasher = collections::FxHasher::default();
        hasher.write(cached);
        upload.resident_quad_spans.push(RetainedResidentSpan {
            id: RetainedChunkId::new(crate::GlobalElementId::default(), 1),
            range: 32..64,
            byte_hash: hasher.finish(),
        });

        let (_, hashed_bytes) = StaticUploadSignature::from_frame_upload(&upload);

        assert_eq!(hashed_bytes, 64);
    }

    #[test]
    fn slot_residency_uploads_dirty_chunk_but_not_clean_sibling() {
        let mut retained = RetainedUpload::default();
        let mut upload = FrameUpload::default();
        upload.quads.resize(64, 1);
        upload.resident_quad_spans = vec![
            resident_span("left", 1, 0..32),
            resident_span("right", 1, 32..64),
        ];
        let (first_signature, _) = StaticUploadSignature::from_frame_upload(&upload);
        retained.replace(
            key(1),
            FrameUploadSummary::default(),
            2,
            first_signature,
            QuadResidentLayout::from_upload(&upload),
        );
        retained.mark_uploaded(0);

        upload.quads[..32].fill(2);
        upload.resident_quad_spans[0] = resident_span("left", 2, 0..32);
        let (second_signature, _) = StaticUploadSignature::from_frame_upload(&upload);
        retained.replace(
            key(2),
            FrameUploadSummary::default(),
            2,
            second_signature,
            QuadResidentLayout::from_upload(&upload),
        );

        assert_eq!(
            retained.quad_upload_plan(0, upload.quads.len()),
            QuadUploadPlan::Ranges(vec![0..32])
        );
        assert_eq!(
            retained.quad_upload_plan(1, upload.quads.len()),
            QuadUploadPlan::Full
        );
    }
}
