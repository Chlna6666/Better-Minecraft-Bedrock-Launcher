//! 回环集成测试：真实 UDP 发现 + 真实 WebRTC 协商 + 数据通道收发。

use bedrock_nethernet::{
    LanSignaling, MAX_SEGMENT_PAYLOAD, NethernetListener, NethernetSession, NethernetStream,
    ServerData,
};
use bytes::Bytes;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

const NEGOTIATION_BUDGET: Duration = Duration::from_secs(30);

fn server_data() -> ServerData {
    ServerData {
        server_name: "BMCBL".to_string(),
        level_name: "PaperConnect".to_string(),
        game_type: 0,
        player_count: 1,
        max_player_count: 20,
        editor_world: false,
        hardcore: false,
        accepts_online_auth: true,
        accepts_self_signed_auth: true,
        transport_layer: 2,
        connection_type: 4,
    }
}

/// 建立一对已完成协商的会话（客户端流 + 服务端会话）。
async fn connected_pair() -> (NethernetStream, Arc<NethernetSession>, NethernetListener) {
    let data = server_data();
    let server_signaling =
        LanSignaling::server(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), data.clone())
            .await
            .expect("绑定服务端信令");
    let server_addr = server_signaling.local_addr().expect("服务端地址");

    let client_signaling = Arc::new(
        LanSignaling::client(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)), server_addr)
            .await
            .expect("绑定客户端信令"),
    );
    let discovered = client_signaling
        .discover(Duration::from_secs(5))
        .await
        .expect("发现服务端");
    assert_eq!(discovered.server_data, data);

    let mut listener =
        NethernetListener::bind(server_signaling, SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
            .expect("绑定监听器");

    let (client, server) = tokio::time::timeout(NEGOTIATION_BUDGET, async {
        tokio::try_join!(
            NethernetStream::connect(
                Arc::clone(&client_signaling),
                discovered.network_id,
                discovered.address,
            ),
            listener.accept(),
        )
    })
    .await
    .expect("协商超时")
    .expect("协商失败");

    // NethernetStream 自己持有信令端点，这里无需再额外保活。
    drop(client_signaling);
    (client, server, listener)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn discovery_and_bidirectional_round_trip() {
    let (client, server, _listener) = connected_pair().await;

    client
        .send(Bytes::from_static(b"request"))
        .await
        .expect("客户端发送");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), server.recv())
            .await
            .expect("服务端接收超时")
            .expect("服务端接收"),
        Some(Bytes::from_static(b"request"))
    );

    server
        .send(Bytes::from_static(b"response"))
        .await
        .expect("服务端发送");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), client.recv())
            .await
            .expect("客户端接收超时")
            .expect("客户端接收"),
        Some(Bytes::from_static(b"response"))
    );

    client.close().await.expect("关闭客户端");
    server.close().await.expect("关闭服务端");
}

/// 回归：webrtc-rs 默认把 SCTP 单消息上限设为 64 KiB，
/// 而 `NetherNet` 的分片是 256 KiB−1。不放开该上限，
/// 任何 64 KiB 到 256 KiB 之间的报文都会被底层拒发。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_segment_above_64k_is_deliverable() {
    let (client, server, _listener) = connected_pair().await;

    let payload = Bytes::from(
        (0..200_000_u32)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect::<Vec<_>>(),
    );
    assert!(payload.len() > 65_536, "测试载荷须超过旧的 64 KiB 上限");
    assert!(payload.len() <= MAX_SEGMENT_PAYLOAD, "本用例只测单分片路径");

    client.send(payload.clone()).await.expect("发送大报文");
    let received = tokio::time::timeout(Duration::from_secs(20), server.recv())
        .await
        .expect("接收大报文超时")
        .expect("接收大报文")
        .expect("会话已关闭");
    assert_eq!(received.len(), payload.len());
    assert_eq!(received, payload);

    client.close().await.ok();
    server.close().await.ok();
}

/// 超过单分片上限的消息走多分片路径。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn multi_segment_message_reassembles() {
    let (client, server, _listener) = connected_pair().await;

    let payload = Bytes::from(
        (0..MAX_SEGMENT_PAYLOAD * 2 + 5000)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect::<Vec<_>>(),
    );
    client.send(payload.clone()).await.expect("发送多分片报文");
    let received = tokio::time::timeout(Duration::from_secs(30), server.recv())
        .await
        .expect("接收多分片报文超时")
        .expect("接收")
        .expect("会话已关闭");
    assert_eq!(received.len(), payload.len());
    assert_eq!(received, payload);

    client.close().await.ok();
    server.close().await.ok();
}

/// 连续多条消息必须保序且不丢。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ordered_burst_is_preserved() {
    const COUNT: u32 = 200;

    let (client, server, _listener) = connected_pair().await;
    let sender = tokio::spawn({
        let client = client.clone();
        async move {
            for i in 0..COUNT {
                client
                    .send(Bytes::from(i.to_be_bytes().to_vec()))
                    .await
                    .expect("发送");
            }
        }
    });

    for expected in 0..COUNT {
        let packet = tokio::time::timeout(Duration::from_secs(20), server.recv())
            .await
            .expect("接收超时")
            .expect("接收")
            .expect("会话已关闭");
        assert_eq!(
            u32::from_be_bytes(packet[..4].try_into().unwrap()),
            expected,
            "可靠有序通道必须按序交付"
        );
    }
    sender.await.unwrap();

    client.close().await.ok();
    server.close().await.ok();
}

/// 关闭后 recv 返回 None 而不是永久挂起。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_terminates_recv() {
    let (client, server, _listener) = connected_pair().await;

    server.close().await.expect("关闭服务端");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), server.recv())
            .await
            .expect("关闭后 recv 应立即返回")
            .expect("recv"),
        None
    );
    // 重复关闭幂等。
    server.close().await.expect("重复关闭");
    assert!(server.is_closed());

    client.close().await.ok();
}

/// 空消息与超限消息在发送侧就被拒绝。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rejects_empty_and_oversized_messages() {
    let (client, server, _listener) = connected_pair().await;

    assert!(client.send(Bytes::new()).await.is_err(), "空消息应被拒绝");
    assert!(
        server
            .send_unreliable(Bytes::from(vec![0_u8; MAX_SEGMENT_PAYLOAD + 1]))
            .await
            .is_err(),
        "不可靠通道不接受多分片消息"
    );

    client.close().await.ok();
    server.close().await.ok();
}

/// 回归：对端先断开时，本端的 `close()` 曾因复用同一个标志而短路，
/// 导致 `RTCPeerConnection` 永不关闭（webrtc-rs 无 Drop，会永久泄漏
/// ICE agent 与 UDP 套接字）。这是最常见的断开路径。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn close_after_peer_disconnect_still_tears_down() {
    let (client, server, _listener) = connected_pair().await;

    // 对端主动断开。
    client.close().await.expect("客户端关闭");

    // 本端观察到关闭。
    let result = tokio::time::timeout(Duration::from_secs(10), server.recv())
        .await
        .expect("对端断开后 recv 应返回")
        .expect("recv");
    assert_eq!(result, None);
    assert!(server.is_closed());

    // 关键：此时 close() 仍必须真正执行拆除，而不是直接短路返回。
    server.close().await.expect("对端先断开后仍应能拆除");
    // 幂等。
    server.close().await.expect("重复拆除");
}

/// 回归：读任务曾持有 `Arc<Session>`，用户不调用 `close()` 直接丢弃
/// 句柄时会话永不 `Drop`，底层 WebRTC 栈随之泄漏。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_session_without_close_releases_it() {
    let (client, server, _listener) = connected_pair().await;
    let weak = Arc::downgrade(&server);
    assert_eq!(Arc::strong_count(&server), 1, "读任务不应持有强引用");

    drop(server);
    // 给 Drop 中 spawn 的拆除任务一点时间。
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(weak.upgrade().is_none(), "会话应已释放");

    client.close().await.ok();
}

/// 两条通道各自独立排队：不可靠通道的流量不得影响可靠通道的交付。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn channels_have_independent_queues() {
    let (client, server, _listener) = connected_pair().await;

    client
        .session()
        .send_unreliable(Bytes::from_static(b"unreliable"))
        .await
        .expect("不可靠发送");
    client
        .send(Bytes::from_static(b"reliable"))
        .await
        .expect("可靠发送");

    // 可靠通道的读取不会被不可靠消息干扰。
    let reliable = tokio::time::timeout(Duration::from_secs(10), server.recv())
        .await
        .expect("可靠接收超时")
        .expect("接收")
        .expect("会话已关闭");
    assert_eq!(&reliable[..], b"reliable");

    let unreliable = tokio::time::timeout(Duration::from_secs(10), server.recv_unreliable())
        .await
        .expect("不可靠接收超时")
        .expect("接收")
        .expect("会话已关闭");
    assert_eq!(&unreliable[..], b"unreliable");

    client.close().await.ok();
    server.close().await.ok();
}

#[test]
fn public_types_are_send_and_sync() {
    fn assert_send<T: Send>() {}
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send::<NethernetListener>();
    assert_send_sync::<NethernetStream>();
    assert_send_sync::<NethernetSession>();
    assert_send_sync::<LanSignaling>();
}
