use super::*;

/// Emits the *first* failed admission guard for an unsafe static prefix.
/// Do not turn an unknown/unsupported paint operation into an accepted cache:
/// retaining an invalid source would reuse stale GPU pixels.
fn reject_auto_static_capture(scene: &crate::Scene, reason: &'static str) -> Option<StaticPrefix> {
    log::trace!(
        target: "gpui::diagnostics",
        "GPUI AutoStatic admission rejected: scene_revision={} reason={} paint_operations={} blur_captures={} full_redraw_fallback={}",
        scene.revision,
        reason,
        scene.paint_operations.len(),
        scene.blurs.len(),
        crate::paint::requires_full_redraw_fallback(scene),
    );
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_chunks_allow_capture_only_outside_the_split() {
        let mut source = crate::window::retained::PaintFixture::default();
        let viewport = crate::bounds(
            crate::point(crate::ScaledPixels(0.), crate::ScaledPixels(0.)),
            crate::size(crate::ScaledPixels(64.), crate::ScaledPixels(64.)),
        );
        for _ in 0..128 {
            source.insert_primitive(crate::Quad {
                bounds: viewport,
                content_mask: crate::ContentMask::new(viewport),
                ..Default::default()
            });
        }
        source.record(
            crate::GlobalElementId::from_path(&["static".into()]),
            1,
            0..128,
        );
        source.insert_animated_primitive(
            crate::Quad {
                bounds: viewport,
                content_mask: crate::ContentMask::new(viewport),
                ..Default::default()
            },
            crate::SceneAnimationId(1),
        );
        source.finish();
        let chunks = source.chunks();
        assert_eq!(chunks.len(), 1);
        let size = DrawableSize {
            width: 64,
            height: 64,
        };
        assert!(auto_capture_static_background(&source, chunks, size).is_some());
        let mut crossing = chunks.to_vec();
        crossing[0].paint_range.end = source.len();
        assert!(auto_capture_static_background(&source, &crossing, size).is_none());
    }
}

pub(super) fn auto_capture_static_background(
    scene: &crate::Scene,
    retained_chunks: &[crate::PreparedRetainedQuadChunk],
    drawable_size: DrawableSize,
) -> Option<StaticPrefix> {
    use crate::{Primitive, scene::PaintOperation};

    // Preserve resource/capture guards. Chunk residency does not change source
    // pixels; its boundary is checked separately below. Atlas inputs, nested
    // filters and background blur still require their own dependency proofs.
    let rejection = if scene.revision == 0 {
        Some("uncommitted_scene")
    } else if scene.has_backdrop_blurs() {
        Some("backdrop_blur_present")
    } else if crate::paint::requires_full_redraw_fallback(scene) {
        Some("scene_requires_full_redraw")
    } else if scene.paint_operations.len() < 25 {
        Some("insufficient_paint_operations")
    } else if drawable_size.width == 0 || drawable_size.height == 0 {
        Some("empty_drawable")
    } else if scene.blurs.iter().any(|blur| blur.radius.0 != 0.0) {
        Some("gaussian_element_capture_present")
    } else if scene.blurs.iter().any(|blur| blur.layer_id.is_none()) {
        Some("unstable_capture_identity")
    } else if scene
        .blurs
        .iter()
        .any(|blur| !blur.content.blurs.is_empty())
    {
        Some("nested_element_capture")
    } else {
        None
    };
    if let Some(reason) = rejection {
        return reject_auto_static_capture(scene, reason);
    }

    let viewport = crate::bounds(
        crate::point(crate::ScaledPixels(0.0), crate::ScaledPixels(0.0)),
        crate::size(
            crate::ScaledPixels(drawable_size.width as f32),
            crate::ScaledPixels(drawable_size.height as f32),
        ),
    );

    // Split only at a complete painter-order boundary. StartBlur begins a
    // dynamic capture; its composite order lives in Scene::blurs rather than
    // the child primitives recorded between StartBlur and EndBlur.
    let mut static_prefix = 0usize;
    let mut static_quads = 0usize;
    let mut static_shadows = 0usize;
    let mut static_raster_area = 0.0_f64;
    let mut prefix_end = 0usize;
    let mut saw_dynamic = false;
    for (index, operation) in scene.paint_operations.iter().enumerate() {
        let primitive = match operation {
            PaintOperation::Primitive(primitive) => primitive,
            PaintOperation::StartLayer(_) | PaintOperation::EndLayer => continue,
            PaintOperation::StartBlur(capture)
                if capture.radius.0 == 0.0 && capture.layer_id.is_some() =>
            {
                if !saw_dynamic {
                    prefix_end = index;
                    saw_dynamic = true;
                }
                continue;
            }
            PaintOperation::StartBlur(_) => {
                return reject_auto_static_capture(scene, "unsupported_capture_boundary");
            }
            PaintOperation::EndBlur => continue,
        };
        match primitive {
            Primitive::Quad(_) | Primitive::Shadow(_) => {}
            _ if !saw_dynamic => {
                // A sprite/atlas dependency cannot become a static GPU texture
                // without versioning its atlas placement/content first.
                let kind = match primitive {
                    Primitive::Path(_) => "path",
                    Primitive::Underline(_) => "underline",
                    Primitive::MonochromeSprite(_) => "monochrome_sprite",
                    Primitive::PolychromeSprite(_) => "polychrome_sprite",
                    Primitive::Surface(_) => "native_surface",
                    Primitive::BackdropBlur(_) => "backdrop_blur",
                    Primitive::Blur(_) => "blur",
                    Primitive::RendererExtension(_) => "renderer_extension",
                    Primitive::Quad(_) | Primitive::Shadow(_) => "quad_or_shadow",
                };
                log::trace!(
                    target: "gpui::diagnostics",
                    "GPUI AutoStatic prefix blocker: scene_revision={} operation_index={} kind={} static_prefix={}",
                    scene.revision, index, kind, static_prefix,
                );
                return reject_auto_static_capture(scene, "non_quad_or_shadow_static_prefix");
            }
            Primitive::Surface(_)
            | Primitive::RendererExtension(_)
            | Primitive::BackdropBlur(_) => {
                return reject_auto_static_capture(scene, "unsupported_foreground_primitive");
            }
            Primitive::Blur(blur) if blur.radius.0 == 0.0 && blur.layer_id.is_some() => {}
            Primitive::Blur(_) => {
                return reject_auto_static_capture(scene, "unsupported_blur_composite");
            }
            _ => {}
        }
        if !saw_dynamic && primitive.animation_id().is_none() {
            static_prefix += 1;
            let (mask, count) = match primitive {
                Primitive::Quad(quad) => (&quad.content_mask, &mut static_quads),
                Primitive::Shadow(shadow) => (&shadow.content_mask, &mut static_shadows),
                _ => unreachable!("the static prefix contains only Quads and Shadows"),
            };
            if !(mask.bounds.size.width.0 <= 0.0 || mask.bounds.size.height.0 <= 0.0) {
                *count += 1;
            }
            let bounds = primitive.visual_bounds().intersect(&viewport);
            static_raster_area +=
                f64::from(bounds.size.width.0.max(0.0)) * f64::from(bounds.size.height.0.max(0.0));
        } else {
            if !saw_dynamic {
                prefix_end = index;
            }
            saw_dynamic = true;
        }
    }

    // Preserve the existing 96-primitive flat-scene admission. For smaller
    // pages, only pay for an extra retained target when measured static raster
    // coverage is substantial; 24 tiny controls are not enough by themselves.
    let viewport_area = f64::from(drawable_size.width) * f64::from(drawable_size.height);
    if static_prefix < 96 && (static_prefix < 24 || static_raster_area < viewport_area * 2.0) {
        return reject_auto_static_capture(scene, "insufficient_static_prefix_coverage");
    }
    if !saw_dynamic {
        return reject_auto_static_capture(scene, "no_dynamic_foreground");
    }
    if retained_chunks
        .iter()
        .any(|chunk| chunk.paint_range.start < prefix_end && prefix_end < chunk.paint_range.end)
    {
        return reject_auto_static_capture(scene, "prefix_splits_retained_chunk");
    }
    let structure = crate::window::retained::ReplayStructureIndex::new(scene);
    if !structure.range_is_replayable(scene, 0..prefix_end) {
        return reject_auto_static_capture(scene, "prefix_not_independently_replayable");
    }
    if !structure.range_is_replayable(scene, prefix_end..scene.paint_operations.len()) {
        return reject_auto_static_capture(scene, "foreground_not_independently_replayable");
    }

    // The static prefix is Quad/Shadow-only; a zero-filter popup contributes
    // its output as a later Primitive::Blur, never as a member of the prefix.
    // Keep its draw-order barrier intact to avoid moving a popup behind chrome.
    let mut foreground_orders =
        top_level_orders(scene, scene.paint_operations.range(prefix_end..scene.len()));
    let Some(first_dynamic_order) = foreground_orders.clone().next().flatten() else {
        return reject_auto_static_capture(scene, "foreground_without_draw_order");
    };
    if scene
        .paint_operations
        .range(0..prefix_end)
        .iter()
        .any(|operation| {
            matches!(operation, PaintOperation::Primitive(primitive)
            if primitive.order() >= first_dynamic_order)
        })
        || foreground_orders.any(|order| order.is_none_or(|order| order < first_dynamic_order))
    {
        return reject_auto_static_capture(scene, "interleaved_painter_order");
    }

    // The tight background target covers only static pixels. Popup source
    // Capture and its changing ClipReveal composite stay in the main pass.
    let Some(bounds) = scene.bounds_for_range(0..prefix_end) else {
        return reject_auto_static_capture(scene, "prefix_without_bounds");
    };
    let bounds = bounds.intersect(&viewport);
    if bounds.is_empty() {
        return reject_auto_static_capture(scene, "empty_static_bounds");
    }

    // One finalized Scene, one encoding. Capture only the boundary here:
    // the GPU encoder fingerprints the static packed source without Replay.
    Some(StaticPrefix {
        quads: static_quads,
        shadows: static_shadows,
        bounds,
        content_mask: crate::ContentMask::new(viewport),
    })
}

/// Capture-local orders belong to the child Scene. Resolve each outer capture's
/// unique stable identity against the parent's actual composite. An unresolved
/// identity rejects admission. The caller has already proved balanced captures.
fn top_level_orders<'a>(
    scene: &'a crate::Scene,
    operations: crate::scene::OperationRange<'a>,
) -> impl Iterator<Item = Option<crate::DrawOrder>> + Clone + 'a {
    use crate::scene::PaintOperation;
    let mut capture_depth = 0usize;
    operations
        .iter()
        .filter_map(move |operation| match operation {
            PaintOperation::StartBlur(capture) => {
                let outermost = capture_depth == 0;
                capture_depth += 1;
                if !outermost {
                    return None;
                }
                let mut composites = scene
                    .blurs
                    .iter()
                    .filter(|blur| blur.layer_id == capture.layer_id);
                let composite = composites.next();
                Some(if composites.next().is_none() {
                    composite.map(|blur| blur.order)
                } else {
                    None
                })
            }
            PaintOperation::EndBlur => {
                capture_depth -= 1;
                None
            }
            PaintOperation::Primitive(primitive) if capture_depth == 0 => {
                Some(Some(primitive.order()))
            }
            _ => None,
        })
}
