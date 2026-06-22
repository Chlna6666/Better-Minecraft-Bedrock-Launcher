use anyhow::Result;
use std::path::Path;

#[allow(unused_imports)]
pub use bedrock_world::level_dat::{LevelDatDocument, LevelDatHeader, LevelDatReadWarning};
#[allow(unused_imports)]
pub use bedrock_world::nbt::{NbtReader, NbtRef, NbtTag, NbtValue, NbtWriter};

pub fn parse_root_nbt(data: &[u8]) -> Result<NbtTag> {
    bedrock_world::nbt::parse_root_nbt(data).map_err(Into::into)
}

pub fn serialize_root_nbt(tag: &NbtTag) -> Result<Vec<u8>> {
    bedrock_world::nbt::serialize_root_nbt(tag).map_err(Into::into)
}

pub fn validate_root_nbt_for_write(tag: &NbtTag) -> Result<()> {
    bedrock_world::nbt::validate_root_nbt_for_write(tag).map_err(Into::into)
}

pub fn parse_root_nbt_with_header(data: &[u8]) -> Result<NbtTag> {
    Ok(parse_level_dat_document(data)?.root)
}

pub fn parse_root_nbt_header(data: &[u8]) -> Result<(u32, NbtTag)> {
    let document = parse_level_dat_document(data)?;
    Ok((document.header.version, document.root))
}

pub fn parse_level_dat_document(data: &[u8]) -> Result<LevelDatDocument> {
    bedrock_world::level_dat::parse_level_dat_document(data).map_err(Into::into)
}

pub fn read_level_dat(path: &Path) -> Result<NbtTag> {
    Ok(read_level_dat_document(path)?.root)
}

pub fn read_level_dat_with_version(path: &Path) -> Result<(u32, NbtTag)> {
    let document = read_level_dat_document(path)?;
    Ok((document.header.version, document.root))
}

pub fn read_level_dat_document(path: &Path) -> Result<LevelDatDocument> {
    bedrock_world::level_dat::read_level_dat_document(path).map_err(Into::into)
}

pub fn write_level_dat(path: &Path, tag: &NbtTag, version: u32) -> Result<()> {
    write_level_dat_document(path, &LevelDatDocument::new(version, tag.clone()))
}

pub fn write_level_dat_document(path: &Path, document: &LevelDatDocument) -> Result<()> {
    bedrock_world::level_dat::write_level_dat_document(path, document).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    #[test]
    fn parse_level_dat_document_keeps_header_version() {
        let mut root = IndexMap::new();
        root.insert("LevelName".to_string(), NbtTag::String("Test".to_string()));
        let payload = serialize_root_nbt(&NbtTag::Compound(root)).expect("serialize");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&10_u32.to_le_bytes());
        bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);

        let document = parse_level_dat_document(&bytes).expect("parse");
        assert_eq!(document.header.version, 10);
        assert_eq!(document.header.declared_len, payload.len() as u32);
        assert_eq!(document.header.actual_payload_len, payload.len());
        assert!(document.warnings.is_empty());
    }

    #[test]
    fn parse_level_dat_document_warns_when_declared_length_is_too_large() {
        let mut root = IndexMap::new();
        root.insert("LevelName".to_string(), NbtTag::String("Test".to_string()));
        let payload = serialize_root_nbt(&NbtTag::Compound(root)).expect("serialize");

        let mut bytes = Vec::new();
        bytes.extend_from_slice(&10_u32.to_le_bytes());
        bytes.extend_from_slice(&((payload.len() + 128) as u32).to_le_bytes());
        bytes.extend_from_slice(&payload);

        let document = parse_level_dat_document(&bytes).expect("parse");
        assert_eq!(document.header.actual_payload_len, payload.len());
        assert_eq!(
            document.warnings,
            vec![LevelDatReadWarning::DeclaredLengthTooLarge {
                declared_len: (payload.len() + 128) as u32,
                actual_payload_len: payload.len(),
            }]
        );
    }

    #[test]
    fn validate_root_nbt_for_write_rejects_mixed_lists() {
        let mut root = IndexMap::new();
        root.insert(
            "BrokenList".to_string(),
            NbtTag::List(vec![NbtTag::Int(1), NbtTag::String("bad".to_string())]),
        );

        assert!(validate_root_nbt_for_write(&NbtTag::Compound(root)).is_err());
    }
}
