use easytier::proto::acl::{Action, Acl, AclV1, Chain, ChainType, GroupInfo, Protocol, Rule};

fn allow_rule(
    name: &str,
    priority: u32,
    protocol: Protocol,
    ports: Vec<String>,
    source_ips: Vec<String>,
    destination_ips: Vec<String>,
    source_ports: Vec<String>,
    app_protocols: Vec<i32>,
    rate_limit: u32,
    burst_limit: u32,
) -> Rule {
    Rule {
        name: name.to_string(),
        description: String::new(),
        priority,
        enabled: true,
        protocol: protocol as i32,
        ports,
        source_ips,
        destination_ips,
        source_ports,
        app_protocols,
        action: Action::Allow as i32,
        rate_limit,
        burst_limit,
        stateful: false,
        source_groups: vec![],
        destination_groups: vec![],
    }
}

// PaperConnect/BDS security policy:
// - Block peer-to-peer traffic between members (joiners).
// - Allow LAN discovery traffic (UDP 7551) so the game can discover the host.
// - Allow game traffic (UDP, any port) between host <-> members.
//
// Notes:
// - Joiners receive replies on ephemeral local UDP ports. For that reason, joiners must not rely on
//   a destination-port whitelist. Instead, they allow inbound UDP packets from the host (any port).
// - PaperConnect control plane uses a TCP "protocol port" on the host. Joiners allow TCP from the
//   host on any source port to keep the policy compatible with dynamic host protocol port.
pub fn build_paperconnect_acl(is_host: bool, host_vip: &str, host_protocol_port: Option<u16>) -> Acl {
    // Highest priority wins (processed in priority order).
    let mut inbound_rules: Vec<Rule> = Vec::new();
    let mut outbound_rules: Vec<Rule> = Vec::new();

    // EasyTier app_protocols:
    // RakNet=10, WebRtc=20, WebRtcStun=21, WebRtcDtls=22, WebRtcRtp=23.
    let bedrock_udp_app_protocols: Vec<i32> = vec![10, 20, 21, 22, 23];
    // 7551 is a LAN-discovery broadcast channel. EasyTier may not be able to classify its
    // application protocol, so we do NOT filter it by app_protocols. Instead, we rate-limit it.
    let discovery_rate_limit: u32 = 20;
    let discovery_burst_limit: u32 = 40;

    if is_host {
        // Inbound: allow LAN discovery broadcast probes (clients send to 10.144.144.255:7551).
        inbound_rules.push(allow_rule(
            "allow_udp_discovery_broadcast_in",
            5000,
            Protocol::Udp,
            vec!["7551".to_string()],
            vec![],
            vec!["10.144.144.255".to_string()],
            vec![],
            vec![],
            discovery_rate_limit,
            discovery_burst_limit,
        ));

        // Inbound: allow UDP to host VIP for discovery and game traffic (any port).
        inbound_rules.push(allow_rule(
            "allow_udp_to_host",
            4500,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![],
            vec![host_vip.to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            0,
            0,
        ));

        // Inbound: allow PaperConnect control plane TCP to the host protocol port.
        if let Some(p) = host_protocol_port {
            inbound_rules.push(allow_rule(
                "allow_tcp_to_host_protocol_port",
                4000,
                Protocol::Tcp,
                vec![p.to_string()],
                vec![],
                vec![host_vip.to_string()],
                vec![],
                vec![],
                0,
                0,
            ));
        } else {
            inbound_rules.push(allow_rule(
                "allow_tcp_to_host",
                3500,
                Protocol::Tcp,
                vec!["0-65535".to_string()],
                vec![],
                vec![host_vip.to_string()],
                vec![],
                vec![],
                0,
                0,
            ));
        }

        // Outbound: host may talk to members on any UDP port (RakNet / NetherNet / WebRTC).
        outbound_rules.push(allow_rule(
            "allow_udp_from_host_to_members",
            5000,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            0,
            0,
        ));

        // Outbound: allow host TCP replies/control traffic to members (PaperConnect protocol port).
        outbound_rules.push(allow_rule(
            "allow_tcp_from_host_to_members",
            4800,
            Protocol::Tcp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            vec![],
            0,
            0,
        ));

        // Outbound: allow host broadcast for discovery.
        outbound_rules.push(allow_rule(
            "allow_udp_discovery_broadcast_out",
            4500,
            Protocol::Udp,
            vec!["7551".to_string()],
            vec![host_vip.to_string()],
            vec!["10.144.144.255".to_string()],
            vec![],
            vec![],
            discovery_rate_limit,
            discovery_burst_limit,
        ));
    } else {
        // Inbound: joiners only accept inbound UDP from host VIP (any port).
        inbound_rules.push(allow_rule(
            "allow_udp_from_host",
            5000,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            0,
            0,
        ));

        // Inbound: joiners accept control plane TCP from host VIP.
        inbound_rules.push(allow_rule(
            "allow_tcp_from_host",
            4500,
            Protocol::Tcp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            vec![],
            0,
            0,
        ));

        // Outbound: joiners can only talk to host VIP (any UDP/TCP port).
        outbound_rules.push(allow_rule(
            "allow_udp_to_host",
            5000,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![],
            vec![host_vip.to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            0,
            0,
        ));
        outbound_rules.push(allow_rule(
            "allow_tcp_to_host",
            4500,
            Protocol::Tcp,
            vec!["0-65535".to_string()],
            vec![],
            vec![host_vip.to_string()],
            vec![],
            vec![],
            0,
            0,
        ));

        // Outbound: joiners must be able to broadcast 7551 for host discovery ("ping pong").
        outbound_rules.push(allow_rule(
            "allow_udp_discovery_broadcast_out",
            4000,
            Protocol::Udp,
            vec!["7551".to_string()],
            vec![],
            vec!["10.144.144.255".to_string()],
            vec![],
            vec![],
            discovery_rate_limit,
            discovery_burst_limit,
        ));
    }

    let inbound_chain = Chain {
        name: "paperconnect_inbound".to_string(),
        chain_type: ChainType::Inbound as i32,
        description: "Auto-generated PaperConnect inbound ACL".to_string(),
        enabled: true,
        rules: inbound_rules,
        default_action: Action::Drop as i32,
    };

    let outbound_chain = Chain {
        name: "paperconnect_outbound".to_string(),
        chain_type: ChainType::Outbound as i32,
        description: "Auto-generated PaperConnect outbound ACL".to_string(),
        enabled: true,
        rules: outbound_rules,
        default_action: Action::Drop as i32,
    };

    Acl {
        acl_v1: Some(AclV1 {
            chains: vec![inbound_chain, outbound_chain],
            group: Some(GroupInfo {
                declares: vec![],
                members: vec![],
            }),
        }),
    }
}
