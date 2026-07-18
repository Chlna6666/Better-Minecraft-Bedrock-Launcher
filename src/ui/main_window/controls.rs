use super::*;

impl MainWindowView {
    pub(super) fn ensure_route_controls(
        &mut self,
        route: AppRoute,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match route {
            AppRoute::Home | AppRoute::Tasks => {}
            AppRoute::Download => {
                self.ensure_download_controls(window, cx);
                self.ensure_curseforge_controls(window, cx);
            }
            AppRoute::Manage => {
                self.ensure_manage_controls(window, cx);
            }
            AppRoute::Tools => {
                self.ensure_tools_controls(window, cx);
            }
            AppRoute::Settings => {
                self.ensure_settings_controls(window, cx);
            }
        }
    }

    pub(super) fn ensure_manage_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.manage_controls_initialized {
            return;
        }
        self.manage_controls_initialized = true;

        let input = cx.update_global(
            |s: &mut crate::ui::views::manage::state::ManagePageState, cx| {
                if s.search_input.is_none() {
                    let initial = s.search_query.to_string();
                    let input = cx.new(|cx| {
                        let mut st = InputState::new(window, cx);
                        st.set_placeholder(SharedString::from("搜索版本..."), window, cx);
                        if !initial.trim().is_empty() {
                            st.set_value(SharedString::from(initial), window, cx);
                        }
                        st
                    });
                    s.search_input = Some(input);
                }
                s.search_input.clone()
            },
        );

        if let Some(input) = input {
            let sub = cx.subscribe(
                &input,
                |this, input: Entity<InputState>, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        let query = input.read(cx).value();
                        cx.update_global(
                            |s: &mut crate::ui::views::manage::state::ManagePageState, _cx| {
                                s.search_query = query;
                            },
                        );
                        this.notify_manage_page(cx);
                    }
                },
            );
            self.manage_controls_subscriptions.push(sub);
        }
    }

    pub(super) fn ensure_tools_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tools_controls_initialized {
            return;
        }
        self.tools_controls_initialized = true;

        let (room_input, bootstrap_input, player_input, game_ports_input) = cx.update_global(
            |s: &mut crate::ui::views::tools::state::ToolsPageState, cx| {
                if s.room_code_input.is_none() {
                    let initial = s.room_code.to_string();
                    let input = cx.new(|cx| {
                        let mut st = InputState::new(window, cx);
                        st.set_placeholder(SharedString::from("P/NNNN-NNNN-SSSS-SSSS"), window, cx);
                        if !initial.trim().is_empty() {
                            st.set_value(SharedString::from(initial), window, cx);
                        }
                        st
                    });
                    s.room_code_input = Some(input);
                }

                if s.bootstrap_peers_input.is_none() {
                    let initial = s.bootstrap_peers.to_string();
                    let input = cx.new(|cx| {
                        let mut st = InputState::new(window, cx);
                        st.set_placeholder(
                            SharedString::from("留空自动获取公共节点，或手动输入 tcp://host:port"),
                            window,
                            cx,
                        );
                        if !initial.trim().is_empty() {
                            st.set_value(SharedString::from(initial), window, cx);
                        }
                        st
                    });
                    s.bootstrap_peers_input = Some(input);
                }

                if s.player_name_input.is_none() {
                    let initial = s.player_name.to_string();
                    let input = cx.new(|cx| {
                        let mut st = InputState::new(window, cx);
                        st.set_placeholder(SharedString::from("BMCBL_USER"), window, cx);
                        if !initial.trim().is_empty() {
                            st.set_value(SharedString::from(initial), window, cx);
                        }
                        st
                    });
                    s.player_name_input = Some(input);
                }

                if s.game_ports_input.is_none() {
                    let initial = s.game_ports.to_string();
                    let input = cx.new(|cx| {
                        let mut st = InputState::new(window, cx);
                        st.set_placeholder(SharedString::from("7551, 19132"), window, cx);
                        if !initial.trim().is_empty() {
                            st.set_value(SharedString::from(initial), window, cx);
                        }
                        st
                    });
                    s.game_ports_input = Some(input);
                }

                (
                    s.room_code_input.clone(),
                    s.bootstrap_peers_input.clone(),
                    s.player_name_input.clone(),
                    s.game_ports_input.clone(),
                )
            },
        );

        if let Some(input) = room_input {
            let sub = cx.subscribe(&input, |this, input, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let value = input.read(cx).value();
                    cx.update_global(
                        |s: &mut crate::ui::views::tools::state::ToolsPageState, _cx| {
                            s.room_code = value;
                        },
                    );
                    this.notify_tools_page(cx);
                }
            });
            self.tools_controls_subscriptions.push(sub);
        }

        if let Some(input) = bootstrap_input {
            let sub = cx.subscribe(&input, |this, input, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let value = input.read(cx).value();
                    cx.update_global(
                        |s: &mut crate::ui::views::tools::state::ToolsPageState, _cx| {
                            s.bootstrap_peers = value;
                        },
                    );
                    crate::ui::views::tools::online_controls::persist_tools_online_settings(cx);
                    this.notify_tools_page(cx);
                }
            });
            self.tools_controls_subscriptions.push(sub);
        }

        if let Some(input) = player_input {
            let sub = cx.subscribe(&input, |this, input, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let value = input.read(cx).value();
                    cx.update_global(
                        |s: &mut crate::ui::views::tools::state::ToolsPageState, _cx| {
                            s.player_name = value;
                        },
                    );
                    crate::ui::views::tools::online_controls::persist_tools_online_settings(cx);
                    this.notify_tools_page(cx);
                }
            });
            self.tools_controls_subscriptions.push(sub);
        }

        if let Some(input) = game_ports_input {
            let sub = cx.subscribe(&input, |this, input, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change) {
                    let value = input.read(cx).value();
                    cx.update_global(
                        |s: &mut crate::ui::views::tools::state::ToolsPageState, _cx| {
                            s.game_ports = value;
                        },
                    );
                    crate::ui::views::tools::online_controls::persist_tools_online_settings(cx);
                    this.notify_tools_page(cx);
                }
            });
            self.tools_controls_subscriptions.push(sub);
        }
    }

    pub(super) fn persist_settings_launcher_download_texts(
        &mut self,
        curseforge_api_base: String,
        http_proxy_url: String,
        socks_proxy_url: String,
        cx: &mut Context<Self>,
    ) {
        cx.spawn(async move |_this, _cx| {
            let res = tokio::task::spawn_blocking(move || {
                crate::config::config::update_config(|cfg| {
                    cfg.launcher.download.curseforge_api_base = curseforge_api_base;
                    cfg.launcher.download.proxy.http_proxy_url = http_proxy_url;
                    cfg.launcher.download.proxy.socks_proxy_url = socks_proxy_url;
                })?;
                Ok::<(), std::io::Error>(())
            })
            .await;

            if let Err(join_err) = res {
                tracing::warn!("persist launcher download text join error: {join_err}");
            } else if let Ok(Err(io_err)) = res {
                tracing::warn!("persist launcher download text failed: {io_err}");
            }
        })
        .detach();
    }

    pub(super) fn persist_settings_custom_style(
        &mut self,
        theme_color: String,
        background_option: String,
        local_image_path: String,
        network_image_url: String,
        background_blur: f32,
        show_launch_animation: bool,
        font_source: String,
        local_font_path: String,
        local_font_family: String,
        system_font_family: String,
        cx: &mut Context<Self>,
    ) {
        let normalized_theme_color = normalize_hex_color(&theme_color).unwrap_or(theme_color);
        let background_blur = crate::config::config::clamp_background_blur(background_blur);
        let font_source = crate::config::config::normalize_font_source(&font_source);
        cx.spawn(async move |_this, _cx| {
            let res = tokio::task::spawn_blocking(move || {
                crate::config::config::update_config(|cfg| {
                    cfg.custom_style.theme_color = normalized_theme_color;
                    cfg.custom_style.background_option = background_option;
                    cfg.custom_style.local_image_path = local_image_path;
                    cfg.custom_style.network_image_url = network_image_url;
                    cfg.custom_style.background_blur = background_blur;
                    cfg.custom_style.show_launch_animation = show_launch_animation;
                    cfg.custom_style.font_source = font_source;
                    cfg.custom_style.local_font_path = local_font_path;
                    cfg.custom_style.local_font_family = local_font_family;
                    cfg.custom_style.system_font_family = system_font_family;
                })?;
                Ok::<(), std::io::Error>(())
            })
            .await;

            if let Err(join_err) = res {
                tracing::warn!("persist custom style join error: {join_err}");
            } else if let Ok(Err(io_err)) = res {
                tracing::warn!("persist custom style failed: {io_err}");
            }
        })
        .detach();
    }

    pub(super) fn ensure_settings_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings_controls_initialized {
            return;
        }
        if !cx
            .global::<crate::ui::views::settings::state::SettingsPageState>()
            .loaded
        {
            return;
        }
        self.settings_controls_initialized = true;

        let (base_input, http_input, socks_input, theme_input, local_input, network_input) = cx
            .update_global(
                |s: &mut crate::ui::views::settings::state::SettingsPageState, cx| {
                    if s.download_curseforge_api_base_input.is_none() {
                        let initial = s.download_curseforge_api_base.to_string();
                        let input = cx.new(|cx| {
                            let mut st = InputState::new(window, cx);
                            st.set_placeholder(
                                SharedString::from("https://mod.mcimirror.top/curseforge"),
                                window,
                                cx,
                            );
                            if !initial.trim().is_empty() {
                                st.set_value(SharedString::from(initial), window, cx);
                            }
                            st
                        });
                        s.download_curseforge_api_base_input = Some(input);
                    }

                    if s.download_http_proxy_url_input.is_none() {
                        let initial = s.download_http_proxy_url.to_string();
                        let input = cx.new(|cx| {
                            let mut st = InputState::new(window, cx);
                            st.set_placeholder(
                                SharedString::from("http(s)://host:port"),
                                window,
                                cx,
                            );
                            if !initial.trim().is_empty() {
                                st.set_value(SharedString::from(initial), window, cx);
                            }
                            st
                        });
                        s.download_http_proxy_url_input = Some(input);
                    }

                    if s.download_socks_proxy_url_input.is_none() {
                        let initial = s.download_socks_proxy_url.to_string();
                        let input = cx.new(|cx| {
                            let mut st = InputState::new(window, cx);
                            st.set_placeholder(
                                SharedString::from("socks5://host:port"),
                                window,
                                cx,
                            );
                            if !initial.trim().is_empty() {
                                st.set_value(SharedString::from(initial), window, cx);
                            }
                            st
                        });
                        s.download_socks_proxy_url_input = Some(input);
                    }

                    if s.theme_color_input.is_none() {
                        let initial = s.theme_color.to_string();
                        let input = cx.new(|cx| {
                            let mut st = InputState::new(window, cx);
                            st.set_placeholder(
                                SharedString::from("#a0d9b6 / rgba(59,130,246,0.6)"),
                                window,
                                cx,
                            );
                            if !initial.trim().is_empty() {
                                st.set_value(SharedString::from(initial), window, cx);
                            }
                            st
                        });
                        s.theme_color_input = Some(input);
                    }

                    if s.local_image_path_input.is_none() {
                        let initial = s.local_image_path.to_string();
                        let input = cx.new(|cx| {
                            let mut st = InputState::new(window, cx);
                            st.set_placeholder(SharedString::from("C:/.../image.webp"), window, cx);
                            if !initial.trim().is_empty() {
                                st.set_value(SharedString::from(initial), window, cx);
                            }
                            st
                        });
                        s.local_image_path_input = Some(input);
                    }

                    if s.network_image_url_input.is_none() {
                        let initial = s.network_image_url.to_string();
                        let input = cx.new(|cx| {
                            let mut st = InputState::new(window, cx);
                            st.set_placeholder(
                                SharedString::from("https://host/bg.webp"),
                                window,
                                cx,
                            );
                            if !initial.trim().is_empty() {
                                st.set_value(SharedString::from(initial), window, cx);
                            }
                            st
                        });
                        s.network_image_url_input = Some(input);
                    }

                    (
                        s.download_curseforge_api_base_input.clone(),
                        s.download_http_proxy_url_input.clone(),
                        s.download_socks_proxy_url_input.clone(),
                        s.theme_color_input.clone(),
                        s.local_image_path_input.clone(),
                        s.network_image_url_input.clone(),
                    )
                },
            );

        if let Some(input) = base_input {
            let sub = cx.subscribe(
                &input,
                |this, input: Entity<InputState>, ev: &InputEvent, cx| match ev {
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        cx.update_global(
                            |s: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                                s.download_curseforge_api_base = value;
                            },
                        );
                        this.notify_settings_page(cx);
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        let (base, http, socks) = cx.read_global(
                            |s: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                                (
                                    s.download_curseforge_api_base.to_string(),
                                    s.download_http_proxy_url.to_string(),
                                    s.download_socks_proxy_url.to_string(),
                                )
                            },
                        );
                        this.persist_settings_launcher_download_texts(base, http, socks, cx);
                    }
                    _ => {}
                },
            );
            self.settings_controls_subscriptions.push(sub);
        }

        if let Some(input) = http_input {
            let sub = cx.subscribe(
                &input,
                |this, input: Entity<InputState>, ev: &InputEvent, cx| match ev {
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        cx.update_global(
                            |s: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                                s.download_http_proxy_url = value;
                            },
                        );
                        this.notify_settings_page(cx);
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        let (base, http, socks) = cx.read_global(
                            |s: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                                (
                                    s.download_curseforge_api_base.to_string(),
                                    s.download_http_proxy_url.to_string(),
                                    s.download_socks_proxy_url.to_string(),
                                )
                            },
                        );
                        this.persist_settings_launcher_download_texts(base, http, socks, cx);
                    }
                    _ => {}
                },
            );
            self.settings_controls_subscriptions.push(sub);
        }

        if let Some(input) = socks_input {
            let sub = cx.subscribe(
                &input,
                |this, input: Entity<InputState>, ev: &InputEvent, cx| match ev {
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        cx.update_global(
                            |s: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                                s.download_socks_proxy_url = value;
                            },
                        );
                        this.notify_settings_page(cx);
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        let (base, http, socks) = cx.read_global(
                            |s: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                                (
                                    s.download_curseforge_api_base.to_string(),
                                    s.download_http_proxy_url.to_string(),
                                    s.download_socks_proxy_url.to_string(),
                                )
                            },
                        );
                        this.persist_settings_launcher_download_texts(base, http, socks, cx);
                    }
                    _ => {}
                },
            );
            self.settings_controls_subscriptions.push(sub);
        }

        if let Some(input) = theme_input {
            let sub = cx.subscribe(
                &input,
                |this, input: Entity<InputState>, ev: &InputEvent, cx| match ev {
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        let normalized = normalize_hex_color(value.as_ref());
                        let stored = normalized.clone().unwrap_or_else(|| value.to_string());
                        cx.update_global(
                            |s: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                                s.theme_color = SharedString::from(stored);
                            },
                        );

                        if let Some(hex) = normalized {
                            ThemeState::set_accent_hex(&hex, cx);
                        }

                        this.notify_all_page_views(cx);
                        cx.notify();
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        let (
                            theme,
                            option,
                            local,
                            network,
                            blur,
                            anim,
                            font_source,
                            local_font_path,
                            local_font_family,
                            system_font_family,
                        ) = cx.read_global(
                            |s: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                                (
                                    s.theme_color.to_string(),
                                    s.background_option.to_string(),
                                    s.local_image_path.to_string(),
                                    s.network_image_url.to_string(),
                                    s.background_blur,
                                    s.show_launch_animation,
                                    s.font_source.to_string(),
                                    s.local_font_path.to_string(),
                                    s.local_font_family.to_string(),
                                    s.system_font_family.to_string(),
                                )
                            },
                        );
                        this.persist_settings_custom_style(
                            theme,
                            option,
                            local,
                            network,
                            blur,
                            anim,
                            font_source,
                            local_font_path,
                            local_font_family,
                            system_font_family,
                            cx,
                        );
                    }
                    _ => {}
                },
            );
            self.settings_controls_subscriptions.push(sub);
        }

        if let Some(input) = local_input {
            let sub = cx.subscribe(
                &input,
                |this, input: Entity<InputState>, ev: &InputEvent, cx| match ev {
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        cx.update_global(
                            |s: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                                s.local_image_path = value;
                            },
                        );
                        this.notify_settings_page(cx);
                        cx.notify();
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        let (
                            theme,
                            option,
                            local,
                            network,
                            blur,
                            anim,
                            font_source,
                            local_font_path,
                            local_font_family,
                            system_font_family,
                        ) = cx.read_global(
                            |s: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                                (
                                    s.theme_color.to_string(),
                                    s.background_option.to_string(),
                                    s.local_image_path.to_string(),
                                    s.network_image_url.to_string(),
                                    s.background_blur,
                                    s.show_launch_animation,
                                    s.font_source.to_string(),
                                    s.local_font_path.to_string(),
                                    s.local_font_family.to_string(),
                                    s.system_font_family.to_string(),
                                )
                            },
                        );
                        this.persist_settings_custom_style(
                            theme,
                            option,
                            local,
                            network,
                            blur,
                            anim,
                            font_source,
                            local_font_path,
                            local_font_family,
                            system_font_family,
                            cx,
                        );
                    }
                    _ => {}
                },
            );
            self.settings_controls_subscriptions.push(sub);
        }

        if let Some(input) = network_input {
            let sub = cx.subscribe(
                &input,
                |this, input: Entity<InputState>, ev: &InputEvent, cx| match ev {
                    InputEvent::Change => {
                        let value = input.read(cx).value();
                        cx.update_global(
                            |s: &mut crate::ui::views::settings::state::SettingsPageState, _cx| {
                                s.network_image_url = value;
                                if s.network_image_refreshing {
                                    s.network_image_refreshing = false;
                                    s.network_image_refresh_started_at = None;
                                    s.network_image_refresh_target_url = SharedString::from("");
                                }
                            },
                        );
                        this.notify_settings_page(cx);
                        cx.notify();
                    }
                    InputEvent::Blur | InputEvent::PressEnter { .. } => {
                        let (
                            theme,
                            option,
                            local,
                            network,
                            blur,
                            anim,
                            font_source,
                            local_font_path,
                            local_font_family,
                            system_font_family,
                        ) = cx.read_global(
                            |s: &crate::ui::views::settings::state::SettingsPageState, _cx| {
                                (
                                    s.theme_color.to_string(),
                                    s.background_option.to_string(),
                                    s.local_image_path.to_string(),
                                    s.network_image_url.to_string(),
                                    s.background_blur,
                                    s.show_launch_animation,
                                    s.font_source.to_string(),
                                    s.local_font_path.to_string(),
                                    s.local_font_family.to_string(),
                                    s.system_font_family.to_string(),
                                )
                            },
                        );
                        this.persist_settings_custom_style(
                            theme,
                            option,
                            local,
                            network,
                            blur,
                            anim,
                            font_source,
                            local_font_path,
                            local_font_family,
                            system_font_family,
                            cx,
                        );
                    }
                    _ => {}
                },
            );
            self.settings_controls_subscriptions.push(sub);
        }
    }
}
