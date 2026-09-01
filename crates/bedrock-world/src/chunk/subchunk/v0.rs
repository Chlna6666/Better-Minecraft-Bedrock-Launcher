//! Minecraft Bedrock SubChunk V0.

use crate::chunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;

pub(crate) fn read(y: i8, bytes: Bytes, mode: SubChunkDecodeMode) -> Result<SubChunk> {
    if bytes.first().copied() != Some(0) {
        return Err(BedrockWorldError::Validation(
            "SubChunk V0 reader received another version".to_string(),
        ));
    }
    super::decode(y, bytes, mode)
}

pub(crate) fn write(subchunk: &SubChunk) -> Result<Bytes> {
    match &subchunk.format {
        SubChunkFormat::LegacySubChunk(value) if value.version() == 0 => Ok(value.raw().clone()),
        SubChunkFormat::Raw {
            version: Some(0),
            bytes,
        } => Ok(bytes.clone()),
        _ => Err(BedrockWorldError::UnsupportedChunkFormat(
            "writing SubChunk V0 from another block representation requires an authoritative BlockState -> numeric id/meta mapping"
                .to_string(),
        )),
    }
}
