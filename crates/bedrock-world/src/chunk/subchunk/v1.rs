//! Minecraft Bedrock SubChunk V1.

use crate::chunk::encoding::encode_paletted_subchunk_from_palettes;
use crate::chunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;

pub(crate) fn read(y: i8, bytes: Bytes, mode: SubChunkDecodeMode) -> Result<SubChunk> {
    if bytes.first().copied() != Some(1) {
        return Err(BedrockWorldError::Validation(
            "SubChunk V1 reader received another version".to_string(),
        ));
    }
    super::decode(y, bytes, mode)
}

pub(crate) fn write(subchunk: &SubChunk) -> Result<Bytes> {
    match &subchunk.format {
        SubChunkFormat::Paletted { storages, .. } => {
            let storages = storages.iter().collect::<Vec<_>>();
            encode_paletted_subchunk_from_palettes(1, subchunk.y, &storages)
        }
        SubChunkFormat::Raw {
            version: Some(1),
            bytes,
        } => Ok(bytes.clone()),
        _ => Err(BedrockWorldError::UnsupportedChunkFormat(
            "SubChunk V1 requires one paletted block storage".to_string(),
        )),
    }
}
