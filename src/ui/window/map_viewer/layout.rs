use gpui::{Bounds, Pixels, point, px, size};

pub const IDE_TOP_BAR_HEIGHT: f32 = 62.0;
pub const IDE_LEFT_STRIPE_WIDTH: f32 = 76.0;
pub const IDE_LEFT_DOCK_WIDTH: f32 = 276.0;
pub const IDE_SPLITTER_WIDTH: f32 = 6.0;
pub const IDE_DIVIDER_WIDTH: f32 = 1.0;
pub const IDE_STATUS_BAR_HEIGHT: f32 = 30.0;

// Chrome design tokens — unify the alpha layers scattered across the map_viewer
// chrome so top bar / docks / status bar share one consistent visual hierarchy.
// Three tiers: opaque-ish surface, semi-elevated controls, faint hairlines.
pub const CHROME_SURFACE_ALPHA: f32 = 0.92; // top bar / status bar / dock backgrounds
pub const CHROME_ELEVATED_ALPHA: f32 = 0.55; // buttons / badges / input fields
pub const CHROME_HAIRLINE_ALPHA: f32 = 0.16; // all dividers / borders
pub const CHROME_ICON_SIZE: f32 = 18.0;
pub const CHROME_TOOLBAR_ICON_SIZE: f32 = 16.0;
pub const CHROME_TAB_ICON_SIZE: f32 = 14.0;
pub const CHROME_SECTION_GAP: f32 = 14.0;
pub const CHROME_ACTIVE_RAIL_WIDTH: f32 = 2.0; // left accent rail on active stripe tab

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TopToolbarLayout {
    pub title_width: f32,
    pub show_modes: bool,
    pub show_y_controls: bool,
    pub show_zoom_controls: bool,
    pub overflow_count: usize,
}

#[must_use]
pub fn top_toolbar_layout(window_width: f32) -> TopToolbarLayout {
    TopToolbarLayout {
        title_width: if window_width < 1_080.0 { 144.0 } else { 184.0 },
        show_modes: window_width >= 1_040.0,
        show_y_controls: window_width >= 920.0,
        show_zoom_controls: window_width >= 920.0,
        overflow_count: toolbar_overflow_count(window_width),
    }
}

#[must_use]
pub fn toolbar_overflow_count(window_width: f32) -> usize {
    let mut count = 0;
    if window_width < 1_040.0 {
        count += 5;
    }
    if window_width < 920.0 {
        count += 2;
        count += 2;
    }
    count
}

#[must_use]
pub fn center_stage_rect_for_layout(
    window_width: f32,
    window_height: f32,
    left_open: bool,
    right_open: bool,
    right_width: f32,
    bottom_open: bool,
    bottom_height: f32,
    min_center_width: f32,
    min_center_height: f32,
) -> Bounds<Pixels> {
    let mut left = IDE_LEFT_STRIPE_WIDTH + IDE_DIVIDER_WIDTH;
    if left_open {
        left += IDE_LEFT_DOCK_WIDTH + IDE_DIVIDER_WIDTH;
    }

    let mut width = window_width - left;
    if right_open {
        width -= right_width + IDE_SPLITTER_WIDTH;
    }
    let mut height = window_height - IDE_TOP_BAR_HEIGHT - IDE_STATUS_BAR_HEIGHT;
    if bottom_open {
        height -= bottom_height + IDE_SPLITTER_WIDTH;
    }

    Bounds::new(
        point(px(left), px(IDE_TOP_BAR_HEIGHT)),
        size(
            px(width.max(min_center_width)),
            px(height.max(min_center_height)),
        ),
    )
}

#[must_use]
#[cfg_attr(not(test), allow(dead_code))]
pub fn hud_stack_rects(
    viewport_width: f32,
    _viewport_height: f32,
    ruler_visible: bool,
) -> (Option<Bounds<Pixels>>, Bounds<Pixels>) {
    let right = 16.0;
    let top = 16.0;
    let gap = 8.0;
    let ruler_size = size(px(128.0), px(24.0));
    let coord_size = size(px(190.0), px(34.0));
    let coord_origin = point(
        px((viewport_width - right - coord_size.width / px(1.0)).max(0.0)),
        px(top
            + if ruler_visible {
                ruler_size.height / px(1.0) + gap
            } else {
                0.0
            }),
    );
    let coord = Bounds::new(coord_origin, coord_size);
    let ruler = ruler_visible.then(|| {
        Bounds::new(
            point(
                px((viewport_width - right - ruler_size.width / px(1.0)).max(0.0)),
                px(top),
            ),
            ruler_size,
        )
    });
    (ruler, coord)
}
