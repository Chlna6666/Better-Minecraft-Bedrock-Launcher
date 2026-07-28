//! 回环集成测试：真实 UDP 套接字上的完整握手与数据交换。

use bytes::Bytes;
use raknet_tokio::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

const MTU: u16 = 1200;

/// 绑定随机端口的服务端并返回实际地址。
async fn server_on_random_port(max_connections: usize) -> (RakServer, SocketAddr) {
    // 先用一个临时套接字拿到空闲端口，再让服务端绑定它。
    // （竞态概率极低，测试可接受。）
    let probe = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    let mut server = RakServer::new(addr, |config| {
        config.max_connections = max_connections;
        config.max_mtu_size = MTU;
        config.message =
            Box::from(&b"MCPE;LoopbackTest;589;1.20.0;0;20;42;World;Survival;0;19132;19132;"[..]);
    });
    server.start().await.expect("server start");
    (server, addr)
}

async fn connect_client(addr: SocketAddr) -> (RakClient, RakSession) {
    let mut client = RakClient::new(|config| {
        config.max_mtu_size = MTU;
    });
    client.start().await.expect("client start");
    let session = tokio::time::timeout(Duration::from_secs(10), client.connect(addr))
        .await
        .expect("connect 超时")
        .expect("connect 失败");
    (client, session)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_and_echo() {
    let (mut server, addr) = server_on_random_port(4).await;
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.expect("accept");
        (server, session)
    });

    let (mut client, guest) = connect_client(addr).await;
    let (server, mut host) = accept_task.await.unwrap();

    // 客户端 → 服务端。
    guest
        .send(
            &b"\xFEhello from guest"[..],
            RakReliability::ReliableOrdered,
            RakPriority::High,
        )
        .await
        .expect("guest send");
    let received: Box<[u8]> =
        tokio::time::timeout(Duration::from_secs(5), host.recv::<Box<[u8]>>())
            .await
            .expect("host recv 超时")
            .expect("host recv");
    assert_eq!(&received[..], b"\xFEhello from guest");

    // 服务端 → 客户端。
    let mut guest = guest;
    host.send(
        &b"\xFEhello from host"[..],
        RakReliability::ReliableOrdered,
        RakPriority::High,
    )
    .await
    .expect("host send");
    let received: Box<[u8]> =
        tokio::time::timeout(Duration::from_secs(5), guest.recv::<Box<[u8]>>())
            .await
            .expect("guest recv 超时")
            .expect("guest recv");
    assert_eq!(&received[..], b"\xFEhello from host");

    drop(server);
    client.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_transfer_with_splits_and_ordering() {
    let (mut server, addr) = server_on_random_port(4).await;
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.expect("accept");
        (server, session)
    });
    let (mut client, guest) = connect_client(addr).await;
    let (server, mut host) = accept_task.await.unwrap();

    // 512 条带序号的消息，其中混有超过 MTU 的大消息（触发拆分）。
    let count = 512u32;
    let sender = tokio::spawn(async move {
        for i in 0..count {
            let mut payload = vec![0xFEu8];
            payload.extend_from_slice(&i.to_be_bytes());
            if i.is_multiple_of(16) {
                payload.extend(std::iter::repeat_n(i as u8, 8000));
            }
            guest
                .send(payload, RakReliability::ReliableOrdered, RakPriority::High)
                .await
                .expect("send");
        }
        guest
    });

    let mut received = 0u32;
    while received < count {
        let packet: Box<[u8]> =
            tokio::time::timeout(Duration::from_secs(15), host.recv::<Box<[u8]>>())
                .await
                .expect("recv 超时")
                .expect("recv");
        let i = u32::from_be_bytes([packet[1], packet[2], packet[3], packet[4]]);
        assert_eq!(i, received, "有序通道必须按序交付");
        if i.is_multiple_of(16) {
            assert_eq!(packet.len(), 5 + 8000);
            assert!(packet[5..].iter().all(|&b| b == i as u8));
        }
        received += 1;
    }

    let guest = sender.await.unwrap();
    guest.close().await.ok();
    drop(server);
    client.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multiple_concurrent_sessions() {
    let (mut server, addr) = server_on_random_port(8).await;
    let server_task = tokio::spawn(async move {
        let mut sessions = Vec::new();
        for _ in 0..3 {
            let mut session = server.accept().await.expect("accept");
            sessions.push(tokio::spawn(async move {
                let packet: Box<[u8]> = session.recv().await.expect("recv");
                packet
            }));
        }
        let mut out = Vec::new();
        for handle in sessions {
            out.push(handle.await.unwrap());
        }
        (server, out)
    });

    let mut clients = Vec::new();
    for i in 0..3u8 {
        let (client, session) = connect_client(addr).await;
        session
            .send(
                vec![0xFE, i],
                RakReliability::ReliableOrdered,
                RakPriority::High,
            )
            .await
            .expect("send");
        clients.push((client, session));
    }

    let (server, received) = tokio::time::timeout(Duration::from_secs(10), server_task)
        .await
        .expect("多会话测试超时")
        .unwrap();
    let mut markers: Vec<u8> = received.iter().map(|p| p[1]).collect();
    markers.sort_unstable();
    assert_eq!(markers, vec![0, 1, 2]);

    drop(server);
    for (mut client, session) in clients {
        session.close().await.ok();
        client.stop();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_wakes_peer_recv() {
    let (mut server, addr) = server_on_random_port(4).await;
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.expect("accept");
        (server, session)
    });
    let (mut client, guest) = connect_client(addr).await;
    let (server, mut host) = accept_task.await.unwrap();

    guest.close().await.expect("close");
    // 重复 close 报 Closed。
    assert!(guest.close().await.is_err());

    // 对端 recv 应因 Disconnect 而结束。
    let result = tokio::time::timeout(Duration::from_secs(5), host.recv::<Box<[u8]>>()).await;
    assert!(
        matches!(result, Ok(Err(_))),
        "对端关闭后 recv 应返回错误，实际 {result:?}"
    );

    drop(server);
    client.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unconnected_ping_returns_motd() {
    let (server, addr) = server_on_random_port(4).await;

    let mut client = RakClient::new(|config| {
        config.max_mtu_size = MTU;
    });
    client.start().await.expect("client start");
    let (motd, rtt) = tokio::time::timeout(Duration::from_secs(5), client.ping(addr))
        .await
        .expect("ping 超时")
        .expect("ping");
    let text = String::from_utf8_lossy(&motd);
    assert!(text.starts_with("MCPE;LoopbackTest;"), "MOTD 异常：{text}");
    assert!(rtt < Duration::from_secs(2));

    drop(server);
    client.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_connections_rejects_extra_client() {
    let (mut server, addr) = server_on_random_port(1).await;
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.expect("accept");
        (server, session)
    });
    let (mut client1, session1) = connect_client(addr).await;
    let (server, host) = accept_task.await.unwrap();

    let mut client2 = RakClient::new(|config| {
        config.max_mtu_size = MTU;
        config.conn_attempt_timeout = Duration::from_secs(5);
    });
    client2.start().await.expect("client2 start");
    let result = client2.connect(addr).await;
    assert!(result.is_err(), "超出 max_connections 的连接应失败");

    drop(host);
    drop(server);
    session1.close().await.ok();
    client1.stop();
    client2.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn send_via_shared_reference_while_recv_pending() {
    // 验证 send(&self) 与 recv(&mut self) 可并发（消费方的 select! 模式）。
    let (mut server, addr) = server_on_random_port(4).await;
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.expect("accept");
        (server, session)
    });
    let (mut client, mut guest) = connect_client(addr).await;
    let (server, host) = accept_task.await.unwrap();

    // host 侧并发发送（send 只需 &self，可与他处 recv 并发）。
    let host = std::sync::Arc::new(host);
    let sender = {
        let host = host.clone();
        tokio::spawn(async move {
            for i in 0..50u8 {
                host.send(
                    vec![0xFE, i],
                    RakReliability::ReliableOrdered,
                    RakPriority::High,
                )
                .await
                .expect("host send");
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
    };

    // guest 侧在 select! 中 recv：另一分支频繁胜出以验证 cancel-safety
    // （recv 被取消后不允许丢消息或乱序）。
    let mut received = Vec::new();
    let mut noise = tokio::time::interval(Duration::from_millis(1));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while received.len() < 50 {
        tokio::select! {
            packet = guest.recv::<Box<[u8]>>() => {
                received.push(packet.expect("guest recv"));
            }
            _ = noise.tick() => {}
            _ = tokio::time::sleep_until(deadline) => panic!("接收超时，仅收到 {}", received.len()),
        }
    }
    for (i, packet) in received.iter().enumerate() {
        assert_eq!(packet[1] as usize, i, "select! 下不允许丢消息或乱序");
    }
    sender.await.unwrap();

    drop(server);
    client.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn immediate_reconnect_from_same_client() {
    // 回归：旧会话的 dead 通知曾按地址盲删路由表，可把重连建立的
    // 新会话一并抹掉，使重连握手挂起到超时。
    let (mut server, addr) = server_on_random_port(4).await;
    let server_task = tokio::spawn(async move {
        let mut first = server.accept().await.expect("accept #1");
        let packet: Box<[u8]> = first.recv().await.expect("recv #1");
        assert_eq!(&packet[..], b"\xFEfirst");
        drop(first);
        let mut second = server.accept().await.expect("accept #2");
        let packet: Box<[u8]> = second.recv().await.expect("recv #2");
        (server, packet)
    });

    let (mut client1, session1) = connect_client(addr).await;
    session1
        .send(
            &b"\xFEfirst"[..],
            RakReliability::ReliableOrdered,
            RakPriority::High,
        )
        .await
        .expect("send #1");
    session1.close().await.ok();
    client1.stop();

    // 立即重连（新客户端，通常是新端口；服务端侧路由清理与新会话
    // 插入的竞态窗口在这里被反复触发）。
    let (mut client2, session2) = connect_client(addr).await;
    session2
        .send(
            &b"\xFEsecond"[..],
            RakReliability::ReliableOrdered,
            RakPriority::High,
        )
        .await
        .expect("send #2");

    let (server, packet) = tokio::time::timeout(Duration::from_secs(15), server_task)
        .await
        .expect("重连测试超时")
        .unwrap();
    assert_eq!(&packet[..], b"\xFEsecond");

    drop(server);
    session2.close().await.ok();
    client2.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ping_to_dead_address_errors_instead_of_hanging() {
    // 回归：ping 等待者曾永不过期，对无响应地址的调用永久挂起。
    let probe = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let dead: SocketAddr = probe.local_addr().unwrap();
    drop(probe);

    let mut client = RakClient::new(|_| {});
    client.start().await.expect("client start");
    let result = tokio::time::timeout(Duration::from_secs(20), client.ping(dead)).await;
    assert!(
        matches!(result, Ok(Err(_))),
        "无响应地址的 ping 应在超时后返回错误，实际 {result:?}"
    );
    client.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_connect_frees_the_dialer() {
    // 回归：connect() 被取消后拨号状态机继续占用，期间所有新 connect
    // 一律误报 AlreadyConnected。
    let probe = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let dead: SocketAddr = probe.local_addr().unwrap();
    drop(probe);

    let mut client = RakClient::new(|config| {
        config.conn_attempt_timeout = Duration::from_secs(60);
    });
    client.start().await.expect("client start");

    // 取消一次拨号。
    let cancelled = tokio::time::timeout(Duration::from_millis(300), client.connect(dead)).await;
    assert!(cancelled.is_err(), "拨号到黑洞地址不应立刻返回");

    // 状态机应已释放：下一次拨号必须真正开始（而非立刻 AlreadyConnected）。
    tokio::time::sleep(Duration::from_millis(200)).await;
    let retry = tokio::time::timeout(Duration::from_millis(500), client.connect(dead)).await;
    assert!(
        retry.is_err(),
        "重试应进入拨号流程（超时），而不是立刻返回 AlreadyConnected：{retry:?}"
    );
    client.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn zero_copy_bytes_api() {
    let (mut server, addr) = server_on_random_port(4).await;
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.expect("accept");
        (server, session)
    });
    let (mut client, guest) = connect_client(addr).await;
    let (server, mut host) = accept_task.await.unwrap();

    guest
        .send_bytes(
            Bytes::from_static(b"\xFEbytes api"),
            RakReliability::ReliableOrdered,
            RakPriority::High,
        )
        .await
        .expect("send_bytes");
    let received = tokio::time::timeout(Duration::from_secs(5), host.recv_bytes())
        .await
        .expect("recv_bytes 超时")
        .expect("recv_bytes");
    assert_eq!(&received[..], b"\xFEbytes api");

    drop(server);
    client.stop();
}
