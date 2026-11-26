use std::sync::atomic::{AtomicU64, Ordering};

/// Identity of immutable captured paint or encoded layer bytes. Replay copies
/// the token; rebuilding allocates a new one, even for the same element path.
/// No UI path or path hash participates in production GPU resource equality.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RetainedChunkId {
    token: u64,
    pub(super) depth: usize,
}

impl RetainedChunkId {
    pub(crate) fn fresh(depth: usize) -> Self {
        static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
        let token = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1).filter(|next| *next < (1u64 << 63))
            })
            .expect("retained chunk identity exhausted");
        Self { token, depth }
    }

    // Existing renderer layout fixtures construct equivalent descriptors
    // independently. Keep their synthetic identities out of the real namespace.
    #[cfg(test)]
    pub(crate) fn new(identity: crate::GlobalElementId, generation: u64) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = collections::FxHasher::default();
        identity.hash(&mut hasher);
        generation.hash(&mut hasher);
        Self {
            token: hasher.finish() | (1u64 << 63),
            depth: identity.0.len(),
        }
    }
}
