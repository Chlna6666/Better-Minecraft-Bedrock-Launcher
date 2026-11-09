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
#[path = "runtime_capture_pool_v3.rs"]
mod pool_v3;
use pool_v3::{Appearance as CaptureAppearanceV3, Geometry as CaptureGeometryV3, PoolV3};

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
    shared_material_ref: Option<u32>,
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
    shared_geometry_refs: Vec<u32>,
    #[serde(default)]
    shared_appearance_refs: Vec<u32>,
}
#[derive(Debug, Deserialize)]
struct CaptureMaterialTable {
    schema: u32,
    texture_bindings: Vec<CaptureTexture>,
}
#[derive(Clone, Debug)]
struct StateRecipe {
    meshes: Vec<(u32, u32)>,
    texture_aliases: HashMap<usize, String>,
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
/// The BMCBL model cache is scoped to a game installation, not to a process-global
/// Minecraft version. No path or material table is compiled into the executable.
#[derive(Debug)]
pub struct RuntimeCaptureCatalog {
    root: PathBuf,
    pool: PoolV3,
    version: String,
    states: HashMap<StateKey, StateRecipe>,
    // Negative cache also prevents repeated I/O for corrupt or missing meshes.
    geometries: Mutex<HashMap<u32, Option<Arc<CaptureGeometryV3>>>>,
    appearances: Mutex<HashMap<u32, Option<Arc<CaptureAppearanceV3>>>>,
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
        if manifest.shared_geometry_pool_schema != 3 || manifest.legacy_meshes_retained {
            return Err("Geometry Pool v3 binary export required".to_owned());
        }
        let binary = if packed.is_some() {
            Some(read_catalog_entry(root, packed.as_ref(), "geometry_pool.bin", 512 * 1024 * 1024)
                .ok_or_else(|| "geometry_pool.bin missing or too large".to_owned())?)
        } else {
            None
        };
        let pool = PoolV3::open(root, binary)?;
        let index: CaptureIndex = serde_json::from_slice(
            &read_catalog_entry(root, packed.as_ref(), "blocks.json", 64 * 1024 * 1024)
                .ok_or_else(|| "blocks.json: missing or too large".to_owned())?,
        )
        .map_err(|e| format!("blocks.json: {e}"))?;
        let mut states = HashMap::with_capacity(index.blocks.len());
        let mut canonical_by_network_id: HashMap<u32, (String, StateRecipe)> = HashMap::new();
        let mut geometry_aliases = Vec::new();
        let mut material_cache: HashMap<u32, HashMap<usize, String>> = HashMap::new();
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
            let Some(material_ref) = block.shared_material_ref else {
                continue;
            };
            let aliases = if let Some(cached) = material_cache.get(&material_ref) {
                cached.clone()
            } else {
                let Some(table) = pool.read(2, material_ref)
                    .and_then(|bytes| serde_json::from_slice::<CaptureMaterialTable>(&bytes).ok())
                else {
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
                material_cache.insert(material_ref, aliases.clone());
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
            pool,
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
        self.shape_for_state_shared(state)
            .map(|shape| (*shape).clone())
    }

    /// Return an immutable shared model instead of cloning its plane and
    /// cuboid arrays on every state-cache hit. Renderer workers can cheaply
    /// retain the same captured geometry across chunk palette lookups.
    pub fn shape_for_state_shared(&self, state: &BlockStateQuery) -> Option<Arc<ModelShape>> {
        let key = query_key(state);
        if let Some(cached) = self.shapes.lock().ok()?.get(&key).cloned() {
            return cached;
        }
        let recipe = self.states.get(&key)?;
        let mut shape = ModelShape::default();
        let mut complete = !recipe.meshes.is_empty();
        for (geometry, appearance) in &recipe.meshes {
            let Some(mesh) = self.shared_geometry(geometry) else {
                complete = false;
                break;
            };
            let Some(appearance) = self.shared_appearance(appearance) else {
                complete = false;
                break;
            };
            let Some(mut planes) =
                shared_mesh_planes(&mesh, &appearance, geometry, &recipe.texture_aliases)
            else {
                complete = false;
                break;
            };
            shape.planes.append(&mut planes);
        }
        // A captured state is an all-or-nothing geometry recipe. Rendering
        // the successfully decoded fragments as if they were complete
        // silently hides missing BlockEntity or connected-model parts.
        // Keep the negative cache and let the explicit fallback path decide.
        if !complete {
            shape = ModelShape::default();
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
        result
    }
    fn shared_geometry(&self, id: &u32) -> Option<Arc<CaptureGeometryV3>> {
        let mut cache = self.geometries.lock().ok()?;
        if let Some(value) = cache.get(id) {
            return value.clone();
        }
        let result = self.pool.read(0, *id)
            .and_then(|bytes| CaptureGeometryV3::parse(&bytes))
            .map(Arc::new);
        cache.insert(*id, result.clone());
        result
    }
    fn shared_appearance(&self, id: &u32) -> Option<Arc<CaptureAppearanceV3>> {
        let mut cache = self.appearances.lock().ok()?;
        if let Some(value) = cache.get(id) {
            return value.clone();
        }
        let result = self.pool.read(1, *id)
            .and_then(|bytes| CaptureAppearanceV3::parse(&bytes))
            .map(Arc::new);
        cache.insert(*id, result.clone());
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
// V3 canonical triangles are sorted independently from source quad order.
// Match two oppositely directed shared edges to recover a *real* quad;
// never invent a face from unrelated triangles or assume adjacent indices.
fn shared_mesh_planes(
    geometry: &CaptureGeometryV3,
    appearance: &CaptureAppearanceV3,
    geometry_id: &u32,
    aliases: &HashMap<usize, String>,
) -> Option<Vec<ModelPlane>> {
    if appearance.geometry_id != *geometry_id
        || !appearance.uv_complete
        || !matches!(appearance.source_kind, 0 | 1)
    {
        return None;
    }
    let count = geometry.triangles.len();
    if count == 0
        || count > 32768
        || count % 2 != 0
        || appearance.corners.len() != count * 3
        || appearance.slots.len() != count
    {
        return None;
    }
    let points = &geometry.vertices;
    let attr = &appearance.corners;
    let slots = &appearance.slots;
    // Index each directed triangle edge once. Native triangulation and
    // deduplicated Geometry Pool ordering need not put a quad's two halves
    // next to one another. Hashing by vertex pair + texture slot reduces the
    // common pairing path from O(triangles²) to O(triangles), while all UV,
    // winding and coplanarity validation remains unchanged.
    let mut half_edges =
        HashMap::<(usize, usize, Option<usize>), Vec<(usize, usize)>>::with_capacity(count * 2);
    for (j, &triangle) in geometry.triangles.iter().enumerate() {
        if triangle.iter().any(|&index| index >= points.len()) {
            return None;
        }
        for edge in 0..3 {
            half_edges.entry((
                triangle[edge], triangle[(edge + 1) % 3], slots[j],
            )).or_default().push((j, edge));
        }
    }
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
        let mut candidates = Vec::new();
        for ra in 0..3 {
            let key = (a[ra], a[(ra + 2) % 3], slots[i]);
            if let Some(mates) = half_edges.get(&key) {
                candidates.extend(
                    mates.iter().copied()
                        .filter(|(j, _)| *j > i && !used[*j])
                        .map(|(j, rb)| (j, ra, rb)),
                );
            }
        }
        // Preserve the original deterministic first-valid pairing: lowest
        // second-triangle index, then first and second edge rotations.
        candidates.sort_unstable();
        'candidate: for (j, ra, rb) in candidates {
            let b = geometry.triangles[j];
            let ai = [ra % 3, (ra + 1) % 3, (ra + 2) % 3];
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
                    if rows.iter().any(|row| row.normal != rows[0].normal) {
                        continue;
                    }
                    let corners = [
                        points[a[ai[0]]],
                        points[a[ai[1]]],
                        points[a[ai[2]]],
                        points[b[bj[2]]],
                    ];
                    let uvs = rows.map(|row| row.uv);
                    if !corners
                        .iter()
                        .flatten()
                        .chain(uvs.iter().flatten())
                        .all(|v| v.is_finite())
                    {
                        continue;
                    }
                    let normal = [
                        rows[0].normal[0].round() as i32,
                        rows[0].normal[1].round() as i32,
                        rows[0].normal[2].round() as i32,
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
    use serde_json::{json, Value};
    use std::io::Write;

    fn u32bytes(values: &[u32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
    fn pool_bytes(groups: &[Vec<Vec<u8>>;4]) -> Vec<u8> {
        let total: usize = groups.iter().map(Vec::len).sum();
        let mut out = b"BSCPOOL3".to_vec();
        out.extend(3u32.to_le_bytes());
        for group in groups {
            out.extend((group.len() as u32).to_le_bytes());
        }
        let mut offset = 28 + 12 * total;
        for group in groups {
            for entry in group {
                out.extend((offset as u64).to_le_bytes());
                out.extend((entry.len() as u32).to_le_bytes());
                offset += entry.len();
            }
        }
        for group in groups {
            for entry in group {
                out.extend(entry);
            }
        }
        out
    }
    fn example_geometry() -> Vec<u8> {
        let mut bytes = u32bytes(&[4,2]);
        for point in [[0f32,0.,0.],[1.,0.,0.],[1.,1.,0.],[0.,1.,0.]] {
            for coord in point {
                bytes.extend(coord.to_le_bytes());
            }
        }
        bytes.extend(u32bytes(&[0,2,3,0,1,2]));
        bytes
    }
    fn example_appearance() -> Vec<u8> {
        let mut bytes = u32bytes(&[0,0,0,1,2]);
        let uv = [[0.,0.],[1.,1.],[0.,1.],[0.,0.],[1.,0.],[1.,1.]];
        for [u,v] in uv {
            for value in [0f32,0.,-1.,u,v] {
                bytes.extend(value.to_le_bytes());
            }
            bytes.extend(u32::MAX.to_le_bytes());
        }
        for _ in 0..2 {
            bytes.extend(u32bytes(&[2,u32::MAX,u32::MAX]));
        }
        bytes
    }
    fn write(root: &Path, name: &str, value: &Value) {
        fs::write(root.join(name), serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn state_nbt(note: i32, alias: bool) -> String {
        let mut root = BTreeMap::new();
        root.insert("name".to_owned(),
            NbtTag::String(if alias {"minecraft:noteblock"} else {"minecraft:oak_stairs"}.into()));
        root.insert("states".to_owned(), NbtTag::Compound(
            BTreeMap::from([((if alias {"note"} else {"weirdo_direction"}).to_owned(),
                NbtTag::Int(note))]).into_iter().collect()));
        NbtWriter::write_root(&NbtTag::Compound(root.into_iter().collect())).unwrap()
            .iter().map(|b| format!("{b:02x}")).collect()
    }
    fn fixture(root: &Path, alias: bool) {
        let groups = [
            vec![example_geometry()],
            vec![example_appearance()],
            vec![serde_json::to_vec(&json!({"schema":2,
                "texture_bindings":[{"slot":2,"resource_alias":"grass_side"}]})).unwrap()],
            vec![]
        ];
        fs::write(root.join("geometry_pool.bin"), pool_bytes(&groups)).unwrap();
        write(root, "manifest.json", &json!({"registry_only":true,
            "target_game_version":"1.26.60.5", "shared_geometry_pool_schema":3,
            "legacy_meshes_retained":false}));
        let name = if alias { "minecraft:noteblock" } else { "minecraft:oak_stairs" };
        let mut blocks = vec![json!({"name":name,"network_id":12,
            "state_nbt_hex":state_nbt(2,alias),"status":"partial",
            "geometry_context":"none","shared_material_ref":0,
            "variants":[{"neighborhood":"registry_visual_bounds",
                         "shared_geometry_refs":[0],"shared_appearance_refs":[0]}]})];
        if alias {
            blocks.push(json!({"name":name,"network_id":13,
                "state_nbt_hex":state_nbt(24,true),"status":"partial",
                "geometry_context":"none","geometry_alias_network_id":12,"variants":[]}));
        }
        write(root, "blocks.json", &json!({"blocks":blocks}));
    }
    #[test]
    fn v3_binary_geometry_uv_material_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), false);
        let catalog = RuntimeCaptureCatalog::open(dir.path()).unwrap();
        assert_eq!(catalog.state_count(),1);
        let state = BlockStateQuery::new("oak_stairs").with_state("weirdo_direction",2);
        let first = catalog.shape_for_state_shared(&state).expect("v3 quad");
        assert_eq!(first.planes.len(),1);
        assert_eq!(first.planes[0].material_slot.as_deref(),
            Some(capture_texture_slot("grass_side").as_str()));
        assert!(first.planes[0].uv.is_some());
        let second = catalog.shape_for_state_shared(&state).unwrap();
        assert!(Arc::ptr_eq(&first,&second));
        assert!(catalog.shape_for_state(&BlockStateQuery::new("oak_stairs")
            .with_state("weirdo_direction",3)).is_none());
    }
    #[test]
    fn v3_rejects_v2_hashes_and_incomplete_pool() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(),false);
        let mut manifest: Value = serde_json::from_slice(
            &fs::read(dir.path().join("manifest.json")).unwrap()).unwrap();
        manifest["shared_geometry_pool_schema"]=json!(2);
        write(dir.path(),"manifest.json",&manifest);
        assert!(RuntimeCaptureCatalog::open(dir.path()).is_err());
        manifest["shared_geometry_pool_schema"]=json!(3);
        write(dir.path(),"manifest.json",&manifest);
        let mut bytes=fs::read(dir.path().join("geometry_pool.bin")).unwrap();
        bytes[28]=255;
        fs::write(dir.path().join("geometry_pool.bin"),bytes).unwrap();
        assert!(RuntimeCaptureCatalog::open(dir.path()).is_err());
    }
    #[test]
    fn v3_invalid_resource_is_negative_cached() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(),false);
        let mut index: Value=serde_json::from_slice(
            &fs::read(dir.path().join("blocks.json")).unwrap()).unwrap();
        index["blocks"][0]["variants"][0]["shared_geometry_refs"]=json!([0,1]);
        index["blocks"][0]["variants"][0]["shared_appearance_refs"]=json!([0,1]);
        write(dir.path(),"blocks.json",&index);
        let catalog=RuntimeCaptureCatalog::open(dir.path()).unwrap();
        let state=BlockStateQuery::new("oak_stairs").with_state("weirdo_direction",2);
        assert!(catalog.shape_for_state_shared(&state).is_none());
    }
    #[test]
    fn v3_note_states_reuse_exact_recipe() {
        let dir=tempfile::tempdir().unwrap();
        fixture(dir.path(),true);
        let catalog=RuntimeCaptureCatalog::open(dir.path()).unwrap();
        assert_eq!(catalog.state_count(),2);
        assert_eq!(
            catalog.shape_for_state(&BlockStateQuery::new("noteblock").with_state("note",2)),
            catalog.shape_for_state(&BlockStateQuery::new("noteblock").with_state("note",24))
        );
    }
    #[test]
    fn v3_embedded_zip_reads_indexed_binary_pool() {
        let dir=tempfile::tempdir().unwrap();
        fixture(dir.path(),false);
        let mut writer=zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts=zip::write::SimpleFileOptions::default();
        for name in ["manifest.json","blocks.json","geometry_pool.bin"] {
            writer.start_file(name,opts).unwrap();
            writer.write_all(&fs::read(dir.path().join(name)).unwrap()).unwrap();
        }
        let bytes=writer.finish().unwrap().into_inner();
        let leaked: &'static [u8]=Box::leak(bytes.into_boxed_slice());
        let catalog=RuntimeCaptureCatalog::open_with_source(Path::new("<embedded>"),Some(leaked)).unwrap();
        let state=BlockStateQuery::new("oak_stairs").with_state("weirdo_direction",2);
        assert_eq!(catalog.shape_for_state_shared(&state).unwrap().planes.len(),1);
    }
    #[test]
    fn v3_diagonal_cross_uv_preserved() {
        let geometry=CaptureGeometryV3 {
            vertices:vec![[0.,0.,0.],[1.,0.,1.],[1.,1.,1.],[0.,1.,0.]],
            triangles:vec![[0,2,3],[0,1,2]],
        };
        let corner=|u:f32,v:f32| pool_v3::Corner {
            normal:[-0.70710677,0.,0.70710677],uv:[u,v],abgr:u32::MAX
        };
        let appearance=CaptureAppearanceV3 {
            geometry_id:0,source_kind:1,uv_complete:true,
            corners:vec![corner(0.,0.),corner(1.,1.),corner(0.,1.),
                         corner(0.,0.),corner(1.,0.),corner(1.,1.)],
            slots:vec![Some(1),Some(1)],
        };
        let result=shared_mesh_planes(&geometry,&appearance,&0,
            &HashMap::from([(1,"grass".into())])).expect("diagonal cross");
        assert_eq!(result.len(),1);
        assert_eq!(result[0].normal,[-1,0,1]);
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
