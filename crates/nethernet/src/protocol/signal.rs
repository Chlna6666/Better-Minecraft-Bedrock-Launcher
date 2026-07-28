//! 信令文本：`TYPE ConnectionID Data`。

use crate::error::{NethernetError, Result, SignalErrorCode};
use std::fmt;
use std::str::FromStr;

/// 信令类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignalType {
    /// `CONNECTREQUEST`：客户端发起，携带 offer SDP。
    Offer,
    /// `CONNECTRESPONSE`：服务端应答，携带 answer SDP。
    Answer,
    /// `CANDIDATEADD`：双方增量通告 ICE 候选。
    Candidate,
    /// `CONNECTERROR`：双方上报错误码。
    Error,
}

impl SignalType {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Offer => "CONNECTREQUEST",
            Self::Answer => "CONNECTRESPONSE",
            Self::Candidate => "CANDIDATEADD",
            Self::Error => "CONNECTERROR",
        }
    }
}

impl FromStr for SignalType {
    type Err = NethernetError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "CONNECTREQUEST" => Ok(Self::Offer),
            "CONNECTRESPONSE" => Ok(Self::Answer),
            "CANDIDATEADD" => Ok(Self::Candidate),
            "CONNECTERROR" => Ok(Self::Error),
            other => Err(NethernetError::protocol(format!("未知信令类型：{other}"))),
        }
    }
}

impl fmt::Display for SignalType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一条信令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub kind: SignalType,
    /// 单次协商内唯一；与跨连接复用的网络 ID 不同。
    pub connection_id: u64,
    /// 负载：SDP、ICE 候选文本或错误码。
    pub data: String,
    /// 对端网络 ID。发送时是收件人，接收时是发件人。
    pub network_id: u64,
}

impl Signal {
    #[must_use]
    pub const fn new(kind: SignalType, connection_id: u64, data: String, network_id: u64) -> Self {
        Self {
            kind,
            connection_id,
            data,
            network_id,
        }
    }

    #[must_use]
    pub const fn offer(connection_id: u64, sdp: String, network_id: u64) -> Self {
        Self::new(SignalType::Offer, connection_id, sdp, network_id)
    }

    #[must_use]
    pub const fn answer(connection_id: u64, sdp: String, network_id: u64) -> Self {
        Self::new(SignalType::Answer, connection_id, sdp, network_id)
    }

    #[must_use]
    pub const fn candidate(connection_id: u64, candidate: String, network_id: u64) -> Self {
        Self::new(SignalType::Candidate, connection_id, candidate, network_id)
    }

    #[must_use]
    pub fn error(connection_id: u64, code: SignalErrorCode, network_id: u64) -> Self {
        Self::new(
            SignalType::Error,
            connection_id,
            code.code().to_string(),
            network_id,
        )
    }

    /// 若本条是 `CONNECTERROR`，解析其错误码。
    #[must_use]
    pub fn error_code(&self) -> Option<SignalErrorCode> {
        if self.kind != SignalType::Error {
            return None;
        }
        Some(self.data.trim().parse::<u32>().map_or(
            SignalErrorCode::SignalingUnknownError,
            SignalErrorCode::from_code,
        ))
    }

    /// 解析线上文本；`network_id` 由外层报文提供。
    ///
    /// # Errors
    ///
    /// 字段数不足或连接编号非法时返回错误。
    pub fn parse(text: &str, network_id: u64) -> Result<Self> {
        let mut fields = text.splitn(3, ' ');
        let kind: SignalType = fields
            .next()
            .ok_or_else(|| NethernetError::protocol("信令为空"))?
            .parse()?;
        let connection_id = fields
            .next()
            .ok_or_else(|| NethernetError::protocol("信令缺少连接编号"))?
            .parse::<u64>()
            .map_err(|error| NethernetError::protocol(format!("信令连接编号非法：{error}")))?;
        let data = fields
            .next()
            .ok_or_else(|| NethernetError::protocol("信令缺少数据"))?
            .to_string();
        Ok(Self {
            kind,
            connection_id,
            data,
            network_id,
        })
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} {}",
            self.kind.as_str(),
            self.connection_id,
            self.data
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_types() {
        for kind in [
            SignalType::Offer,
            SignalType::Answer,
            SignalType::Candidate,
            SignalType::Error,
        ] {
            let signal = Signal::new(kind, 12345, "payload data".to_string(), 99);
            let parsed = Signal::parse(&signal.to_string(), 99).unwrap();
            assert_eq!(parsed, signal);
        }
    }

    #[test]
    fn data_may_contain_spaces_and_newlines() {
        // SDP 含空格与 CRLF，必须原样保留（splitn 上限 3）。
        let sdp = "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\na=ice-options:trickle\r\n";
        let signal = Signal::offer(1, sdp.to_string(), 5);
        let parsed = Signal::parse(&signal.to_string(), 5).unwrap();
        assert_eq!(parsed.data, sdp);
    }

    #[test]
    fn error_code_parsed() {
        let signal = Signal::error(3, SignalErrorCode::NegotiationTimeout, 8);
        assert_eq!(signal.data, "2");
        assert_eq!(
            Signal::parse(&signal.to_string(), 8).unwrap().error_code(),
            Some(SignalErrorCode::NegotiationTimeout)
        );
    }

    #[test]
    fn non_error_signal_has_no_code() {
        assert_eq!(Signal::offer(1, "sdp".into(), 2).error_code(), None);
    }

    #[test]
    fn malformed_signals_rejected() {
        assert!(Signal::parse("", 0).is_err());
        assert!(Signal::parse("CONNECTREQUEST", 0).is_err());
        assert!(Signal::parse("CONNECTREQUEST 1", 0).is_err());
        assert!(Signal::parse("BOGUS 1 data", 0).is_err());
        assert!(Signal::parse("CONNECTREQUEST notanumber data", 0).is_err());
    }
}
