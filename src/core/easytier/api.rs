use easytier::common::stun::{StunInfoCollector, StunInfoCollectorTrait};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NatTypeSnapshot {
    pub udp_nat_type: i32,
    pub tcp_nat_type: i32,
}

impl NatTypeSnapshot {
    fn unknown() -> Self {
        Self {
            udp_nat_type: 0,
            tcp_nat_type: 0,
        }
    }
}

pub async fn detect_nat_types() -> NatTypeSnapshot {
    // NAT 检测最长会阻塞 6 秒，放到专用线程执行，
    // 避免长时间占用共享阻塞线程槽位（blocking_slots）。
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let spawn_result = std::thread::Builder::new()
        .name("bmcbl-nat-detect".to_string())
        .spawn(move || {
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

            let _ = sender.send(NatTypeSnapshot {
                udp_nat_type: last.udp_nat_type,
                tcp_nat_type: last.tcp_nat_type,
            });
        });

    if spawn_result.is_err() {
        return NatTypeSnapshot::unknown();
    }

    receiver
        .await
        .unwrap_or_else(|_| NatTypeSnapshot::unknown())
}
