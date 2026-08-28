use crate::ui::state::i18n::I18n;
use crate::ui::state::theme::ThemeState;
use crate::ui::theme::colors::{DarkColors, LightColors, ThemeColors, lerp_theme_colors};
use crate::ui::views::settings::state::SettingsPageState;
use crate::ui::views::tools::state::{
    OnlineOperation, OnlinePeerEntry, OnlinePlayerEntry, ToolsPageState, ToolsTab,
};
use gpui::*;
use std::time::Duration;

use crate::ui::views::tools::online::actions;

pub(crate) mod online;
mod sidebar;
pub mod state;

#[derive(PartialEq)]
struct ToolsRenderSignature {
    tab: ToolsTab,
    nat_checking: bool,
    nat_udp_type: Option<i32>,
    nat_tcp_type: Option<i32>,
    nat_error: Option<SharedString>,
    has_room_code_input: bool,
    room_code: SharedString,
    has_bootstrap_peers_input: bool,
    bootstrap_peers: SharedString,
    has_player_name_input: bool,
    player_name: SharedString,
    has_game_ports_input: bool,
    game_ports: SharedString,
    room_advanced_open: bool,
    easytier_settings_open: bool,
    disable_p2p: bool,
    online_operation: OnlineOperation,
    online_blocking_issue: Option<state::OnlineBlockingIssue>,
    online_blocking_issue_visible: bool,
    discovery_retrying: bool,
    online_error: Option<SharedString>,
    online_log: SharedString,
    abandoned_nodes: Vec<SharedString>,
    abandoned_nodes_visible: bool,
    easytier_running: bool,
    easytier_hostname: SharedString,
    easytier_ipv4: Option<SharedString>,
    easytier_game_host: SharedString,
    easytier_game_port: Option<u16>,
    active_room_code: SharedString,
    active_network_name: SharedString,
    host_room_code: SharedString,
    peers_loading: bool,
    network_nodes_expanded: bool,
    players: Vec<OnlinePlayerEntry>,
    peers: Vec<OnlinePeerEntry>,
}

impl ToolsRenderSignature {
    fn from_state(state: &ToolsPageState) -> Self {
        Self {
            tab: state.tab,
            nat_checking: state.nat_checking,
            nat_udp_type: state.nat_udp_type,
            nat_tcp_type: state.nat_tcp_type,
            nat_error: state.nat_error.clone(),
            has_room_code_input: state.room_code_input.is_some(),
            room_code: state.room_code.clone(),
            has_bootstrap_peers_input: state.bootstrap_peers_input.is_some(),
            bootstrap_peers: state.bootstrap_peers.clone(),
            has_player_name_input: state.player_name_input.is_some(),
            player_name: state.player_name.clone(),
            has_game_ports_input: state.game_ports_input.is_some(),
            game_ports: state.game_ports.clone(),
            room_advanced_open: state.room_advanced_open,
            easytier_settings_open: state.easytier_settings_open,
            disable_p2p: state.disable_p2p,
            online_operation: state.online_operation,
            online_blocking_issue: state.online_blocking_issue,
            online_blocking_issue_visible: state.is_online_blocking_issue_visible(),
            discovery_retrying: state.discovery_retrying,
            online_error: state.online_error.clone(),
            online_log: state.online_log.clone(),
            abandoned_nodes: state.abandoned_nodes.clone(),
            abandoned_nodes_visible: state.are_abandoned_nodes_visible(),
            easytier_running: state.easytier_running,
            easytier_hostname: state.easytier_hostname.clone(),
            easytier_ipv4: state.easytier_ipv4.clone(),
            easytier_game_host: state.easytier_game_host.clone(),
            easytier_game_port: state.easytier_game_port,
            active_room_code: state.active_room_code.clone(),
            active_network_name: state.active_network_name.clone(),
            host_room_code: state.host_room_code.clone(),
            peers_loading: state.peers_loading,
            network_nodes_expanded: state.network_nodes_expanded,
            players: state.players.clone(),
            peers: state.peers.clone(),
        }
    }
}

pub struct ToolsPageView {
    _subscriptions: Vec<Subscription>,
    _online_refresh_task: Task<()>,
    last_render_signature: Option<ToolsRenderSignature>,
}

impl ToolsPageView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            cx.observe_global::<ToolsPageState>(|this: &mut Self, cx| {
                let signature = ToolsRenderSignature::from_state(cx.global::<ToolsPageState>());
                if this.last_render_signature.as_ref() != Some(&signature) {
                    this.last_render_signature = Some(signature);
                    cx.notify();
                }
            }),
            cx.observe_global::<ThemeState>(|_, cx| {
                cx.notify();
            }),
            cx.observe_global::<SettingsPageState>(|_, cx| {
                cx.notify();
            }),
        ];
        let online_refresh_task = cx.spawn(async move |_this, cx| {
            loop {
                Timer::after(Duration::from_secs(3)).await;
                if let Err(error) = cx.update(|cx| {
                    actions::refresh_status(cx);
                    actions::check_nat(cx);
                }) {
                    tracing::warn!("online refresh task update failed: {error:?}");
                }
            }
        });
        Self {
            _subscriptions: subscriptions,
            _online_refresh_task: online_refresh_task,
            last_render_signature: Some(ToolsRenderSignature::from_state(
                cx.global::<ToolsPageState>(),
            )),
        }
    }
}

impl Render for ToolsPageView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = std::time::Instant::now();
        let theme = cx.global::<ThemeState>();
        let colors = lerp_theme_colors(
            &LightColors::colors(),
            &DarkColors::colors(),
            theme.factor(now),
            theme.accent,
        );
        let window_size = window.bounds().size;
        render_tools_page(
            colors,
            window_size.width,
            cx.global::<ToolsPageState>(),
            cx.global::<I18n>(),
        )
    }
}

pub fn render_tools_page(
    colors: ThemeColors,
    window_width: Pixels,
    state: &ToolsPageState,
    i18n: &I18n,
) -> impl IntoElement {
    let sidebar = sidebar::render_sidebar(&colors, state.tab);
    let content: AnyElement = match state.tab {
        ToolsTab::Online => {
            online::render_online_panel(&colors, i18n, state, window_width).into_any_element()
        }
    };

    crate::ui::components::page_shell::page_frame(crate::ui::components::page_shell::split_page(
        sidebar, content,
    ))
}

pub fn render_tools_overlay(
    colors: &ThemeColors,
    i18n: &I18n,
    window_width: Pixels,
    window_height: Pixels,
    state: &ToolsPageState,
) -> Option<AnyElement> {
    match state.tab {
        ToolsTab::Online => {
            online::render_online_overlay(colors, i18n, window_width, window_height, state)
        }
    }
}
