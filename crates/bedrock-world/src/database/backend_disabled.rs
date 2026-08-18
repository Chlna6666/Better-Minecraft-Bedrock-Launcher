//! Internal placeholder used when the optional Mojang LevelDB backend is disabled.

use super::storage::{
    StorageBatch, StorageReadOptions, StorageScanOutcome, StorageVisitorControl, WorldStorage,
};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;
use std::path::Path;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BedrockLevelDbStorage;

impl BedrockLevelDbStorage {
    pub(crate) fn open(_path: impl AsRef<Path>) -> Result<Self> {
        Err(feature_disabled())
    }

    pub(crate) fn open_read_only(_path: impl AsRef<Path>) -> Result<Self> {
        Err(feature_disabled())
    }

    #[allow(dead_code)]
    pub(crate) fn open_read_only_best_effort(_path: impl AsRef<Path>) -> Result<Self> {
        Err(feature_disabled())
    }
}

impl WorldStorage for BedrockLevelDbStorage {
    fn get(&self, _key: &[u8]) -> Result<Option<Bytes>> {
        Err(feature_disabled())
    }

    fn get_many(&self, _keys: &[Bytes]) -> Result<Vec<Option<Bytes>>> {
        Err(feature_disabled())
    }

    fn put(&self, _key: &[u8], _value: &[u8]) -> Result<()> {
        Err(feature_disabled())
    }

    fn delete(&self, _key: &[u8]) -> Result<()> {
        Err(feature_disabled())
    }

    fn for_each_key(
        &self,
        _options: StorageReadOptions,
        _visitor: &mut (dyn FnMut(&[u8]) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        Err(feature_disabled())
    }

    fn for_each_prefix(
        &self,
        _prefix: &[u8],
        _options: StorageReadOptions,
        _visitor: &mut (dyn FnMut(&[u8], &Bytes) -> Result<StorageVisitorControl> + Send),
    ) -> Result<StorageScanOutcome> {
        Err(feature_disabled())
    }

    fn write_batch(&self, _batch: &StorageBatch) -> Result<()> {
        Err(feature_disabled())
    }

    fn flush(&self) -> Result<()> {
        Err(feature_disabled())
    }

    fn compact(&self) -> Result<()> {
        Err(feature_disabled())
    }
}

fn feature_disabled() -> BedrockWorldError {
    BedrockWorldError::UnsupportedChunkFormat(
        "Mojang LevelDB backend is disabled; enable the backend-bedrock-leveldb feature".to_string(),
    )
}
