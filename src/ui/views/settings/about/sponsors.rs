use crate::ui::components::modal;
use crate::ui::state::i18n::I18n;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::{AboutSponsorEntry, SettingsPageState};
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::path::PathBuf;
use std::rc::Rc;

use super::{action_btn, icon_btn};

const SPONSOR_AVATAR_SIZE: f32 = 32.0;
const SPONSOR_AVATAR_RADIUS: f32 = 10.0;
const SPONSOR_CARD_WIDTH: f32 = 210.0;
const SPONSOR_MODAL_RADIUS: f32 = 18.0;

pub(super) fn open_sponsors_modal(cx: &mut App) {
    let should_load = cx.update_global(|state: &mut SettingsPageState, cx| {
        state.about_sponsors_open = true;
        state.about_sponsors_page = 0;
        state.about_sponsors_page_size = 60;
        state.about_sponsors_error = None;
        !state.about_sponsors_loading
    });

    if should_load {
        spawn_load_sponsors(cx);
    }
}

pub(super) fn render_sponsors_modal(
    colors: &ThemeColors,
    i18n: &I18n,
    settings: &SettingsPageState,
) -> Div {
    let overlay_background = hsla(0., 0., 0., 0.26);

    let close = Rc::new(|cx: &mut App| {
        close_sponsors_modal(cx);
    });

    let header = {
        let close_button = close.clone();

        div()
            .flex()
            .items_center()
            .justify_between()
            .px(px(18.))
            .py(px(14.))
            .rounded_t(px(SPONSOR_MODAL_RADIUS))
            .bg(Hsla {
                l: (colors.surface.l * 0.92).clamp(0.0, 1.0),
                a: 1.0,
                ..colors.surface
            })
            .border_b_1()
            .border_color(Hsla {
                a: 0.22,
                ..colors.border
            })
            .child(
                div()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(i18n.t("AboutSection.sponsors.title")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(icon_btn(
                        colors,
                        i18n.t("AboutSection.sponsors.support_link"),
                        lucide_icons::icon_link(),
                        true,
                        Rc::new(|cx: &mut App| {
                            cx.open_url("https://afdian.com/a/Chlna6666");
                        }),
                    ))
                    .child(icon_btn(
                        colors,
                        i18n.t("common.close"),
                        lucide_icons::icon_x(),
                        true,
                        close_button,
                    )),
            )
    };

    let card = div()
        .id("about-sponsors-modal")
        .relative()
        .w_full()
        .max_w(px(920.))
        .h(px(560.))
        .max_h(px(640.))
        .rounded(px(SPONSOR_MODAL_RADIUS))
        .overflow_hidden()
        .border_1()
        .border_color(Hsla {
            a: 0.34,
            ..colors.border
        })
        .bg(Hsla {
            l: (colors.surface.l * 0.93).clamp(0.0, 1.0),
            a: 0.99,
            ..colors.surface
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.30,
                ..rgb(0x000000).into()
            },
            blur_radius: px(40.),
            spread_radius: px(0.),
            offset: point(px(0.), px(16.)),
        }])
        .flex()
        .flex_col()
        .child(
            div()
                .absolute()
                .inset(px(1.))
                .rounded(px(SPONSOR_MODAL_RADIUS - 1.0))
                .border_1()
                .border_color(Hsla {
                    a: 0.16,
                    ..colors.border
                })
                .occlude(),
        )
        .child(header)
        .child(render_sponsors_body(colors, i18n, settings));

    modal::modal_layer_dismissible(
        div()
            .w_full()
            .h_full()
            .p(px(18.))
            .flex()
            .items_center()
            .justify_center()
            .child(card),
        overlay_background,
        close,
    )
}

fn close_sponsors_modal(cx: &mut App) {
    if let Err(error) = crate::core::sponsors::clear_avatar_cache() {
        tracing::warn!("clear sponsor avatar cache failed: {error}");
    }

    let _ = cx.update_global(|state: &mut SettingsPageState, cx| {
        state.about_sponsors_req_id = state.about_sponsors_req_id.saturating_add(1);
        state.about_sponsors_open = false;
        state.about_sponsors_loading = false;
        state.about_sponsors_page = 0;
        state.about_sponsors_error = None;
        state.about_sponsors_skeleton_phase = 0;
        state.about_sponsors.clear();
        state.about_sponsors.shrink_to_fit();
    });
}

fn spawn_load_sponsors(cx: &mut App) {
    if let Err(error) = crate::core::sponsors::clear_avatar_cache() {
        tracing::warn!("clear sponsor avatar cache failed: {error}");
    }

    let request_id = cx.update_global(|state: &mut SettingsPageState, cx| {
        state.about_sponsors_req_id = state.about_sponsors_req_id.saturating_add(1);
        state.about_sponsors_loading = true;
        state.about_sponsors_skeleton_phase = 0;
        state.about_sponsors_error = None;
        state.about_sponsors_page = 0;
        state.about_sponsors.clear();
        state.about_sponsors_req_id
    });

    cx.spawn({
        async move |cx| {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(180)).await;

                let should_continue = cx
                    .update_global(|state: &mut SettingsPageState, cx| {
                        if state.about_sponsors_req_id != request_id
                            || !state.about_sponsors_loading
                        {
                            return false;
                        }

                        state.about_sponsors_skeleton_phase =
                            state.about_sponsors_skeleton_phase.wrapping_add(1);
                        true
                    })
                    .unwrap_or(false);

                if !should_continue {
                    break;
                }
            }
        }
    })
    .detach();

    cx.spawn(async move |cx| {
        let result = crate::core::sponsors::load_sponsors().await;
        let (list, error) = match result {
            Ok(records) => (sponsor_records_to_state(records), None),
            Err(error) => (Vec::new(), Some(SharedString::from(error))),
        };

        let _ = cx.update_global(|state: &mut SettingsPageState, cx| {
            if state.about_sponsors_req_id != request_id {
                return;
            }

            state.about_sponsors_loading = false;
            state.about_sponsors_error = error;

            if state.about_sponsors_error.is_none() {
                state.about_sponsors = list;
                let max_page = state
                    .about_sponsors
                    .len()
                    .saturating_sub(1)
                    .saturating_div(state.about_sponsors_page_size.max(1));
                state.about_sponsors_page = state.about_sponsors_page.min(max_page);
            }
        });
    })
    .detach();
}

fn sponsor_records_to_state(
    records: Vec<crate::core::sponsors::SponsorRecord>,
) -> Vec<AboutSponsorEntry> {
    records
        .into_iter()
        .map(|record| AboutSponsorEntry {
            user_id: SharedString::from(record.user_id),
            name: SharedString::from(record.name),
            avatar_url: SharedString::from(record.avatar_path),
            all_sum_amount: SharedString::from(record.total_amount),
        })
        .collect()
}

fn render_sponsors_body(
    colors: &ThemeColors,
    i18n: &I18n,
    settings: &SettingsPageState,
) -> impl IntoElement {
    let total = settings.about_sponsors.len();
    let page_size = settings.about_sponsors_page_size.max(1);
    let max_page = total.saturating_sub(1) / page_size;
    let page = settings.about_sponsors_page.min(max_page);
    let start = page.saturating_mul(page_size);
    let end = (start + page_size).min(total);

    let mut body = div()
        .id("about-sponsors-body")
        .flex_1()
        .min_h(px(0.))
        .p(px(16.))
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(px(12.));

    if settings.about_sponsors_loading {
        return body.child(sponsor_skeleton_grid(
            colors,
            settings.about_sponsors_skeleton_phase,
        ));
    }

    if let Some(error) = settings.about_sponsors_error.clone() {
        return body.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .py(px(40.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(colors.text_primary)
                        .child(i18n.t("AboutSection.sponsors.error")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(error),
                )
                .child(action_btn(
                    colors,
                    i18n.t("retry"),
                    Rc::new(|cx: &mut App| spawn_load_sponsors(cx)),
                )),
        );
    }

    if total == 0 {
        return body.child(
            div().py(px(40.)).flex().justify_center().child(
                div()
                    .text_size(px(13.))
                    .text_color(colors.text_secondary)
                    .child(i18n.t("AboutSection.sponsors.empty")),
            ),
        );
    }

    body = body.child(
        div()
            .px(px(2.))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .child(SharedString::from(format!(
                        "{} / {}",
                        end.saturating_sub(start),
                        total
                    ))),
            )
            .child(sponsor_pager(colors, page, max_page)),
    );

    body.child(
        div()
            .flex()
            .flex_wrap()
            .justify_center()
            .gap(px(12.))
            .children(
                settings.about_sponsors[start..end]
                    .iter()
                    .map(|item| sponsor_item(colors, item.clone()).into_any_element()),
            ),
    )
}

fn sponsor_pager(colors: &ThemeColors, page: usize, max_page: usize) -> Div {
    let previous = Rc::new(move |cx: &mut App| {
        cx.update_global(|state: &mut SettingsPageState, cx| {
            state.about_sponsors_page = state.about_sponsors_page.saturating_sub(1);
        });
    });

    let next = Rc::new(move |cx: &mut App| {
        cx.update_global(|state: &mut SettingsPageState, cx| {
            state.about_sponsors_page = (state.about_sponsors_page + 1).min(
                state.about_sponsors.len().saturating_sub(1)
                    / state.about_sponsors_page_size.max(1),
            );
        });
    });

    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon_btn(
            colors,
            SharedString::from("previous"),
            lucide_icons::icon_chevron_left(),
            page > 0,
            previous,
        ))
        .child(
            div()
                .min_w(px(48.))
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .text_align(TextAlign::Center)
                .child(SharedString::from(format!("{}/{}", page + 1, max_page + 1))),
        )
        .child(icon_btn(
            colors,
            SharedString::from("next"),
            lucide_icons::icon_chevron_right(),
            page < max_page,
            next,
        ))
}

fn sponsor_item(colors: &ThemeColors, item: AboutSponsorEntry) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!(
            "sponsor-{}",
            item.user_id.as_ref()
        )))
        .w(px(SPONSOR_CARD_WIDTH))
        .rounded(px(14.))
        .border_1()
        .border_color(Hsla {
            a: 0.20,
            ..colors.border
        })
        .bg(Hsla {
            l: (colors.surface.l * 0.94).clamp(0.0, 1.0),
            a: 1.0,
            ..colors.surface
        })
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: 0.08,
                ..rgb(0x000000).into()
            },
            blur_radius: px(10.),
            spread_radius: px(0.),
            offset: point(px(0.), px(3.)),
        }])
        .p(px(12.))
        .flex()
        .items_center()
        .gap(px(10.))
        .hover(|this| {
            this.bg(Hsla {
                a: 1.0,
                ..colors.surface
            })
            .shadow(vec![BoxShadow {
                color: Hsla {
                    a: 0.14,
                    ..rgb(0x000000).into()
                },
                blur_radius: px(14.),
                spread_radius: px(0.),
                offset: point(px(0.), px(4.)),
            }])
        })
        .child(sponsor_avatar(colors, &item))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_size(px(12.))
                .text_color(colors.text_primary)
                .child(item.name),
        )
}

fn sponsor_avatar(colors: &ThemeColors, item: &AboutSponsorEntry) -> Div {
    let radius = px(SPONSOR_AVATAR_RADIUS);

    let base = div()
        .w(px(SPONSOR_AVATAR_SIZE))
        .h(px(SPONSOR_AVATAR_SIZE))
        .rounded(radius)
        .overflow_hidden()
        .bg(colors.border)
        .flex_none();

    if item.avatar_url.as_ref().is_empty() {
        return base;
    }

    let avatar_path = PathBuf::from(item.avatar_url.as_ref());
    if !avatar_path.is_file() {
        return base;
    }

    base.child(
        img(avatar_path)
            .w(px(SPONSOR_AVATAR_SIZE))
            .h(px(SPONSOR_AVATAR_SIZE))
            .rounded(radius),
    )
}

fn sponsor_skeleton_grid(colors: &ThemeColors, phase: u8) -> Div {
    let mut grid = div().flex().flex_wrap().gap(px(12.));
    let base_alpha = 0.42 + (f32::from(phase % 6) * 0.04);

    for index in 0..9 {
        let card_alpha = (base_alpha + ((index % 3) as f32 * 0.04)).min(0.74);
        grid = grid.child(
            div()
                .id(SharedString::from(format!("sponsor-skel-{}", index)))
                .w(px(SPONSOR_CARD_WIDTH))
                .rounded(px(14.))
                .border_1()
                .border_color(Hsla {
                    a: 0.10,
                    ..colors.border
                })
                .bg(Hsla {
                    a: card_alpha,
                    ..colors.surface
                })
                .p(px(12.))
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .w(px(SPONSOR_AVATAR_SIZE))
                        .h(px(SPONSOR_AVATAR_SIZE))
                        .rounded(px(SPONSOR_AVATAR_RADIUS))
                        .bg(colors.border),
                )
                .child(
                    div()
                        .h(px(10.))
                        .w(px(120.))
                        .rounded(px(6.))
                        .bg(colors.border),
                ),
        );
    }

    grid
}
