use super::*;

pub struct ManagePageView {
    pub(super) _subscriptions: Vec<Subscription>,
    pub(super) asset_search_input: Option<Entity<InputState>>,
    pub(super) screenshot_search_input: Option<Entity<InputState>>,
    pub(super) server_search_input: Option<Entity<InputState>>,
    pub(super) asset_scroll_handle: ScrollHandle,
    pub(super) screenshot_scroll_handle: ScrollHandle,
    pub(super) server_scroll_handle: ScrollHandle,
    pub(super) asset_list_cache: AssetListRenderCache,
    pub(super) screenshot_list_cache: ScreenshotListRenderCache,
    pub(super) server_list_cache: ServerListRenderCache,
    pub(super) version_settings_modal: Option<version_settings::VersionSettingsModalState>,
    pub(super) confirm_dialog: Option<ConfirmDialogState>,
    pub(super) value_prompt: Option<ValuePromptDialogState>,
    pub(super) mod_type_dialog: Option<ModTypeDialogState>,
    pub(super) server_editor_dialog: Option<ServerEditorDialogState>,
    pub(super) level_dat_editor: Option<level_dat_editor::LevelDatEditorModalState>,
    pub(super) last_selected_folder: Option<SharedString>,
    pub(super) last_version_config_signature: Option<VersionConfigLoadSignature>,
    pub(super) last_gdk_users_signature: Option<GdkUsersLoadSignature>,
    pub(super) last_assets_signature: Option<AssetsLoadSignature>,
    pub(super) last_screenshots_signature: Option<ScreenshotsLoadSignature>,
    pub(super) last_servers_signature: Option<ServersLoadSignature>,
    pub(super) last_global_render_signature: ManageRenderSignature,
}

impl ManagePageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let initial_render_signature =
            ManageRenderSignature::from_state(cx.global::<ManagePageState>());
        let subscriptions = vec![
            cx.observe_global::<ManagePageState>(|this, cx| {
                let signature = ManageRenderSignature::from_state(cx.global::<ManagePageState>());
                if this.last_global_render_signature != signature {
                    this.last_global_render_signature = signature;
                    cx.notify();
                }
            }),
            cx.observe_global::<ThemeState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<I18n>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(|_, cx| {
                cx.notify();
            }),
        ];

        Self {
            _subscriptions: subscriptions,
            asset_search_input: None,
            screenshot_search_input: None,
            server_search_input: None,
            asset_scroll_handle: ScrollHandle::new(),
            screenshot_scroll_handle: ScrollHandle::new(),
            server_scroll_handle: ScrollHandle::new(),
            asset_list_cache: AssetListRenderCache::default(),
            screenshot_list_cache: ScreenshotListRenderCache::default(),
            server_list_cache: ServerListRenderCache::default(),
            version_settings_modal: None,
            confirm_dialog: None,
            value_prompt: None,
            mod_type_dialog: None,
            server_editor_dialog: None,
            level_dat_editor: None,
            last_selected_folder: None,
            last_version_config_signature: None,
            last_gdk_users_signature: None,
            last_assets_signature: None,
            last_screenshots_signature: None,
            last_servers_signature: None,
            last_global_render_signature: initial_render_signature,
        }
    }

    pub(super) fn reset_asset_list_view(&mut self) {
        self.asset_list_cache.clear();
        self.asset_scroll_handle.set_offset(point(px(0.), px(0.)));
    }

    pub(super) fn reset_screenshot_list_view(&mut self) {
        self.screenshot_list_cache.clear();
        self.screenshot_scroll_handle
            .set_offset(point(px(0.), px(0.)));
    }

    pub(super) fn reset_server_list_view(&mut self) {
        self.server_list_cache.clear();
        self.server_scroll_handle.set_offset(point(px(0.), px(0.)));
    }
}

impl Render for ManagePageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_asset_search_input(window, cx);
        self.ensure_screenshot_search_input(window, cx);
        self.ensure_server_search_input(window, cx);
        self.sync_selected_version(cx);
        self.sync_data_requests(cx);

        let now = Instant::now();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let i18n = cx.global::<I18n>().clone();
        let state = cx.global::<ManagePageState>().clone();
        let page = self.render_page(window, &colors, &state, cx);
        let _ = i18n;

        div()
            .size_full()
            .relative()
            .child(page_shell(page, &colors))
    }
}

impl ManagePageView {
    fn render_page(
        &mut self,
        window: &mut Window,
        colors: &ThemeColors,
        state: &ManagePageState,
        cx: &mut Context<Self>,
    ) -> Div {
        if is_level_dat_editor_route(cx) {
            return div()
                .size_full()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(self.render_main(window, colors, state, cx));
        }

        div()
            .size_full()
            .flex()
            .gap(px(16.))
            .child(self.render_sidebar(colors, state, cx))
            .child(self.render_main(window, colors, state, cx))
    }

    fn render_sidebar(
        &self,
        colors: &ThemeColors,
        state: &ManagePageState,
        cx: &mut Context<Self>,
    ) -> Div {
        let filtered_versions = filtered_versions(state);
        panel_shell(colors)
            .w(px(280.))
            .h_full()
            .bg(colors.settings_panel_bg)
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(state.search_input.as_ref().map_or_else(
                || div().h(px(32.)).w_full().into_any_element(),
                |input| {
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(10.))
                        .child(render_toolbar_search_input(input, colors, px(150.)))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    sidebar_icon_button(
                                        "manage-import-version",
                                        lucide_icons::icon_plus(),
                                        colors,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, window, cx| {
                                            this.import_version_package(window, cx);
                                        }),
                                    ),
                                )
                                .child(
                                    sidebar_icon_button(
                                        "manage-refresh-version",
                                        lucide_icons::icon_refresh_cw(),
                                        colors,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.refresh_versions(cx);
                                        }),
                                    ),
                                ),
                        )
                        .into_any_element()
                },
            ))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .overflow_y_scrollbar()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .when(state.loading, |this| {
                        this.child(subtle_badge(colors, "正在加载版本列表"))
                    })
                    .when_some(state.error.clone(), |this, error| {
                        this.child(
                            div()
                                .rounded(px(12.))
                                .p(px(10.))
                                .bg(Hsla {
                                    a: 0.12,
                                    ..colors.danger
                                })
                                .text_size(px(12.))
                                .text_color(colors.danger)
                                .child(error),
                        )
                    })
                    .when(filtered_versions.is_empty(), |this| {
                        this.child(
                            empty_state(
                                colors,
                                "images/manage/empty.svg",
                                "没有找到版本",
                                "请先导入或安装一个可管理的版本。",
                            )
                            .h(px(220.)),
                        )
                    })
                    .children(filtered_versions.into_iter().map(|version| {
                        let selected = state
                            .selected_folder
                            .as_ref()
                            .is_some_and(|folder| folder == &version.folder);
                        let folder = version.folder.clone();
                        let version_badge = if version.is_gdk() { "GDK" } else { "UWP" };
                        let icon = launch_version_icon_path(version.name.as_ref());
                        let (badge_bg, badge_fg): (Hsla, Hsla) = if version.is_gdk() {
                            (
                                Hsla {
                                    a: 0.15,
                                    ..rgb(0x8b5cf6).into()
                                },
                                rgb(0x7c3aed).into(),
                            )
                        } else {
                            (
                                Hsla {
                                    a: 0.15,
                                    ..rgb(0x06b6d4).into()
                                },
                                rgb(0x0891b2).into(),
                            )
                        };

                        div()
                            .id(SharedString::from(format!("manage-version-{}", folder)))
                            .w_full()
                            .px(px(10.))
                            .py(px(9.))
                            .rounded(px(12.))
                            .cursor_pointer()
                            .border_1()
                            .border_color(if selected {
                                Hsla {
                                    a: 0.42,
                                    ..colors.accent
                                }
                            } else {
                                Hsla {
                                    a: 0.0,
                                    ..colors.border
                                }
                            })
                            .bg(if selected {
                                Hsla {
                                    a: 0.05,
                                    ..colors.accent
                                }
                            } else {
                                Hsla {
                                    a: 0.0,
                                    ..colors.surface
                                }
                            })
                            .hover(|style| {
                                style.bg(Hsla {
                                    a: 0.06,
                                    ..colors.surface_hover
                                })
                            })
                            .shadow(if selected {
                                vec![BoxShadow {
                                    color: Hsla {
                                        a: 0.10,
                                        ..colors.accent
                                    },
                                    blur_radius: px(10.0),
                                    spread_radius: px(-6.0),
                                    offset: point(px(0.), px(2.)),
                                }]
                            } else {
                                Vec::new()
                            })
                            .child(
                                div()
                                    .flex()
                                    .gap(px(10.))
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(46.))
                                            .h(px(46.))
                                            .rounded(px(10.))
                                            .overflow_hidden()
                                            .border_1()
                                            .border_color(Hsla {
                                                a: 0.22,
                                                ..colors.border
                                            })
                                            .bg(rgb(0xffffff))
                                            .shadow(vec![BoxShadow {
                                                color: Hsla {
                                                    h: 0.0,
                                                    s: 0.0,
                                                    l: 0.0,
                                                    a: 0.10,
                                                },
                                                blur_radius: px(8.0),
                                                spread_radius: px(-4.0),
                                                offset: point(px(0.), px(2.)),
                                            }])
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                img(icon)
                                                    .size_full()
                                                    .rounded(px(10.))
                                                    .object_fit(ObjectFit::Cover),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.))
                                            .flex()
                                            .flex_col()
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .text_size(px(14.))
                                                    .font_weight(FontWeight::BOLD)
                                                    .text_color(colors.text_primary)
                                                    .overflow_hidden()
                                                    .text_ellipsis()
                                                    .child(version.folder.clone()),
                                            )
                                            .child(
                                                div()
                                                    .flex()
                                                    .items_center()
                                                    .gap(px(6.))
                                                    .overflow_hidden()
                                                    .child(
                                                        div()
                                                            .text_size(px(11.))
                                                            .text_color(colors.text_secondary)
                                                            .overflow_hidden()
                                                            .text_ellipsis()
                                                            .child(version.version.clone()),
                                                    )
                                                    .child(
                                                        div()
                                                            .px(px(5.))
                                                            .py(px(1.))
                                                            .rounded(px(4.))
                                                            .bg(badge_bg)
                                                            .text_size(px(9.))
                                                            .font_weight(FontWeight::BOLD)
                                                            .text_color(badge_fg)
                                                            .child(version_badge),
                                                    ),
                                            ),
                                    ),
                            )
                            .on_mouse_down(MouseButton::Left, {
                                let folder = folder.clone();
                                cx.listener(move |this, _, _, cx| {
                                    this.select_version(folder.clone(), cx);
                                })
                            })
                    })),
            )
    }

    fn render_main(
        &mut self,
        window: &mut Window,
        colors: &ThemeColors,
        state: &ManagePageState,
        cx: &mut Context<Self>,
    ) -> Div {
        if is_level_dat_editor_route(cx) {
            return div()
                .flex_1()
                .h_full()
                .min_w(px(0.))
                .min_h(px(0.))
                .overflow_hidden()
                .child(self.level_dat_editor.as_ref().map_or_else(
                    || {
                        panel_shell(colors)
                            .size_full()
                            .p(px(20.))
                            .child(empty_state(
                                colors,
                                "images/manage/empty.svg",
                                "编辑器状态不可用",
                                "返回资源列表后重新打开 level.dat 编辑器。",
                            ))
                            .into_any_element()
                    },
                    |editor| {
                        level_dat_editor::render_page(
                            editor,
                            colors,
                            &cx.global::<I18n>().clone(),
                            cx.entity().downgrade(),
                            cx,
                        )
                    },
                ));
        }

        let Some(version) = self.selected_version(state) else {
            return panel_shell(colors).flex_1().h_full().child(empty_state(
                colors,
                "images/manage/empty.svg",
                "请选择一个版本",
                "左侧列表会展示所有本地已导入的游戏实例。",
            ));
        };

        if is_asset_tab(state.tab) && self.asset_list_cache.refresh(state) {
            self.asset_scroll_handle.set_offset(point(px(0.), px(0.)));
        }
        if state.tab == ManageTab::Screenshot && self.screenshot_list_cache.refresh(state) {
            self.screenshot_scroll_handle
                .set_offset(point(px(0.), px(0.)));
        }
        if state.tab == ManageTab::Server && self.server_list_cache.refresh(state) {
            self.server_scroll_handle.set_offset(point(px(0.), px(0.)));
        }
        let filtered_assets = self.asset_list_cache.filtered_indices();
        let filtered_screenshots = self.screenshot_list_cache.filtered_indices();
        let filtered_servers = self.server_list_cache.filtered_indices();
        let active_count = match state.tab {
            ManageTab::Mod | ManageTab::ResourcePack | ManageTab::Map => filtered_assets.len(),
            ManageTab::Screenshot => filtered_screenshots.len(),
            ManageTab::Server => filtered_servers.len(),
        };

        let main_panel = div()
            .flex_1()
            .h_full()
            .relative()
            .overflow_hidden()
            .rounded(px(12.))
            .border_1()
            .border_color(Hsla {
                a: 0.18,
                ..colors.border
            })
            .bg(colors.settings_panel_bg)
            .flex()
            .flex_col()
            .child(
                div()
                    .px(px(18.))
                    .pt(px(12.))
                    .pb(px(0.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(14.))
                    .child(render_version_header(colors, version, state))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(self.level_dat_editor.as_ref().map_or_else(
                                || div().into_any_element(),
                                |_| {
                                    toolbar_glyph_button(
                                        "manage-resume-level-dat-editor",
                                        lucide_icons::icon_file_pen_line(),
                                        colors,
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.resume_level_dat_editor(cx);
                                        }),
                                    )
                                    .into_any_element()
                                },
                            ))
                            .child(
                                toolbar_glyph_button(
                                    "manage-open-folder",
                                    lucide_icons::icon_folder_open(),
                                    colors,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.open_selected_version_folder(cx);
                                    }),
                                ),
                            )
                            .child(
                                toolbar_glyph_button(
                                    "manage-version-settings",
                                    lucide_icons::icon_settings(),
                                    colors,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.open_version_settings(cx);
                                    }),
                                ),
                            )
                            .child(
                                toolbar_glyph_button(
                                    "manage-delete-version",
                                    lucide_icons::icon_trash_2(),
                                    colors,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.request_delete_version(cx);
                                    }),
                                ),
                            )
                            .child(
                                toolbar_glyph_button(
                                    "manage-launch-version",
                                    lucide_icons::icon_play(),
                                    colors,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.launch_selected_version(cx);
                                    }),
                                ),
                            ),
                    ),
            )
            .child(
                div()
                    .px(px(18.))
                    .pt(px(6.))
                    .child(render_tab_bar(colors, state, cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .mt(px(8.))
                    .rounded_b(px(12.))
                    .overflow_hidden()
                    .border_t_1()
                    .border_color(Hsla {
                        a: 0.10,
                        ..colors.border
                    })
                    .bg(Hsla {
                        a: 1.0,
                        ..colors.surface
                    })
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(12.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .when(state.tab == ManageTab::ResourcePack, |this| {
                                        this.child(render_pack_subtype_switch(colors, state, cx))
                                    })
                                    .when(should_render_gdk_dropdown(state, version), |this| {
                                        this.child(render_gdk_dropdown(colors, state, cx))
                                    })
                                    .child(
                                        match state.tab {
                                            ManageTab::Mod
                                            | ManageTab::ResourcePack
                                            | ManageTab::Map => self.asset_search_input.as_ref(),
                                            ManageTab::Screenshot => {
                                                self.screenshot_search_input.as_ref()
                                            }
                                            ManageTab::Server => self.server_search_input.as_ref(),
                                        }
                                        .map_or_else(
                                            || div().w(px(144.)).h(px(32.)).into_any_element(),
                                            |input| {
                                                render_toolbar_search_input(input, colors, px(144.))
                                            },
                                        ),
                                    )
                                    .child(subtle_badge(colors, format!("{active_count} 项"))),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .children(render_active_toolbar_actions(colors, state, cx)),
                            ),
                    )
                    .child(div().flex_1().min_h(px(0.)).child(match state.tab {
                        ManageTab::Mod | ManageTab::ResourcePack | ManageTab::Map => {
                            render_asset_list(
                                colors,
                                version,
                                state,
                                filtered_assets,
                                &self.asset_scroll_handle,
                                cx,
                            )
                        }
                        ManageTab::Screenshot => render_screenshot_list(
                            colors,
                            version,
                            state,
                            filtered_screenshots,
                            &self.screenshot_scroll_handle,
                            cx,
                        ),
                        ManageTab::Server => render_server_list(
                            colors,
                            version,
                            state,
                            filtered_servers,
                            &self.server_scroll_handle,
                            cx,
                        ),
                    })),
            );

        main_panel
    }
}
