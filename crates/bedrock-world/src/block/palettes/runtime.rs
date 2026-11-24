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

/// One runtime block state (one entry in a version's palette dump).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaletteState {
    palette_index: u32,
    name: String,
    version: i32,
    states: BTreeMap<String, RuntimePaletteValue>,
}

impl RuntimePaletteState {
    /// Index of this state in the source palette dump.
    ///
    /// This is a PALETTE INDEX, not necessarily the network runtime ID. Older
    /// versions use sequential network IDs that match this index, but Bedrock
    /// 1.26.51+ uses hashed block network IDs. Do not send this value on the
    /// wire for those versions; use a protocol-aware runtime ID resolver.
    #[must_use]
    pub const fn palette_index(&self) -> u32 {
        self.palette_index
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

    /// Looks up a state by its index in the palette dump.
    #[must_use]
    pub fn state(&self, palette_index: u32) -> Option<&RuntimePaletteState> {
        self.states.get(palette_index as usize)
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

// Decompression happens once. The lightweight index maps a version label to
// its byte range in the decompressed body; per-version states are decoded only
// when a caller asks for that version and then cached.
struct BlobIndex {
    body: Vec<u8>,
    pool: Vec<String>,
    ranges: BTreeMap<String, (usize, usize)>,
}

fn blob_index() -> &'static BlobIndex {
    static INDEX: OnceLock<BlobIndex> = OnceLock::new();
    INDEX.get_or_init(|| build_index().expect("embedded runtime palette catalog must decode"))
}

fn build_index() -> Result<BlobIndex> {
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
    let mut ranges = BTreeMap::new();
    for _ in 0..version_count {
        let label = reader.string(&pool)?;
        let state_count = reader.u32()? as usize;
        if state_count > 1_000_000 {
            return Err(BedrockWorldError::Validation("runtime palette state count too large".to_string()));
        }
        let start = reader.position;
        // Walk past this version's states without materializing them.
        for _ in 0..state_count {
            let _name = reader.u32()?;
            let _version = reader.i32()?;
            let property_count = reader.u32()? as usize;
            if property_count > 128 {
                return Err(BedrockWorldError::Validation("runtime palette property count too large".to_string()));
            }
            for _ in 0..property_count {
                let _prop = reader.u32()?;
                let kind = reader.byte()?;
                match kind {
                    0 | 1 | 2 | 3 => { reader.take(4)?; }
                    8 => { reader.u32()?; }
                    other => return Err(BedrockWorldError::Validation(format!(
                        "runtime palette has unknown property type {other}"
                    ))),
                }
            }
        }
        ranges.insert(label, (start, reader.position));
    }
    Ok(BlobIndex { body, pool, ranges })
}

fn decode_version(index: &BlobIndex, label: &str, start: usize, end: usize) -> Result<RuntimePalette> {
    let mut reader = Reader::at(&index.body, start);
    let mut states = Vec::new();
    while reader.position < end {
        let name = reader.string(&index.pool)?;
        let version = reader.i32()?;
        let property_count = reader.u32()? as usize;
        let mut properties = BTreeMap::new();
        for _ in 0..property_count {
            let property = reader.string(&index.pool)?;
            let kind = reader.byte()?;
            let value = match kind {
                0 => RuntimePaletteValue::Byte(reader.i32()?),
                1 => RuntimePaletteValue::Short(reader.i32()?),
                2 => RuntimePaletteValue::Int(reader.i32()?),
                3 => RuntimePaletteValue::Long(reader.i64()?),
                8 => RuntimePaletteValue::String(reader.string(&index.pool)?),
                other => {
                    return Err(BedrockWorldError::Validation(format!(
                        "runtime palette has unknown property type {other}"
                    )))
                }
            };
            properties.insert(property, value);
        }
        let palette_index = states.len() as u32;
        states.push(RuntimePaletteState {
            palette_index,
            name,
            version,
            states: properties,
        });
    }
    Ok(RuntimePalette { label: label.to_string(), states })
}

// Per-version cache: the index is built cheaply on first access, then each
// version's states are decoded only when requested and cached separately.
fn version_slot(label: &str) -> Option<(&'static OnceLock<RuntimePalette>, (usize, usize))> {
    static SLOTS: OnceLock<Vec<OnceLock<RuntimePalette>>> = OnceLock::new();
    let index = blob_index();
    let (position, (start, end)) = index.ranges.get_key_value(label)?;
    // Build one slot per bundled version the first time any version is asked
    // for. `get_or_init` is safe because the version set is fixed in the blob.
    let slots = SLOTS.get_or_init(|| (0..index.ranges.len()).map(|_| OnceLock::new()).collect());
    let ordinal = index.ranges.keys().position(|key| key == position)?;
    Some((&slots[ordinal], (*start, *end)))
}

fn decode_cached(
    slot: &'static OnceLock<RuntimePalette>,
    index: &'static BlobIndex,
    label: &str,
    start: usize,
    end: usize,
) -> &'static RuntimePalette {
    slot.get_or_init(|| decode_version(index, label, start, end).expect("embedded runtime palette version must decode"))
}

/// All exact bundled version labels.
#[must_use]
pub fn runtime_palette_versions() -> Vec<&'static str> {
    blob_index().ranges.keys().map(String::as_str).collect()
}

/// Returns the runtime palette for one verbatim version label.
///
/// The version's states are decoded on first access and cached; other versions
/// remain untouched until requested.
#[must_use]
pub fn runtime_palette_for(label: &str) -> Option<&'static RuntimePalette> {
    let (slot, (start, end)) = version_slot(label)?;
    Some(decode_cached(slot, blob_index(), label, start, end))
}

/// Looks up one state by version label and palette index.
#[must_use]
pub fn runtime_palette_state(label: &str, palette_index: u32) -> Option<&'static RuntimePaletteState> {
    runtime_palette_for(label)?.state(palette_index)
}

struct Reader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn at(data: &'a [u8], position: usize) -> Self {
        Self { data, position }
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
        assert_eq!(state.palette_index(), 2);
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
        assert_eq!(state.palette_index(), 0);
        // Out-of-range network IDs return None rather than panicking.
        assert!(runtime_palette_state("1.26.60", u32::MAX).is_none());
    }
}
