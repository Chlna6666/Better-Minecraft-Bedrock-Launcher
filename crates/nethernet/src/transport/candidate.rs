//! ICE 候选交换模式与信令投递。

use crate::error::{NethernetError, Result};
use crate::protocol::Signal;
use crate::signaling::LanSignaling;
use crate::transport::negotiate::format_candidate;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_gatherer::RTCIceGatherer;

const LOCAL_CANDIDATE_CAPACITY: usize = 64;

/// ICE 候选通过独立信令增量投递，或等待收集完成后内联进 SDP。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IceExchangeMode {
    #[default]
    Trickle,
    NonTrickle,
}

impl IceExchangeMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Trickle => "trickle",
            Self::NonTrickle => "non-trickle",
        }
    }
}

/// `CANDIDATEADD` 信令的数据编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CandidateEncoding {
    /// 完整序列化 `RTCIceCandidateInit`，与 bedrock-crustaceans/nethernet 一致。
    Json,
    /// C++ WebRTC 使用的裸 candidate 文本，与 go-nethernet 一致。
    #[default]
    Cpp,
}

impl CandidateEncoding {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Cpp => "cpp",
        }
    }
}

/// `NetherNet` 的 ICE 信令交换配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NegotiationConfig {
    pub ice_exchange: IceExchangeMode,
    pub candidate_encoding: CandidateEncoding,
}

impl NegotiationConfig {
    #[must_use]
    pub const fn new(ice_exchange: IceExchangeMode, candidate_encoding: CandidateEncoding) -> Self {
        Self {
            ice_exchange,
            candidate_encoding,
        }
    }

    #[must_use]
    pub const fn json_trickle() -> Self {
        Self::new(IceExchangeMode::Trickle, CandidateEncoding::Json)
    }

    #[must_use]
    pub const fn cpp_trickle() -> Self {
        Self::new(IceExchangeMode::Trickle, CandidateEncoding::Cpp)
    }

    #[must_use]
    pub const fn non_trickle() -> Self {
        Self::new(IceExchangeMode::NonTrickle, CandidateEncoding::Cpp)
    }
}

pub(crate) enum LocalCandidateEvent {
    Candidate { index: usize, data: String },
    Complete { count: usize },
}

pub(crate) fn install_ortc_candidate_queue(
    gatherer: &RTCIceGatherer,
    ice_ufrag: String,
    encoding: CandidateEncoding,
    connection_id: u64,
    remote_network_id: u64,
) -> mpsc::Receiver<LocalCandidateEvent> {
    let (candidate_tx, candidate_rx) = mpsc::channel(LOCAL_CANDIDATE_CAPACITY);
    let candidate_count = Arc::new(AtomicUsize::new(0));
    gatherer.on_local_candidate(Box::new(move |candidate| {
        let candidate_tx = candidate_tx.clone();
        let candidate_count = Arc::clone(&candidate_count);
        let ice_ufrag = ice_ufrag.clone();
        Box::pin(async move {
            let Some(candidate) = candidate else {
                let count = candidate_count.load(Ordering::Relaxed);
                if candidate_tx
                    .send(LocalCandidateEvent::Complete { count })
                    .await
                    .is_err()
                {
                    tracing::debug!(
                        connection_id,
                        remote_network_id,
                        "NetherNet ORTC 本地 ICE 候选接收端已关闭"
                    );
                }
                return;
            };
            let candidate = match candidate.to_json() {
                Ok(candidate) => candidate,
                Err(error) => {
                    tracing::warn!(
                        connection_id,
                        remote_network_id,
                        "生成 NetherNet ORTC 本地 ICE 候选失败：{error}"
                    );
                    return;
                }
            };
            let index = candidate_count.fetch_add(1, Ordering::Relaxed);
            let data = match encode_candidate(&candidate, &ice_ufrag, index, encoding) {
                Ok(data) => data,
                Err(error) => {
                    tracing::warn!(
                        connection_id,
                        remote_network_id,
                        "序列化 NetherNet ORTC 本地 ICE 候选失败：{error}"
                    );
                    return;
                }
            };
            if candidate_tx
                .send(LocalCandidateEvent::Candidate { index, data })
                .await
                .is_err()
            {
                tracing::debug!(
                    connection_id,
                    remote_network_id,
                    "NetherNet ORTC 本地 ICE 候选接收端已关闭"
                );
            }
        })
    }));
    candidate_rx
}

pub(crate) fn spawn_local_candidate_sender(
    signaling: Arc<LanSignaling>,
    mut candidates: mpsc::Receiver<LocalCandidateEvent>,
    connection_id: u64,
    remote_network_id: u64,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                () = cancel.cancelled() => break,
                event = candidates.recv() => event,
            };
            let Some(event) = event else { break };
            match event {
                LocalCandidateEvent::Candidate { index, data } => {
                    if let Err(error) = signaling
                        .signal(Signal::candidate(connection_id, data, remote_network_id))
                        .await
                    {
                        tracing::warn!(
                            connection_id,
                            remote_network_id,
                            "发送 NetherNet 本地 ICE 候选失败：{error}"
                        );
                        break;
                    }
                    tracing::info!(
                        connection_id,
                        remote_network_id,
                        candidate_index = index,
                        "NetherNet 本地 ICE 候选已发送"
                    );
                }
                LocalCandidateEvent::Complete { count } => {
                    tracing::info!(
                        connection_id,
                        remote_network_id,
                        candidate_count = count,
                        "NetherNet 本地 ICE 候选收集与投递完成"
                    );
                    break;
                }
            }
        }
    });
}

fn encode_candidate(
    candidate: &RTCIceCandidateInit,
    ice_ufrag: &str,
    network_id: usize,
    encoding: CandidateEncoding,
) -> Result<String> {
    match encoding {
        CandidateEncoding::Json => serde_json::to_string(candidate)
            .map_err(|error| NethernetError::protocol(format!("ICE 候选 JSON 编码失败：{error}"))),
        CandidateEncoding::Cpp => Ok(format_candidate(
            &candidate.candidate,
            ice_ufrag,
            network_id,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> RTCIceCandidateInit {
        RTCIceCandidateInit {
            candidate: "candidate:1 1 udp 1 10.0.0.2 5001 typ host".to_string(),
            sdp_mid: Some("0".to_string()),
            sdp_mline_index: Some(0),
            username_fragment: Some("AbCd".to_string()),
        }
    }

    #[test]
    fn defaults_to_minecraft_cpp_trickle() {
        assert_eq!(
            NegotiationConfig::default(),
            NegotiationConfig::cpp_trickle()
        );
    }

    #[test]
    fn json_encoding_preserves_candidate_init_fields() {
        let encoded = encode_candidate(&candidate(), "AbCd", 0, CandidateEncoding::Json)
            .expect("JSON 编码候选");
        let decoded: RTCIceCandidateInit = serde_json::from_str(&encoded).expect("JSON 解码候选");
        assert_eq!(decoded, candidate());
    }

    #[test]
    fn cpp_encoding_uses_raw_candidate_format() {
        let encoded = encode_candidate(&candidate(), "AbCd", 3, CandidateEncoding::Cpp)
            .expect("C++ 编码候选");
        assert!(encoded.starts_with("candidate:"));
        assert!(encoded.contains("network-id 3"));
    }
}
