use super::*;

impl FrameUpload {
    pub(super) fn encode_backdrop_blurs(
        &mut self,
        blurs: &[crate::PaintBackdropBlur],
        drawable_size: DrawableSize,
        quality: BackdropBlurQuality,
        summary: &mut FrameUploadSummary,
    ) {
        if quality == BackdropBlurQuality::Disabled {
            self.encode_backdrop_tints(blurs, summary);
            return;
        }

        let first = (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
        let mut count = 0_u32;
        for blur in blurs {
            if self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES >= MAX_BACKDROP_BLURS {
                break;
            }
            let Some(blur) = quality.adjusted_blur(blur) else {
                continue;
            };
            let blur = blur.as_ref();
            let primitive_index = (self.backdrop_blurs.len() / PACKED_BACKDROP_BLUR_BYTES) as u32;
            write_backdrop_blur(&mut self.backdrop_blurs, blur, drawable_size);
            register_scene_animated_primitive(
                self,
                summary,
                blur.animation_id
                    .map(|_| crate::Primitive::BackdropBlur(blur.clone())),
                AnimatedPrimitiveKind::BackdropBlur,
                primitive_index,
            );
            count = count.saturating_add(1);
        }
        if count > 0 {
            self.batches
                .push(UploadedBatch::BackdropBlurs { first, count });
            summary.backdrop_blur_count = summary.backdrop_blur_count.saturating_add(count);
        }
    }

    fn encode_backdrop_tints(
        &mut self,
        blurs: &[crate::PaintBackdropBlur],
        summary: &mut FrameUploadSummary,
    ) {
        let first = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
        let mut count = 0_u32;
        for blur in blurs {
            if self.quads.len() / PACKED_QUAD_BYTES >= MAX_QUADS {
                break;
            }
            let Some(mut tint) = blur.tint.filter(|tint| !tint.is_transparent()) else {
                continue;
            };
            tint.a *= blur.opacity;
            let primitive_index = (self.quads.len() / PACKED_QUAD_BYTES) as u32;
            let quad = Quad {
                order: blur.order,
                border_style: crate::BorderStyle::Solid,
                animation_id: blur.animation_id,
                bounds: blur.bounds,
                content_mask: blur.content_mask,
                background: tint.into(),
                border_color: crate::Hsla::transparent_black().into(),
                corner_radii: blur.corner_radii,
                border_widths: Default::default(),
            };
            self.quads.write(|bytes| write_quad(bytes, &quad));
            register_scene_animated_primitive(
                self,
                summary,
                quad.animation_id.map(|_| crate::Primitive::Quad(quad)),
                AnimatedPrimitiveKind::Quad,
                primitive_index,
            );
            count = count.saturating_add(1);
        }
        if count > 0 {
            self.batches.push(UploadedBatch::Quads { first, count });
            summary.quad_count = summary.quad_count.saturating_add(count);
        }
    }
}
