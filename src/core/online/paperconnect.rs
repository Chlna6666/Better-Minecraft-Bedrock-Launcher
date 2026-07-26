use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::MissedTickBehavior;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const PLAYER_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const PLAYER_EXPIRY: Duration = Duration::from_secs(10);
const PLAYER_CLEANUP_INTERVAL: Duration = Duration::from_secs(1);

static SERVER_TASK: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static CLIENT_TASK: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
static PLAYER_SNAPSHOT: Mutex<Vec<PaperConnectPlayer>> = Mutex::new(Vec::new());

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub host: String,
    pub server_port: u16,
    pub game_host: String,
    pub game_port: u16,
    pub game_type: String,
    pub game_protocol_type: String,
    pub protocol: PaperConnectProtocol,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum PaperConnectProtocol {
    Nethernet,
    #[default]
    Raknet,
}

impl PaperConnectProtocol {
    pub const fn hostname_marker(self) -> char {
        match self {
            Self::Nethernet => 'g',
            Self::Raknet => 'r',
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ServerHostname {
    pub server_port: u16,
    pub game_port: Option<u16>,
    pub protocol: Option<PaperConnectProtocol>,
}

#[derive(Debug, Deserialize)]
struct PingRequest {
    time: i64,
}

#[derive(Debug, Deserialize)]
struct PlayerRequest {
    #[serde(rename = "clientId")]
    client_id: String,
    #[serde(rename = "playerName")]
    player_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Eq, PartialEq)]
pub struct PaperConnectPlayer {
    #[serde(alias = "playerName")]
    pub player: String,
    #[serde(rename = "clientId")]
    pub client_id: String,
    #[serde(rename = "isRoomHost")]
    pub is_room_host: bool,
    #[serde(skip)]
    last_seen: i64,
}

#[derive(Debug, Deserialize, Serialize)]
struct PingResponse {
    time: i64,
    #[serde(rename = "returnTime")]
    return_time: i64,
    #[serde(rename = "gameType")]
    game_type: String,
    #[serde(rename = "gameProtocolType")]
    game_protocol_type: String,
    #[serde(rename = "gamePort")]
    game_port: u16,
    #[serde(default)]
    protocol: Option<PaperConnectProtocol>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PlayerResponse {
    #[serde(rename = "returnTime")]
    return_time: i64,
    players: Vec<PaperConnectPlayer>,
}

pub fn players() -> Vec<PaperConnectPlayer> {
    PLAYER_SNAPSHOT
        .lock()
        .map(|players| players.clone())
        .unwrap_or_default()
}

pub fn clear_players() {
    if let Ok(mut players) = PLAYER_SNAPSHOT.lock() {
        players.clear();
    }
}

fn replace_player_snapshot(mut players: Vec<PaperConnectPlayer>) {
    players.sort_by(|left, right| {
        right
            .is_room_host
            .cmp(&left.is_room_host)
            .then_with(|| left.player.cmp(&right.player))
    });
    if let Ok(mut snapshot) = PLAYER_SNAPSHOT.lock() {
        *snapshot = players;
    }
}

pub fn build_server_hostname(
    server_port: u16,
    protocol: PaperConnectProtocol,
    game_port: u16,
) -> Result<String, String> {
    validate_port("联机中心", server_port)?;
    validate_port("游戏", game_port)?;
    Ok(format!(
        "pcs-{server_port}-{}-{game_port}",
        protocol.hostname_marker()
    ))
}

pub fn parse_server_hostname(hostname: &str) -> Option<ServerHostname> {
    let hostname = hostname.trim();
    if let Some(port) = hostname.strip_prefix("paper-connect-server-") {
        return parse_port(port).map(|server_port| ServerHostname {
            server_port,
            game_port: None,
            protocol: None,
        });
    }

    let rest = hostname.strip_prefix("pcs-")?;
    let (server_port, protocol, game_port) =
        if let Some((server_port, game_port)) = rest.split_once("-g-") {
            (server_port, PaperConnectProtocol::Nethernet, game_port)
        } else {
            let (server_port, game_port) = rest.split_once("-r-")?;
            (server_port, PaperConnectProtocol::Raknet, game_port)
        };
    Some(ServerHostname {
        server_port: parse_port(server_port)?,
        game_port: Some(parse_port(game_port)?),
        protocol: Some(protocol),
    })
}

pub fn server_port_from_hostname(hostname: &str) -> Option<u16> {
    parse_server_hostname(hostname).map(|hostname| hostname.server_port)
}

fn parse_port(port: &str) -> Option<u16> {
    let port = port.parse::<u16>().ok()?;
    (1025..=65535).contains(&port).then_some(port)
}

fn validate_port(description: &str, port: u16) -> Result<(), String> {
    if (1025..=65535).contains(&port) {
        Ok(())
    } else {
        Err(format!("PaperConnect {description}端口无效：{port}"))
    }
}

pub async fn start_server(
    server_port: u16,
    game_port: u16,
    protocol: PaperConnectProtocol,
    host_player_name: String,
) -> Result<(), String> {
    validate_port("联机中心", server_port)?;
    validate_port("游戏", game_port)?;
    if host_player_name.trim().is_empty() {
        return Err("PaperConnect 房主名称不能为空".to_string());
    }

    stop_server();
    let listener = TcpListener::bind(("0.0.0.0", server_port))
        .await
        .map_err(|error| format!("PaperConnect 联机中心监听 {server_port} 失败：{error}"))?;
    let host_player = PaperConnectPlayer {
        player: host_player_name.trim().to_string(),
        client_id: client_id(),
        is_room_host: true,
        last_seen: now_ms(),
    };
    let players = Arc::new(Mutex::new(HashMap::from([(
        host_player.player.clone(),
        host_player.clone(),
    )])));
    replace_player_snapshot(vec![host_player]);
    let task = crate::tasks::runtime::spawn_io(async move {
        let mut cleanup = tokio::time::interval(PLAYER_CLEANUP_INTERVAL);
        cleanup.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut connections = JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, _address)) = accepted else {
                        break;
                    };
                    let players = Arc::clone(&players);
                    connections.spawn(async move {
                        if let Err(error) =
                            handle_connection(stream, game_port, protocol, players).await
                        {
                            tracing::debug!("PaperConnect 请求失败：{error}");
                        }
                    });
                }
                _ = cleanup.tick() => {
                    prune_inactive_players(&players);
                }
                Some(joined) = connections.join_next(), if !connections.is_empty() => {
                    if let Err(error) = joined {
                        tracing::debug!("PaperConnect 请求任务结束异常：{error}");
                    }
                }
            }
        }
    })?;
    if let Ok(mut server_task) = SERVER_TASK.lock() {
        *server_task = Some(task);
    }
    Ok(())
}

pub fn stop_server() {
    if let Ok(mut server_task) = SERVER_TASK.lock()
        && let Some(task) = server_task.take()
    {
        task.abort();
    }
}

pub fn stop_client() {
    if let Ok(mut client_task) = CLIENT_TASK.lock()
        && let Some(task) = client_task.take()
    {
        task.abort();
    }
}

pub async fn start_client(
    host: String,
    server_port: u16,
    player_name: String,
) -> Result<(), String> {
    stop_client();
    let client_id = client_id();
    let players = send_player(&host, server_port, &player_name, &client_id).await?;
    replace_player_snapshot(players);
    let mut client_task = CLIENT_TASK
        .lock()
        .map_err(|_| "PaperConnect 心跳任务锁已损坏".to_string())?;
    let task = crate::tasks::runtime::spawn_io(async move {
        loop {
            tokio::time::sleep(PLAYER_HEARTBEAT_INTERVAL).await;
            match send_player(&host, server_port, &player_name, &client_id).await {
                Ok(players) => replace_player_snapshot(players),
                Err(error) => tracing::debug!("PaperConnect 玩家心跳失败：{error}"),
            }
        }
    })?;
    *client_task = Some(task);
    Ok(())
}

fn client_id() -> String {
    format!(
        "BMCBL {}",
        crate::utils::app_info::get_version().trim_start_matches('v')
    )
}

pub async fn ping(host: &str, server_port: u16) -> Result<ServerInfo, String> {
    let mut stream = tokio::time::timeout(REQUEST_TIMEOUT, TcpStream::connect((host, server_port)))
        .await
        .map_err(|_| "连接 PaperConnect 联机中心超时".to_string())?
        .map_err(|error| format!("连接 PaperConnect 联机中心失败：{error}"))?;
    let request = format!("c:ping\0{}", serde_json::json!({ "time": now_ms() }));
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("发送 PaperConnect c:ping 失败：{error}"))?;
    let value: PingResponse = tokio::time::timeout(
        REQUEST_TIMEOUT,
        read_json_response(&mut stream, "PaperConnect c:ping 响应"),
    )
    .await
    .map_err(|_| "等待 PaperConnect 联机中心响应超时".to_string())??;
    if !(1025..=65535).contains(&value.game_port) {
        return Err(format!(
            "PaperConnect c:ping 返回无效游戏端口：{}",
            value.game_port
        ));
    }
    Ok(ServerInfo {
        host: host.to_string(),
        server_port,
        game_host: host.to_string(),
        game_port: value.game_port,
        game_type: value.game_type,
        game_protocol_type: value.game_protocol_type,
        protocol: value.protocol.unwrap_or_default(),
    })
}

async fn handle_connection(
    mut stream: TcpStream,
    game_port: u16,
    protocol: PaperConnectProtocol,
    players: Arc<Mutex<HashMap<String, PaperConnectPlayer>>>,
) -> Result<(), String> {
    let request = tokio::time::timeout(REQUEST_TIMEOUT, read_request(&mut stream))
        .await
        .map_err(|_| "读取 PaperConnect 请求超时".to_string())?
        .map_err(|error| format!("读取 PaperConnect 请求失败：{error}"))?;
    let (request_type, body) = request
        .split_once('\0')
        .ok_or_else(|| "PaperConnect 请求缺少协议分隔符".to_string())?;
    let response = match request_type {
        "c:ping" => handle_ping(body, game_port, protocol)?,
        "c:player" => handle_player(body, players)?,
        _ => return Err(format!("未知 PaperConnect 请求：{request_type}")),
    };
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|error| format!("发送 PaperConnect 响应失败：{error}"))?;
    stream
        .shutdown()
        .await
        .map_err(|error| format!("关闭 PaperConnect 响应失败：{error}"))?;
    Ok(())
}

async fn read_request(stream: &mut TcpStream) -> Result<String, String> {
    const MAX_REQUEST_SIZE: usize = 4096;
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];

    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("读取 PaperConnect 请求失败：{error}"))?;
        if read == 0 {
            break;
        }
        if request.len().saturating_add(read) > MAX_REQUEST_SIZE {
            return Err("PaperConnect 请求过大".to_string());
        }
        request.extend_from_slice(&buffer[..read]);

        let Some(separator) = request.iter().position(|byte| *byte == 0) else {
            continue;
        };
        if separator + 1 >= request.len() {
            continue;
        }
        let body = std::str::from_utf8(&request[separator + 1..])
            .map_err(|error| format!("PaperConnect 请求不是 UTF-8：{error}"))?;
        if serde_json::from_str::<serde_json::Value>(body).is_ok() {
            break;
        }
    }

    String::from_utf8(request).map_err(|error| format!("PaperConnect 请求不是 UTF-8：{error}"))
}

async fn read_json_response<T>(stream: &mut TcpStream, description: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    const MAX_RESPONSE_SIZE: usize = 4096;
    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];

    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .map_err(|error| format!("读取{description}失败：{error}"))?;
        if read == 0 {
            return serde_json::from_slice(&response)
                .map_err(|error| format!("{description}无效：{error}"));
        }
        if response.len().saturating_add(read) > MAX_RESPONSE_SIZE {
            return Err(format!("{description}超过 {MAX_RESPONSE_SIZE} 字节限制"));
        }
        response.extend_from_slice(&buffer[..read]);

        match serde_json::from_slice(&response) {
            Ok(value) => return Ok(value),
            Err(error) if error.is_eof() => {}
            Err(error) => return Err(format!("{description}无效：{error}")),
        }
    }
}

async fn send_player(
    host: &str,
    server_port: u16,
    player_name: &str,
    client_id: &str,
) -> Result<Vec<PaperConnectPlayer>, String> {
    let mut stream = tokio::time::timeout(REQUEST_TIMEOUT, TcpStream::connect((host, server_port)))
        .await
        .map_err(|_| "连接 PaperConnect 联机中心超时".to_string())?
        .map_err(|error| format!("连接 PaperConnect 联机中心失败：{error}"))?;
    let request = format!(
        "c:player\0{}",
        serde_json::json!({
            "clientId": client_id,
            "playerName": player_name,
        })
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|error| format!("发送 PaperConnect c:player 失败：{error}"))?;
    let response: PlayerResponse = tokio::time::timeout(
        REQUEST_TIMEOUT,
        read_json_response(&mut stream, "PaperConnect c:player 响应"),
    )
    .await
    .map_err(|_| "等待 PaperConnect 玩家心跳响应超时".to_string())??;
    if response
        .players
        .iter()
        .any(|player| player.player.trim().is_empty() || player.client_id.trim().is_empty())
    {
        return Err("PaperConnect c:player 返回了无效玩家信息".to_string());
    }
    Ok(response.players)
}

fn handle_ping(
    body: &str,
    game_port: u16,
    protocol: PaperConnectProtocol,
) -> Result<String, String> {
    let request: PingRequest = serde_json::from_str(body)
        .map_err(|error| format!("PaperConnect c:ping 请求无效：{error}"))?;
    serde_json::to_string(&PingResponse {
        time: request.time,
        return_time: now_ms(),
        game_type: "MinecraftBedrock".to_string(),
        game_protocol_type: "UDP".to_string(),
        game_port,
        protocol: Some(protocol),
    })
    .map_err(|error| format!("序列化 PaperConnect c:ping 响应失败：{error}"))
}

fn handle_player(
    body: &str,
    players: Arc<Mutex<HashMap<String, PaperConnectPlayer>>>,
) -> Result<String, String> {
    let request: PlayerRequest = serde_json::from_str(body)
        .map_err(|error| format!("PaperConnect c:player 请求无效：{error}"))?;
    if request.client_id.trim().is_empty() || request.player_name.trim().is_empty() {
        return Err("PaperConnect c:player 缺少 clientId 或 playerName".to_string());
    }
    let now = now_ms();
    let mut players = players
        .lock()
        .map_err(|_| "PaperConnect 玩家状态锁已损坏".to_string())?;
    players.retain(|_, player| {
        player.is_room_host
            || now.saturating_sub(player.last_seen) <= PLAYER_EXPIRY.as_millis() as i64
    });
    let is_room_host = players
        .get(request.player_name.trim())
        .is_some_and(|player| player.is_room_host);
    players.insert(
        request.player_name.trim().to_string(),
        PaperConnectPlayer {
            player: request.player_name.trim().to_string(),
            client_id: request.client_id,
            is_room_host,
            last_seen: now,
        },
    );
    let active_players: Vec<_> = players.values().cloned().collect();
    replace_player_snapshot(active_players.clone());
    serde_json::to_string(&PlayerResponse {
        return_time: now,
        players: active_players,
    })
    .map_err(|error| format!("序列化 PaperConnect c:player 响应失败：{error}"))
}

fn prune_inactive_players(players: &Mutex<HashMap<String, PaperConnectPlayer>>) {
    let Ok(mut players) = players.lock() else {
        tracing::warn!("PaperConnect 玩家状态锁已损坏，跳过过期清理");
        return;
    };
    let previous_count = players.len();
    let now = now_ms();
    players.retain(|_, player| {
        player.is_room_host
            || now.saturating_sub(player.last_seen) <= PLAYER_EXPIRY.as_millis() as i64
    });
    if players.len() != previous_count {
        replace_player_snapshot(players.values().cloned().collect());
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        PaperConnectPlayer, PaperConnectProtocol, PlayerResponse, REQUEST_TIMEOUT,
        build_server_hostname, client_id, handle_player, now_ms, parse_server_hostname, ping,
        players as player_snapshot, read_request, send_player, server_port_from_hostname,
        start_client, start_server, stop_client, stop_server,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::{TcpListener, TcpStream};

    #[test]
    fn only_paperconnect_server_hostname_is_discoverable() {
        assert_eq!(
            server_port_from_hostname("paper-connect-server-19132"),
            Some(19132)
        );
        assert_eq!(
            server_port_from_hostname("other-protocol-server-19132"),
            None
        );
        assert_eq!(
            server_port_from_hostname("scaffolding-mc-server-19132"),
            None
        );
        assert_eq!(server_port_from_hostname("paper-connect-server-1024"), None);
        let hostname = build_server_hostname(22000, PaperConnectProtocol::Nethernet, 23000)
            .expect("v2 hostname should build");
        assert_eq!(hostname, "pcs-22000-g-23000");
        let parsed = parse_server_hostname(&hostname).expect("v2 hostname should parse");
        assert_eq!(parsed.server_port, 22000);
        assert_eq!(parsed.game_port, Some(23000));
        assert_eq!(parsed.protocol, Some(PaperConnectProtocol::Nethernet));
    }

    #[tokio::test]
    async fn local_server_answers_ping_with_bedrock_metadata() {
        crate::tasks::runtime::initialize_app_runtime().expect("initialize application runtime");
        let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("pick test port");
        let port = probe.local_addr().expect("read test port").port();
        drop(probe);

        start_server(
            port,
            19132,
            PaperConnectProtocol::Raknet,
            "房主玩家".to_string(),
        )
        .await
        .expect("start PaperConnect server");
        let response = ping("127.0.0.1", port)
            .await
            .expect("ping PaperConnect server");
        assert_eq!(response.game_type, "MinecraftBedrock");
        assert_eq!(response.game_protocol_type, "UDP");
        assert_eq!(response.server_port, port);
        assert_eq!(response.game_host, "127.0.0.1");
        assert_eq!(response.game_port, 19132);
        assert_eq!(response.protocol, PaperConnectProtocol::Raknet);
        let players = send_player("127.0.0.1", port, "房主玩家", &client_id())
            .await
            .expect("send host c:player heartbeat");
        assert!(players.iter().any(|player| {
            player.player == "房主玩家" && player.is_room_host && !player.client_id.is_empty()
        }));
        start_client("127.0.0.1".to_string(), port, "房客玩家".to_string())
            .await
            .expect("start guest c:player heartbeat");
        let snapshot = player_snapshot();
        assert!(
            snapshot
                .iter()
                .any(|player| player.player == "房主玩家" && player.is_room_host)
        );
        assert!(
            snapshot
                .iter()
                .any(|player| player.player == "房客玩家" && !player.is_room_host)
        );
        stop_client();
        stop_server();
    }

    #[test]
    fn player_heartbeat_returns_host_and_guest_metadata() {
        let players = Arc::new(Mutex::new(HashMap::from([(
            "Host".to_string(),
            PaperConnectPlayer {
                player: "Host".to_string(),
                client_id: "BMCBL host".to_string(),
                is_room_host: true,
                last_seen: now_ms(),
            },
        )])));
        let response = handle_player(
            r#"{"clientId":"PaperConnect 0.0.1","playerName":"Guest"}"#,
            players,
        )
        .expect("handle PaperConnect player heartbeat");
        let response: PlayerResponse =
            serde_json::from_str(&response).expect("parse player response");

        assert!(
            response
                .players
                .iter()
                .any(|player| player.player == "Host" && player.is_room_host)
        );
        assert!(response.players.iter().any(|player| {
            player.player == "Guest"
                && player.client_id == "PaperConnect 0.0.1"
                && !player.is_room_host
        }));
    }

    #[tokio::test]
    async fn request_reader_accepts_client_without_write_shutdown() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind request reader test listener");
        let address = listener.local_addr().expect("read request reader address");
        let client = TcpStream::connect(address)
            .await
            .expect("connect request reader test client");
        let (mut server, _) = listener.accept().await.expect("accept request reader test");
        let mut client = client;
        client
            .write_all(b"c:ping\0{\"time\":1}")
            .await
            .expect("write complete PaperConnect request");

        let request = tokio::time::timeout(REQUEST_TIMEOUT, read_request(&mut server))
            .await
            .expect("request reader should not wait for EOF")
            .expect("request should be valid");
        assert_eq!(request, "c:ping\0{\"time\":1}");
    }

    #[tokio::test]
    async fn ping_accepts_response_without_connection_close() {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind PaperConnect compatibility listener");
        let address = listener
            .local_addr()
            .expect("read PaperConnect compatibility address");
        let (release_server, keep_connection_open) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener
                .accept()
                .await
                .expect("accept PaperConnect compatibility request");
            let request = read_request(&mut stream)
                .await
                .expect("read PaperConnect compatibility request");
            assert!(request.starts_with("c:ping\0"));

            let mut trailing = [0_u8; 1];
            match tokio::time::timeout(
                std::time::Duration::from_millis(100),
                stream.read(&mut trailing),
            )
            .await
            {
                Err(_) => {}
                Ok(Ok(0)) => panic!("PaperConnect client closed its write half before response"),
                Ok(Ok(_)) => panic!("PaperConnect client sent unexpected trailing bytes"),
                Ok(Err(error)) => panic!("PaperConnect compatibility read failed: {error}"),
            }

            stream
                .write_all(
                    br#"{"time":1,"returnTime":2,"gameType":"MinecraftBedrock","gameProtocolType":"UDP","gamePort":19132}"#,
                )
                .await
                .expect("write PaperConnect compatibility response");
            keep_connection_open
                .await
                .expect("hold GravityCone-style persistent connection");
        });

        let response = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            ping("127.0.0.1", address.port()),
        )
        .await
        .expect("complete response should not wait for EOF")
        .expect("PaperConnect ping should wait with write half open");
        assert_eq!(response.game_port, 19132);
        assert_eq!(response.protocol, PaperConnectProtocol::Raknet);
        release_server
            .send(())
            .expect("release compatibility server connection");
        server
            .await
            .expect("join PaperConnect compatibility server");
    }
}
