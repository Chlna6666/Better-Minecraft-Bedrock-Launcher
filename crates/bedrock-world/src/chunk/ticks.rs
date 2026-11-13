//! Scheduled and random block-tick queues stored with a Bedrock chunk.

use crate::error::{BedrockWorldError, Result};
use crate::nbt::{NbtTag, parse_consecutive_roots};
use bytes::Bytes;

/// The persisted tick queues for one chunk.
///
/// Entries remain NBT compounds so fields introduced by newer Bedrock versions are preserved.
/// A value read from a world also retains its source bytes for stale-write detection.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChunkTicks {
    pending: Option<Vec<NbtTag>>,
    random: Option<Vec<NbtTag>>,
    pending_source: Option<Bytes>,
    random_source: Option<Bytes>,
}

impl ChunkTicks {
    /// Parses optional pending/random record bytes without accessing world storage.
    ///
    /// `None` means an absent record; `Some(Bytes::new())` means a present empty
    /// record. The original bytes are retained for transaction source checks.
    ///
    /// # Errors
    /// Returns an error for malformed NBT or non-compound roots.
    pub fn from_records(
        pending_source: Option<Bytes>,
        random_source: Option<Bytes>,
    ) -> Result<Self> {
        let pending = decode_queue("PendingTicks", pending_source.as_ref())?;
        let random = decode_queue("RandomTicks", random_source.as_ref())?;
        Ok(Self {
            pending,
            random,
            pending_source,
            random_source,
        })
    }

    /// Returns scheduled ticks, if this chunk has a `PendingTicks` record.
    #[must_use]
    pub fn pending(&self) -> Option<&[NbtTag]> {
        self.pending.as_deref()
    }

    /// Returns random tick queue data, if this chunk has a `RandomTicks` record.
    #[must_use]
    pub fn random(&self) -> Option<&[NbtTag]> {
        self.random.as_deref()
    }

    /// Returns whether the pending record exists, even when it contains no roots.
    #[must_use]
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Returns whether the random record exists, even when it contains no roots.
    #[must_use]
    pub fn has_random(&self) -> bool {
        self.random.is_some()
    }

    /// Encodes the pending record, preserving absence and all supplied NBT fields.
    ///
    /// # Errors
    /// Returns an error for non-compound roots or invalid NBT.
    pub fn encode_pending(&self) -> Result<Option<Bytes>> {
        self.pending
            .as_deref()
            .map(|roots| encode_queue("PendingTicks", roots))
            .transpose()
    }

    /// Encodes the random record, preserving absence and all supplied NBT fields.
    ///
    /// # Errors
    /// Returns an error for non-compound roots or invalid NBT.
    pub fn encode_random(&self) -> Result<Option<Bytes>> {
        self.random
            .as_deref()
            .map(|roots| encode_queue("RandomTicks", roots))
            .transpose()
    }

    /// Replaces this chunk's scheduled tick entries.
    pub fn set_pending(&mut self, ticks: Vec<NbtTag>) {
        self.pending = Some(ticks);
    }

    /// Removes this chunk's scheduled tick record.
    pub fn remove_pending(&mut self) {
        self.pending = None;
    }

    /// Replaces this chunk's random tick queue data.
    pub fn set_random(&mut self, ticks: Vec<NbtTag>) {
        self.random = Some(ticks);
    }

    /// Removes this chunk's random tick record.
    pub fn remove_random(&mut self) {
        self.random = None;
    }

    /// Returns the original pending bytes retained for source validation.
    #[must_use]
    pub fn pending_source(&self) -> Option<&Bytes> {
        self.pending_source.as_ref()
    }

    /// Returns the original random bytes retained for source validation.
    #[must_use]
    pub fn random_source(&self) -> Option<&Bytes> {
        self.random_source.as_ref()
    }
}

fn decode_queue(name: &str, value: Option<&Bytes>) -> Result<Option<Vec<NbtTag>>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let roots = parse_consecutive_roots(value)
        .map_err(|error| BedrockWorldError::CorruptWorld(format!("{name}: {error}")))?;
    if roots
        .iter()
        .any(|root| !matches!(root, NbtTag::Compound(_)))
    {
        return Err(BedrockWorldError::CorruptWorld(format!(
            "{name} contains a non-compound NBT root"
        )));
    }
    Ok(Some(roots))
}

pub(crate) fn encode_queue(name: &str, roots: &[NbtTag]) -> Result<Bytes> {
    if roots
        .iter()
        .any(|root| !matches!(root, NbtTag::Compound(_)))
    {
        return Err(BedrockWorldError::Validation(format!(
            "{name} entries must be NBT compounds"
        )));
    }
    Ok(Bytes::from(crate::nbt::serialize_consecutive_roots(roots)?))
}
