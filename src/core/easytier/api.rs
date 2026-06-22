use easytier::common::stun::{StunInfoCollector, StunInfoCollectorTrait};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NatTypeSnapshot {
    pub udp_nat_type: i32,
    pub tcp_nat_type: i32,
}

pub async fn detect_nat_types() -> NatTypeSnapshot {
    tokio::task::spawn_blocking(|| {
        let collector = StunInfoCollector::new_with_default_servers();
        collector.update_stun_info();

        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        let mut last = collector.get_stun_info();
        while std::time::Instant::now() < deadline {
            last = collector.get_stun_info();
            if last.udp_nat_type != 0 || last.tcp_nat_type != 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }

        NatTypeSnapshot {
            udp_nat_type: last.udp_nat_type,
            tcp_nat_type: last.tcp_nat_type,
        }
    })
    .await
    .unwrap_or(NatTypeSnapshot {
        udp_nat_type: 0,
        tcp_nat_type: 0,
    })
}
