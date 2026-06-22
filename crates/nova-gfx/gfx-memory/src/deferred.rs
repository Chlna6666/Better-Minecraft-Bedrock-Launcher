/// Payload retired until a GPU fence reaches `fence_value`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeferredFree<T> {
    /// Fence value that must complete before `payload` can be freed.
    pub fence_value: u64,
    /// Retired payload.
    pub payload: T,
}

/// FIFO queue for GPU resources that cannot be freed until a fence completes.
#[derive(Clone, Debug, Default)]
pub struct DeferredFreeQueue<T> {
    pending: Vec<DeferredFree<T>>,
}

impl<T> DeferredFreeQueue<T> {
    /// Creates an empty deferred-free queue.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            pending: Vec::new(),
        }
    }

    /// Retires `payload` until `fence_value` has completed.
    pub fn retire(&mut self, fence_value: u64, payload: T) {
        self.pending.push(DeferredFree {
            fence_value,
            payload,
        });
    }

    /// Removes and returns every payload whose fence has completed.
    #[must_use]
    pub fn collect_completed(&mut self, completed_fence: u64) -> Vec<T> {
        self.collect_ready(|fence_value, _payload| fence_value <= completed_fence)
    }

    /// Removes and returns every payload accepted by `ready`.
    #[must_use]
    pub fn collect_ready(&mut self, mut ready: impl FnMut(u64, &T) -> bool) -> Vec<T> {
        let mut completed = Vec::new();
        let mut pending = Vec::with_capacity(self.pending.len());
        for item in self.pending.drain(..) {
            if ready(item.fence_value, &item.payload) {
                completed.push(item.payload);
            } else {
                pending.push(item);
            }
        }
        self.pending = pending;
        completed
    }

    /// Drains every pending payload regardless of fence value.
    #[must_use]
    pub fn drain_all(&mut self) -> Vec<T> {
        self.pending.drain(..).map(|item| item.payload).collect()
    }

    /// Returns the number of pending payloads.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// Returns whether the queue is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Returns the sum of pending payload sizes.
    #[must_use]
    pub fn pending_bytes(&self, size: impl Fn(&T) -> u64) -> u64 {
        self.pending
            .iter()
            .fold(0, |total, item| total.saturating_add(size(&item.payload)))
    }

    /// Iterates over pending fence values.
    pub fn pending_fence_values(&self) -> impl Iterator<Item = u64> + '_ {
        self.pending.iter().map(|item| item.fence_value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_collects_only_completed_fences() {
        let mut queue = DeferredFreeQueue::new();
        queue.retire(2, "a");
        queue.retire(4, "b");
        queue.retire(3, "c");

        assert_eq!(queue.collect_completed(3), vec!["a", "c"]);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.collect_completed(4), vec!["b"]);
        assert!(queue.is_empty());
    }

    #[test]
    fn queue_drain_all_releases_shutdown_payloads() {
        let mut queue = DeferredFreeQueue::new();
        queue.retire(10, 4_u64);
        queue.retire(11, 8_u64);

        assert_eq!(queue.pending_bytes(|size| *size), 12);
        assert_eq!(queue.drain_all(), vec![4, 8]);
        assert!(queue.is_empty());
    }
}
