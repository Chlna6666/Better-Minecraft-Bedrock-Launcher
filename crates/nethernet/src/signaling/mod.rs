//! 信令抽象与局域网实现。
//!
//! [`Signaling`] 把「如何把 [`Signal`] 送到对端」与协商流程解耦：
//! 目前只有 [`LanSignaling`]（UDP 广播发现），若将来要接入
//! Xbox Live WebSocket 信令，只需再实现一次该 trait。

pub mod lan;

pub use lan::{DiscoveredServer, LanSignaling};

use crate::error::Result;
use crate::protocol::Signal;
use tokio::sync::broadcast;

/// 信令通道。
pub trait Signaling: Send + Sync + 'static {
    /// 本端网络 ID。
    fn network_id(&self) -> u64;

    /// 把一条信令发往 `signal.network_id` 指向的对端。
    fn send_signal(&self, signal: Signal) -> impl std::future::Future<Output = Result<()>> + Send;

    /// 订阅入站信令。每个订阅者独立收到全部信令（广播而非负载均衡）。
    fn subscribe(&self) -> broadcast::Receiver<Signal>;
}
