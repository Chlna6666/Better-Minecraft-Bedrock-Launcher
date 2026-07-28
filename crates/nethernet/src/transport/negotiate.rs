//! WebRTC 协商辅助：对等连接构造、SDP 修补与 ICE 候选格式。

use crate::consts::{RELIABLE_CHANNEL, SCTP_MAX_MESSAGE_SIZE, UNRELIABLE_CHANNEL};
use crate::error::{NethernetError, Result};
use std::sync::Arc;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::{SctpMaxMessageSize, SettingEngine};
use webrtc::api::{API, APIBuilder};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::ice::mdns::MulticastDnsMode;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;

pub(crate) struct SdpSummary {
    pub(crate) setup_role: String,
    pub(crate) inline_candidates: usize,
    pub(crate) has_identity: bool,
    pub(crate) max_message_size: Option<usize>,
}

#[must_use]
pub(crate) fn summarize_sdp(sdp: &str) -> SdpSummary {
    let setup_role = sdp
        .lines()
        .find_map(|line| line.trim().strip_prefix("a=setup:"))
        .unwrap_or("missing")
        .to_string();
    let inline_candidates = sdp
        .lines()
        .filter(|line| line.trim().starts_with("a=candidate:"))
        .count();
    let has_identity = sdp
        .lines()
        .any(|line| line.trim().starts_with("a=identity:"));
    let max_message_size = sdp.lines().find_map(|line| {
        line.trim()
            .strip_prefix("a=max-message-size:")
            .and_then(|value| value.parse().ok())
    });
    SdpSummary {
        setup_role,
        inline_candidates,
        has_identity,
        max_message_size,
    }
}

/// 建立一个按 `NetherNet` 要求配置的对等连接。
///
/// # Errors
///
/// WebRTC API 构造失败时返回错误。
pub async fn create_peer_connection() -> Result<Arc<RTCPeerConnection>> {
    let api = create_api();
    Ok(Arc::new(
        api.new_peer_connection(RTCConfiguration::default()).await?,
    ))
}

pub(crate) fn create_api() -> API {
    let mut setting_engine = SettingEngine::default();
    // 使用 webrtc-rs 默认的 IPv4/IPv6 和接口集合，与 GravityCone 的默认 API 一致。
    // 接受 Minecraft 提供的 mDNS 候选。
    // 禁用 mDNS 会让带 `.local` 候选的客户端无法建立 ICE。
    setting_engine.set_ice_multicast_dns_mode(MulticastDnsMode::QueryOnly);
    // NetherNet 的分片是 256 KiB−1，而 webrtc-rs 有两处 64 KiB 硬限：
    //
    // 1. 发送侧：SCTP 单消息上限默认 65536，超过即 ErrOutboundPacketTooLarge。
    //    这里放开到 262144。
    // 2. 接收侧：`RTCDataChannel::read_loop` 用固定的 65535 字节缓冲
    //    （webrtc-0.17.2 data_channel/mod.rs:33），更大的入站消息会读失败
    //    并关闭通道，且**无法通过配置修改**。因此必须启用 detach，由我们
    //    自己按 256 KiB 缓冲跑读循环（见 session.rs）。
    setting_engine
        .set_sctp_max_message_size_can_send(SctpMaxMessageSize::Bounded(SCTP_MAX_MESSAGE_SIZE));
    setting_engine.detach_data_channels();

    APIBuilder::new()
        .with_media_engine(MediaEngine::default())
        .with_setting_engine(setting_engine)
        .build()
}

/// 创建 `NetherNet` 约定的两条数据通道。
///
/// # Errors
///
/// 通道创建失败时返回错误。
pub async fn create_data_channels(
    peer_connection: &RTCPeerConnection,
) -> Result<(Arc<RTCDataChannel>, Arc<RTCDataChannel>)> {
    let reliable = peer_connection
        .create_data_channel(
            RELIABLE_CHANNEL,
            Some(RTCDataChannelInit {
                ordered: Some(true),
                ..Default::default()
            }),
        )
        .await?;
    let unreliable = peer_connection
        .create_data_channel(
            UNRELIABLE_CHANNEL,
            Some(RTCDataChannelInit {
                ordered: Some(false),
                max_retransmits: Some(0),
                ..Default::default()
            }),
        )
        .await?;
    Ok((reliable, unreliable))
}

/// 在 SDP 的媒体段中补齐 `a=max-message-size`。
///
/// webrtc-rs 不会生成该属性，而对端按 RFC 8841 会默认认为上限是 64 KiB，
/// 于是把自己的分片压到 64 KiB。vanilla 通告的是 262144，我们照做。
#[must_use]
pub fn patch_max_message_size(sdp: &str) -> String {
    if sdp.contains("a=max-message-size:") {
        return sdp.to_string();
    }
    let attribute = format!("a=max-message-size:{SCTP_MAX_MESSAGE_SIZE}\r\n");
    // 插到第一个媒体段内部：紧跟 a=sctp-port 之后最稳妥，
    // 没有该属性时退化为追加到末尾。
    if let Some(position) = sdp.find("a=sctp-port:") {
        let insert_at = sdp[position..]
            .find("\r\n")
            .map_or(sdp.len(), |offset| position + offset + 2);
        let mut patched = String::with_capacity(sdp.len() + attribute.len());
        patched.push_str(&sdp[..insert_at]);
        patched.push_str(&attribute);
        patched.push_str(&sdp[insert_at..]);
        return patched;
    }
    let mut patched = String::with_capacity(sdp.len() + attribute.len());
    patched.push_str(sdp);
    if !patched.ends_with("\r\n") {
        patched.push_str("\r\n");
    }
    patched.push_str(&attribute);
    patched
}

/// 设置本地描述并等待 ICE 收集完成，返回可直接信令的 SDP。
///
/// 采用非 trickle 模式：候选全部内联在 SDP 中，对端只需解析一次描述。
///
/// # Errors
///
/// 设置描述失败或本地描述缺失时返回错误。
pub async fn finish_local_description(
    peer_connection: &RTCPeerConnection,
    description: RTCSessionDescription,
) -> Result<String> {
    let mut gathering_complete = peer_connection.gathering_complete_promise().await;
    peer_connection.set_local_description(description).await?;
    gathering_complete.recv().await;
    let sdp = peer_connection
        .local_description()
        .await
        .map(|description| description.sdp)
        .ok_or_else(|| NethernetError::protocol("WebRTC 本地描述缺失"))?;
    Ok(patch_max_message_size(&sdp))
}

/// 解析对端信令来的 ICE 候选。
///
/// vanilla 与 go-nethernet 发送的是 C++ WebRTC 风格的裸候选文本
/// （`candidate:...`），部分实现会发 JSON，两者都接受。
#[must_use]
pub fn parse_candidate(data: &str) -> RTCIceCandidateInit {
    let text = data.trim();
    if text.starts_with('{')
        && let Ok(parsed) = serde_json::from_str::<RTCIceCandidateInit>(text)
    {
        return parsed;
    }
    RTCIceCandidateInit {
        candidate: text.to_string(),
        // vanilla 只有一个媒体段，mid 固定为 "0"。
        sdp_mid: Some("0".to_string()),
        sdp_mline_index: Some(0),
        username_fragment: None,
    }
}

/// 把本端候选格式化为 C++ WebRTC 风格文本，供 `CANDIDATEADD` 信令使用。
///
/// vanilla 只认这种格式；发 JSON 会被直接忽略。
#[must_use]
pub fn format_candidate(candidate: &str, ufrag: &str, network_id: usize) -> String {
    let trimmed = candidate.trim();
    let body = trimmed.strip_prefix("candidate:").unwrap_or(trimmed);
    if body.contains(" ufrag ") {
        return format!("candidate:{body}");
    }
    format!("candidate:{body} generation 0 ufrag {ufrag} network-id {network_id} network-cost 0")
}

/// 从 SDP 中提取 `ice-ufrag`。
#[must_use]
pub fn extract_ufrag(sdp: &str) -> Option<String> {
    sdp.lines()
        .find_map(|line| line.trim().strip_prefix("a=ice-ufrag:"))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SDP: &str = "v=0\r\n\
o=- 1 2 IN IP4 127.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
c=IN IP4 0.0.0.0\r\n\
a=ice-ufrag:AbCd\r\n\
a=ice-pwd:secret\r\n\
a=mid:0\r\n\
a=sctp-port:5000\r\n";

    #[test]
    fn sdp_summary_reports_only_non_sensitive_negotiation_metadata() {
        let summary = summarize_sdp(
            "v=0\r\n\
a=identity:redacted\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
a=setup:active\r\n\
a=candidate:redacted-one\r\n\
a=candidate:redacted-two\r\n\
a=max-message-size:262144\r\n",
        );

        assert_eq!(
            (
                summary.setup_role.as_str(),
                summary.inline_candidates,
                summary.has_identity,
                summary.max_message_size,
            ),
            ("active", 2, true, Some(262_144))
        );
    }

    #[test]
    fn patch_inserts_after_sctp_port() {
        let patched = patch_max_message_size(SDP);
        let sctp = patched.find("a=sctp-port:5000\r\n").unwrap();
        let attr = patched.find("a=max-message-size:262144\r\n").unwrap();
        assert!(attr > sctp, "属性应插在 sctp-port 之后");
        assert!(patched.starts_with("v=0\r\n"));
        assert!(patched.contains("a=mid:0\r\n"));
    }

    #[test]
    fn patch_is_idempotent() {
        let once = patch_max_message_size(SDP);
        assert_eq!(patch_max_message_size(&once), once);
        assert_eq!(once.matches("a=max-message-size").count(), 1);
    }

    #[test]
    fn patch_appends_when_no_sctp_port() {
        let patched = patch_max_message_size("v=0\r\nm=application 9 UDP/DTLS/SCTP\r\n");
        assert!(patched.ends_with("a=max-message-size:262144\r\n"));
    }

    #[test]
    fn advertised_size_matches_segment_limit() {
        assert_eq!(
            SCTP_MAX_MESSAGE_SIZE as usize,
            crate::consts::MAX_SEGMENT_PAYLOAD + 1
        );
    }

    #[test]
    fn extracts_ufrag() {
        assert_eq!(extract_ufrag(SDP).as_deref(), Some("AbCd"));
        assert_eq!(extract_ufrag("v=0\r\n"), None);
    }

    #[test]
    fn formats_candidate_in_cpp_style() {
        let formatted = format_candidate(
            "candidate:1 1 udp 2130706431 192.168.1.5 50000 typ host",
            "AbCd",
            0,
        );
        assert_eq!(
            formatted,
            "candidate:1 1 udp 2130706431 192.168.1.5 50000 typ host \
generation 0 ufrag AbCd network-id 0 network-cost 0"
        );
    }

    #[test]
    fn format_candidate_preserves_existing_ufrag() {
        let original = "candidate:1 1 udp 1 1.2.3.4 5 typ host generation 0 ufrag ZZ network-id 1";
        assert_eq!(format_candidate(original, "AbCd", 0), original);
    }

    #[test]
    fn parses_raw_and_json_candidates() {
        let raw = parse_candidate("candidate:1 1 udp 2130706431 10.0.0.1 5000 typ host");
        assert!(raw.candidate.starts_with("candidate:"));
        assert_eq!(raw.sdp_mid.as_deref(), Some("0"));

        let json = parse_candidate(
            r#"{"candidate":"candidate:2 1 udp 1 10.0.0.2 5001 typ host","sdpMid":"0","sdpMLineIndex":0}"#,
        );
        assert!(json.candidate.contains("10.0.0.2"));
        assert_eq!(json.sdp_mline_index, Some(0));
    }

    #[test]
    fn parse_candidate_tolerates_malformed_json() {
        let candidate = parse_candidate("{not json");
        assert_eq!(candidate.candidate, "{not json");
    }
}
