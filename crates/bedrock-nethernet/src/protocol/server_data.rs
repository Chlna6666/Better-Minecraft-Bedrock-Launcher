//! `ServerData`：局域网世界卡片的应用层数据。
//!
//! 线格式与 go-nethernet `discovery/server_data.go` 逐字节一致（v5），
//! 并兼容旧版 v4 载荷。

use crate::error::{NethernetError, Result};
use crate::protocol::codec::{Rd, put_var_bytes, put_var_i32};
use bytes::{BufMut, Bytes, BytesMut};

/// 当前 `ServerData` 版本。
pub const VERSION: u8 = 5;

/// 游戏模式取值。
pub mod game_type {
    pub const SURVIVAL: i32 = 0;
    pub const CREATIVE: i32 = 1;
    pub const ADVENTURE: i32 = 2;
    pub const SURVIVAL_VIEWER: i32 = 3;
    pub const CREATIVE_VIEWER: i32 = 4;
    pub const DEFAULT: i32 = 5;
}

/// 传输层取值。
pub mod transport_layer {
    pub const RAKNET: i32 = 0;
    pub const NETHERNET: i32 = 2;
    pub const LOCAL: i32 = 4;
}

/// 世界卡片数据。
///
/// 字段名与顺序对应线格式，不要重排。
#[derive(Debug, Clone, Eq, PartialEq)]
// 这些布尔字段与 ServerData v5 线格式一一对应。
#[allow(clippy::struct_excessive_bools)]
pub struct ServerData {
    /// 服务器名，显示在世界卡片下方（通常是房主玩家名）。
    pub server_name: String,
    /// 世界名，显示在世界卡片顶部。
    pub level_name: String,
    /// 默认游戏模式。
    pub game_type: i32,
    /// 当前在线人数。**小于等于 0 时客户端不会显示该世界**，
    /// 因此即使实际为 0 也应上报 1。
    pub player_count: i32,
    /// 人数上限。
    pub max_player_count: i32,
    /// 是否为编辑器模式创建的项目世界。
    pub editor_world: bool,
    /// 是否为极限模式。
    pub hardcore: bool,
    /// 是否接受 Xbox Live 在线认证的玩家。
    pub accepts_online_auth: bool,
    /// 是否接受自签名（局域网）认证的玩家。
    pub accepts_self_signed_auth: bool,
    /// 传输层，NetherNet 为 2。
    pub transport_layer: i32,
    /// 连接类型，局域网信令为 4。
    pub connection_type: i32,
}

impl Default for ServerData {
    fn default() -> Self {
        Self {
            server_name: String::new(),
            level_name: String::new(),
            game_type: game_type::SURVIVAL,
            player_count: 1,
            max_player_count: 8,
            editor_world: false,
            hardcore: false,
            accepts_online_auth: true,
            accepts_self_signed_auth: true,
            transport_layer: transport_layer::NETHERNET,
            connection_type: 4,
        }
    }
}

impl ServerData {
    /// 编码为 v5 载荷。
    ///
    /// # Errors
    ///
    /// 字符串长度超过 `u32` 时返回错误。
    pub fn encode(&self) -> Result<Bytes> {
        let mut buf = BytesMut::with_capacity(32 + self.server_name.len() + self.level_name.len());
        buf.put_u8(VERSION);
        put_var_bytes(&mut buf, self.server_name.as_bytes())?;
        put_var_bytes(&mut buf, self.level_name.as_bytes())?;
        put_var_i32(&mut buf, self.game_type);
        buf.put_i32_le(self.player_count);
        buf.put_i32_le(self.max_player_count);
        buf.put_u8(u8::from(self.editor_world));
        buf.put_u8(u8::from(self.hardcore));
        buf.put_u8(u8::from(self.accepts_online_auth));
        buf.put_u8(u8::from(self.accepts_self_signed_auth));
        put_var_i32(&mut buf, self.transport_layer);
        put_var_i32(&mut buf, self.connection_type);
        Ok(buf.freeze())
    }

    /// 编码为兼容旧版 Bedrock 客户端的 v4 载荷。
    ///
    /// # Errors
    ///
    /// 字符串超过 255 字节，或枚举值无法用 v4 的单字节格式表示时返回错误。
    pub(crate) fn encode_v4(&self) -> Result<Bytes> {
        let mut buf = BytesMut::with_capacity(24 + self.server_name.len() + self.level_name.len());
        buf.put_u8(4);
        put_u8_bytes(&mut buf, self.server_name.as_bytes(), "服务器名")?;
        put_u8_bytes(&mut buf, self.level_name.as_bytes(), "世界名")?;
        buf.put_u8(encode_v4_enum(self.game_type, "游戏模式")?);
        buf.put_i32_le(self.player_count);
        buf.put_i32_le(self.max_player_count);
        buf.put_u8(u8::from(self.editor_world));
        buf.put_u8(u8::from(self.hardcore));
        buf.put_u8(encode_v4_enum(self.transport_layer, "传输层")?);
        buf.put_u8(encode_v4_enum(self.connection_type, "连接类型")?);
        Ok(buf.freeze())
    }

    /// 解析 v5 或旧版 v4 载荷。
    ///
    /// # Errors
    ///
    /// 版本不支持、字段截断或含尾随数据时返回错误。
    pub fn decode(data: Bytes) -> Result<Self> {
        let mut rd = Rd::new(data);
        match rd.u8()? {
            5 => Self::decode_v5(&mut rd),
            4 => Self::decode_v4(&mut rd),
            version => Err(NethernetError::protocol(format!(
                "不支持的 ServerData 版本：{version}"
            ))),
        }
    }

    fn decode_v5(rd: &mut Rd) -> Result<Self> {
        let data = Self {
            server_name: rd.var_string()?,
            level_name: rd.var_string()?,
            game_type: rd.var_i32()?,
            player_count: rd.i32_le()?,
            max_player_count: rd.i32_le()?,
            editor_world: rd.bool()?,
            hardcore: rd.bool()?,
            accepts_online_auth: rd.bool()?,
            accepts_self_signed_auth: rd.bool()?,
            transport_layer: rd.var_i32()?,
            connection_type: rd.var_i32()?,
        };
        if !rd.is_empty() {
            return Err(NethernetError::protocol("ServerData 含有尾随数据"));
        }
        Ok(data)
    }

    /// v4：字符串用 u8 长度前缀，若干整型字段用左移一位的编码。
    fn decode_v4(rd: &mut Rd) -> Result<Self> {
        let server_name = rd.u8_string()?;
        let level_name = rd.u8_string()?;
        let game_type = i32::from(rd.u8()? >> 1);
        let player_count = rd.i32_le()?;
        let max_player_count = rd.i32_le()?;
        let editor_world = rd.bool()?;
        let hardcore = rd.bool()?;
        let transport_layer = i32::from(rd.u8()? >> 1);
        let connection_type = i32::from(rd.u8()? >> 1);
        if !rd.is_empty() {
            return Err(NethernetError::protocol("ServerData v4 含有尾随数据"));
        }
        Ok(Self {
            server_name,
            level_name,
            game_type,
            player_count,
            max_player_count,
            editor_world,
            hardcore,
            // v4 没有这两个字段，按接受两种认证处理。
            accepts_online_auth: true,
            accepts_self_signed_auth: true,
            transport_layer,
            connection_type,
        })
    }
}

fn put_u8_bytes(buf: &mut BytesMut, data: &[u8], field: &str) -> Result<()> {
    let length = u8::try_from(data.len())
        .map_err(|_| NethernetError::protocol(format!("ServerData v4 {field}超过 255 字节")))?;
    buf.put_u8(length);
    buf.put_slice(data);
    Ok(())
}

fn encode_v4_enum(value: i32, field: &str) -> Result<u8> {
    let value = u8::try_from(value)
        .map_err(|_| NethernetError::protocol(format!("ServerData v4 {field}超出范围")))?;
    value
        .checked_mul(2)
        .ok_or_else(|| NethernetError::protocol(format!("ServerData v4 {field}超出范围")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ServerData {
        ServerData {
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
        }
    }

    #[test]
    fn round_trips() {
        let original = sample();
        assert_eq!(
            ServerData::decode(original.encode().unwrap()).unwrap(),
            original
        );
    }

    #[test]
    fn zigzag_boundaries_round_trip() {
        let original = ServerData {
            server_name: String::new(),
            level_name: String::new(),
            game_type: i32::MIN,
            player_count: i32::MIN,
            max_player_count: i32::MAX,
            transport_layer: i32::MAX,
            connection_type: -1,
            ..sample()
        };
        assert_eq!(
            ServerData::decode(original.encode().unwrap()).unwrap(),
            original
        );
    }

    /// 与 go-nethernet `discovery/server_data_test.go` 相同的向量。
    #[test]
    fn matches_reference_v5_vector() {
        let data = ServerData {
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
        let expected: &[u8] = &[
            0x05, 0x06, b's', b'e', b'r', b'v', b'e', b'r', 0x05, b'w', b'o', b'r', b'l', b'd',
            0x04, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x04,
            0x08,
        ];
        assert_eq!(&data.encode().unwrap()[..], expected);
        assert_eq!(
            ServerData::decode(Bytes::from_static(expected)).unwrap(),
            data
        );
    }

    #[test]
    fn matches_reference_v4_vector() {
        let data = ServerData {
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
        let expected: &[u8] = &[
            0x04, 0x06, b's', b'e', b'r', b'v', b'e', b'r', 0x05, b'w', b'o', b'r', b'l', b'd',
            0x04, 0x01, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x01, 0x04, 0x08,
        ];
        assert_eq!(&data.encode_v4().unwrap()[..], expected);
        assert_eq!(
            ServerData::decode(Bytes::from_static(expected)).unwrap(),
            data
        );
    }

    #[test]
    fn reads_legacy_v4() {
        let legacy = Bytes::from_static(&[
            4, 5, b'B', b'M', b'C', b'B', b'L', 5, b'W', b'o', b'r', b'l', b'd', 2, 1, 0, 0, 0, 20,
            0, 0, 0, 0, 0, 4, 8,
        ]);
        let decoded = ServerData::decode(legacy).unwrap();
        assert_eq!(decoded.server_name, "BMCBL");
        assert_eq!(decoded.level_name, "World");
        assert_eq!(decoded.game_type, 1);
        assert_eq!(decoded.transport_layer, 2);
        assert_eq!(decoded.connection_type, 4);
        assert!(decoded.accepts_online_auth);
    }

    #[test]
    fn rejects_trailing_data() {
        let mut encoded = sample().encode().unwrap().to_vec();
        encoded.push(0);
        assert!(ServerData::decode(Bytes::from(encoded)).is_err());
    }

    #[test]
    fn rejects_unknown_version() {
        assert!(ServerData::decode(Bytes::from_static(&[9, 0, 0])).is_err());
    }

    #[test]
    fn rejects_truncated_payload() {
        let encoded = sample().encode().unwrap();
        for cut in 1..encoded.len() {
            assert!(
                ServerData::decode(encoded.slice(..cut)).is_err(),
                "截断到 {cut} 字节时应报错"
            );
        }
    }
}
