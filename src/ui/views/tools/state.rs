use gpui::{Entity, Global, SharedString};

use crate::ui::components::input::InputState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolsTab {
    Online,
}

pub struct ToolsPageState {
    pub tab: ToolsTab,
    pub nat_checking: bool,
    pub nat_udp_type: Option<i32>,
    pub nat_tcp_type: Option<i32>,
    pub nat_error: Option<SharedString>,
    pub room_code_input: Option<Entity<InputState>>,
    pub room_code: SharedString,
    pub bootstrap_peers_input: Option<Entity<InputState>>,
    pub bootstrap_peers: SharedString,
    pub player_name_input: Option<Entity<InputState>>,
    pub player_name: SharedString,
    pub game_ports_input: Option<Entity<InputState>>,
    pub game_ports: SharedString,
    pub easytier_settings_open: bool,
    pub disable_p2p: bool,
    pub no_tun: bool,
    pub online_loading: bool,
    pub online_error: Option<SharedString>,
    pub online_log: SharedString,
    pub easytier_running: bool,
    pub easytier_hostname: SharedString,
    pub easytier_ipv4: Option<SharedString>,
    pub active_room_code: SharedString,
    pub active_network_name: SharedString,
    pub host_room_code: SharedString,
    pub peers_loading: bool,
    pub peers: Vec<OnlinePeerEntry>,
}

impl Default for ToolsPageState {
    fn default() -> Self {
        Self {
            tab: ToolsTab::Online,
            nat_checking: false,
            nat_udp_type: None,
            nat_tcp_type: None,
            nat_error: None,
            room_code_input: None,
            room_code: SharedString::from(""),
            bootstrap_peers_input: None,
            bootstrap_peers: SharedString::from(""),
            player_name_input: None,
            player_name: SharedString::from(crate::config::config::default_online_player_name()),
            game_ports_input: None,
            game_ports: SharedString::from("7551"),
            easytier_settings_open: false,
            disable_p2p: true,
            no_tun: true,
            online_loading: false,
            online_error: None,
            online_log: SharedString::from(""),
            easytier_running: false,
            easytier_hostname: SharedString::from(""),
            easytier_ipv4: None,
            active_room_code: SharedString::from(""),
            active_network_name: SharedString::from(""),
            host_room_code: SharedString::from(""),
            peers_loading: false,
            peers: Vec::new(),
        }
    }
}

impl ToolsPageState {
    pub(crate) fn apply_config(&mut self, config: &crate::config::config::OnlineConfig) {
        self.bootstrap_peers = SharedString::from(config.bootstrap_peers.clone());
        self.player_name = SharedString::from(config.player_name.clone());
        self.game_ports = SharedString::from(config.game_ports.clone());
        self.disable_p2p = config.disable_p2p;
        self.no_tun = config.no_tun;
    }
}

impl Global for ToolsPageState {}

#[derive(Clone, Debug)]
pub struct OnlinePeerEntry {
    pub ipv4: Option<SharedString>,
    pub hostname: SharedString,
}
