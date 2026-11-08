//! Dependency-free FNV-1a content hashing for in-process cache fingerprints.
//!
//! Fingerprints are process-local cache keys, never persisted: a collision can
//! only cost a stale cache miss (the caller falls back to recomputation), never
//! wrong output.

use std::hash::Hasher;

/// FNV-1a 64-bit hasher with explicit integer writes.
pub(crate) struct FingerprintHasher(u64);

impl FingerprintHasher {
    pub(crate) const fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    pub(crate) const fn finish(self) -> u64 {
        self.0
    }

    pub(crate) fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(crate) fn write_u64(&mut self, value: u64) {
        self.write_bytes(&value.to_le_bytes());
    }

    pub(crate) fn write_usize(&mut self, value: usize) {
        self.write_u64(value as u64);
    }

    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

impl Hasher for FingerprintHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        self.write_bytes(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::Hash;

    #[test]
    fn integer_writes_match_byte_writes() {
        let mut hashed = FingerprintHasher::new();
        hashed.write(&7u32.to_le_bytes());
        let mut explicit = FingerprintHasher::new();
        explicit.write_u32(7);
        assert_eq!(hashed.finish(), explicit.finish());

        let mut left = FingerprintHasher::new();
        left.write_u64(u64::MAX);
        let mut right = FingerprintHasher::new();
        u64::MAX.hash(&mut right);
        assert_eq!(left.finish(), right.finish());
    }

    #[test]
    fn distinct_contents_hash_differently() {
        let mut one = FingerprintHasher::new();
        one.write_u32(1);
        one.write_u32(2);
        let mut two = FingerprintHasher::new();
        two.write_u32(2);
        two.write_u32(1);
        assert_ne!(one.finish(), two.finish());
    }
}
