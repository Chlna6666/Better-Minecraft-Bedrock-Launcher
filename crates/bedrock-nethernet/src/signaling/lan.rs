//! 局域网信令：UDP 上的世界发现与信令投递。
//!
//! 并发模型：一个专职接收任务读取套接字，解密/校验后按类型分发。
//! 共享状态用 `std::sync::RwLock`（临界区内无 await），
//! 发送路径直接写套接字，不经过任务间转发。

use crate::consts::{
    DISCOVERY_RETRY_INTERVAL, DISCOVERY_TTL, LAN_DISCOVERY_PORT, MAX_DISCOVERY_PACKET,
    MAX_SIGNAL_SIZE,
};
use crate::error::{NethernetError, Result};
use crate::protocol::{DiscoveryPacket, ServerData, Signal, SignalType};
use crate::signaling::Signaling;
use rand::RngExt as _;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};
use tokio::net::UdpSocket;
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;

/// 入站信令广播通道容量。
const SIGNAL_BROADCAST_CAPACITY: usize = 256;
/// 记录的对端网络数上限，防止无界增长。
const MAX_TRACKED_NETWORKS: usize = 1024;
/// 单地址每 100ms 允许处理的发现报文数。
const PER_ADDR_RATE: u32 = 64;
const RATE_WINDOW: Duration = Duration::from_millis(100);

/// 发现到的一个局域网世界。
#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub network_id: u64,
    pub address: SocketAddr,
    pub server_data: ServerData,
}

/// 网络 ID → 地址 + 最近活跃时间。
#[derive(Debug, Clone, Copy)]
struct Peer {
    address: SocketAddr,
    seen: Instant,
}

/// 发现到的世界条目。
#[derive(Debug, Clone)]
struct Entry {
    server_data: ServerData,
    seen: Instant,
}

#[derive(Default)]
struct Shared {
    peers: RwLock<HashMap<u64, Peer>>,
    discovered: RwLock<HashMap<u64, Entry>>,
    /// 本端作为服务端时对外通告的数据；`None` 表示纯客户端。
    advertised: RwLock<Option<ServerData>>,
    /// 发现响应的固定目标；未设置时按协议回复请求源。
    response_target: Option<SocketAddr>,
}

impl Shared {
    fn touch_peer(&self, network_id: u64, address: SocketAddr, now: Instant) {
        let mut peers = self
            .peers
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if peers.len() >= MAX_TRACKED_NETWORKS && !peers.contains_key(&network_id) {
            peers.retain(|_, peer| now.duration_since(peer.seen) < DISCOVERY_TTL);
            // 表仍然满：淘汰最久未活跃的一条，而不是拒绝新对端。
            // 「失败关闭」会让攻击者用伪造网络 ID 灌满表之后，
            // 所有合法对端都再也无法建立连接。
            if peers.len() >= MAX_TRACKED_NETWORKS
                && let Some(oldest) = peers
                    .iter()
                    .min_by_key(|(_, peer)| peer.seen)
                    .map(|(&id, _)| id)
            {
                peers.remove(&oldest);
            }
        }
        peers.insert(network_id, Peer { address, seen: now });
    }

    fn peer_address(&self, network_id: u64) -> Option<SocketAddr> {
        self.peers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&network_id)
            .map(|peer| peer.address)
    }

    fn record_discovery(&self, network_id: u64, server_data: ServerData, now: Instant) {
        let mut discovered = self
            .discovered
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if discovered.len() >= MAX_TRACKED_NETWORKS && !discovered.contains_key(&network_id) {
            discovered.retain(|_, entry| now.duration_since(entry.seen) < DISCOVERY_TTL);
            if discovered.len() >= MAX_TRACKED_NETWORKS {
                return;
            }
        }
        discovered.insert(
            network_id,
            Entry {
                server_data,
                seen: now,
            },
        );
    }
}

/// 简单的按源地址限速器。
struct RateLimiter {
    window_start: Instant,
    counts: HashMap<SocketAddr, u32>,
}

impl RateLimiter {
    fn new(now: Instant) -> Self {
        Self {
            window_start: now,
            counts: HashMap::new(),
        }
    }

    fn allow(&mut self, address: SocketAddr, now: Instant) -> bool {
        if now.duration_since(self.window_start) >= RATE_WINDOW {
            self.window_start = now;
            self.counts.clear();
        }
        let count = self.counts.entry(address).or_insert(0);
        *count += 1;
        *count <= PER_ADDR_RATE
    }
}

/// 局域网信令端点。
pub struct LanSignaling {
    network_id: u64,
    socket: Arc<UdpSocket>,
    /// 客户端模式下发现请求的目标地址。
    target: Option<SocketAddr>,
    shared: Arc<Shared>,
    discovered_notify: Arc<Notify>,
    signal_tx: broadcast::Sender<Signal>,
    cancel: CancellationToken,
    /// 统计：已丢弃的非法入站报文数。
    dropped: Arc<AtomicU64>,
}

impl LanSignaling {
    /// 绑定客户端端点，发现请求发往 `target`。
    ///
    /// # Errors
    ///
    /// UDP 套接字无法绑定或无法开启广播时返回错误。
    pub async fn client(bind_addr: SocketAddr, target: SocketAddr) -> Result<Self> {
        Self::new(bind_addr, Some(target), None).await
    }

    /// 绑定服务端端点，对外通告 `server_data`。
    ///
    /// # Errors
    ///
    /// UDP 套接字无法绑定或无法开启广播时返回错误。
    pub async fn server(bind_addr: SocketAddr, server_data: ServerData) -> Result<Self> {
        Self::new(bind_addr, None, Some(server_data)).await
    }

    /// 使用已绑定的 UDP 套接字创建服务端端点。
    ///
    /// 套接字的共享、独占和平台选项由调用方在绑定前决定。
    ///
    /// # Errors
    ///
    /// 无法读取套接字地址时返回错误。启用广播失败不会阻止单播信令。
    pub async fn server_from_socket(socket: UdpSocket, server_data: ServerData) -> Result<Self> {
        Self::from_socket(socket, None, Some(server_data), None)
    }

    /// 使用已绑定的 UDP 套接字创建服务端，并把发现响应发送到固定目标。
    ///
    /// 信令消息仍发送给网络 ID 对应的请求源地址；固定目标只影响发现响应。
    ///
    /// # Errors
    ///
    /// 无法读取套接字地址时返回错误。启用广播失败不会阻止单播信令。
    pub async fn server_from_socket_with_target(
        socket: UdpSocket,
        server_data: ServerData,
        response_target: SocketAddr,
    ) -> Result<Self> {
        Self::from_socket(socket, None, Some(server_data), Some(response_target))
    }

    async fn new(
        bind_addr: SocketAddr,
        target: Option<SocketAddr>,
        server_data: Option<ServerData>,
    ) -> Result<Self> {
        let socket = UdpSocket::bind(bind_addr).await?;
        Self::from_socket(socket, target, server_data, None)
    }

    fn from_socket(
        socket: UdpSocket,
        target: Option<SocketAddr>,
        server_data: Option<ServerData>,
        response_target: Option<SocketAddr>,
    ) -> Result<Self> {
        let is_server = server_data.is_some();
        let socket = Arc::new(socket);
        // 广播失败不致命：回环或受限环境下仍可单播工作。
        if let Err(error) = socket.set_broadcast(true) {
            tracing::debug!("启用 UDP 广播失败：{error}");
        }
        let shared = Arc::new(Shared {
            response_target,
            ..Shared::default()
        });
        *shared
            .advertised
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = server_data;

        let network_id = rand::rng().random::<u64>();
        let (signal_tx, _) = broadcast::channel(SIGNAL_BROADCAST_CAPACITY);
        let discovered_notify = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let dropped = Arc::new(AtomicU64::new(0));
        let local_addr = socket.local_addr()?;
        tracing::info!(
            %local_addr,
            ?target,
            network_id,
            mode = if is_server { "server" } else { "client" },
            "NetherNet LAN 信令端点已启动"
        );

        tokio::spawn(receive_loop(
            Arc::clone(&socket),
            network_id,
            Arc::clone(&shared),
            Arc::clone(&discovered_notify),
            signal_tx.clone(),
            cancel.clone(),
            Arc::clone(&dropped),
        ));

        Ok(Self {
            network_id,
            socket,
            target,
            shared,
            discovered_notify,
            signal_tx,
            cancel,
            dropped,
        })
    }

    /// 本端网络 ID。
    #[must_use]
    pub const fn network_id(&self) -> u64 {
        self.network_id
    }

    /// 操作系统分配的本地地址。
    ///
    /// # Errors
    ///
    /// 套接字地址不可用时返回错误。
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }

    /// 已丢弃的非法入站报文数。
    #[must_use]
    pub fn dropped_packets(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// 更新对外通告的世界数据（服务端模式）。
    pub fn set_server_data(&self, server_data: ServerData) {
        *self
            .shared
            .advertised
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(server_data);
    }

    /// 当前已发现且未过期的全部世界。
    #[must_use]
    pub fn discovered(&self) -> Vec<DiscoveredServer> {
        let now = Instant::now();
        let peers = self
            .shared
            .peers
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.shared
            .discovered
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|(_, entry)| now.duration_since(entry.seen) < DISCOVERY_TTL)
            .filter_map(|(&network_id, entry)| {
                peers.get(&network_id).map(|peer| DiscoveredServer {
                    network_id,
                    address: peer.address,
                    server_data: entry.server_data.clone(),
                })
            })
            .collect()
    }

    /// 主动广播一次发现请求。
    ///
    /// # Errors
    ///
    /// 本端为服务端模式（无目标地址）或发送失败时返回错误。
    pub async fn probe(&self) -> Result<()> {
        let target = self
            .target
            .ok_or_else(|| NethernetError::protocol("服务端信令不能主动发现世界"))?;
        let request = DiscoveryPacket::Request.encode(self.network_id)?;
        self.socket.send_to(&request, target).await?;
        Ok(())
    }

    /// 在 `timeout` 内发现第一个局域网世界。
    ///
    /// 多个世界同时存在时返回网络 ID 最小的一个，保证结果稳定可复现。
    ///
    /// # Errors
    ///
    /// 超时未发现或 UDP 传输失败时返回错误。
    pub async fn discover(&self, timeout: Duration) -> Result<DiscoveredServer> {
        let deadline = Instant::now() + timeout;
        loop {
            // 先注册等待，再发请求，避免应答早于等待造成的丢唤醒。
            let notified = self.discovered_notify.notified();
            self.probe().await?;
            if let Some(server) = self.first_discovered() {
                return Ok(server);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(NethernetError::Timeout);
            }
            let wait = DISCOVERY_RETRY_INTERVAL.min(deadline.duration_since(now));
            if tokio::time::timeout(wait, notified).await.is_ok()
                && let Some(server) = self.first_discovered()
            {
                return Ok(server);
            }
        }
    }

    fn first_discovered(&self) -> Option<DiscoveredServer> {
        self.discovered()
            .into_iter()
            .min_by_key(|server| server.network_id)
    }

    /// 发送一条信令。
    ///
    /// # Errors
    ///
    /// 目标网络地址未知、信令过长或 UDP 发送失败时返回错误。
    pub async fn signal(&self, signal: Signal) -> Result<()> {
        let destination = self.shared.peer_address(signal.network_id).ok_or_else(|| {
            NethernetError::protocol(format!("未知的对端网络 {}", signal.network_id))
        })?;
        let text = signal.to_string();
        if text.len() > MAX_SIGNAL_SIZE {
            return Err(NethernetError::TooLarge {
                size: text.len(),
                max: MAX_SIGNAL_SIZE,
            });
        }
        let packet = DiscoveryPacket::Message {
            recipient_id: signal.network_id,
            data: text,
        }
        .encode(self.network_id)?;
        self.socket.send_to(&packet, destination).await?;
        Ok(())
    }

    /// 订阅入站信令。
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.signal_tx.subscribe()
    }
}

impl Signaling for LanSignaling {
    fn network_id(&self) -> u64 {
        self.network_id
    }

    async fn send_signal(&self, signal: Signal) -> Result<()> {
        self.signal(signal).await
    }

    fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.signal_tx.subscribe()
    }
}

impl Drop for LanSignaling {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

#[allow(clippy::too_many_arguments)]
async fn receive_loop(
    socket: Arc<UdpSocket>,
    own_network_id: u64,
    shared: Arc<Shared>,
    discovered_notify: Arc<Notify>,
    signal_tx: broadcast::Sender<Signal>,
    cancel: CancellationToken,
    dropped: Arc<AtomicU64>,
) {
    let mut buffer = vec![0_u8; MAX_DISCOVERY_PACKET];
    let mut limiter = RateLimiter::new(Instant::now());
    loop {
        let received = tokio::select! {
            () = cancel.cancelled() => break,
            received = socket.recv_from(&mut buffer) => received,
        };
        let Ok((length, source)) = received else {
            // 单个数据报的接收错误（如 Windows 上的 ICMP 端口不可达）不致命。
            continue;
        };
        let now = Instant::now();
        if !limiter.allow(source, now) {
            dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        let result = handle_packet(
            &buffer[..length],
            source,
            own_network_id,
            &socket,
            &shared,
            &discovered_notify,
            &signal_tx,
            now,
        )
        .await;
        if let Err(error) = result {
            dropped.fetch_add(1, Ordering::Relaxed);
            tracing::trace!(%source, "丢弃非法发现报文：{error}");
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_packet(
    data: &[u8],
    source: SocketAddr,
    own_network_id: u64,
    socket: &UdpSocket,
    shared: &Shared,
    discovered_notify: &Notify,
    signal_tx: &broadcast::Sender<Signal>,
    now: Instant,
) -> Result<()> {
    let (packet, sender_id) = DiscoveryPacket::decode(data)?;
    if sender_id == own_network_id {
        return Ok(()); // 自己的广播回环。
    }
    shared.touch_peer(sender_id, source, now);

    match packet {
        DiscoveryPacket::Request => {
            let advertised = shared
                .advertised
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            let Some(server_data) = advertised else {
                return Ok(()); // 纯客户端不应答。
            };
            let response_destination = shared.response_target.unwrap_or(source);
            let responses = encode_server_responses(&server_data, own_network_id)?;
            for response in &responses {
                socket.send_to(response, response_destination).await?;
            }
            tracing::info!(
                request_source = %source,
                %response_destination,
                remote_network_id = sender_id,
                local_network_id = own_network_id,
                server_name = %server_data.server_name,
                transport_layer = server_data.transport_layer,
                connection_type = server_data.connection_type,
                "NetherNet 已发送房间发现响应（v5 + v4）"
            );
        }
        DiscoveryPacket::Response { application_data } => {
            let server_data = ServerData::decode(application_data)?;
            tracing::info!(
                %source,
                remote_network_id = sender_id,
                server_name = %server_data.server_name,
                level_name = %server_data.level_name,
                transport_layer = server_data.transport_layer,
                connection_type = server_data.connection_type,
                "发现 NetherNet 局域网世界"
            );
            shared.record_discovery(sender_id, server_data, now);
            discovered_notify.notify_waiters();
        }
        DiscoveryPacket::Message { recipient_id, data } => {
            if recipient_id != own_network_id {
                return Ok(()); // 不是发给我们的。
            }
            // vanilla 会周期性发送 Data 为 "Ping"（或空串）的 MessagePacket，
            // 它不是信令。不白名单掉的话每个 ping 都会被计入「非法报文」，
            // 让 dropped_packets() 这个诊断指标失真、日志被刷屏。
            if data == "Ping" || data.is_empty() {
                return Ok(());
            }
            let signal = Signal::parse(&data, sender_id)?;
            if matches!(
                signal.kind,
                SignalType::Offer | SignalType::Answer | SignalType::Error
            ) {
                tracing::info!(
                    %source,
                    remote_network_id = sender_id,
                    connection_id = signal.connection_id,
                    signal_type = ?signal.kind,
                    "收到 NetherNet 协商信令"
                );
            }
            // 没有订阅者不是错误：可能尚未 accept。
            if signal_tx.send(signal).is_err() {
                tracing::debug!(
                    remote_network_id = sender_id,
                    "NetherNet 信令暂时没有订阅者"
                );
            }
        }
    }
    Ok(())
}

fn encode_server_responses(server_data: &ServerData, network_id: u64) -> Result<[bytes::Bytes; 2]> {
    let v5 = DiscoveryPacket::Response {
        application_data: server_data.encode()?,
    }
    .encode(network_id)?;
    let v4 = DiscoveryPacket::Response {
        application_data: server_data.encode_v4()?,
    }
    .encode(network_id)?;
    Ok([v5, v4])
}

/// 默认的局域网发现广播地址。
#[must_use]
pub fn broadcast_addr() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::BROADCAST, LAN_DISCOVERY_PORT))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn client_and_server_discover_each_other() {
        let server_data = ServerData {
            server_name: "Host".to_string(),
            level_name: "World".to_string(),
            ..ServerData::default()
        };
        let server = LanSignaling::server(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            server_data.clone(),
        )
        .await
        .unwrap();
        let server_addr = server.local_addr().unwrap();
        let client = LanSignaling::client(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), server_addr)
            .await
            .unwrap();

        let discovered = client.discover(Duration::from_secs(5)).await.unwrap();
        assert_eq!(discovered.server_data, server_data);
        assert_eq!(discovered.network_id, server.network_id());
        assert_eq!(discovered.address, server_addr);
    }

    #[tokio::test]
    async fn server_cannot_probe() {
        let server = LanSignaling::server(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            ServerData::default(),
        )
        .await
        .unwrap();
        assert!(server.probe().await.is_err());
    }

    #[test]
    fn server_responses_use_reference_v5_and_v4_payloads() {
        let encoded = encode_server_responses(&ServerData::default(), 42).unwrap();
        for (response, version) in encoded.iter().zip([5, 4]) {
            let (packet, sender) = DiscoveryPacket::decode(response).unwrap();
            let DiscoveryPacket::Response { application_data } = packet else {
                panic!("服务端应发送发现响应");
            };

            assert_eq!(sender, 42);
            assert_eq!(application_data.first(), Some(&version));
        }
    }

    #[tokio::test]
    async fn signal_round_trips_between_endpoints() {
        let server = LanSignaling::server(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            ServerData::default(),
        )
        .await
        .unwrap();
        let server_addr = server.local_addr().unwrap();
        let client = LanSignaling::client(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), server_addr)
            .await
            .unwrap();
        let discovered = client.discover(Duration::from_secs(5)).await.unwrap();

        let mut inbound = server.subscribe();
        client
            .signal(Signal::offer(
                77,
                "v=0\r\na=ice-options:trickle\r\n".to_string(),
                discovered.network_id,
            ))
            .await
            .unwrap();
        let signal = tokio::time::timeout(Duration::from_secs(5), inbound.recv())
            .await
            .expect("等待信令超时")
            .unwrap();
        assert_eq!(signal.connection_id, 77);
        assert_eq!(signal.network_id, client.network_id());
        assert!(signal.data.contains("ice-options"));

        // 服务端可以回信（地址簿记已在收到信令时建立）。
        let mut client_inbound = client.subscribe();
        server
            .signal(Signal::answer(77, "v=0\r\n".to_string(), signal.network_id))
            .await
            .unwrap();
        let answer = tokio::time::timeout(Duration::from_secs(5), client_inbound.recv())
            .await
            .expect("等待应答超时")
            .unwrap();
        assert_eq!(answer.kind, crate::protocol::SignalType::Answer);
    }

    #[tokio::test]
    async fn signal_to_unknown_network_fails() {
        let client = LanSignaling::client(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        )
        .await
        .unwrap();
        assert!(
            client
                .signal(Signal::offer(1, "sdp".to_string(), 12345))
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn discovery_times_out_without_server() {
        let client = LanSignaling::client(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            SocketAddr::from((Ipv4Addr::LOCALHOST, 1)),
        )
        .await
        .unwrap();
        let result = client.discover(Duration::from_millis(600)).await;
        assert!(matches!(result, Err(NethernetError::Timeout)));
    }

    #[test]
    fn rate_limiter_caps_per_address() {
        let now = Instant::now();
        let mut limiter = RateLimiter::new(now);
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
        for _ in 0..PER_ADDR_RATE {
            assert!(limiter.allow(addr, now));
        }
        assert!(!limiter.allow(addr, now), "超出窗口配额应被拒绝");
        // 新窗口后恢复。
        assert!(limiter.allow(addr, now + RATE_WINDOW));
    }
}
