//! Mojang Bedrock LevelDB integration for world storage.

use crate::error::{BedrockWorldError, Result};
use std::path::Path;

pub use super::storage::backend::BedrockLevelDbStorage;

#[cfg(feature = "bedrock-leveldb")]
pub(crate) fn create_bedrock_leveldb(path: impl AsRef<Path>) -> Result<()> {
    let options = bedrock_leveldb::LevelDbOpenOptions {
        read_only: false,
        create_if_missing: true,
        error_if_exists: true,
        paranoid_checks: true,
        compression_policy: bedrock_leveldb::CompressionPolicy::RawDeflate,
        cache: bedrock_leveldb::NativeCacheOptions::default(),
        write_buffer_size: 0,
    };
    let database = bedrock_leveldb::Db::open(path, options)
        .map_err(|error| BedrockWorldError::LevelDb(error.to_string()))?;
    database
        .flush()
        .map_err(|error| BedrockWorldError::LevelDb(error.to_string()))
}

#[cfg(not(feature = "bedrock-leveldb"))]
pub(crate) fn create_bedrock_leveldb(_path: impl AsRef<Path>) -> Result<()> {
    Err(BedrockWorldError::LevelDb(
        "bedrock-leveldb feature is disabled".to_string(),
    ))
}
