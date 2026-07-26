//! 协议常量：报文 ID、标志位、尺寸与默认参数。

use std::time::Duration;

/// RakNet 协议版本（当前基岩版使用 11）。
pub const PROTOCOL: u8 = 11;

/// 离线报文魔数。
pub const MAGIC: [u8; 16] = [
    0x00, 0xFF, 0xFF, 0x00, 0xFE, 0xFE, 0xFE, 0xFE, 0xFD, 0xFD, 0xFD, 0xFD, 0x12, 0x34, 0x56, 0x78,
];

// ---- 报文 ID ----
pub const ID_CONNECTED_PING: u8 = 0x00;
pub const ID_UNCONNECTED_PING: u8 = 0x01;
pub const ID_UNCONNECTED_PING_OPEN_CONNECTIONS: u8 = 0x02;
pub const ID_CONNECTED_PONG: u8 = 0x03;
pub const ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
pub const ID_OPEN_CONNECTION_REPLY_1: u8 = 0x06;
pub const ID_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;
pub const ID_OPEN_CONNECTION_REPLY_2: u8 = 0x08;
pub const ID_CONNECTION_REQUEST: u8 = 0x09;
pub const ID_CONNECTION_REQUEST_ACCEPTED: u8 = 0x10;
pub const ID_CONNECTION_REQUEST_FAILED: u8 = 0x11;
pub const ID_ALREADY_CONNECTED: u8 = 0x12;
pub const ID_NEW_INCOMING_CONNECTION: u8 = 0x13;
pub const ID_NO_FREE_INCOMING_CONNECTIONS: u8 = 0x14;
pub const ID_DISCONNECT: u8 = 0x15;
pub const ID_INCOMPATIBLE_PROTOCOL: u8 = 0x19;
pub const ID_IP_RECENTLY_CONNECTED: u8 = 0x1A;
pub const ID_UNCONNECTED_PONG: u8 = 0x1C;

// ---- 数据报标志位（首字节） ----
pub const FLAG_VALID: u8 = 0x80;
pub const FLAG_ACK: u8 = 0x40;
pub const FLAG_NACK: u8 = 0x20;
pub const FLAG_PAIR: u8 = 0x10;
pub const FLAG_CONTINUOUS_SEND: u8 = 0x08;
pub const FLAG_NEEDS_B_AND_AS: u8 = 0x04;
/// 帧头中的拆分标记。
pub const FLAG_SPLIT: u8 = 0x10;

// ---- 尺寸 ----
/// UDP 头大小。
pub const UDP_HEADER_SIZE: u16 = 8;
/// 数据报头（标志位 1 + u24 序号 3）。
pub const DGRAM_HEADER_SIZE: usize = 4;
/// 帧头最大长度：flags(1) + len(2) + rel(3) + seq(3) + ord(3+1) + split(4+2+4)。
pub const FRAME_HEADER_MAX: usize = 23;
/// 允许协商的最小 MTU。
pub const MIN_MTU_SIZE: u16 = 400;
/// 允许协商的最大 MTU。
pub const MAX_MTU_SIZE: u16 = 1492;
/// 单个 UDP 数据报的解析上限（含余量）。
pub const MAX_DATAGRAM_SIZE: usize = 2048;

/// IPv4 / IPv6 的 IP 头开销。
pub fn ip_overhead(addr: &std::net::SocketAddr) -> u16 {
    match addr {
        std::net::SocketAddr::V4(_) => 20,
        std::net::SocketAddr::V6(_) => 40,
    }
}

// ---- 可靠层参数 ----
/// 有序通道数上限。
pub const MAX_ORDERING_CHANNELS: usize = 32;
/// 可靠帧去重窗口大小（以可靠序号计）。
pub const RELIABLE_WINDOW: u64 = 65536;
/// 单条消息最大拆分片数。
pub const MAX_SPLIT_PARTS: u32 = 4096;
/// 并发拆分重组的消息数上限。
pub const MAX_ACTIVE_SPLITS: usize = 64;
/// 拆分重组总缓冲字节上限。
pub const MAX_SPLIT_BYTES: usize = 32 * 1024 * 1024;
/// 单通道乱序缓冲帧数上限。
pub const MAX_ORDERED_PENDING: usize = 16384;
/// 乱序缓冲总字节上限。
pub const MAX_ORDERED_BYTES: usize = 32 * 1024 * 1024;
/// 解析 ACK/NACK 时接受的最大记录数。
pub const MAX_ACK_RECORDS: usize = 4096;
/// 单帧最多重传次数，超过判定链路死亡。
pub const MAX_RETRIES: u8 = 12;

// ---- 定时参数 ----
/// 会话 tick（ACK 冲刷 / 重传检查）间隔。
pub const TICK_INTERVAL: Duration = Duration::from_millis(10);
/// keep-alive ConnectedPing 间隔。
pub const PING_INTERVAL: Duration = Duration::from_secs(2);
/// 静默超时：超过该时长未收到对端任何数据则断开。
pub const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
/// RTO 下限 / 上限 / 初始值。
pub const RTO_MIN: Duration = Duration::from_millis(50);
pub const RTO_MAX: Duration = Duration::from_millis(2000);
pub const RTO_INITIAL: Duration = Duration::from_millis(400);

// ---- 握手默认参数 ----
pub const CONNECTION_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(10_000);
pub const CONNECTION_ATTEMPT_INTERVAL: Duration = Duration::from_millis(1_000);
pub const CONNECTION_ATTEMPT_MAX: usize = 10;

/// 默认发送队列字节上限。
pub const MAX_QUEUED_BYTES: usize = 64 * 1024 * 1024;
/// 服务端离线/握手报文限速：单地址每 10ms 窗口的报文数。
pub const PACKET_LIMIT: i32 = 120;
/// 服务端离线/握手报文限速：全局每 10ms 窗口的报文数。
pub const TOTAL_PACKET_LIMIT: i32 = 100_000;
