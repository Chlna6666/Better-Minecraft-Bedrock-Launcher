use crate::music::{MusicDragTarget, MusicPlaybackMode, MusicSnapshot, MusicState};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::time::Instant;

fn icon_path(path: &'static str) -> Svg {
    svg().path(path)
}

fn format_time(seconds: f32) -> SharedString {
    let seconds = seconds.max(0.0).floor() as u32;
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    format!("{minutes}:{seconds:02}").into()
}

pub struct MusicPlayerRender {
    pub inline: AnyElement,
    pub overlay: Option<AnyElement>,
    pub backdrop: Option<AnyElement>,
}

const POPUP_WIDTH: f32 = 324.0;
const POPUP_HEIGHT: f32 = 164.0;
const POPUP_PAD: f32 = 14.0;
const COVER_SIZE: f32 = 78.0;
const COVER_RADIUS: f32 = 16.0;
const POPUP_GAP: f32 = 10.0;
const LEFT_COLUMN_WIDTH: f32 = COVER_SIZE;
const RIGHT_COLUMN_WIDTH: f32 = POPUP_WIDTH - POPUP_PAD * 2.0 - LEFT_COLUMN_WIDTH - POPUP_GAP;
const META_TEXT_WIDTH: f32 = RIGHT_COLUMN_WIDTH;
const ERROR_TEXT_WIDTH: f32 = RIGHT_COLUMN_WIDTH;
const PROGRESS_TRACK_WIDTH: f32 = RIGHT_COLUMN_WIDTH;
const PROGRESS_TRACK_HEIGHT: f32 = 6.0;
const PROGRESS_HIT_HEIGHT: f32 = 18.0;
const VOLUME_TRACK_WIDTH: f32 = 56.0;
const VOLUME_TRACK_HEIGHT: f32 = 6.0;
const VOLUME_HIT_HEIGHT: f32 = 18.0;
const SLIDER_THUMB_SIZE: f32 = 14.0;
const MINI_CAPSULE_COLLAPSED_WIDTH: f32 = 36.0;
const MINI_CAPSULE_EXPANDED_WIDTH: f32 = 220.0;
const MINI_CAPSULE_EXPANDED_GAP: f32 = 8.0;
const MINI_CAPSULE_SIDE_PAD: f32 = 10.0;
const MINI_CAPSULE_COMPACT_PAD: f32 = 4.0;
const MINI_COVER_SIZE: f32 = 20.0;
const MINI_PLAY_BUTTON_SIZE: f32 = 24.0;
const MINI_BODY_SAFE_RIGHT_INSET: f32 = 4.0;

fn marquee_text(
    text: SharedString,
    visible_width: Pixels,
    font_size: f32,
    line_height: f32,
    color: Hsla,
    weight: FontWeight,
    opacity: f32,
    _speed: f32,
) -> impl IntoElement {
    clipped_text(
        text,
        visible_width,
        font_size,
        line_height,
        color,
        weight,
        opacity,
    )
}

fn clipped_text(
    text: SharedString,
    visible_width: Pixels,
    font_size: f32,
    line_height: f32,
    color: Hsla,
    weight: FontWeight,
    opacity: f32,
) -> impl IntoElement {
    div()
        .w(visible_width)
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(font_size))
        .line_height(px(line_height))
        .font_weight(weight)
        .text_color(color)
        .opacity(opacity)
        .child(text)
}

fn popup_cover(content: AnyElement) -> impl IntoElement {
    div()
        .w(px(COVER_SIZE))
        .h(px(COVER_SIZE))
        .flex_none()
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(COVER_RADIUS))
        .child(
            div()
                .size_full()
                .overflow_hidden()
                .rounded(px(COVER_RADIUS))
                .child(content),
        )
}

fn inline_cover(content: AnyElement) -> impl IntoElement {
    div()
        .w(px(MINI_COVER_SIZE))
        .h(px(MINI_COVER_SIZE))
        .flex_shrink_0()
        .overflow_hidden()
        .rounded_full()
        .child(
            div()
                .size_full()
                .overflow_hidden()
                .rounded_full()
                .child(content),
        )
}

pub fn mini_capsule_width(window_width: Pixels, available: bool) -> Pixels {
    if !available {
        return px(0.0);
    }
    if window_width / px(1.0) >= 1210.0 {
        px(MINI_CAPSULE_EXPANDED_WIDTH)
    } else {
        px(MINI_CAPSULE_COLLAPSED_WIDTH)
    }
}

pub fn mini_capsule_width_for_factor(available: bool, factor: f32) -> Pixels {
    if !available {
        return px(0.0);
    }

    let factor = factor.clamp(0.0, 1.06);
    px(MINI_CAPSULE_COLLAPSED_WIDTH
        + (MINI_CAPSULE_EXPANDED_WIDTH - MINI_CAPSULE_COLLAPSED_WIDTH) * factor)
}

#[allow(clippy::too_many_arguments)]
pub fn render_music_player(
    snapshot: MusicSnapshot,
    expanded_factor: f32,
    displayed_progress_ratio: f32,
    displayed_volume_ratio: f32,
    drag_target: Option<MusicDragTarget>,
    inline_factor: f32,
    window_width: Pixels,
    popup_top: Pixels,
    popup_right: Pixels,
    accent: Hsla,
    text_color: Hsla,
    border_color: Hsla,
    capsule_bg: Hsla,
    popup_bg: Hsla,
    muted_bg: Hsla,
) -> MusicPlayerRender {
    let inline_visual_factor = inline_factor.clamp(0.0, 1.06);
    let inline_width = mini_capsule_width_for_factor(snapshot.available, inline_visual_factor);
    if !snapshot.available {
        return MusicPlayerRender {
            inline: div().w(px(0.0)).h(px(0.0)).into_any_element(),
            overlay: None,
            backdrop: None,
        };
    }

    let inline_k_raw = inline_visual_factor.clamp(0.0, 1.0);
    // 在收起/展开端点做吸附，避免浮点尾差让“圆形态”残留半截内容。
    let inline_k = if inline_k_raw <= 0.04 {
        0.0
    } else if inline_k_raw >= 0.995 {
        1.0
    } else {
        inline_k_raw
    };
    // 内容显隐使用独立进度，延后到胶囊明显拉开后再出现，防止按钮溢出。
    let content_k = ((inline_k - 0.12) / 0.88).clamp(0.0, 1.0);
    let compact = inline_k <= 0.12;
    let mini_text_opacity = content_k;
    let mini_outer_gap = px(MINI_CAPSULE_EXPANDED_GAP * content_k);
    let mini_side_pad =
        MINI_CAPSULE_COMPACT_PAD + (MINI_CAPSULE_SIDE_PAD - MINI_CAPSULE_COMPACT_PAD) * inline_k;
    // Use the real expanded inline layout budget:
    // [left pad] + [cover] + [outer gap] + [body] + [right pad] = expanded width.
    // This avoids the play button overflowing when side paddings differ between
    // collapsed and expanded states.
    let mini_body_full_width = (MINI_CAPSULE_EXPANDED_WIDTH
        - MINI_CAPSULE_SIDE_PAD * 2.0
        - MINI_COVER_SIZE
        - MINI_CAPSULE_EXPANDED_GAP
        - MINI_BODY_SAFE_RIGHT_INSET)
        .max(0.0);
    let mini_body_width = px(mini_body_full_width * content_k);
    let mini_body_offset = px(14.0 * (1.0 - content_k));
    let mini_label_max_width =
        (mini_body_full_width - MINI_CAPSULE_EXPANDED_GAP - MINI_PLAY_BUTTON_SIZE).max(0.0);
    let mini_label_space = px(mini_label_max_width);
    let mini_label_offset = px(-10.0 * (1.0 - content_k));
    let mini_button_opacity = (0.15 + content_k * 0.85).clamp(0.0, 1.0);
    let mini_button_offset = px(8.0 * (1.0 - content_k));
    let progress_ratio = displayed_progress_ratio.clamp(0.0, 1.0);
    let volume_ratio = displayed_volume_ratio.clamp(0.0, 1.0);
    let preview_current_seconds = if snapshot.total_seconds <= 0.0 {
        snapshot.current_seconds
    } else {
        snapshot.total_seconds * progress_ratio
    };
    let popup_opacity = expanded_factor.clamp(0.0, 1.0);
    let popup_visible = popup_opacity > 0.001 || snapshot.expanded;
    let popup_content_live = popup_opacity >= 0.97;
    let popup_slide = (1.0 - popup_opacity).powf(1.15);
    let popup_scale = 0.965 + popup_opacity * 0.035;
    let popup_width = px(POPUP_WIDTH);
    let popup_height = px(POPUP_HEIGHT);
    let popup_inner_padding = px(POPUP_PAD);
    let popup_offset = px(24.0 * popup_slide);
    let popup_left = window_width - popup_right - popup_width;
    let popup_y = popup_top - popup_offset;
    let content_left = popup_left + px(POPUP_PAD + LEFT_COLUMN_WIDTH + POPUP_GAP);
    let slider_top = popup_y + px(86.0);
    let popup_inner_width = POPUP_WIDTH - POPUP_PAD * 2.0;
    let bottom_row_width = 24.0 * 4.0 + 28.0 + VOLUME_TRACK_WIDTH + 24.0 + 6.0 * 6.0;
    let bottom_row_left = popup_left
        + popup_inner_padding
        + px((popup_inner_width - bottom_row_width).max(0.0) / 2.0);
    let volume_left = bottom_row_left + px(154.0);
    let bottom_row_top = popup_y + popup_height - popup_inner_padding - px(28.0);
    let progress_bounds = Bounds::new(
        point(content_left, slider_top),
        size(px(PROGRESS_TRACK_WIDTH), px(PROGRESS_HIT_HEIGHT)),
    );
    let volume_bounds = Bounds::new(
        point(
            volume_left,
            bottom_row_top + px((28.0 - VOLUME_HIT_HEIGHT) / 2.0),
        ),
        size(px(VOLUME_TRACK_WIDTH), px(VOLUME_HIT_HEIGHT)),
    );

    let render_cover = || {
        snapshot
            .cover_render_image
            .clone()
            .map(|render_image| {
                img(render_image)
                    .w_full()
                    .h_full()
                    .rounded(px(COVER_RADIUS))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element()
            })
            .unwrap_or_else(|| {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(text_color.opacity(0.7))
                    .child(
                        icon_path(lucide_icons::icon_disc())
                            .size(px(28.0))
                            .text_color(text_color.opacity(0.7)),
                    )
                    .into_any_element()
            })
    };

    let icon_button = |icon_path_value: &'static str, active: bool, size: f32| {
        div()
            .w(px(24.0))
            .h(px(24.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .text_color(if active {
                text_color
            } else {
                text_color.opacity(0.92)
            })
            .bg(if active {
                Hsla { a: 0.9, ..accent }
            } else {
                muted_bg.opacity(0.6)
            })
            .border_1()
            .border_color(border_color.opacity(if active { 0.0 } else { 0.12 }))
            .child(
                icon_path(icon_path_value)
                    .size(px(size))
                    .text_color(if active {
                        text_color
                    } else {
                        text_color.opacity(0.92)
                    }),
            )
    };

    let inline_play_button = div()
        .w(px(MINI_PLAY_BUTTON_SIZE))
        .h(px(MINI_PLAY_BUTTON_SIZE))
        .rounded_full()
        .bg(rgb(0xffffff))
        .text_color(rgb(0x111111))
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .opacity(mini_button_opacity)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx: &mut App| {
            cx.stop_propagation();
            let now = Instant::now();
            cx.update_global(|music: &mut MusicState, cx: &mut App| {
                music.toggle_playback(now);
            });
        })
        .child(
            icon_path(if snapshot.is_playing {
                lucide_icons::icon_pause()
            } else {
                lucide_icons::icon_play()
            })
            .size(px(11.0))
            .text_color(rgb(0x111111)),
        );

    let inline = div()
        .w(inline_width)
        .h(px(36.0))
        .rounded(if compact { px(18.0) } else { px(20.0) })
        .px(px(mini_side_pad))
        .py(px(4.0))
        .flex()
        .items_center()
        .when(compact, |this: Div| this.justify_center())
        .gap(mini_outer_gap)
        .overflow_hidden()
        .bg(capsule_bg)
        .border_1()
        .border_color(border_color)
        .cursor_pointer()
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, _, cx| {
            cx.stop_propagation();
            let now = Instant::now();
            cx.update_global(|music: &mut MusicState, cx| {
                let popup_visible = music.snapshot.expanded
                    || music.popup_animating(now)
                    || music.expanded_factor(now) > 0.001;
                music.set_expanded(!popup_visible, now);
            });
        })
        .child(inline_cover(
            snapshot
                .cover_render_image
                .clone()
                .map(|render_image| {
                    img(render_image)
                        .w_full()
                        .h_full()
                        .rounded_full()
                        .object_fit(ObjectFit::Cover)
                        .into_any_element()
                })
                .unwrap_or_else(|| {
                    div()
                        .size_full()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(text_color.opacity(0.7))
                        .child(
                            icon_path(lucide_icons::icon_disc())
                                .size(px(14.0))
                                .text_color(text_color.opacity(0.7)),
                        )
                        .into_any_element()
                }),
        ))
        .child(
            div()
                .w(mini_body_width)
                .min_w(px(0.0))
                .overflow_hidden()
                .opacity(content_k)
                .child(
                    div()
                        .relative()
                        .left(mini_body_offset)
                        .w(px(mini_body_full_width))
                        .flex()
                        .items_center()
                        .gap(px(MINI_CAPSULE_EXPANDED_GAP))
                        .child(
                            div()
                                .w(mini_label_space)
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .child(
                                    div()
                                        .relative()
                                        .left(mini_label_offset)
                                        .opacity(mini_text_opacity)
                                        .child(clipped_text(
                                            snapshot.title.clone(),
                                            mini_label_space,
                                            12.0,
                                            16.0,
                                            text_color,
                                            FontWeight::SEMIBOLD,
                                            mini_text_opacity,
                                        )),
                                ),
                        )
                        .child(
                            div()
                                .relative()
                                .left(mini_button_offset)
                                .child(inline_play_button),
                        ),
                ),
        )
        .into_any_element();

    let overlay = if popup_visible {
        let drag_ratio = move |target: MusicDragTarget, position: Point<Pixels>| {
            let bounds = match target {
                MusicDragTarget::Progress => progress_bounds,
                MusicDragTarget::Volume => volume_bounds,
            };
            ((position.x - bounds.left()) / bounds.size.width).clamp(0.0, 1.0)
        };

        let begin_drag = move |target: MusicDragTarget, position: Point<Pixels>, cx: &mut App| {
            let ratio = drag_ratio(target, position);
            cx.update_global(|music: &mut MusicState, cx| {
                music.begin_drag(target);
                let _ = music.update_drag_ratio(ratio);
            });
        };

        let update_drag = move |target: MusicDragTarget, position: Point<Pixels>, cx: &mut App| {
            let ratio = drag_ratio(target, position);
            cx.update_global(|music: &mut MusicState, cx| {
                let _ = music.update_drag_ratio(ratio);
            });
        };

        let commit_and_clear_drag = move |cx: &mut App| {
            let now = Instant::now();
            cx.update_global(|music: &mut MusicState, cx| {
                music.commit_drag(now, cx);
                music.clear_drag();
            });
        };

        let progress_fill_width = progress_ratio * PROGRESS_TRACK_WIDTH;
        let progress_thumb_center = progress_fill_width.clamp(
            SLIDER_THUMB_SIZE / 2.0,
            PROGRESS_TRACK_WIDTH - SLIDER_THUMB_SIZE / 2.0,
        );
        let progress_thumb_left = px((progress_thumb_center - SLIDER_THUMB_SIZE / 2.0)
            .clamp(0.0, PROGRESS_TRACK_WIDTH - SLIDER_THUMB_SIZE));
        let volume_fill_width = volume_ratio * VOLUME_TRACK_WIDTH;
        let volume_thumb_center = volume_fill_width.clamp(
            SLIDER_THUMB_SIZE / 2.0,
            VOLUME_TRACK_WIDTH - SLIDER_THUMB_SIZE / 2.0,
        );
        let volume_thumb_left = px((volume_thumb_center - SLIDER_THUMB_SIZE / 2.0)
            .clamp(0.0, VOLUME_TRACK_WIDTH - SLIDER_THUMB_SIZE));
        Some(
            div()
                .absolute()
                .top(popup_y)
                .right(popup_right)
                .w(popup_width)
                .h(popup_height)
                .rounded(px(20.0))
                .overflow_hidden()
                .opacity(popup_opacity)
                .bg(popup_bg)
                .border_1()
                .border_color(border_color.opacity(0.18))
                .p(popup_inner_padding)
                .flex()
                .flex_col()
                .gap(px(POPUP_GAP))
                .on_mouse_down(
                    MouseButton::Left,
                    |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
                        cx.stop_propagation();
                    },
                )
                .on_mouse_up(
                    MouseButton::Left,
                    move |_: &MouseUpEvent, _: &mut Window, cx: &mut App| {
                        cx.stop_propagation();
                        commit_and_clear_drag(cx);
                    },
                )
                .on_mouse_down(
                    MouseButton::Left,
                    |_: &MouseDownEvent, _: &mut Window, cx: &mut App| {
                        cx.stop_propagation();
                    },
                )
                .when(popup_opacity < 0.999, |this: Div| this.top(popup_y))
                .when(drag_target.is_some(), |this: Div| {
                    this.on_mouse_move(
                        move |event: &MouseMoveEvent, _: &mut Window, cx: &mut App| {
                            let Some(target) = cx.global::<MusicState>().drag_target() else {
                                return;
                            };
                            if !event.dragging() {
                                return;
                            }
                            update_drag(target, event.position, cx);
                        },
                    )
                })
                .child(
                    div()
                        .w_full()
                        .flex_1()
                        .opacity((0.35 + popup_opacity * 0.65).clamp(0.0, 1.0))
                        .flex()
                        .gap(px(POPUP_GAP))
                        .child(
                            div()
                                .w(px(LEFT_COLUMN_WIDTH))
                                .flex_none()
                                .child(popup_cover(render_cover())),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .h_full()
                                .flex()
                                .flex_col()
                                .justify_start()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.0))
                                        .child(
                                            div()
                                                .text_size(px(9.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .opacity(0.65)
                                                .text_color(text_color)
                                                .child("BMCBL Music"),
                                        )
                                        .child(if popup_content_live {
                                            marquee_text(
                                                snapshot.title.clone(),
                                                px(META_TEXT_WIDTH),
                                                15.0,
                                                19.0,
                                                text_color,
                                                FontWeight::BOLD,
                                                1.0,
                                                34.0,
                                            )
                                            .into_any_element()
                                        } else {
                                            clipped_text(
                                                snapshot.title.clone(),
                                                px(META_TEXT_WIDTH),
                                                15.0,
                                                19.0,
                                                text_color,
                                                FontWeight::BOLD,
                                                1.0,
                                            )
                                            .into_any_element()
                                        })
                                        .child(if popup_content_live {
                                            marquee_text(
                                                snapshot.artist.clone(),
                                                px(META_TEXT_WIDTH),
                                                12.0,
                                                16.0,
                                                text_color,
                                                FontWeight::NORMAL,
                                                0.72,
                                                26.0,
                                            )
                                            .into_any_element()
                                        } else {
                                            clipped_text(
                                                snapshot.artist.clone(),
                                                px(META_TEXT_WIDTH),
                                                12.0,
                                                16.0,
                                                text_color,
                                                FontWeight::NORMAL,
                                                0.72,
                                            )
                                            .into_any_element()
                                        }),
                                )
                                .when_some(snapshot.last_error.clone(), |this, error| {
                                    this.child(if popup_content_live {
                                        marquee_text(
                                            error,
                                            px(ERROR_TEXT_WIDTH),
                                            10.0,
                                            13.0,
                                            accent,
                                            FontWeight::MEDIUM,
                                            0.9,
                                            24.0,
                                        )
                                        .into_any_element()
                                    } else {
                                        clipped_text(
                                            error,
                                            px(ERROR_TEXT_WIDTH),
                                            10.0,
                                            13.0,
                                            accent,
                                            FontWeight::MEDIUM,
                                            0.9,
                                        )
                                        .into_any_element()
                                    })
                                })
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(5.0))
                                        .child(
                                            div()
                                                .w_full()
                                                .h(px(PROGRESS_HIT_HEIGHT))
                                                .flex()
                                                .items_center()
                                                .cursor_pointer()
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |event, _, cx| {
                                                        cx.stop_propagation();
                                                        begin_drag(
                                                            MusicDragTarget::Progress,
                                                            event.position,
                                                            cx,
                                                        );
                                                    },
                                                )
                                                .on_mouse_move(move |event, _, cx| {
                                                    if event.dragging()
                                                        && cx.global::<MusicState>().drag_target()
                                                            == Some(MusicDragTarget::Progress)
                                                    {
                                                        update_drag(
                                                            MusicDragTarget::Progress,
                                                            event.position,
                                                            cx,
                                                        );
                                                    }
                                                })
                                                .child(
                                                    div()
                                                        .relative()
                                                        .w(px(PROGRESS_TRACK_WIDTH))
                                                        .h(px(PROGRESS_TRACK_HEIGHT))
                                                        .rounded_full()
                                                        .bg(muted_bg.opacity(0.85))
                                                        .child(
                                                            div()
                                                                .h_full()
                                                                .w(px(progress_fill_width))
                                                                .rounded_full()
                                                                .bg(accent),
                                                        )
                                                        .child(
                                                            div()
                                                                .absolute()
                                                                .top(px(-(SLIDER_THUMB_SIZE
                                                                    - PROGRESS_TRACK_HEIGHT)
                                                                    / 2.0))
                                                                .left(progress_thumb_left)
                                                                .w(px(SLIDER_THUMB_SIZE))
                                                                .h(px(SLIDER_THUMB_SIZE))
                                                                .rounded_full()
                                                                .bg(rgb(0xffffff))
                                                                .border_1()
                                                                .border_color(accent.opacity(0.30)),
                                                        ),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex()
                                                .justify_between()
                                                .text_size(px(9.0))
                                                .opacity(0.72)
                                                .text_color(text_color)
                                                .child(format_time(preview_current_seconds))
                                                .child(format_time(snapshot.total_seconds)),
                                        ),
                                ),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(28.0))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap(px(6.0))
                        .opacity(0.92)
                        .child(
                            icon_button(
                                if snapshot.mode == MusicPlaybackMode::Shuffle {
                                    lucide_icons::icon_shuffle()
                                } else {
                                    lucide_icons::icon_repeat()
                                },
                                true,
                                12.0,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                |_, _, cx| {
                                    cx.stop_propagation();
                                    let now = Instant::now();
                                    cx.update_global(|music: &mut MusicState, cx| {
                                        music.toggle_mode(now, cx);
                                    });
                                },
                            ),
                        )
                        .child(
                            icon_button(lucide_icons::icon_skip_back(), false, 13.0).on_mouse_down(
                                MouseButton::Left,
                                |_, _, cx| {
                                    cx.stop_propagation();
                                    let now = Instant::now();
                                    cx.update_global(|music: &mut MusicState, cx| {
                                        music.play_previous(now, cx);
                                    });
                                },
                            ),
                        )
                        .child(
                            div()
                                .w(px(28.0))
                                .h(px(28.0))
                                .rounded_full()
                                .bg(rgb(0xffffff))
                                .text_color(rgb(0x1f2937))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                    let now = Instant::now();
                                    cx.update_global(|music: &mut MusicState, cx| {
                                        music.toggle_playback(now);
                                    });
                                })
                                .child(
                                    icon_path(if snapshot.is_playing {
                                        lucide_icons::icon_pause()
                                    } else {
                                        lucide_icons::icon_play()
                                    })
                                    .size(px(13.0))
                                    .text_color(rgb(0x1f2937)),
                                ),
                        )
                        .child(
                            icon_button(lucide_icons::icon_skip_forward(), false, 13.0)
                                .on_mouse_down(MouseButton::Left, |_, _, cx| {
                                    cx.stop_propagation();
                                    let now = Instant::now();
                                    cx.update_global(|music: &mut MusicState, cx| {
                                        music.play_next(now, cx);
                                    });
                                }),
                        )
                        .child(
                            icon_button(
                                if snapshot.muted || snapshot.volume <= 0.01 {
                                    lucide_icons::icon_volume_x()
                                } else {
                                    lucide_icons::icon_volume_2()
                                },
                                false,
                                12.0,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                |_, _, cx| {
                                    cx.stop_propagation();
                                    let now = Instant::now();
                                    cx.update_global(|music: &mut MusicState, cx| {
                                        music.toggle_mute(now, cx);
                                    });
                                },
                            ),
                        )
                        .child(
                            div()
                                .w(px(VOLUME_TRACK_WIDTH))
                                .flex_none()
                                .h(px(VOLUME_HIT_HEIGHT))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .on_mouse_down(MouseButton::Left, move |event, _, cx| {
                                    cx.stop_propagation();
                                    begin_drag(MusicDragTarget::Volume, event.position, cx);
                                })
                                .on_mouse_move(move |event, _, cx| {
                                    if event.dragging()
                                        && cx.global::<MusicState>().drag_target()
                                            == Some(MusicDragTarget::Volume)
                                    {
                                        update_drag(MusicDragTarget::Volume, event.position, cx);
                                    }
                                })
                                .child(
                                    div()
                                        .relative()
                                        .w(px(VOLUME_TRACK_WIDTH))
                                        .h(px(VOLUME_TRACK_HEIGHT))
                                        .rounded_full()
                                        .bg(muted_bg.opacity(0.85))
                                        .child(
                                            div()
                                                .h_full()
                                                .w(px(volume_fill_width))
                                                .rounded_full()
                                                .bg(accent),
                                        )
                                        .child(
                                            div()
                                                .absolute()
                                                .top(px(-(SLIDER_THUMB_SIZE - VOLUME_TRACK_HEIGHT)
                                                    / 2.0))
                                                .left(volume_thumb_left)
                                                .w(px(SLIDER_THUMB_SIZE))
                                                .h(px(SLIDER_THUMB_SIZE))
                                                .rounded_full()
                                                .bg(rgb(0xffffff))
                                                .border_1()
                                                .border_color(accent.opacity(0.30)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .w(px(24.0))
                                .flex_shrink_0()
                                .text_right()
                                .text_size(px(9.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(text_color)
                                .child(format!("{:.0}%", volume_ratio * 100.0)),
                        ),
                )
                .into_any_element(),
        )
    } else {
        None
    };

    // 创建 backdrop：展开时覆盖外部区域，点击关闭弹窗
    let backdrop = if popup_visible {
        Some(
            div()
                .id("music-popup-backdrop")
                .absolute()
                .top(popup_top - px(10.0))
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                    cx.stop_propagation();
                    let should_update = {
                        let music = cx.global::<MusicState>();
                        music.drag_target().is_some() || music.expanded_target_open()
                    };
                    if !should_update {
                        return;
                    }

                    let now = Instant::now();
                    cx.update_global(|music: &mut MusicState, cx| {
                        music.commit_drag(now, cx);
                        music.clear_drag();
                        if music.expanded_target_open() {
                            music.set_expanded(false, now);
                        }
                    });
                })
                .into_any_element(),
        )
    } else {
        None
    };

    MusicPlayerRender {
        inline,
        overlay,
        backdrop,
    }
}
