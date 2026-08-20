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
    RefreshingPeers,
    Stopping,
}

impl OnlineOperation {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::CreatingRoom => "正在创建房间",
            Self::JoiningRoom => "正在加入房间",
            Self::RefreshingPeers => "正在刷新节点",
            Self::Stopping => "正在断开连接",
        }
    }

    pub(crate) fn is_busy(self) -> bool {
        self != Self::Idle
    }

    pub(crate) fn changes_room_content(self) -> bool {
        matches!(
            self,
            Self::CreatingRoom | Self::JoiningRoom | Self::Stopping
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnlineBlockingIssue {
    LocalWorldMissing,
    DiscoveryPortOccupied,
}

impl OnlineBlockingIssue {
    pub(crate) fn from_room_error(error: &str) -> Option<Self> {
        error
            .contains("未检测到本机 Minecraft 基岩版局域网世界")
            .then_some(Self::LocalWorldMissing)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MinecraftTerminationDialogState {
    pub open: bool,
    pub pending: bool,
    pub error: Option<SharedString>,
}

pub struct ToolsPageState {
    pub tab: ToolsTab,
    pub nat_checking: bool,
    pub nat_udp_type: Option<i32>,
    pub nat_tcp_type: Option<i32>,
    pub nat_error: Option<SharedString>,
    nat_check_generation: u64,
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
    pub online_operation: OnlineOperation,
    online_operation_generation: u64,
    pub online_blocking_issue: Option<OnlineBlockingIssue>,
    online_blocking_issue_dismissed: bool,
    pub discovery_retrying: bool,
    discovery_retry_generation: u64,
    pub minecraft_termination_dialog: MinecraftTerminationDialogState,
    status_refresh_generation: u64,
    status_refresh_in_progress: bool,
    pub online_error: Option<SharedString>,
    pub online_log: SharedString,
    pub abandoned_nodes: Vec<SharedString>,
    abandoned_nodes_dismissed: bool,
    pub easytier_running: bool,
    pub easytier_hostname: SharedString,
    pub easytier_ipv4: Option<SharedString>,
    pub easytier_game_host: SharedString,
    pub easytier_game_port: Option<u16>,
    pub active_room_code: SharedString,
    pub active_network_name: SharedString,
    pub host_room_code: SharedString,
    pub peers_loading: bool,
    pub network_nodes_expanded: bool,
    pub players: Vec<OnlinePlayerEntry>,
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
            nat_check_generation: 0,
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
            online_operation: OnlineOperation::Idle,
            online_operation_generation: 0,
            online_blocking_issue: None,
            online_blocking_issue_dismissed: false,
            discovery_retrying: false,
            discovery_retry_generation: 0,
            minecraft_termination_dialog: MinecraftTerminationDialogState::default(),
            status_refresh_generation: 0,
            status_refresh_in_progress: false,
            online_error: None,
            online_log: SharedString::from(""),
            abandoned_nodes: Vec::new(),
            abandoned_nodes_dismissed: false,
            easytier_running: false,
            easytier_hostname: SharedString::from(""),
            easytier_ipv4: None,
            easytier_game_host: SharedString::from(""),
            easytier_game_port: None,
            active_room_code: SharedString::from(""),
            active_network_name: SharedString::from(""),
            host_room_code: SharedString::from(""),
            peers_loading: false,
            network_nodes_expanded: false,
            players: Vec::new(),
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
    }

    pub fn host_or_avg_latency(&self) -> Option<u64> {
        if !self.easytier_running {
            return None;
        }
        if let Some(server_peer) = self.peers.iter().find(|p| p.role == OnlinePeerRole::Server) {
            if let Some(latency) = server_peer.latency_ms {
                return Some(latency);
            }
        }
        let latencies: Vec<u64> = self.peers.iter().filter_map(|p| p.latency_ms).collect();
        if latencies.is_empty() {
            None
        } else {
            let sum: u64 = latencies.iter().sum();
            Some(sum / latencies.len() as u64)
        }
    }

    pub(crate) fn begin_online_operation(&mut self, operation: OnlineOperation) -> Option<u64> {
        if self.online_operation.is_busy() {
            return None;
        }

        self.invalidate_status_refresh();
        self.online_operation_generation = self.online_operation_generation.wrapping_add(1);
        self.online_operation = operation;
        self.set_online_blocking_issue(None);
        self.online_error = None;
        if matches!(
            operation,
            OnlineOperation::CreatingRoom | OnlineOperation::JoiningRoom
        ) {
            self.clear_abandoned_nodes();
        }
        Some(self.online_operation_generation)
    }

    pub(crate) fn begin_stop_operation(&mut self) -> Option<u64> {
        if self.online_operation.is_busy() {
            if self.online_operation == OnlineOperation::Stopping {
                return None;
            }
            self.online_operation = OnlineOperation::Stopping;
            return Some(self.online_operation_generation);
        }
        self.begin_online_operation(OnlineOperation::Stopping)
    }

    pub(crate) fn is_current_online_operation(&self, generation: u64) -> bool {
        self.online_operation.is_busy() && self.online_operation_generation == generation
    }

    pub(crate) fn is_current_room_operation(&self, generation: u64) -> bool {
        self.is_current_online_operation(generation)
            && self.online_operation != OnlineOperation::Stopping
    }

    pub(crate) fn finish_room_operation(&mut self, generation: u64) -> bool {
        self.is_current_room_operation(generation) && {
            self.online_operation = OnlineOperation::Idle;
            true
        }
    }

    pub(crate) fn finish_online_operation(&mut self, generation: u64) -> bool {
        if !self.is_current_online_operation(generation) {
            return false;
        }

        self.online_operation = OnlineOperation::Idle;
        true
    }

    pub(crate) fn set_online_blocking_issue(&mut self, issue: Option<OnlineBlockingIssue>) {
        self.online_blocking_issue = issue;
        self.online_blocking_issue_dismissed = false;
    }

    pub(crate) fn is_online_blocking_issue_visible(&self) -> bool {
        self.online_blocking_issue.is_some() && !self.online_blocking_issue_dismissed
    }

    pub(crate) fn dismiss_online_blocking_issue(&mut self) -> bool {
        if !self.is_online_blocking_issue_visible() {
            return false;
        }

        self.online_blocking_issue_dismissed = true;
        true
    }

    pub(crate) fn add_abandoned_node(&mut self, node: SharedString) -> bool {
        if self
            .abandoned_nodes
            .iter()
            .any(|existing| existing == &node)
        {
            return false;
        }

        self.abandoned_nodes.push(node);
        self.abandoned_nodes_dismissed = false;
        true
    }

    pub(crate) fn are_abandoned_nodes_visible(&self) -> bool {
        !self.abandoned_nodes.is_empty() && !self.abandoned_nodes_dismissed
    }

    pub(crate) fn dismiss_abandoned_nodes(&mut self) -> bool {
        if !self.are_abandoned_nodes_visible() {
            return false;
        }

        self.abandoned_nodes_dismissed = true;
        true
    }

    fn clear_abandoned_nodes(&mut self) {
        self.abandoned_nodes.clear();
        self.abandoned_nodes_dismissed = false;
    }

    pub(crate) fn begin_discovery_retry(&mut self) -> Option<u64> {
        if !self.easytier_running
            || self.discovery_retrying
            || self.online_operation.is_busy()
            || self.online_blocking_issue != Some(OnlineBlockingIssue::DiscoveryPortOccupied)
        {
            return None;
        }

        self.discovery_retry_generation = self.discovery_retry_generation.wrapping_add(1);
        self.discovery_retrying = true;
        self.online_error = None;
        Some(self.discovery_retry_generation)
    }

    pub(crate) fn finish_discovery_retry(&mut self, generation: u64) -> bool {
        if !self.discovery_retrying || self.discovery_retry_generation != generation {
            return false;
        }

        self.discovery_retrying = false;
        true
    }

    pub(crate) fn open_minecraft_termination_dialog(&mut self) -> bool {
        if self.online_blocking_issue != Some(OnlineBlockingIssue::DiscoveryPortOccupied)
            || self.minecraft_termination_dialog.open
        {
            return false;
        }

        self.minecraft_termination_dialog.open = true;
        self.minecraft_termination_dialog.pending = false;
        self.minecraft_termination_dialog.error = None;
        true
    }

    pub(crate) fn dismiss_minecraft_termination_dialog(&mut self) -> bool {
        if !self.minecraft_termination_dialog.open || self.minecraft_termination_dialog.pending {
            return false;
        }

        self.minecraft_termination_dialog = MinecraftTerminationDialogState::default();
        true
    }

    pub(crate) fn begin_minecraft_termination(&mut self) -> bool {
        if !self.minecraft_termination_dialog.open || self.minecraft_termination_dialog.pending {
            return false;
        }

        self.minecraft_termination_dialog.pending = true;
        self.minecraft_termination_dialog.error = None;
        true
    }

    pub(crate) fn finish_minecraft_termination(
        &mut self,
        result: Result<(), SharedString>,
    ) -> bool {
        if !self.minecraft_termination_dialog.open || !self.minecraft_termination_dialog.pending {
            return false;
        }

        match result {
            Ok(()) => {
                self.minecraft_termination_dialog = MinecraftTerminationDialogState::default();
            }
            Err(error) => {
                self.minecraft_termination_dialog.pending = false;
                self.minecraft_termination_dialog.error = Some(error);
            }
        }
        true
    }

    pub(crate) fn begin_status_refresh(&mut self) -> Option<u64> {
        if !self.easytier_running
            || self.online_operation.is_busy()
            || self.status_refresh_in_progress
        {
            return None;
        }

        self.status_refresh_generation = self.status_refresh_generation.wrapping_add(1);
        self.status_refresh_in_progress = true;
        Some(self.status_refresh_generation)
    }

    pub(crate) fn finish_status_refresh(&mut self, generation: u64) -> bool {
        if !self.status_refresh_in_progress || self.status_refresh_generation != generation {
            return false;
        }

        self.status_refresh_in_progress = false;
        true
    }

    pub(crate) fn needs_nat_check(&self) -> bool {
        self.easytier_running
            && !self.nat_checking
            && (self.nat_udp_type.is_none() || self.nat_tcp_type.is_none())
    }

    pub(crate) fn begin_nat_check(&mut self) -> Option<u64> {
        if !self.needs_nat_check() {
            return None;
        }

        self.nat_check_generation = self.nat_check_generation.wrapping_add(1);
        self.nat_checking = true;
        self.nat_error = None;
        Some(self.nat_check_generation)
    }

    pub(crate) fn finish_nat_check(&mut self, generation: u64) -> bool {
        if !self.nat_checking || self.nat_check_generation != generation {
            return false;
        }

        self.nat_checking = false;
        true
    }

    pub(crate) fn clear_online_session(&mut self) {
        self.invalidate_status_refresh();
        self.nat_check_generation = self.nat_check_generation.wrapping_add(1);
        self.nat_checking = false;
        self.nat_udp_type = None;
        self.nat_tcp_type = None;
        self.nat_error = None;
        self.discovery_retry_generation = self.discovery_retry_generation.wrapping_add(1);
        self.discovery_retrying = false;
        self.set_online_blocking_issue(None);
        self.minecraft_termination_dialog = MinecraftTerminationDialogState::default();
        self.clear_abandoned_nodes();
        self.easytier_running = false;
        self.easytier_hostname = SharedString::from("");
        self.easytier_ipv4 = None;
        self.easytier_game_host = SharedString::from("");
        self.easytier_game_port = None;
        self.active_room_code = SharedString::from("");
        self.active_network_name = SharedString::from("");
        self.host_room_code = SharedString::from("");
        self.players.clear();
        self.peers.clear();
        self.peers_loading = false;
    }

    fn invalidate_status_refresh(&mut self) {
        self.status_refresh_generation = self.status_refresh_generation.wrapping_add(1);
        self.status_refresh_in_progress = false;
    }
}

impl Global for ToolsPageState {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OnlinePeerEntry {
    pub ipv4: Option<SharedString>,
    pub hostname: SharedString,
    pub role: OnlinePeerRole,
    pub connection_kind: crate::core::online::EasyTierConnectionKind,
    pub protocol: Option<SharedString>,
    pub remote_endpoint: Option<SharedString>,
    pub latency_ms: Option<u64>,
    pub via_hostname: Option<SharedString>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OnlinePlayerEntry {
    pub player_name: SharedString,
    pub client_id: SharedString,
    pub is_room_host: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OnlinePeerRole {
    User,
    Relay,
    Server,
    Unknown,
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

    #[test]
    fn stopping_operation_rejects_room_completion() {
        let mut state = ToolsPageState::default();
        let generation = state
            .begin_online_operation(OnlineOperation::CreatingRoom)
            .expect("idle state accepts an operation");

        assert_eq!(state.begin_stop_operation(), Some(generation));
        assert!(!state.is_current_room_operation(generation));
        assert!(!state.finish_room_operation(generation));
        assert!(state.finish_online_operation(generation));
        assert_eq!(state.online_operation, OnlineOperation::Idle);
    }

    #[test]
    fn status_refresh_does_not_mark_room_operation_busy() {
        let mut state = ToolsPageState {
            easytier_running: true,
            ..ToolsPageState::default()
        };

        let generation = state
            .begin_status_refresh()
            .expect("connected state accepts a status refresh");

        assert_eq!(state.online_operation, OnlineOperation::Idle);
        assert!(!state.peers_loading);
        assert!(state.finish_status_refresh(generation));
    }

    #[test]
    fn starting_new_room_clears_previous_abandoned_nodes() {
        let mut state = ToolsPageState {
            abandoned_nodes: vec![SharedString::from("tcp://unreachable.example:11010")],
            ..ToolsPageState::default()
        };

        state
            .begin_online_operation(OnlineOperation::JoiningRoom)
            .expect("idle state accepts a room operation");

        assert!(state.abandoned_nodes.is_empty());
    }

    #[test]
    fn room_operation_invalidates_stale_status_refresh() {
        let mut state = ToolsPageState {
            easytier_running: true,
            ..ToolsPageState::default()
        };
        let refresh_generation = state
            .begin_status_refresh()
            .expect("connected state accepts a status refresh");

        state
            .begin_online_operation(OnlineOperation::Stopping)
            .expect("stop operation should start");

        assert!(!state.finish_status_refresh(refresh_generation));
    }

    #[test]
    fn nat_check_is_needed_once_per_connected_session() {
        let mut state = ToolsPageState {
            easytier_running: true,
            ..ToolsPageState::default()
        };
        assert!(state.needs_nat_check());

        state.nat_udp_type = Some(0);
        state.nat_tcp_type = Some(0);
        assert!(!state.needs_nat_check());
    }

    #[test]
    fn clearing_session_invalidates_stale_nat_result() {
        let mut state = ToolsPageState {
            easytier_running: true,
            abandoned_nodes: vec![SharedString::from("tcp://unreachable.example:11010")],
            ..ToolsPageState::default()
        };
        let generation = state.begin_nat_check().expect("NAT check should start");

        state.clear_online_session();

        assert!(!state.finish_nat_check(generation));
        assert!(state.abandoned_nodes.is_empty());
    }

    #[test]
    fn local_world_error_maps_to_persistent_blocking_issue() {
        assert_eq!(
            OnlineBlockingIssue::from_room_error(
                "未检测到本机 Minecraft 基岩版局域网世界，请先在游戏中开启局域网联机"
            ),
            Some(OnlineBlockingIssue::LocalWorldMissing)
        );
        assert_eq!(
            OnlineBlockingIssue::from_room_error("EasyTier 网络启动失败"),
            None
        );
    }

    #[test]
    fn dismissed_blocking_issue_reappears_when_issue_is_reported_again() {
        let mut state = ToolsPageState::default();
        state.set_online_blocking_issue(Some(OnlineBlockingIssue::DiscoveryPortOccupied));

        assert!(state.is_online_blocking_issue_visible());
        assert!(state.dismiss_online_blocking_issue());
        assert!(!state.is_online_blocking_issue_visible());

        state.set_online_blocking_issue(Some(OnlineBlockingIssue::DiscoveryPortOccupied));

        assert!(state.is_online_blocking_issue_visible());
    }

    #[test]
    fn dismissed_abandoned_nodes_reappear_when_a_new_node_is_added() {
        let mut state = ToolsPageState::default();
        assert!(state.add_abandoned_node(SharedString::from("tcp://unreachable.example:11010")));

        assert!(state.are_abandoned_nodes_visible());
        assert!(state.dismiss_abandoned_nodes());
        assert!(!state.are_abandoned_nodes_visible());
        assert!(!state.add_abandoned_node(SharedString::from("tcp://unreachable.example:11010")));
        assert!(!state.are_abandoned_nodes_visible());

        assert!(state.add_abandoned_node(SharedString::from("tcp://another.example:11010")));
        assert!(state.are_abandoned_nodes_visible());
    }

    #[test]
    fn discovery_retry_only_runs_for_connected_port_conflict() {
        let mut state = ToolsPageState {
            easytier_running: true,
            online_blocking_issue: Some(OnlineBlockingIssue::DiscoveryPortOccupied),
            ..ToolsPageState::default()
        };

        let generation = state
            .begin_discovery_retry()
            .expect("port conflict should allow retry");
        assert!(state.begin_discovery_retry().is_none());
        assert!(state.finish_discovery_retry(generation));
        assert!(!state.discovery_retrying);
    }

    #[test]
    fn clearing_session_closes_port_conflict_dialog_and_invalidates_retry() {
        let mut state = ToolsPageState {
            easytier_running: true,
            online_blocking_issue: Some(OnlineBlockingIssue::DiscoveryPortOccupied),
            ..ToolsPageState::default()
        };
        let generation = state
            .begin_discovery_retry()
            .expect("port conflict should allow retry");
        assert!(state.open_minecraft_termination_dialog());

        state.clear_online_session();

        assert!(!state.finish_discovery_retry(generation));
        assert_eq!(state.online_blocking_issue, None);
        assert!(!state.minecraft_termination_dialog.open);
    }
}
