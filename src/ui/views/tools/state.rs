use gpui::{Entity, Global, SharedString};

use crate::ui::components::input::InputState;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolsTab {
    Online,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OnlineOperation {
    #[default]
    Idle,
    CreatingRoom,
    JoiningRoom,
    Refreshing,
    RefreshingPeers,
    Stopping,
}

impl OnlineOperation {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::CreatingRoom => "正在创建房间",
            Self::JoiningRoom => "正在加入房间",
            Self::Refreshing => "正在刷新状态",
            Self::RefreshingPeers => "正在刷新节点",
            Self::Stopping => "正在断开连接",
        }
    }

    pub(crate) fn is_busy(self) -> bool {
        self != Self::Idle
    }
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
    pub room_advanced_open: bool,
    pub easytier_settings_open: bool,
    pub disable_p2p: bool,
    pub no_tun: bool,
    pub online_operation: OnlineOperation,
    online_operation_generation: u64,
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
            room_advanced_open: false,
            easytier_settings_open: false,
            disable_p2p: false,
            no_tun: true,
            online_operation: OnlineOperation::Idle,
            online_operation_generation: 0,
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

    pub(crate) fn begin_online_operation(&mut self, operation: OnlineOperation) -> Option<u64> {
        if self.online_operation.is_busy() {
            return None;
        }

        self.online_operation_generation = self.online_operation_generation.wrapping_add(1);
        self.online_operation = operation;
        self.online_error = None;
        Some(self.online_operation_generation)
    }

    pub(crate) fn is_current_online_operation(&self, generation: u64) -> bool {
        self.online_operation.is_busy() && self.online_operation_generation == generation
    }

    pub(crate) fn finish_online_operation(&mut self, generation: u64) -> bool {
        if !self.is_current_online_operation(generation) {
            return false;
        }

        self.online_operation = OnlineOperation::Idle;
        true
    }

    pub(crate) fn clear_online_session(&mut self) {
        self.easytier_running = false;
        self.easytier_hostname = SharedString::from("");
        self.easytier_ipv4 = None;
        self.active_room_code = SharedString::from("");
        self.active_network_name = SharedString::from("");
        self.host_room_code = SharedString::from("");
        self.peers.clear();
        self.peers_loading = false;
    }
}

impl Global for ToolsPageState {}

#[derive(Clone, Debug)]
pub struct OnlinePeerEntry {
    pub ipv4: Option<SharedString>,
    pub hostname: SharedString,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn online_operation_rejects_overlap_and_stale_completion() {
        let mut state = ToolsPageState::default();
        let generation = state
            .begin_online_operation(OnlineOperation::CreatingRoom)
            .expect("idle state accepts an operation");

        assert!(
            state
                .begin_online_operation(OnlineOperation::JoiningRoom)
                .is_none()
        );
        assert!(!state.finish_online_operation(generation.wrapping_add(1)));
        assert_eq!(state.online_operation, OnlineOperation::CreatingRoom);
        assert!(state.finish_online_operation(generation));
        assert_eq!(state.online_operation, OnlineOperation::Idle);
    }
}
