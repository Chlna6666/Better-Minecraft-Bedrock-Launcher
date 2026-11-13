use super::*;

pub async fn start(request: EasyTierStartRequest) -> Result<(), String> {
    let EasyTierStartRequest {
        network_name,
        network_secret,
        peers,
        mut hostname,
        player_name,
        mut game_port,
        options,
    } = request;
    if !(1025..=65535).contains(&game_port) {
        return Err(format!("invalid PaperConnect game port: {game_port}"));
    }
    if player_name.trim().is_empty() {
        return Err("PaperConnect player name is empty".to_string());
    }
    {
        let instance_id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        if instance_id.is_some() {
            return Err("EasyTier already running".to_string());
        }
        if ONLINE_STATE
            .easytier_cleanup_in_progress
            .load(Ordering::Acquire)
        {
            return Err("上一条联机连接仍在清理，请稍候再试".to_string());
        }
    }
    ONLINE_STATE
        .easytier_abandoned_connectors
        .lock()
        .map_err(|_| "EasyTier 放弃节点状态锁已损坏".to_string())?
        .clear();

    let peers = if peers.iter().any(|p| !p.trim().is_empty()) {
        let sanitized = sanitize_bootstrap_peers(peers);
        if sanitized.is_empty() {
            tracing::warn!("configured bootstrap peers are invalid; using fallback peers");
            default_bootstrap_peers().await
        } else {
            sanitized
        }
    } else {
        default_bootstrap_peers().await
    };

    let host_server_port = hostname
        .as_deref()
        .and_then(paperconnect::server_port_from_hostname);
    let mut host_protocol = None;
    if let Some(server_port) = host_server_port {
        let transport = paperconnect_transport::start_host().await?;
        game_port = transport.game_port;
        hostname = Some(paperconnect::build_server_hostname(
            server_port,
            transport.protocol,
            game_port,
        )?);
        host_protocol = Some(transport.protocol);
    }

    {
        let mut id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        if id.is_some() {
            paperconnect_transport::stop_all();
            return Err("EasyTier already running".to_string());
        }
        if ONLINE_STATE
            .easytier_cleanup_in_progress
            .load(Ordering::Acquire)
        {
            paperconnect_transport::stop_all();
            return Err("上一条联机连接仍在清理，请稍候再试".to_string());
        }
        let (cfg, resolved_hostname, resolved_ipv4) = match build_embedded_easytier_config(
            network_name.clone(),
            network_secret.clone(),
            peers.clone(),
            hostname.clone(),
            options.clone(),
        ) {
            Ok(config) => config,
            Err(error) => {
                paperconnect_transport::stop_all();
                return Err(error.to_string());
            }
        };

        *ONLINE_STATE.easytier_last_start.lock().unwrap() = Some(EasyTierLastStart {
            hostname: hostname.clone(),
            resolved_hostname,
            resolved_ipv4,
            game_port,
            options: options.clone(),
        });

        let is_host = hostname
            .as_deref()
            .and_then(paperconnect::server_port_from_hostname)
            .is_some();
        *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() =
            is_host.then(|| EasyTierGameEndpoint {
                host: "127.0.0.1".to_string(),
                port: game_port,
            });

        let instance_id = match ONLINE_STATE.easytier_manager.run_network_instance(
            cfg,
            true,
            ConfigFileControl::STATIC_CONFIG,
        ) {
            Ok(instance_id) => instance_id,
            Err(error) => {
                paperconnect_transport::stop_all();
                return Err(format!("start embedded EasyTier failed: {error}"));
            }
        };
        *id = Some(instance_id);
    }

    let instance_id = *ONLINE_STATE
        .easytier_instance_id
        .lock()
        .unwrap()
        .as_ref()
        .unwrap();
    start_easytier_event_monitor(instance_id);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let has_api = ONLINE_STATE
            .easytier_manager
            .get_instance_service(&instance_id)
            .is_some();
        if has_api {
            break;
        }

        let mut is_running = false;
        let mut last_err: Option<String> = None;
        for i in ONLINE_STATE.easytier_manager.iter() {
            if *i.key() != instance_id {
                continue;
            }
            is_running = i.value().is_easytier_running();
            last_err = i.value().get_latest_error_msg();
            break;
        }

        if !is_running {
            paperconnect_transport::stop_all();
            *ONLINE_STATE.easytier_instance_id.lock().unwrap() = None;
            *ONLINE_STATE.easytier_last_start.lock().unwrap() = None;
            *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() = None;
            let _ = ONLINE_STATE
                .easytier_manager
                .delete_network_instance(vec![instance_id]);
            return Err(format!(
                "embedded EasyTier stopped during startup: {}",
                last_err.unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        if Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if let Some(server_port) = hostname
        .as_deref()
        .and_then(paperconnect::server_port_from_hostname)
    {
        if let Err(error) = paperconnect::start_server(
            server_port,
            game_port,
            host_protocol.unwrap_or_default(),
            player_name.clone(),
        )
        .await
        {
            stop().await?;
            return Err(error);
        }
        if let Err(error) = paperconnect::ping("127.0.0.1", server_port).await {
            if let Err(stop_error) = stop().await {
                tracing::warn!("PaperConnect 联机中心自检失败后停止 EasyTier 失败：{stop_error}");
            }
            return Err(format!("PaperConnect 联机中心本机自检失败：{error}"));
        }
        if let Err(error) =
            paperconnect::start_client("127.0.0.1".to_string(), server_port, player_name).await
        {
            if let Err(stop_error) = stop().await {
                tracing::warn!("房主 c:player 首包失败后停止 EasyTier 失败：{stop_error}");
            }
            return Err(format!("PaperConnect 房主玩家心跳失败：{error}"));
        }
        tracing::info!(
            server_port,
            game_port,
            "PaperConnect 联机中心已启动并通过本机自检"
        );
    }

    Ok(())
}

pub async fn stop() -> Result<(), String> {
    paperconnect_transport::stop_all();
    paperconnect::stop_server();
    paperconnect::stop_client();
    *ONLINE_STATE
        .paperconnect_guest_transport
        .lock()
        .map_err(|_| "PaperConnect 成员传输状态锁已损坏".to_string())? = None;
    paperconnect::clear_players();
    let instance_id = {
        let mut instance_id = ONLINE_STATE.easytier_instance_id.lock().unwrap();
        let instance_id = instance_id.take();
        if instance_id.is_some() {
            ONLINE_STATE
                .easytier_cleanup_in_progress
                .store(true, Ordering::Release);
        }
        instance_id
    };
    *ONLINE_STATE.easytier_last_start.lock().unwrap() = None;
    *ONLINE_STATE.easytier_game_endpoint.lock().unwrap() = None;
    if let Some(id) = instance_id {
        let manager = ONLINE_STATE.easytier_manager.clone();
        let cleanup_in_progress = ONLINE_STATE.easytier_cleanup_in_progress.clone();
        let mut cleanup = Box::pin(crate::tasks::runtime::blocking::run_io(move || {
            manager.delete_network_instance(vec![id])
        }));
        match tokio::time::timeout(EASYTIER_STOP_TIMEOUT, cleanup.as_mut()).await {
            Ok(joined) => {
                cleanup_in_progress.store(false, Ordering::Release);
                match joined {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => tracing::warn!(
                        instance_id = %id,
                        "EasyTier 实例清理失败，但联机会话已关闭：{error}"
                    ),
                    Err(error) => tracing::warn!(
                        instance_id = %id,
                        "EasyTier 实例清理任务异常，但联机会话已关闭：{error}"
                    ),
                }
            }
            Err(_) => {
                tracing::warn!(
                    instance_id = %id,
                    timeout = ?EASYTIER_STOP_TIMEOUT,
                    "停止 EasyTier 超时，连接状态已关闭，后台继续清理实例"
                );
                let cleanup_flag = cleanup_in_progress.clone();
                if let Err(error) = crate::tasks::runtime::spawn_io(async move {
                    match cleanup.await {
                        Ok(Ok(_)) => tracing::info!(instance_id = %id, "EasyTier 后台清理完成"),
                        Ok(Err(error)) => tracing::warn!(
                            instance_id = %id,
                            "EasyTier 后台清理失败：{error}"
                        ),
                        Err(error) => tracing::warn!(
                            instance_id = %id,
                            "EasyTier 后台清理任务异常：{error}"
                        ),
                    }
                    cleanup_flag.store(false, Ordering::Release);
                }) {
                    cleanup_in_progress.store(false, Ordering::Release);
                    tracing::warn!(
                        instance_id = %id,
                        "无法调度 EasyTier 后台清理：{error}"
                    );
                }
            }
        }
    }
    Ok(())
}

pub async fn status() -> Result<Option<EasyTierEmbeddedStatus>, String> {
    let id = match ONLINE_STATE.easytier_instance_id.lock().unwrap().as_ref() {
        Some(v) => *v,
        None => return Ok(None),
    };

    let svc = match ONLINE_STATE.easytier_manager.get_instance_service(&id) {
        Some(v) => v,
        None => return Ok(None),
    };

    let resp = tokio::time::timeout(
        EASYTIER_API_TIMEOUT,
        svc.get_peer_manage_service()
            .list_route(BaseController::default(), ListRouteRequest::default()),
    )
    .await
    .map_err(|_| "EasyTier 路由查询超时".to_string())?
    .map_err(|e| format!("list_route failed: {e}"))?;

    let inst_id = id.to_string();
    let mut hostname = String::new();
    let mut ipv4: Option<String> = None;
    for r in resp.routes {
        if r.inst_id != inst_id {
            continue;
        }
        hostname = r.hostname;
        ipv4 = r.ipv4_addr.map(|inet| {
            let s = inet.to_string();
            s.split_once('/').map(|v| v.0.to_string()).unwrap_or(s)
        });
        break;
    }

    let last_start = ONLINE_STATE.easytier_last_start.lock().unwrap().clone();
    if hostname.trim().is_empty() || ipv4.as_deref().unwrap_or_default().trim().is_empty() {
        if let Some(last) = last_start.as_ref() {
            if hostname.trim().is_empty() {
                if let Some(hn) = last
                    .resolved_hostname
                    .clone()
                    .or_else(|| last.hostname.clone())
                {
                    hostname = hn;
                }
            }
            if ipv4.as_deref().unwrap_or_default().trim().is_empty() {
                if let Some(v) = last.resolved_ipv4.clone() {
                    if !v.trim().is_empty() {
                        ipv4 = Some(v);
                    }
                } else if let Some(opts) = last.options.clone() {
                    if let Some(v) = opts.ipv4 {
                        let raw = v.trim();
                        if !raw.is_empty() {
                            let ip = raw.split_once('/').map(|v| v.0).unwrap_or(raw);
                            ipv4 = Some(ip.to_string());
                        }
                    }
                }
                if ipv4.is_none() {
                    let hn = hostname.trim();
                    if paperconnect::server_port_from_hostname(hn).is_some() {
                        ipv4 = Some(DEFAULT_PAPERCONNECT_VIP.to_string());
                    }
                }
            }
        }
    }

    let game_endpoint = ONLINE_STATE.easytier_game_endpoint.lock().unwrap().clone();
    let game_port = game_endpoint
        .as_ref()
        .map(|endpoint| endpoint.port)
        .or_else(|| last_start.as_ref().map(|value| value.game_port));
    let game_host = game_endpoint.map(|endpoint| endpoint.host);

    Ok(Some(EasyTierEmbeddedStatus {
        instance_id: inst_id,
        hostname,
        ipv4,
        game_host,
        game_port,
    }))
}

pub async fn peers() -> Result<Vec<EasyTierPeer>, String> {
    let id = ONLINE_STATE
        .easytier_instance_id
        .lock()
        .unwrap()
        .ok_or_else(|| "EasyTier not running".to_string())?;

    let svc = ONLINE_STATE
        .easytier_manager
        .get_instance_service(&id)
        .ok_or_else(|| "EasyTier API service not available".to_string())?;

    let (route_result, peer_result) = tokio::join!(
        tokio::time::timeout(
            EASYTIER_API_TIMEOUT,
            svc.get_peer_manage_service()
                .list_route(BaseController::default(), ListRouteRequest::default()),
        ),
        tokio::time::timeout(
            EASYTIER_API_TIMEOUT,
            svc.get_peer_manage_service()
                .list_peer(BaseController::default(), ListPeerRequest::default()),
        ),
    );
    let route_response = route_result
        .map_err(|_| "EasyTier 路由查询超时".to_string())?
        .map_err(|error| format!("EasyTier 路由查询失败：{error}"))?;
    let peer_infos = match peer_result {
        Ok(Ok(response)) => response.peer_infos,
        Ok(Err(error)) => {
            tracing::debug!("EasyTier 连接详情查询失败，保留路由节点：{error}");
            Vec::new()
        }
        Err(_) => {
            tracing::debug!("EasyTier 连接详情查询超时，保留路由节点");
            Vec::new()
        }
    };

    let routes = route_response.routes;
    let pairs = list_peer_route_pair(peer_infos.clone(), routes.clone());
    let instance_id = id.to_string();
    let mut peers = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let Some(route) = pair.route else {
            continue;
        };
        let ipv4 = route.ipv4_addr.map(|inet| {
            let s = inet.to_string();
            s.split_once('/').map(|v| v.0.to_string()).unwrap_or(s)
        });

        let mut hostname = route.hostname.clone();
        if hostname.trim().is_empty() {
            let route_instance_id = route.inst_id.trim();
            hostname = if route_instance_id.is_empty() {
                "node-unknown".to_string()
            } else {
                format!("node-{route_instance_id}")
            };
        }
        let connection_kind = if route.inst_id == instance_id {
            EasyTierConnectionKind::Local
        } else if route.cost == 1 {
            EasyTierConnectionKind::Direct
        } else if route.cost > 1 {
            EasyTierConnectionKind::Relayed
        } else {
            EasyTierConnectionKind::Unknown
        };
        let next_hop_peer_id = route
            .next_hop_peer_id_latency_first
            .unwrap_or(route.next_hop_peer_id);
        let connection_peer = match connection_kind {
            EasyTierConnectionKind::Direct => pair.peer.as_ref(),
            EasyTierConnectionKind::Relayed => peer_infos
                .iter()
                .find(|peer| peer.peer_id == next_hop_peer_id),
            EasyTierConnectionKind::Local | EasyTierConnectionKind::Unknown => None,
        };
        let connection = connection_peer.and_then(preferred_peer_connection);
        let protocol = connection
            .and_then(|connection| connection.tunnel.as_ref())
            .map(|tunnel| tunnel.tunnel_type.to_ascii_uppercase())
            .filter(|protocol| !protocol.is_empty());
        let remote_endpoint = connection
            .and_then(|connection| connection.tunnel.as_ref())
            .and_then(|tunnel| tunnel.remote_addr.as_ref())
            .map(ToString::to_string)
            .filter(|endpoint| !endpoint.is_empty());
        let latency_ms = match connection_kind {
            EasyTierConnectionKind::Direct => connection
                .and_then(|connection| connection.stats.as_ref())
                .map(|stats| stats.latency_us.div_ceil(1_000)),
            EasyTierConnectionKind::Relayed => route
                .path_latency_latency_first
                .filter(|latency| *latency > 0)
                .or_else(|| (route.path_latency > 0).then_some(route.path_latency))
                .map(|latency| latency as u64),
            EasyTierConnectionKind::Local | EasyTierConnectionKind::Unknown => None,
        };
        let via_hostname = (connection_kind == EasyTierConnectionKind::Relayed)
            .then(|| {
                routes
                    .iter()
                    .find(|candidate| candidate.peer_id == next_hop_peer_id)
                    .map(|candidate| candidate.hostname.trim().to_string())
            })
            .flatten()
            .filter(|hostname| !hostname.is_empty());
        peers.push(EasyTierPeer {
            ipv4,
            hostname,
            connection_kind,
            protocol,
            remote_endpoint,
            latency_ms,
            via_hostname,
        });
    }
    Ok(peers)
}

pub fn take_abandoned_connectors() -> Vec<EasyTierAbandonedConnector> {
    match ONLINE_STATE.easytier_abandoned_connectors.lock() {
        Ok(mut abandoned) => std::mem::take(&mut *abandoned),
        Err(_) => {
            tracing::warn!("EasyTier 放弃节点状态锁已损坏");
            Vec::new()
        }
    }
}
