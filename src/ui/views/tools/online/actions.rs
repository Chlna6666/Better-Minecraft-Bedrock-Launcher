use crate::core::online::{EasyTierPeer, EasyTierStartOptions, PaperConnectRoom};
use crate::ui::components::toast;
use crate::ui::views::tools::state::{OnlineOperation, OnlinePeerEntry, ToolsPageState};
use gpui::*;
use tracing::warn;

use super::{append_online_log, normalized_player_name, parse_bootstrap_peers, primary_game_port};

#[derive(Clone, Copy)]
enum RoomIntent {
    Create,
    Join,
}

impl RoomIntent {
    fn operation(self) -> OnlineOperation {
        match self {
            Self::Create => OnlineOperation::CreatingRoom,
            Self::Join => OnlineOperation::JoiningRoom,
        }
    }

    fn action_label(self) -> &'static str {
        match self {
            Self::Create => "创建房间",
            Self::Join => "加入房间",
        }
    }

    fn hostname(self, game_port: u16, player_name: &str) -> String {
        match self {
            Self::Create => format!("paper-connect-server-{game_port}"),
            Self::Join => format!("bmcbl-client-{player_name}"),
        }
    }
}

struct RoomRequest {
    generation: u64,
    intent: RoomIntent,
    room_code: String,
    peers: Vec<String>,
    disable_p2p: bool,
    no_tun: bool,
    player_name: String,
    game_port: u16,
}

pub(super) fn create_room(cx: &mut App) {
    start_room(RoomIntent::Create, cx);
}

pub(super) fn join_room(cx: &mut App) {
    start_room(RoomIntent::Join, cx);
}

fn start_room(intent: RoomIntent, cx: &mut App) {
    let Some(request) = prepare_room_request(intent, cx) else {
        return;
    };
    let action = intent.action_label();
    append_online_log(format!("{action}：正在建立 EasyTier 网络"), cx);

    cx.spawn(async move |cx| establish_room(request, action, cx).await)
        .detach();
}

fn prepare_room_request(intent: RoomIntent, cx: &mut App) -> Option<RoomRequest> {
    let running = cx.read_global(|state: &ToolsPageState, _cx| state.easytier_running);
    if running {
        toast::error(cx, SharedString::from("请先断开当前房间，再开始新的联机"));
        return None;
    }

    let room_code = cx.read_global(|state: &ToolsPageState, _cx| state.room_code.to_string());
    if matches!(intent, RoomIntent::Join) && room_code.trim().is_empty() {
        toast::error(cx, SharedString::from("请输入或粘贴联机码"));
        return None;
    }

    let generation = cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.begin_online_operation(intent.operation())
    });
    let Some(generation) = generation else {
        toast::error(cx, SharedString::from("已有联机操作正在进行，请稍候"));
        return None;
    };

    Some(cx.read_global(|state: &ToolsPageState, _cx| RoomRequest {
        generation,
        intent,
        room_code,
        peers: parse_bootstrap_peers(state.bootstrap_peers.as_ref()),
        disable_p2p: state.disable_p2p,
        no_tun: state.no_tun,
        player_name: normalized_player_name(state),
        game_port: primary_game_port(state),
    }))
}

async fn establish_room(request: RoomRequest, action: &'static str, cx: &mut AsyncApp) {
    let RoomRequest {
        generation,
        intent,
        room_code,
        peers,
        disable_p2p,
        no_tun,
        player_name,
        game_port,
    } = request;
    let room = match resolve_room(intent, room_code).await {
        Ok(room) => room,
        Err(error) => {
            apply_room_error(generation, action, error, cx);
            return;
        }
    };

    let options = EasyTierStartOptions {
        disable_p2p: Some(disable_p2p),
        no_tun: Some(no_tun),
        compression: Some("zstd".to_string()),
        ipv4: None,
    };
    let hostname = intent.hostname(game_port, &player_name);
    if let Err(error) = crate::core::online::easytier_start(
        room.network_name.clone(),
        room.network_secret.clone(),
        peers,
        Some(hostname),
        Some(options),
    )
    .await
    {
        apply_room_error(generation, action, error, cx);
        return;
    }

    let status = crate::core::online::easytier_embedded_status()
        .await
        .ok()
        .flatten();
    let peers = crate::core::online::easytier_embedded_peers()
        .await
        .map(peer_entries)
        .unwrap_or_default();
    apply_room_success(generation, intent, room, status, peers, cx);
}

async fn resolve_room(intent: RoomIntent, room_code: String) -> Result<PaperConnectRoom, String> {
    match intent {
        RoomIntent::Create => crate::core::online::paperconnect_generate_room().await,
        RoomIntent::Join => crate::core::online::paperconnect_parse_room_code(room_code).await,
    }
}

fn apply_room_error(generation: u64, action: &'static str, error: String, cx: &mut AsyncApp) {
    let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
        if !state.finish_online_operation(generation) {
            return false;
        }
        state.online_error = Some(SharedString::from(error.clone()));
        state.peers_loading = false;
        true
    });
    match applied {
        Ok(true) => {
            if let Err(update_error) = cx.update(|cx| {
                append_online_log(format!("{action}失败：{error}"), cx);
                toast::error(
                    cx,
                    SharedString::from(format!("{action}失败，请检查联机设置")),
                );
            }) {
                warn!("failed to report online room error: {update_error:?}");
            }
        }
        Ok(false) => {}
        Err(update_error) => warn!("failed to apply online room error: {update_error:?}"),
    }
}

fn apply_room_success(
    generation: u64,
    intent: RoomIntent,
    room: PaperConnectRoom,
    status: Option<crate::core::online::EasyTierEmbeddedStatus>,
    peers: Vec<OnlinePeerEntry>,
    cx: &mut AsyncApp,
) {
    let room_code = room.room_code.clone();
    let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
        if !state.finish_online_operation(generation) {
            return false;
        }
        state.online_error = None;
        state.easytier_running = true;
        state.active_room_code = SharedString::from(room.room_code);
        state.active_network_name = SharedString::from(room.network_name);
        state.host_room_code = if matches!(intent, RoomIntent::Create) {
            state.active_room_code.clone()
        } else {
            SharedString::from("")
        };
        if let Some(status) = status {
            state.easytier_hostname = SharedString::from(status.hostname);
            state.easytier_ipv4 = status.ipv4.map(SharedString::from);
        }
        state.peers = peers;
        state.peers_loading = false;
        true
    });
    match applied {
        Ok(true) => {
            if let Err(update_error) = cx.update(|cx| {
                if matches!(intent, RoomIntent::Create) {
                    cx.write_to_clipboard(ClipboardItem::new_string(room_code.clone()));
                    toast::push(cx, SharedString::from("房间已创建，联机码已复制"));
                } else {
                    toast::push(cx, SharedString::from("已加入房间"));
                }
                append_online_log(format!("联机成功：{room_code}"), cx);
            }) {
                warn!("failed to report online room success: {update_error:?}");
            }
        }
        Ok(false) => {}
        Err(update_error) => warn!("failed to apply online room success: {update_error:?}"),
    }
}

pub(super) fn stop_session(cx: &mut App) {
    let generation = cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.begin_online_operation(OnlineOperation::Stopping)
    });
    let Some(generation) = generation else {
        return;
    };
    append_online_log("正在断开 EasyTier", cx);

    cx.spawn(async move |cx| {
        let result = crate::core::online::easytier_stop().await;
        let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
            if !state.finish_online_operation(generation) {
                return false;
            }
            match &result {
                Ok(()) => {
                    state.clear_online_session();
                    state.online_error = None;
                }
                Err(error) => state.online_error = Some(SharedString::from(error.clone())),
            }
            true
        });
        match applied {
            Ok(true) => {
                if let Err(update_error) = cx.update(|cx| match result {
                    Ok(()) => {
                        append_online_log("已断开联机", cx);
                        toast::push(cx, SharedString::from("已断开联机"));
                    }
                    Err(error) => {
                        append_online_log(format!("断开失败：{error}"), cx);
                        toast::error(cx, SharedString::from("断开失败，当前连接状态已保留"));
                    }
                }) {
                    warn!("failed to report online stop result: {update_error:?}");
                }
            }
            Ok(false) => {}
            Err(update_error) => warn!("failed to apply online stop result: {update_error:?}"),
        }
    })
    .detach();
}

pub(super) fn refresh_status(cx: &mut App) {
    let generation = cx.update_global(|state: &mut ToolsPageState, _cx| {
        let generation = state.begin_online_operation(OnlineOperation::Refreshing)?;
        state.peers_loading = true;
        Some(generation)
    });
    let Some(generation) = generation else {
        return;
    };

    cx.spawn(async move |cx| {
        let status_result = crate::core::online::easytier_embedded_status().await;
        let peers_result = crate::core::online::easytier_embedded_peers().await;
        let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
            if !state.finish_online_operation(generation) {
                return false;
            }
            state.peers_loading = false;
            match status_result {
                Ok(Some(status)) => {
                    state.easytier_running = true;
                    state.easytier_hostname = SharedString::from(status.hostname);
                    state.easytier_ipv4 = status.ipv4.map(SharedString::from);
                    state.online_error = None;
                }
                Ok(None) => {
                    state.clear_online_session();
                    state.online_error = None;
                }
                Err(error) => state.online_error = Some(SharedString::from(error)),
            }
            if let Ok(peers) = peers_result {
                state.peers = peer_entries(peers);
            }
            true
        });
        if let Err(update_error) = applied {
            warn!("failed to refresh online status: {update_error:?}");
        }
    })
    .detach();
}

pub(super) fn refresh_peers(cx: &mut App) {
    let generation = cx.update_global(|state: &mut ToolsPageState, _cx| {
        let generation = state.begin_online_operation(OnlineOperation::RefreshingPeers)?;
        state.peers_loading = true;
        Some(generation)
    });
    let Some(generation) = generation else {
        return;
    };

    cx.spawn(async move |cx| {
        let result = crate::core::online::easytier_embedded_peers().await;
        let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
            if !state.finish_online_operation(generation) {
                return false;
            }
            state.peers_loading = false;
            match result {
                Ok(peers) => {
                    state.peers = peer_entries(peers);
                    state.online_error = None;
                }
                Err(error) => state.online_error = Some(SharedString::from(error)),
            }
            true
        });
        if let Err(update_error) = applied {
            warn!("failed to refresh online peers: {update_error:?}");
        }
    })
    .detach();
}

pub(super) fn check_nat(cx: &mut App) {
    let started = cx.update_global(|state: &mut ToolsPageState, _cx| {
        if state.nat_checking {
            return false;
        }
        state.nat_checking = true;
        state.nat_error = None;
        true
    });
    if !started {
        return;
    }

    cx.spawn(async move |cx| {
        let snapshot = crate::core::easytier::api::detect_nat_types().await;
        if let Err(update_error) = cx.update_global(|state: &mut ToolsPageState, _cx| {
            state.nat_checking = false;
            state.nat_udp_type = Some(snapshot.udp_nat_type);
            state.nat_tcp_type = Some(snapshot.tcp_nat_type);
        }) {
            warn!("failed to apply NAT result: {update_error:?}");
        }
    })
    .detach();
}

fn peer_entries(peers: Vec<EasyTierPeer>) -> Vec<OnlinePeerEntry> {
    peers
        .into_iter()
        .map(|peer| OnlinePeerEntry {
            hostname: SharedString::from(peer.hostname),
            ipv4: peer.ipv4.map(SharedString::from),
        })
        .collect()
}
