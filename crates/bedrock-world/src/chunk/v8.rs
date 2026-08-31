//! Minecraft Bedrock SubChunk V8.

use crate::chunk::encoding::encode_paletted_subchunk_from_palettes;
use crate::chunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;

pub(crate) fn read(y: i8, bytes: Bytes, mode: SubChunkDecodeMode) -> Result<SubChunk> {
    if bytes.first().copied() != Some(8) {
        return Err(BedrockWorldError::Validation(
            "SubChunk V8 reader received another version".to_string(),
        ));
    }
    crate::chunk::subchunk::parse_subchunk_with_mode(y, bytes, mode)
}

pub(crate) fn write(subchunk: &SubChunk) -> Result<Bytes> {
    match &subchunk.format {
        SubChunkFormat::Paletted { storages, .. } => {
            let storages = storages.iter().collect::<Vec<_>>();
            encode_paletted_subchunk_from_palettes(8, subchunk.y, &storages)
        }
        SubChunkFormat::Raw {
            version: Some(8),
            bytes,
        } => Ok(bytes.clone()),
        _ => Err(BedrockWorldError::UnsupportedChunkFormat(
            "SubChunk V8 requires paletted block storages".to_string(),
        )),
    }
}
