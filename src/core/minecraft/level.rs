use anyhow::Result;
use std::path::Path;

use bedrock_world::nbt::NbtTag;
#[allow(unused_imports)]
pub use bedrock_world::{LevelDatDocument, LevelDatHeader, LevelDatReadWarning};

pub fn decode(data: &[u8]) -> Result<LevelDatDocument> {
    bedrock_world::level::parse(data).map_err(Into::into)
}

pub fn decode_root(data: &[u8]) -> Result<NbtTag> {
    Ok(decode(data)?.root)
}

pub fn read(path: &Path) -> Result<LevelDatDocument> {
    bedrock_world::level::read(path).map_err(Into::into)
}

pub fn write(path: &Path, document: &LevelDatDocument) -> Result<()> {
    bedrock_world::level::write(path, document).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_nbt::serialize_root;
    use indexmap::IndexMap;

    #[test]
    fn decode_keeps_header_version() {
        let mut root = IndexMap::new();
        root.insert("LevelName".to_string(), NbtTag::String("Test".to_string()));
        let payload = serialize_root(&NbtTag::Compound(root)).expect("serialize");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&10_u32.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);

        let document = decode(&bytes).expect("decode");
        assert_eq!(document.header.version, 10);
        assert_eq!(document.header.declared_len, payload.len() as u32);
        assert_eq!(document.header.actual_payload_len, payload.len());
        assert!(document.warnings.is_empty());
    }

    #[test]
    fn decode_warns_when_declared_length_is_too_large() {
        let mut root = IndexMap::new();
        root.insert("LevelName".to_string(), NbtTag::String("Test".to_string()));
        let payload = serialize_root(&NbtTag::Compound(root)).expect("serialize");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&10_u32.to_le_bytes());
        bytes.extend_from_slice(&((payload.len() + 128) as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);

        let document = decode(&bytes).expect("decode");
        assert_eq!(document.header.actual_payload_len, payload.len());
        assert_eq!(
            document.warnings,
            vec![LevelDatReadWarning::DeclaredLengthTooLarge {
                declared_len: (payload.len() + 128) as u32,
                actual_payload_len: payload.len(),
            }]
        );
    }
}
