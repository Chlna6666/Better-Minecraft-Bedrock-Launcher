//! 吞吐量冒烟测试（默认忽略，手动运行）：
//! `cargo test -p raknet-tokio --release --test throughput -- --ignored --nocapture`

use bytes::Bytes;
use raknet_tokio::prelude::*;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn loopback_throughput() {
    let probe = tokio::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let addr: SocketAddr = probe.local_addr().unwrap();
    drop(probe);

    let mut server = RakServer::new(addr, |config| {
        config.max_connections = 2;
        config.max_mtu_size = 1200;
    });
    server.start().await.unwrap();
    let accept_task = tokio::spawn(async move {
        let session = server.accept().await.unwrap();
        (server, session)
    });

    let mut client = RakClient::new(|config| {
        config.max_mtu_size = 1200;
    });
    client.start().await.unwrap();
    let guest = client.connect(addr).await.unwrap();
    let (server, mut host) = accept_task.await.unwrap();

    // 模拟 PaperConnect 隧道负载：900B 一条，共 64MB。
    const MSG: usize = 900;
    const TOTAL: usize = 64 * 1024 * 1024;
    const COUNT: usize = TOTAL / MSG;
    let payload = Bytes::from(vec![0xFEu8; MSG]);

    let start = Instant::now();
    let sender = tokio::spawn(async move {
        for _ in 0..COUNT {
            guest
                .send_bytes(
                    payload.clone(),
                    RakReliability::ReliableOrdered,
                    RakPriority::High,
                )
                .await
                .unwrap();
        }
        guest
    });

    let mut received = 0usize;
    while received < COUNT {
        let packet = tokio::time::timeout(Duration::from_secs(30), host.recv_bytes())
            .await
            .expect("吞吐测试接收超时")
            .unwrap();
        assert_eq!(packet.len(), MSG);
        received += 1;
    }
    let elapsed = start.elapsed();
    let mib = TOTAL as f64 / 1024.0 / 1024.0;
    println!(
        "回环吞吐：{mib:.0} MiB / {:.2}s = {:.1} MiB/s（{} 条消息，RTT {:?}）",
        elapsed.as_secs_f64(),
        mib / elapsed.as_secs_f64(),
        COUNT,
        sender.await.map(|g| futures_rtt(&g)).unwrap_or_default(),
    );

    drop(server);
    client.stop();
}

fn futures_rtt(session: &RakSession) -> Duration {
    session.rtt()
}
