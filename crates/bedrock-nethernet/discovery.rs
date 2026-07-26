use crate::{NethernetError, Result};
use aes::Aes256;
use aes::cipher::{Block, BlockDecrypt, BlockEncrypt, KeyInit};
use hmac::{Hmac, Mac};
use rand::RngExt as _;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;
use tokio::net::UdpSocket;
use tokio::sync::{Notify, RwLock as AsyncRwLock, broadcast};
use tokio::task::JoinHandle;
use tokio::time::Instant;

const REQUEST_PACKET: u16 = 0;
const RESPONSE_PACKET: u16 = 1;
const MESSAGE_PACKET: u16 = 2;
const CHECKSUM_SIZE: usize = 32;
const HEADER_SIZE: usize = 18;
const MAX_DISCOVERY_PACKET: usize = u16::MAX as usize + CHECKSUM_SIZE + 18;
const MAX_SIGNAL_SIZE: usize = 60 * 1024;

static ENCRYPTION_KEY: LazyLock<[u8; 32]> = LazyLock::new(|| {
    let mut hasher = Sha256::new();
    hasher.update(0xdead_beef_u64.to_le_bytes());
    hasher.finalize().into()
});

#[derive(Debug, Clone, Eq, PartialEq)]
// These booleans mirror GravityCone's ServerData v5 wire fields one-to-one.
#[allow(clippy::struct_excessive_bools)]
pub struct ServerData {
    pub server_name: String,
    pub level_name: String,
    pub game_type: i32,
    pub player_count: i32,
    pub max_player_count: i32,
    pub editor_world: bool,
    pub hardcore: bool,
    pub accepts_online_auth: bool,
    pub accepts_self_signed_auth: bool,
    pub transport_layer: i32,
    pub connection_type: i32,
}

impl ServerData {
    /// Serializes the GravityCone-compatible `ServerData` v5 payload.
    ///
    /// # Errors
    ///
    /// Returns an error when either string exceeds the protocol length limit.
    pub fn marshal(&self) -> Result<Vec<u8>> {
        let mut data = Vec::with_capacity(32 + self.server_name.len() + self.level_name.len());
        data.push(5);
        write_var_bytes(&mut data, self.server_name.as_bytes())?;
        write_var_bytes(&mut data, self.level_name.as_bytes())?;
        write_var_i32(&mut data, self.game_type);
        data.extend_from_slice(&self.player_count.to_le_bytes());
        data.extend_from_slice(&self.max_player_count.to_le_bytes());
        data.push(u8::from(self.editor_world));
        data.push(u8::from(self.hardcore));
        data.push(u8::from(self.accepts_online_auth));
        data.push(u8::from(self.accepts_self_signed_auth));
        write_var_i32(&mut data, self.transport_layer);
        write_var_i32(&mut data, self.connection_type);
        Ok(data)
    }

    /// Parses `GravityCone` `ServerData` v5 and legacy v4 payloads.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, truncated, or unsupported payloads.
    pub fn unmarshal(data: &[u8]) -> Result<Self> {
        let mut cursor = Cursor::new(data);
        match cursor.read_u8()? {
            5 => Self::unmarshal_v5(&mut cursor),
            4 => Self::unmarshal_v4(&mut cursor),
            version => Err(NethernetError::Protocol(format!(
                "不支持的 NetherNet ServerData 版本：{version}"
            ))),
        }
    }

    fn unmarshal_v5(cursor: &mut Cursor<'_>) -> Result<Self> {
        let server_name = cursor.read_string_var()?;
        let level_name = cursor.read_string_var()?;
        let game_type = cursor.read_var_i32()?;
        let player_count = cursor.read_i32_le()?;
        let max_player_count = cursor.read_i32_le()?;
        let editor_world = cursor.read_u8()? != 0;
        let hardcore = cursor.read_u8()? != 0;
        let accepts_online_auth = cursor.read_u8()? != 0;
        let accepts_self_signed_auth = cursor.read_u8()? != 0;
        let transport_layer = cursor.read_var_i32()?;
        let connection_type = cursor.read_var_i32()?;
        if !cursor.is_empty() {
            return Err(NethernetError::Protocol(
                "NetherNet ServerData 含有尾随数据".to_string(),
            ));
        }
        Ok(Self {
            server_name,
            level_name,
            game_type,
            player_count,
            max_player_count,
            editor_world,
            hardcore,
            accepts_online_auth,
            accepts_self_signed_auth,
            transport_layer,
            connection_type,
        })
    }

    fn unmarshal_v4(cursor: &mut Cursor<'_>) -> Result<Self> {
        let server_name = cursor.read_string_u8()?;
        let level_name = cursor.read_string_u8()?;
        let game_type = i32::from(cursor.read_u8()? >> 1);
        let player_count = cursor.read_i32_le()?;
        let max_player_count = cursor.read_i32_le()?;
        let editor_world = cursor.read_u8()? != 0;
        let hardcore = cursor.read_u8()? != 0;
        let transport_layer = i32::from(cursor.read_u8()? >> 1);
        let connection_type = i32::from(cursor.read_u8()? >> 1);
        if !cursor.is_empty() {
            return Err(NethernetError::Protocol(
                "NetherNet v4 ServerData 含有尾随数据".to_string(),
            ));
        }
        Ok(Self {
            server_name,
            level_name,
            game_type,
            player_count,
            max_player_count,
            editor_world,
            hardcore,
            accepts_online_auth: true,
            accepts_self_signed_auth: true,
            transport_layer,
            connection_type,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub network_id: u64,
    pub address: SocketAddr,
    pub server_data: ServerData,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum SignalType {
    Offer,
    Answer,
    Candidate,
    Error,
}

#[derive(Debug, Clone)]
pub(crate) struct Signal {
    pub kind: SignalType,
    pub connection_id: u64,
    pub data: String,
    pub network_id: u64,
}

impl Signal {
    pub fn encode(&self) -> String {
        let signal_type = match self.kind {
            SignalType::Offer => "CONNECTREQUEST",
            SignalType::Answer => "CONNECTRESPONSE",
            SignalType::Candidate => "CANDIDATEADD",
            SignalType::Error => "CONNECTERROR",
        };
        format!("{signal_type} {} {}", self.connection_id, self.data)
    }

    fn decode(data: &str, network_id: u64) -> Result<Self> {
        let mut fields = data.splitn(3, ' ');
        let signal_type = match fields.next() {
            Some("CONNECTREQUEST") => SignalType::Offer,
            Some("CONNECTRESPONSE") => SignalType::Answer,
            Some("CANDIDATEADD") => SignalType::Candidate,
            Some("CONNECTERROR") => SignalType::Error,
            Some(value) => {
                return Err(NethernetError::Protocol(format!(
                    "未知 NetherNet 信令类型：{value}"
                )));
            }
            None => return Err(NethernetError::Protocol("NetherNet 信令为空".to_string())),
        };
        let connection_id = fields
            .next()
            .ok_or_else(|| NethernetError::Protocol("NetherNet 信令缺少连接编号".to_string()))?
            .parse::<u64>()
            .map_err(|error| {
                NethernetError::Protocol(format!("NetherNet 连接编号无效：{error}"))
            })?;
        let data = fields
            .next()
            .ok_or_else(|| NethernetError::Protocol("NetherNet 信令缺少数据".to_string()))?
            .to_string();
        Ok(Self {
            kind: signal_type,
            connection_id,
            data,
            network_id,
        })
    }
}

pub struct LanSignaling {
    network_id: u64,
    socket: Arc<UdpSocket>,
    target: Option<SocketAddr>,
    addresses: Arc<AsyncRwLock<HashMap<u64, SocketAddr>>>,
    discovered: Arc<AsyncRwLock<HashMap<u64, ServerData>>>,
    discovered_notify: Arc<Notify>,
    signal_sender: broadcast::Sender<Signal>,
    background_task: Mutex<Option<JoinHandle<()>>>,
}

impl LanSignaling {
    /// Binds a discovery client and directs discovery packets to `target`.
    ///
    /// # Errors
    ///
    /// Returns an error when the UDP socket cannot be bound or configured.
    pub async fn client(bind_addr: SocketAddr, target: SocketAddr) -> Result<Self> {
        Self::new(bind_addr, Some(target), None).await
    }

    /// Binds a discovery server that advertises `server_data`.
    ///
    /// # Errors
    ///
    /// Returns an error when the UDP socket cannot be bound or configured.
    pub async fn server(bind_addr: SocketAddr, server_data: ServerData) -> Result<Self> {
        Self::new(bind_addr, None, Some(server_data)).await
    }

    async fn new(
        bind_addr: SocketAddr,
        target: Option<SocketAddr>,
        server_data: Option<ServerData>,
    ) -> Result<Self> {
        let socket = Arc::new(UdpSocket::bind(bind_addr).await?);
        socket.set_broadcast(true)?;
        let network_id = rand::rng().random::<u64>();
        let addresses = Arc::new(AsyncRwLock::new(HashMap::new()));
        let discovered = Arc::new(AsyncRwLock::new(HashMap::new()));
        let discovered_notify = Arc::new(Notify::new());
        let server_data = Arc::new(RwLock::new(server_data));
        let (signal_sender, _) = broadcast::channel(256);
        let background_task = tokio::spawn({
            let socket = Arc::clone(&socket);
            let addresses = Arc::clone(&addresses);
            let discovered = Arc::clone(&discovered);
            let discovered_notify = Arc::clone(&discovered_notify);
            let server_data = Arc::clone(&server_data);
            let signal_sender = signal_sender.clone();
            async move {
                let mut buffer = vec![0_u8; MAX_DISCOVERY_PACKET];
                loop {
                    let Ok((length, source)) = socket.recv_from(&mut buffer).await else {
                        break;
                    };
                    if let Err(error) = handle_packet(
                        &buffer[..length],
                        source,
                        network_id,
                        &socket,
                        &addresses,
                        &discovered,
                        &discovered_notify,
                        &signal_sender,
                        &server_data,
                    )
                    .await
                    {
                        tracing::trace!(%source, "忽略无效 NetherNet 数据：{error}");
                    }
                }
            }
        });
        Ok(Self {
            network_id,
            socket,
            target,
            addresses,
            discovered,
            discovered_notify,
            signal_sender,
            background_task: Mutex::new(Some(background_task)),
        })
    }

    /// Discovers the first `NetherNet` server before `timeout` expires.
    ///
    /// # Errors
    ///
    /// Returns an error when discovery times out or UDP transport fails.
    pub async fn discover(&self, timeout: Duration) -> Result<DiscoveredServer> {
        let target = self
            .target
            .ok_or_else(|| NethernetError::Protocol("服务端信令不能主动发现房间".to_string()))?;
        let request = encode_packet(REQUEST_PACKET, self.network_id, &[])?;
        let deadline = Instant::now() + timeout;
        loop {
            let discovered = self.discovered_notify.notified();
            self.socket.send_to(&request, target).await?;
            if let Some(server) = self.first_discovered().await {
                return Ok(server);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(NethernetError::Timeout);
            }
            let retry_delay = Duration::from_millis(250).min(deadline.duration_since(now));
            if tokio::time::timeout(retry_delay, discovered).await.is_ok()
                && let Some(server) = self.first_discovered().await
            {
                return Ok(server);
            }
        }
    }

    /// Returns the UDP address selected by the operating system.
    ///
    /// # Errors
    ///
    /// Returns an error when the local socket address is unavailable.
    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.socket.local_addr()?)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Signal> {
        self.signal_sender.subscribe()
    }

    pub(crate) async fn send_signal(&self, signal: &Signal) -> Result<()> {
        let destination = self
            .addresses
            .read()
            .await
            .get(&signal.network_id)
            .copied()
            .ok_or_else(|| {
                NethernetError::Protocol(format!(
                    "未找到 NetherNet 节点 {} 的地址",
                    signal.network_id
                ))
            })?;
        let encoded_signal = signal.encode();
        if encoded_signal.len() > MAX_SIGNAL_SIZE {
            return Err(NethernetError::Protocol(
                "NetherNet 信令数据过大".to_string(),
            ));
        }
        let signal_length = u32::try_from(encoded_signal.len())
            .map_err(|_| NethernetError::Protocol("NetherNet 信令数据过大".to_string()))?;
        let mut body = Vec::with_capacity(12 + encoded_signal.len());
        body.extend_from_slice(&signal.network_id.to_le_bytes());
        body.extend_from_slice(&signal_length.to_le_bytes());
        body.extend_from_slice(encoded_signal.as_bytes());
        let packet = encode_packet(MESSAGE_PACKET, self.network_id, &body)?;
        self.socket.send_to(&packet, destination).await?;
        Ok(())
    }

    async fn first_discovered(&self) -> Option<DiscoveredServer> {
        let discovered = self.discovered.read().await;
        let (&network_id, server_data) = discovered.iter().next()?;
        let address = self.addresses.read().await.get(&network_id).copied()?;
        Some(DiscoveredServer {
            network_id,
            address,
            server_data: server_data.clone(),
        })
    }
}

impl Drop for LanSignaling {
    fn drop(&mut self) {
        if let Ok(mut task) = self.background_task.lock()
            && let Some(task) = task.take()
        {
            task.abort();
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_packet(
    data: &[u8],
    source: SocketAddr,
    own_network_id: u64,
    socket: &UdpSocket,
    addresses: &AsyncRwLock<HashMap<u64, SocketAddr>>,
    discovered: &AsyncRwLock<HashMap<u64, ServerData>>,
    discovered_notify: &Notify,
    signal_sender: &broadcast::Sender<Signal>,
    server_data: &RwLock<Option<ServerData>>,
) -> Result<()> {
    let packet = decode_packet(data)?;
    if packet.sender_id == own_network_id {
        return Ok(());
    }
    addresses.write().await.insert(packet.sender_id, source);
    match packet.packet_id {
        REQUEST_PACKET => {
            let server_data = server_data
                .read()
                .map_err(|_| NethernetError::Protocol("ServerData 锁已损坏".to_string()))?
                .clone();
            let Some(server_data) = server_data else {
                return Ok(());
            };
            let server_data = server_data.marshal()?;
            let hexadecimal = hex_encode(&server_data);
            let length = u32::try_from(hexadecimal.len())
                .map_err(|_| NethernetError::Protocol("ServerData 响应过大".to_string()))?;
            let mut body = Vec::with_capacity(4 + hexadecimal.len());
            body.extend_from_slice(&length.to_le_bytes());
            body.extend_from_slice(&hexadecimal);
            let response = encode_packet(RESPONSE_PACKET, own_network_id, &body)?;
            socket.send_to(&response, source).await?;
        }
        RESPONSE_PACKET => {
            let mut cursor = Cursor::new(&packet.body);
            let length = usize::try_from(cursor.read_u32_le()?)
                .map_err(|_| NethernetError::Protocol("ServerData 长度溢出".to_string()))?;
            let hexadecimal = cursor.read_exact(length)?;
            if !cursor.is_empty() {
                return Err(NethernetError::Protocol(
                    "ServerData 响应含有尾随数据".to_string(),
                ));
            }
            let server_data = ServerData::unmarshal(&hex_decode(hexadecimal)?)?;
            discovered
                .write()
                .await
                .insert(packet.sender_id, server_data);
            discovered_notify.notify_one();
        }
        MESSAGE_PACKET => {
            let mut cursor = Cursor::new(&packet.body);
            let recipient_id = cursor.read_u64_le()?;
            let length = usize::try_from(cursor.read_u32_le()?)
                .map_err(|_| NethernetError::Protocol("信令长度溢出".to_string()))?;
            if length > MAX_SIGNAL_SIZE {
                return Err(NethernetError::Protocol("信令数据过大".to_string()));
            }
            let signal = std::str::from_utf8(cursor.read_exact(length)?)
                .map_err(|error| NethernetError::Protocol(format!("信令不是 UTF-8：{error}")))?;
            if recipient_id == own_network_id {
                let signal = Signal::decode(signal, packet.sender_id)?;
                match signal_sender.send(signal) {
                    Ok(_) => {}
                    Err(error) => tracing::trace!("NetherNet 信令没有接收方：{error}"),
                }
            }
        }
        value => {
            return Err(NethernetError::Protocol(format!(
                "未知发现数据包类型：{value}"
            )));
        }
    }
    Ok(())
}

struct DecodedPacket {
    packet_id: u16,
    sender_id: u64,
    body: Vec<u8>,
}

fn encode_packet(packet_id: u16, sender_id: u64, body: &[u8]) -> Result<Vec<u8>> {
    let payload_body_length = HEADER_SIZE
        .checked_add(body.len())
        .ok_or_else(|| NethernetError::Protocol("发现数据包长度溢出".to_string()))?;
    let payload_length = payload_body_length
        .checked_add(2)
        .ok_or_else(|| NethernetError::Protocol("发现数据包长度溢出".to_string()))?;
    let payload_length_u16 = u16::try_from(payload_length)
        .map_err(|_| NethernetError::Protocol("发现数据包过大".to_string()))?;
    let mut payload = Vec::with_capacity(payload_length + 16);
    payload.extend_from_slice(&payload_length_u16.to_le_bytes());
    payload.extend_from_slice(&packet_id.to_le_bytes());
    payload.extend_from_slice(&sender_id.to_le_bytes());
    payload.extend_from_slice(&[0_u8; 8]);
    payload.extend_from_slice(body);
    let checksum = compute_checksum(&payload)?;
    encrypt(&mut payload)?;
    let mut packet = Vec::with_capacity(CHECKSUM_SIZE + payload.len());
    packet.extend_from_slice(&checksum);
    packet.extend_from_slice(&payload);
    Ok(packet)
}

fn decode_packet(data: &[u8]) -> Result<DecodedPacket> {
    if data.len() < CHECKSUM_SIZE + 16 || data.len() > MAX_DISCOVERY_PACKET {
        return Err(NethernetError::Protocol(
            "NetherNet 发现数据包长度无效".to_string(),
        ));
    }
    let checksum: [u8; CHECKSUM_SIZE] = data[..CHECKSUM_SIZE]
        .try_into()
        .map_err(|_| NethernetError::Protocol("发现数据校验字段无效".to_string()))?;
    let mut payload = data[CHECKSUM_SIZE..].to_vec();
    decrypt(&mut payload)?;
    verify_checksum(&payload, &checksum)?;
    let mut cursor = Cursor::new(&payload);
    let declared_length = usize::from(cursor.read_u16_le()?);
    let inclusive_length = payload.len();
    let legacy_exclusive_length = payload.len().saturating_sub(2);
    if !matches!(declared_length, length if length == inclusive_length || length == legacy_exclusive_length)
        || declared_length < HEADER_SIZE
    {
        return Err(NethernetError::Protocol(
            "发现数据包声明长度不匹配".to_string(),
        ));
    }
    let packet_id = cursor.read_u16_le()?;
    let sender_id = cursor.read_u64_le()?;
    cursor.read_exact(8)?;
    Ok(DecodedPacket {
        packet_id,
        sender_id,
        body: cursor.remaining().to_vec(),
    })
}

fn encrypt(data: &mut Vec<u8>) -> Result<()> {
    let padding_length = 16 - data.len() % 16;
    let padding_byte = u8::try_from(padding_length)
        .map_err(|_| NethernetError::Protocol("AES 填充长度溢出".to_string()))?;
    data.resize(data.len() + padding_length, padding_byte);
    let cipher = Aes256::new_from_slice(ENCRYPTION_KEY.as_slice())
        .map_err(|error| NethernetError::Protocol(format!("AES 密钥无效：{error}")))?;
    for chunk in data.chunks_exact_mut(16) {
        cipher.encrypt_block(Block::<Aes256>::from_mut_slice(chunk));
    }
    Ok(())
}

fn decrypt(data: &mut Vec<u8>) -> Result<()> {
    if data.is_empty() || data.len() % 16 != 0 {
        return Err(NethernetError::Protocol("AES 数据长度无效".to_string()));
    }
    let cipher = Aes256::new_from_slice(ENCRYPTION_KEY.as_slice())
        .map_err(|error| NethernetError::Protocol(format!("AES 密钥无效：{error}")))?;
    for chunk in data.chunks_exact_mut(16) {
        cipher.decrypt_block(Block::<Aes256>::from_mut_slice(chunk));
    }
    let padding_length = usize::from(
        *data
            .last()
            .ok_or_else(|| NethernetError::Protocol("AES 填充数据缺失".to_string()))?,
    );
    if padding_length == 0
        || padding_length > 16
        || padding_length > data.len()
        || data[data.len() - padding_length..]
            .iter()
            .any(|byte| usize::from(*byte) != padding_length)
    {
        return Err(NethernetError::Protocol("AES 填充数据无效".to_string()));
    }
    data.truncate(data.len() - padding_length);
    Ok(())
}

fn compute_checksum(data: &[u8]) -> Result<[u8; CHECKSUM_SIZE]> {
    let mut hmac = <Hmac<Sha256> as Mac>::new_from_slice(ENCRYPTION_KEY.as_slice())
        .map_err(|error| NethernetError::Protocol(format!("HMAC 密钥无效：{error}")))?;
    hmac.update(data);
    Ok(hmac.finalize().into_bytes().into())
}

fn verify_checksum(data: &[u8], checksum: &[u8; CHECKSUM_SIZE]) -> Result<()> {
    let mut hmac = <Hmac<Sha256> as Mac>::new_from_slice(ENCRYPTION_KEY.as_slice())
        .map_err(|error| NethernetError::Protocol(format!("HMAC 密钥无效：{error}")))?;
    hmac.update(data);
    hmac.verify_slice(checksum)
        .map_err(|_| NethernetError::Protocol("NetherNet 数据校验失败".to_string()))
}

fn write_var_bytes(output: &mut Vec<u8>, data: &[u8]) -> Result<()> {
    let length = u32::try_from(data.len())
        .map_err(|_| NethernetError::Protocol("ServerData 字符串过长".to_string()))?;
    write_var_u32(output, length);
    output.extend_from_slice(data);
    Ok(())
}

fn write_var_i32(output: &mut Vec<u8>, value: i32) {
    let encoded = (value.unsigned_abs() << 1).wrapping_sub(u32::from(value.is_negative()));
    write_var_u32(output, encoded);
}

fn write_var_u32(output: &mut Vec<u8>, mut value: u32) {
    while value >= 0x80 {
        output.push((value.to_le_bytes()[0] & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value.to_le_bytes()[0]);
}

fn hex_encode(data: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = Vec::with_capacity(data.len() * 2);
    for byte in data {
        output.push(HEX[usize::from(byte >> 4)]);
        output.push(HEX[usize::from(byte & 0x0f)]);
    }
    output
}

fn hex_decode(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() % 2 != 0 {
        return Err(NethernetError::Protocol(
            "ServerData 十六进制长度无效".to_string(),
        ));
    }
    data.chunks_exact(2)
        .map(|pair| {
            let high = hex_digit(pair[0])?;
            let low = hex_digit(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_digit(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(NethernetError::Protocol(
            "ServerData 十六进制字符无效".to_string(),
        )),
    }
}

struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| NethernetError::Protocol("协议游标溢出".to_string()))?;
        let value = self
            .data
            .get(self.position..end)
            .ok_or_else(|| NethernetError::Protocol("协议数据不完整".to_string()))?;
        self.position = end;
        Ok(value)
    }

    fn read_u8(&mut self) -> Result<u8> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16_le(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.read_exact(2)?.try_into().map_err(
            |_| NethernetError::Protocol("u16 字段无效".to_string()),
        )?))
    }

    fn read_u32_le(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read_exact(4)?.try_into().map_err(
            |_| NethernetError::Protocol("u32 字段无效".to_string()),
        )?))
    }

    fn read_u64_le(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.read_exact(8)?.try_into().map_err(
            |_| NethernetError::Protocol("u64 字段无效".to_string()),
        )?))
    }

    fn read_i32_le(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read_exact(4)?.try_into().map_err(
            |_| NethernetError::Protocol("i32 字段无效".to_string()),
        )?))
    }

    fn read_var_u32(&mut self) -> Result<u32> {
        let mut value = 0_u32;
        for shift in (0..35).step_by(7) {
            let byte = self.read_u8()?;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(NethernetError::Protocol(
            "ServerData varuint32 未在五字节内结束".to_string(),
        ))
    }

    fn read_var_i32(&mut self) -> Result<i32> {
        let value = self.read_var_u32()?;
        let magnitude = i32::from_le_bytes((value >> 1).to_le_bytes());
        let sign = 0_i32.wrapping_sub(i32::from(value.to_le_bytes()[0] & 1));
        Ok(magnitude ^ sign)
    }

    fn read_string_u8(&mut self) -> Result<String> {
        let length = usize::from(self.read_u8()?);
        String::from_utf8(self.read_exact(length)?.to_vec()).map_err(|error| {
            NethernetError::Protocol(format!("ServerData 字符串不是 UTF-8：{error}"))
        })
    }

    fn read_string_var(&mut self) -> Result<String> {
        let length = usize::try_from(self.read_var_u32()?)
            .map_err(|_| NethernetError::Protocol("ServerData 字符串长度溢出".to_string()))?;
        String::from_utf8(self.read_exact(length)?.to_vec()).map_err(|error| {
            NethernetError::Protocol(format!("ServerData 字符串不是 UTF-8：{error}"))
        })
    }

    fn remaining(&self) -> &'a [u8] {
        &self.data[self.position..]
    }

    fn is_empty(&self) -> bool {
        self.position == self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{ServerData, decode_packet, encode_packet};

    #[test]
    fn server_data_round_trips() {
        let original = ServerData {
            server_name: "BMCBL".to_string(),
            level_name: "PaperConnect".to_string(),
            game_type: 1,
            player_count: 2,
            max_player_count: 20,
            editor_world: false,
            hardcore: true,
            accepts_online_auth: false,
            accepts_self_signed_auth: true,
            transport_layer: 2,
            connection_type: 4,
        };
        let decoded = ServerData::unmarshal(&original.marshal().expect("marshal ServerData"))
            .expect("unmarshal ServerData");
        assert_eq!(decoded, original);
    }

    #[test]
    fn server_data_zigzag_boundaries_round_trip() {
        let original = ServerData {
            server_name: String::new(),
            level_name: String::new(),
            game_type: i32::MIN,
            player_count: i32::MIN,
            max_player_count: i32::MAX,
            editor_world: false,
            hardcore: false,
            accepts_online_auth: true,
            accepts_self_signed_auth: true,
            transport_layer: i32::MAX,
            connection_type: -1,
        };
        let decoded = ServerData::unmarshal(&original.marshal().expect("marshal ServerData"))
            .expect("unmarshal ServerData");
        assert_eq!(decoded, original);
    }

    #[test]
    fn server_data_matches_gravitycone_v5_vector() {
        let server_data = ServerData {
            server_name: "server".to_string(),
            level_name: "world".to_string(),
            game_type: 2,
            player_count: 1,
            max_player_count: 8,
            editor_world: false,
            hardcore: true,
            accepts_online_auth: true,
            accepts_self_signed_auth: true,
            transport_layer: 2,
            connection_type: 4,
        };
        let expected = [
            0x05, 0x06, b's', b'e', b'r', b'v', b'e', b'r', 0x05, b'w', b'o', b'r', b'l', b'd',
            0x04, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x04,
            0x08,
        ];
        assert_eq!(server_data.marshal().expect("marshal ServerData"), expected);
    }

    #[test]
    fn reads_legacy_v4_server_data() {
        let legacy = [
            4, 5, b'B', b'M', b'C', b'B', b'L', 5, b'W', b'o', b'r', b'l', b'd', 2, 1, 0, 0, 0, 20,
            0, 0, 0, 0, 0, 4, 8,
        ];
        let decoded = ServerData::unmarshal(&legacy).expect("unmarshal v4 ServerData");
        assert_eq!(decoded.server_name, "BMCBL");
        assert_eq!(decoded.game_type, 1);
        assert!(decoded.accepts_online_auth);
        assert!(decoded.accepts_self_signed_auth);
    }

    #[test]
    fn encrypted_packet_round_trips() {
        let packet = encode_packet(2, 42, b"payload").expect("encode packet");
        let decoded = decode_packet(&packet).expect("decode packet");
        assert_eq!(decoded.packet_id, 2);
        assert_eq!(decoded.sender_id, 42);
        assert_eq!(decoded.body, b"payload");
    }

    #[test]
    fn discovery_packet_writes_inclusive_length() {
        let packet = encode_packet(0, 42, &[]).expect("encode packet");
        let mut payload = packet[32..].to_vec();
        super::decrypt(&mut payload).expect("decrypt packet");
        assert_eq!(
            u16::from_le_bytes([payload[0], payload[1]]) as usize,
            payload.len()
        );
    }
}
