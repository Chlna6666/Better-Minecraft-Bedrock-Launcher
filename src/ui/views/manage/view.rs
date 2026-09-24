use super::*;
use crate::ui::animation::{tab_content_animation_key, tab_content_motion};

pub struct ManagePageView {
    pub(super) _subscriptions: Vec<Subscription>,
    pub(super) active: bool,
    pub(super) asset_search_input: Option<Entity<InputState>>,
    pub(super) screenshot_search_input: Option<Entity<InputState>>,
    pub(super) server_search_input: Option<Entity<InputState>>,
    pub(super) asset_scroll_handle: ScrollHandle,
    pub(super) screenshot_scroll_handle: ScrollHandle,
    pub(super) server_scroll_handle: ScrollHandle,
    pub(super) version_scroll_handle: ScrollHandle,
    pub(super) version_list_cache: VersionListRenderCache,
    pub(super) asset_list_cache: AssetListRenderCache,
    pub(super) screenshot_list_cache: ScreenshotListRenderCache,
    pub(super) server_list_cache: ServerListRenderCache,
    pub(super) version_settings_modal: Option<version_settings::VersionSettingsModalState>,
    pub(super) confirm_dialog: Option<ConfirmDialogState>,
    pub(super) value_prompt: Option<ValuePromptDialogState>,
    pub(super) mod_type_dialog: Option<ModTypeDialogState>,
    pub(super) drop_hover: Option<ManageDropHoverState>,
    pub(super) pending_mod_import_dialogs: VecDeque<ModTypeDialogState>,
    pub(super) pending_mod_import_items: Vec<crate::core::native_mods::NativeModImportItem>,
    pub(super) server_editor_dialog: Option<ServerEditorDialogState>,
    pub(super) level_dat_editor: Option<level_dat_editor::LevelDatEditorModalState>,
    pub(super) last_selected_instance_revision: ManagedInstanceRevision,
    pub(super) last_version_config_signature: Option<VersionConfigLoadSignature>,
    pub(super) last_gdk_users_signature: Option<GdkUsersLoadSignature>,
    pub(super) last_assets_signature: Option<AssetsLoadSignature>,
    pub(super) last_screenshots_signature: Option<ScreenshotsLoadSignature>,
    pub(super) last_servers_signature: Option<ServersLoadSignature>,
    pub(super) last_global_render_signature: ManageRenderSignature,
}

impl ManagePageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.reset_transient_requests();
        });
        let state = cx.global::<ManagePageState>();
        let initial_selected_instance_revision = state.selected_instance_revision();
        let initial_render_signature = ManageRenderSignature::from_state(state);
        let subscriptions = vec![
            cx.observe_global::<ManagePageState>(|this, cx| {
                let signature = ManageRenderSignature::from_state(cx.global::<ManagePageState>());
                if this.last_global_render_signature != signature {
                    this.last_global_render_signature = signature;
                    if this.active {
                        cx.notify();
                    }
                }
            }),
            cx.observe_global::<ThemeState>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
            cx.observe_global::<I18n>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
            cx.observe_global::<ImportCompletionState>(|this, cx| {
                let imported_folder = cx.global::<ImportCompletionState>().version_folder.clone();
                let is_selected = imported_folder.is_some_and(|folder| {
                    cx.global::<ManagePageState>()
                        .selected_folder
                        .as_ref()
                        .is_some_and(|selected| selected == &folder)
                });
                if is_selected {
                    cx.update_global(|state: &mut ManagePageState, _cx| {
                        state.selected_asset_keys.clear();
                        state.assets_loaded = false;
                        state.assets_loading = false;
                        state.assets_error = None;
                    });
                    this.last_assets_signature = None;
                    this.reset_asset_list_view();
                    if this.active {
                        cx.notify();
                    }
                }
            }),
            cx.observe_global::<crate::ui::views::settings::state::SettingsPageState>(|this, cx| {
                if this.active {
                    cx.notify();
                }
            }),
        ];

        Self {
            _subscriptions: subscriptions,
            active: false,
            asset_search_input: None,
            screenshot_search_input: None,
            server_search_input: None,
            asset_scroll_handle: ScrollHandle::new(),
            screenshot_scroll_handle: ScrollHandle::new(),
            server_scroll_handle: ScrollHandle::new(),
            version_scroll_handle: ScrollHandle::new(),
            version_list_cache: VersionListRenderCache::default(),
            asset_list_cache: AssetListRenderCache::default(),
            screenshot_list_cache: ScreenshotListRenderCache::default(),
            server_list_cache: ServerListRenderCache::default(),
            version_settings_modal: None,
            confirm_dialog: None,
            value_prompt: None,
            mod_type_dialog: None,
            drop_hover: None,
            pending_mod_import_dialogs: VecDeque::new(),
            pending_mod_import_items: Vec::new(),
            server_editor_dialog: None,
            level_dat_editor: None,
            last_selected_instance_revision: initial_selected_instance_revision,
            last_version_config_signature: None,
            last_gdk_users_signature: None,
            last_assets_signature: None,
            last_screenshots_signature: None,
            last_servers_signature: None,
            last_global_render_signature: initial_render_signature,
        }
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            // Inactive residents absorb state/signature updates without waking the window. One
            // notification on reactivation is enough to render the latest state.
            cx.notify();
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
        if !cx.has_active_drag() {
            self.drop_hover = None;
        }
        self.ensure_asset_search_input(window, cx);
        self.ensure_screenshot_search_input(window, cx);
        self.ensure_server_search_input(window, cx);
        self.sync_selected_version(cx);
        self.sync_data_requests(cx);

        let now = window.animation_time();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let state = cx.global::<ManagePageState>().clone();
        let page = self.render_page(window, &colors, &state, now, cx);

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
        now: Instant,
        cx: &mut Context<Self>,
    ) -> Div {
        if is_level_dat_editor_route(cx) {
            return div()
                .size_full()
                .min_w(px(0.))
                .min_h(px(0.))
                .child(self.render_main(window, colors, state, now, cx));
        }

        let page = crate::ui::components::page_shell::split_page(
            self.render_sidebar(colors, state, cx),
            self.render_main(window, colors, state, now, cx),
        )
        .relative();
        if let Some(target) = self.drop_hover.as_ref().map(|preview| preview.target) {
            page.when_some(
                self.render_drop_hover_overlay(target, colors, cx),
                |this, overlay| this.child(overlay),
            )
        } else {
            page
        }
    }

    fn render_sidebar(
        &mut self,
        colors: &ThemeColors,
        state: &ManagePageState,
        cx: &mut Context<Self>,
    ) -> Div {
        let _i18n = cx.global::<I18n>().clone();
        if matches!(
            self.version_list_cache.refresh(state),
            VersionListRefresh::QueryChanged
        ) {
            self.version_scroll_handle.set_offset(point(px(0.), px(0.)));
        }
        let filtered_version_indices = self.version_list_cache.filtered_indices();
        let virtual_list_plan = compute_virtual_list_plan(
            filtered_version_indices.len(),
            MANAGE_VERSION_ROW_PITCH_PX,
            self.version_scroll_handle.offset().y,
            self.version_scroll_handle.bounds().size.height,
            MANAGE_VERSION_ROW_OVERSCAN,
            usize::MAX,
        );
        let no_versions = filtered_version_indices.is_empty();
        let visible_version_indices = &filtered_version_indices
            [virtual_list_plan.render_slice.start_index
                ..virtual_list_plan
                    .render_slice
                    .end_index
                    .min(filtered_version_indices.len())];
        let version_scroll_handle_for_event = self.version_scroll_handle.clone();
        crate::ui::components::page_shell::split_sidebar_panel(colors)
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .on_drag_move::<ExternalPaths>(cx.listener(
                |this, event: &DragMoveEvent<ExternalPaths>, _window, cx| {
                    let paths = event.drag(cx).paths().to_vec();
                    this.update_drop_hover(ManageDropTarget::Versions, &paths, cx);
                },
            ))
            .on_drop(cx.listener(
                |this, paths: &ExternalPaths, _window, cx| {
                    this.import_dropped_versions(paths.paths(), cx);
                    cx.stop_propagation();
                },
            ))
            .child(state.search_input.as_ref().map_or_else(
                || div().h(px(32.)).w_full().into_any_element(),
                |input| {
                    div()
                        .w_full()
                        .min_w(px(0.))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .child(render_toolbar_search_input(input, colors)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .child(
                                    sidebar_icon_button(
                                        "manage-import-version",
                                        lucide_gpui::icon!(plus),
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
                                        lucide_gpui::icon!(refresh_cw),
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
                    .track_scroll(&self.version_scroll_handle)
                    .on_scroll_wheel(move |event, window, cx| {
                        clamp_scroll_at_edges(
                            &version_scroll_handle_for_event,
                            event,
                            window,
                            cx,
                        );
                    })
                    .flex()
                    .flex_col()
                    .when(state.loading && state.versions.is_empty(), |this| {
                        this.child(subtle_badge(colors, t!("ManagePage.loading_versions")))
                    })
                    .when_some(state.error.clone(), |this, error| {
                        this.child(
                            div()
                                .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                    .when(no_versions, |this| {
                        this.child(
                            empty_state(
                                colors,
                                "images/manage/empty.svg",
                                t!("ManagePage.no_versions"),
                                t!("ManagePage.no_versions_hint"),
                            )
                            .h(px(220.)),
                        )
                    })
                    .when(virtual_list_plan.render_slice.top_spacer > px(0.), |this| {
                        this.child(div().h(virtual_list_plan.render_slice.top_spacer))
                    })
                    .children(visible_version_indices.iter().filter_map(|&index| {
                        state.versions.get(index)
                    }).map(|version| {
                        let selected = state
                            .selected_folder
                            .as_ref()
                            .is_some_and(|folder| folder == &version.folder);
                        let folder = version.folder.clone();
                        let version_badge = if version.is_gdk() { "GDK" } else { "UWP" };
                        let is_preview = version.is_preview();
                        let channel_label =
                            crate::ui::hooks::use_local_versions::version_channel_label(
                                cx.global::<I18n>(),
                                version.name.as_ref(),
                            );
                        let channel_color = if is_preview {
                            colors.danger
                        } else {
                            colors.accent
                        };
                        let icon = launch_version_icon_path(
                            version
                                .icon_path
                                .as_ref()
                                .map(|icon_path| icon_path.as_ref()),
                            version.name.as_ref(),
                        );
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
                            .h(px(MANAGE_VERSION_ROW_PITCH_PX))
                            .pb(px(MANAGE_VERSION_ROW_GAP_PX))
                            .child(
                                div()
                                    .id(SharedString::from(format!("manage-version-{}", folder)))
                                    .w_full()
                                    .h_full()
                                    .px(px(10.))
                                    .py(px(9.))
                                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                                    .cursor_pointer()
                                    .border_1()
                                    .border_color(if selected {
                                        Hsla {
                                            a: 0.34,
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
                                    .child(
                                        div()
                                            .w_full()
                                            .min_w(px(0.))
                                            .overflow_hidden()
                                            .flex()
                                            .gap(px(10.))
                                            .items_center()
                                            .child(
                                                div()
                                                    .flex_none()
                                                    .w(px(46.))
                                                    .h(px(46.))
                                                    .rounded(px(crate::ui::theme::tokens::radius::SM))
                                                    .overflow_hidden()
                                                    .border_1()
                                                    .border_color(Hsla {
                                                        a: 0.22,
                                                        ..colors.border
                                                    })
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
                                                            .rounded(px(crate::ui::theme::tokens::radius::SM))
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
                                                            .w_full()
                                                            .min_w(px(0.))
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(6.))
                                                            .overflow_hidden()
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .min_w(px(0.))
                                                                    .text_size(px(14.))
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_color(colors.text_primary)
                                                                    .overflow_hidden()
                                                                    .text_ellipsis()
                                                                    .child(version.folder.clone()),
                                                            )
                                                            .when(
                                                                !version.mod_loaders.is_empty(),
                                                                |this| {
                                                                    this.child(
                                                                        div()
                                                                            .flex_none()
                                                                            .max_w(px(118.))
                                                                            .overflow_hidden()
                                                                            .flex()
                                                                            .items_center()
                                                                            .gap(px(4.))
                                                                            .children(
                                                                                version.mod_loaders.iter().map(
                                                                                    |loader| {
                                                                                        render_mod_loader_badge(
                                                                                            colors, loader, true,
                                                                                        )
                                                                                    },
                                                                                ),
                                                                            ),
                                                                    )
                                                                },
                                                            ),
                                                    )
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .min_w(px(0.))
                                                            .flex()
                                                            .items_center()
                                                            .gap(px(6.))
                                                            .overflow_hidden()
                                                            .child(
                                                                div()
                                                                    .flex_1()
                                                                    .min_w(px(0.))
                                                                    .text_size(px(11.))
                                                                    .text_color(
                                                                        colors.text_secondary,
                                                                    )
                                                                    .overflow_hidden()
                                                                    .text_ellipsis()
                                                                    .child(version.version.clone()),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_none()
                                                                    .px(px(5.))
                                                                    .py(px(1.))
                                                                    .rounded(px(crate::ui::theme::tokens::radius::XS))
                                                                    .bg(badge_bg)
                                                                    .text_size(px(9.))
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_color(badge_fg)
                                                                    .child(version_badge),
                                                            )
                                                            .child(
                                                                div()
                                                                    .flex_none()
                                                                    .px(px(5.))
                                                                    .py(px(1.))
                                                                    .rounded(px(crate::ui::theme::tokens::radius::XS))
                                                                    .bg(Hsla {
                                                                        a: 0.14,
                                                                        ..channel_color
                                                                    })
                                                                    .text_size(px(9.))
                                                                    .font_weight(FontWeight::BOLD)
                                                                    .text_color(channel_color)
                                                                    .child(channel_label),
                                                            ),
                                                    ),
                                            ),
                                    )
                                    .on_mouse_down(MouseButton::Left, {
                                        let folder = folder.clone();
                                        cx.listener(move |this, _, _, cx| {
                                            this.select_version(folder.clone(), cx);
                                        })
                                    }),
                            )
                    }))
                    .when(
                        virtual_list_plan.render_slice.bottom_spacer > px(0.),
                        |this| this.child(div().h(virtual_list_plan.render_slice.bottom_spacer)),
                    ),
            )
    }

    fn render_main(
        &mut self,
        window: &mut Window,
        colors: &ThemeColors,
        state: &ManagePageState,
        now: Instant,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let i18n = cx.global::<I18n>().clone();
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
                                t!("ManagePage.editor_unavailable"),
                                t!("ManagePage.editor_unavailable_hint"),
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
                ))
                .into_any_element();
        }

        let Some(version) = self.selected_version(state) else {
            let drop_tab = state.tab;
            return crate::ui::components::page_shell::split_content_panel(colors)
                .on_drag_move::<ExternalPaths>(cx.listener(
                    move |this, event: &DragMoveEvent<ExternalPaths>, _window, cx| {
                        let paths = event.drag(cx).paths().to_vec();
                        this.update_drop_hover(ManageDropTarget::Assets(drop_tab), &paths, cx);
                    },
                ))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                    this.import_dropped_assets(paths.paths(), window, cx);
                    cx.stop_propagation();
                }))
                .child(empty_state(
                    colors,
                    "images/manage/empty.svg",
                    t!("ManagePage.select_version"),
                    t!("ManagePage.select_version_hint"),
                ))
                .into_any_element();
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
            ManageTab::Statistics => 0,
            ManageTab::Mod | ManageTab::ResourcePack | ManageTab::SkinPack | ManageTab::Map => {
                filtered_assets.len()
            }
            ManageTab::Screenshot => filtered_screenshots.len(),
            ManageTab::Server => filtered_servers.len(),
        };
        let active_count_string = active_count.to_string();

        let drop_tab = state.tab;
        let main_panel = crate::ui::components::page_shell::split_content_panel(colors)
            .on_drag_move::<ExternalPaths>(cx.listener(
                move |this, event: &DragMoveEvent<ExternalPaths>, _window, cx| {
                    let paths = event.drag(cx).paths().to_vec();
                    this.update_drop_hover(ManageDropTarget::Assets(drop_tab), &paths, cx);
                },
            ))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.import_dropped_assets(paths.paths(), window, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .px(px(18.))
                    .pt(px(12.))
                    .pb(px(0.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(14.))
                    .child(render_version_header(colors, version, state, cx))
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
                                        lucide_gpui::icon!(file_pen_line),
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
                                    lucide_gpui::icon!(folder_open),
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
                                    "manage-create-shortcut",
                                    lucide_gpui::icon!(external_link),
                                    colors,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.create_selected_version_shortcut(cx);
                                    }),
                                ),
                            )
                            .child(
                                toolbar_glyph_button(
                                    "manage-version-settings",
                                    lucide_gpui::icon!(settings),
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
                                    lucide_gpui::icon!(trash_2),
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
                                    lucide_gpui::icon!(play),
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
                        a: 0.55,
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
                            .when(state.tab != ManageTab::Statistics, |this| {
                                this.child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(10.))
                                        .when(state.tab == ManageTab::ResourcePack, |this| {
                                            this.child(render_pack_subtype_switch(
                                                colors, state, cx,
                                            ))
                                        })
                                        .when(should_render_gdk_dropdown(state, version), |this| {
                                            this.child(render_gdk_dropdown(colors, state, cx))
                                        })
                                        .child(
                                            match state.tab {
                                                ManageTab::Statistics => None,
                                                ManageTab::Mod
                                                | ManageTab::ResourcePack
                                                | ManageTab::SkinPack
                                                | ManageTab::Map => {
                                                    self.asset_search_input.as_ref()
                                                }
                                                ManageTab::Screenshot => {
                                                    self.screenshot_search_input.as_ref()
                                                }
                                                ManageTab::Server => {
                                                    self.server_search_input.as_ref()
                                                }
                                            }
                                            .map_or_else(
                                                || div().w(px(144.)).h(px(32.)).into_any_element(),
                                                |input| {
                                                    div()
                                                        .w(px(144.))
                                                        .child(render_toolbar_search_input(
                                                            input, colors,
                                                        ))
                                                        .into_any_element()
                                                },
                                            ),
                                        )
                                        .child(subtle_badge(
                                            colors,
                                            t!(
                                                "ManagePage.active_count",
                                                count = &active_count_string
                                            ),
                                        )),
                                )
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .children(render_active_toolbar_actions(colors, state, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h(px(0.))
                            .child({
                                let content = if state.version_config_loading {
                                    empty_state(
                                        colors,
                                        "images/manage/empty.svg",
                                        "正在读取版本配置",
                                        "请稍候，BMCBL 正在准备当前实例的管理设置。",
                                    )
                                    .into_any_element()
                                } else {
                                    match state.tab {
                                        ManageTab::Statistics => {
                                            render_statistics_tab(colors, version, state, cx)
                                        }
                                        ManageTab::Mod
                                        | ManageTab::ResourcePack
                                        | ManageTab::Map => render_asset_list(
                                            colors,
                                            version,
                                            state,
                                            filtered_assets,
                                            &self.asset_scroll_handle,
                                            window,
                                            cx,
                                        ),
                                        ManageTab::SkinPack => render_skin_pack_management(
                                            colors,
                                            version,
                                            state,
                                            filtered_assets,
                                            &self.asset_scroll_handle,
                                            window,
                                            cx,
                                        ),
                                        ManageTab::Screenshot => render_screenshot_list(
                                            colors,
                                            &i18n,
                                            version,
                                            state,
                                            filtered_screenshots,
                                            &self.screenshot_scroll_handle,
                                            window,
                                            cx,
                                        ),
                                        ManageTab::Server => render_server_list(
                                            colors,
                                            version,
                                            state,
                                            filtered_servers,
                                            &self.server_scroll_handle,
                                            window,
                                            cx,
                                        ),
                                    }
                                };

                                let reduced_motion = crate::core::ui_prefs::reduced_motion();
                                if state.tab_anim_seq != 0
                                    && state.tab_anim_from != state.tab
                                    && !reduced_motion
                                {
                                    div()
                                        .size_full()
                                        .min_w(px(0.))
                                        .min_h(px(0.))
                                        .child(content)
                                        .composite_layer()
                                        .with_animation(
                                            tab_content_animation_key(
                                                "manage-tab-content",
                                                state.tab_anim_seq,
                                            ),
                                            tab_content_motion(
                                                state.tab_anim_from.index(),
                                                state.tab.index(),
                                            ),
                                            |content, _progress| content,
                                        )
                                        .into_any_element()
                                } else if state.pack_subtype_animation_active(now)
                                    && !reduced_motion
                                {
                                    div()
                                        .size_full()
                                        .min_w(px(0.))
                                        .min_h(px(0.))
                                        .child(content)
                                        .composite_layer()
                                        .with_animation(
                                            tab_content_animation_key(
                                                "manage-pack-subtype-content",
                                                state.pack_subtype_anim_seq,
                                            ),
                                            tab_content_motion(
                                                state.pack_subtype_anim_from.index(),
                                                state.pack_subtype.index(),
                                            ),
                                            |content, _progress| content,
                                        )
                                        .into_any_element()
                                } else {
                                    content
                                }
                            }),
                    ),
            );

        // Version/tab changes render at final geometry. The tab controls update independently;
        // page-sized lists, images and editors never become animation cadence targets.
        main_panel.into_any_element()
    }
}
