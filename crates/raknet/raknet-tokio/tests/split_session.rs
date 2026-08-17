//! 会话拆分回归测试：接收端等待期间，发送句柄必须可以独立发送大包。

use bytes::Bytes;
use raknet_tokio::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

const MTU: u16 = 1200;

async fn server_on_random_port() -> (RakServer, SocketAddr) {
    let probe = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    let mut server = RakServer::new(addr, |config| {
        config.max_connections = 4;
        config.max_mtu_size = MTU;
        config.message = Box::from(
            &b"MCPE;SplitSessionTest;589;1.20.0;0;4;42;World;Survival;0;19132;19132;"[..],
        );
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
        .expect("connect timeout")
        .expect("connect failed");
    (client, session)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn split_receiver_does_not_block_large_reliable_send() {
    let (mut server, addr) = server_on_random_port().await;
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.expect("accept");
        (server, session)
    });

    let (mut client, mut guest) = connect_client(addr).await;
    let (server, host) = accept_task.await.unwrap();
    let (host_tx, mut host_rx) = host.into_split();

    // 先让唯一接收端进入等待状态。旧的 Calcite 适配层把整个 RakSession 放进
    // Tokio Mutex，这种情况下等待 recv 会把 send 一并串行化。
    let receiver_task = tokio::spawn(async move {
        let packet = tokio::time::timeout(Duration::from_secs(10), host_rx.recv_bytes())
            .await
            .expect("host recv timeout")
            .expect("host recv");
        (host_rx, packet)
    });
    tokio::task::yield_now().await;

    // 模拟 PlayerList/区块等大消息：发送端必须无需接收端锁即可立刻入可靠层。
    let mut payload = vec![0xFE; 256 * 1024];
    payload[1..9].copy_from_slice(b"split-ok");
    host_tx
        .clone()
        .send_bytes(
            Bytes::from(payload),
            RakReliability::ReliableOrdered,
            RakPriority::High,
        )
        .await
        .expect("split sender large send");

    let received = tokio::time::timeout(Duration::from_secs(15), guest.recv_bytes())
        .await
        .expect("guest recv timeout")
        .expect("guest recv");
    assert_eq!(received.len(), 256 * 1024);
    assert_eq!(&received[1..9], b"split-ok");

    guest
        .send_bytes(
            Bytes::from_static(b"\xFEack"),
            RakReliability::ReliableOrdered,
            RakPriority::High,
        )
        .await
        .expect("guest ack");

    let (host_rx, ack) = receiver_task.await.unwrap();
    assert_eq!(&ack[..], b"\xFEack");

    host_tx.close().await.ok();
    drop(host_rx);
    drop(server);
    client.stop();
}
