//! Privacy-safe diagnostics for inbound NetherNet negotiation.

use crate::protocol::Signal;
use crate::session::NethernetSession;
use crate::transport::negotiate::summarize_sdp;
use crate::transport::ortc::{OrtcStack, summarize_candidates};
use webrtc::ice_transport::ice_candidate::RTCIceCandidate;

pub(super) fn log_offer_summary(offer: &Signal) {
    let summary = summarize_sdp(&offer.data);
    tracing::info!(
        connection_id = offer.connection_id,
        remote_network_id = offer.network_id,
        setup_role = summary.setup_role,
        inline_candidates = summary.inline_candidates,
        has_identity = summary.has_identity,
        max_message_size = ?summary.max_message_size,
        "NetherNet Offer 摘要"
    );
}

pub(super) async fn log_negotiation_timeout(
    stack: &OrtcStack,
    session: &NethernetSession,
    connection_id: u64,
    remote_network_id: u64,
    remote_candidate_count: usize,
) {
    let local_candidates = match stack.gatherer.get_local_candidates().await {
        Ok(candidates) => candidates,
        Err(error) => {
            tracing::warn!(
                connection_id,
                remote_network_id,
                "读取 NetherNet 本地 ICE 候选失败：{error}"
            );
            Vec::new()
        }
    };
    log_candidate_summary(
        "NetherNet 本地 ICE 候选摘要",
        connection_id,
        remote_network_id,
        &local_candidates,
    );
    let selected_pair = stack.ice.get_selected_candidate_pair().await.is_some();
    let diagnostics = stack.ice_diagnostics().await;
    tracing::warn!(
        connection_id,
        remote_network_id,
        ice_state = ?stack.ice.state(),
        dtls_state = ?stack.dtls.state(),
        sctp_state = ?stack.sctp.state(),
        open_channels = session.open_channel_count(),
        expected_channels = 2,
        local_candidates = local_candidates.len(),
        remote_candidates = remote_candidate_count,
        candidate_pairs = diagnostics.candidate_pairs,
        waiting_pairs = diagnostics.waiting_pairs,
        in_progress_pairs = diagnostics.in_progress_pairs,
        succeeded_pairs = diagnostics.succeeded_pairs,
        failed_pairs = diagnostics.failed_pairs,
        requests_received = diagnostics.requests_received,
        requests_sent = diagnostics.requests_sent,
        responses_received = diagnostics.responses_received,
        responses_sent = diagnostics.responses_sent,
        remote_binding_request_received = diagnostics.requests_received > 0,
        selected_pair,
        "NetherNet 协商超时状态"
    );
}

pub(super) fn log_candidate_summary(
    scope: &'static str,
    connection_id: u64,
    remote_network_id: u64,
    candidates: &[RTCIceCandidate],
) {
    let summary = summarize_candidates(candidates);
    tracing::info!(
        connection_id,
        remote_network_id,
        total = candidates.len(),
        host = summary.host,
        server_reflexive = summary.server_reflexive,
        peer_reflexive = summary.peer_reflexive,
        relay = summary.relay,
        udp = summary.udp,
        tcp = summary.tcp,
        ipv4 = summary.ipv4,
        ipv6 = summary.ipv6,
        mdns = summary.mdns,
        other_address = summary.other_address,
        scope,
        "NetherNet ICE 候选摘要"
    );
}
