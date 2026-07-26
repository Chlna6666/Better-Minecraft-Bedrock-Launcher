//! 错误类型与协议错误码。

use thiserror::Error;

pub type Result<T> = std::result::Result<T, NethernetError>;

/// `CONNECTERROR` 信令携带的错误码。
///
/// 取值与 vanilla / go-nethernet 的 `ErrorCode*` 常量一致，
/// 用于把失败原因告知对端并解释对端发来的失败原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SignalErrorCode {
    None = 0,
    DestinationNotLoggedIn = 1,
    NegotiationTimeout = 2,
    WrongTransportVersion = 3,
    FailedToCreatePeerConnection = 4,
    Ice = 5,
    ConnectRequest = 6,
    ConnectResponse = 7,
    CandidateAdd = 8,
    InactivityTimeout = 9,
    FailedToCreateOffer = 10,
    FailedToCreateAnswer = 11,
    FailedToSetLocalDescription = 12,
    FailedToSetRemoteDescription = 13,
    NegotiationTimeoutWaitingForResponse = 14,
    NegotiationTimeoutWaitingForAccept = 15,
    IncomingConnectionIgnored = 16,
    SignalingParsingFailure = 17,
    SignalingUnknownError = 18,
    SignalingUnicastMessageDeliveryFailed = 19,
    SignalingBroadcastDeliveryFailed = 20,
    SignalingMessageDeliveryFailed = 21,
    SignalingTurnAuthFailed = 22,
    SignalingFallbackToBestEffortDelivery = 23,
    NoSignalingChannel = 24,
    NotLoggedIn = 25,
    SignalingFailedToSend = 26,
    IdentityVerificationFailed = 37,
}

impl SignalErrorCode {
    /// 由线上数值解析；未知取值归入 [`Self::SignalingUnknownError`]。
    #[must_use]
    pub fn from_code(code: u32) -> Self {
        match code {
            0 => Self::None,
            1 => Self::DestinationNotLoggedIn,
            2 => Self::NegotiationTimeout,
            3 => Self::WrongTransportVersion,
            4 => Self::FailedToCreatePeerConnection,
            5 => Self::Ice,
            6 => Self::ConnectRequest,
            7 => Self::ConnectResponse,
            8 => Self::CandidateAdd,
            9 => Self::InactivityTimeout,
            10 => Self::FailedToCreateOffer,
            11 => Self::FailedToCreateAnswer,
            12 => Self::FailedToSetLocalDescription,
            13 => Self::FailedToSetRemoteDescription,
            14 => Self::NegotiationTimeoutWaitingForResponse,
            15 => Self::NegotiationTimeoutWaitingForAccept,
            16 => Self::IncomingConnectionIgnored,
            17 => Self::SignalingParsingFailure,
            19 => Self::SignalingUnicastMessageDeliveryFailed,
            20 => Self::SignalingBroadcastDeliveryFailed,
            21 => Self::SignalingMessageDeliveryFailed,
            22 => Self::SignalingTurnAuthFailed,
            23 => Self::SignalingFallbackToBestEffortDelivery,
            24 => Self::NoSignalingChannel,
            25 => Self::NotLoggedIn,
            26 => Self::SignalingFailedToSend,
            37 => Self::IdentityVerificationFailed,
            _ => Self::SignalingUnknownError,
        }
    }

    #[must_use]
    pub const fn code(self) -> u32 {
        self as u32
    }

    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::None => "无错误",
            Self::DestinationNotLoggedIn => "目标未登录",
            Self::NegotiationTimeout => "协商超时",
            Self::WrongTransportVersion => "传输层版本不匹配",
            Self::FailedToCreatePeerConnection => "创建对等连接失败",
            Self::Ice => "ICE 失败",
            Self::ConnectRequest => "连接请求无效",
            Self::ConnectResponse => "连接响应无效",
            Self::CandidateAdd => "添加 ICE 候选失败",
            Self::InactivityTimeout => "空闲超时",
            Self::FailedToCreateOffer => "创建 offer 失败",
            Self::FailedToCreateAnswer => "创建 answer 失败",
            Self::FailedToSetLocalDescription => "设置本地描述失败",
            Self::FailedToSetRemoteDescription => "设置远端描述失败",
            Self::NegotiationTimeoutWaitingForResponse => "等待响应超时",
            Self::NegotiationTimeoutWaitingForAccept => "等待接受超时",
            Self::IncomingConnectionIgnored => "入站连接被忽略",
            Self::SignalingParsingFailure => "信令解析失败",
            Self::SignalingUnknownError => "未知信令错误",
            Self::SignalingUnicastMessageDeliveryFailed => "单播信令投递失败",
            Self::SignalingBroadcastDeliveryFailed => "广播信令投递失败",
            Self::SignalingMessageDeliveryFailed => "信令投递失败",
            Self::SignalingTurnAuthFailed => "TURN 认证失败",
            Self::SignalingFallbackToBestEffortDelivery => "信令降级为尽力投递",
            Self::NoSignalingChannel => "没有信令通道",
            Self::NotLoggedIn => "未登录",
            Self::SignalingFailedToSend => "信令发送失败",
            Self::IdentityVerificationFailed => "身份校验失败",
        }
    }
}

impl std::fmt::Display for SignalErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（错误码 {}）", self.describe(), self.code())
    }
}

#[derive(Debug, Error)]
pub enum NethernetError {
    #[error("I/O 错误：{0}")]
    Io(#[from] std::io::Error),
    #[error("WebRTC 错误：{0}")]
    WebRtc(#[from] webrtc::Error),
    #[error("协议错误：{0}")]
    Protocol(String),
    #[error("数据截断：需要 {needed} 字节，剩余 {remaining}")]
    Truncated { needed: usize, remaining: usize },
    #[error("消息过大：{size} 字节，上限 {max}")]
    TooLarge { size: usize, max: usize },
    #[error("对端拒绝连接：{0}")]
    Refused(SignalErrorCode),
    #[error("连接超时")]
    Timeout,
    #[error("连接已关闭")]
    Closed,
}

impl NethernetError {
    pub(crate) fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }
}

#[cfg(test)]
mod tests {
    use super::SignalErrorCode;

    #[test]
    fn known_codes_round_trip() {
        for code in [0u32, 1, 5, 9, 16, 26, 37] {
            assert_eq!(SignalErrorCode::from_code(code).code(), code);
        }
    }

    #[test]
    fn unknown_code_maps_to_unknown_error() {
        assert_eq!(
            SignalErrorCode::from_code(9999),
            SignalErrorCode::SignalingUnknownError
        );
    }
}
