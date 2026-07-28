//! 公共类型：可靠性等级与发送优先级。

/// RakNet 帧可靠性等级（线上取值 0-7）。
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum RakReliability {
    Unreliable = 0,
    UnreliableSequenced = 1,
    Reliable = 2,
    ReliableOrdered = 3,
    ReliableSequenced = 4,
    UnreliableWithAckReceipt = 5,
    ReliableWithAckReceipt = 6,
    ReliableOrderedWithAckReceipt = 7,
}

impl RakReliability {
    #[inline]
    pub fn is_reliable(self) -> bool {
        matches!(
            self,
            Self::Reliable
                | Self::ReliableOrdered
                | Self::ReliableSequenced
                | Self::ReliableWithAckReceipt
                | Self::ReliableOrderedWithAckReceipt
        )
    }

    #[inline]
    pub fn is_sequenced(self) -> bool {
        matches!(self, Self::UnreliableSequenced | Self::ReliableSequenced)
    }

    #[inline]
    pub fn is_ordered(self) -> bool {
        matches!(
            self,
            Self::ReliableOrdered | Self::ReliableOrderedWithAckReceipt
        )
    }

    /// 拆分时的升级规则：不可靠消息一旦拆分必须升级为可靠，
    /// 否则任一分片丢失都无法重组。
    #[inline]
    pub fn upgrade_for_split(self) -> Self {
        match self {
            Self::Unreliable => Self::Reliable,
            Self::UnreliableSequenced => Self::ReliableSequenced,
            Self::UnreliableWithAckReceipt => Self::ReliableWithAckReceipt,
            other => other,
        }
    }
}

impl TryFrom<u8> for RakReliability {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0 => Self::Unreliable,
            1 => Self::UnreliableSequenced,
            2 => Self::Reliable,
            3 => Self::ReliableOrdered,
            4 => Self::ReliableSequenced,
            5 => Self::UnreliableWithAckReceipt,
            6 => Self::ReliableWithAckReceipt,
            7 => Self::ReliableOrderedWithAckReceipt,
            _ => return Err(()),
        })
    }
}

impl From<RakReliability> for u8 {
    fn from(value: RakReliability) -> Self {
        value as u8
    }
}

/// 发送优先级。`Immediate` 不受拥塞窗口约束立即发出，
/// 其余等级在窗口受限时按优先级出队。
#[derive(Copy, Clone, Debug, Ord, PartialOrd, Eq, PartialEq, Hash)]
pub enum RakPriority {
    Immediate,
    High,
    Normal,
    Low,
}

impl RakPriority {
    #[inline]
    pub(crate) fn queue_index(self) -> usize {
        match self {
            Self::Immediate => 0,
            Self::High => 1,
            Self::Normal => 2,
            Self::Low => 3,
        }
    }
}
