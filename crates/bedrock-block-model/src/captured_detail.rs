//! Live-captured Bedrock detail-block geometry, compiled as a compact binary index.
//!
//! The index is generated offline from the 26.40 runtime and embedded with `include_bytes!`.
//! Positions are quantized to 1/256, UVs to 1/65535 and repeated quads/states/strings are stored
//! once, so the whole catalog stays under 1 MB and needs no JSON parsing at startup.

use crate::model_family::{ModelPlane, ModelShape};
use crate::state::{BlockStateQuery, BlockStateValue};
use std::collections::HashMap;
use std::sync::OnceLock;

static CATALOG: OnceLock<Catalog> = OnceLock::new();
static RAW: &[u8] = include_bytes!("../generated/native/captured_detail_models.bin");

#[derive(Debug)]
struct Catalog {
    strings: Vec<String>,
    quads: Vec<RawQuad>,
    geometries: Vec<Vec<u16>>,
    states: Vec<Vec<StatePair>>,
    blocks: HashMap<String, Vec<StateVariant>>,
    block_count: usize,
    variant_count: usize,
}

#[derive(Debug, Clone, Copy)]
struct RawQuad {
    positions: [i16; 12],
    uvs: [u16; 8],
    normal: u8,
    texture: u16,
}

#[derive(Debug)]
struct StatePair {
    key: u16,
    value: StateValue,
}

#[derive(Debug)]
enum StateValue {
    Int(i32),
    Str(u32),
}

#[derive(Debug)]
struct StateVariant {
    state: u16,
    geometry: u16,
}

struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}

impl<'a> Reader<'a> {
    const fn new(data: &'a [u8]) -> Self {
        Self { data, offset: 0 }
    }

    fn take(&mut self, length: usize) -> &'a [u8] {
        let bytes = &self.data[self.offset..self.offset + length];
        self.offset += length;
        bytes
    }

    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }

    fn u16(&mut self) -> u16 {
        u16::from_le_bytes(self.take(2).try_into().expect("u16"))
    }

    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().expect("u32"))
    }

    fn i16(&mut self) -> i16 {
        i16::from_le_bytes(self.take(2).try_into().expect("i16"))
    }

    fn i32(&mut self) -> i32 {
        i32::from_le_bytes(self.take(4).try_into().expect("i32"))
    }
}

fn parse() -> Catalog {
    let mut reader = Reader::new(RAW);
    assert_eq!(reader.take(4), b"CBD2", "captured detail catalog magic");
    assert_eq!(reader.u16(), 2, "captured detail catalog version");

    let string_count = reader.u32() as usize;
    let mut strings = Vec::with_capacity(string_count);
    for _ in 0..string_count {
        let length = usize::from(reader.u16());
        let bytes = reader.take(length);
        strings.push(
            std::str::from_utf8(bytes)
                .expect("catalog string")
                .to_owned(),
        );
    }

    let quad_count = reader.u32() as usize;
    let mut quads = Vec::with_capacity(quad_count);
    for _ in 0..quad_count {
        let mut positions = [0_i16; 12];
        for value in &mut positions {
            *value = reader.i16();
        }
        let mut uvs = [0_u16; 8];
        for value in &mut uvs {
            *value = reader.u16();
        }
        let normal = reader.u8();
        let texture = reader.u16();
        quads.push(RawQuad {
            positions,
            uvs,
            normal,
            texture,
        });
    }

    let geometry_count = reader.u32() as usize;
    let mut geometries = Vec::with_capacity(geometry_count);
    for _ in 0..geometry_count {
        let count = usize::from(reader.u16());
        let mut indices = Vec::with_capacity(count);
        for _ in 0..count {
            indices.push(reader.u16());
        }
        geometries.push(indices);
    }

    let state_count = reader.u32() as usize;
    let mut states = Vec::with_capacity(state_count);
    for _ in 0..state_count {
        let pair_count = usize::from(reader.u8());
        let mut pairs = Vec::with_capacity(pair_count);
        for _ in 0..pair_count {
            let key = reader.u16();
            let kind = reader.u8();
            let value = if kind == 0 {
                StateValue::Int(reader.i32())
            } else {
                StateValue::Str(reader.u32())
            };
            pairs.push(StatePair { key, value });
        }
        states.push(pairs);
    }

    let block_count = reader.u32() as usize;
    let mut blocks = HashMap::with_capacity(block_count);
    let mut variant_count = 0usize;
    for _ in 0..block_count {
        let name_index = reader.u32() as usize;
        let variants = reader.u16() as usize;
        let mut entries = Vec::with_capacity(variants);
        for _ in 0..variants {
            let state = reader.u16();
            let geometry = reader.u16();
            entries.push(StateVariant { state, geometry });
        }
        variant_count += variants;
        blocks.insert(strings[name_index].clone(), entries);
    }

    Catalog {
        strings,
        quads,
        geometries,
        states,
        blocks,
        block_count,
        variant_count,
    }
}

fn catalog() -> &'static Catalog {
    CATALOG.get_or_init(parse)
}

/// Reports whether captured geometry is enabled for this process.
///
/// `BMCBL_BLOCK_MODEL_SOURCE=legacy` disables every native/captured source.
#[must_use]
pub fn use_captured_models() -> bool {
    match std::env::var("BMCBL_BLOCK_MODEL_SOURCE") {
        Ok(value) => !value.eq_ignore_ascii_case("legacy"),
        Err(_) => true,
    }
}

/// Returns live-captured geometry for one block state, when it was captured.
#[must_use]
pub fn captured_shape_for_block_state(state: &BlockStateQuery) -> Option<ModelShape> {
    let catalog = catalog();
    let variants = catalog.blocks.get(&state.name).or_else(|| {
        state
            .name
            .strip_prefix("minecraft:")
            .and_then(|plain| catalog.blocks.get(&format!("minecraft:{plain}")))
    })?;
    let variant = variants
        .iter()
        .find(|variant| state_matches(catalog, variant.state, &state.states))?;
    let geometry = catalog.geometries.get(usize::from(variant.geometry))?;
    let is_door = state.name.ends_with("_door") && !state.name.contains("trapdoor");
    let planes = geometry
        .iter()
        .filter_map(|index| {
            let quad = catalog.quads.get(usize::from(*index))?;
            let mut plane = quad_to_plane(quad);
            if is_door && let Some(texture) = catalog.strings.get(usize::from(quad.texture)) {
                // Door lower/upper halves share one captured geometry but use different textures;
                // carry the captured texture stem so the resolver can pick the right file.
                plane.material_slot = Some(texture.clone());
            }
            Some(plane)
        })
        .collect::<Vec<_>>();
    if planes.is_empty() {
        return None;
    }
    Some(ModelShape::default().with_planes(planes))
}

/// Returns how many captured block ids are embedded.
#[must_use]
pub fn captured_block_count() -> usize {
    catalog().block_count
}

/// Returns how many captured state variants are embedded.
#[must_use]
pub fn captured_variant_count() -> usize {
    catalog().variant_count
}

fn state_matches(
    catalog: &Catalog,
    state: u16,
    query: &std::collections::BTreeMap<String, BlockStateValue>,
) -> bool {
    let Some(pairs) = catalog.states.get(usize::from(state)) else {
        return false;
    };
    if pairs.len() != query.len() {
        return false;
    }
    pairs.iter().all(|pair| {
        let Some(key) = catalog.strings.get(usize::from(pair.key)) else {
            return false;
        };
        let Some(value) = query.get(key) else {
            return false;
        };
        match (&pair.value, value) {
            (StateValue::Int(expected), BlockStateValue::Int(actual)) => {
                i64::from(*expected) == *actual
            }
            (StateValue::Int(expected), BlockStateValue::Bool(actual)) => {
                (*expected != 0) == *actual
            }
            (StateValue::Str(expected), BlockStateValue::String(actual)) => catalog
                .strings
                .get(*expected as usize)
                .is_some_and(|text| text == actual),
            _ => false,
        }
    })
}

fn quad_to_plane(quad: &RawQuad) -> ModelPlane {
    let mut corners = [[0.0_f32; 3]; 4];
    for (index, corner) in corners.iter_mut().enumerate() {
        for axis in 0..3 {
            *corner.get_mut(axis).expect("axis") =
                f32::from(quad.positions[index * 3 + axis]) / 256.0;
        }
    }
    let mut uv = [[0.0_f32; 2]; 4];
    for (index, pair) in uv.iter_mut().enumerate() {
        pair[0] = f32::from(quad.uvs[index * 2]) / 65535.0;
        pair[1] = f32::from(quad.uvs[index * 2 + 1]) / 65535.0;
    }
    ModelPlane::new(corners, normal_from_index(quad.normal))
        .with_uv(uv)
        .front_only()
}

const fn normal_from_index(index: u8) -> [i32; 3] {
    match index {
        0 => [1, 0, 0],
        1 => [-1, 0, 0],
        2 => [0, 1, 0],
        3 => [0, -1, 0],
        4 => [0, 0, 1],
        _ => [0, 0, -1],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_captured_details() {
        assert!(captured_block_count() > 100);
        assert!(captured_variant_count() > 1_000);
    }

    #[test]
    fn oak_stairs_returns_captured_planes() {
        let query = BlockStateQuery::new("minecraft:oak_stairs")
            .with_state("upside_down_bit", 0)
            .with_state("weirdo_direction", 3);
        let shape = captured_shape_for_block_state(&query).expect("captured oak stairs");
        assert!(!shape.planes.is_empty());
        assert!(shape.planes.iter().all(|plane| plane.uv.is_some()));
    }

    #[test]
    fn unknown_state_returns_none() {
        let query = BlockStateQuery::new("minecraft:oak_stairs")
            .with_state("upside_down_bit", 0)
            .with_state("weirdo_direction", 99);
        assert!(captured_shape_for_block_state(&query).is_none());
    }
}
