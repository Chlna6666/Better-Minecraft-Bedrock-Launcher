use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

pub async fn race_ipv4(domain: &str, connect_timeout: Duration) -> Option<SocketAddr> {
    info!("正在解析优选域名: {}", domain);

    let addrs = match tokio::net::lookup_host(domain).await {
        Ok(iter) => iter,
        Err(e) => {
            warn!("解析优选域名失败: {}", e);
            return None;
        }
    };

    let ips: Vec<SocketAddr> = addrs.filter(|ip| ip.is_ipv4()).collect();
    if ips.is_empty() {
        warn!("优选域名未解析到有效的 IPv4 地址");
        return None;
    }
    info!("解析到 {} 个候选 IP", ips.len());

    let mut set = JoinSet::new();
    for (i, ip) in ips.iter().cloned().enumerate() {
        let connect_timeout = connect_timeout;
        set.spawn(async move {
            let start = Instant::now();
            if let Ok(Ok(_)) =
                tokio::time::timeout(connect_timeout, tokio::net::TcpStream::connect(ip)).await
            {
                let elapsed = start.elapsed();
                debug!(
                    "[Race #{}] ✅ 连接成功! IP: {}, 耗时: {:.2?}",
                    i, ip, elapsed
                );
                return Some(ip);
            }
            None
        });
    }

    while let Some(res) = set.join_next().await {
        match res {
            Ok(Some(ip)) => {
                info!(
                    "🏁 竞速冠军诞生: {}。正在终止其他 {} 个测速任务...",
                    ip,
                    set.len()
                );
                set.abort_all();
                return Some(ip);
            }
            _ => continue,
        }
    }

    warn!("所有优选 IP 测速均失败或超时，回退默认解析");
    None
}

pub async fn get_optimized_ip() -> Option<SocketAddr> {
    race_ipv4("cf.090227.xyz:443", Duration::from_secs(2)).await
}
