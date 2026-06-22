use crate::ui::state::i18n::I18n;
use crate::ui::state::update::UpdateState;
use crate::ui::theme::colors::ThemeColors;
use crate::ui::views::settings::state::SettingsPageState;
use crate::utils::app_info;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use lucide_gpui::icons as lucide_icons;
use std::rc::Rc;

use super::rows::tab_title;

mod dependencies;
mod sponsors;
mod update_flow;

const IMG_LOGO: &str = "images/logo.png";
const IMG_DEV: &str = "images/about/Chlna6666.jpg";
const IMG_MCAPPX: &str = "images/about/MCAPPX.webp";
const IMG_MCIM: &str = "images/about/MCIM.png";
const IMG_EASYTIER: &str = "images/about/easytier.png";
const IMG_BL_CORE: &str = "images/about/BedrockLauncher.Core.webp";
const IMG_FUFUHA: &str = "images/about/Fufuha.jpg";
const IMG_USTINIANA: &str = "images/about/Ustiniana1641.jpg";
const IMG_GITHUB: &str = "images/about/github.png";
const IMG_AFDIAN: &str = "images/about/afdian.png";

pub(crate) const ABOUT_INTERACTION_PRELOAD_RESOURCES: &[&str] =
    &[IMG_LOGO, IMG_DEV, IMG_FUFUHA, IMG_USTINIANA];

pub(super) fn render_about_tab(
    colors: &ThemeColors,
    window_width: Pixels,
    render_engine: SharedString,
    i18n: &I18n,
    settings: &SettingsPageState,
    update: &UpdateState,
) -> Div {
    let section = i18n.t("Settings.tabs.about");

    let app_version = SharedString::from(app_info::get_version());

    let version_line = i18n.t_args(
        "AboutSection.app.version",
        crate::i18n_args![
            ("appVersion", app_version.as_ref()),
            ("renderEngine", render_engine.as_ref()),
        ],
    );

    let checking = update.checking;
    let update_btn_title = if checking {
        i18n.t("AboutSection.app.checking")
    } else {
        i18n.t("AboutSection.app.official")
    };
    let no_update_msg = i18n.t("AboutSection.update.no_update");

    let dev_card = about_big_card(
        colors,
        "about-dev",
        img(IMG_DEV)
            .w(px(44.))
            .h(px(44.))
            .rounded(px(14.))
            .opacity(0.95),
        SharedString::from("Chlna6666"),
        i18n.t("AboutSection.dev.description"),
        Some(IconAction {
            title: i18n.t("AboutSection.dev.sponsor"),
            icon_path: lucide_icons::icon_link(),
            enabled: true,
            on_click: Rc::new(|cx: &mut App| {
                cx.open_url("https://afdian.com/a/Chlna6666");
            }),
        }),
    );

    let app_card = about_big_card(
        colors,
        "about-app",
        img(IMG_LOGO)
            .w(px(44.))
            .h(px(44.))
            .rounded(px(14.))
            .opacity(0.95),
        SharedString::from("Better-Minecraft-Bedrock-Launcher"),
        version_line,
        Some(IconAction {
            title: update_btn_title,
            icon_path: if checking {
                lucide_icons::icon_loader()
            } else {
                lucide_icons::icon_refresh_cw()
            },
            enabled: !checking,
            on_click: Rc::new(move |cx: &mut App| {
                update_flow::spawn_manual_update_check(no_update_msg.clone(), cx);
            }),
        }),
    );

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(tab_title(colors, section))
        .child(dev_card)
        .child(app_card)
        .child(sub_title(colors, i18n.t("AboutSection.thanks.title")))
        .child(render_thanks_grid(colors, window_width, i18n))
        .child(render_dependencies_card(colors, i18n))
        .child(sub_title(colors, i18n.t("AboutSection.legal.title")))
        .child(render_legal_cards(colors, i18n, settings))
}

pub(super) fn render_sponsors_modal(
    colors: &ThemeColors,
    i18n: &I18n,
    settings: &SettingsPageState,
) -> Div {
    sponsors::render_sponsors_modal(colors, i18n, settings)
}

pub(super) fn render_dependencies_modal(
    colors: &ThemeColors,
    i18n: &I18n,
    settings: &SettingsPageState,
    window_width: Pixels,
    window_height: Pixels,
) -> Div {
    dependencies::render_dependencies_modal(colors, i18n, settings, window_width, window_height)
}

pub(super) fn render_engine_label(window: &Window) -> SharedString {
    let backend = gpui_backend_name();
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let _ = window;
        return SharedString::from(backend);
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let Some(gpu_specs) = window.gpu_specs() else {
            return SharedString::from(backend);
        };

        let device_name = gpu_specs.device_name.trim();
        if device_name.is_empty() {
            return SharedString::from(backend);
        }

        let software_suffix = if gpu_specs.is_software_emulated {
            " (software)"
        } else {
            ""
        };
        SharedString::from(format!("{backend} · {device_name}{software_suffix}"))
    }
}

fn gpui_backend_name() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "GPUI / DirectX 11"
    }

    #[cfg(target_os = "macos")]
    {
        "GPUI / Metal"
    }

    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        "GPUI / Vulkan"
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "freebsd"
    )))]
    {
        "GPUI"
    }
}

fn sub_title(colors: &ThemeColors, title: SharedString) -> Div {
    div()
        .pt(px(10.))
        .pb(px(4.))
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_secondary)
        .child(title)
}

fn base_card(colors: &ThemeColors, id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .rounded(px(18.))
        .border_1()
        .border_color(Hsla {
            a: 0.14,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.88,
            ..colors.surface
        })
        .p(px(18.))
        .hover(|this| {
            this.bg(Hsla {
                a: 0.95,
                ..colors.surface
            })
        })
}

#[derive(Clone)]
struct IconAction {
    title: SharedString,
    icon_path: &'static str,
    enabled: bool,
    on_click: Rc<dyn Fn(&mut App)>,
}

fn icon_btn(
    colors: &ThemeColors,
    title: SharedString,
    icon_path: &'static str,
    enabled: bool,
    on_click: Rc<dyn Fn(&mut App)>,
) -> Stateful<Div> {
    let mut btn = div()
        .id(SharedString::from(format!("icon-btn-{}", title.as_ref())))
        .w(px(34.))
        .h(px(34.))
        .rounded(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.96,
            ..colors.surface
        })
        .child(
            svg()
                .path(icon_path)
                .w(px(16.))
                .h(px(16.))
                .text_color(colors.text_primary)
                .opacity(0.96),
        );

    if enabled {
        btn = btn
            .cursor_pointer()
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
                    blur_radius: px(10.),
                    spread_radius: px(0.),
                    offset: point(px(0.), px(3.)),
                }])
            })
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| (on_click)(cx));
    } else {
        btn = btn.opacity(0.55);
    }

    btn
}

fn about_big_card(
    colors: &ThemeColors,
    id: &'static str,
    icon: impl IntoElement,
    title: SharedString,
    subtitle: SharedString,
    action: Option<IconAction>,
) -> Stateful<Div> {
    let action_el = action.map(|act| {
        icon_btn(colors, act.title, act.icon_path, act.enabled, act.on_click).into_any_element()
    });

    base_card(colors, id)
        .flex()
        .items_center()
        .gap(px(14.))
        .child(div().w(px(44.)).h(px(44.)).rounded(px(14.)).child(icon))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(colors.text_secondary)
                        .child(subtitle),
                ),
        )
        .when_some(action_el, |this, el| this.child(el))
}

#[derive(Clone)]
struct ThanksItem {
    img: SharedString,
    title: SharedString,
    desc: SharedString,
    link: Option<SharedString>,
    action: Option<Rc<dyn Fn(&mut App)>>,
    is_square: bool,
    is_small: bool,
}

fn render_thanks_grid(colors: &ThemeColors, window_width: Pixels, i18n: &I18n) -> Div {
    let open_sponsors = Rc::new(sponsors::open_sponsors_modal as fn(&mut App));

    let items: Vec<ThanksItem> = vec![
        ThanksItem {
            img: SharedString::from(IMG_MCAPPX),
            title: SharedString::from("MCAPPX"),
            desc: i18n.t("AboutSection.thanks.MCAPPX"),
            link: Some(SharedString::from("https://www.mcappx.com/")),
            action: None,
            is_square: true,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_MCIM),
            title: SharedString::from("MCIM"),
            desc: i18n.t("AboutSection.thanks.mcim"),
            link: Some(SharedString::from("https://www.mcimirror.top/")),
            action: None,
            is_square: true,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_EASYTIER),
            title: SharedString::from("EasyTier"),
            desc: i18n.t("AboutSection.thanks.easytier"),
            link: Some(SharedString::from("https://github.com/EasyTier/EasyTier")),
            action: None,
            is_square: true,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_BL_CORE),
            title: SharedString::from("BedrockLauncher.Core"),
            desc: i18n.t("AboutSection.thanks.bl_core"),
            link: Some(SharedString::from(
                "https://github.com/Round-Studio/BedrockLauncher.Core",
            )),
            action: None,
            is_square: true,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from("https://avatars.githubusercontent.com/u/5191659?v=4"),
            title: SharedString::from("MCMrARM"),
            desc: i18n.t("AboutSection.thanks.mcmrarm"),
            link: Some(SharedString::from(
                "https://github.com/MCMrARM/mc-w10-versiondb",
            )),
            action: None,
            is_square: false,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_FUFUHA),
            title: SharedString::from("Fufuha"),
            desc: i18n.t("AboutSection.thanks.fufuha"),
            link: Some(SharedString::from("https://space.bilibili.com/1798893653/")),
            action: None,
            is_square: false,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_USTINIANA),
            title: SharedString::from("Ustiniana1641"),
            desc: i18n.t("AboutSection.thanks.ustiniana1641"),
            link: None,
            action: None,
            is_square: false,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_GITHUB),
            title: i18n.t("AboutSection.thanks.contributors"),
            desc: i18n.t("AboutSection.thanks.community"),
            link: Some(SharedString::from(
                "https://github.com/BMCBL/Better-Minecraft-Bedrock-Launcher/graphs/contributors",
            )),
            action: None,
            is_square: true,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_AFDIAN),
            title: i18n.t("AboutSection.thanks.sponsors"),
            desc: i18n.t("AboutSection.thanks.support"),
            link: None,
            action: Some(open_sponsors),
            is_square: true,
            is_small: false,
        },
        ThanksItem {
            img: SharedString::from(IMG_LOGO),
            title: i18n.t("AboutSection.thanks.users"),
            desc: i18n.t("AboutSection.thanks.user_support"),
            link: None,
            action: None,
            is_square: true,
            is_small: true,
        },
    ];

    let window_width_value = window_width / px(1.);
    // Match the real content area:
    // - content container has max width 1000 and horizontal padding 14*2.
    // Keep cards spanning the full row width so left/right outer gaps stay aligned.
    let content_width = window_width_value.min(1000.0);
    let available_width = (content_width - 28.0).max(220.0);
    let two_columns = available_width >= 620.0;
    let view_label = i18n.t("AboutSection.common.view");
    if two_columns {
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut index = 0usize;
        while index < items.len() {
            let left_item = items[index].clone();
            let right_item = items.get(index + 1).cloned();

            let mut row = div().w_full().flex().gap(px(12.)).items_start().child(
                thanks_card(colors, index, left_item, view_label.clone())
                    .flex_1()
                    .min_w(px(0.)),
            );

            if let Some(item) = right_item {
                row = row.child(
                    thanks_card(colors, index + 1, item, view_label.clone())
                        .flex_1()
                        .min_w(px(0.)),
                );
            }

            rows.push(row.into_any_element());
            index += 2;
        }

        div().w_full().flex().flex_col().gap(px(12.)).children(rows)
    } else {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(12.))
            .children(items.into_iter().enumerate().map(|(idx, item)| {
                thanks_card(colors, idx, item, view_label.clone())
                    .w_full()
                    .into_any_element()
            }))
    }
}

fn thanks_card(
    colors: &ThemeColors,
    index: usize,
    item: ThanksItem,
    view_label: SharedString,
) -> Stateful<Div> {
    let mut icon = img(item.img.clone()).w(px(44.)).h(px(44.)).opacity(0.95);
    if item.is_square {
        icon = icon.rounded(px(14.));
    } else {
        icon = icon.rounded_full();
    }
    if item.is_small {
        icon = icon.w(px(36.)).h(px(36.)).rounded(px(12.)).opacity(0.92);
    }

    let has_action = item.link.is_some() || item.action.is_some();
    let mut card = base_card(colors, SharedString::from(format!("thanks-card-{}", index)))
        .flex()
        .items_center()
        .gap(px(14.))
        .h(px(100.))
        .child(div().w(px(44.)).h(px(44.)).rounded(px(14.)).child(icon))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .child(item.desc.clone()),
                ),
        );

    if has_action {
        let on_click = item.action.clone().unwrap_or_else(|| {
            let link = item.link.clone().unwrap_or_else(|| SharedString::from(""));
            Rc::new(move |cx: &mut App| {
                if !link.as_ref().is_empty() {
                    cx.open_url(link.as_ref());
                }
            })
        });

        let on_click_row = on_click.clone();
        let view_label_btn = view_label.clone();
        card = card
            .cursor_pointer()
            .on_mouse_up(MouseButton::Left, move |_ev, _window, cx| {
                (on_click_row)(cx)
            })
            .child(icon_btn(
                colors,
                view_label_btn,
                lucide_icons::icon_external_link(),
                true,
                on_click,
            ));
    } else {
        card = card.opacity(0.95);
    }

    card
}

fn render_legal_cards(colors: &ThemeColors, i18n: &I18n, _settings: &SettingsPageState) -> Div {
    let license = SharedString::from(app_info::get_license());
    let open_agreement = Rc::new(|cx: &mut App| {
        cx.update_global(|state: &mut SettingsPageState, cx| {
            state.about_agreement_open = true;
            state.about_agreement_scroll_handle = ScrollHandle::new();
        });
    });

    let cards = vec![
        LegalItem {
            title: i18n.t("AboutSection.legal.copyright.title"),
            content: i18n.t("AboutSection.legal.copyright.content"),
            link: Some(SharedString::from(
                "https://github.com/Chlna6666/Better-Minecraft-Bedrock-Launcher",
            )),
            action: None,
        },
        LegalItem {
            title: i18n.t("AboutSection.legal.agreement.title"),
            content: i18n.t("AboutSection.legal.agreement.content"),
            link: None,
            action: Some(open_agreement),
        },
        LegalItem {
            title: i18n.t("AboutSection.legal.license.title"),
            content: license,
            link: None,
            action: None,
        },
    ];

    div().flex().flex_col().gap(px(10.)).children(
        cards
            .into_iter()
            .enumerate()
            .map(|(idx, item)| legal_card(colors, idx, item).into_any_element()),
    )
}

fn render_dependencies_card(colors: &ThemeColors, i18n: &I18n) -> Stateful<Div> {
    let open_dependencies = Rc::new(dependencies::open_dependencies_modal as fn(&mut App));

    base_card(colors, "about-dependencies-card")
        .flex()
        .items_center()
        .gap(px(14.))
        .cursor_pointer()
        .on_mouse_up(MouseButton::Left, {
            let open_dependencies = open_dependencies.clone();
            move |_ev, _window, cx| {
                (open_dependencies)(cx);
            }
        })
        .child(
            div()
                .w(px(44.))
                .h(px(44.))
                .rounded(px(14.))
                .bg(Hsla {
                    a: 0.12,
                    ..colors.accent
                })
                .border_1()
                .border_color(Hsla {
                    a: 0.20,
                    ..colors.accent
                })
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_package())
                        .w(px(19.))
                        .h(px(19.))
                        .text_color(colors.accent),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .flex_1()
                .min_w(px(0.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.text_primary)
                        .child(i18n.t("AboutSection.dependencies.title")),
                )
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(18.))
                        .text_color(colors.text_secondary)
                        .whitespace_normal()
                        .child(i18n.t("AboutSection.dependencies.content")),
                ),
        )
        .child(icon_btn(
            colors,
            i18n.t("AboutSection.common.view"),
            lucide_icons::icon_external_link(),
            true,
            open_dependencies,
        ))
}

#[derive(Clone)]
struct LegalItem {
    title: SharedString,
    content: SharedString,
    link: Option<SharedString>,
    action: Option<Rc<dyn Fn(&mut App)>>,
}

fn legal_card(colors: &ThemeColors, index: usize, item: LegalItem) -> Stateful<Div> {
    let mut card = base_card(colors, SharedString::from(format!("legal-card-{}", index)))
        .flex()
        .flex_col()
        .gap(px(8.));

    let title = if let Some(link) = item.link.clone() {
        let title_text = item.title.clone();
        div()
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| {
                cx.open_url(link.as_ref());
            })
            .child(
                div()
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.text_primary)
                    .child(title_text),
            )
            .into_any_element()
    } else {
        div()
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(colors.text_primary)
            .child(item.title.clone())
            .into_any_element()
    };

    card = card.child(title).child(
        div()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(colors.text_secondary)
            .whitespace_normal()
            .child(item.content.clone()),
    );

    if let Some(action) = item.action.clone() {
        card = card
            .cursor_pointer()
            .on_mouse_up(MouseButton::Left, move |_ev, _window, cx| {
                (action)(cx);
            });
    }

    card
}

fn action_btn(
    colors: &ThemeColors,
    label: SharedString,
    on_click: Rc<dyn Fn(&mut App)>,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("btn-{}", label.as_ref())))
        .px(px(14.))
        .py(px(10.))
        .rounded(px(14.))
        .border_1()
        .border_color(Hsla {
            a: 0.24,
            ..colors.border
        })
        .bg(Hsla {
            a: 0.95,
            ..colors.surface
        })
        .cursor_pointer()
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
                blur_radius: px(10.),
                spread_radius: px(0.),
                offset: point(px(0.), px(3.)),
            }])
        })
        .on_mouse_down(MouseButton::Left, move |_ev, _window, cx| (on_click)(cx))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(colors.text_primary)
                .child(label),
        )
}
