use crate::error::{LevelDbError, Result};
use bytes::Bytes;

pub(crate) const FILTER_META_KEY: &[u8] = b"filter.leveldb.BuiltinBloomFilter2";
const FILTER_BASE_LG: u8 = 11;
const BITS_PER_KEY: usize = 10;
const PROBES: u8 = 6;
const BLOOM_HASH_SEED: u32 = 0xbc9f_1d34;
const BLOOM_HASH_MUL: u32 = 0xc6a4_a793;

#[derive(Debug)]
pub(crate) struct BloomFilterBlockBuilder {
    result: Vec<u8>,
    offsets: Vec<u32>,
    hashes: Vec<u32>,
    finished: bool,
}

impl BloomFilterBlockBuilder {
    pub(crate) fn new() -> Self {
        Self {
            result: Vec::with_capacity(256),
            offsets: Vec::with_capacity(8),
            hashes: Vec::with_capacity(128),
            finished: false,
        }
    }

    pub(crate) fn start_block(&mut self, block_offset: u64) -> Result<()> {
        if self.finished {
            return Err(LevelDbError::invalid_argument(
                "cannot start a finished bloom filter block".to_string(),
            ));
        }
        let filter_index = usize::try_from(block_offset >> FILTER_BASE_LG)
            .map_err(|_| LevelDbError::invalid_argument("filter index overflow".to_string()))?;
        while filter_index > self.offsets.len() {
            self.generate_filter()?;
        }
        Ok(())
    }

    pub(crate) fn add_key(&mut self, key: &[u8]) -> Result<()> {
        if self.finished {
            return Err(LevelDbError::invalid_argument(
                "cannot append to a finished bloom filter block".to_string(),
            ));
        }
        self.hashes.push(bloom_hash(key));
        Ok(())
    }

    pub(crate) fn finish(&mut self) -> Result<&[u8]> {
        if !self.finished {
            if !self.hashes.is_empty() {
                self.generate_filter()?;
            }
            let array_offset = u32::try_from(self.result.len()).map_err(|_| {
                LevelDbError::invalid_argument("filter block offset exceeds u32".to_string())
            })?;
            for offset in &self.offsets {
                self.result.extend_from_slice(&offset.to_le_bytes());
            }
            self.result.extend_from_slice(&array_offset.to_le_bytes());
            self.result.push(FILTER_BASE_LG);
            self.finished = true;
        }
        Ok(&self.result)
    }

    #[must_use]
    pub(crate) fn estimated_size(&self) -> usize {
        self.result
            .len()
            .saturating_add(self.hashes.len().saturating_mul(BITS_PER_KEY).div_ceil(8))
            .saturating_add(self.offsets.len().saturating_mul(4))
            .saturating_add(5)
    }

    fn generate_filter(&mut self) -> Result<()> {
        let offset = u32::try_from(self.result.len()).map_err(|_| {
            LevelDbError::invalid_argument("filter block offset exceeds u32".to_string())
        })?;
        self.offsets.push(offset);
        if self.hashes.is_empty() {
            return Ok(());
        }

        let bits = self
            .hashes
            .len()
            .saturating_mul(BITS_PER_KEY)
            .max(64);
        let bytes = bits.div_ceil(8);
        let bits = bytes.saturating_mul(8);
        let start = self.result.len();
        self.result.resize(start.saturating_add(bytes), 0);
        self.result.push(PROBES);

        for &hash in &self.hashes {
            let mut h = hash;
            let delta = h.rotate_right(17);
            for _ in 0..PROBES {
                let bit_position = usize::try_from(h).unwrap_or(usize::MAX) % bits;
                self.result[start + bit_position / 8] |= 1_u8 << (bit_position % 8);
                h = h.wrapping_add(delta);
            }
        }
        self.hashes.clear();
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BloomFilterBlock {
    data: Bytes,
    array_offset: usize,
    filter_count: usize,
    base_lg: u8,
}

impl BloomFilterBlock {
    #[must_use]
    pub(crate) fn parse(data: Bytes) -> Option<Self> {
        if data.len() < 5 {
            return None;
        }
        let base_lg = data[data.len() - 1];
        if base_lg >= 64 {
            return None;
        }
        let array_offset = usize::try_from(u32::from_le_bytes(
            data[data.len() - 5..data.len() - 1].try_into().ok()?,
        ))
        .ok()?;
        if array_offset > data.len() - 5 {
            return None;
        }
        let offset_bytes = data.len() - 5 - array_offset;
        if offset_bytes % 4 != 0 {
            return None;
        }
        Some(Self {
            data,
            array_offset,
            filter_count: offset_bytes / 4,
            base_lg,
        })
    }

    #[must_use]
    pub(crate) fn key_may_match(&self, block_offset: u64, key: &[u8]) -> bool {
        let index_u64 = block_offset >> self.base_lg;
        let Ok(index) = usize::try_from(index_u64) else {
            return true;
        };
        if index >= self.filter_count {
            return true;
        }
        let offset_position = self.array_offset.saturating_add(index.saturating_mul(4));
        let next_position = offset_position.saturating_add(4);
        let Some(start_bytes) = self.data.get(offset_position..next_position) else {
            return true;
        };
        let Some(limit_bytes) = self.data.get(next_position..next_position.saturating_add(4)) else {
            return true;
        };
        let Ok(start) = usize::try_from(u32::from_le_bytes(match start_bytes.try_into() {
            Ok(bytes) => bytes,
            Err(_) => return true,
        })) else {
            return true;
        };
        let Ok(limit) = usize::try_from(u32::from_le_bytes(match limit_bytes.try_into() {
            Ok(bytes) => bytes,
            Err(_) => return true,
        })) else {
            return true;
        };
        if start == limit {
            return false;
        }
        if start > limit || limit > self.array_offset {
            return true;
        }
        bloom_key_may_match(key, &self.data[start..limit])
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }
}

fn bloom_key_may_match(key: &[u8], filter: &[u8]) -> bool {
    if filter.len() < 2 {
        return false;
    }
    let probes = filter[filter.len() - 1];
    if probes > 30 {
        return true;
    }
    let bit_bytes = filter.len() - 1;
    let bits = bit_bytes.saturating_mul(8);
    if bits == 0 {
        return false;
    }
    let mut h = bloom_hash(key);
    let delta = h.rotate_right(17);
    for _ in 0..probes {
        let bit_position = usize::try_from(h).unwrap_or(usize::MAX) % bits;
        if filter[bit_position / 8] & (1_u8 << (bit_position % 8)) == 0 {
            return false;
        }
        h = h.wrapping_add(delta);
    }
    true
}

fn bloom_hash(data: &[u8]) -> u32 {
    leveldb_hash(data, BLOOM_HASH_SEED)
}

fn leveldb_hash(mut data: &[u8], seed: u32) -> u32 {
    let mut hash = seed ^ (u32::try_from(data.len()).unwrap_or(u32::MAX).wrapping_mul(BLOOM_HASH_MUL));
    while data.len() >= 4 {
        let word = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        data = &data[4..];
        hash = hash.wrapping_add(word);
        hash = hash.wrapping_mul(BLOOM_HASH_MUL);
        hash ^= hash >> 16;
    }
    match data.len() {
        3 => {
            hash = hash.wrapping_add(u32::from(data[2]) << 16);
            hash = hash.wrapping_add(u32::from(data[1]) << 8);
            hash = hash.wrapping_add(u32::from(data[0]));
            hash = hash.wrapping_mul(BLOOM_HASH_MUL);
            hash ^= hash >> 24;
        }
        2 => {
            hash = hash.wrapping_add(u32::from(data[1]) << 8);
            hash = hash.wrapping_add(u32::from(data[0]));
            hash = hash.wrapping_mul(BLOOM_HASH_MUL);
            hash ^= hash >> 24;
        }
        1 => {
            hash = hash.wrapping_add(u32::from(data[0]));
            hash = hash.wrapping_mul(BLOOM_HASH_MUL);
            hash ^= hash >> 24;
        }
        _ => {}
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leveldb_hash_matches_upstream_vectors() {
        assert_eq!(leveldb_hash(&[], BLOOM_HASH_SEED), 0xbc9f_1d34);
        assert_eq!(leveldb_hash(&[0x62], BLOOM_HASH_SEED), 0xef13_45c4);
        assert_eq!(leveldb_hash(&[0xc3, 0x97], BLOOM_HASH_SEED), 0x5b66_3814);
        assert_eq!(leveldb_hash(&[0xe2, 0x99, 0xa5], BLOOM_HASH_SEED), 0x323c_078f);
        assert_eq!(leveldb_hash(&[0xe1, 0x80, 0xb9, 0x32], BLOOM_HASH_SEED), 0xed21_633a);
    }

    #[test]
    fn filter_block_matches_present_keys_and_rejects_empty_bucket() {
        let mut builder = BloomFilterBlockBuilder::new();
        builder.start_block(0).expect("start first block");
        builder.add_key(b"alpha").expect("alpha");
        builder.add_key(b"beta").expect("beta");
        builder.start_block(4096).expect("skip one 2KiB bucket");
        builder.add_key(b"omega").expect("omega");
        let bytes = Bytes::copy_from_slice(builder.finish().expect("finish"));
        let filter = BloomFilterBlock::parse(bytes).expect("parse");

        assert!(filter.key_may_match(0, b"alpha"));
        assert!(filter.key_may_match(0, b"beta"));
        assert!(!filter.key_may_match(2048, b"alpha"));
        assert!(filter.key_may_match(4096, b"omega"));
    }
}
