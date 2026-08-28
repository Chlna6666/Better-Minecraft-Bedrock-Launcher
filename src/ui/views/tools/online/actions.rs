use std::future::Future;

use crate::core::online::{
    EasyTierPeer, EasyTierStartOptions, EasyTierStartRequest, PaperConnectPlayer, PaperConnectRoom,
};
use crate::ui::components::toast;
use crate::ui::state::i18n::I18n;
use crate::ui::views::tools::state::{
    OnlineBlockingIssue, OnlineOperation, OnlinePeerEntry, OnlinePeerRole, OnlinePlayerEntry,
    ToolsPageState,
};
use gpui::*;
use tracing::{info, warn};

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

    fn hostname(self, server_port: Option<u16>, player_name: &str) -> Option<String> {
        match self {
            Self::Create => server_port.map(|port| format!("paper-connect-server-{port}")),
            Self::Join => Some(format!("bmcbl-client-{player_name}")),
        }
    }
}

struct RoomRequest {
    generation: u64,
    intent: RoomIntent,
    room_code: String,
    server_port: Option<u16>,
    peers: Vec<String>,
    disable_p2p: bool,
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
        toast::error(cx, t!("Online.room_already_running"));
        return None;
    }

    let room_code = cx.read_global(|state: &ToolsPageState, _cx| state.room_code.to_string());
    if matches!(intent, RoomIntent::Join) && room_code.trim().is_empty() {
        toast::error(cx, t!("Online.err_need_room_code"));
        return None;
    }

    let generation = cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.begin_online_operation(intent.operation())
    });
    let Some(generation) = generation else {
        toast::error(cx, t!("Online.operation_in_progress"));
        return None;
    };

    let server_port = if matches!(intent, RoomIntent::Create) {
        match crate::core::online::paperconnect_pick_listen_port() {
            Ok(port) => Some(port),
            Err(error) => {
                cx.update_global(|state: &mut ToolsPageState, _cx| {
                    state.finish_online_operation(generation);
                    state.online_error = Some(SharedString::from(error.clone()));
                });
                toast::error(cx, t!("Online.center_port_unavailable"));
                return None;
            }
        }
    } else {
        None
    };

    Some(cx.read_global(|state: &ToolsPageState, _cx| RoomRequest {
        generation,
        intent,
        room_code,
        server_port,
        peers: parse_bootstrap_peers(state.bootstrap_peers.as_ref()),
        disable_p2p: state.disable_p2p,
        player_name: normalized_player_name(state),
        game_port: primary_game_port(state),
    }))
}

async fn establish_room(request: RoomRequest, action: &'static str, cx: &mut AsyncApp) {
    let RoomRequest {
        generation,
        intent,
        room_code,
        server_port,
        peers,
        disable_p2p,
        player_name,
        game_port,
    } = request;
    let room = match run_online(cx, async move { resolve_room(intent, room_code).await }).await {
        Ok(room) => room,
        Err(error) => {
            apply_room_error(generation, action, error, cx);
            return;
        }
    };

    let options = EasyTierStartOptions {
        disable_p2p: Some(disable_p2p),
        compression: Some("none".to_string()),
        ipv4: None,
    };
    let hostname = match intent.hostname(server_port, &player_name) {
        Some(hostname) => Some(hostname),
        None => {
            apply_room_error(
                generation,
                action,
                "无法生成 PaperConnect 联机中心标识".to_string(),
                cx,
            );
            return;
        }
    };
    let start_request = EasyTierStartRequest {
        network_name: room.network_name.clone(),
        network_secret: room.network_secret.clone(),
        peers,
        hostname,
        player_name: player_name.clone(),
        game_port,
        options: Some(options),
    };
    if let Err(error) = run_online(cx, crate::core::online::easytier_start(start_request)).await {
        apply_room_error(generation, action, error, cx);
        return;
    }

    let still_active = match cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.is_current_room_operation(generation)
    }) {
        Ok(active) => active,
        Err(error) => {
            warn!("failed to check online operation state: {error:?}");
            false
        }
    };
    if !still_active {
        if let Err(error) = run_online(cx, crate::core::online::easytier_stop()).await {
            warn!("failed to stop cancelled online operation: {error}");
        }
        return;
    }

    let mut client_state = None;
    if matches!(intent, RoomIntent::Join) {
        info!(
            network_name = %room.network_name,
            "PaperConnect 成员步骤 1/6：EasyTier 房间连接成功"
        );
        let server = match run_online(cx, crate::core::online::paperconnect_probe_server()).await {
            Ok(server) => server,
            Err(error) => {
                if let Err(stop_error) = run_online(cx, crate::core::online::easytier_stop()).await
                {
                    warn!("failed to stop after PaperConnect discovery failure: {stop_error}");
                }
                apply_room_error(generation, action, error, cx);
                return;
            }
        };
        match run_online(
            cx,
            crate::core::online::paperconnect_start_client(server, player_name.clone()),
        )
        .await
        {
            Ok(state) => client_state = Some(state),
            Err(error) => {
                if let Err(stop_error) = run_online(cx, crate::core::online::easytier_stop()).await
                {
                    warn!(
                        "failed to stop after PaperConnect player heartbeat failure: {stop_error}"
                    );
                }
                apply_room_error(
                    generation,
                    action,
                    format!("PaperConnect 玩家心跳失败：{error}"),
                    cx,
                );
                return;
            }
        }
    }

    let (status_result, peers_result) = run_online(cx, async {
        let results = tokio::join!(
            crate::core::online::easytier_embedded_status(),
            crate::core::online::easytier_embedded_peers(),
        );
        Ok(results)
    })
    .await
    .unwrap_or_else(|error| (Err(error.clone()), Err(error)));
    let status = status_result.ok().flatten();
    let peers = peers_result.map(peer_entries).unwrap_or_default();
    let players = player_entries(crate::core::online::paperconnect_players());
    apply_room_success(
        generation,
        intent,
        room,
        status,
        players,
        peers,
        client_state,
        cx,
    );
}

async fn resolve_room(intent: RoomIntent, room_code: String) -> Result<PaperConnectRoom, String> {
    match intent {
        RoomIntent::Create => crate::core::online::paperconnect_generate_room().await,
        RoomIntent::Join => crate::core::online::paperconnect_parse_room_code(room_code).await,
    }
}

async fn run_online<T, F>(cx: &AsyncApp, future: F) -> Result<T, String>
where
    T: Send + 'static,
    F: Future<Output = Result<T, String>> + Send + 'static,
{
    gpui_tokio::Tokio::spawn_result(cx, async move { future.await.map_err(anyhow::Error::msg) })
        .await
        .map_err(|error| error.to_string())
}

fn apply_room_error(generation: u64, action: &'static str, error: String, cx: &mut AsyncApp) {
    let abandoned_connectors = crate::core::online::easytier_take_abandoned_connectors();
    let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
        if !state.finish_room_operation(generation) {
            return false;
        }
        state.set_online_blocking_issue(OnlineBlockingIssue::from_room_error(&error));
        state.online_error = Some(SharedString::from(error.clone()));
        state.peers_loading = false;
        apply_abandoned_connectors(state, &abandoned_connectors);
        true
    });
    match applied {
        Ok(true) => {
            if let Err(update_error) = cx.update(|cx| {
                append_online_log(format!("{action}失败：{error}"), cx);
                toast::error(cx, t!("Online.room_failed"));
                append_abandoned_connector_logs(&abandoned_connectors, cx);
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
    players: Vec<OnlinePlayerEntry>,
    peers: Vec<OnlinePeerEntry>,
    client_state: Option<crate::core::online::PaperConnectClientState>,
    cx: &mut AsyncApp,
) {
    let room_code = room.room_code.clone();
    let abandoned_connectors = crate::core::online::easytier_take_abandoned_connectors();
    let discovery_port_occupied = matches!(
        client_state,
        Some(crate::core::online::PaperConnectClientState::DiscoveryPortOccupied)
    );
    let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
        if !state.finish_room_operation(generation) {
            return false;
        }
        state.online_error = discovery_port_occupied.then(|| {
            SharedString::from("本机 UDP 7551 已被占用，游戏代理未启动；关闭占用程序后请重新检查")
        });
        state.set_online_blocking_issue(
            discovery_port_occupied.then_some(OnlineBlockingIssue::DiscoveryPortOccupied),
        );
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
            state.easytier_game_host = status
                .game_host
                .map(SharedString::from)
                .unwrap_or_else(|| SharedString::from(""));
            state.easytier_game_port = status.game_port;
        }
        state.players = players;
        state.peers = peers;
        state.peers_loading = false;
        apply_abandoned_connectors(state, &abandoned_connectors);
        true
    });
    match applied {
        Ok(true) => {
            if let Err(update_error) = cx.update(|cx| {
                if matches!(intent, RoomIntent::Create) {
                    cx.write_to_clipboard(ClipboardItem::new_string(room_code.clone()));
                    toast::push(cx, t!("Online.room_created"));
                    append_online_log(format!("联机成功：{room_code}"), cx);
                } else if discovery_port_occupied {
                    toast::error(
                        cx,
                        t!("Online.joined_port_busy"),
                    );
                    append_online_log(
                        "房间连接已保留，但本机 7551 游戏代理启动失败；关闭占用程序后可直接重新检查",
                        cx,
                    );
                    warn!(
                        room_code,
                        "PaperConnect 成员联机未完成：EasyTier 已连接，但 UDP 7551 模拟代理未启动"
                    );
                } else {
                    toast::push(cx, t!("Online.joined_room"));
                    append_online_log(format!("联机成功：{room_code}"), cx);
                    info!(
                        room_code,
                        "PaperConnect 成员步骤 6/6：前端联机成功状态已应用"
                    );
                }
                append_abandoned_connector_logs(&abandoned_connectors, cx);
            }) {
                warn!("failed to report online room success: {update_error:?}");
            }
        }
        Ok(false) => {}
        Err(update_error) => warn!("failed to apply online room success: {update_error:?}"),
    }
}

pub(super) fn retry_discovery_proxy(cx: &mut App) {
    let generation =
        cx.update_global(|state: &mut ToolsPageState, _cx| state.begin_discovery_retry());
    let Some(generation) = generation else {
        return;
    };

    append_online_log("正在重新检测 UDP 7551 并启动本机游戏代理", cx);
    info!("PaperConnect 成员开始重新检测 UDP 7551");
    cx.spawn(async move |cx| {
        let result = run_online(
            cx,
            crate::core::online::paperconnect_retry_guest_transport(),
        )
        .await;
        let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
            if !state.finish_discovery_retry(generation) {
                return false;
            }

            match &result {
                Ok(crate::core::online::PaperConnectClientState::Ready) => {
                    state.set_online_blocking_issue(None);
                    state.online_error = None;
                }
                Ok(crate::core::online::PaperConnectClientState::DiscoveryPortOccupied) => {
                    state.set_online_blocking_issue(Some(
                        OnlineBlockingIssue::DiscoveryPortOccupied,
                    ));
                    state.online_error = Some(SharedString::from(
                        "本机 UDP 7551 仍被占用，游戏代理尚未启动",
                    ));
                }
                Err(error) => {
                    state.online_error = Some(SharedString::from(format!(
                        "重新启动 7551 游戏代理失败：{error}"
                    )));
                }
            }
            true
        });

        match applied {
            Ok(true) => {
                if let Err(update_error) = cx.update(|cx| match result {
                    Ok(crate::core::online::PaperConnectClientState::Ready) => {
                        append_online_log("UDP 7551 已释放，本机游戏代理启动成功", cx);
                        toast::push(cx, t!("Online.proxy_ready"));
                        info!("PaperConnect 成员重新检测成功：本机 UDP 7551 模拟代理已启动");
                    }
                    Ok(crate::core::online::PaperConnectClientState::DiscoveryPortOccupied) => {
                        append_online_log("UDP 7551 仍被占用，请关闭占用程序后再次检查", cx);
                        toast::error(cx, t!("Online.discovery_port_busy"));
                        warn!("PaperConnect 成员重新检测失败：UDP 7551 仍被占用");
                    }
                    Err(error) => {
                        append_online_log(format!("重新启动 7551 游戏代理失败：{error}"), cx);
                        toast::error(cx, t!("Online.proxy_restart_failed"));
                        warn!("PaperConnect 成员重新启动 7551 游戏代理失败：{error}");
                    }
                }) {
                    warn!("failed to report discovery proxy retry result: {update_error:?}");
                }
            }
            Ok(false) => {}
            Err(update_error) => {
                warn!("failed to apply discovery proxy retry result: {update_error:?}")
            }
        }
    })
    .detach();
}

pub(super) fn open_minecraft_termination_dialog(cx: &mut App) {
    cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.open_minecraft_termination_dialog();
    });
}

pub(super) fn dismiss_minecraft_termination_dialog(cx: &mut App) {
    cx.update_global(|state: &mut ToolsPageState, _cx| {
        state.dismiss_minecraft_termination_dialog();
    });
}

pub(super) fn confirm_minecraft_termination(cx: &mut App) {
    let started =
        cx.update_global(|state: &mut ToolsPageState, _cx| state.begin_minecraft_termination());
    if !started {
        return;
    }

    append_online_log("正在结束占用 UDP 7551 的应用", cx);
    cx.spawn(async move |cx| {
        let result = run_online(
            cx,
            crate::core::minecraft::process::terminate_discovery_port_owners(),
        )
        .await;
        let state_result = result
            .as_ref()
            .map(|_| ())
            .map_err(|error| SharedString::from(format!("结束 UDP 7551 占用应用失败：{error}")));
        let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
            state.finish_minecraft_termination(state_result)
        });

        match applied {
            Ok(true) => match result {
                Ok(summary) => {
                    if let Err(update_error) = cx.update(|cx| {
                        if summary.matched == 0 {
                            append_online_log("未查询到 UDP 7551 占用进程，继续重新检查端口", cx);
                            toast::error(cx, t!("Online.no_port_owner"));
                        } else {
                            append_online_log(
                                format!(
                                    "已结束 {} 个 UDP 7551 占用进程，正在重新检查端口",
                                    summary.terminated
                                ),
                                cx,
                            );
                            toast::push(cx, t!("Online.port_owner_stopped"));
                        }
                        retry_discovery_proxy(cx);
                    }) {
                        warn!(
                            "failed to retry discovery proxy after termination: {update_error:?}"
                        );
                    }
                }
                Err(error) => {
                    if let Err(update_error) = cx.update(|cx| {
                        append_online_log(format!("结束 UDP 7551 占用应用失败：{error}"), cx);
                        toast::error(cx, t!("Online.termination_failed"));
                    }) {
                        warn!(
                            "failed to report UDP 7551 owner termination error: {update_error:?}"
                        );
                    }
                }
            },
            Ok(false) => {}
            Err(update_error) => {
                warn!("failed to apply Minecraft termination result: {update_error:?}")
            }
        }
    })
    .detach();
}

pub(super) fn stop_session(cx: &mut App) {
    let generation =
        cx.update_global(|state: &mut ToolsPageState, _cx| state.begin_stop_operation());
    let Some(generation) = generation else {
        return;
    };
    append_online_log("正在断开 EasyTier", cx);

    cx.spawn(async move |cx| {
        let result = run_online(cx, crate::core::online::easytier_stop()).await;
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
                if let Err(update_error) = cx.update(|cx| {
                    let i18n = cx.global::<I18n>().clone();
                    match result {
                        Ok(()) => {
                            append_online_log("已断开联机", cx);
                            toast::push(cx, t!("Online.disconnected"));
                        }
                        Err(error) => {
                            append_online_log(format!("断开失败：{error}"), cx);
                            toast::error(cx, t!("Online.disconnect_failed"));
                        }
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

pub(crate) fn refresh_status(cx: &mut App) {
    let generation =
        cx.update_global(|state: &mut ToolsPageState, _cx| state.begin_status_refresh());
    let Some(generation) = generation else {
        return;
    };

    cx.spawn(async move |cx| {
        let (status_result, peers_result) = run_online(cx, async {
            let results = tokio::join!(
                crate::core::online::easytier_embedded_status(),
                crate::core::online::easytier_embedded_peers(),
            );
            Ok(results)
        })
        .await
        .unwrap_or_else(|error| (Err(error.clone()), Err(error)));
        let players = player_entries(crate::core::online::paperconnect_players());
        let abandoned_connectors = crate::core::online::easytier_take_abandoned_connectors();
        let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
            if !state.finish_status_refresh(generation) {
                return false;
            }
            match status_result {
                Ok(Some(status)) => {
                    state.easytier_running = true;
                    state.easytier_hostname = SharedString::from(status.hostname);
                    state.easytier_ipv4 = status.ipv4.map(SharedString::from);
                    state.easytier_game_host = status
                        .game_host
                        .map(SharedString::from)
                        .unwrap_or_else(|| SharedString::from(""));
                    state.easytier_game_port = status.game_port;
                    if state.online_blocking_issue.is_none() {
                        state.online_error = None;
                    }
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
            state.players = players;
            apply_abandoned_connectors(state, &abandoned_connectors);
            true
        });
        match applied {
            Ok(true) if !abandoned_connectors.is_empty() => {
                if let Err(update_error) = cx.update(|cx| {
                    append_abandoned_connector_logs(&abandoned_connectors, cx);
                    toast::error(
                        cx,
                        t!(
                            "Online.abandoned_retry_stopped",
                            count = &abandoned_connectors.len().to_string()
                        ),
                    );
                }) {
                    warn!("failed to report abandoned EasyTier connectors: {update_error:?}");
                }
            }
            Ok(_) => {}
            Err(update_error) => {
                warn!("failed to refresh online status: {update_error:?}");
            }
        }
    })
    .detach();
}

fn apply_abandoned_connectors(
    state: &mut ToolsPageState,
    connectors: &[crate::core::online::EasyTierAbandonedConnector],
) {
    for connector in connectors {
        state.add_abandoned_node(SharedString::from(connector.url.clone()));
    }
}

fn append_abandoned_connector_logs(
    connectors: &[crate::core::online::EasyTierAbandonedConnector],
    cx: &mut App,
) {
    for connector in connectors {
        append_online_log(
            format!(
                "节点 {} 连续连接失败 {} 次，本次联机已停止使用",
                connector.url, connector.failed_attempts
            ),
            cx,
        );
    }
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
        let result = run_online(cx, crate::core::online::easytier_embedded_peers()).await;
        let players = player_entries(crate::core::online::paperconnect_players());
        let applied = cx.update_global(|state: &mut ToolsPageState, _cx| {
            if !state.finish_online_operation(generation) {
                return false;
            }
            state.peers_loading = false;
            match result {
                Ok(peers) => {
                    state.peers = peer_entries(peers);
                    state.players = players;
                    if state.online_blocking_issue.is_none() {
                        state.online_error = None;
                    }
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

pub(crate) fn check_nat(cx: &mut App) {
    let generation = cx.update_global(|state: &mut ToolsPageState, _cx| state.begin_nat_check());
    let Some(generation) = generation else {
        return;
    };

    cx.spawn(async move |cx| {
        let snapshot_task = gpui_tokio::Tokio::spawn_result(cx, async {
            Ok::<_, anyhow::Error>(crate::core::easytier::api::detect_nat_types().await)
        });
        let snapshot = match snapshot_task.await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if let Err(update_error) = cx.update_global(|state: &mut ToolsPageState, _cx| {
                    if !state.finish_nat_check(generation) {
                        return;
                    }
                    state.nat_error = Some(SharedString::from(error.to_string()));
                }) {
                    warn!("failed to apply NAT error: {update_error:?}");
                }
                return;
            }
        };
        if let Err(update_error) = cx.update_global(|state: &mut ToolsPageState, _cx| {
            if !state.finish_nat_check(generation) {
                return;
            }
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
        .map(|peer| {
            let role = classify_peer_role(&peer.hostname);
            OnlinePeerEntry {
                hostname: SharedString::from(peer.hostname),
                ipv4: peer.ipv4.map(SharedString::from),
                role,
                connection_kind: peer.connection_kind,
                protocol: peer.protocol.map(SharedString::from),
                remote_endpoint: peer.remote_endpoint.map(SharedString::from),
                latency_ms: peer.latency_ms,
                via_hostname: peer.via_hostname.map(SharedString::from),
            }
        })
        .collect()
}

fn player_entries(players: Vec<PaperConnectPlayer>) -> Vec<OnlinePlayerEntry> {
    players
        .into_iter()
        .map(|player| OnlinePlayerEntry {
            player_name: SharedString::from(player.player),
            client_id: SharedString::from(player.client_id),
            is_room_host: player.is_room_host,
        })
        .collect()
}

fn classify_peer_role(hostname: &str) -> OnlinePeerRole {
    let hostname = hostname.trim().to_ascii_lowercase();
    if hostname.starts_with("paper-connect-server-") || hostname.starts_with("pcs-") {
        OnlinePeerRole::Server
    } else if hostname.starts_with("bmcbl-client-") {
        OnlinePeerRole::User
    } else if hostname.contains("public") || hostname.contains("relay") {
        OnlinePeerRole::Relay
    } else {
        OnlinePeerRole::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::apply_abandoned_connectors;
    use crate::core::online::EasyTierAbandonedConnector;
    use crate::ui::views::tools::state::ToolsPageState;

    #[test]
    fn abandoned_connector_notice_keeps_builtin_and_custom_nodes() {
        let mut state = ToolsPageState::default();
        let connectors = [
            EasyTierAbandonedConnector {
                url: "wss://center.node.1tmc.top".to_string(),
                failed_attempts: 3,
            },
            EasyTierAbandonedConnector {
                url: "tcp://custom.example:11010".to_string(),
                failed_attempts: 3,
            },
        ];

        apply_abandoned_connectors(&mut state, &connectors);
        apply_abandoned_connectors(&mut state, &connectors);

        assert_eq!(state.abandoned_nodes.len(), 2);
        assert_eq!(
            state.abandoned_nodes[0].as_ref(),
            "wss://center.node.1tmc.top"
        );
        assert_eq!(
            state.abandoned_nodes[1].as_ref(),
            "tcp://custom.example:11010"
        );
    }
}
