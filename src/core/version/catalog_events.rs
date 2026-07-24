use std::sync::LazyLock;
use std::sync::atomic::{AtomicU64, Ordering};

use futures::Stream;
use tokio::sync::watch;

static LOCAL_VERSION_GENERATION: AtomicU64 = AtomicU64::new(0);
static LOCAL_VERSION_CHANGES: LazyLock<watch::Sender<u64>> = LazyLock::new(|| {
    let (sender, _receiver) = watch::channel(0);
    sender
});

pub fn notify_local_versions_changed() {
    let generation = LOCAL_VERSION_GENERATION
        .fetch_add(1, Ordering::AcqRel)
        .saturating_add(1);
    LOCAL_VERSION_CHANGES.send_replace(generation);
    tracing::debug!(generation, "local version catalog invalidated");
}

pub fn local_version_changes() -> impl Stream<Item = u64> {
    let receiver = LOCAL_VERSION_CHANGES.subscribe();
    futures::stream::unfold(receiver, |mut receiver| async move {
        if receiver.changed().await.is_err() {
            return None;
        }

        let generation = *receiver.borrow_and_update();
        Some((generation, receiver))
    })
}
