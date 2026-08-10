use super::model::{
    EntityOverlayPoint, MapViewport, ProfessionalOverlayPaintCache, entity_cache_short_label,
};
use super::paint::{overlay_marker_screen_x, overlay_marker_screen_y};
use super::prelude::*;
use std::collections::HashSet;

const MAX_ENTITY_DEBUG_LABELS: usize = 128;
const LABEL_CELL_WIDTH: f32 = 150.0;
const LABEL_CELL_HEIGHT: f32 = 22.0;

pub(super) struct EntityDebugPaintContext<'a> {
    pub(super) bounds: Bounds<Pixels>,
    pub(super) viewport: MapViewport,
    pub(super) layout: RenderLayout,
    pub(super) overlay_paint: &'a ProfessionalOverlayPaintCache,
    pub(super) entity_avatar_pool: &'a BTreeMap<String, Arc<RenderImage>>,
}

pub(super) fn paint_missing_entity_avatar_labels(
    context: EntityDebugPaintContext<'_>,
    window: &mut Window,
    cx: &mut App,
) {
    let mut occupied_cells = HashSet::new();
    let mut painted = 0usize;
    for entity in &context.overlay_paint.entity_points {
        if entity
            .avatar_key
            .as_ref()
            .is_some_and(|key| context.entity_avatar_pool.contains_key(key))
        {
            continue;
        }
        let Some((cell, screen_position)) = debug_label_position(&context, entity) else {
            continue;
        };
        if !occupied_cells.insert(cell) {
            continue;
        }
        paint_debug_label(
            entity_debug_label(entity, !context.entity_avatar_pool.is_empty()),
            context.bounds,
            screen_position,
            window,
            cx,
        );
        painted = painted.saturating_add(1);
        if painted >= MAX_ENTITY_DEBUG_LABELS {
            break;
        }
    }
}

fn debug_label_position(
    context: &EntityDebugPaintContext<'_>,
    entity: &EntityOverlayPoint,
) -> Option<((i32, i32), (f32, f32))> {
    let screen_x = overlay_marker_screen_x(
        context.bounds,
        context.viewport,
        context.layout,
        entity.block_x,
    );
    let screen_y = overlay_marker_screen_y(
        context.bounds,
        context.viewport,
        context.layout,
        entity.block_z,
    );
    if !debug_label_position_is_visible(context.bounds, screen_x, screen_y) {
        return None;
    }
    let cell = (
        ((screen_x - context.bounds.left() / px(1.0)) / LABEL_CELL_WIDTH).floor() as i32,
        ((screen_y - context.bounds.top() / px(1.0)) / LABEL_CELL_HEIGHT).floor() as i32,
    );
    Some((cell, (screen_x, screen_y)))
}

fn debug_label_position_is_visible(bounds: Bounds<Pixels>, x: f32, y: f32) -> bool {
    x.is_finite()
        && y.is_finite()
        && x >= bounds.left() / px(1.0)
        && y >= bounds.top() / px(1.0)
        && x <= bounds.right() / px(1.0)
        && y <= bounds.bottom() / px(1.0)
}

fn entity_debug_label(entity: &EntityOverlayPoint, avatar_pool_loaded: bool) -> SharedString {
    let identifier = entity.identifier.as_deref().unwrap_or("<missing-id>");
    let avatar_status = if avatar_pool_loaded {
        "avatar:missing"
    } else {
        "avatar:loading"
    };
    SharedString::from(format!(
        "{identifier} · {avatar_status} · {}",
        entity_cache_short_label(entity.cache_status)
    ))
}

fn paint_debug_label(
    text: SharedString,
    bounds: Bounds<Pixels>,
    screen_position: (f32, f32),
    window: &mut Window,
    cx: &mut App,
) {
    let (screen_x, screen_y) = screen_position;
    let text_style = window.text_style();
    let line = window.text_system().shape_line(
        text.clone(),
        px(10.0),
        &[TextRun {
            len: text.len(),
            font: Font {
                family: text_style.font_family,
                features: text_style.font_features,
                fallbacks: text_style.font_fallbacks,
                weight: FontWeight::MEDIUM,
                style: text_style.font_style,
            },
            color: rgb(0xffffff).into(),
            background_color: Some(Hsla {
                a: 0.84,
                ..rgb(0x111827).into()
            }),
            background_corner_radius: Some(px(3.0)),
            background_padding: Some(TextBackgroundPadding {
                top: px(1.0),
                right: px(3.0),
                bottom: px(1.0),
                left: px(3.0),
            }),
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    let minimum_x = bounds.left() + px(3.0);
    let maximum_x = (bounds.right() - line.width - px(4.0)).max(minimum_x);
    let origin = point(
        (px(screen_x) + px(7.0)).clamp(minimum_x, maximum_x),
        (px(screen_y) + px(5.0)).clamp(bounds.top() + px(2.0), bounds.bottom() - px(16.0)),
    );
    if let Err(error) = line.paint(origin, px(14.0), window, cx) {
        tracing::debug!(?error, "failed to paint entity debug label");
    }
}
