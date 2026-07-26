//! 客户端 / 服务端 / 会话配置。
//!
//! 字段布局与旧版（bedrock-crustaceans vendored 版本）保持兼容，
//! 既有调用方无需改动。

use crate::consts;
use std::time::Duration;

/// 客户端配置。
#[derive(Clone, Debug)]
pub struct RakClientConfig {
    /// 客户端 GUID。生成时强制符号位为 1（i64 视角为负数）：
    /// go-raknet 监听端会拒绝非负 GUID 的 OpenConnectionRequest2
    /// （"vanilla clients always provide a negative ClientGUID"）。
    pub guid: u64,
    /// RakNet 协议版本。
    pub protocol: u8,
    pub min_mtu_size: u16,
    pub max_mtu_size: u16,
    /// 整个握手流程的总超时。
    pub conn_attempt_timeout: Duration,
    /// 相邻两次握手尝试的间隔。
    pub conn_attempt_interval: Duration,
    /// 握手尝试次数上限。
    pub conn_attempt_max: usize,
}

impl Default for RakClientConfig {
    fn default() -> Self {
        Self {
            guid: rand::random::<u64>() | (1 << 63),
            protocol: consts::PROTOCOL,
            min_mtu_size: consts::MIN_MTU_SIZE,
            max_mtu_size: consts::MAX_MTU_SIZE,
            conn_attempt_timeout: consts::CONNECTION_ATTEMPT_TIMEOUT,
            conn_attempt_interval: consts::CONNECTION_ATTEMPT_INTERVAL,
            conn_attempt_max: consts::CONNECTION_ATTEMPT_MAX,
        }
    }
}

/// 服务端配置。
#[derive(Clone, Debug)]
pub struct RakServerConfig {
    pub max_ordering_channels: i32,
    pub guid: u64,
    /// 接受的协议版本列表。
    pub protocols: Box<[u8]>,
    pub max_connections: usize,
    /// Unconnected Pong 返回的 MOTD 内容。
    pub message: Box<[u8]>,
    pub min_mtu_size: u16,
    pub max_mtu_size: u16,
    /// 离线/握手报文限速：单地址每 10ms 窗口的报文数（<=0 表示不限）。
    pub packet_limit: i32,
    /// 离线/握手报文限速：全局每 10ms 窗口的报文数（<=0 表示不限）。
    pub total_packet_limit: i32,
    /// 开启后 OpenConnectionReply1 携带 cookie，
    /// OpenConnectionRequest2 必须回显（go-raknet v1.14+ 的默认防护）。
    pub security: bool,
}

impl Default for RakServerConfig {
    fn default() -> Self {
        Self {
            max_ordering_channels: consts::MAX_ORDERING_CHANNELS as i32,
            guid: rand::random::<u64>(),
            protocols: Box::new([consts::PROTOCOL]),
            max_connections: 10,
            message: Box::new([]),
            min_mtu_size: consts::MIN_MTU_SIZE,
            max_mtu_size: consts::MAX_MTU_SIZE,
            packet_limit: consts::PACKET_LIMIT,
            total_packet_limit: consts::TOTAL_PACKET_LIMIT,
            security: false,
        }
    }
}

/// 会话配置。
#[derive(Clone, Debug)]
pub struct RakSessionConfig {
    /// 有序通道数（1..=32）。
    pub ordering_channels: i32,
    /// 是否由驱动定期 tick（保留字段，raknet-tokio 恒定期 tick）。
    pub autoflush: bool,
    /// tick 间隔（ACK 冲刷 / 重传检查）。
    pub autoflush_interval_ms: Duration,
    /// 发送队列字节上限。
    pub max_queued_bytes: i32,
    /// 静默超时。
    pub session_timeout: Duration,
    /// keep-alive ping 间隔。
    pub ping_interval: Duration,
}

impl Default for RakSessionConfig {
    fn default() -> Self {
        Self {
            ordering_channels: consts::MAX_ORDERING_CHANNELS as i32,
            autoflush: true,
            autoflush_interval_ms: consts::TICK_INTERVAL,
            max_queued_bytes: consts::MAX_QUEUED_BYTES as i32,
            session_timeout: consts::SESSION_TIMEOUT,
            ping_interval: consts::PING_INTERVAL,
        }
    }
}
