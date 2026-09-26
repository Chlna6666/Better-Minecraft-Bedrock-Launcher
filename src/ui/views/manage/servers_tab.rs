use super::*;
use crate::ui::animation::{
    settled_animation, tab_list_item_motion, tab_list_stagger_active,
};

#[derive(Clone, PartialEq, Eq)]
pub(super) struct ServerListSignature {
    pub(super) servers_ptr: usize,
    pub(super) servers_len: usize,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) query: SharedString,
}

#[derive(Default)]
pub(super) struct ServerListRenderCache {
    pub(super) signature: Option<ServerListSignature>,
    pub(super) filtered_indices: Vec<usize>,
}

#[derive(Clone)]
pub(super) struct ServerEditorDialogState {
    pub(super) version: ManagedVersionEntry,
    pub(super) config: ManageVersionConfig,
    pub(super) selected_gdk_user: Option<SharedString>,
    pub(super) editing_key: Option<SharedString>,
    pub(super) name_input: Entity<InputState>,
    pub(super) address_input: Entity<InputState>,
    pub(super) port_input: Entity<InputState>,
    pub(super) pending: bool,
}

impl ManagePageView {
    pub(super) fn refresh_servers(&mut self, cx: &mut Context<Self>) {
        self.last_servers_signature = None;
        self.reset_server_list_view();
        cx.update_global(|state: &mut ManagePageState, _cx| {
            state.servers_loaded = false;
            state.servers_loading = false;
            state.servers_error = None;
            state.server_motd = Arc::new(HashMap::new());
            state.server_motd_loading = false;
            state.server_motd_request_id = state.server_motd_request_id.wrapping_add(1);
        });
        cx.notify();
    }
    pub(super) fn request_delete_server(
        &mut self,
        entry: ManageServerEntry,
        cx: &mut Context<Self>,
    ) {
        let state = cx.global::<ManagePageState>();
        let Some(version) = self.selected_version(state).cloned() else {
            return;
        };
        let _i18n = cx.global::<I18n>().clone();
        self.confirm_dialog = Some(ConfirmDialogState {
            title: t!("ManagePage.server_delete"),
            description: t!(
                "ManagePage.server_delete_confirm",
                name = &entry.name,
                address = &entry.address
            ),
            confirm_label: t!("ManagePage.server_delete"),
            danger: true,
            pending: false,
            action: ConfirmAction::DeleteServer {
                version,
                config: state.version_config.clone(),
                selected_gdk_user: state.selected_gdk_user.clone(),
                entry,
            },
        });
        cx.notify();
    }

    pub(super) fn open_add_server_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (version, config, selected_gdk_user) = {
            let state = cx.global::<ManagePageState>();
            let Some(version) = self.selected_version(state).cloned() else {
                return;
            };
            (
                version,
                state.version_config.clone(),
                state.selected_gdk_user.clone(),
            )
        };
        let Some(name_input) =
            create_text_input(window, cx, t!("ManagePage.server_name").as_ref(), "")
        else {
            return;
        };
        let Some(address_input) =
            create_text_input(window, cx, t!("ManagePage.server_address").as_ref(), "")
        else {
            return;
        };
        let Some(port_input) =
            create_text_input(window, cx, t!("ManagePage.server_port").as_ref(), "19132")
        else {
            return;
        };
        self.server_editor_dialog = Some(ServerEditorDialogState {
            version,
            config,
            selected_gdk_user,
            editing_key: None,
            name_input,
            address_input,
            port_input,
            pending: false,
        });
        cx.notify();
    }

    pub(super) fn open_edit_server_dialog(
        &mut self,
        entry: ManageServerEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (version, config, selected_gdk_user) = {
            let state = cx.global::<ManagePageState>();
            let Some(version) = self.selected_version(state).cloned() else {
                return;
            };
            (
                version,
                state.version_config.clone(),
                state.selected_gdk_user.clone(),
            )
        };
        let Some(name_input) = create_text_input(
            window,
            cx,
            t!("ManagePage.server_name").as_ref(),
            entry.name.as_ref(),
        ) else {
            return;
        };
        let Some(address_input) = create_text_input(
            window,
            cx,
            t!("ManagePage.server_address").as_ref(),
            entry.address.as_ref(),
        ) else {
            return;
        };
        let Some(port_input) = create_text_input(
            window,
            cx,
            t!("ManagePage.server_port").as_ref(),
            &entry.port.to_string(),
        ) else {
            return;
        };
        self.server_editor_dialog = Some(ServerEditorDialogState {
            version,
            config,
            selected_gdk_user,
            editing_key: Some(entry.key),
            name_input,
            address_input,
            port_input,
            pending: false,
        });
        cx.notify();
    }

    pub(super) fn close_server_editor_dialog(&mut self, cx: &mut Context<Self>) {
        self.server_editor_dialog = None;
        cx.notify();
    }

    pub(super) fn save_server_editor_dialog(&mut self, cx: &mut Context<Self>) {
        let _i18n = cx.global::<I18n>().clone();
        let Some(dialog) = self.server_editor_dialog.as_mut() else {
            return;
        };
        if dialog.pending {
            return;
        }

        let name = dialog.name_input.read(cx).value().to_string();
        let address = dialog.address_input.read(cx).value().to_string();
        if name.trim().is_empty() {
            toast::error(cx, t!("ManagePage.server_name_required"));
            return;
        }
        if address.trim().is_empty() {
            toast::error(cx, t!("ManagePage.server_address_required"));
            return;
        }
        let port_text = dialog.port_input.read(cx).value().to_string();
        let port = match port_text.trim().parse::<u16>() {
            Ok(port) if port != 0 => port,
            Ok(_) => {
                toast::error(cx, t!("ManagePage.server_port_positive"));
                return;
            }
            Err(error) => {
                toast::error(cx, t!("ManagePage.server_port_invalid", error = &error));
                return;
            }
        };

        let version = dialog.version.clone();
        let config = dialog.config.clone();
        let selected_gdk_user = dialog.selected_gdk_user.clone();
        let editing_key = dialog.editing_key.clone();
        let editing = editing_key.is_some();
        let task_result = if let Some(key) = editing_key {
            data::start_update_external_server_task(
                &version,
                &config,
                selected_gdk_user.as_ref().map(SharedString::as_ref),
                key.to_string(),
                name,
                address,
                port,
            )
        } else {
            data::start_add_external_server_task(
                &version,
                &config,
                selected_gdk_user.as_ref().map(SharedString::as_ref),
                name,
                address,
                port,
            )
        };

        match task_result {
            Ok(task_id) => {
                self.server_editor_dialog = None;
                let view_handle = cx.entity().downgrade();
                watch_server_mutation_task(
                    task_id,
                    if editing {
                        t!("ManagePage.server_saved")
                    } else {
                        t!("ManagePage.server_added")
                    },
                    view_handle,
                    cx,
                );
            }
            Err(error) => {
                if let Some(dialog) = self.server_editor_dialog.as_mut() {
                    dialog.pending = false;
                }
                toast::error(cx, SharedString::from(error));
            }
        }
        cx.notify();
    }
}

impl ServerListSignature {
    pub(super) fn from_state(state: &ManagePageState) -> Self {
        Self {
            servers_ptr: state.servers.as_ref().as_ptr() as usize,
            servers_len: state.servers.len(),
            selected_gdk_user: state.selected_gdk_user.clone(),
            query: SharedString::from(state.server_search_query.trim().to_string()),
        }
    }
}

impl ServerListRenderCache {
    pub(super) fn clear(&mut self) {
        self.signature = None;
        self.filtered_indices.clear();
    }

    pub(super) fn refresh(&mut self, state: &ManagePageState) -> bool {
        let signature = ServerListSignature::from_state(state);
        if self.signature.as_ref() == Some(&signature) {
            return false;
        }
        self.filtered_indices = build_filtered_server_indices(state, &signature);
        self.signature = Some(signature);
        true
    }

    pub(super) fn filtered_indices(&self) -> &[usize] {
        &self.filtered_indices
    }
}

fn cmp_ascii_case_insensitive(left: &str, right: &str) -> std::cmp::Ordering {
    left.bytes()
        .map(|byte| byte.to_ascii_lowercase())
        .cmp(right.bytes().map(|byte| byte.to_ascii_lowercase()))
}

pub(super) fn build_filtered_server_indices(
    state: &ManagePageState,
    signature: &ServerListSignature,
) -> Vec<usize> {
    let query = signature.query.as_ref();
    let mut indices = Vec::with_capacity(state.servers.len());
    for (index, server) in state.servers.iter().enumerate() {
        if query.is_empty()
            || text_contains_query(&server.name, &query)
            || text_contains_query(&server.address, &query)
        {
            indices.push(index);
        }
    }
    indices.sort_by(|left, right| {
        let left = &state.servers[*left];
        let right = &state.servers[*right];
        cmp_ascii_case_insensitive(left.name.as_ref(), right.name.as_ref())
            .then_with(|| left.address.as_ref().cmp(right.address.as_ref()))
            .then_with(|| left.port.cmp(&right.port))
    });
    indices
}
pub(super) fn render_server_list(
    colors: &ThemeColors,
    version: &ManagedVersionEntry,
    state: &ManagePageState,
    filtered_indices: &[usize],
    scroll_handle: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    if state.gdk_users_loading && state.gdk_users.is_empty() && version.is_gdk() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            t!("AssetManager.user_scan_title"),
            t!("AssetManager.user_scan_hint"),
        )
        .into_any_element();
    }

    if version.is_gdk() && state.selected_gdk_user.is_none() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            t!("AssetManager.user_scan_empty_title"),
            t!("AssetManager.user_scan_empty_hint"),
        )
        .into_any_element();
    }

    if state.servers_loading && state.servers.is_empty() {
        return render_manage_loading_rows(colors, 6).into_any_element();
    }

    if let Some(error) = state.servers_error.clone() {
        return error_panel(colors, error);
    }

    if filtered_indices.is_empty() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            t!("AssetManager.empty_server"),
            t!("AssetManager.empty_server_hint"),
        )
        .into_any_element();
    }

    let scroll_handle_for_event = scroll_handle.clone();
    let virtual_list_plan = compute_virtual_list_plan(
        filtered_indices.len(),
        MANAGE_ASSET_ROW_PITCH_PX,
        scroll_handle.offset().y,
        scroll_handle.bounds().size.height,
        MANAGE_ASSET_ROW_OVERSCAN,
        MANAGE_ASSET_HEAVY_BUDGET,
    );

    let animate_rows = state.servers_loaded
        && state.tab_anim_seq != 0
        && state.tab_anim_from != state.tab
        && !crate::core::ui_prefs::reduced_motion()
        && tab_list_stagger_active(window, cx, "manage-server-list-stagger", state.tab_anim_seq);
    let animation_from = state.tab_anim_from.index();
    let animation_to = state.tab.index();

    let mut rows = div().w_full().flex().flex_col().min_w(px(0.));
    if virtual_list_plan.render_slice.top_spacer > px(0.) {
        rows = rows.child(div().h(virtual_list_plan.render_slice.top_spacer));
    }

    for virtual_index in virtual_list_plan.render_slice.start_index
        ..virtual_list_plan
            .render_slice
            .end_index
            .min(filtered_indices.len())
    {
        let Some(index) = filtered_indices.get(virtual_index).copied() else {
            continue;
        };
        let Some(entry) = state.servers.get(index) else {
            continue;
        };
        let motd_status = state.server_motd.get(&entry.key);
        let animate_row =
            animate_rows && virtual_list_plan.visible_slice.contains(virtual_index);
        let visible_index =
            virtual_index.saturating_sub(virtual_list_plan.visible_slice.start_index);
        let row = div()
            .w_full()
            .h(px(MANAGE_ASSET_ROW_PITCH_PX))
            .pb(px(MANAGE_ASSET_ROW_GAP_PX))
            .flex_none()
            .child(render_server_row(colors, entry, motd_status, cx));
        let direction =
            crate::ui::animation::tab_transition_direction(animation_from, animation_to);
        let row = row
            .with_animation(
                SharedString::from(format!(
                    "manage-server-row-enter-{}-{}",
                    state.tab_anim_seq,
                    entry.key.as_ref()
                )),
                if animate_row {
                    tab_list_item_motion(animation_from, animation_to, visible_index)
                        .with_property(AnimationProperty::translation(
                            point(px(12.0 * direction), px(0.0)),
                            Point::default(),
                        ))
                } else {
                    settled_animation().with_property(AnimationProperty::translation(
                        Point::default(),
                        Point::default(),
                    ))
                },
                |row, _progress| row,
            )
            .into_any_element();
        rows = rows.child(row);
    }

    if virtual_list_plan.render_slice.bottom_spacer > px(0.) {
        rows = rows.child(div().h(virtual_list_plan.render_slice.bottom_spacer));
    }

    div()
        .id("manage-server-list-scroll")
        .w_full()
        .h_full()
        .min_h(px(0.))
        .min_w(px(0.))
        .overflow_y_scroll()
        .track_scroll(scroll_handle)
        .on_scroll_wheel(move |event, window, cx| {
            clamp_scroll_at_edges(&scroll_handle_for_event, event, window, cx);
        })
        .child(rows)
        .into_any_element()
}
pub(super) fn render_server_row(
    colors: &ThemeColors,
    entry: &ManageServerEntry,
    motd_status: Option<&ManageServerMotdStatus>,
    cx: &mut Context<ManagePageView>,
) -> Stateful<Div> {
    let _i18n = cx.global::<I18n>();
    let key = entry.key.clone();
    let mut badges = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .overflow_hidden()
        .flex_shrink_0();
    let mut error_text = None;

    let motd_line = match motd_status {
        Some(ManageServerMotdStatus::Online(motd)) => {
            if let Some(version) = motd.version.clone() {
                badges = badges.child(subtle_badge(colors, version));
            }
            if let (Some(online), Some(max)) = (motd.players_online, motd.players_max) {
                badges = badges.child(subtle_badge(colors, format!("{online}/{max}")));
            }
            if let Some(latency) = motd.latency_ms {
                badges = badges.child(subtle_badge(
                    colors,
                    t!("ManagePage.server_latency", latency = latency),
                ));
            }
            Some(if let Some(line_2) = motd.line_2.clone() {
                SharedString::from(format!("{} {}", motd.line_1, line_2))
            } else {
                motd.line_1.clone()
            })
        }
        Some(ManageServerMotdStatus::Loading) => {
            badges = badges.child(subtle_badge(colors, t!("ManagePage.server_querying")));
            None
        }
        Some(ManageServerMotdStatus::Offline(error)) => {
            badges = badges.child(subtle_badge(colors, t!("ManagePage.server_offline")));
            error_text = Some(error.clone());
            None
        }
        None => {
            badges = badges.child(subtle_badge(colors, t!("ManagePage.server_not_queried")));
            None
        }
    };

    let actions = div()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.))
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-server-edit-{}", entry.key)),
                lucide_gpui::icon!(file_pen_line),
            )
            .on_mouse_down(MouseButton::Left, {
                let key = entry.key.clone();
                cx.listener(move |this, _, window, cx| {
                    let server = resolve_server_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(server) = server {
                        this.open_edit_server_dialog(server, window, cx);
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-server-refresh-{}", entry.key)),
                lucide_gpui::icon!(refresh_cw),
            )
            .on_mouse_down(MouseButton::Left, {
                cx.listener(move |this, _, _, cx| {
                    let server = resolve_server_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(server) = server {
                        this.request_server_motds(vec![ManageServerMotdTarget::from(&server)], cx);
                    }
                })
            }),
        )
        .child(
            compact_icon_button(
                colors,
                SharedString::from(format!("manage-server-delete-{}", entry.key)),
                lucide_gpui::icon!(trash_2),
            )
            .on_mouse_down(MouseButton::Left, {
                let key = entry.key.clone();
                cx.listener(move |this, _, _, cx| {
                    let server = resolve_server_by_key(cx.global::<ManagePageState>(), &key);
                    if let Some(server) = server {
                        this.request_delete_server(server, cx);
                    }
                })
            }),
        );

    let title = div()
        .flex_1()
        .min_w(px(0.))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(colors.text_primary)
        .child(entry.name.clone());
    let title_row = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w(px(0.))
        .child(title)
        .child(badges);
    let has_motd_line = motd_line.is_some();

    div()
        .id(SharedString::from(format!(
            "manage-server-row-{}",
            entry.key
        )))
        .w_full()
        .h(px(MANAGE_ASSET_ROW_HEIGHT_PX))
        .rounded(px(crate::ui::theme::tokens::radius::SM))
        .border_1()
        .border_color(colors.border)
        .bg(colors.surface)
        .px(px(10.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .w(px(32.))
                .h(px(32.))
                .rounded(px(crate::ui::theme::tokens::radius::MD))
                .bg(colors.surface_hover)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_gpui::icon!(server))
                        .w(px(16.))
                        .h(px(16.))
                        .text_color(colors.text_secondary),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(title_row)
                .child(
                    div()
                        .overflow_hidden()
                        .when_some(motd_line, |this, text| {
                            this.child(
                                MinecraftFormattedText::new(text, colors)
                                    .text_size(px(11.))
                                    .line_height(relative(1.2))
                                    .color(colors.text_secondary)
                                    .wrap(false),
                            )
                        })
                        .when(!has_motd_line, |this| {
                            this.when_some(error_text, |this, error| {
                                this.child(
                                    div()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_size(px(11.))
                                        .text_color(colors.danger)
                                        .child(error),
                                )
                            })
                        }),
                ),
        )
        .child(actions)
}
pub(super) fn render_server_editor_dialog(
    dialog: &ServerEditorDialogState,
    colors: &ThemeColors,
    _i18n: &I18n,
    view_handle: WeakEntity<ManagePageView>,
) -> AnyElement {
    let editing = dialog.editing_key.is_some();
    let dismiss_handle = view_handle.clone();
    let dismiss = Rc::new(move |cx: &mut App| {
        let _ = dismiss_handle.update(cx, |this, cx| {
            if this
                .server_editor_dialog
                .as_ref()
                .is_some_and(|dialog| dialog.pending)
            {
                return;
            }
            this.close_server_editor_dialog(cx);
        });
    });

    let input_row = |label: SharedString, input: &Entity<InputState>| {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(colors.text_secondary)
                    .child(label),
            )
            .child(Input::new(input).with_size(InputSize::Medium).w_full())
    };

    modal::modal_layer_dismissible(
        div()
            .w_full()
            .max_w(px(520.))
            .rounded(px(crate::ui::theme::tokens::radius::MD))
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
                    .p(px(22.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_weight(FontWeight::BOLD)
                            .text_color(colors.text_primary)
                            .child(if editing {
                                t!("ManagePage.server_edit")
                            } else {
                                t!("ManagePage.server_add")
                            }),
                    )
                    .child(input_row(t!("ManagePage.server_name"), &dialog.name_input))
                    .child(input_row(
                        t!("ManagePage.server_address"),
                        &dialog.address_input,
                    ))
                    .child(input_row(t!("ManagePage.server_port"), &dialog.port_input)),
            )
            .child(
                div()
                    .px(px(22.))
                    .pb(px(22.))
                    .flex()
                    .justify_end()
                    .gap(px(10.))
                    .child({
                        let view_handle = view_handle.clone();
                        ghost_button(
                            colors,
                            "manage-server-editor-cancel",
                            t!("ManagePage.server_cancel"),
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.close_server_editor_dialog(cx);
                                });
                            },
                        )
                    })
                    .child({
                        let view_handle = view_handle.clone();
                        primary_button(
                            colors,
                            "manage-server-editor-save",
                            if dialog.pending {
                                t!("ManagePage.server_saving")
                            } else if editing {
                                t!("common.save")
                            } else {
                                t!("ManagePage.server_add")
                            },
                        )
                        .opacity(if dialog.pending { 0.72 } else { 1.0 })
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_, _, cx| {
                                let _ = view_handle.update(cx, |this, cx| {
                                    this.save_server_editor_dialog(cx);
                                });
                            },
                        )
                    }),
            ),
        colors.backdrop,
        dismiss,
    )
    .into_any_element()
}
