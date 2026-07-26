//! Minecraft 基岩版 `NetherNet` 传输的 Rust 实现。
//!
//! `NetherNet` 是基岩版较新版本使用的、基于 WebRTC 的点对点协议：
//! 局域网上先用 UDP 7551 端口做世界发现与信令交换，再在 WebRTC
//! 的两条数据通道（`ReliableDataChannel` / `UnreliableDataChannel`）
//! 上收发游戏报文。
//!
//! # 分层
//!
//! - [`protocol`]：纯线格式编解码，无 IO。发现报文（AES-256-ECB +
//!   HMAC-SHA256）、[`ServerData`]、[`Signal`] 与消息分片。
//! - [`signaling`]：[`Signaling`] 抽象与局域网实现 [`LanSignaling`]。
//! - [`session`]：数据通道之上的消息收发。
//! - [`transport`]：WebRTC 协商，对外提供 [`NethernetListener`]
//!   与 [`NethernetStream`]。
//!
//! # 零拷贝
//!
//! 全链路使用 `bytes::Bytes`：入站数据报解密后一次成型，帧载荷是其
//! 切片视图；单片消息直达上层不经过重组缓冲；出站分片用 `Bytes::slice`
//! 切分，仅在拼接分片头时拷贝。
//!
//! # 互通
//!
//! 线格式对齐 go-nethernet 与 vanilla：
//! - 发现报文声明长度包含其自身的 2 字节前缀；
//! - `ServerData` v5 逐字节一致，并兼容旧版 v4；
//! - SDP 中通告 `a=max-message-size:262144`，且把 webrtc-rs 默认
//!   64 KiB 的 SCTP 单消息上限放开到同一数值——否则任何超过 64 KiB
//!   的游戏报文都会被底层拒发。

pub mod consts;
pub mod error;
pub mod protocol;
pub mod session;
pub mod signaling;
pub mod transport;

pub use consts::{
    LAN_DISCOVERY_PORT, MAX_MESSAGE_SIZE, MAX_SEGMENT_PAYLOAD, RELIABLE_CHANNEL, UNRELIABLE_CHANNEL,
};
pub use error::{NethernetError, Result, SignalErrorCode};
pub use protocol::{DiscoveryPacket, ServerData, Signal, SignalType};
pub use session::{NethernetSession, SessionStats};
pub use signaling::{DiscoveredServer, LanSignaling, Signaling};
pub use transport::{NethernetListener, NethernetStream};
