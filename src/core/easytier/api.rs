use easytier::common::stun::{StunInfoCollector, StunInfoCollectorTrait};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::Instant;

const NAT_DETECTION_TIMEOUT: Duration = Duration::from_secs(6);
const NAT_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NatTypeSnapshot {
    pub udp_nat_type: i32,
    pub tcp_nat_type: i32,
}

pub async fn detect_nat_types() -> NatTypeSnapshot {
    let collector = StunInfoCollector::new_with_default_servers();
    wait_for_nat_types(
        || {
            let info = collector.get_stun_info();
            NatTypeSnapshot {
                udp_nat_type: info.udp_nat_type,
                tcp_nat_type: info.tcp_nat_type,
            }
        },
        NAT_DETECTION_TIMEOUT,
        NAT_POLL_INTERVAL,
    )
    .await
}

async fn wait_for_nat_types(
    mut read_snapshot: impl FnMut() -> NatTypeSnapshot,
    timeout: Duration,
    poll_interval: Duration,
) -> NatTypeSnapshot {
    let deadline = Instant::now() + timeout;
    loop {
        let snapshot = read_snapshot();
        if snapshot.udp_nat_type != 0 || snapshot.tcp_nat_type != 0 || Instant::now() >= deadline {
            return snapshot;
        }
        tokio::time::sleep(poll_interval).await;
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[tokio::test]
    async fn wait_for_nat_types_reads_snapshots_inside_tokio_runtime() {
        let reads = Cell::new(0);
        let snapshot = wait_for_nat_types(
            || {
                tokio::runtime::Handle::current();
                let next_read = reads.get() + 1;
                reads.set(next_read);
                if next_read == 1 {
                    NatTypeSnapshot {
                        udp_nat_type: 0,
                        tcp_nat_type: 0,
                    }
                } else {
                    NatTypeSnapshot {
                        udp_nat_type: 1,
                        tcp_nat_type: 2,
                    }
                }
            },
            Duration::from_secs(1),
            Duration::ZERO,
        )
        .await;

        assert_eq!(
            snapshot,
            NatTypeSnapshot {
                udp_nat_type: 1,
                tcp_nat_type: 2,
            }
        );
    }

    #[tokio::test]
    async fn wait_for_nat_types_returns_unknown_snapshot_after_timeout() {
        let snapshot = wait_for_nat_types(
            || NatTypeSnapshot {
                udp_nat_type: 0,
                tcp_nat_type: 0,
            },
            Duration::ZERO,
            Duration::from_secs(1),
        )
        .await;

        assert_eq!(
            snapshot,
            NatTypeSnapshot {
                udp_nat_type: 0,
                tcp_nat_type: 0,
            }
        );
    }
}
