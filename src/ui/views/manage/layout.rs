use super::*;

pub(super) fn render_version_header(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
) -> Div {
    let version_title = {
        let display_name = version.display_name();
        if display_name.as_ref().trim().is_empty() {
            version.version.clone()
        } else {
            display_name
        }
    };
    let version_type_label = if version.is_preview() {
        SharedString::from("预览")
    } else {
        SharedString::from("正式")
    };
    let version_type_color = if version.is_preview() {
        colors.danger
    } else {
        colors.text_secondary
    };
    let version_meta = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .flex_shrink_0()
        .child(subtle_badge(colors, version.version.clone()))
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(version_type_color)
                .child(version_type_label),
        );

    let mut status_badges = div().flex().gap(px(8.)).flex_wrap();
    let mut has_status_badges = false;

    if state.version_config.enable_redirection {
        has_status_badges = true;
        status_badges =
            status_badges.child(tonal_badge(colors, "隔离模式", colors.stat_green_text));
    }
    if state.version_config.editor_mode {
        has_status_badges = true;
        status_badges = status_badges.child(subtle_badge(colors, "编辑器模式"));
    }
    if state.version_config.disable_mod_loading {
        has_status_badges = true;
        status_badges = status_badges.child(tonal_badge(colors, "禁用 Mod", colors.danger));
    }

    let mut header = div()
        .flex_1()
        .min_w(px(0.))
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .text_color(colors.text_primary)
                        .flex_shrink_0()
                        .child(version_title),
                )
                .child(version_meta),
        );

    if has_status_badges {
        header = header.child(status_badges);
    }

    if state.version_config_loading {
        header = header.child(
            div()
                .text_size(px(12.))
                .text_color(colors.text_secondary)
                .child("正在读取版本配置..."),
        );
    }
    if let Some(error) = state.version_config_error.clone() {
        header = header.child(
            div()
                .text_size(px(12.))
                .text_color(colors.danger)
                .child(error),
        );
    }

    header
}

pub(super) fn render_tab_bar(
    colors: &ThemeColors,
    state: &ManagePageState,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    let view_handle = cx.entity().downgrade();

    UnderlineTabs::new(
        colors,
        vec![
            TabItem::new("manage-tab-mod", "Mod", state.tab == ManageTab::Mod, {
                let view_handle = view_handle.clone();
                move |_window, cx| {
                    let _ = view_handle.update(cx, |this, cx| {
                        this.set_tab(ManageTab::Mod, cx);
                    });
                }
            })
            .icon(lucide_icons::icon_layers()),
            TabItem::new(
                "manage-tab-pack",
                "资源包",
                state.tab == ManageTab::ResourcePack,
                {
                    let view_handle = view_handle.clone();
                    move |_window, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.set_tab(ManageTab::ResourcePack, cx);
                        });
                    }
                },
            )
            .icon(lucide_icons::icon_package()),
            TabItem::new(
                "manage-tab-skin-pack",
                "皮肤",
                state.tab == ManageTab::SkinPack,
                {
                    let view_handle = view_handle.clone();
                    move |_window, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.set_tab(ManageTab::SkinPack, cx);
                        });
                    }
                },
            )
            .icon(lucide_icons::icon_user()),
            TabItem::new("manage-tab-map", "地图", state.tab == ManageTab::Map, {
                let view_handle = view_handle.clone();
                move |_window, cx| {
                    let _ = view_handle.update(cx, |this, cx| {
                        this.set_tab(ManageTab::Map, cx);
                    });
                }
            })
            .icon(lucide_icons::icon_map()),
            TabItem::new(
                "manage-tab-screenshot",
                "截图",
                state.tab == ManageTab::Screenshot,
                {
                    let view_handle = view_handle.clone();
                    move |_window, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.set_tab(ManageTab::Screenshot, cx);
                        });
                    }
                },
            )
            .icon(lucide_icons::icon_image()),
            TabItem::new(
                "manage-tab-server",
                "服务器",
                state.tab == ManageTab::Server,
                move |_window, cx| {
                    let _ = view_handle.update(cx, |this, cx| {
                        this.set_tab(ManageTab::Server, cx);
                    });
                },
            )
            .icon(lucide_icons::icon_server()),
        ],
    )
    .gap(px(14.))
    .into_any_element()
}

pub(super) fn render_pack_subtype_switch(
    colors: &ThemeColors,
    state: &ManagePageState,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    let view_handle = cx.entity().downgrade();

    AnimatedSegmentTabs::new(
        "manage-pack-subtype-tabs",
        colors,
        vec![
            TabItem::new(
                "manage-pack-resource",
                "资源包",
                state.pack_subtype == ManagePackSubtype::Resource,
                {
                    let view_handle = view_handle.clone();
                    move |_window, cx| {
                        let _ = view_handle.update(cx, |this, cx| {
                            this.set_pack_subtype(ManagePackSubtype::Resource, cx);
                        });
                    }
                },
            )
            .icon(lucide_icons::icon_package()),
            TabItem::new(
                "manage-pack-behavior",
                "行为包",
                state.pack_subtype == ManagePackSubtype::Behavior,
                move |_window, cx| {
                    let _ = view_handle.update(cx, |this, cx| {
                        this.set_pack_subtype(ManagePackSubtype::Behavior, cx);
                    });
                },
            )
            .icon(lucide_icons::icon_layers()),
        ],
    )
    .height(px(28.))
    .item_width(px(76.))
    .into_any_element()
}

pub(super) fn render_toolbar_search_input(
    input: &Entity<InputState>,
    colors: &ThemeColors,
    width: Pixels,
) -> AnyElement {
    let dark_mode = colors.bg.l < 0.5;
    let shell_background = colors.settings_field_bg;
    let shell_border = Hsla {
        a: if dark_mode { 0.24 } else { 0.14 },
        ..colors.accent
    };
    let icon_color = Hsla {
        a: if dark_mode { 0.72 } else { 0.86 },
        ..colors.text_muted
    };
    let width_px: f32 = width.into();

    div()
        .id("manage-search-input-wrapper")
        .w(width)
        .h(px(32.))
        .px(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(shell_border)
        .bg(shell_background)
        .shadow(vec![BoxShadow {
            color: Hsla {
                a: if dark_mode { 0.12 } else { 0.06 },
                ..colors.accent
            },
            blur_radius: px(10.0),
            spread_radius: px(-6.0),
            offset: point(px(0.), px(2.)),
        }])
        .child(
            Input::new(input)
                .with_size(InputSize::Small)
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .cleanable(true)
                .h_full()
                .w(px(width_px - 20.0))
                .prefix(
                    svg()
                        .path(lucide_icons::icon_search())
                        .w(px(13.))
                        .h(px(13.))
                        .text_color(icon_color),
                ),
        )
        .into_any_element()
}

pub(super) fn gdk_user_label(user: &ManageGdkUser) -> SharedString {
    if user.folder_name.as_ref().eq_ignore_ascii_case("shared") {
        SharedString::from("Shared")
    } else {
        user.folder_name.clone()
    }
}

pub(super) fn render_gdk_dropdown(
    colors: &ThemeColors,
    state: &ManagePageState,
    _cx: &mut Context<ManagePageView>,
) -> AnyElement {
    let options: Vec<_> = state
        .gdk_users
        .iter()
        .map(gdk_user_label)
        .map(DropdownOption::from)
        .collect();

    let selected_index = state
        .selected_gdk_user
        .as_ref()
        .and_then(|selected| {
            state
                .gdk_users
                .iter()
                .position(|user| user.folder_name == *selected)
        })
        .or_else(|| {
            ManagePageView::preferred_gdk_user(&state.gdk_users).and_then(|selected| {
                state
                    .gdk_users
                    .iter()
                    .position(|user| user.folder_name == selected)
            })
        })
        .unwrap_or(0);

    let label = options
        .get(selected_index)
        .map(|option| option.label.clone())
        .unwrap_or_else(|| SharedString::from("用户目录"));

    Dropdown::with_trigger(
        SharedString::from("manage-gdk-user-dropdown"),
        colors,
        px(128.),
        px(34.),
        label,
        options,
        selected_index,
        !state.gdk_users_loading,
        |colors, _width, _height, enabled, open_k, label| {
            div()
                .size_full()
                .px(px(10.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_size(px(12.))
                        .text_color(if enabled {
                            colors.text_primary
                        } else {
                            colors.text_muted
                        })
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(label.clone()),
                )
                .child(
                    svg()
                        .path(lucide_icons::icon_chevron_down())
                        .w(px(14.))
                        .h(px(14.))
                        .opacity(if enabled { 0.75 } else { 0.35 })
                        .text_color(colors.text_secondary)
                        .with_transformation(Transformation::rotate(radians(
                            open_k * std::f32::consts::PI,
                        ))),
                )
                .into_any_element()
        },
        {
            let users = state.gdk_users.clone();
            move |index, _window, cx| {
                let selected = users.get(index).map(|user| user.folder_name.clone());
                cx.update_global(|state: &mut ManagePageState, _cx| {
                    state.selected_gdk_user = selected.clone();
                    state.selected_asset_keys.clear();
                    state.assets_loaded = false;
                    state.assets_loading = false;
                    state.assets_error = None;
                });
            }
        },
    )
    .into_any_element()
}

pub(super) fn should_render_gdk_dropdown(
    state: &ManagePageState,
    version: &ManagedVersionEntry,
) -> bool {
    is_gdk_user_scoped_tab(state.tab) && version.is_gdk() && state.gdk_users.len() > 1
}

pub(super) fn render_sort_controls(
    colors: &ThemeColors,
    state: &ManagePageState,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    let render_sort_button = |id: &'static str,
                              key: ManageAssetSortKey,
                              icon_path: &'static str,
                              cx: &mut Context<ManagePageView>| {
        let is_active = state.asset_sort_key == key;
        div()
            .id(id)
            .w(px(28.))
            .h(px(28.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .border_1()
            .border_color(if is_active {
                colors.accent
            } else {
                colors.border
            })
            .bg(if is_active {
                Hsla {
                    a: 0.10,
                    ..colors.accent
                }
            } else {
                colors.surface
            })
            .cursor_pointer()
            .child(
                svg()
                    .path(icon_path)
                    .w(px(14.))
                    .h(px(14.))
                    .text_color(if is_active {
                        colors.accent
                    } else {
                        colors.text_secondary
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.set_asset_sort(key, cx);
                }),
            )
    };

    div()
        .flex()
        .gap(px(6.))
        .child(render_sort_button(
            "manage-sort-name",
            ManageAssetSortKey::Name,
            lucide_icons::icon_file_text(),
            cx,
        ))
        .child(render_sort_button(
            "manage-sort-date",
            ManageAssetSortKey::Date,
            lucide_icons::icon_calendar(),
            cx,
        ))
        .child(render_sort_button(
            "manage-sort-size",
            ManageAssetSortKey::Size,
            lucide_icons::icon_box(),
            cx,
        ))
        .into_any_element()
}

pub(super) fn render_active_toolbar_actions(
    colors: &ThemeColors,
    state: &ManagePageState,
    cx: &mut Context<ManagePageView>,
) -> Vec<AnyElement> {
    match state.tab {
        ManageTab::Mod | ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map => {
            vec![
                render_sort_controls(colors, state, cx),
                if state.selected_asset_keys.is_empty() {
                    toolbar_glyph_button(
                        "manage-import-assets",
                        lucide_icons::icon_file_up(),
                        colors,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.import_assets(window, cx);
                        }),
                    )
                    .into_any_element()
                } else {
                    toolbar_glyph_button(
                        "manage-delete-assets",
                        lucide_icons::icon_trash_2(),
                        colors,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.request_delete_selected_assets(cx);
                        }),
                    )
                    .into_any_element()
                },
            ]
        }
        ManageTab::Screenshot => vec![
            toolbar_glyph_button(
                "manage-refresh-screenshots",
                lucide_icons::icon_refresh_cw(),
                colors,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.refresh_screenshots(cx);
                }),
            )
            .into_any_element(),
        ],
        ManageTab::Server => vec![
            toolbar_glyph_button(
                "manage-refresh-servers",
                lucide_icons::icon_refresh_cw(),
                colors,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.refresh_servers(cx);
                }),
            )
            .into_any_element(),
            toolbar_glyph_button("manage-add-server", lucide_icons::icon_plus(), colors)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        this.open_add_server_dialog(window, cx);
                    }),
                )
                .into_any_element(),
        ],
    }
}
