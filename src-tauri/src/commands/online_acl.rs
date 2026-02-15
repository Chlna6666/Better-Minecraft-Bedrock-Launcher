use easytier::proto::acl::{Action, Acl, AclV1, Chain, ChainType, GroupInfo, Protocol, Rule};

fn allow_rule(
    name: &str,
    priority: u32,
    protocol: Protocol,
    ports: Vec<String>,
    source_ips: Vec<String>,
    destination_ips: Vec<String>,
    source_ports: Vec<String>,
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
        action: Action::Allow as i32,
        rate_limit: 0,
        burst_limit: 0,
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
    let mut rules: Vec<Rule> = Vec::new();

    if is_host {
        // Allow LAN discovery broadcast probes (clients send to 10.144.144.255:7551).
        rules.push(allow_rule(
            "allow_udp_discovery_broadcast_in",
            4000,
            Protocol::Udp,
            vec!["7551".to_string()],
            vec![],
            vec!["10.144.144.255".to_string()],
            vec![],
        ));

        // Allow direct UDP to the host for discovery and game traffic (any port).
        rules.push(allow_rule(
            "allow_udp_to_host",
            3500,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![],
            vec![host_vip.to_string()],
            vec![],
        ));

        // Allow PaperConnect control plane TCP to the host protocol port.
        if let Some(p) = host_protocol_port {
            rules.push(allow_rule(
                "allow_tcp_to_host_protocol_port",
                3000,
                Protocol::Tcp,
                vec![p.to_string()],
                vec![],
                vec![host_vip.to_string()],
                vec![],
            ));
        } else {
            // Fallback: allow TCP to host (still blocks joiner-to-joiner).
            rules.push(allow_rule(
                "allow_tcp_to_host",
                2500,
                Protocol::Tcp,
                vec!["0-65535".to_string()],
                vec![],
                vec![host_vip.to_string()],
                vec![],
            ));
        }
    } else {
        // Joiners: allow inbound packets from the host only (any port).
        rules.push(allow_rule(
            "allow_udp_from_host",
            4000,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec![],
            vec![],
        ));

        // PaperConnect control plane / other host communications (keep permissive on source port).
        rules.push(allow_rule(
            "allow_tcp_from_host",
            3000,
            Protocol::Tcp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec![],
            vec![],
        ));
    }

    let chain = Chain {
        name: "paperconnect_inbound".to_string(),
        chain_type: ChainType::Inbound as i32,
        description: "Auto-generated PaperConnect inbound ACL".to_string(),
        enabled: true,
        rules,
        default_action: Action::Drop as i32,
    };

    Acl {
        acl_v1: Some(AclV1 {
            chains: vec![chain],
            group: Some(GroupInfo {
                declares: vec![],
                members: vec![],
            }),
        }),
    }
}

