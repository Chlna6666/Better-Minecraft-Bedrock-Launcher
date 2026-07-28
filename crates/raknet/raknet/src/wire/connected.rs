//! 连接内（在线）控制报文编解码。

use crate::consts::*;
use crate::error::RakCodecError;
use crate::wire::{Rd, addr_len, put_addr};
use bytes::{BufMut, Bytes, BytesMut};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// ConnectionRequestAccepted / NewIncomingConnection 携带的系统地址数。
/// go-raknet 与基岩版均写 20 个。
pub const SYSTEM_ADDRESS_COUNT: usize = 20;

fn zero_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))
}

/// 0x00 ConnectedPing。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedPing {
    pub time_ms: u64,
}

impl ConnectedPing {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(9);
        buf.put_u8(ID_CONNECTED_PING);
        buf.put_u64(self.time_ms);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_CONNECTED_PING)?;
        Ok(Self {
            time_ms: rd.u64_be()?,
        })
    }
}

/// 0x03 ConnectedPong。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectedPong {
    pub ping_time_ms: u64,
    pub pong_time_ms: u64,
}

impl ConnectedPong {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(17);
        buf.put_u8(ID_CONNECTED_PONG);
        buf.put_u64(self.ping_time_ms);
        buf.put_u64(self.pong_time_ms);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_CONNECTED_PONG)?;
        let ping_time_ms = rd.u64_be()?;
        // 个别实现省略第二个时间戳，容忍缺失。
        let pong_time_ms = rd.u64_be().unwrap_or(ping_time_ms);
        Ok(Self {
            ping_time_ms,
            pong_time_ms,
        })
    }
}

/// 0x09 ConnectionRequest。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionRequest {
    pub client_guid: u64,
    pub time_ms: u64,
    pub security: bool,
}

impl ConnectionRequest {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(18);
        buf.put_u8(ID_CONNECTION_REQUEST);
        buf.put_u64(self.client_guid);
        buf.put_u64(self.time_ms);
        buf.put_u8(self.security as u8);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_CONNECTION_REQUEST)?;
        let client_guid = rd.u64_be()?;
        let time_ms = rd.u64_be()?;
        let security = rd.u8().map(|b| b != 0).unwrap_or(false);
        Ok(Self {
            client_guid,
            time_ms,
            security,
        })
    }
}

/// 0x10 ConnectionRequestAccepted。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectionRequestAccepted {
    pub client_address: SocketAddr,
    pub system_index: u16,
    pub request_time_ms: u64,
    pub time_ms: u64,
}

impl ConnectionRequestAccepted {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(
            1 + addr_len(&self.client_address) + 2 + SYSTEM_ADDRESS_COUNT * 7 + 16,
        );
        buf.put_u8(ID_CONNECTION_REQUEST_ACCEPTED);
        put_addr(&mut buf, &self.client_address);
        buf.put_u16(self.system_index);
        for _ in 0..SYSTEM_ADDRESS_COUNT {
            put_addr(&mut buf, &zero_addr());
        }
        buf.put_u64(self.request_time_ms);
        buf.put_u64(self.time_ms);
        buf.freeze()
    }

    /// 系统地址数量因实现而异（0/10/20 都有），自适应读取：
    /// 只要剩余字节多于两个时间戳（16 字节）就继续读地址。
    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_CONNECTION_REQUEST_ACCEPTED)?;
        let client_address = rd.addr()?;
        let system_index = rd.u16_be()?;
        while rd.remaining() > 16 {
            rd.addr()?;
        }
        let request_time_ms = rd.u64_be()?;
        let time_ms = rd.u64_be()?;
        Ok(Self {
            client_address,
            system_index,
            request_time_ms,
            time_ms,
        })
    }
}

/// 0x13 NewIncomingConnection。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewIncomingConnection {
    pub server_address: SocketAddr,
    pub request_time_ms: u64,
    pub time_ms: u64,
}

impl NewIncomingConnection {
    pub fn encode(&self) -> Bytes {
        let mut buf = BytesMut::with_capacity(
            1 + addr_len(&self.server_address) + SYSTEM_ADDRESS_COUNT * 7 + 16,
        );
        buf.put_u8(ID_NEW_INCOMING_CONNECTION);
        put_addr(&mut buf, &self.server_address);
        for _ in 0..SYSTEM_ADDRESS_COUNT {
            put_addr(&mut buf, &zero_addr());
        }
        buf.put_u64(self.request_time_ms);
        buf.put_u64(self.time_ms);
        buf.freeze()
    }

    pub fn decode(buf: Bytes) -> Result<Self, RakCodecError> {
        let mut rd = Rd::new(buf);
        rd.packet_id(ID_NEW_INCOMING_CONNECTION)?;
        let server_address = rd.addr()?;
        while rd.remaining() > 16 {
            rd.addr()?;
        }
        let request_time_ms = rd.u64_be()?;
        let time_ms = rd.u64_be()?;
        Ok(Self {
            server_address,
            request_time_ms,
            time_ms,
        })
    }
}

/// 0x15 Disconnect。
pub fn encode_disconnect() -> Bytes {
    Bytes::from_static(&[ID_DISCONNECT])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_pong_round_trip() {
        let ping = ConnectedPing { time_ms: 1 };
        assert_eq!(ConnectedPing::decode(ping.encode()).unwrap(), ping);
        let pong = ConnectedPong {
            ping_time_ms: 1,
            pong_time_ms: 2,
        };
        assert_eq!(ConnectedPong::decode(pong.encode()).unwrap(), pong);
    }

    #[test]
    fn connection_request_round_trip() {
        let req = ConnectionRequest {
            client_guid: u64::MAX,
            time_ms: 5,
            security: false,
        };
        assert_eq!(ConnectionRequest::decode(req.encode()).unwrap(), req);
    }

    #[test]
    fn accepted_round_trip_and_adaptive_addresses() {
        let acc = ConnectionRequestAccepted {
            client_address: "192.168.1.2:60000".parse().unwrap(),
            system_index: 0,
            request_time_ms: 111,
            time_ms: 222,
        };
        assert_eq!(
            ConnectionRequestAccepted::decode(acc.encode()).unwrap(),
            acc
        );

        // 无系统地址（旧实现）也能解析。
        let mut short = BytesMut::new();
        short.put_u8(ID_CONNECTION_REQUEST_ACCEPTED);
        put_addr(&mut short, &"1.1.1.1:1".parse().unwrap());
        short.put_u16(0);
        short.put_u64(9);
        short.put_u64(10);
        let parsed = ConnectionRequestAccepted::decode(short.freeze()).unwrap();
        assert_eq!(parsed.request_time_ms, 9);
        assert_eq!(parsed.time_ms, 10);
    }

    #[test]
    fn new_incoming_round_trip() {
        let nic = NewIncomingConnection {
            server_address: "[::1]:19132".parse().unwrap(),
            request_time_ms: 1,
            time_ms: 2,
        };
        assert_eq!(NewIncomingConnection::decode(nic.encode()).unwrap(), nic);
    }
}
