use rand::RngExt as _;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};

const RAKNET_DISCOVERY_PORT: u16 = 19132;
const UNCONNECTED_PING: u8 = 0x01;
const UNCONNECTED_PING_OPEN_CONNECTIONS: u8 = 0x02;
const UNCONNECTED_PONG: u8 = 0x1c;
const RAKNET_MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];

#[derive(Debug, Clone)]
pub struct RakNetServerInfo {
    pub motd: String,
    pub server_name: String,
    pub level_name: String,
    pub game_port: u16,
    pub server_guid: u64,
}

pub async fn scan_local_raknet(timeout: Duration) -> Result<RakNetServerInfo, String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|error| format!("RakNet 局域网扫描监听失败：{error}"))?;
    socket
        .set_broadcast(true)
        .map_err(|error| format!("RakNet 局域网扫描启用广播失败：{error}"))?;
    let targets = [
        SocketAddr::from(([127, 0, 0, 1], RAKNET_DISCOVERY_PORT)),
        SocketAddr::from(([255, 255, 255, 255], RAKNET_DISCOVERY_PORT)),
    ];
    let deadline = Instant::now() + timeout;
    let mut interval = tokio::time::interval(Duration::from_millis(500));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut buffer = [0_u8; 2048];

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let ping = build_unconnected_ping();
                for target in targets {
                    if let Err(error) = socket.send_to(&ping, target).await {
                        tracing::debug!(%target, "发送 RakNet 探测包失败：{error}");
                    }
                }
            }
            received = socket.recv_from(&mut buffer) => {
                let (length, _) = received
                    .map_err(|error| format!("读取 RakNet 局域网响应失败：{error}"))?;
                if let Ok(server) = parse_unconnected_pong(&buffer[..length]) {
                    return Ok(server);
                }
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err("未检测到本机 RakNet 局域网世界".to_string());
            }
        }
    }
}

pub async fn start_fake_raknet_server(
    display_name: String,
    proxy_port: u16,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) -> Result<JoinHandle<()>, String> {
    let responder = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, RAKNET_DISCOVERY_PORT))
        .await
        .map_err(|error| format!("无法监听 RakNet 发现端口 {RAKNET_DISCOVERY_PORT}：{error}"))?;
    responder
        .set_broadcast(true)
        .map_err(|error| format!("RakNet 发现端口启用广播失败：{error}"))?;
    let broadcast = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|error| format!("RakNet 主动广播监听失败：{error}"))?;
    broadcast
        .set_broadcast(true)
        .map_err(|error| format!("RakNet 主动广播启用失败：{error}"))?;

    let server_guid = rand::rng().random::<u64>();
    let motd = query_forwarded_motd(proxy_port, &display_name, server_guid).await;
    crate::tasks::runtime::spawn_io(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(1500));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut buffer = [0_u8; 2048];
        loop {
            tokio::select! {
                _ = &mut cancel => break,
                received = responder.recv_from(&mut buffer) => {
                    let Ok((length, source)) = received else {
                        break;
                    };
                    let Some(timestamp) = parse_unconnected_ping(&buffer[..length]) else {
                        continue;
                    };
                    let pong = build_unconnected_pong(&motd, server_guid, timestamp);
                    if let Err(error) = responder.send_to(&pong, source).await {
                        tracing::debug!(%source, "回复 RakNet 发现请求失败：{error}");
                    }
                }
                _ = interval.tick() => {
                    let pong = build_unconnected_pong(&motd, server_guid, now_ms());
                    for target in [
                        SocketAddr::from(([127, 0, 0, 1], RAKNET_DISCOVERY_PORT)),
                        SocketAddr::from(([255, 255, 255, 255], RAKNET_DISCOVERY_PORT)),
                    ] {
                        if let Err(error) = broadcast.send_to(&pong, target).await {
                            tracing::debug!(%target, "发送 RakNet 房间广播失败：{error}");
                        }
                    }
                }
            }
        }
    })
}

async fn query_forwarded_motd(proxy_port: u16, display_name: &str, server_guid: u64) -> String {
    match scan_raknet_endpoint(
        SocketAddr::from(([127, 0, 0, 1], proxy_port)),
        Duration::from_secs(3),
    )
    .await
    {
        Ok(server) => rewrite_motd(&server.motd, display_name, server_guid, proxy_port),
        Err(error) => {
            tracing::warn!(
                proxy_port,
                "无法读取转发后的基岩版 MOTD，使用兼容信息：{error}"
            );
            fallback_motd(display_name, server_guid, proxy_port)
        }
    }
}

async fn scan_raknet_endpoint(
    endpoint: SocketAddr,
    timeout: Duration,
) -> Result<RakNetServerInfo, String> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .await
        .map_err(|error| format!("RakNet 状态查询监听失败：{error}"))?;
    socket
        .send_to(&build_unconnected_ping(), endpoint)
        .await
        .map_err(|error| format!("发送 RakNet 状态查询失败：{error}"))?;
    let mut buffer = [0_u8; 2048];
    let (length, _) = tokio::time::timeout(timeout, socket.recv_from(&mut buffer))
        .await
        .map_err(|_| "RakNet 状态查询超时".to_string())?
        .map_err(|error| format!("读取 RakNet 状态响应失败：{error}"))?;
    parse_unconnected_pong(&buffer[..length])
}

fn build_unconnected_ping() -> [u8; 33] {
    let mut packet = [0_u8; 33];
    packet[0] = UNCONNECTED_PING;
    packet[1..9].copy_from_slice(&now_ms().to_be_bytes());
    packet[9..25].copy_from_slice(&RAKNET_MAGIC);
    packet[25..33].copy_from_slice(&rand::rng().random::<u64>().to_be_bytes());
    packet
}

fn parse_unconnected_ping(packet: &[u8]) -> Option<u64> {
    if packet.len() < 25
        || !matches!(
            packet[0],
            UNCONNECTED_PING | UNCONNECTED_PING_OPEN_CONNECTIONS
        )
        || packet[9..25] != RAKNET_MAGIC
    {
        return None;
    }
    Some(u64::from_be_bytes(packet[1..9].try_into().ok()?))
}

fn build_unconnected_pong(motd: &str, server_guid: u64, timestamp: u64) -> Vec<u8> {
    let motd_bytes = motd.as_bytes();
    let motd_length = u16::try_from(motd_bytes.len()).unwrap_or(u16::MAX);
    let motd_bytes = &motd_bytes[..usize::from(motd_length)];
    let mut packet = Vec::with_capacity(35 + motd_bytes.len());
    packet.push(UNCONNECTED_PONG);
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&server_guid.to_be_bytes());
    packet.extend_from_slice(&RAKNET_MAGIC);
    packet.extend_from_slice(&motd_length.to_be_bytes());
    packet.extend_from_slice(motd_bytes);
    packet
}

fn parse_unconnected_pong(packet: &[u8]) -> Result<RakNetServerInfo, String> {
    if packet.len() < 35 || packet[0] != UNCONNECTED_PONG || packet[17..33] != RAKNET_MAGIC {
        return Err("RakNet Pong 数据无效".to_string());
    }
    let server_guid = u64::from_be_bytes(
        packet[9..17]
            .try_into()
            .map_err(|_| "RakNet GUID 无效".to_string())?,
    );
    let motd_length = usize::from(u16::from_be_bytes(
        packet[33..35]
            .try_into()
            .map_err(|_| "RakNet MOTD 长度无效".to_string())?,
    ));
    let motd = packet
        .get(35..35 + motd_length)
        .ok_or_else(|| "RakNet MOTD 数据不完整".to_string())?;
    let motd = std::str::from_utf8(motd)
        .map_err(|error| format!("RakNet MOTD 不是 UTF-8：{error}"))?
        .to_string();
    parse_motd(&motd, server_guid)
}

fn parse_motd(motd: &str, server_guid: u64) -> Result<RakNetServerInfo, String> {
    let fields: Vec<&str> = motd.split(';').collect();
    if fields.len() < 12 || fields[0] != "MCPE" {
        return Err("RakNet MOTD 字段不完整".to_string());
    }
    let game_port = fields[10]
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "RakNet MOTD 游戏端口无效".to_string())?;
    Ok(RakNetServerInfo {
        motd: motd.to_string(),
        server_name: fields[1].to_string(),
        level_name: fields[7].to_string(),
        game_port,
        server_guid,
    })
}

fn rewrite_motd(motd: &str, display_name: &str, server_guid: u64, proxy_port: u16) -> String {
    let mut fields: Vec<String> = motd.split(';').map(str::to_string).collect();
    if fields.len() < 12 || fields.first().is_none_or(|field| field != "MCPE") {
        return fallback_motd(display_name, server_guid, proxy_port);
    }
    fields[1] = display_name.to_string();
    fields[6] = server_guid.to_string();
    fields[7] = "PaperConnect".to_string();
    fields[10] = proxy_port.to_string();
    fields[11] = proxy_port.to_string();
    if fields.last().is_some_and(String::is_empty) {
        fields.join(";")
    } else {
        format!("{};", fields.join(";"))
    }
}

fn fallback_motd(display_name: &str, server_guid: u64, proxy_port: u16) -> String {
    format!(
        "MCPE;{display_name};589;1.20.0;1;20;{server_guid};PaperConnect;Survival;0;{proxy_port};{proxy_port};"
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{build_unconnected_pong, parse_unconnected_pong, rewrite_motd};

    #[test]
    fn pong_round_trip_keeps_server_port() {
        let motd = "MCPE;Host;999;1.99.0;1;8;42;World;Creative;1;19133;19133;";
        let packet = build_unconnected_pong(motd, 42, 7);
        let parsed = parse_unconnected_pong(&packet).expect("pong should parse");
        assert_eq!(parsed.game_port, 19133);
        assert_eq!(parsed.server_name, "Host");
    }

    #[test]
    fn rewritten_motd_uses_custom_name_and_proxy_port() {
        let motd = "MCPE;Host;999;1.99.0;1;8;42;World;Creative;1;19133;19133;";
        let rewritten = rewrite_motd(motd, "BMCBL Room", 77, 23000);
        assert_eq!(
            rewritten,
            "MCPE;BMCBL Room;999;1.99.0;1;8;77;PaperConnect;Creative;1;23000;23000;"
        );
    }
}
