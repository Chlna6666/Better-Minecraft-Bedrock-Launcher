use super::chunk_upload::{QuadResidentLayout, QuadUploadPlan};

mod auto_static;
mod color;
use auto_static::auto_capture_static_background;
mod sharing;
use super::*;
use std::hash::Hasher;
use std::time::Duration;

// Primitive-count retention remains a fallback for complex unfiltered scenes.
// Small animation pages use the raster-work policy below: buffer/filter reuse
// alone does not preserve static pixels in the final swapchain pass.
const AUTO_RETAINED_COLOR_MIN_PRIMITIVES: usize = 384;

/// Classifies *painter-order* scene content; this is not an ownership or
/// GPU-residency proof. A static prefix is only a candidate for future
/// independent retained color capture after its exact bytes, texture
/// dependencies and window-relative geometry have been versioned.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct StaticForegroundPartition {
    static_prefix: usize,
    static_after_dynamic: usize,
    animated: usize,
}

impl StaticForegroundPartition {
    fn total(self) -> usize {
        self.static_prefix
            .saturating_add(self.static_after_dynamic)
            .saturating_add(self.animated)
    }

    /// An early animated primitive followed by large amounts of content is
    /// not a separable static background. Wrapping the entire window in an
    /// extra full-screen color texture can be more expensive than direct
    /// rendering in this case.
    fn has_proven_static_majority(self) -> bool {
        self.animated == 0 || self.static_prefix.saturating_mul(3) >= self.total().saturating_mul(2)
    }

    fn has_contiguous_static_background(self) -> bool {
        self.static_prefix > 0 && self.animated > 0 && self.static_after_dynamic == 0
    }
}

fn classify_static_foreground(scene: &crate::Scene) -> StaticForegroundPartition {
    let mut partition = StaticForegroundPartition::default();
    let mut saw_animation = false;
    for operation in &scene.paint_operations {
        let crate::scene::PaintOperation::Primitive(primitive) = operation else {
            // StartLayer/EndLayer express batching and layout, not raster
            // ownership. Actual blur/filter scenes are excluded by the caller.
            continue;
        };
        if primitive.animation_id().is_some() {
            saw_animation = true;
            partition.animated += 1;
        } else if saw_animation {
            partition.static_after_dynamic += 1;
        } else {
            partition.static_prefix += 1;
        }
    }
    partition
}

fn should_retain_complex_scene_color(scene: &crate::Scene, summary: &FrameUploadSummary) -> bool {
    let primitives = summary.quad_count as usize
        + summary.shadow_count as usize
        + summary.path_sprite_count as usize
        + summary.mono_sprite_count as usize
        + summary.poly_sprite_count as usize
        + summary.underline_count as usize;
    // Outside the local-motion policy, retained filter textures alone do not
    // justify another full-window color target.
    if scene.has_backdrop_blurs() || !scene.blurs.is_empty() {
        return false;
    }
    if primitives < AUTO_RETAINED_COLOR_MIN_PRIMITIVES {
        return false;
    }
    let partition = classify_static_foreground(scene);
    // A cheap first-pass scene classification avoids installing a costly
    // fullscreen compositor on highly animated or heavily interleaved pages.
    // GPU-only animation is still rendered using the normal direct path.
    // This is intentionally NOT a license to reuse static pixels: the
    // retained source keeps existing pixel-accurate dirty invalidation.
    partition.has_proven_static_majority()
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct UploadKey {
    scene_revision: u64,
    atlas_placement_generation: u64,
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

    fn from_segmented_bytes(stream: &PackedQuadStream) -> (Self, usize) {
        let mut aggregate = collections::FxHasher::default();
        let mut hashed_bytes = 0usize;
        for (_, bytes, cached_hash) in stream.segments() {
            let hash = cached_hash.unwrap_or_else(|| {
                hashed_bytes = hashed_bytes.saturating_add(bytes.len());
                Self::from_bytes(bytes).byte_hash
            });
            aggregate.write_u8(u8::from(cached_hash.is_some()));
            aggregate.write_usize(bytes.len());
            aggregate.write_u64(hash);
        }
        (
            Self {
                byte_len: stream.len(),
                byte_hash: aggregate.finish(),
            },
            hashed_bytes,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct StaticStreamToken {
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
}

impl StaticUploadSignature {
    fn from_frame_upload(upload: &FrameUpload) -> (Self, usize) {
        let (animation_topology, animation_topology_bytes) =
            AnimationTopologyTokens::from_frame_upload(upload);
        let (quad_content, quad_hashed_bytes) =
            BufferContentToken::from_segmented_bytes(&upload.quads);
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
}

impl StaticUploadMask {
    const STREAM_COUNT: usize = 11;

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
            || self.backdrop_blur)
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
    /// Sole owner of packed retained Quad payloads across Scene revisions.
    pub(super) quad_chunks: RetainedQuadChunkCache,
    key: Option<UploadKey>,
    summary: FrameUploadSummary,
    static_signature: Option<StaticUploadSignature>,
    uploaded_slots: Vec<Option<StaticUploadSignature>>,
    current_quad_layout: QuadResidentLayout,
    uploaded_quad_layouts: Vec<Option<QuadResidentLayout>>,
    current_shareable: [bool; 10],
    uploaded_shareable: Vec<[bool; 10]>,
}

#[cfg(test)]
mod capacity_tests {
    use super::*;

    #[test]
    fn mask_residency_uses_path_content_not_other_static_streams() {
        let original = StaticUploadSignature {
            path_rasterization_vertex: StaticStreamToken::from_bytes(&[1, 2, 3]),
            ..Default::default()
        };
        let mut retained = RetainedUpload {
            static_signature: Some(original),
            ..Default::default()
        };
        let token = retained.path_mask_token().expect("packed content");
        let mut residency = super::super::path_mask::Residency::default();
        let key = super::super::path_mask::Key {
            content: Some(token),
            draw_plan: Some(1),
            texture_view: TextureViewId::new(1),
            target_size: Extent2d::new(64, 64).expect("extent"),
            viewport: DrawableSize {
                width: 64,
                height: 64,
            },
            format: Format::Bgra8Unorm,
            pipeline: RenderPipelineId::new(2),
        };
        residency.commit(key);
        retained.static_signature = Some(StaticUploadSignature {
            quad: StaticStreamToken::from_bytes(&[4]),
            ..original
        });
        assert!(!residency.begin(super::super::path_mask::Key {
            content: retained.path_mask_token(),
            ..key
        }));
        retained.static_signature = Some(StaticUploadSignature {
            path_rasterization_vertex: StaticStreamToken::from_bytes(&[1, 2, 4]),
            ..original
        });
        assert!(residency.begin(super::super::path_mask::Key {
            content: retained.path_mask_token(),
            ..key
        }));
    }

    #[test]
    fn replaced_slot_uploads_every_stream_without_invalidating_the_other_slot() {
        let signature = StaticUploadSignature::default();
        let mut retained = RetainedUpload {
            static_signature: Some(signature),
            uploaded_slots: vec![Some(signature), Some(signature)],
            ..Default::default()
        };
        assert!(retained.static_upload_mask(0).is_empty());
        assert!(retained.static_upload_mask(1).is_empty());
        retained.invalidate_slot(0);
        assert!(!retained.static_upload_mask(0).is_empty());
        assert!(retained.static_upload_mask(0).shadow);
        assert!(retained.static_upload_mask(0).mono_sprite);
        assert!(retained.static_upload_mask(0).poly_sprite);
        assert!(retained.static_upload_mask(1).is_empty());
    }
}

impl RetainedUpload {
    pub(super) fn path_mask_token(&self) -> Option<StaticStreamToken> {
        self.static_signature
            .map(|signature| signature.path_rasterization_vertex)
    }

    pub(super) fn invalidate_slot(&mut self, slot: usize) {
        if let Some(signature) = self.uploaded_slots.get_mut(slot) {
            *signature = None;
        }
        if let Some(layout) = self.uploaded_quad_layouts.get_mut(slot) {
            *layout = None;
        }
        if let Some(shareable) = self.uploaded_shareable.get_mut(slot) {
            *shareable = [false; 10];
        }
    }

    pub(super) fn invalidate_encode_key(&mut self) {
        self.key = None;
    }

    pub(super) fn invalidate_quad_slot(&mut self, slot: usize) {
        if let Some(Some(signature)) = self.uploaded_slots.get_mut(slot) {
            signature.quad = StaticStreamToken {
                content: BufferContentToken {
                    byte_len: usize::MAX,
                    byte_hash: u64::MAX,
                },
                animation_topology: BufferContentToken::default(),
            };
        }
        if let Some(layout) = self.uploaded_quad_layouts.get_mut(slot) {
            *layout = None;
        }
    }

    pub(super) fn invalidate_path_rasterization_slot(&mut self, slot: usize) {
        let Some(Some(signature)) = self.uploaded_slots.get_mut(slot) else {
            return;
        };
        signature.path_rasterization_vertex = StaticStreamToken {
            content: BufferContentToken {
                byte_len: usize::MAX,
                byte_hash: u64::MAX,
            },
            animation_topology: BufferContentToken::default(),
        };
    }

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
        // A slot has exactly one authoritative static-signature comparison.
        // Reuse its stream-level dirty flags to decide which streams have
        // physically received fresh base bytes. Matching CPU-patched streams
        // retain their previous non-shareable provenance until re-uploaded.
        let previous = self.uploaded_slots.get(slot).copied().flatten();
        let changed = signature.diff(previous);
        let refreshed = [
            changed.text_raster,
            changed.quad,
            changed.shadow,
            changed.path_rasterization_vertex,
            changed.path_sprite,
            changed.mono_sprite,
            changed.poly_sprite,
            changed.underline,
            changed.backdrop_blur_pass,
            changed.backdrop_blur,
        ];
        let old_shareable = self
            .uploaded_shareable
            .get(slot)
            .copied()
            .unwrap_or([false; 10]);
        let shareable = std::array::from_fn(|index| {
            self.current_shareable[index] && (refreshed[index] || old_shareable[index])
        });
        if let Some(uploaded) = self.uploaded_slots.get_mut(slot) {
            *uploaded = Some(signature);
        }
        if let Some(uploaded) = self.uploaded_quad_layouts.get_mut(slot) {
            *uploaded = Some(self.current_quad_layout.clone());
        }
        if let Some(uploaded) = self.uploaded_shareable.get_mut(slot) {
            *uploaded = shareable;
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
        self.uploaded_shareable.resize(slots, [false; 10]);
    }
}

impl NovaRenderer {
    pub(super) fn pack_scene(
        &mut self,
        scene: &crate::Scene,
        presentation_animation_values: &[crate::SceneAnimationValue],
        blur_quality: BackdropBlurQuality,
    ) -> FrameUploadSummary {
        crate::diagnostics::performance_metrics::reset_frame_upload_metrics();
        let started_at = Instant::now();
        #[cfg(target_os = "windows")]
        if self.draw_step_scratch.backdrop_blur_damage_region.is_full()
            && self.rendering_parameters.refresh_for_current_monitor()
        {
            // A monitor transition already forces a full high-level redraw and glyph-atlas refresh.
            // Drop only the CPU encode key here: per-slot static signatures remain resident, so the
            // subsequent stream diff uploads text-raster parameters without poisoning unrelated data.
            self.retained_upload.invalidate_encode_key();
        }
        let key = UploadKey {
            scene_revision: scene.revision,
            atlas_placement_generation: self.atlas.placement_generation(),
            size: self.current_size,
            premultiplied_alpha: self.surface_alpha.outputs_premultiplied_alpha(),
            blur_quality,
        };
        // Display-specific text raster parameters invalidate only this CPU encode key when their
        // visual values change. Element-blur child scenes are flattened into the same static upload,
        // and retained-animation refresh recursively rebuilds only the animation-value stream.
        // Admission depends on presented motion, not just scene topology. A
        // large forwards-filled layer can finish without changing the revision.
        // Reconsider a rejected root once at completion, rather than freezing
        // the first moving sample's decision for the scene's entire lifetime.
        let reconsider_color = self.frame_upload.retained_root_blur.is_none()
            && presentation_animation_values.iter().any(|current| {
                current.progress == 1.
                    && self.frame_upload.sampled_animation_values.iter().any(|previous| {
                        previous.animation_id == current.animation_id
                            && previous.property == current.property
                            && (previous.progress != 1.
                                || (current.property == crate::TransitionProperty::VisualState
                                    && current.to == [0., 0., 1., 1.]
                                    && previous.to != current.to))
                    })
            });
        let reusable = scene.revision != 0
            && self.retained_upload.key == Some(key)
            && !reconsider_color;
        // Once useful static window color is resident, finishing/removing a
        // small animation does not make those cached pixels disposable.
        let keep_retained_root = self.frame_upload.retained_root_blur.is_some()
            && self.retained_upload.key.is_some_and(|previous| {
                previous.size == key.size && previous.premultiplied_alpha == key.premultiplied_alpha
            });
        // Rebuild the capture only on Scene commits. Retained animation-only
        // frames keep the already encoded recursive prefix (which owns no
        // animation samples), so their normal root value refresh is unchanged.
        // Native GPU validation is still required before enabling this
        // optimization by default across all rendering backends.
        // The candidate builder rejects non-flat streams, filters, atlas-backed
        // prefixes and unstable resources. Its proven quad/shadow prefix can be
        // retained by default without changing ordinary/complex Scene handling.
        let auto_background = (!reusable)
            .then(|| auto_capture_static_background(scene, self.current_size))
            .flatten();
        let mut summary = self.retained_upload.summary;
        let mut encode_time = Duration::ZERO;
        let mut signature_time = Duration::ZERO;
        let mut hashed_bytes = 0usize;
        if reusable {
            summary.retained_chunk_hits = 0;
            summary.retained_chunk_misses = 0;
            summary.retained_chunk_reused_bytes = 0;
            self.frame_upload.refresh_retained_animation_values(
                scene,
                presentation_animation_values,
                &mut summary,
            );
        } else {
            let encode_started_at = Instant::now();
            self.atlas
                .copy_placements_into(&mut self.frame_upload.atlas_placements);
            summary = self.frame_upload.encode(
                &mut self.retained_upload.quad_chunks,
                scene,
                presentation_animation_values,
                self.current_size,
                &self.rendering_parameters,
                key.premultiplied_alpha,
                blur_quality,
            );
            // Match real Scene captures before AutoStatic inserts its synthetic
            // BeginBlur marker. Existing packed blur indices remain unchanged.
            self.frame_upload
                .register_element_blur_animations(scene, &mut summary);
            // Capture the encoded static prefix in place. No separate foreground
            // Scene, second Scene::finish, or second primitive encoding pass.
            let captured_static_prefix = auto_background.as_ref().is_some_and(|candidate| {
                self.frame_upload.capture_encoded_static_prefix(
                    scene,
                    candidate.bounds,
                    candidate.content_mask,
                    candidate.first_dynamic_order,
                    self.current_size,
                    &mut summary,
                )
            });
            encode_time = encode_started_at.elapsed();
            // Only CPU/GPU-native ordinary primitives can be replayed inside
            // a retained root target. Native surfaces/extensions use the old
            // full-redraw path until they expose their own damage semantics.
            if !captured_static_prefix
                && !scene.requires_full_redraw_fallback()
                && self.frame_upload.renderer_extensions.is_empty()
                && summary.unsupported_batches.total() == 0
                // Never add the extra full-surface texture for every window.
                // Small static pages use direct rendering; busy scenes with
                // nested filters/composites can reuse the retained root color
                // texture and only repaint the damaged source pixels. Exact
                // filter-order barriers are preserved inside the root capture.
                && (keep_retained_root
                    || color::should_retain_local_animation(
                        scene,
                        self.current_size,
                        presentation_animation_values,
                    )
                    || should_retain_complex_scene_color(scene, &summary))
            {
                self.frame_upload
                    .append_retained_root(self.current_size, &mut summary);
            }
            log::trace!(
                target: "gpui::diagnostics",
                "GPUI color retention result: scene_revision={} static_prefix_candidate={} static_prefix_captured={} retained_root={} kept_root={} full_redraw_fallback={} renderer_extensions={} unsupported_batches={}",
                scene.revision,
                auto_background.is_some(),
                captured_static_prefix,
                self.frame_upload.retained_root_blur.is_some(),
                keep_retained_root,
                scene.requires_full_redraw_fallback(),
                self.frame_upload.renderer_extensions.len(),
                summary.unsupported_batches.total(),
            );
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
            // Prepare the immutable layout before mutably replacing its owner.
            // Equal span sets share the previous Arc without rebuilding descriptors.
            let quad_layout = QuadResidentLayout::from_upload_reusing(
                &self.retained_upload.current_quad_layout,
                &self.frame_upload,
            );
            self.retained_upload.replace(
                key,
                summary,
                self.frame_resources.len(),
                static_signature,
                quad_layout,
            );
        }
        // GPU-indexed source animations are intentionally absent from `animated_primitives`.
        // During their active/previous-frame window, disable aggressive retained blur self-damage
        // suppression so a filtered backdrop cannot reuse stale source pixels.
        let gpu_indexed_animation_blocks_blur_reuse = self.frame_upload.affects_blur_history();
        self.frame_upload.retained_static_reused =
            reusable && !gpu_indexed_animation_blocks_blur_reuse;
        self.frame_upload
            .sample_animated_primitives(self.current_size);
        self.retained_upload.update_shareability(&self.frame_upload);

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

    #[test]
    fn auto_static_capture_owns_real_gpu_layer_only_for_flat_safe_prefix() {
        let mut original = crate::Scene::default();
        for _ in 0..100 {
            let bounds = crate::bounds(
                crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                crate::size(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
            );
            original.insert_primitive(crate::Quad {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                ..Default::default()
            });
        }
        let mut animated = crate::Quad::default();
        animated.bounds = crate::bounds(
            crate::point(crate::ScaledPixels(5.0), crate::ScaledPixels(5.0)),
            crate::size(crate::ScaledPixels(8.0), crate::ScaledPixels(8.0)),
        );
        animated.content_mask = crate::ContentMask::new(animated.bounds);
        animated.animation_id = Some(crate::SceneAnimationId(42));
        original.insert_primitive(animated);
        original.finish();
        original.revision = 1;
        let captured = auto_capture_static_background(
            &original,
            DrawableSize {
                width: 640,
                height: 480,
            },
        )
        .expect("flat page has independent static prefix");
        assert!(!captured.bounds.is_empty());
        assert_eq!(captured.content_mask.bounds.size.width, crate::ScaledPixels(640.0));
        assert_eq!(captured.first_dynamic_order, original.quads[100].order);
        assert_eq!(original.quads.len(), 101);
        assert_eq!(original.quads[100].animation_id, Some(crate::SceneAnimationId(42)));
        let viewport = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(640.0), crate::ScaledPixels(480.0)),
        );
        let mut nested = crate::Scene::default();
        nested.push_layer(viewport);
        nested.replay(0..100, &original);
        nested.pop_layer();
        nested.replay(100..101, &original);
        nested.finish();
        nested.revision = 2;
        let size = DrawableSize { width: 640, height: 480 };
        let nested_capture = auto_capture_static_background(&nested, size)
            .expect("a closed nested prefix retains its layer ordering");
        assert_eq!(nested_capture.first_dynamic_order, nested.quads.last().unwrap().order);
        assert_eq!(nested_capture.content_mask.bounds, viewport);
        let mut split = crate::Scene::default();
        split.push_layer(viewport);
        split.replay(0..101, &original);
        split.pop_layer();
        split.finish();
        split.revision = 3;
        assert!(auto_capture_static_background(&split, size).is_none(),
            "capture cannot cut through an open layer");
        assert!(
            auto_capture_static_background(
                &crate::Scene::default(),
                DrawableSize {
                    width: 640,
                    height: 480
                },
            )
            .is_none()
        );

        // The entire cached background is independent from the dynamic
        // foreground. A change to one background quad must invalidate its
        // real source texture on the next committed scene.
        let mut next = crate::Scene::default();
        for index in 0..100 {
            let bounds = crate::bounds(
                crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
                crate::size(
                    crate::ScaledPixels(if index == 50 { 12.0 } else { 10.0 }),
                    crate::ScaledPixels(10.0),
                ),
            );
            next.insert_primitive(crate::Quad {
                bounds,
                content_mask: crate::ContentMask::new(bounds),
                ..Default::default()
            });
        }
        next.insert_primitive(animated);
        next.finish();
        next.revision = 4;
        let changed = auto_capture_static_background(
            &next,
            DrawableSize {
                width: 640,
                height: 480,
            },
        )
        .expect("changed background still eligible");
        assert!(!changed.bounds.is_empty());
        assert_eq!(changed.first_dynamic_order, captured.first_dynamic_order);
        // Pixel changes are compared using packed source fingerprints after
        // encoding, not by deep-comparing cloned child Scenes.
    }

    #[test]
    fn static_prefix_keeps_zero_filter_popup_outside_root_capture() {
        let size = DrawableSize { width: 640, height: 480 };
        let viewport = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(640.0), crate::ScaledPixels(480.0)),
        );
        let id = crate::GlobalElementId::from_path(&["popup".into()]);
        let mut scene = crate::Scene::default();
        for _ in 0..24 {
            scene.insert_primitive(crate::Quad {
                bounds: viewport,
                content_mask: crate::ContentMask::new(viewport),
                ..Default::default()
            });
        }
        let popup = crate::bounds(
            crate::point(crate::ScaledPixels(40.0), crate::ScaledPixels(30.0)),
            crate::size(crate::ScaledPixels(120.0), crate::ScaledPixels(160.0)),
        );
        scene.begin_blur(crate::BlurCapture {
            animation_id: Some(crate::SceneAnimationId(9)),
            layer_id: Some(id),
            bounds: popup,
            content_mask: crate::ContentMask::new(popup),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
        });
        scene.insert_primitive(crate::Quad {
            bounds: popup,
            content_mask: crate::ContentMask::new(popup),
            ..Default::default()
        });
        scene.end_blur();
        scene.finish();
        scene.revision = 1;
        let captured = auto_capture_static_background(&scene, size)
            .expect("static background can be retained below an isolated popup");
        assert_eq!(captured.first_dynamic_order, scene.blurs[0].order);
        let mut upload = FrameUpload::default();
        let mut cache = RetainedQuadChunkCache::default();
        let mut summary = upload.encode(
            &mut cache, &scene, &[], size,
            &RenderingParameters::system(), false, BackdropBlurQuality::Full,
        );
        assert!(upload.capture_encoded_static_prefix(
            &scene, captured.bounds, captured.content_mask,
            captured.first_dynamic_order, size, &mut summary,
        ));
        assert_eq!(upload.element_blur_inputs.len(), 2,
            "the background and popup retain two independent sources");
        assert!(upload.retained_root_blur.is_none(),
            "the full-window root capture must not wrap the popup");
        upload.refresh_blur_content_ranges();
        assert_eq!(upload.blur_content_ranges().len(), 2);
    }

    #[test]
    fn static_prefix_rejects_gaussian_and_interleaved_capture() {
        let size = DrawableSize { width: 640, height: 480 };
        let rect = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(640.0), crate::ScaledPixels(480.0)),
        );
        let build = |radius| {
            let mut scene = crate::Scene::default();
            for _ in 0..24 {
                scene.insert_primitive(crate::Quad {
                    bounds: rect,
                    content_mask: crate::ContentMask::new(rect),
                    ..Default::default()
                });
            }
            scene.begin_blur(crate::BlurCapture {
                animation_id: Some(crate::SceneAnimationId(7)),
                layer_id: Some(crate::GlobalElementId::from_path(&["popup".into()])),
                bounds: rect,
                content_mask: crate::ContentMask::new(rect),
                radius: crate::ScaledPixels(radius),
                opacity: 1.0,
            });
            scene.insert_primitive(crate::Quad {
                bounds: rect,
                content_mask: crate::ContentMask::new(rect),
                ..Default::default()
            });
            scene.end_blur();
            scene.finish();
            scene
        };
        assert!(auto_capture_static_background(&build(8.0), size).is_none());
        let mut interleaved = crate::Scene::default();
        interleaved.begin_blur(crate::BlurCapture {
            animation_id: None,
            layer_id: Some(crate::GlobalElementId::from_path(&["panel".into()])),
            bounds: rect,
            content_mask: crate::ContentMask::new(rect),
            radius: crate::ScaledPixels(0.0),
            opacity: 1.0,
        });
        for _ in 0..30 {
            interleaved.insert_primitive(crate::Quad {
                bounds: rect,
                content_mask: crate::ContentMask::new(rect),
                ..Default::default()
            });
        }
        interleaved.end_blur();
        interleaved.finish();
        assert!(auto_capture_static_background(&interleaved, size).is_none(),
            "a prefix inside an open Capture is never independently replayable");
    }

    #[test]
    fn painter_order_distinguishes_static_background_from_interleaved_content() {
        let mut background = crate::Scene::default();
        let mut quad = crate::Quad::default();
        background.paint_operations.extend(
            (0..12).map(|_| crate::scene::PaintOperation::Primitive(crate::Primitive::Quad(quad))),
        );
        quad.animation_id = Some(crate::SceneAnimationId(42));
        background
            .paint_operations
            .push(crate::scene::PaintOperation::Primitive(
                crate::Primitive::Quad(quad),
            ));
        let split = classify_static_foreground(&background);
        assert_eq!(split.static_prefix, 12);
        assert_eq!(split.animated, 1);
        assert_eq!(split.static_after_dynamic, 0);
        assert!(split.has_contiguous_static_background());
        assert!(split.has_proven_static_majority());

        let mut interleaved = background;
        interleaved.paint_operations.extend((0..12).map(|_| {
            crate::scene::PaintOperation::Primitive(crate::Primitive::Quad(crate::Quad::default()))
        }));
        let mixed = classify_static_foreground(&interleaved);
        assert_eq!(mixed.static_after_dynamic, 12);
        assert!(!mixed.has_contiguous_static_background());
        assert!(!mixed.has_proven_static_majority());
    }

    #[test]
    fn animated_first_scene_must_not_trigger_full_window_auto_retention() {
        let mut scene = crate::Scene::default();
        let mut quad = crate::Quad::default();
        quad.animation_id = Some(crate::SceneAnimationId(9));
        scene
            .paint_operations
            .push(crate::scene::PaintOperation::Primitive(
                crate::Primitive::Quad(quad),
            ));
        scene.paint_operations.extend((0..512).map(|_| {
            crate::scene::PaintOperation::Primitive(crate::Primitive::Quad(crate::Quad::default()))
        }));
        let summary = FrameUploadSummary {
            quad_count: 513,
            ..Default::default()
        };
        assert!(!should_retain_complex_scene_color(&scene, &summary));
    }

    fn key(scene_revision: u64) -> UploadKey {
        UploadKey {
            scene_revision,
            atlas_placement_generation: 0,
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
                crate::GlobalElementId::from_path(&[name.into()]),
                generation,
            ),
            range,
            byte_hash: generation,
        }
    }

    #[test]
    fn complex_scene_color_cache_is_not_forced_on_small_ui() {
        let scene = crate::Scene::default();
        let mut summary = FrameUploadSummary::default();
        summary.quad_count = 48;
        assert!(!should_retain_complex_scene_color(&scene, &summary));

        summary.mono_sprite_count = 500;
        assert!(should_retain_complex_scene_color(&scene, &summary));

        // A filter scene already retains its isolated GPU results, so the
        // automatic full-window root must not duplicate that compositor.
        let mut scene_with_blur = crate::Scene::default();
        let bounds = crate::bounds(
            crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
            crate::size(crate::ScaledPixels(10.0), crate::ScaledPixels(10.0)),
        );
        scene_with_blur.insert_primitive(crate::PaintBackdropBlur {
            order: 0,
            animation_id: None,
            bounds,
            content_mask: crate::ContentMask::new(bounds),
            corner_radii: crate::Corners::default(),
            radius: crate::ScaledPixels(2.0),
            downsample: 1,
            levels: 1,
            saturation: 1.0,
            opacity: 1.0,
            tint: None,
            recompute_overlap: false,
        });
        summary.quad_count = 47;
        summary.mono_sprite_count = 0;
        summary.backdrop_blur_count = 1;
        assert!(!should_retain_complex_scene_color(
            &scene_with_blur,
            &summary
        ));
        summary.backdrop_blur_count = 3;
        assert!(!should_retain_complex_scene_color(
            &scene_with_blur,
            &summary
        ));
        // This is a topology decision, not an additional primitive threshold:
        // even a large filtered scene reuses its real filter/layer caches.
        summary.quad_count = 500;
        assert!(!should_retain_complex_scene_color(
            &scene_with_blur,
            &summary
        ));
        summary.quad_count = 8;
        assert!(!should_retain_complex_scene_color(
            &scene_with_blur,
            &summary
        ));
    }

    #[test]
    fn static_upload_is_tracked_per_stream_and_per_slot() {
        let mut retained = RetainedUpload::default();
        let mut upload = FrameUpload::default();
        upload
            .quads
            .write(|bytes| bytes.extend_from_slice(b"quad-a"));
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
        assert_eq!(
            retained.static_upload_mask(1).count(),
            StaticUploadMask::STREAM_COUNT
        );

        upload.quads.clear();
        upload
            .quads
            .write(|bytes| bytes.extend_from_slice(b"quad-b"));
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
        assert_eq!(
            retained.static_upload_mask(1).count(),
            StaticUploadMask::STREAM_COUNT
        );

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
    fn replacing_quad_buffer_forces_full_quad_refill_for_slot() {
        let mut retained = RetainedUpload::default();
        let mut upload = FrameUpload::default();
        upload
            .quads
            .write(|bytes| bytes.resize(PACKED_QUAD_BYTES * 2, 1));
        upload.resident_quad_spans = vec![
            resident_span("left", 1, 0..PACKED_QUAD_BYTES),
            resident_span("right", 1, PACKED_QUAD_BYTES..PACKED_QUAD_BYTES * 2),
        ];
        let (signature, _) = StaticUploadSignature::from_frame_upload(&upload);
        retained.replace(
            key(1),
            FrameUploadSummary::default(),
            2,
            signature,
            QuadResidentLayout::from_upload(&upload),
        );
        retained.mark_uploaded(0);
        retained.invalidate_quad_slot(0);

        assert!(retained.static_upload_mask(0).quad);
        assert_eq!(
            retained.quad_upload_plan(0, upload.quads.len()),
            QuadUploadPlan::Full
        );
    }

    #[test]
    fn replacing_path_buffer_invalidates_only_path_stream_for_slot() {
        let mut retained = RetainedUpload::default();
        let mut upload = FrameUpload::default();
        upload.quads.write(|bytes| bytes.extend_from_slice(b"quad"));
        upload
            .path_rasterization_vertices
            .extend_from_slice(b"path");
        let (signature, _) = StaticUploadSignature::from_frame_upload(&upload);
        retained.replace(
            key(1),
            FrameUploadSummary::default(),
            2,
            signature,
            QuadResidentLayout::from_upload(&upload),
        );
        retained.mark_uploaded(0);
        retained.invalidate_path_rasterization_slot(0);

        let dirty = retained.static_upload_mask(0);
        assert!(dirty.path_rasterization_vertex);
        assert!(!dirty.quad);
        assert_eq!(dirty.count(), 1);
    }

    #[test]
    fn retained_span_signature_hashes_only_dirty_bytes() {
        let mut upload = FrameUpload::default();
        let cached: Vec<_> = (32_u8..64).collect();
        let mut hasher = collections::FxHasher::default();
        hasher.write(&cached);
        upload.quads.write(|bytes| bytes.extend(0_u8..32));
        upload
            .quads
            .append_shared(Arc::new(cached), hasher.finish());
        upload.quads.write(|bytes| bytes.extend(64_u8..96));
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
        upload.quads.write(|bytes| bytes.resize(64, 1));
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

        upload.quads.slice_mut(0..32).fill(2);
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
            QuadUploadPlan::Ranges(smallvec::smallvec![0..32])
        );
        assert_eq!(
            retained.quad_upload_plan(1, upload.quads.len()),
            QuadUploadPlan::Full
        );
    }
}
