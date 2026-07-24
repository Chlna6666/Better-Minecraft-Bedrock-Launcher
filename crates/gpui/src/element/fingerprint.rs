use std::hash::{Hash, Hasher};

use seahash::SeaHasher;

/// A framework-owned fingerprint builder for render cache validation.
///
/// Use this for UI cache keys and frame validation signatures instead of choosing a hasher in
/// application code. Applications should only record the small semantic values that affect the
/// rendered output; GPUI owns the hashing implementation.
#[derive(Clone)]
pub struct RenderFingerprint {
    hasher: SeaHasher,
}

impl RenderFingerprint {
    /// Creates an empty render fingerprint.
    pub fn new() -> Self {
        Self {
            hasher: SeaHasher::new(),
        }
    }

    /// Records a semantic value into the fingerprint.
    pub fn record<T: Hash + ?Sized>(&mut self, value: &T) -> &mut Self {
        value.hash(self);
        self
    }

    /// Returns the current fingerprint value.
    pub fn value(&self) -> u64 {
        self.hasher.finish()
    }

    /// Returns the current fingerprint value as a fixed-width hex string.
    pub fn hex(&self) -> String {
        format!("{:016x}", self.value())
    }
}

impl Default for RenderFingerprint {
    fn default() -> Self {
        Self::new()
    }
}

impl Hasher for RenderFingerprint {
    fn finish(&self) -> u64 {
        self.hasher.finish()
    }

    fn write(&mut self, bytes: &[u8]) {
        self.hasher.write(bytes);
    }
}

/// Computes a framework-owned render fingerprint for a semantic cache key.
pub fn render_fingerprint<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut fingerprint = RenderFingerprint::new();
    fingerprint.record(value);
    fingerprint.value()
}

/// Computes a framework-owned render fingerprint as a fixed-width hex string.
pub fn render_fingerprint_hex<T: Hash + ?Sized>(value: &T) -> String {
    let mut fingerprint = RenderFingerprint::new();
    fingerprint.record(value);
    fingerprint.hex()
}
