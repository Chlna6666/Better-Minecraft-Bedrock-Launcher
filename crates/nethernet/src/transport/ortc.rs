//! 入站 `NetherNet` 使用的显式 ORTC 传输栈。

use crate::consts::SCTP_MAX_MESSAGE_SIZE;
use crate::error::{NethernetError, Result};
use crate::transport::negotiate::{create_api, format_candidate};
use std::fmt::Write;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use webrtc::data_channel::RTCDataChannel;
use webrtc::dtls_transport::RTCDtlsTransport;
use webrtc::dtls_transport::dtls_fingerprint::RTCDtlsFingerprint;
use webrtc::dtls_transport::dtls_parameters::DTLSParameters;
use webrtc::dtls_transport::dtls_role::DTLSRole;
use webrtc::ice::candidate::Candidate;
use webrtc::ice::candidate::candidate_base::unmarshal_candidate;
use webrtc::ice_transport::RTCIceTransport;
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;
use webrtc::ice_transport::ice_candidate_type::RTCIceCandidateType;
use webrtc::ice_transport::ice_gatherer::{RTCIceGatherOptions, RTCIceGatherer};
use webrtc::ice_transport::ice_parameters::RTCIceParameters;
use webrtc::ice_transport::ice_protocol::RTCIceProtocol;
use webrtc::ice_transport::ice_role::RTCIceRole;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::sctp_transport::RTCSctpTransport;
use webrtc::sctp_transport::sctp_transport_capabilities::SCTPTransportCapabilities;

const SCTP_PORT: u16 = 5000;

pub(crate) struct RemoteDescription {
    pub(crate) ice: RTCIceParameters,
    pub(crate) dtls: DTLSParameters,
    pub(crate) sctp: SCTPTransportCapabilities,
    pub(crate) sctp_port: u16,
    pub(crate) candidates: Vec<RTCIceCandidate>,
}

#[derive(Default)]
pub(crate) struct CandidateSummary {
    pub(crate) host: usize,
    pub(crate) server_reflexive: usize,
    pub(crate) peer_reflexive: usize,
    pub(crate) relay: usize,
    pub(crate) udp: usize,
    pub(crate) tcp: usize,
    pub(crate) ipv4: usize,
    pub(crate) ipv6: usize,
    pub(crate) mdns: usize,
    pub(crate) other_address: usize,
}

pub(crate) fn summarize_candidates(candidates: &[RTCIceCandidate]) -> CandidateSummary {
    let mut summary = CandidateSummary::default();
    for candidate in candidates {
        match candidate.typ {
            RTCIceCandidateType::Host => summary.host += 1,
            RTCIceCandidateType::Srflx => summary.server_reflexive += 1,
            RTCIceCandidateType::Prflx => summary.peer_reflexive += 1,
            RTCIceCandidateType::Relay => summary.relay += 1,
            RTCIceCandidateType::Unspecified => {}
        }
        match candidate.protocol {
            RTCIceProtocol::Udp => summary.udp += 1,
            RTCIceProtocol::Tcp => summary.tcp += 1,
            RTCIceProtocol::Unspecified => {}
        }
        match candidate.address.parse::<IpAddr>() {
            Ok(IpAddr::V4(_)) => summary.ipv4 += 1,
            Ok(IpAddr::V6(_)) => summary.ipv6 += 1,
            Err(_) if candidate.address.ends_with(".local") => summary.mdns += 1,
            Err(_) => summary.other_address += 1,
        }
    }
    summary
}

pub(crate) struct OrtcStack {
    pub(crate) gatherer: Arc<RTCIceGatherer>,
    pub(crate) ice: Arc<RTCIceTransport>,
    pub(crate) dtls: Arc<RTCDtlsTransport>,
    pub(crate) sctp: Arc<RTCSctpTransport>,
    closed: AtomicBool,
}

impl OrtcStack {
    pub(crate) async fn new() -> Result<Arc<Self>> {
        let api = create_api();
        let gatherer = Arc::new(api.new_ice_gatherer(RTCIceGatherOptions::default())?);
        let ice = Arc::new(api.new_ice_transport(Arc::clone(&gatherer)));
        let dtls = Arc::new(api.new_dtls_transport(Arc::clone(&ice), Vec::new())?);
        let sctp = Arc::new(api.new_sctp_transport(Arc::clone(&dtls))?);
        // 在发送 Answer 前创建 ICE agent 和凭据；Gather 仍由协商模式决定何时启动。
        gatherer.get_local_parameters().await?;
        Ok(Arc::new(Self {
            gatherer,
            ice,
            dtls,
            sctp,
            closed: AtomicBool::new(false),
        }))
    }

    pub(crate) async fn local_parameters(&self) -> Result<(RTCIceParameters, DTLSParameters)> {
        Ok((
            self.gatherer.get_local_parameters().await?,
            self.dtls.get_local_parameters()?,
        ))
    }

    pub(crate) async fn add_remote_candidates(&self, candidates: &[RTCIceCandidate]) -> Result<()> {
        self.ice.set_remote_candidates(candidates).await?;
        Ok(())
    }

    pub(crate) async fn add_remote_candidate(&self, candidate: RTCIceCandidate) -> Result<()> {
        self.ice.add_remote_candidate(Some(candidate)).await?;
        Ok(())
    }

    pub(crate) async fn start(&self, remote: &RemoteDescription) -> Result<()> {
        self.ice
            .start(&remote.ice, Some(RTCIceRole::Controlled))
            .await?;
        self.dtls.start(remote.dtls.clone()).await?;
        self.sctp
            .start(remote.sctp, SCTP_PORT, remote.sctp_port)
            .await?;
        Ok(())
    }

    pub(crate) async fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let mut first_error = None;
        for result in [
            self.sctp.stop().await,
            self.dtls.stop().await,
            self.ice.stop().await,
        ] {
            if let Err(error) = result {
                if first_error.is_none() {
                    first_error = Some(error);
                } else {
                    tracing::debug!("清理 NetherNet ORTC 传输失败：{error}");
                }
            }
        }
        match first_error {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }

    pub(crate) fn install_incoming_channel_handler(
        &self,
        handler: impl FnMut(Arc<RTCDataChannel>) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync
        + 'static,
    ) {
        self.sctp.on_data_channel(Box::new(handler));
    }
}

pub(crate) fn parse_offer(sdp: &str) -> Result<RemoteDescription> {
    let parsed = RTCSessionDescription::offer(sdp.to_string())?.unmarshal()?;
    if parsed.media_descriptions.len() != 1 {
        return Err(NethernetError::protocol(format!(
            "NetherNet Offer 媒体段数量无效：{}",
            parsed.media_descriptions.len()
        )));
    }
    let media = &parsed.media_descriptions[0];
    if media.media_name.media != "application" {
        return Err(NethernetError::protocol(
            "NetherNet Offer 缺少 application 媒体段",
        ));
    }

    let ice_ufrag = media_attribute(media, "ice-ufrag")
        .or_else(|| parsed.attribute("ice-ufrag").map(String::as_str))
        .ok_or_else(|| NethernetError::protocol("NetherNet Offer 缺少 ICE ufrag"))?;
    let ice_password = media_attribute(media, "ice-pwd")
        .or_else(|| parsed.attribute("ice-pwd").map(String::as_str))
        .ok_or_else(|| NethernetError::protocol("NetherNet Offer 缺少 ICE pwd"))?;
    let fingerprint = media_attribute(media, "fingerprint")
        .or_else(|| parsed.attribute("fingerprint").map(String::as_str))
        .ok_or_else(|| NethernetError::protocol("NetherNet Offer 缺少 DTLS fingerprint"))?;
    let (fingerprint_algorithm, fingerprint_value) = fingerprint
        .split_once(' ')
        .ok_or_else(|| NethernetError::protocol("NetherNet Offer fingerprint 格式无效"))?;
    let remote_role = match media_attribute(media, "setup") {
        Some("active") => DTLSRole::Client,
        Some("passive") => DTLSRole::Server,
        Some("actpass") | None => DTLSRole::Auto,
        Some(role) => {
            return Err(NethernetError::protocol(format!(
                "NetherNet Offer DTLS setup 无效：{role}"
            )));
        }
    };
    let max_message_size = media_attribute(media, "max-message-size")
        .and_then(|value| value.parse().ok())
        .unwrap_or(SCTP_MAX_MESSAGE_SIZE);
    let sctp_port = media_attribute(media, "sctp-port")
        .and_then(|value| value.parse().ok())
        .unwrap_or(SCTP_PORT);

    let mut candidates = Vec::new();
    for attribute in parsed.attributes.iter().chain(&media.attributes) {
        if attribute.key == "candidate"
            && let Some(value) = &attribute.value
        {
            candidates.push(parse_ice_candidate(value)?);
        }
    }

    Ok(RemoteDescription {
        ice: RTCIceParameters {
            username_fragment: ice_ufrag.to_string(),
            password: ice_password.to_string(),
            ice_lite: false,
        },
        dtls: DTLSParameters {
            role: remote_role,
            fingerprints: vec![RTCDtlsFingerprint {
                algorithm: fingerprint_algorithm.to_string(),
                value: fingerprint_value.to_string(),
            }],
        },
        sctp: SCTPTransportCapabilities { max_message_size },
        sctp_port,
        candidates,
    })
}

pub(crate) fn parse_ice_candidate(value: &str) -> Result<RTCIceCandidate> {
    let value = value.trim();
    let value = value.strip_prefix("a=").unwrap_or(value);
    let value = value.strip_prefix("candidate:").unwrap_or(value);
    let candidate = unmarshal_candidate(value)
        .map_err(|error| NethernetError::protocol(format!("解析 ICE 候选失败：{error}")))?;
    let related = candidate.related_address();
    Ok(RTCIceCandidate {
        stats_id: candidate.id(),
        foundation: candidate.foundation(),
        priority: candidate.priority(),
        address: candidate.address(),
        protocol: RTCIceProtocol::from(candidate.network_type().network_short().as_str()),
        port: candidate.port(),
        typ: RTCIceCandidateType::from(candidate.candidate_type()),
        component: candidate.component(),
        related_address: related
            .as_ref()
            .map_or_else(String::new, |address| address.address.clone()),
        related_port: related.as_ref().map_or(0, |address| address.port),
        tcp_type: candidate.tcp_type().to_string(),
    })
}

pub(crate) fn build_answer(
    ice: &RTCIceParameters,
    dtls: &DTLSParameters,
    candidates: &[RTCIceCandidate],
) -> Result<String> {
    if dtls.fingerprints.is_empty() {
        return Err(NethernetError::protocol(
            "NetherNet 本地 DTLS fingerprint 缺失",
        ));
    }
    let session_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| NethernetError::protocol(format!("创建 SDP 会话编号失败：{error}")))?
        .as_nanos();
    let mut answer = format!(
        "v=0\r\no=- {session_id} 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\n\
a=ice-ufrag:{}\r\na=ice-pwd:{}\r\na=ice-options:trickle\r\n",
        ice.username_fragment, ice.password
    );
    for (index, candidate) in candidates.iter().enumerate() {
        let candidate = candidate.to_json()?;
        answer.push_str("a=");
        answer.push_str(&format_candidate(
            &candidate.candidate,
            &ice.username_fragment,
            index,
        ));
        answer.push_str("\r\n");
    }
    for fingerprint in &dtls.fingerprints {
        write!(
            answer,
            "a=fingerprint:{} {}\r\n",
            fingerprint.algorithm, fingerprint.value
        )
        .map_err(|error| NethernetError::protocol(format!("创建 SDP fingerprint 失败：{error}")))?;
    }
    write!(
        answer,
        "a=setup:active\r\na=mid:0\r\na=sctp-port:{SCTP_PORT}\r\n\
a=max-message-size:{SCTP_MAX_MESSAGE_SIZE}\r\n"
    )
    .map_err(|error| NethernetError::protocol(format!("创建 SDP SCTP 属性失败：{error}")))?;
    Ok(answer)
}

fn media_attribute<'a>(
    media: &'a webrtc::sdp::description::media::MediaDescription,
    key: &str,
) -> Option<&'a str> {
    media.attribute(key).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    const OFFER: &str = "v=0\r\n\
o=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\n\
a=ice-ufrag:remote\r\na=ice-pwd:remote-password\r\n\
a=fingerprint:sha-256 00:11:22:33\r\na=setup:actpass\r\na=mid:0\r\n\
a=sctp-port:5000\r\na=max-message-size:262144\r\n\
a=candidate:1 1 udp 2130706431 127.0.0.1 50000 typ host\r\n";

    #[test]
    fn parses_minecraft_offer_transport_parameters() {
        let remote = parse_offer(OFFER).expect("Offer 应解析");
        assert_eq!(remote.ice.username_fragment, "remote");
        assert_eq!(remote.dtls.role, DTLSRole::Auto);
        assert_eq!(remote.sctp.max_message_size, SCTP_MAX_MESSAGE_SIZE);
        assert_eq!(remote.sctp_port, SCTP_PORT);
        assert_eq!(remote.candidates.len(), 1);
        let summary = summarize_candidates(&remote.candidates);
        assert_eq!(summary.host, 1);
        assert_eq!(summary.udp, 1);
        assert_eq!(summary.ipv4, 1);
    }

    #[tokio::test]
    async fn answer_uses_local_ortc_parameters() {
        let stack = OrtcStack::new().await.expect("ORTC 栈应创建");
        let (ice, dtls) = stack.local_parameters().await.expect("本地参数应可读");
        let answer = build_answer(&ice, &dtls, &[]).expect("Answer 应创建");
        assert!(answer.contains("a=setup:active\r\n"));
        assert!(answer.contains("a=sctp-port:5000\r\n"));
        assert!(answer.contains("a=max-message-size:262144\r\n"));
        assert!(answer.contains("a=fingerprint:"));
        stack.close().await.expect("ORTC 栈应关闭");
    }
}
