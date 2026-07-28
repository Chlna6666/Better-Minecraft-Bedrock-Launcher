//! 离线（无连接）报文编解码。

use crate::consts::*;
use crate::error::RakCodecError;
use crate::wire::{Rd, addr_len, put_addr};
use bytes::{BufMut, Bytes, BytesMut};
use std::net::SocketAddr;

/// 0x01 UnconnectedPing。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnconnectedPing {
    pub time_ms: u64,
    pub client_guid: u64,
}

impl UnconnectedPing {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(1 + 8 + 16 + 8);
        buf.put_u8(ID_UNCONNECTED_PING);
        buf.put_u64(self.time_ms);
        buf.put_slice(&MAGIC);
        buf.put_u64(self.client_guid);
        buf.freeze()
    }

    /// 同时接受 0x01 与 0x02（含开放连接检测变体）。
    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        let id = rd.u8()?;
        if id != ID_UNCONNECTED_PING && id != ID_UNCONNECTED_PING_OPEN_CONNECTIONS {
            return Err(RakCodecError::UnexpectedPacketId {
                expected: ID_UNCONNECTED_PING,
                found: id,
            });
        }
        let time_ms = rd.u64_be()?;
        rd.magic()?;
        // 部分实现省略 client guid，容忍缺失。
        let client_guid = rd.u64_be().unwrap_or(0);
        Ok(Self {
            time_ms,
            client_guid,
        })
    }
}

/// 0x1C UnconnectedPong。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnconnectedPong {
    pub time_ms: u64,
    pub server_guid: u64,
    pub motd: Bytes,
}

impl UnconnectedPong {
    pub fn encode(&self) -> Bytes {
        let motd_len = self.motd.len().min(u16::MAX as usize);
        let mut buf = BytesMut::with_capacity(1 + 8 + 8 + 16 + 2 + motd_len);
        buf.put_u8(ID_UNCONNECTED_PONG);
        buf.put_u64(self.time_ms);
        buf.put_u64(self.server_guid);
        buf.put_slice(&MAGIC);
        buf.put_u16(motd_len as u16);
        buf.put_slice(&self.motd[..motd_len]);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_UNCONNECTED_PONG)?;
        let time_ms = rd.u64_be()?;
        let server_guid = rd.u64_be()?;
        rd.magic()?;
        let len = rd.u16_be()? as usize;
        let motd = rd.take(len.min(rd.remaining()))?;
        Ok(Self {
            time_ms,
            server_guid,
            motd,
        })
    }
}

/// 0x05 OpenConnectionRequest1。
///
/// `mtu_payload` 为整个 UDP 载荷长度：发送时填充零字节至该长度，
/// 接收方以数据报长度探测路径 MTU。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenConnectionRequest1 {
    pub protocol: u8,
    pub mtu_payload: u16,
}

impl OpenConnectionRequest1 {
    pub fn encode(&self) -> Bytes {
        let total = (self.mtu_payload as usize).max(1 + 16 + 1);
        let mut buf = BytesMut::with_capacity(total);
        buf.put_u8(ID_OPEN_CONNECTION_REQUEST_1);
        buf.put_slice(&MAGIC);
        buf.put_u8(self.protocol);
        buf.resize(total, 0);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mtu_payload = buf.len().min(u16::MAX as usize) as u16;
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_OPEN_CONNECTION_REQUEST_1)?;
        rd.magic()?;
        let protocol = rd.u8()?;
        Ok(Self {
            protocol,
            mtu_payload,
        })
    }
}

/// 0x06 OpenConnectionReply1。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenConnectionReply1 {
    pub server_guid: u64,
    /// go-raknet v1.14+ 默认开启 cookie 防护；客户端必须在
    /// OpenConnectionRequest2 中回显该值。
    pub cookie: Option<i32>,
    pub mtu: u16,
}

impl OpenConnectionReply1 {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(1 + 16 + 8 + 5 + 2);
        buf.put_u8(ID_OPEN_CONNECTION_REPLY_1);
        buf.put_slice(&MAGIC);
        buf.put_u64(self.server_guid);
        match self.cookie {
            Some(cookie) => {
                buf.put_u8(1);
                buf.put_i32(cookie);
            }
            None => buf.put_u8(0),
        }
        buf.put_u16(self.mtu);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_OPEN_CONNECTION_REPLY_1)?;
        rd.magic()?;
        let server_guid = rd.u64_be()?;
        let cookie = if rd.u8()? != 0 {
            Some(rd.i32_be()?)
        } else {
            None
        };
        let mtu = rd.u16_be()?;
        Ok(Self {
            server_guid,
            cookie,
            mtu,
        })
    }
}

/// 0x07 OpenConnectionRequest2。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenConnectionRequest2 {
    pub cookie: Option<i32>,
    pub server_address: SocketAddr,
    pub mtu: u16,
    pub client_guid: u64,
}

impl OpenConnectionRequest2 {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(1 + 16 + 5 + addr_len(&self.server_address) + 2 + 8);
        buf.put_u8(ID_OPEN_CONNECTION_REQUEST_2);
        buf.put_slice(&MAGIC);
        if let Some(cookie) = self.cookie {
            buf.put_i32(cookie);
            buf.put_u8(0); // 客户端不携带安全挑战
        }
        put_addr(&mut buf, &self.server_address);
        buf.put_u16(self.mtu);
        buf.put_u64(self.client_guid);
        buf.freeze()
    }

    /// 是否携带 cookie 由剩余长度判定：
    /// cookie(4)+challenge(1) + addr(7|29) + mtu(2) + guid(8)
    /// → 带 cookie 时剩余 22（IPv4）或 44（IPv6）字节。
    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_OPEN_CONNECTION_REQUEST_2)?;
        rd.magic()?;
        let cookie = match rd.remaining() {
            22 | 44 => {
                let value = rd.i32_be()?;
                rd.u8()?; // 忽略安全挑战标记
                Some(value)
            }
            _ => None,
        };
        let server_address = rd.addr()?;
        let mtu = rd.u16_be()?;
        let client_guid = rd.u64_be()?;
        Ok(Self {
            cookie,
            server_address,
            mtu,
            client_guid,
        })
    }
}

/// 0x08 OpenConnectionReply2。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenConnectionReply2 {
    pub server_guid: u64,
    pub client_address: SocketAddr,
    pub mtu: u16,
    pub security: bool,
}

impl OpenConnectionReply2 {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(1 + 16 + 8 + addr_len(&self.client_address) + 2 + 1);
        buf.put_u8(ID_OPEN_CONNECTION_REPLY_2);
        buf.put_slice(&MAGIC);
        buf.put_u64(self.server_guid);
        put_addr(&mut buf, &self.client_address);
        buf.put_u16(self.mtu);
        buf.put_u8(self.security as u8);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_OPEN_CONNECTION_REPLY_2)?;
        rd.magic()?;
        let server_guid = rd.u64_be()?;
        let client_address = rd.addr()?;
        let mtu = rd.u16_be()?;
        let security = rd.u8()? != 0;
        Ok(Self {
            server_guid,
            client_address,
            mtu,
            security,
        })
    }
}

/// 0x19 IncompatibleProtocol。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IncompatibleProtocol {
    pub protocol: u8,
    pub server_guid: u64,
}

impl IncompatibleProtocol {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(1 + 1 + 16 + 8);
        buf.put_u8(ID_INCOMPATIBLE_PROTOCOL);
        buf.put_u8(self.protocol);
        buf.put_slice(&MAGIC);
        buf.put_u64(self.server_guid);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_INCOMPATIBLE_PROTOCOL)?;
        let protocol = rd.u8()?;
        rd.magic()?;
        let server_guid = rd.u64_be()?;
        Ok(Self {
            protocol,
            server_guid,
        })
    }
}

/// 简单拒绝类报文（0x12 AlreadyConnected / 0x14 NoFreeIncomingConnections /
/// 0x1A IpRecentlyConnected）：ID + magic + server guid。
pub fn encode_simple_refusal(id: u8, server_guid: u64) -> Bytes {
    let mut buf = BytesMut::with_capacity(1 + 16 + 8);
    buf.put_u8(id);
    buf.put_slice(&MAGIC);
    buf.put_u64(server_guid);
    buf.freeze()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_pong_round_trip() {
        let ping = UnconnectedPing {
            time_ms: 12345,
            client_guid: 0xDEAD_BEEF,
        };
        assert_eq!(UnconnectedPing::decode(ping.encode()).unwrap(), ping);

        let pong = UnconnectedPong {
            time_ms: 777,
            server_guid: 42,
            motd: Bytes::from_static(b"MCPE;Test;589;1.20.0;1;20;42;World;Survival;0;19132;19132;"),
        };
        assert_eq!(UnconnectedPong::decode(pong.encode()).unwrap(), pong);
    }

    #[test]
    fn ocr1_pads_to_mtu() {
        let req = OpenConnectionRequest1 {
            protocol: 11,
            mtu_payload: 1200,
        };
        let encoded = req.encode();
        assert_eq!(encoded.len(), 1200);
        assert_eq!(OpenConnectionRequest1::decode(encoded).unwrap(), req);
    }

    #[test]
    fn reply1_cookie_round_trip() {
        for cookie in [None, Some(-1), Some(0x1234_5678)] {
            let reply = OpenConnectionReply1 {
                server_guid: 9,
                cookie,
                mtu: 1228,
            };
            assert_eq!(OpenConnectionReply1::decode(reply.encode()).unwrap(), reply);
        }
    }

    #[test]
    fn ocr2_cookie_detection_v4_v6() {
        for server_address in [
            "127.0.0.1:19132".parse::<SocketAddr>().unwrap(),
            "[::1]:19132".parse::<SocketAddr>().unwrap(),
        ] {
            for cookie in [None, Some(77)] {
                let req = OpenConnectionRequest2 {
                    cookie,
                    server_address,
                    mtu: 1228,
                    client_guid: u64::MAX,
                };
                assert_eq!(OpenConnectionRequest2::decode(req.encode()).unwrap(), req);
            }
        }
    }

    #[test]
    fn reply2_round_trip() {
        let reply = OpenConnectionReply2 {
            server_guid: 3,
            client_address: "10.0.0.2:54321".parse().unwrap(),
            mtu: 1228,
            security: false,
        };
        assert_eq!(OpenConnectionReply2::decode(reply.encode()).unwrap(), reply);
    }
}
