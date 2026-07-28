//! 局域网发现报文的编解码。
//!
//! 线上布局（与 go-nethernet `discovery/packet.go` 一致）：
//!
//! ```text
//! [HMAC-SHA256(明文) : 32] [AES-256-ECB(PKCS7(明文)) : N]
//!
//! 明文 = [总长 u16le（含自身 2 字节）]
//!        [报文 ID u16le] [发送方网络 ID u64le] [保留填充 8 字节]
//!        [body]
//! ```

use crate::consts::{
    CHECKSUM_SIZE, ID_MESSAGE_PACKET, ID_REQUEST_PACKET, ID_RESPONSE_PACKET, MAX_DISCOVERY_PACKET,
    MAX_PAYLOAD_LENGTH, MAX_SIGNAL_SIZE,
};
use crate::error::{NethernetError, Result};
use crate::protocol::codec::{Rd, put_u32_bytes};
use crate::protocol::crypto;
use bytes::{BufMut, Bytes, BytesMut};

/// 发现报文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryPacket {
    /// 客户端广播的世界查询请求。
    Request,
    /// 服务端应答，`application_data` 通常是 [`ServerData`] 编码。
    ///
    /// [`ServerData`]: crate::protocol::ServerData
    Response { application_data: Bytes },
    /// 信令投递。
    Message { recipient_id: u64, data: String },
}

impl DiscoveryPacket {
    #[must_use]
    pub const fn id(&self) -> u16 {
        match self {
            Self::Request => ID_REQUEST_PACKET,
            Self::Response { .. } => ID_RESPONSE_PACKET,
            Self::Message { .. } => ID_MESSAGE_PACKET,
        }
    }

    fn encode_body(&self, buf: &mut BytesMut) -> Result<()> {
        match self {
            Self::Request => Ok(()),
            Self::Response { application_data } => {
                // 应用数据在线上是十六进制文本。
                let hex = hex_encode(application_data);
                put_u32_bytes(buf, &hex)
            }
            Self::Message { recipient_id, data } => {
                buf.put_u64_le(*recipient_id);
                put_u32_bytes(buf, data.as_bytes())
            }
        }
    }

    fn decode_body(id: u16, rd: &mut Rd) -> Result<Self> {
        let packet = match id {
            ID_REQUEST_PACKET => Self::Request,
            ID_RESPONSE_PACKET => {
                let length = read_length(rd)?;
                let hex = rd.take(length)?;
                Self::Response {
                    application_data: hex_decode(&hex)?,
                }
            }
            ID_MESSAGE_PACKET => {
                let recipient_id = rd.u64_le()?;
                let length = read_length(rd)?;
                if length > MAX_SIGNAL_SIZE {
                    return Err(NethernetError::TooLarge {
                        size: length,
                        max: MAX_SIGNAL_SIZE,
                    });
                }
                let data = rd.take(length)?;
                Self::Message {
                    recipient_id,
                    data: String::from_utf8(data.to_vec()).map_err(|error| {
                        NethernetError::protocol(format!("信令不是 UTF-8：{error}"))
                    })?,
                }
            }
            other => {
                return Err(NethernetError::protocol(format!(
                    "未知发现报文 ID：{other}"
                )));
            }
        };
        // 真实 Bedrock 的 Message/Response 有时会在内层声明长度之后附加填充。
        // go-nethernet 同样只解析已声明的字段并忽略剩余字节；HMAC 已覆盖整个
        // 明文，因此容忍这些字节不会绕过完整性校验。
        Ok(packet)
    }

    /// 编码、加密并加上校验和。
    ///
    /// # Errors
    ///
    /// 报文体超过 u16 长度前缀的表示范围时返回错误。
    pub fn encode(&self, sender_id: u64) -> Result<Bytes> {
        let mut plaintext = BytesMut::with_capacity(64);
        plaintext.put_u16_le(0); // 长度占位
        plaintext.put_u16_le(self.id());
        plaintext.put_u64_le(sender_id);
        plaintext.put_bytes(0, 8); // 保留填充
        self.encode_body(&mut plaintext)?;

        // vanilla 统计的是整个明文长度（含这 2 字节前缀本身）。
        let total = plaintext.len();
        let declared = u16::try_from(total).map_err(|_| NethernetError::TooLarge {
            size: total,
            max: MAX_PAYLOAD_LENGTH,
        })?;
        plaintext[..2].copy_from_slice(&declared.to_le_bytes());

        let mut payload = plaintext.to_vec();
        let mac = crypto::checksum(&payload);
        crypto::encrypt_in_place(&mut payload);

        let mut packet = BytesMut::with_capacity(CHECKSUM_SIZE + payload.len());
        packet.put_slice(&mac);
        packet.put_slice(&payload);
        Ok(packet.freeze())
    }

    /// 校验、解密并解析，返回（报文, 发送方网络 ID）。
    ///
    /// # Errors
    ///
    /// 长度非法、校验失败、解密失败或字段截断时返回错误。
    pub fn decode(data: &[u8]) -> Result<(Self, u64)> {
        if data.len() < CHECKSUM_SIZE + 16 || data.len() > MAX_DISCOVERY_PACKET {
            return Err(NethernetError::protocol(format!(
                "发现报文长度非法：{}",
                data.len()
            )));
        }
        let (mac, ciphertext) = data.split_at(CHECKSUM_SIZE);
        let mut payload = ciphertext.to_vec();
        crypto::decrypt_in_place(&mut payload)?;
        crypto::verify_checksum(&payload, mac)?;

        let payload_len = payload.len();
        let mut rd = Rd::new(Bytes::from(payload));
        // 长度前缀读完即弃：真实边界由密文长度决定，body 取自剩余字节。
        // 各实现对该字段的口径不一（含/不含自身 2 字节），go-nethernet
        // 直接跳过不校验（discovery/packet.go:67）。这里跟随，避免因
        // 口径差异丢掉合法报文。
        let declared = usize::from(rd.u16_le()?);
        if declared != payload_len && declared != payload_len.saturating_sub(2) {
            tracing::trace!(
                declared,
                actual = payload_len,
                "发现报文长度前缀口径不同，已忽略"
            );
        }
        let id = rd.u16_le()?;
        let sender_id = rd.u64_le()?;
        rd.skip(8)?; // 保留填充
        Ok((Self::decode_body(id, &mut rd)?, sender_id))
    }
}

fn read_length(rd: &mut Rd) -> Result<usize> {
    let length = rd.u32_le()? as usize;
    if length > rd.remaining() {
        return Err(NethernetError::Truncated {
            needed: length,
            remaining: rd.remaining(),
        });
    }
    Ok(length)
}

fn hex_encode(data: &[u8]) -> Vec<u8> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = Vec::with_capacity(data.len() * 2);
    for byte in data {
        out.push(HEX[usize::from(byte >> 4)]);
        out.push(HEX[usize::from(byte & 0x0f)]);
    }
    out
}

fn hex_decode(data: &[u8]) -> Result<Bytes> {
    if data.len() % 2 != 0 {
        return Err(NethernetError::protocol("十六进制载荷长度为奇数"));
    }
    let mut out = BytesMut::with_capacity(data.len() / 2);
    for pair in data.chunks_exact(2) {
        out.put_u8((hex_digit(pair[0])? << 4) | hex_digit(pair[1])?);
    }
    Ok(out.freeze())
}

fn hex_digit(value: u8) -> Result<u8> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(NethernetError::protocol("十六进制字符非法")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::HEADER_SIZE;
    use crate::protocol::ServerData;

    #[test]
    fn request_round_trips() {
        let (packet, sender) =
            DiscoveryPacket::decode(&DiscoveryPacket::Request.encode(42).unwrap()).unwrap();
        assert_eq!(packet, DiscoveryPacket::Request);
        assert_eq!(sender, 42);
    }

    #[test]
    fn response_round_trips_server_data() {
        let data = ServerData::default();
        let original = DiscoveryPacket::Response {
            application_data: data.encode().unwrap(),
        };
        let (packet, sender) = DiscoveryPacket::decode(&original.encode(7).unwrap()).unwrap();
        assert_eq!(packet, original);
        assert_eq!(sender, 7);
        let DiscoveryPacket::Response { application_data } = packet else {
            panic!("类型不符");
        };
        assert_eq!(ServerData::decode(application_data).unwrap(), data);
    }

    #[test]
    fn response_matches_gravitycone_wire_vector() {
        let application_data =
            hex_decode(b"050673657276657205776f726c64040100000008000000000101010408").unwrap();
        let response = DiscoveryPacket::Response {
            application_data: application_data.clone(),
        };
        let expected = hex_decode(
            b"83fa57c952db80741b7c86c8a1ab1cd99c0b4f05f1b7492504c5bfb5603fd4c9\
              7246c0d4857a93589e058bc307e2ac104321c241ed41ad8f82a21ab0fa2e6ccc\
              37365ef81c9b16fb1d9deaf48d91271b59c635dc2f2639236d24b92379c12b27\
              ae339e038cae33cf5beef026706c5b95421b0592adef3dd7825400cc0bbb3c3e",
        )
        .unwrap();

        let encoded = response.encode(0x1020_3040_5060_7080).unwrap();

        assert_eq!(encoded, expected);
        let (decoded, sender_id) = DiscoveryPacket::decode(&expected).unwrap();
        assert_eq!(decoded, response);
        assert_eq!(sender_id, 0x1020_3040_5060_7080);
    }

    #[test]
    fn message_round_trips() {
        let original = DiscoveryPacket::Message {
            recipient_id: u64::MAX,
            data: "CONNECTREQUEST 1234 v=0\r\n".to_string(),
        };
        let (packet, sender) = DiscoveryPacket::decode(&original.encode(1).unwrap()).unwrap();
        assert_eq!(packet, original);
        assert_eq!(sender, 1);
    }

    #[test]
    fn message_with_authenticated_trailing_bytes_is_accepted() {
        let original = DiscoveryPacket::Message {
            recipient_id: 42,
            data: "CONNECTREQUEST 1234 v=0\r\n".to_string(),
        };
        let encoded = original.encode(7).unwrap();
        let mut plaintext = encoded[CHECKSUM_SIZE..].to_vec();
        crypto::decrypt_in_place(&mut plaintext).unwrap();
        plaintext.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        let total = u16::try_from(plaintext.len()).unwrap();
        plaintext[..2].copy_from_slice(&total.to_le_bytes());

        let mac = crypto::checksum(&plaintext);
        crypto::encrypt_in_place(&mut plaintext);
        let mut packet = Vec::with_capacity(CHECKSUM_SIZE + plaintext.len());
        packet.extend_from_slice(&mac);
        packet.extend_from_slice(&plaintext);

        let (decoded, sender) = DiscoveryPacket::decode(&packet).unwrap();
        assert_eq!(decoded, original);
        assert_eq!(sender, 7);
    }

    #[test]
    fn declared_length_includes_prefix() {
        let encoded = DiscoveryPacket::Request.encode(42).unwrap();
        let mut payload = encoded[CHECKSUM_SIZE..].to_vec();
        crypto::decrypt_in_place(&mut payload).unwrap();
        assert_eq!(
            usize::from(u16::from_le_bytes([payload[0], payload[1]])),
            payload.len(),
            "长度前缀必须包含自身"
        );
        assert_eq!(payload.len(), HEADER_SIZE + 2);
    }

    #[test]
    fn rejects_tampered_ciphertext() {
        let mut encoded = DiscoveryPacket::Request.encode(1).unwrap().to_vec();
        let last = encoded.len() - 1;
        encoded[last] ^= 0xFF;
        assert!(DiscoveryPacket::decode(&encoded).is_err());
    }

    #[test]
    fn rejects_tampered_checksum() {
        let mut encoded = DiscoveryPacket::Request.encode(1).unwrap().to_vec();
        encoded[0] ^= 0xFF;
        assert!(DiscoveryPacket::decode(&encoded).is_err());
    }

    #[test]
    fn rejects_short_and_oversized_packets() {
        assert!(DiscoveryPacket::decode(&[0_u8; 8]).is_err());
        assert!(DiscoveryPacket::decode(&vec![0_u8; MAX_DISCOVERY_PACKET + 1]).is_err());
    }

    /// 回归：长度前缀口径因实现而异，不能据此丢包
    /// （go-nethernet 读侧直接跳过该字段）。
    #[test]
    fn tolerates_any_declared_length() {
        let template = DiscoveryPacket::Request.encode(42).unwrap();
        let mut plaintext = template[CHECKSUM_SIZE..].to_vec();
        crypto::decrypt_in_place(&mut plaintext).unwrap();

        for declared in [0_u16, 1, 17, 18, 19, 20, 21, 0xFFFF] {
            let mut payload = plaintext.clone();
            payload[..2].copy_from_slice(&declared.to_le_bytes());
            let mac = crypto::checksum(&payload);
            crypto::encrypt_in_place(&mut payload);
            let mut packet = Vec::with_capacity(CHECKSUM_SIZE + payload.len());
            packet.extend_from_slice(&mac);
            packet.extend_from_slice(&payload);
            let (decoded, sender) = DiscoveryPacket::decode(&packet)
                .unwrap_or_else(|error| panic!("declared={declared} 应可解析：{error}"));
            assert_eq!(decoded, DiscoveryPacket::Request);
            assert_eq!(sender, 42);
        }
    }

    /// 内层长度字段仍必须校验：声明超出剩余字节的报文要被拒。
    #[test]
    fn rejects_inner_length_beyond_payload() {
        for id in [ID_RESPONSE_PACKET, ID_MESSAGE_PACKET] {
            let mut plaintext = BytesMut::new();
            plaintext.put_u16_le(0);
            plaintext.put_u16_le(id);
            plaintext.put_u64_le(1);
            plaintext.put_bytes(0, 8);
            if id == ID_MESSAGE_PACKET {
                plaintext.put_u64_le(2);
            }
            plaintext.put_u32_le(0xFFFF); // 声明长度远超实际
            let total = u16::try_from(plaintext.len()).unwrap();
            plaintext[..2].copy_from_slice(&total.to_le_bytes());
            let mut payload = plaintext.to_vec();
            let mac = crypto::checksum(&payload);
            crypto::encrypt_in_place(&mut payload);
            let mut packet = Vec::with_capacity(CHECKSUM_SIZE + payload.len());
            packet.extend_from_slice(&mac);
            packet.extend_from_slice(&payload);
            assert!(
                DiscoveryPacket::decode(&packet).is_err(),
                "报文 {id} 的越界内层长度应被拒绝"
            );
        }
    }

    #[test]
    fn rejects_oversized_signal_body() {
        // 手工构造一个声明长度超过上限的 MessagePacket。
        let mut plaintext = BytesMut::new();
        plaintext.put_u16_le(0);
        plaintext.put_u16_le(ID_MESSAGE_PACKET);
        plaintext.put_u64_le(1);
        plaintext.put_bytes(0, 8);
        plaintext.put_u64_le(2);
        let oversized = MAX_SIGNAL_SIZE + 1;
        plaintext.put_u32_le(u32::try_from(oversized).unwrap());
        plaintext.put_bytes(b'x', oversized);
        let total = u16::try_from(plaintext.len()).unwrap_or(u16::MAX);
        plaintext[..2].copy_from_slice(&total.to_le_bytes());
        let mut payload = plaintext.to_vec();
        let mac = crypto::checksum(&payload);
        crypto::encrypt_in_place(&mut payload);
        let mut packet = Vec::with_capacity(CHECKSUM_SIZE + payload.len());
        packet.extend_from_slice(&mac);
        packet.extend_from_slice(&payload);
        assert!(DiscoveryPacket::decode(&packet).is_err());
    }

    #[test]
    fn hex_helpers_round_trip() {
        let data: Vec<u8> = (0..=255_u8).collect();
        assert_eq!(&hex_decode(&hex_encode(&data)).unwrap()[..], &data[..]);
        assert!(hex_decode(b"0g").is_err());
        assert!(hex_decode(b"abc").is_err());
    }
}
