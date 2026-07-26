//! 错误类型。

use thiserror::Error;

/// 编解码错误。
#[derive(Debug, Error)]
pub enum RakCodecError {
    #[error("数据截断：需要 {needed} 字节，剩余 {remaining}")]
    Truncated { needed: usize, remaining: usize },
    #[error("报文 ID 不符：期望 {expected:#04X}，实际 {found:#04X}")]
    UnexpectedPacketId { expected: u8, found: u8 },
    #[error("离线魔数不符")]
    BadMagic,
    #[error("报文格式非法：{0}")]
    Malformed(&'static str),
}

/// 会话错误。
#[derive(Debug, Error)]
pub enum RakSessionError {
    #[error("会话已关闭")]
    Closed,
    #[error("消息过大：{size} 字节，上限 {max}")]
    TooLarge { size: usize, max: usize },
    #[error("发送队列已满（超过 {max} 字节）")]
    QueueFull { max: usize },
    #[error("编解码错误：{0}")]
    Codec(#[from] RakCodecError),
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

/// 客户端错误。
#[derive(Debug, Error)]
pub enum RakClientError {
    #[error("客户端已关闭")]
    Closed,
    #[error("连接失败：{attempts} 次尝试均无响应")]
    ConnectionFailed { attempts: usize },
    #[error("连接超时")]
    Timeout,
    #[error("协议版本不兼容：服务端要求 {server_protocol}")]
    IncompatibleProtocol { server_protocol: u8 },
    #[error("该地址已存在连接")]
    AlreadyConnected,
    #[error("服务端连接数已满")]
    NoFreeIncomingConnections,
    #[error("该 IP 近期连接过，被服务端暂时拒绝")]
    RecentlyConnected,
    #[error("连接请求被服务端拒绝")]
    ConnectionRequestFailed,
    #[error("服务端要求加密握手，不支持")]
    SecurityUnsupported,
    #[error("编解码错误：{0}")]
    Codec(#[from] RakCodecError),
    #[error("会话错误：{0}")]
    Session(#[from] RakSessionError),
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}

/// 服务端错误。
#[derive(Debug, Error)]
pub enum RakServerError {
    #[error("服务端已关闭")]
    Closed,
    #[error("编解码错误：{0}")]
    Codec(#[from] RakCodecError),
    #[error("IO 错误：{0}")]
    Io(#[from] std::io::Error),
}
