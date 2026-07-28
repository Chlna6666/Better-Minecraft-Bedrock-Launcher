//! 协议常量。

use std::time::Duration;

/// 局域网发现的默认 UDP 端口（vanilla 客户端在此广播 `RequestPacket`）。
pub const LAN_DISCOVERY_PORT: u16 = 7551;

/// 发现报文 ID。
pub const ID_REQUEST_PACKET: u16 = 0;
pub const ID_RESPONSE_PACKET: u16 = 1;
pub const ID_MESSAGE_PACKET: u16 = 2;

/// 发现报文头：packetID(2) + senderID(8) + 保留填充(8)。
pub const HEADER_SIZE: usize = 18;
/// HMAC-SHA256 校验字段长度。
pub const CHECKSUM_SIZE: usize = 32;
/// 长度前缀为 u16，因此单个发现报文的明文载荷不超过该值。
pub const MAX_PAYLOAD_LENGTH: usize = u16::MAX as usize;
/// 入站发现报文的接收上限（校验和 + 密文，密文含 PKCS7 填充最多 16 字节）。
pub const MAX_DISCOVERY_PACKET: usize = CHECKSUM_SIZE + MAX_PAYLOAD_LENGTH + 16;
/// 单条信令文本的长度上限。
///
/// 取 u16 的上界与 go-nethernet 的 `maxPacketPayloadLength` 一致：
/// 更小的自定义上限会丢掉对端认为合法的信令（长 SDP 很容易接近该值）。
pub const MAX_SIGNAL_SIZE: usize = u16::MAX as usize;

/// 应用 ID：发现层加密密钥由 `SHA256(u64le(APPLICATION_ID))` 派生。
pub const APPLICATION_ID: u64 = 0xdead_beef;

/// 数据通道标签。
pub const RELIABLE_CHANNEL: &str = "ReliableDataChannel";
pub const UNRELIABLE_CHANNEL: &str = "UnreliableDataChannel";

/// 单个数据通道消息的最大载荷：256 KiB 减去 1 字节分片计数。
///
/// 与 vanilla 对等端 SDP 中的 `a=max-message-size:262144` 对应。
pub const MAX_SEGMENT_PAYLOAD: usize = 262_143;
/// SDP 中通告的 SCTP 单消息上限（= [`MAX_SEGMENT_PAYLOAD`] + 1）。
pub const SCTP_MAX_MESSAGE_SIZE: u32 = 262_144;
/// 分片计数为 u8，因此一条消息最多 256 片。
pub const MAX_SEGMENTS: usize = u8::MAX as usize + 1;
/// 单条完整消息的字节上限。
pub const MAX_MESSAGE_SIZE: usize = MAX_SEGMENT_PAYLOAD * MAX_SEGMENTS;

/// SDP 中的 SCTP 端口（vanilla 固定 5000）。
pub const SCTP_PORT: u16 = 5000;

/// WebRTC 协商总超时。
pub const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(15);
/// 发现条目的存活时间：超过该时长未刷新即视为下线。
pub const DISCOVERY_TTL: Duration = Duration::from_secs(15);
/// 发现请求的重发间隔。
pub const DISCOVERY_RETRY_INTERVAL: Duration = Duration::from_millis(250);
