//! 线格式编解码基础设施。
//!
//! [`Rd`] 是基于 `Bytes` 的零拷贝读取器：`take` 返回底层缓冲的切片
//! 引用计数视图，不发生内存拷贝。所有读取都做边界检查，绝不 panic。

pub mod acknack;
pub mod connected;
pub mod frame;
pub mod offline;

use crate::consts::MAGIC;
use crate::error::RakCodecError;
use bytes::{BufMut, Bytes, BytesMut};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};

/// 零拷贝读取器。
pub struct Rd {
    buf: Bytes,
    pos: usize,
}

impl Rd {
    #[inline]
    pub fn new(buf: Bytes) -> Self {
        Self { buf, pos: 0 }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    #[inline]
    fn check(&self, needed: usize) -> Result<(), RakCodecError> {
        if self.remaining() < needed {
            Err(RakCodecError::Truncated {
                needed,
                remaining: self.remaining(),
            })
        } else {
            Ok(())
        }
    }

    /// 零拷贝取 `len` 字节（底层缓冲的切片视图）。
    #[inline]
    pub fn take(&mut self, len: usize) -> Result<Bytes, RakCodecError> {
        self.check(len)?;
        let out = self.buf.slice(self.pos..self.pos + len);
        self.pos += len;
        Ok(out)
    }

    #[inline]
    pub fn u8(&mut self) -> Result<u8, RakCodecError> {
        self.check(1)?;
        let v = self.buf[self.pos];
        self.pos += 1;
        Ok(v)
    }

    #[inline]
    fn array<const N: usize>(&mut self) -> Result<[u8; N], RakCodecError> {
        self.check(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(&self.buf[self.pos..self.pos + N]);
        self.pos += N;
        Ok(out)
    }

    #[inline]
    pub fn u16_be(&mut self) -> Result<u16, RakCodecError> {
        Ok(u16::from_be_bytes(self.array::<2>()?))
    }

    #[inline]
    pub fn u16_le(&mut self) -> Result<u16, RakCodecError> {
        Ok(u16::from_le_bytes(self.array::<2>()?))
    }

    #[inline]
    pub fn u24_le(&mut self) -> Result<u32, RakCodecError> {
        let b = self.array::<3>()?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], 0]))
    }

    #[inline]
    pub fn u32_be(&mut self) -> Result<u32, RakCodecError> {
        Ok(u32::from_be_bytes(self.array::<4>()?))
    }

    #[inline]
    pub fn i32_be(&mut self) -> Result<i32, RakCodecError> {
        Ok(i32::from_be_bytes(self.array::<4>()?))
    }

    #[inline]
    pub fn u64_be(&mut self) -> Result<u64, RakCodecError> {
        Ok(u64::from_be_bytes(self.array::<8>()?))
    }

    /// 校验并跳过离线魔数。
    pub fn magic(&mut self) -> Result<(), RakCodecError> {
        let m = self.array::<16>()?;
        if m == MAGIC {
            Ok(())
        } else {
            Err(RakCodecError::BadMagic)
        }
    }

    /// 校验首字节报文 ID。
    pub fn packet_id(&mut self, expected: u8) -> Result<(), RakCodecError> {
        let found = self.u8()?;
        if found == expected {
            Ok(())
        } else {
            Err(RakCodecError::UnexpectedPacketId { expected, found })
        }
    }

    /// RakNet 地址编码（与 go-raknet / vanilla 互通的布局）。
    ///
    /// IPv4 八位组在线上按位取反（RakNet 传统，go-raknet 与 PocketMine
    /// 同样如此）；IPv6 的 family 字段是小端 AF_INET6。
    pub fn addr(&mut self) -> Result<SocketAddr, RakCodecError> {
        match self.u8()? {
            4 => {
                let mut octets = self.array::<4>()?;
                for b in &mut octets {
                    *b = !*b;
                }
                let ip = Ipv4Addr::from(octets);
                let port = self.u16_be()?;
                Ok(SocketAddr::V4(SocketAddrV4::new(ip, port)))
            }
            6 => {
                self.u16_le()?; // AF_INET6
                let port = self.u16_be()?;
                let flowinfo = self.u32_be()?;
                let ip = Ipv6Addr::from(self.array::<16>()?);
                let scope_id = self.u32_be()?;
                Ok(SocketAddr::V6(SocketAddrV6::new(
                    ip, port, flowinfo, scope_id,
                )))
            }
            _ => Err(RakCodecError::Malformed("地址版本字节非法")),
        }
    }
}

/// 写入 u24（小端）。
#[inline]
pub fn put_u24_le(buf: &mut BytesMut, value: u32) {
    let b = value.to_le_bytes();
    buf.put_slice(&b[..3]);
}

/// 写入 RakNet 地址。
pub fn put_addr(buf: &mut BytesMut, addr: &SocketAddr) {
    match addr {
        SocketAddr::V4(v4) => {
            buf.put_u8(4);
            for b in v4.ip().octets() {
                buf.put_u8(!b);
            }
            buf.put_u16(v4.port());
        }
        SocketAddr::V6(v6) => {
            buf.put_u8(6);
            buf.put_u16_le(23); // AF_INET6
            buf.put_u16(v6.port());
            buf.put_u32(v6.flowinfo());
            buf.put_slice(&v6.ip().octets());
            buf.put_u32(v6.scope_id());
        }
    }
}

/// 地址编码后的长度。
pub fn addr_len(addr: &SocketAddr) -> usize {
    match addr {
        SocketAddr::V4(_) => 1 + 4 + 2,
        SocketAddr::V6(_) => 1 + 2 + 2 + 4 + 16 + 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rd_bounds_checked() {
        let mut rd = Rd::new(Bytes::from_static(&[1, 2]));
        assert_eq!(rd.u8().unwrap(), 1);
        assert!(rd.u16_be().is_err());
        assert_eq!(rd.remaining(), 1);
    }

    #[test]
    fn u24_round_trip() {
        let mut buf = BytesMut::new();
        put_u24_le(&mut buf, 0xABCDEF);
        let mut rd = Rd::new(buf.freeze());
        assert_eq!(rd.u24_le().unwrap(), 0xABCDEF);
    }

    #[test]
    fn ipv4_octets_inverted_on_wire() {
        // go-raknet / vanilla RakNet 写入按位取反的八位组。
        let mut buf = BytesMut::new();
        put_addr(&mut buf, &"127.0.0.1:19132".parse().unwrap());
        assert_eq!(&buf[..5], &[4, 128, 255, 255, 254]);
    }

    #[test]
    fn ipv6_family_is_little_endian() {
        let mut buf = BytesMut::new();
        put_addr(&mut buf, &"[::1]:1".parse().unwrap());
        assert_eq!(&buf[..3], &[6, 23, 0]);
    }

    #[test]
    fn addr_round_trip_v4_v6() {
        for addr in [
            "1.2.3.4:5678".parse::<SocketAddr>().unwrap(),
            "[2001:db8::1]:19132".parse::<SocketAddr>().unwrap(),
        ] {
            let mut buf = BytesMut::new();
            put_addr(&mut buf, &addr);
            assert_eq!(buf.len(), addr_len(&addr));
            let mut rd = Rd::new(buf.freeze());
            assert_eq!(rd.addr().unwrap(), addr);
        }
    }

    #[test]
    fn take_is_zero_copy_slice() {
        let src = Bytes::from(vec![9u8; 64]);
        let mut rd = Rd::new(src.clone());
        let a = rd.take(16).unwrap();
        // 切片与源共享同一底层分配（指针位于源范围内）。
        let src_ptr = src.as_ptr() as usize;
        let a_ptr = a.as_ptr() as usize;
        assert!(a_ptr >= src_ptr && a_ptr < src_ptr + src.len());
    }
}
