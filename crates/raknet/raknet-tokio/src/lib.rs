//! 自研 RakNet 的 tokio 异步驱动。
//!
//! 架构参考 go-raknet：
//! - 每个套接字一个专职接收任务，按对端地址路由数据报；
//! - 每个会话一个轻量 tick 任务（ACK 冲刷 / 重传 / keep-alive）；
//! - 发送路径无 actor 往返：调用方短暂持锁打包后直接写套接字；
//! - 全链路 `bytes::Bytes` 零拷贝。

mod client;
mod net;
mod server;
mod session;

pub mod prelude {
    pub use crate::client::RakClient;
    pub use crate::server::{RakServer, RakServerError};
    pub use crate::session::{RakReceiver, RakSendHandle, RakSession};
    pub use raknet::prelude::{
        RakClientConfig, RakClientError, RakPriority, RakReliability, RakServerConfig,
        RakSessionConfig, RakSessionError,
    };
}
