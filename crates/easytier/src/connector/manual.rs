use std::{
    collections::BTreeSet,
    future::Future,
    sync::{Arc, Weak},
    time::Duration,
};

use dashmap::{DashMap, DashSet};
use quanta::Instant;
use tokio::{sync::mpsc, task::JoinSet, time::timeout};

use crate::{
    common::{PeerId, dns::socket_addrs, join_joinset_background},
    peers::peer_conn::PeerConnId,
    proto::{
        api::instance::{
            Connector, ConnectorManageRpc, ConnectorStatus, ListConnectorRequest,
            ListConnectorResponse,
        },
        rpc_types::{self, controller::BaseController},
    },
    tunnel::{IpVersion, TunnelConnector, TunnelScheme, matches_scheme},
    utils::weak_upgrade,
};

use crate::{
    common::{
        error::Error,
        global_ctx::{ArcGlobalCtx, GlobalCtxEvent},
        netns::NetNS,
    },
    peers::peer_manager::PeerManager,
    use_global_var,
};

use super::create_connector_by_url;

const DIRECT_RECONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const LONG_RECONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_RECONNECT_FAILURES: u8 = 3;

type ConnectorMap = Arc<DashSet<url::Url>>;

#[derive(Debug, Clone)]
struct ReconnResult {
    peer_id: PeerId,
    conn_id: PeerConnId,
}

struct ConnectorManagerData {
    connectors: ConnectorMap,
    reconnecting: DashSet<url::Url>,
    reconnect_failures: DashMap<url::Url, u8>,
    peer_manager: Weak<PeerManager>,
    alive_conn_urls: Arc<DashSet<url::Url>>,
    // user removed connector urls
    removed_conn_urls: Arc<DashSet<url::Url>>,
    net_ns: NetNS,
    global_ctx: ArcGlobalCtx,
}

pub struct ManualConnectorManager {
    global_ctx: ArcGlobalCtx,
    data: Arc<ConnectorManagerData>,
    tasks: JoinSet<()>,
}

impl ManualConnectorManager {
    pub fn new(global_ctx: ArcGlobalCtx, peer_manager: Arc<PeerManager>) -> Self {
        let connectors = Arc::new(DashSet::new());
        let tasks = JoinSet::new();

        let mut ret = Self {
            global_ctx: global_ctx.clone(),
            data: Arc::new(ConnectorManagerData {
                connectors,
                reconnecting: DashSet::new(),
                reconnect_failures: DashMap::new(),
                peer_manager: Arc::downgrade(&peer_manager),
                alive_conn_urls: Arc::new(DashSet::new()),
                removed_conn_urls: Arc::new(DashSet::new()),
                net_ns: global_ctx.net_ns.clone(),
                global_ctx,
            }),
            tasks,
        };

        ret.tasks
            .spawn(Self::conn_mgr_reconn_routine(ret.data.clone()));

        ret
    }

    fn reconnect_timeout(dead_url: &url::Url) -> Duration {
        let use_long_timeout = matches_scheme!(
            dead_url,
            TunnelScheme::Http | TunnelScheme::Https | TunnelScheme::Txt | TunnelScheme::Srv
        ) || matches!(dead_url.scheme(), "ws" | "wss");

        // PeerConn waits up to five seconds for the handshake response. The
        // reconnect budget must also leave room for DNS resolution and socket
        // establishment before that stage begins.
        if use_long_timeout {
            LONG_RECONNECT_TIMEOUT
        } else {
            DIRECT_RECONNECT_TIMEOUT
        }
    }

    fn should_abandon(failed_attempts: u8) -> bool {
        failed_attempts >= MAX_RECONNECT_FAILURES
    }

    fn remaining_budget(started_at: Instant, total_timeout: Duration) -> Option<Duration> {
        let remaining = total_timeout.checked_sub(started_at.elapsed())?;
        (!remaining.is_zero()).then_some(remaining)
    }

    fn emit_connect_error(
        data: &ConnectorManagerData,
        dead_url: &url::Url,
        ip_version: IpVersion,
        error: &Error,
    ) {
        data.global_ctx.issue_event(GlobalCtxEvent::ConnectError(
            dead_url.to_string(),
            format!("{:?}", ip_version),
            Self::compact_error(error),
        ));
    }

    fn compact_error(error: &Error) -> String {
        match error {
            Error::AnyhowError(error) => format!("{error:#}"),
            error => error.to_string(),
        }
    }

    fn reconnect_timeout_error(stage: &str, duration: Duration) -> Error {
        Error::AnyhowError(anyhow::anyhow!(
            "{stage} timeout after {:.3}s",
            duration.as_secs_f64()
        ))
    }

    async fn with_reconnect_timeout<T, F>(
        stage: &'static str,
        started_at: Instant,
        total_timeout: Duration,
        fut: F,
    ) -> Result<T, Error>
    where
        F: Future<Output = Result<T, Error>>,
    {
        let remaining = Self::remaining_budget(started_at, total_timeout)
            .ok_or_else(|| Self::reconnect_timeout_error(stage, started_at.elapsed()))?;
        timeout(remaining, fut)
            .await
            .map_err(|_| Self::reconnect_timeout_error(stage, remaining))?
    }
}

impl ManualConnectorManager {
    pub fn add_connector<T>(&self, connector: T)
    where
        T: TunnelConnector + 'static,
    {
        tracing::info!("add_connector: {}", connector.remote_url());
        let remote_url = connector.remote_url();
        self.data.reconnect_failures.remove(&remote_url);
        self.data.connectors.insert(remote_url);
    }

    pub async fn add_connector_by_url(&self, url: url::Url) -> Result<(), Error> {
        self.data.reconnect_failures.remove(&url);
        self.data.connectors.insert(url);
        Ok(())
    }

    pub async fn remove_connector(&self, url: url::Url) -> Result<(), Error> {
        tracing::info!("remove_connector: {}", url);
        let url = url.into();
        if !self
            .list_connectors()
            .await
            .iter()
            .any(|x| x.url.as_ref() == Some(&url))
        {
            return Err(Error::NotFound);
        }
        self.data.removed_conn_urls.insert(url.into());
        Ok(())
    }

    pub async fn clear_connectors(&self) {
        self.list_connectors().await.iter().for_each(|x| {
            if let Some(url) = &x.url {
                self.data.removed_conn_urls.insert(url.clone().into());
            }
        });
    }

    pub async fn list_connectors(&self) -> Vec<Connector> {
        let dead_urls: BTreeSet<url::Url> = Self::collect_dead_conns(self.data.clone())
            .await
            .into_iter()
            .collect();

        let mut ret = Vec::new();

        for item in self.data.connectors.iter() {
            let conn_url = item.key().clone();
            let mut status = ConnectorStatus::Connected;
            if dead_urls.contains(&conn_url) {
                status = ConnectorStatus::Disconnected;
            }
            ret.insert(
                0,
                Connector {
                    url: Some(conn_url.into()),
                    status: status.into(),
                },
            );
        }

        let reconnecting_urls: BTreeSet<url::Url> =
            self.data.reconnecting.iter().map(|x| x.clone()).collect();

        for conn_url in reconnecting_urls {
            ret.insert(
                0,
                Connector {
                    url: Some(conn_url.into()),
                    status: ConnectorStatus::Connecting.into(),
                },
            );
        }

        ret
    }

    async fn conn_mgr_reconn_routine(data: Arc<ConnectorManagerData>) {
        tracing::debug!("manual connector manager started");
        let mut reconn_interval = tokio::time::interval(std::time::Duration::from_millis(
            use_global_var!(MANUAL_CONNECTOR_RECONNECT_INTERVAL_MS),
        ));
        let (reconn_result_send, mut reconn_result_recv) = mpsc::channel(100);
        let tasks = Arc::new(std::sync::Mutex::new(JoinSet::new()));
        join_joinset_background(tasks.clone(), "connector_reconnect_tasks".to_string());

        loop {
            tokio::select! {
                _ = reconn_interval.tick() => {
                    let dead_urls = Self::collect_dead_conns(data.clone()).await;
                    if dead_urls.is_empty() {
                        continue;
                    }
                    for dead_url in dead_urls {
                        let data_clone = data.clone();
                        let sender = reconn_result_send.clone();
                        if data.connectors.remove(&dead_url).is_none()
                            || !data.reconnecting.insert(dead_url.clone())
                        {
                            continue;
                        }

                        tasks.lock().unwrap().spawn(async move {
                            let reconn_ret = Self::conn_reconnect(data_clone.clone(), dead_url.clone() ).await;
                            data_clone.reconnecting.remove(&dead_url);
                            if sender.send((dead_url.clone(), reconn_ret)).await.is_err() {
                                tracing::debug!(%dead_url, "reconnect result receiver closed");
                            }
                        });
                    }
                    tracing::debug!("manual connector reconnect interval processed");
                }

                ret = reconn_result_recv.recv() => {
                    let Some((dead_url, result)) = ret else {
                        return;
                    };
                    match result {
                        Ok(result) => {
                            data.reconnect_failures.remove(&dead_url);
                            data.connectors.insert(dead_url.clone());
                            tracing::info!(
                                %dead_url,
                                peer_id = %result.peer_id,
                                conn_id = %result.conn_id,
                                "manual connector reconnected"
                            );
                        }
                        Err(error) => {
                            let failed_attempts = {
                                let mut entry = data
                                    .reconnect_failures
                                    .entry(dead_url.clone())
                                    .or_insert(0);
                                *entry = entry.saturating_add(1);
                                *entry
                            };
                            if Self::should_abandon(failed_attempts) {
                                data.reconnect_failures.remove(&dead_url);
                                let error_message = Self::compact_error(&error);
                                data.global_ctx.issue_event(
                                    GlobalCtxEvent::ConnectorAbandoned(
                                        dead_url.to_string(),
                                        error_message,
                                        failed_attempts,
                                    ),
                                );
                                tracing::warn!(
                                    %dead_url,
                                    failed_attempts,
                                    "manual connector abandoned for this session"
                                );
                            } else {
                                data.connectors.insert(dead_url.clone());
                                let error_message = Self::compact_error(&error);
                                tracing::warn!(
                                    %dead_url,
                                    failed_attempts,
                                    max_attempts = MAX_RECONNECT_FAILURES,
                                    error = %error_message,
                                    "manual connector reconnect failed"
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    fn handle_remove_connector(data: Arc<ConnectorManagerData>) {
        let remove_later = DashSet::new();
        for it in data.removed_conn_urls.iter() {
            let url = it.key();
            if data.connectors.remove(url).is_some() {
                data.reconnect_failures.remove(url);
                tracing::warn!("connector: {}, removed", url);
                continue;
            } else if data.reconnecting.contains(url) {
                tracing::warn!("connector: {}, reconnecting, remove later.", url);
                remove_later.insert(url.clone());
                continue;
            } else {
                tracing::warn!("connector: {}, not found", url);
            }
        }
        data.removed_conn_urls.clear();
        for it in remove_later.iter() {
            data.removed_conn_urls.insert(it.key().clone());
        }
    }

    async fn collect_dead_conns(data: Arc<ConnectorManagerData>) -> BTreeSet<url::Url> {
        Self::handle_remove_connector(data.clone());
        let mut ret = BTreeSet::new();
        let Some(pm) = data.peer_manager.upgrade() else {
            tracing::warn!("peer manager is gone, exit");
            return ret;
        };
        for url in data.connectors.iter().map(|x| x.key().clone()) {
            if !pm.get_peer_map().is_client_url_alive(&url)
                && !pm
                    .get_foreign_network_client()
                    .get_peer_map()
                    .is_client_url_alive(&url)
            {
                ret.insert(url.clone());
            }
        }
        ret
    }

    async fn conn_reconnect_with_ip_version(
        data: Arc<ConnectorManagerData>,
        dead_url: url::Url,
        ip_version: IpVersion,
        started_at: Instant,
        total_timeout: Duration,
    ) -> Result<ReconnResult, Error> {
        let connector = Self::with_reconnect_timeout(
            "resolve",
            started_at,
            total_timeout,
            create_connector_by_url(dead_url.as_str(), &data.global_ctx, ip_version),
        )
        .await?;

        data.global_ctx
            .issue_event(GlobalCtxEvent::Connecting(connector.remote_url()));
        tracing::debug!("reconnect try connect... conn: {:?}", connector);
        let Some(pm) = data.peer_manager.upgrade() else {
            return Err(Error::AnyhowError(anyhow::anyhow!(
                "peer manager is gone, cannot reconnect"
            )));
        };

        let tunnel = Self::with_reconnect_timeout(
            "connect",
            started_at,
            total_timeout,
            pm.connect_tunnel(connector),
        )
        .await?;

        let (peer_id, conn_id) = Self::with_reconnect_timeout(
            "handshake",
            started_at,
            total_timeout,
            pm.add_client_tunnel_with_peer_id_hint(tunnel, true, None),
        )
        .await?;

        tracing::info!("reconnect succ: {} {} {}", peer_id, conn_id, dead_url);
        Ok(ReconnResult { peer_id, conn_id })
    }

    async fn conn_reconnect(
        data: Arc<ConnectorManagerData>,
        dead_url: url::Url,
    ) -> Result<ReconnResult, Error> {
        tracing::debug!("reconnect: {}", dead_url);

        let mut ip_versions = vec![];
        if matches_scheme!(
            dead_url,
            TunnelScheme::Ring | TunnelScheme::Txt | TunnelScheme::Srv
        ) {
            ip_versions.push(IpVersion::Both);
        } else {
            let converted_dead_url =
                match crate::common::idn::convert_idn_to_ascii(dead_url.clone()) {
                    Ok(url) => url,
                    Err(error) => {
                        let error: Error = error.into();
                        Self::emit_connect_error(&data, &dead_url, IpVersion::Both, &error);
                        return Err(error);
                    }
                };
            let addrs = match Self::with_reconnect_timeout(
                "resolve",
                Instant::now(),
                Self::reconnect_timeout(&dead_url),
                socket_addrs(&converted_dead_url, || Some(1000)),
            )
            .await
            {
                Ok(addrs) => addrs,
                Err(error) => {
                    Self::emit_connect_error(&data, &dead_url, IpVersion::Both, &error);
                    return Err(error);
                }
            };
            tracing::debug!(?addrs, ?dead_url, "get ip from url done");
            let mut has_ipv4 = false;
            let mut has_ipv6 = false;
            for addr in addrs {
                if addr.is_ipv4() {
                    if !has_ipv4 {
                        ip_versions.insert(0, IpVersion::V4);
                    }
                    has_ipv4 = true;
                } else if addr.is_ipv6() {
                    if !has_ipv6 {
                        ip_versions.push(IpVersion::V6);
                    }
                    has_ipv6 = true;
                }
            }
        }

        let mut reconn_ret = Err(Error::AnyhowError(anyhow::anyhow!(
            "cannot get ip from url"
        )));
        for ip_version in ip_versions {
            let started_at = Instant::now();
            let ret = Self::conn_reconnect_with_ip_version(
                data.clone(),
                dead_url.clone(),
                ip_version,
                started_at,
                Self::reconnect_timeout(&dead_url),
            )
            .await;
            match &ret {
                Ok(result) => tracing::debug!(
                    %dead_url,
                    peer_id = %result.peer_id,
                    conn_id = %result.conn_id,
                    "manual connector reconnect attempt succeeded"
                ),
                Err(error) => {
                    let error_message = Self::compact_error(error);
                    tracing::debug!(
                        %dead_url,
                        error = %error_message,
                        "manual connector reconnect attempt failed"
                    );
                }
            }

            match ret {
                Ok(result) => return Ok(result),
                Err(error) => {
                    Self::emit_connect_error(&data, &dead_url, ip_version, &error);
                    reconn_ret = Err(error);
                }
            }
        }

        reconn_ret
    }
}

#[derive(Clone)]
pub struct ConnectorManagerRpcService(pub Weak<ManualConnectorManager>);

#[async_trait::async_trait]
impl ConnectorManageRpc for ConnectorManagerRpcService {
    type Controller = BaseController;

    async fn list_connector(
        &self,
        _: BaseController,
        _request: ListConnectorRequest,
    ) -> Result<ListConnectorResponse, rpc_types::error::Error> {
        let mut ret = ListConnectorResponse::default();
        let connectors = weak_upgrade(&self.0)?.list_connectors().await;
        ret.connectors = connectors;
        Ok(ret)
    }
}

#[cfg(test)]
mod tests {
    use crate::{peers::tests::create_mock_peer_manager, set_global_var};

    use super::*;

    #[test]
    fn connector_is_abandoned_after_three_consecutive_failures() {
        assert!(!ManualConnectorManager::should_abandon(1));
        assert!(!ManualConnectorManager::should_abandon(2));
        assert!(ManualConnectorManager::should_abandon(3));
        assert!(ManualConnectorManager::should_abandon(4));
    }

    #[test]
    fn compact_error_omits_anyhow_debug_backtrace() {
        let error = Error::AnyhowError(anyhow::anyhow!("connect timeout"));
        let summary = ManualConnectorManager::compact_error(&error);

        assert_eq!(summary, "connect timeout");
        assert!(!summary.contains("Stack backtrace"));
    }

    #[tokio::test]
    async fn reconnect_timeout_reports_exhausted_budget_for_stage() {
        let started_at = Instant::now() - Duration::from_millis(50);
        let err = ManualConnectorManager::with_reconnect_timeout(
            "resolve",
            started_at,
            Duration::from_millis(1),
            async { Ok::<(), Error>(()) },
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("resolve timeout after"));
    }

    #[tokio::test]
    async fn reconnect_timeout_reports_stage_timeout_with_remaining_budget() {
        let err = ManualConnectorManager::with_reconnect_timeout(
            "handshake",
            Instant::now(),
            Duration::from_millis(10),
            async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<(), Error>(())
            },
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("handshake timeout after"));
    }

    #[tokio::test]
    async fn reconnect_timeout_preserves_success_within_budget() {
        let result = ManualConnectorManager::with_reconnect_timeout(
            "connect",
            Instant::now(),
            Duration::from_millis(50),
            async { Ok::<_, Error>(123_u32) },
        )
        .await
        .unwrap();

        assert_eq!(result, 123);
    }

    #[test]
    fn reconnect_timeout_covers_direct_handshake_window() {
        let tcp: url::Url = "tcp://127.0.0.1:11010".parse().unwrap();
        let udp: url::Url = "udp://127.0.0.1:11010".parse().unwrap();
        let websocket: url::Url = "wss://example.com".parse().unwrap();

        assert_eq!(
            ManualConnectorManager::reconnect_timeout(&tcp),
            DIRECT_RECONNECT_TIMEOUT
        );
        assert_eq!(
            ManualConnectorManager::reconnect_timeout(&udp),
            DIRECT_RECONNECT_TIMEOUT
        );
        assert_eq!(
            ManualConnectorManager::reconnect_timeout(&websocket),
            LONG_RECONNECT_TIMEOUT
        );
        assert!(DIRECT_RECONNECT_TIMEOUT >= Duration::from_secs(5));
    }

    #[tokio::test]
    async fn connector_is_removed_after_third_failed_reconnect() {
        set_global_var!(MANUAL_CONNECTOR_RECONNECT_INTERVAL_MS, 1);

        let peer_mgr = create_mock_peer_manager().await;
        let global_ctx = peer_mgr.get_global_ctx();
        let mut events = global_ctx.subscribe();
        let mgr = ManualConnectorManager::new(global_ctx, peer_mgr.clone());
        let connector_url: url::Url = "tcp://127.0.0.1:1"
            .parse()
            .expect("test connector URL should parse");
        mgr.add_connector_by_url(connector_url.clone())
            .await
            .expect("test connector should be added");

        let abandoned_attempts = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let GlobalCtxEvent::ConnectorAbandoned(url, _, failed_attempts) = events
                    .recv()
                    .await
                    .expect("connector event channel should remain open")
                    && url == connector_url.as_str()
                {
                    break failed_attempts;
                }
            }
        })
        .await
        .expect("unreachable connector should be abandoned within the test timeout");

        assert_eq!(abandoned_attempts, MAX_RECONNECT_FAILURES);
        assert!(mgr.list_connectors().await.is_empty());
    }
}
