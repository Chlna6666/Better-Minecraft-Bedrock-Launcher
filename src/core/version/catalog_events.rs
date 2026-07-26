use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::Stream;
use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

static LOCAL_VERSION_GENERATION: AtomicU64 = AtomicU64::new(0);
static LOCAL_VERSION_CHANGES: LazyLock<broadcast::Sender<u64>> = LazyLock::new(|| {
    let (sender, _receiver) = broadcast::channel(16);
    sender
});

pub fn notify_local_versions_changed() {
    let generation = LOCAL_VERSION_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .saturating_add(1);
    let _ = LOCAL_VERSION_CHANGES.send(generation);
    tracing::debug!(generation, "local version catalog invalidated");
}

pub fn local_version_generation() -> u64 {
    LOCAL_VERSION_GENERATION.load(Ordering::Acquire)
}

pub fn local_version_changes() -> impl Stream<Item = u64> {
    let receiver = LOCAL_VERSION_CHANGES.subscribe();
    futures::stream::unfold(receiver, |mut receiver| async move {
        match receiver.recv().await {
            Ok(generation) => Some((generation, receiver)),
            Err(RecvError::Lagged(_)) => Some((local_version_generation(), receiver)),
            Err(RecvError::Closed) => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt as _;

    use super::*;

    #[tokio::test]
    async fn catalog_change_stream_delivers_new_generation() {
        let previous_generation = local_version_generation();
        let mut changes = Box::pin(local_version_changes());

        notify_local_versions_changed();

        let generation = tokio::time::timeout(Duration::from_secs(1), changes.next())
            .await
            .expect("catalog notification should not time out")
            .expect("catalog notification stream should remain open");
        assert_eq!(generation, previous_generation.saturating_add(1));
    }
}
