//! Decoder and query API for the baked runtime block-palette catalog.
//!
//! Blob format (BMCBRP01) is documented in
//! `scripts/bedrock-block-catalog/gen_runtime_palette_catalog.py`.

use crate::error::{BedrockWorldError, Result};
use std::collections::BTreeMap;
use std::sync::OnceLock;

const MAGIC: &[u8; 8] = b"BMCBRP01";
const BLOB: &[u8] = include_bytes!(
    "../../../data/block-catalog/runtime-palettes/catalogs.bin.zst"
);

/// One typed property value from a runtime block state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuntimePaletteValue {
    /// NBT Byte property.
    Byte(i32),
    /// NBT Short property.
    Short(i32),
    /// NBT Int property.
    Int(i32),
    /// NBT Long property.
    Long(i64),
    /// NBT String property.
    String(String),
}

impl RuntimePaletteValue {
    /// Source NBT type name.
    #[must_use]
    pub const fn type_name(&self) -> &'static str {
        match self {
            Self::Byte(_) => "Byte",
            Self::Short(_) => "Short",
            Self::Int(_) => "Int",
            Self::Long(_) => "Long",
            Self::String(_) => "String",
        }
    }
}

/// One runtime block state (one network runtime ID in one version).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaletteState {
    runtime_id: u32,
    name: String,
    version: i32,
    states: BTreeMap<String, RuntimePaletteValue>,
}

impl RuntimePaletteState {
    /// Network runtime ID (the state's index in the source dump).
    #[must_use]
    pub const fn runtime_id(&self) -> u32 {
        self.runtime_id
    }

    /// Fully qualified block identifier.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Persisted NBT BlockState storage version.
    #[must_use]
    pub const fn storage_version(&self) -> i32 {
        self.version
    }

    /// Typed NBT properties.
    #[must_use]
    pub fn states(&self) -> &BTreeMap<String, RuntimePaletteValue> {
        &self.states
    }
}

/// One version's runtime block palette, indexed by network runtime ID.
#[derive(Debug, Clone)]
pub struct RuntimePalette {
    label: String,
    states: Vec<RuntimePaletteState>,
}

impl RuntimePalette {
    /// Verbatim source version label (many are beta builds).
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Number of network runtime IDs in this palette.
    #[must_use]
    pub fn len(&self) -> usize {
        self.states.len()
    }

    /// Returns true if the palette has no states.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    /// Looks up a state by network runtime ID.
    #[must_use]
    pub fn state(&self, runtime_id: u32) -> Option<&RuntimePaletteState> {
        self.states.get(runtime_id as usize)
    }

    /// Iterates all states in network runtime ID order.
    pub fn states(&self) -> impl ExactSizeIterator<Item = &RuntimePaletteState> {
        self.states.iter()
    }

    /// Returns the persisted storage versions present in this palette.
    #[must_use]
    pub fn storage_versions(&self) -> Vec<i32> {
        let mut versions: Vec<i32> = self.states.iter().map(|state| state.version).collect();
        versions.sort_unstable();
        versions.dedup();
        versions
    }
}

struct Decoded {
    palettes: BTreeMap<String, RuntimePalette>,
}

fn decode_blob() -> Result<Decoded> {
    if BLOB.len() < 12 || &BLOB[..8] != MAGIC {
        return Err(BedrockWorldError::Validation(
            "runtime palette blob has an invalid header".to_string(),
        ));
    }
    let expected_crc = u32::from_le_bytes(BLOB[8..12].try_into().expect("four bytes"));
    let body = zstd::stream::decode_all(&BLOB[12..]).map_err(|error| {
        BedrockWorldError::Validation(format!("runtime palette decompression failed: {error}"))
    })?;
    if crc32(&body) != expected_crc {
        return Err(BedrockWorldError::Validation("runtime palette CRC mismatch".to_string()));
    }

    let mut reader = Reader::new(&body);
    let pool_len = reader.u32()? as usize;
    if pool_len > 100_000 {
        return Err(BedrockWorldError::Validation("runtime palette pool too large".to_string()));
    }
    let mut pool = Vec::with_capacity(pool_len);
    for _ in 0..pool_len {
        let len = reader.u32()? as usize;
        let raw = reader.take(len)?;
        pool.push(std::str::from_utf8(raw).map_err(|error| {
            BedrockWorldError::Validation(format!("runtime palette string is not UTF-8: {error}"))
        })?.to_string());
    }

    let version_count = reader.u32()? as usize;
    let mut palettes = BTreeMap::new();
    for _ in 0..version_count {
        let label = reader.string(&pool)?;
        let state_count = reader.u32()? as usize;
        if state_count > 1_000_000 {
            return Err(BedrockWorldError::Validation("runtime palette state count too large".to_string()));
        }
        let mut states = Vec::with_capacity(state_count);
        for runtime_id in 0..state_count {
            let name = reader.string(&pool)?;
            let version = reader.i32()?;
            let property_count = reader.u32()? as usize;
            if property_count > 128 {
                return Err(BedrockWorldError::Validation("runtime palette property count too large".to_string()));
            }
            let mut properties = BTreeMap::new();
            for _ in 0..property_count {
                let property = reader.string(&pool)?;
                let kind = reader.byte()?;
                let value = match kind {
                    0 => RuntimePaletteValue::Byte(reader.i32()?),
                    1 => RuntimePaletteValue::Short(reader.i32()?),
                    2 => RuntimePaletteValue::Int(reader.i32()?),
                    3 => RuntimePaletteValue::Long(reader.i64()?),
                    8 => RuntimePaletteValue::String(reader.string(&pool)?),
                    other => {
                        return Err(BedrockWorldError::Validation(format!(
                            "runtime palette has unknown property type {other}"
                        )))
                    }
                };
                properties.insert(property, value);
            }
            states.push(RuntimePaletteState {
                runtime_id: runtime_id as u32,
                name,
                version,
                states: properties,
            });
        }
        palettes.insert(label.clone(), RuntimePalette { label, states });
    }
    Ok(Decoded { palettes })
}

fn decoded() -> &'static Decoded {
    static DECODED: OnceLock<Decoded> = OnceLock::new();
    DECODED.get_or_init(|| decode_blob().expect("embedded runtime palette catalog must decode"))
}

/// All exact bundled version labels.
#[must_use]
pub fn runtime_palette_versions() -> Vec<&'static str> {
    decoded().palettes.keys().map(String::as_str).collect()
}

/// Returns the runtime palette for one verbatim version label.
#[must_use]
pub fn runtime_palette_for(label: &str) -> Option<&'static RuntimePalette> {
    decoded().palettes.get(label)
}

/// Looks up one state by version label and network runtime ID.
#[must_use]
pub fn runtime_palette_state(label: &str, runtime_id: u32) -> Option<&'static RuntimePaletteState> {
    runtime_palette_for(label)?.state(runtime_id)
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.position.checked_add(len).ok_or_else(|| {
            BedrockWorldError::Validation("runtime palette offset overflow".to_string())
        })?;
        let slice = self.data.get(self.position..end).ok_or_else(|| {
            BedrockWorldError::Validation("runtime palette truncated".to_string())
        })?;
        self.position = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        let raw = self.take(4)?;
        Ok(u32::from_le_bytes(raw.try_into().expect("four bytes")))
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn i64(&mut self) -> Result<i64> {
        let raw = self.take(8)?;
        Ok(i64::from_le_bytes(raw.try_into().expect("eight bytes")))
    }

    fn string(&mut self, pool: &[String]) -> Result<String> {
        let id = self.u32()? as usize;
        pool.get(id).cloned().ok_or_else(|| {
            BedrockWorldError::Validation("runtime palette pool id out of range".to_string())
        })
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_decodes_with_50_versions() {
        assert_eq!(runtime_palette_versions().len(), 50);
        assert!(runtime_palette_for("1.20.80.24_beta").is_some());
    }

    #[test]
    fn network_id_and_typed_states_are_exact() {
        let palette = runtime_palette_for("1.20.80.24_beta").expect("palette");
        assert_eq!(palette.len(), 14140);
        assert_eq!(palette.storage_versions(), vec![18108419]);
        let state = palette.state(2).expect("runtime id 2");
        assert_eq!(state.name(), "minecraft:blue_candle");
        assert_eq!(state.runtime_id(), 2);
        assert_eq!(
            state.states().get("lit"),
            Some(&RuntimePaletteValue::Byte(0))
        );
        assert_eq!(
            state.states().get("candles"),
            Some(&RuntimePaletteValue::Int(0))
        );
    }

    #[test]
    fn helper_lookup_matches_direct() {
        let state = runtime_palette_state("1.26.60", 0).expect("state");
        let palette = runtime_palette_for("1.26.60").expect("palette");
        assert_eq!(state.name(), palette.state(0).expect("state 0").name());
        assert_eq!(state.runtime_id(), 0);
        // Out-of-range network IDs return None rather than panicking.
        assert!(runtime_palette_state("1.26.60", u32::MAX).is_none());
    }
}
