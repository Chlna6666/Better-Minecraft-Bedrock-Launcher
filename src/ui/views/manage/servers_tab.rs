use super::*;

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
        self.confirm_dialog = Some(ConfirmDialogState {
            title: SharedString::from("删除服务器"),
            description: SharedString::from(format!(
                "确定删除服务器 {} ({}) 吗？",
                entry.name, entry.address
            )),
            confirm_label: SharedString::from("删除服务器"),
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
        let Some(name_input) = create_text_input(window, cx, "服务器名称", "") else {
            return;
        };
        let Some(address_input) = create_text_input(window, cx, "服务器地址", "") else {
            return;
        };
        let Some(port_input) = create_text_input(window, cx, "端口", "19132") else {
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
        let Some(name_input) = create_text_input(window, cx, "服务器名称", entry.name.as_ref())
        else {
            return;
        };
        let Some(address_input) =
            create_text_input(window, cx, "服务器地址", entry.address.as_ref())
        else {
            return;
        };
        let Some(port_input) = create_text_input(window, cx, "端口", &entry.port.to_string())
        else {
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
        let Some(dialog) = self.server_editor_dialog.as_mut() else {
            return;
        };
        if dialog.pending {
            return;
        }

        let name = dialog.name_input.read(cx).value().to_string();
        let address = dialog.address_input.read(cx).value().to_string();
        if name.trim().is_empty() {
            toast::error(cx, SharedString::from("服务器名称不能为空"));
            return;
        }
        if address.trim().is_empty() {
            toast::error(cx, SharedString::from("服务器地址不能为空"));
            return;
        }
        let port_text = dialog.port_input.read(cx).value().to_string();
        let port = match port_text.trim().parse::<u16>() {
            Ok(port) if port != 0 => port,
            Ok(_) => {
                toast::error(cx, SharedString::from("端口必须大于 0"));
                return;
            }
            Err(error) => {
                toast::error(cx, SharedString::from(format!("端口无效: {error}")));
                return;
            }
        };

        dialog.pending = true;
        let version = dialog.version.clone();
        let config = dialog.config.clone();
        let selected_gdk_user = dialog.selected_gdk_user.clone();
        let editing_key = dialog.editing_key.clone();
        cx.spawn(async move |handle, cx| {
            let result = if let Some(key) = editing_key.as_ref() {
                data::update_external_server(
                    &version,
                    &config,
                    selected_gdk_user.as_ref().map(SharedString::as_ref),
                    key.as_ref(),
                    &name,
                    &address,
                    port,
                )
                .await
            } else {
                data::add_external_server(
                    &version,
                    &config,
                    selected_gdk_user.as_ref().map(SharedString::as_ref),
                    &name,
                    &address,
                    port,
                )
                .await
            };
            let _ = handle.update(cx, |this, cx| {
                match result {
                    Ok(_) => {
                        let editing = this
                            .server_editor_dialog
                            .as_ref()
                            .is_some_and(|dialog| dialog.editing_key.is_some());
                        this.server_editor_dialog = None;
                        this.last_servers_signature = None;
                        cx.update_global(|state: &mut ManagePageState, _cx| {
                            state.servers_loaded = false;
                            state.servers_loading = false;
                            state.servers_error = None;
                            state.server_motd = Arc::new(HashMap::new());
                            state.server_motd_loading = false;
                            state.server_motd_request_id =
                                state.server_motd_request_id.wrapping_add(1);
                        });
                        toast::success(
                            cx,
                            SharedString::from(if editing {
                                "服务器已保存"
                            } else {
                                "服务器已添加"
                            }),
                        );
                    }
                    Err(error) => {
                        if let Some(dialog) = this.server_editor_dialog.as_mut() {
                            dialog.pending = false;
                        }
                        toast::error(cx, SharedString::from(error));
                    }
                }
                cx.notify();
            });
            Ok::<(), anyhow::Error>(())
        })
        .detach();
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

pub(super) fn build_filtered_server_indices(
    state: &ManagePageState,
    signature: &ServerListSignature,
) -> Vec<usize> {
    let query = signature.query.as_ref().to_ascii_lowercase();
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
        left.name
            .as_ref()
            .to_ascii_lowercase()
            .cmp(&right.name.as_ref().to_ascii_lowercase())
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
    cx: &mut Context<ManagePageView>,
) -> AnyElement {
    if state.gdk_users_loading && version.is_gdk() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "正在读取用户目录",
            "请稍候，BMCBL 正在扫描当前 GDK 实例的可用用户。",
        )
        .into_any_element();
    }

    if version.is_gdk() && state.selected_gdk_user.is_none() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "未找到可用用户目录",
            "当前 GDK 实例没有扫描到可读取服务器列表的用户目录。",
        )
        .into_any_element();
    }

    if state.servers_loading {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "正在加载服务器",
            "服务器列表正在后台读取。",
        )
        .into_any_element();
    }

    if let Some(error) = state.servers_error.clone() {
        return error_panel(colors, error);
    }

    if filtered_indices.is_empty() {
        return empty_state(
            colors,
            "images/manage/empty.svg",
            "没有服务器",
            "添加服务器后会显示在这里。",
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
        rows = rows.child(
            div()
                .w_full()
                .h(px(MANAGE_ASSET_ROW_PITCH_PX))
                .pb(px(MANAGE_ASSET_ROW_GAP_PX))
                .flex_none()
                .child(render_server_row(colors, entry, motd_status, cx)),
        );
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
                badges = badges.child(subtle_badge(colors, format!("{latency} ms")));
            }
            Some(if let Some(line_2) = motd.line_2.clone() {
                SharedString::from(format!("{} {}", motd.line_1, line_2))
            } else {
                motd.line_1.clone()
            })
        }
        Some(ManageServerMotdStatus::Loading) => {
            badges = badges.child(subtle_badge(colors, "查询中"));
            None
        }
        Some(ManageServerMotdStatus::Offline(error)) => {
            badges = badges.child(subtle_badge(colors, "离线"));
            error_text = Some(error.clone());
            None
        }
        None => {
            badges = badges.child(subtle_badge(colors, "未查询"));
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
                lucide_icons::icon_file_pen_line(),
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
                lucide_icons::icon_refresh_cw(),
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
                lucide_icons::icon_trash_2(),
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
        .rounded(px(10.))
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
                .rounded(px(8.))
                .bg(colors.surface_hover)
                .flex()
                .items_center()
                .justify_center()
                .child(
                    svg()
                        .path(lucide_icons::icon_server())
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

    let input_row = |label: &'static str, input: &Entity<InputState>| {
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
            .rounded(px(22.))
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
                                "编辑服务器"
                            } else {
                                "添加服务器"
                            }),
                    )
                    .child(input_row("名称", &dialog.name_input))
                    .child(input_row("地址", &dialog.address_input))
                    .child(input_row("端口", &dialog.port_input)),
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
                        ghost_button(colors, "manage-server-editor-cancel", "取消").on_mouse_down(
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
                                SharedString::from("保存中...")
                            } else if editing {
                                SharedString::from("保存服务器")
                            } else {
                                SharedString::from("添加服务器")
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
