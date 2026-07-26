//! 套接字工具。

use socket2::{Domain, Protocol, Socket, Type};
use std::net::SocketAddr;
use tokio::net::UdpSocket;

/// 大流量场景的套接字缓冲：Windows 默认 64KB 在突发下会丢包，
/// 直接压制拥塞窗口。
const SOCKET_BUFFER_SIZE: usize = 4 * 1024 * 1024;

/// 绑定 UDP 套接字并放大收发缓冲。
pub(crate) fn bind_udp(addr: SocketAddr) -> std::io::Result<UdpSocket> {
    let domain = if addr.is_ipv4() { Domain::IPV4 } else { Domain::IPV6 };
    let socket = Socket::new(domain, Type::DGRAM, Some(Protocol::UDP))?;
    // 缓冲设置失败不致命（受系统上限约束），尽力而为。
    let _ = socket.set_recv_buffer_size(SOCKET_BUFFER_SIZE);
    let _ = socket.set_send_buffer_size(SOCKET_BUFFER_SIZE);
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    UdpSocket::from_std(socket.into())
}
