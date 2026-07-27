use bedrock_nethernet::{LanSignaling, NethernetError, NethernetListener, ServerData};
use raknet_tokio::prelude::{RakClient, RakSession};
use std::net::SocketAddr;
use std::time::Duration;

#[cfg(windows)]
fn probe_discovery_port(address: SocketAddr) -> std::io::Result<()> {
    use std::net::Ipv6Addr;

    probe_exclusive_bind(address)?;
    if address.is_ipv4() {
        probe_exclusive_bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, address.port())))?;
    }
    Ok(())
}

#[cfg(windows)]
fn probe_exclusive_bind(address: SocketAddr) -> std::io::Result<()> {
    use socket2::{Domain, Protocol, Socket, Type};
    use std::os::windows::io::AsRawSocket as _;
    use windows::Win32::Networking::WinSock::{
        SO_EXCLUSIVEADDRUSE, SOCKET, SOCKET_ERROR, SOL_SOCKET, setsockopt,
    };

    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    if address.is_ipv6() {
        socket.set_only_v6(false)?;
    }
    let raw_socket = usize::try_from(socket.as_raw_socket())
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "无效的 UDP socket"))?;
    let enabled = 1_i32.to_ne_bytes();
    // SAFETY: `socket` owns a live WinSock handle and `enabled` remains valid
    // for the duration of this synchronous option-setting call.
    let result = unsafe {
        setsockopt(
            SOCKET(raw_socket),
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            Some(&enabled),
        )
    };
    if result == SOCKET_ERROR {
        return Err(std::io::Error::last_os_error());
    }
    socket.bind(&address.into())
}

#[cfg(not(windows))]
fn probe_discovery_port(_address: SocketAddr) -> std::io::Result<()> {
    Ok(())
}

#[cfg(windows)]
async fn bind_discovery_socket(address: SocketAddr) -> std::io::Result<tokio::net::UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = Socket::new(
        Domain::for_address(address),
        Type::DGRAM,
        Some(Protocol::UDP),
    )?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&address.into())?;
    tokio::net::UdpSocket::from_std(socket.into())
}

#[cfg(not(windows))]
async fn bind_discovery_socket(address: SocketAddr) -> std::io::Result<tokio::net::UdpSocket> {
    tokio::net::UdpSocket::bind(address).await
}

#[cfg(windows)]
async fn create_guest_signaling(
    socket: tokio::net::UdpSocket,
    server_data: ServerData,
    discovery_port: u16,
) -> bedrock_nethernet::Result<LanSignaling> {
    LanSignaling::server_from_socket_with_target(
        socket,
        server_data,
        SocketAddr::from((std::net::Ipv4Addr::BROADCAST, discovery_port)),
    )
    .await
}

#[cfg(not(windows))]
async fn create_guest_signaling(
    socket: tokio::net::UdpSocket,
    server_data: ServerData,
    _discovery_port: u16,
) -> bedrock_nethernet::Result<LanSignaling> {
    LanSignaling::server_from_socket(socket, server_data).await
}

pub(super) async fn connect_raknet(
    proxy_port: u16,
    max_mtu_size: u16,
    timeout: Duration,
) -> Result<(RakClient, RakSession), String> {
    let target = SocketAddr::from(([127, 0, 0, 1], proxy_port));
    tracing::info!(%target, timeout_secs = timeout.as_secs(), "正在连接房主 RakNet 隧道");
    let mut client = RakClient::new(|config| {
        config.max_mtu_size = max_mtu_size;
    });
    if let Err(error) = client.start().await {
        client.stop();
        return Err(format!("启动 PaperConnect RakNet 客户端失败：{error}"));
    }
    let session = match tokio::time::timeout(timeout, client.connect(target)).await {
        Ok(Ok(session)) => session,
        Ok(Err(error)) => {
            client.stop();
            return Err(format!("连接房主 RakNet 隧道失败：{error}"));
        }
        Err(_) => {
            client.stop();
            return Err("连接房主 RakNet 隧道超时".to_string());
        }
    };
    tracing::info!(%target, "房主 RakNet 隧道连接成功");
    Ok((client, session))
}

pub(super) async fn bind_listener(
    discovery_addr: SocketAddr,
    server_data: ServerData,
    proxy_port: u16,
) -> bedrock_nethernet::Result<NethernetListener> {
    probe_discovery_port(discovery_addr)?;
    tracing::info!(
        port = discovery_addr.port(),
        "PaperConnect 成员步骤 4/6：UDP 7551 占用检测通过"
    );
    let socket = bind_discovery_socket(discovery_addr).await?;
    let signaling = create_guest_signaling(socket, server_data, discovery_addr.port()).await?;
    let listener = NethernetListener::bind(signaling, SocketAddr::from(([127, 0, 0, 1], 0)))?;
    tracing::info!(
        %discovery_addr,
        proxy_port,
        "PaperConnect 成员步骤 5/6：本机 UDP 7551 模拟代理创建成功"
    );
    Ok(listener)
}

pub(super) fn is_port_occupied(error: &NethernetError) -> bool {
    matches!(
        error,
        NethernetError::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied
            )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, UdpSocket};

    #[test]
    fn address_conflicts_are_classified_as_occupied() {
        for kind in [
            std::io::ErrorKind::AddrInUse,
            std::io::ErrorKind::PermissionDenied,
        ] {
            let error = NethernetError::Io(std::io::Error::from(kind));
            assert!(is_port_occupied(&error), "未识别端口冲突：{kind:?}");
        }
    }

    #[tokio::test]
    async fn listener_reports_an_existing_udp_binding_without_waiting() {
        let occupant =
            UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).expect("应占用临时端口");
        let port = occupant.local_addr().expect("应读取占用地址").port();
        let discovery_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));

        let result = tokio::time::timeout(
            Duration::from_secs(1),
            bind_listener(discovery_addr, ServerData::default(), 23000),
        )
        .await
        .expect("端口占用应立即返回");
        let error = match result {
            Ok(_) => panic!("重复绑定必须失败"),
            Err(error) => error,
        };

        assert!(is_port_occupied(&error), "应识别真实 UDP 绑定冲突：{error}");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn listener_rejects_reusable_windows_udp_occupants() {
        use socket2::{Domain, Protocol, Socket, Type};

        let occupant =
            Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).expect("应创建占用 socket");
        occupant
            .set_reuse_address(true)
            .expect("应允许模拟 Minecraft 的共享绑定");
        occupant
            .bind(&SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)).into())
            .expect("应占用临时端口");
        let address = occupant
            .local_addr()
            .expect("应读取占用地址")
            .as_socket()
            .expect("应为 IP socket 地址");
        let shared =
            Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).expect("应创建共享 socket");
        shared.set_reuse_address(true).expect("应设置共享地址");
        shared
            .bind(&address.into())
            .expect("普通共享 socket 应可重复绑定");

        let error = match bind_listener(address, ServerData::default(), 23000).await {
            Ok(_) => panic!("独占探针必须识别已有的共享 UDP socket"),
            Err(error) => error,
        };

        assert!(
            is_port_occupied(&error),
            "应识别 Minecraft 式共享占用：{error}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn listener_rejects_dual_stack_ipv6_windows_udp_occupants() {
        use socket2::{Domain, Protocol, Socket, Type};
        use std::net::Ipv6Addr;

        let occupant =
            Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP)).expect("应创建双栈 socket");
        occupant
            .set_only_v6(false)
            .expect("应允许模拟 Minecraft 的双栈监听");
        occupant
            .bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)).into())
            .expect("应占用 IPv6 通配端口");
        let port = occupant
            .local_addr()
            .expect("应读取占用地址")
            .as_socket()
            .expect("应为 IP socket 地址")
            .port();

        let error = match bind_listener(
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
            ServerData::default(),
            23000,
        )
        .await
        {
            Ok(_) => panic!("IPv6 双栈占用时必须拒绝 IPv4 7551 模拟代理"),
            Err(error) => error,
        };

        assert!(
            is_port_occupied(&error),
            "应识别 Minecraft 的 IPv6 双栈占用：{error}"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn minecraft_style_shared_socket_receives_discovery_response() {
        use bedrock_nethernet::DiscoveryPacket;
        use socket2::{Domain, Protocol, Socket, Type};

        let reservation =
            UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))).expect("应预留测试端口");
        let port = reservation.local_addr().expect("应读取测试端口").port();
        drop(reservation);

        let discovery_addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, port));
        let _server = bind_listener(discovery_addr, ServerData::default(), 23000)
            .await
            .expect("应启动模拟代理发现监听");

        let game_socket =
            Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).expect("应创建游戏 socket");
        game_socket
            .set_reuse_address(true)
            .expect("游戏 socket 应允许共享端口");
        game_socket
            .set_broadcast(true)
            .expect("游戏 socket 应允许广播");
        game_socket
            .bind(&discovery_addr.into())
            .expect("游戏 socket 应与模拟代理共享端口");
        let game_socket: UdpSocket = game_socket.into();
        game_socket
            .set_nonblocking(true)
            .expect("游戏 socket 应切换为非阻塞");
        let game_socket =
            tokio::net::UdpSocket::from_std(game_socket).expect("应接入 Tokio socket");

        let request = DiscoveryPacket::Request.encode(0x1234).unwrap();
        game_socket
            .send_to(
                &request,
                SocketAddr::from((Ipv4Addr::BROADCAST, discovery_addr.port())),
            )
            .await
            .expect("游戏应广播发现请求");

        let sender_id = tokio::time::timeout(Duration::from_secs(1), async {
            let mut response = [0_u8; 2048];
            loop {
                let (length, _) = game_socket
                    .recv_from(&mut response)
                    .await
                    .expect("游戏 socket 接收发现数据报失败");
                let (packet, sender_id) = DiscoveryPacket::decode(&response[..length])
                    .expect("游戏 socket 收到的发现数据报应有效");
                if matches!(packet, DiscoveryPacket::Response { .. }) {
                    break sender_id;
                }
            }
        })
        .await
        .expect("游戏 socket 未收到模拟代理的发现响应");

        assert_ne!(sender_id, 0x1234);
    }
}
