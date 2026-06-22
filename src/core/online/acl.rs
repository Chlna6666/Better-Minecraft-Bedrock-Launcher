use easytier::proto::acl::{Acl, AclV1, Action, Chain, ChainType, GroupInfo, Protocol, Rule};

fn allow_rule(
    name: &str,
    priority: u32,
    protocol: Protocol,
    ports: Vec<String>,
    source_ips: Vec<String>,
    destination_ips: Vec<String>,
    source_ports: Vec<String>,
    app_protocols: Vec<i32>,
    stateful: bool,
    rate_limit: u32,
    burst_limit: u32,
    payload_prefix_hex: Vec<String>,
    payload_min_len: Option<u32>,
    payload_max_len: Option<u32>,
    dst_is_broadcast: Option<bool>,
    dst_is_multicast: Option<bool>,
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
        payload_prefix_hex,
        payload_min_len,
        payload_max_len,
        dst_is_broadcast,
        dst_is_multicast,
        action: Action::Allow as i32,
        rate_limit,
        burst_limit,
        stateful,
        source_groups: vec![],
        destination_groups: vec![],
    }
}

pub fn build_paperconnect_acl(
    is_host: bool,
    host_vip: &str,
    host_protocol_port: Option<u16>,
) -> Acl {
    let mut inbound_rules: Vec<Rule> = Vec::new();
    let mut outbound_rules: Vec<Rule> = Vec::new();

    let bedrock_udp_app_protocols: Vec<i32> = vec![10, 20, 21, 22, 23];
    let discovery_rate_limit: u32 = 0;
    let discovery_burst_limit: u32 = 0;
    let discovery_payload_min_len: Option<u32> = None;
    let discovery_payload_max_len: Option<u32> = None;
    let discovery_payload_prefix_hex: Vec<String> = vec![];
    let discovery_broadcast_ports: Vec<String> =
        vec!["7551".to_string(), "19132".to_string(), "19133".to_string()];
    let discovery_broadcast_ips: Vec<String> =
        vec!["10.144.144.255".to_string(), "255.255.255.255".to_string()];
    let permissive_unicast_ports: Vec<String> = vec!["7551".to_string()];

    if is_host {
        inbound_rules.push(allow_rule(
            "allow_udp_to_host_unicast_permissive",
            5200,
            Protocol::Udp,
            permissive_unicast_ports.clone(),
            vec![],
            vec![host_vip.to_string()],
            vec![],
            vec![],
            false,
            0,
            0,
            vec![],
            None,
            None,
            None,
            None,
        ));

        inbound_rules.push(allow_rule(
            "allow_udp_discovery_broadcast_in",
            5000,
            Protocol::Udp,
            discovery_broadcast_ports.clone(),
            vec![],
            discovery_broadcast_ips.clone(),
            vec![],
            vec![],
            false,
            discovery_rate_limit,
            discovery_burst_limit,
            discovery_payload_prefix_hex.clone(),
            discovery_payload_min_len,
            discovery_payload_max_len,
            None,
            None,
        ));

        inbound_rules.push(allow_rule(
            "allow_udp_to_host",
            4500,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![],
            vec![host_vip.to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            false,
            0,
            0,
            vec![],
            None,
            None,
            Some(false),
            None,
        ));

        if let Some(protocol_port) = host_protocol_port {
            inbound_rules.push(allow_rule(
                "allow_tcp_to_host_protocol_port",
                4000,
                Protocol::Tcp,
                vec![protocol_port.to_string()],
                vec![],
                vec![host_vip.to_string()],
                vec![],
                vec![],
                true,
                0,
                0,
                vec![],
                None,
                None,
                None,
                None,
            ));
        }

        outbound_rules.push(allow_rule(
            "allow_udp_from_host_to_members_unicast_permissive",
            5200,
            Protocol::Udp,
            permissive_unicast_ports.clone(),
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            vec![],
            false,
            0,
            0,
            vec![],
            None,
            None,
            None,
            None,
        ));

        outbound_rules.push(allow_rule(
            "allow_udp_from_host_to_members",
            5000,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            false,
            0,
            0,
            vec![],
            None,
            None,
            Some(false),
            None,
        ));

        if let Some(protocol_port) = host_protocol_port {
            outbound_rules.push(allow_rule(
                "allow_tcp_from_host_to_members_protocol_src_port",
                4800,
                Protocol::Tcp,
                vec!["0-65535".to_string()],
                vec![host_vip.to_string()],
                vec!["10.144.144.0/24".to_string()],
                vec![protocol_port.to_string()],
                vec![],
                true,
                0,
                0,
                vec![],
                None,
                None,
                None,
                None,
            ));
        }

        outbound_rules.push(allow_rule(
            "allow_udp_discovery_broadcast_out",
            4500,
            Protocol::Udp,
            discovery_broadcast_ports,
            vec![host_vip.to_string()],
            discovery_broadcast_ips,
            vec![],
            vec![],
            false,
            discovery_rate_limit,
            discovery_burst_limit,
            discovery_payload_prefix_hex,
            discovery_payload_min_len,
            discovery_payload_max_len,
            None,
            None,
        ));
    } else {
        inbound_rules.push(allow_rule(
            "allow_udp_from_host_unicast_permissive",
            5200,
            Protocol::Udp,
            permissive_unicast_ports.clone(),
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            vec![],
            false,
            0,
            0,
            vec![],
            None,
            None,
            None,
            None,
        ));

        inbound_rules.push(allow_rule(
            "allow_udp_from_host",
            5000,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![host_vip.to_string()],
            vec!["10.144.144.0/24".to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            false,
            0,
            0,
            vec![],
            None,
            None,
            Some(false),
            None,
        ));

        if let Some(protocol_port) = host_protocol_port {
            inbound_rules.push(allow_rule(
                "allow_tcp_from_host_protocol_src_port",
                4500,
                Protocol::Tcp,
                vec!["0-65535".to_string()],
                vec![host_vip.to_string()],
                vec!["10.144.144.0/24".to_string()],
                vec![protocol_port.to_string()],
                vec![],
                true,
                0,
                0,
                vec![],
                None,
                None,
                None,
                None,
            ));
        }

        outbound_rules.push(allow_rule(
            "allow_udp_to_host_unicast_permissive",
            5200,
            Protocol::Udp,
            permissive_unicast_ports,
            vec![],
            vec![host_vip.to_string()],
            vec![],
            vec![],
            false,
            0,
            0,
            vec![],
            None,
            None,
            None,
            None,
        ));

        outbound_rules.push(allow_rule(
            "allow_udp_to_host",
            5000,
            Protocol::Udp,
            vec!["0-65535".to_string()],
            vec![],
            vec![host_vip.to_string()],
            vec![],
            bedrock_udp_app_protocols.clone(),
            false,
            0,
            0,
            vec![],
            None,
            None,
            Some(false),
            None,
        ));

        if let Some(protocol_port) = host_protocol_port {
            outbound_rules.push(allow_rule(
                "allow_tcp_to_host_protocol_port",
                4500,
                Protocol::Tcp,
                vec![protocol_port.to_string()],
                vec![],
                vec![host_vip.to_string()],
                vec![],
                vec![],
                true,
                0,
                0,
                vec![],
                None,
                None,
                None,
                None,
            ));
        }

        outbound_rules.push(allow_rule(
            "allow_udp_discovery_broadcast_out",
            4000,
            Protocol::Udp,
            discovery_broadcast_ports,
            vec![],
            discovery_broadcast_ips,
            vec![],
            vec![],
            false,
            discovery_rate_limit,
            discovery_burst_limit,
            discovery_payload_prefix_hex,
            discovery_payload_min_len,
            discovery_payload_max_len,
            None,
            None,
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
