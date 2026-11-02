//! Per-installation BedrockSceneCapture geometry. No version-specific model/texture paths
//! are compiled into BMCBL. The index is read once on the 3D worker; shared mesh
//! files are loaded only when the corresponding state is first encountered.
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bedrock_world::{NbtReader, NbtTag};
use serde::Deserialize;

use crate::{BlockFace, BlockStateQuery, BlockStateValue, ModelCuboid, ModelPlane, ModelShape};

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
enum StateValueKey {
    Number(i64),
    Text(String),
}
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct StateKey {
    name: String,
    states: Vec<(String, StateValueKey)>,
}

fn normalized_key(key: &str) -> &str {
    key.strip_prefix("minecraft:").unwrap_or(key)
}
fn query_key(query: &BlockStateQuery) -> StateKey {
    StateKey {
        name: normalized_key(&query.name).to_owned(),
        states: query
            .states
            .iter()
            .map(|(name, value)| {
                let key = match value {
                    BlockStateValue::Bool(v) => StateValueKey::Number(i64::from(*v)),
                    BlockStateValue::Int(v) => StateValueKey::Number(*v),
                    BlockStateValue::String(v) => StateValueKey::Text(v.clone()),
                };
                (normalized_key(name).to_owned(), key)
            })
            .collect(),
    }
}
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() > 16384 || text.len() % 2 != 0 {
        return None;
    }
    text.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let h = (pair[0] as char).to_digit(16)?;
            let l = (pair[1] as char).to_digit(16)?;
            Some(((h << 4) | l) as u8)
        })
        .collect()
}
fn capture_key(block: &CaptureBlock) -> Option<StateKey> {
    let bytes = decode_hex(&block.state_nbt_hex)?;
    let NbtTag::Compound(root) = NbtReader::new(&bytes).parse_root().ok()? else {
        return None;
    };
    let states = match root.get("states").or_else(|| root.get("States")) {
        Some(NbtTag::Compound(m)) => m,
        _ => return None,
    };
    let mut pairs = BTreeMap::new();
    for (name, value) in states {
        let value = match value {
            NbtTag::Byte(v) => StateValueKey::Number(i64::from(*v)),
            NbtTag::Short(v) => StateValueKey::Number(i64::from(*v)),
            NbtTag::Int(v) => StateValueKey::Number(i64::from(*v)),
            NbtTag::Long(v) => StateValueKey::Number(*v),
            NbtTag::String(v) => StateValueKey::Text(v.clone()),
            _ => return None,
        };
        pairs.insert(normalized_key(name).to_owned(), value);
    }
    Some(StateKey {
        name: normalized_key(&block.name).to_owned(),
        states: pairs.into_iter().collect(),
    })
}

#[derive(Debug, Deserialize)]
struct CaptureManifest {
    target_game_version: String,
    #[serde(default)]
    registry_only: bool,
    #[serde(default)]
    shared_geometry_pool_schema: u32,
    #[serde(default)]
    legacy_meshes_retained: bool,
}
#[derive(Debug, Deserialize)]
struct CaptureIndex {
    blocks: Vec<CaptureBlock>,
}
#[derive(Debug, Deserialize)]
struct CaptureBlock {
    name: String,
    #[serde(default)]
    network_id: Option<u32>,
    #[serde(default)]
    geometry_alias_network_id: Option<u32>,
    state_nbt_hex: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    geometry_context: String,
    #[serde(default)]
    shared_material_ref: Option<String>,
    #[serde(default)]
    variants: Vec<CaptureVariant>,
}
#[derive(Debug, Deserialize)]
struct CaptureTexture {
    slot: usize,
    #[serde(default)]
    resource_alias: String,
}
#[derive(Debug, Deserialize)]
struct CaptureVariant {
    neighborhood: String,
    #[serde(default)]
    shared_geometry_refs: Vec<String>,
    #[serde(default)]
    shared_appearance_refs: Vec<String>,
}
#[derive(Debug, Deserialize)]
struct CaptureMaterialTable {
    schema: u32,
    texture_bindings: Vec<CaptureTexture>,
}
#[derive(Debug, Deserialize)]
struct CaptureGeometryV2 {
    schema: u32,
    topology: String,
    vertices: Vec<[f32; 3]>,
    triangles: Vec<[usize; 3]>,
}
#[derive(Debug, Deserialize)]
struct CaptureAppearanceV2 {
    schema: u32,
    geometry_ref: String,
    geometry_source: String,
    uv_space: String,
    corner_attributes: Vec<Vec<serde_json::Value>>,
    triangle_texture_slots: Vec<Option<usize>>,
}
#[derive(Clone, Debug)]
struct StateRecipe {
    meshes: Vec<(String, String)>,
    texture_aliases: HashMap<usize, String>,
}
fn valid_ref(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
// The packed ZIP is indexed once and entries are decompressed only on cache misses.
// Serialized access prevents repeated archive-index creation and keeps memory bounded.
struct PackedSource(Mutex<zip::ZipArchive<Cursor<&'static [u8]>>>);
impl std::fmt::Debug for PackedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PackedBscArchive")
    }
}
fn read_catalog_entry(
    root: &Path,
    packed: Option<&PackedSource>,
    name: &str,
    max_size: u64,
) -> Option<Vec<u8>> {
    if let Some(packed) = packed {
        let mut archive = packed.0.lock().ok()?;
        let mut entry = archive.by_name(name).ok()?;
        if entry.size() > max_size {
            return None;
        }
        let mut content = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut content).ok()?;
        (content.len() as u64 <= max_size).then_some(content)
    } else {
        let path = root.join(name);
        fs::metadata(&path)
            .ok()
            .filter(|m| m.len() <= max_size)
            .and_then(|_| fs::read(path).ok())
    }
}
fn read_pool<T: serde::de::DeserializeOwned>(
    root: &Path,
    packed: Option<&PackedSource>,
    kind: &str,
    hash: &str,
) -> Option<T> {
    if !valid_ref(hash) {
        return None;
    }
    let name = format!("geometry_pool/{kind}/{hash}.json");
    read_catalog_entry(root, packed, &name, 8 * 1024 * 1024)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}
/// The BMCBL model cache is scoped to a game installation, not to a process-global
/// Minecraft version. No path or material table is compiled into the executable.
#[derive(Debug)]
pub struct RuntimeCaptureCatalog {
    root: PathBuf,
    packed: Option<PackedSource>,
    version: String,
    states: HashMap<StateKey, StateRecipe>,
    // Negative cache also prevents repeated I/O for corrupt or missing meshes.
    geometries: Mutex<HashMap<String, Option<Arc<CaptureGeometryV2>>>>,
    appearances: Mutex<HashMap<String, Option<Arc<CaptureAppearanceV2>>>>,
    shapes: Mutex<HashMap<StateKey, Option<Arc<ModelShape>>>>,
}
impl PartialEq for RuntimeCaptureCatalog {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root && self.version == other.version
    }
}
impl Eq for RuntimeCaptureCatalog {}

impl RuntimeCaptureCatalog {
    /// Parses the state index once. The heavy geometry blobs remain on disk until used.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, String> {
        Self::open_with_source(root.as_ref(), None)
    }
    /// Use the optional compile-time model library. Disk captures take priority,
    /// so the user may override this with their current installation's export.
    pub fn open_embedded() -> Result<Self, String> {
        const ARCHIVE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/bsc_models.bin"));
        if ARCHIVE.is_empty() {
            return Err("no embedded BSC model library".to_owned());
        }
        Self::open_with_source(Path::new("<embedded>"), Some(ARCHIVE))
    }
    fn open_with_source(root: &Path, blob: Option<&'static [u8]>) -> Result<Self, String> {
        let packed = blob
            .map(|bytes| {
                zip::ZipArchive::new(Cursor::new(bytes))
                    .map(|archive| PackedSource(Mutex::new(archive)))
                    .map_err(|e| format!("packed BSC ZIP: {e}"))
            })
            .transpose()?;
        let manifest: CaptureManifest = serde_json::from_slice(
            &read_catalog_entry(root, packed.as_ref(), "manifest.json", 1024 * 1024)
                .ok_or_else(|| "manifest.json: missing or too large".to_owned())?,
        )
        .map_err(|e| format!("manifest.json: {e}"))?;
        if !manifest.registry_only {
            return Err("capture is not registry-only".to_owned());
        }
        if manifest.shared_geometry_pool_schema != 2 || manifest.legacy_meshes_retained {
            return Err("Geometry Pool v2 exclusive export required".to_owned());
        }
        let index: CaptureIndex = serde_json::from_slice(
            &read_catalog_entry(root, packed.as_ref(), "blocks.json", 64 * 1024 * 1024)
                .ok_or_else(|| "blocks.json: missing or too large".to_owned())?,
        )
        .map_err(|e| format!("blocks.json: {e}"))?;
        let mut states = HashMap::with_capacity(index.blocks.len());
        let mut canonical_by_network_id: HashMap<u32, (String, StateRecipe)> = HashMap::new();
        let mut geometry_aliases = Vec::new();
        let mut material_cache: HashMap<String, HashMap<usize, String>> = HashMap::new();
        for block in index.blocks {
            // Neighbor-driven and BlockEntity geometry must come from explicit
            // connection/NBT rules, never from a single static visual-bounds mesh.
            if !matches!(block.geometry_context.as_str(), "" | "none")
                || matches!(block.status.as_str(), "unsupported" | "dynamic")
            {
                continue;
            }
            let Some(key) = capture_key(&block) else {
                continue;
            };
            // Audio-only states keep their exact NBT key but point to a
            // canonical visual recipe. The referenced network ID is strictly
            // scoped to this capture's own registry, not a cross-version ID.
            if let Some(id) = block.geometry_alias_network_id {
                geometry_aliases.push((key, id));
                continue;
            }
            let network_id = block.network_id;
            let Some(material_ref) = block.shared_material_ref.as_ref() else {
                continue;
            };
            let aliases = if let Some(cached) = material_cache.get(material_ref) {
                cached.clone()
            } else {
                let Some(table) = read_pool::<CaptureMaterialTable>(
                    root,
                    packed.as_ref(),
                    "materials",
                    material_ref,
                ) else {
                    continue;
                };
                if table.schema != 2 {
                    continue;
                }
                let aliases: HashMap<usize, String> = table
                    .texture_bindings
                    .into_iter()
                    .filter(|t| !t.resource_alias.is_empty())
                    .map(|t| (t.slot, t.resource_alias))
                    .collect();
                material_cache.insert(material_ref.clone(), aliases.clone());
                aliases
            };
            let Some(variant) = block
                .variants
                .into_iter()
                .find(|v| v.neighborhood == "registry_visual_bounds")
            else {
                continue;
            };
            if variant.shared_geometry_refs.len() != variant.shared_appearance_refs.len() {
                continue;
            }
            let meshes = variant
                .shared_geometry_refs
                .into_iter()
                .zip(variant.shared_appearance_refs)
                .filter(|(g, a)| valid_ref(g) && valid_ref(a))
                .collect();
            let recipe = StateRecipe {
                meshes,
                texture_aliases: aliases,
            };
            if let Some(id) = network_id {
                canonical_by_network_id.insert(id, (key.name.clone(), recipe.clone()));
            }
            states.insert(key, recipe);
        }
        for (key, id) in geometry_aliases {
            // Refuse cross-type or chained aliases: mismatches must remain
            // unresolved, not render an unrelated face or orientation.
            if let Some((name, recipe)) = canonical_by_network_id.get(&id) {
                if *name == key.name {
                    states.insert(key, recipe.clone());
                }
            }
        }
        Ok(Self {
            root: root.to_owned(),
            packed,
            version: manifest.target_game_version,
            states,
            geometries: Mutex::new(HashMap::new()),
            appearances: Mutex::new(HashMap::new()),
            shapes: Mutex::new(HashMap::new()),
        })
    }
    #[must_use]
    pub fn game_version(&self) -> &str {
        &self.version
    }
    #[must_use]
    pub fn state_count(&self) -> usize {
        self.states.len()
    }

    /// Exact state match. Never silently use a different orientation or
    /// a different set of wall/fence/stair connection states.
    pub fn shape_for_state(&self, state: &BlockStateQuery) -> Option<ModelShape> {
        let key = query_key(state);
        if let Some(cached) = self.shapes.lock().ok()?.get(&key).cloned() {
            return cached.as_ref().map(|s| (**s).clone());
        }
        let recipe = self.states.get(&key)?;
        let mut shape = ModelShape::default();
        for (geometry, appearance) in &recipe.meshes {
            let Some(mesh) = self.shared_geometry(geometry) else {
                continue;
            };
            let Some(appearance) = self.shared_appearance(appearance) else {
                continue;
            };
            let Some(mut planes) =
                shared_mesh_planes(&mesh, &appearance, geometry, &recipe.texture_aliases)
            else {
                continue;
            };
            shape.planes.append(&mut planes);
        }
        // Ordinary full-cube states belong to the 3D editor's greedy voxel
        // mesher, not its unmerged per-block detail mesh path. Preserve their
        // per-face material identifiers in a compact cuboid representation.
        if let Some(cube) = standard_full_cube(&shape) {
            shape = ModelShape::from_cuboids(vec![cube]);
        }
        let result = (!shape.is_empty()).then(|| Arc::new(shape));
        if let Ok(mut cache) = self.shapes.lock() {
            cache.insert(key, result.clone());
        }
        result.as_ref().map(|s| (**s).clone())
    }
    fn shared_geometry(&self, hash: &str) -> Option<Arc<CaptureGeometryV2>> {
        let mut cache = self.geometries.lock().ok()?;
        if let Some(value) = cache.get(hash) {
            return value.clone();
        }
        let result =
            read_pool::<CaptureGeometryV2>(&self.root, self.packed.as_ref(), "geometry", hash)
                .filter(|mesh| mesh.schema == 2 && mesh.topology == "triangles")
                .map(Arc::new);
        cache.insert(hash.to_owned(), result.clone());
        result
    }
    fn shared_appearance(&self, hash: &str) -> Option<Arc<CaptureAppearanceV2>> {
        let mut cache = self.appearances.lock().ok()?;
        if let Some(value) = cache.get(hash) {
            return value.clone();
        }
        let result =
            read_pool::<CaptureAppearanceV2>(&self.root, self.packed.as_ref(), "appearance", hash)
                .filter(|mesh| mesh.schema == 2)
                .map(Arc::new);
        cache.insert(hash.to_owned(), result.clone());
        result
    }
}

fn standard_full_cube(shape: &ModelShape) -> Option<ModelCuboid> {
    if !shape.cuboids.is_empty() || shape.planes.len() != 6 {
        return None;
    }
    let mut cube = ModelCuboid::new([0., 0., 0.], [1., 1., 1.]);
    let mut mask = 0u8;
    for plane in &shape.planes {
        let (face, bit, axis, coordinate) = match plane.normal {
            [0, 1, 0] => (BlockFace::Up, 0, 1, 1.),
            [0, -1, 0] => (BlockFace::Down, 1, 1, 0.),
            [0, 0, -1] => (BlockFace::North, 2, 2, 0.),
            [0, 0, 1] => (BlockFace::South, 3, 2, 1.),
            [-1, 0, 0] => (BlockFace::West, 4, 0, 0.),
            [1, 0, 0] => (BlockFace::East, 5, 0, 1.),
            _ => return None,
        };
        if mask & (1 << bit) != 0 {
            return None;
        }
        mask |= 1 << bit;
        // A plane must cover exactly its 1x1 boundary face. Partial
        // quads, beveled geometry and offset models stay as detail meshes.
        for corner in &plane.corners {
            if (corner[axis] - coordinate).abs() > 0.0001
                || !corner
                    .iter()
                    .all(|v| (*v - 0.).abs() < 0.0001 || (*v - 1.).abs() < 0.0001)
            {
                return None;
            }
        }
        for other_axis in 0..3 {
            if other_axis == axis {
                continue;
            }
            if !plane.corners.iter().any(|p| p[other_axis].abs() < 0.0001)
                || !plane
                    .corners
                    .iter()
                    .any(|p| (p[other_axis] - 1.).abs() < 0.0001)
            {
                return None;
            }
        }
        if let Some(slot) = &plane.material_slot {
            cube = cube.with_face_material_slot(face, slot.clone());
        }
        if let Some(uv) = plane.uv {
            cube = cube.with_face_uv(face, uv);
        }
    }
    (mask == 0b111111).then_some(cube)
}

/// Encodes an engine texture *alias*, never a version-specific file path.
/// The OBJ resolver reverses this encoding and resolves it through the active
/// local terrain_texture.json / resource-pack stack.
pub fn capture_texture_slot(alias: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(4 + alias.len() * 2);
    out.push_str("bsc_");
    for byte in alias.bytes() {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 15)] as char);
    }
    out
}
pub fn decode_capture_texture_slot(slot: &str) -> Option<String> {
    let hex = slot.strip_prefix("bsc_")?;
    if hex.len() % 2 != 0 || hex.len() > 1024 {
        return None;
    }
    let bytes = decode_hex(hex)?;
    String::from_utf8(bytes).ok()
}
// Canonical V2 triangles are sorted independently from source quad order.
// Match two oppositely directed shared edges to recover a *real* quad;
// never invent a face from unrelated triangles or assume adjacent indices.
fn shared_mesh_planes(
    geometry: &CaptureGeometryV2,
    appearance: &CaptureAppearanceV2,
    geometry_hash: &str,
    aliases: &HashMap<usize, String>,
) -> Option<Vec<ModelPlane>> {
    if geometry.topology != "triangles"
        || appearance.geometry_ref != geometry_hash
        || appearance.uv_space != "block_face_local"
        || !matches!(
            appearance.geometry_source.as_str(),
            "registry_visual_bounds" | "registry_cross_planes"
        )
    {
        return None;
    }
    let count = geometry.triangles.len();
    if count == 0
        || count > 32768
        || count % 2 != 0
        || appearance.corner_attributes.len() != count * 3
        || appearance.triangle_texture_slots.len() != count
    {
        return None;
    }
    let points = &geometry.vertices;
    let attr = &appearance.corner_attributes;
    let slots = &appearance.triangle_texture_slots;
    let mut used = vec![false; count];
    let mut planes = Vec::with_capacity(count / 2);
    for i in 0..count {
        if used[i] {
            continue;
        }
        let a = geometry.triangles[i];
        if a.iter().any(|&index| index >= points.len()) {
            return None;
        }
        let mut selected = None;
        'candidate: for j in i + 1..count {
            if used[j] || slots[i] != slots[j] {
                continue;
            }
            let b = geometry.triangles[j];
            if b.iter().any(|&index| index >= points.len()) {
                return None;
            }
            for ra in 0..3 {
                let ai = [(ra) % 3, (ra + 1) % 3, (ra + 2) % 3];
                for rb in 0..3 {
                    let bj = [rb % 3, (rb + 1) % 3, (rb + 2) % 3];
                    if a[ai[0]] != b[bj[0]] || a[ai[2]] != b[bj[1]] || a[ai[1]] == b[bj[2]] {
                        continue;
                    }
                    let left = &attr[3 * i + ai[0]];
                    let right = &attr[3 * j + bj[0]];
                    let left2 = &attr[3 * i + ai[2]];
                    let right2 = &attr[3 * j + bj[1]];
                    if left != right || left2 != right2 {
                        continue;
                    }
                    let rows = [
                        &attr[3 * i + ai[0]],
                        &attr[3 * i + ai[1]],
                        &attr[3 * i + ai[2]],
                        &attr[3 * j + bj[2]],
                    ];
                    if rows.iter().any(|row| row.len() != 6) {
                        continue;
                    }
                    if rows.iter().any(|row| row[0..3] != rows[0][0..3]) {
                        continue;
                    }
                    let corners = [
                        points[a[ai[0]]],
                        points[a[ai[1]]],
                        points[a[ai[2]]],
                        points[b[bj[2]]],
                    ];
                    let uv = rows.map(|row| -> Option<[f32; 2]> {
                        Some([row[3].as_f64()? as f32, row[4].as_f64()? as f32])
                    });
                    let [Some(u0), Some(u1), Some(u2), Some(u3)] = uv else {
                        continue;
                    };
                    let uvs = [u0, u1, u2, u3];
                    if !corners
                        .iter()
                        .flatten()
                        .chain(uvs.iter().flatten())
                        .all(|v| v.is_finite())
                    {
                        continue;
                    }
                    let normal = [
                        rows[0][0].as_f64()?.round() as i32,
                        rows[0][1].as_f64()?.round() as i32,
                        rows[0][2].as_f64()?.round() as i32,
                    ];
                    // The previous reader also supported diagonal cross
                    // planes (flowers, grasses). Do not restrict normals to
                    // coordinate axes, but verify the four positions are
                    // coplanar before reconstructing a native quad.
                    let ab = [
                        corners[1][0] - corners[0][0],
                        corners[1][1] - corners[0][1],
                        corners[1][2] - corners[0][2],
                    ];
                    let ac = [
                        corners[2][0] - corners[0][0],
                        corners[2][1] - corners[0][1],
                        corners[2][2] - corners[0][2],
                    ];
                    let ad = [
                        corners[3][0] - corners[0][0],
                        corners[3][1] - corners[0][1],
                        corners[3][2] - corners[0][2],
                    ];
                    let cross = [
                        ab[1] * ac[2] - ab[2] * ac[1],
                        ab[2] * ac[0] - ab[0] * ac[2],
                        ab[0] * ac[1] - ab[1] * ac[0],
                    ];
                    let norm_sq = cross.iter().map(|v| v * v).sum::<f32>();
                    if norm_sq < 1e-12 {
                        continue;
                    }
                    let deviation = (cross[0] * ad[0] + cross[1] * ad[1] + cross[2] * ad[2]).abs();
                    if deviation > 1e-4 * norm_sq.sqrt() {
                        continue;
                    }
                    let plane = ModelPlane::new(corners, normal).with_uv(uvs).front_only();
                    selected = Some((j, plane));
                    break 'candidate;
                }
            }
        }
        let (j, mut plane) = selected?;
        if let Some(alias) = slots[i].and_then(|id| aliases.get(&id)) {
            plane = plane.with_material_slot(capture_texture_slot(alias));
        }
        used[i] = true;
        used[j] = true;
        planes.push(plane);
    }
    Some(planes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_world::NbtWriter;
    use serde_json::{Value, json};

    fn write(root: &Path, name: &str, doc: &Value) {
        fs::write(root.join(name), serde_json::to_vec(doc).expect("json")).expect(name);
    }
    fn fixture(root: &Path, aliases: bool) -> (String, String, String) {
        let geometry = "a".repeat(64);
        let appearance = "b".repeat(64);
        let material = "c".repeat(64);
        for kind in ["geometry", "appearance", "materials"] {
            fs::create_dir_all(root.join("geometry_pool").join(kind)).expect("pool directory");
        }
        write(
            &root.join("geometry_pool/geometry"),
            &format!("{geometry}.json"),
            &json!({"schema":2,"topology":"triangles",
                "vertices":[[0.,0.,0.],[1.,0.,0.],[1.,1.,0.],[0.,1.,0.]],
                "triangles":[[0,2,3],[0,1,2]]}),
        );
        write(
            &root.join("geometry_pool/appearance"),
            &format!("{appearance}.json"),
            &json!({"schema":2,"geometry_ref":geometry,
                "geometry_source":"registry_visual_bounds","uv_space":"block_face_local",
                "corner_attributes":[
                    [0,0,-1,0,0,4294967295_u32],
                    [0,0,-1,1,1,4294967295_u32],
                    [0,0,-1,0,1,4294967295_u32],
                    [0,0,-1,0,0,4294967295_u32],
                    [0,0,-1,1,0,4294967295_u32],
                    [0,0,-1,1,1,4294967295_u32]],
                "triangle_texture_slots":[2,2]}),
        );
        write(
            &root.join("geometry_pool/materials"),
            &format!("{material}.json"),
            &json!({"schema":2,"texture_bindings":[{"slot":2,"resource_alias":"grass_side"}]}),
        );
        write(
            root,
            "manifest.json",
            &json!({
            "registry_only":true,"target_game_version":"1.26.51.1",
            "shared_geometry_pool_schema":2,"legacy_meshes_retained":false}),
        );
        let state_nbt = |note: i32| {
            let mut obj = BTreeMap::new();
            obj.insert(
                "name".to_owned(),
                NbtTag::String(if aliases {
                    "minecraft:noteblock".to_owned()
                } else {
                    "minecraft:oak_stairs".to_owned()
                }),
            );
            let mut states = BTreeMap::new();
            states.insert(
                if aliases { "note" } else { "weirdo_direction" }.to_owned(),
                NbtTag::Int(note),
            );
            obj.insert(
                "states".to_owned(),
                NbtTag::Compound(states.into_iter().collect()),
            );
            NbtWriter::write_root(&NbtTag::Compound(obj.into_iter().collect()))
                .expect("nbt")
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        };
        let name = if aliases {
            "minecraft:noteblock"
        } else {
            "minecraft:oak_stairs"
        };
        let mut blocks = vec![json!({
            "name":name,"network_id":12,"state_nbt_hex":state_nbt(2),
            "status":"partial","geometry_context":"none","shared_material_ref":material,
            "variants":[{"neighborhood":"registry_visual_bounds",
                "shared_geometry_refs":[geometry],
                "shared_appearance_refs":[appearance]}]})];
        if aliases {
            blocks.push(json!({
                "name":name,"network_id":13,"geometry_alias_network_id":12,
                "state_nbt_hex":state_nbt(24),"status":"partial",
                "geometry_context":"none","variants":[]}));
        }
        write(root, "blocks.json", &json!({"blocks":blocks}));
        (geometry, appearance, material)
    }
    #[test]
    fn texture_alias_roundtrip_without_version_index() {
        for alias in [
            "stone",
            "minecraft:grass_side",
            "textures/blocks/oak_log_top",
            "红色",
        ] {
            assert_eq!(
                decode_capture_texture_slot(&capture_texture_slot(alias)).as_deref(),
                Some(alias)
            );
        }
        assert!(decode_capture_texture_slot("bsc_xyz").is_none());
    }
    #[test]
    fn v2_triangle_pairing_keeps_uv_and_shared_material_slot() {
        let dir = tempfile::tempdir().expect("temporary export");
        let (geo, app, _) = fixture(dir.path(), false);
        let mesh =
            read_pool::<CaptureGeometryV2>(dir.path(), None, "geometry", &geo).expect("geometry");
        let appearance = read_pool::<CaptureAppearanceV2>(dir.path(), None, "appearance", &app)
            .expect("appearance");
        let aliases = HashMap::from([(2, "grass_side".to_owned())]);
        let planes = shared_mesh_planes(&mesh, &appearance, &geo, &aliases).expect("valid quad");
        assert_eq!(planes.len(), 1);
        assert_eq!(planes[0].normal, [0, 0, -1]);
        assert_eq!(
            planes[0].material_slot.as_deref(),
            Some(capture_texture_slot("grass_side").as_str())
        );
        assert_eq!(planes[0].corners.len(), 4);
        assert!(planes[0].uv.is_some());
        let mut invalid = appearance;
        invalid.triangle_texture_slots[1] = Some(9);
        assert!(shared_mesh_planes(&mesh, &invalid, &geo, &aliases).is_none());
    }
    #[test]
    fn embedded_zip_archive_reads_geometry_lazily() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("temporary capture");
        fixture(dir.path(), false);
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for name in ["manifest.json", "blocks.json"] {
            writer.start_file(name, options).expect("ZIP index");
            writer
                .write_all(&fs::read(dir.path().join(name)).expect("index bytes"))
                .expect("write index");
        }
        for kind in ["geometry", "appearance", "materials"] {
            let root = dir.path().join("geometry_pool").join(kind);
            for file in fs::read_dir(root).expect("pool files") {
                let entry = file.expect("pool entry");
                let file_name = entry.file_name().into_string().expect("UTF-8 hash");
                writer
                    .start_file(format!("geometry_pool/{kind}/{file_name}"), options)
                    .expect("ZIP pool");
                writer
                    .write_all(&fs::read(entry.path()).expect("pool bytes"))
                    .expect("pool write");
            }
        }
        let bytes = writer.finish().expect("ZIP finalize").into_inner();
        let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
        let catalog = RuntimeCaptureCatalog::open_with_source(Path::new("<embedded>"), Some(bytes))
            .expect("archive catalog");
        assert_eq!(catalog.state_count(), 1);
        assert!(
            catalog
                .shape_for_state(
                    &BlockStateQuery::new("oak_stairs").with_state("weirdo_direction", 2)
                )
                .is_some()
        );
    }
    #[test]
    fn v2_diagonal_cross_planes_remain_supported() {
        let geometry = CaptureGeometryV2 {
            schema: 2,
            topology: "triangles".to_owned(),
            vertices: vec![[0., 0., 0.], [1., 0., 1.], [1., 1., 1.], [0., 1., 0.]],
            triangles: vec![[0, 2, 3], [0, 1, 2]],
        };
        let make =
            |u: f32, v: f32| serde_json::json!([-0.70710677, 0., 0.70710677, u, v, 4294967295u32]);
        let appearance = CaptureAppearanceV2 {
            schema: 2,
            geometry_ref: "a".repeat(64),
            geometry_source: "registry_cross_planes".to_owned(),
            uv_space: "block_face_local".to_owned(),
            corner_attributes: vec![
                make(0., 0.),
                make(1., 1.),
                make(0., 1.),
                make(0., 0.),
                make(1., 0.),
                make(1., 1.),
            ]
            .into_iter()
            .map(|v| v.as_array().expect("vertex").clone())
            .collect(),
            triangle_texture_slots: vec![Some(1), Some(1)],
        };
        let planes = shared_mesh_planes(
            &geometry,
            &appearance,
            &"a".repeat(64),
            &HashMap::from([(1, "grass".to_owned())]),
        )
        .expect("diagonal cross");
        assert_eq!(planes.len(), 1);
        assert_eq!(planes[0].normal, [-1, 0, 1]);
    }
    #[test]
    fn v2_catalog_matches_exact_state_and_rejects_v1() {
        let dir = tempfile::tempdir().expect("temp model library");
        fixture(dir.path(), false);
        let catalog = RuntimeCaptureCatalog::open(dir.path()).expect("v2 catalog");
        assert_eq!(catalog.state_count(), 1);
        let state = BlockStateQuery::new("oak_stairs").with_state("weirdo_direction", 2);
        let shape = catalog.shape_for_state(&state).expect("shape");
        assert_eq!(shape.planes.len(), 1);
        assert_eq!(
            shape.planes[0].material_slot.as_deref(),
            Some(capture_texture_slot("grass_side").as_str())
        );
        assert!(
            catalog
                .shape_for_state(
                    &BlockStateQuery::new("oak_stairs").with_state("weirdo_direction", 1)
                )
                .is_none()
        );
        write(
            dir.path(),
            "manifest.json",
            &json!({
            "registry_only":true,"target_game_version":"1.26.51.1",
            "shared_geometry_pool_schema":1,"legacy_meshes_retained":true}),
        );
        assert!(RuntimeCaptureCatalog::open(dir.path()).is_err());
    }
    #[test]
    fn note_pitch_changes_reuse_one_visual_recipe() {
        let dir = tempfile::tempdir().expect("temp note model");
        fixture(dir.path(), true);
        let catalog = RuntimeCaptureCatalog::open(dir.path()).expect("catalog");
        assert_eq!(catalog.state_count(), 2);
        let first = catalog
            .shape_for_state(&BlockStateQuery::new("noteblock").with_state("note", 2))
            .expect("base shape");
        let second = catalog
            .shape_for_state(&BlockStateQuery::new("noteblock").with_state("note", 24))
            .expect("shared shape");
        assert_eq!(first, second);
        assert!(
            catalog
                .shape_for_state(&BlockStateQuery::new("noteblock").with_state("note", 12))
                .is_none()
        );
    }
    #[test]
    fn complete_six_face_cube_preserves_slots_and_greedy_merge() {
        let make = |normal: [i32; 3], corners: [[f32; 3]; 4], face: &str| {
            ModelPlane::new(corners, normal)
                .front_only()
                .with_material_slot(capture_texture_slot(face))
                .with_uv([[0., 0.], [0., 1.], [1., 1.], [1., 0.]])
        };
        let shape = ModelShape::default().with_planes(vec![
            make(
                [0, 1, 0],
                [[0., 1., 0.], [0., 1., 1.], [1., 1., 1.], [1., 1., 0.]],
                "top",
            ),
            make(
                [0, -1, 0],
                [[0., 0., 0.], [1., 0., 0.], [1., 0., 1.], [0., 0., 1.]],
                "bottom",
            ),
            make(
                [0, 0, -1],
                [[0., 0., 0.], [0., 1., 0.], [1., 1., 0.], [1., 0., 0.]],
                "north",
            ),
            make(
                [0, 0, 1],
                [[0., 0., 1.], [1., 0., 1.], [1., 1., 1.], [0., 1., 1.]],
                "south",
            ),
            make(
                [-1, 0, 0],
                [[0., 0., 0.], [0., 0., 1.], [0., 1., 1.], [0., 1., 0.]],
                "west",
            ),
            make(
                [1, 0, 0],
                [[1., 0., 0.], [1., 1., 0.], [1., 1., 1.], [1., 0., 1.]],
                "east",
            ),
        ]);
        let cube = standard_full_cube(&shape).expect("solid full cube");
        assert_eq!(
            cube.face_material_slots
                .get(&BlockFace::Up)
                .map(String::as_str),
            Some(capture_texture_slot("top").as_str())
        );
        assert_eq!(cube.face_material_slots.len(), 6);
        assert_eq!(cube.face_uvs.len(), 6);
        let mut broken = shape;
        broken.planes[0].corners[0][1] = 0.5;
        assert!(standard_full_cube(&broken).is_none());
    }
    #[test]
    fn key_normalizes_namespace_without_erasing_state() {
        let a = BlockStateQuery::new("minecraft:cobblestone_wall")
            .with_state("minecraft:wall_connection_type_north", "tall");
        let b = BlockStateQuery::new("cobblestone_wall")
            .with_state("wall_connection_type_north", "tall");
        let c = BlockStateQuery::new("cobblestone_wall")
            .with_state("wall_connection_type_north", "low");
        assert_eq!(query_key(&a), query_key(&b));
        assert_ne!(query_key(&a), query_key(&c));
    }
}
