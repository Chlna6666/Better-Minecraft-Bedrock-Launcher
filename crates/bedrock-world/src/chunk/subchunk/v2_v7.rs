//! Minecraft Bedrock fixed-array SubChunk V2 through V7.

use crate::chunk::{SubChunk, SubChunkDecodeMode, SubChunkFormat};
use crate::error::{BedrockWorldError, Result};
use bytes::Bytes;

pub(crate) fn read(
    version: u8,
    y: i8,
    bytes: Bytes,
    mode: SubChunkDecodeMode,
) -> Result<SubChunk> {
    if !(2..=7).contains(&version) || bytes.first().copied() != Some(version) {
        return Err(BedrockWorldError::Validation(format!(
            "SubChunk V2-V7 reader received invalid version {version}"
        )));
    }
    crate::chunk::subchunk::parse_subchunk_with_mode(y, bytes, mode)
}

pub(crate) fn write(version: u8, subchunk: &SubChunk) -> Result<Bytes> {
    if !(2..=7).contains(&version) {
        return Err(BedrockWorldError::Validation(format!(
            "SubChunk fixed-array writer requires V2-V7, got V{version}"
        )));
    }
    match &subchunk.format {
        SubChunkFormat::LegacySubChunk(value) if value.version() == version => {
            Ok(value.raw().clone())
        }
        SubChunkFormat::Raw {
            version: Some(source),
            bytes,
        } if *source == version => Ok(bytes.clone()),
        _ => Err(BedrockWorldError::UnsupportedChunkFormat(format!(
            "writing SubChunk V{version} from another block representation requires an authoritative BlockState -> numeric id/meta mapping"
        ))),
    }
}
