//! 自研 RakNet 协议核心。
//!
//! 设计参考 go-raknet 的连接模型：本 crate 只包含协议编解码与可靠传输
//! 状态机（无任何 IO / 运行时依赖），由 `raknet-tokio` 驱动真实的 UDP
//! 套接字与定时器。
//!
//! 关键特性：
//! - 全链路 `bytes::Bytes` 零拷贝：入站数据报切片直达用户，出站载荷
//!   仅在合帧进数据报时拷贝一次；
//! - 完整可靠层：拆分/重组、有序/序列通道、可靠帧去重窗口、
//!   ACK/NACK、RTO 重传与拥塞窗口；
//! - u24 线上序号在内部展开为 u64，长连接不受回绕影响；
//! - 面向不可信输入的防护上限（拆分重组、乱序缓冲、ACK 范围）。

pub mod config;
pub mod consts;
pub mod error;
pub mod reliability;
pub mod types;
pub mod wire;

pub mod prelude {
    pub use crate::config::{RakClientConfig, RakServerConfig, RakSessionConfig};
    pub use crate::error::{RakClientError, RakCodecError, RakServerError, RakSessionError};
    pub use crate::types::{RakPriority, RakReliability};
}
