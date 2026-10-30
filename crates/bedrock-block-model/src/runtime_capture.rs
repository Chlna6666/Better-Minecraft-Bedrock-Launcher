//! Per-installation BedrockSceneCapture geometry. No version-specific model/texture paths
//! are compiled into BMCBL. The index is read once on the 3D worker; shared mesh
//! files are loaded only when the corresponding state is first encountered.
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bedrock_world::{NbtReader, NbtTag};
use serde::Deserialize;

use crate::{BlockStateQuery, BlockStateValue, ModelPlane, ModelShape};

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
        states: query.states.iter().map(|(name, value)| {
            let key = match value {
                BlockStateValue::Bool(v) => StateValueKey::Number(i64::from(*v)),
                BlockStateValue::Int(v) => StateValueKey::Number(*v),
                BlockStateValue::String(v) => StateValueKey::Text(v.clone()),
            };
            (normalized_key(name).to_owned(), key)
        }).collect(),
    }
}
fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len()>16384 || text.len()%2!=0 { return None; }
    text.as_bytes().chunks_exact(2).map(|pair| {
        let h=(pair[0] as char).to_digit(16)?;
        let l=(pair[1] as char).to_digit(16)?;
        Some(((h<<4)|l) as u8)
    }).collect()
}
fn capture_key(block: &CaptureBlock) -> Option<StateKey> {
    let bytes=decode_hex(&block.state_nbt_hex)?;
    let NbtTag::Compound(root)=NbtReader::new(&bytes).parse_root().ok()? else {return None};
    let states=match root.get("states").or_else(||root.get("States")) {
        Some(NbtTag::Compound(m)) => m,
        _ => return None,
    };
    let mut pairs=BTreeMap::new();
    for (name,value) in states {
        let value=match value {
            NbtTag::Byte(v) => StateValueKey::Number(i64::from(*v)),
            NbtTag::Short(v) => StateValueKey::Number(i64::from(*v)),
            NbtTag::Int(v) => StateValueKey::Number(i64::from(*v)),
            NbtTag::Long(v) => StateValueKey::Number(*v),
            NbtTag::String(v) => StateValueKey::Text(v.clone()),
            _ => return None,
        };
        pairs.insert(normalized_key(name).to_owned(),value);
    }
    Some(StateKey {name:normalized_key(&block.name).to_owned(),states:pairs.into_iter().collect()})
}

#[derive(Debug, Deserialize)]
struct CaptureManifest {
    target_game_version: String,
    #[serde(default)]
    registry_only: bool,
}
#[derive(Debug, Deserialize)]
struct CaptureIndex {
    blocks: Vec<CaptureBlock>,
}
#[derive(Debug, Deserialize)]
struct CaptureBlock {
    name: String,
    state_nbt_hex: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    geometry_context: String,
    #[serde(default)]
    texture_bindings: Vec<CaptureTexture>,
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
    mesh_refs: Vec<String>,
    #[serde(default)]
    mesh_triangle_texture_slots: Vec<Vec<Option<usize>>>,
}
#[derive(Debug, Deserialize)]
struct CaptureMesh {
    topology: String,
    geometry_source: String,
    uv_space: String,
    vertices: Vec<Vec<serde_json::Value>>,
    indices: Vec<usize>,
    #[serde(default)]
    face_index_per_triangle: Vec<usize>,
}

#[derive(Clone, Debug)]
struct StateRecipe {
    meshes: Vec<(String, Vec<Option<usize>>)>,
    texture_aliases: HashMap<usize,String>,
}

/// The BMCBL model cache is scoped to a game installation, not to a process-global
/// Minecraft version. No path or material table is compiled into the executable.
#[derive(Debug)]
pub struct RuntimeCaptureCatalog {
    root: PathBuf,
    version: String,
    states: HashMap<StateKey,StateRecipe>,
    // Negative cache also prevents repeated I/O for corrupt or missing meshes.
    meshes: Mutex<HashMap<String,Option<Arc<CaptureMesh>>>>,
    shapes: Mutex<HashMap<StateKey,Option<Arc<ModelShape>>>>,
}
impl PartialEq for RuntimeCaptureCatalog {
    fn eq(&self,other:&Self)->bool { self.root==other.root && self.version==other.version }
}
impl Eq for RuntimeCaptureCatalog {}

impl RuntimeCaptureCatalog {
    /// Parses the state index once. The heavy geometry blobs remain on disk until used.
    pub fn open(root: impl AsRef<Path>) -> Result<Self,String> {
        let root=root.as_ref();
        let manifest:CaptureManifest=serde_json::from_slice(&fs::read(root.join("manifest.json"))
            .map_err(|e|format!("manifest.json: {e}"))?).map_err(|e|format!("manifest.json: {e}"))?;
        if !manifest.registry_only { return Err("capture is not registry-only".to_owned()); }
        let index:CaptureIndex=serde_json::from_slice(&fs::read(root.join("blocks.json"))
            .map_err(|e|format!("blocks.json: {e}"))?).map_err(|e|format!("blocks.json: {e}"))?;
        let mut states=HashMap::with_capacity(index.blocks.len());
        for block in index.blocks {
            // Neighbor-driven and BlockEntity geometry must come from explicit
            // connection/NBT rules, never from a single static visual-bounds mesh.
            if !matches!(block.geometry_context.as_str(),""|"none") ||
                matches!(block.status.as_str(),"unsupported"|"dynamic") {
                continue;
            }
            let Some(key)=capture_key(&block) else {continue};
            let aliases=block.texture_bindings.into_iter()
                .filter(|x|!x.resource_alias.is_empty())
                .map(|x|(x.slot,x.resource_alias))
                .collect();
            let Some(variant)=block.variants.into_iter().find(|v|v.neighborhood=="registry_visual_bounds") else {continue};
            let meshes=variant.mesh_refs.into_iter()
                .zip(variant.mesh_triangle_texture_slots)
                .filter(|(hash,_)|hash.len()==64 && hash.bytes().all(|b|b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
                .collect();
            states.insert(key,StateRecipe{meshes,texture_aliases:aliases});
        }
        Ok(Self{root:root.to_owned(),version:manifest.target_game_version,states,
            meshes:Mutex::new(HashMap::new()),shapes:Mutex::new(HashMap::new())})
    }
    #[must_use]
    pub fn game_version(&self)->&str { &self.version }
    #[must_use]
    pub fn state_count(&self)->usize { self.states.len() }

    /// Exact state match. Never silently use a different orientation or
    /// a different set of wall/fence/stair connection states.
    pub fn shape_for_state(&self,state:&BlockStateQuery)->Option<ModelShape>{
        let key=query_key(state);
        if let Some(cached)=self.shapes.lock().ok()?.get(&key).cloned(){
            return cached.as_ref().map(|s|(**s).clone());
        }
        let recipe=self.states.get(&key)?;
        let mut shape=ModelShape::default();
        for (hash,slots) in &recipe.meshes {
            let Some(mesh)=self.mesh(hash) else {continue};
            let Some(mut planes)=mesh_planes(&mesh,slots,&recipe.texture_aliases) else {continue};
            shape.planes.append(&mut planes);
        }
        let result=(!shape.is_empty()).then(||Arc::new(shape));
        if let Ok(mut cache)=self.shapes.lock(){cache.insert(key,result.clone());}
        result.as_ref().map(|s|(**s).clone())
    }
    fn mesh(&self,hash:&str)->Option<Arc<CaptureMesh>>{
        let mut cache=self.meshes.lock().ok()?;
        if let Some(v)=cache.get(hash){return v.clone();}
        let file=self.root.join("meshes").join(format!("{hash}.json"));
        let result=fs::metadata(&file).ok()
            .filter(|m|m.len()<=4*1024*1024)
            .and_then(|_|fs::read(file).ok())
            .and_then(|data|serde_json::from_slice::<CaptureMesh>(&data).ok())
            .filter(|m|m.topology=="triangles" && m.uv_space=="block_face_local"
                && matches!(m.geometry_source.as_str(),"registry_visual_bounds"|"registry_cross_planes"))
            .map(Arc::new);
        cache.insert(hash.to_owned(),result.clone());
        result
    }
}

/// Encodes an engine texture *alias*, never a version-specific file path.
/// The OBJ resolver reverses this encoding and resolves it through the active
/// local terrain_texture.json / resource-pack stack.
pub fn capture_texture_slot(alias:&str)->String {
    const HEX:&[u8;16]=b"0123456789abcdef";
    let mut out=String::with_capacity(4+alias.len()*2);
    out.push_str("bsc_");
    for byte in alias.bytes(){
        out.push(HEX[usize::from(byte>>4)] as char);
        out.push(HEX[usize::from(byte&15)] as char);
    }
    out
}
pub fn decode_capture_texture_slot(slot:&str)->Option<String>{
    let hex=slot.strip_prefix("bsc_")?;
    if hex.len()%2!=0 || hex.len()>1024{return None}
    let bytes=decode_hex(hex)?;
    String::from_utf8(bytes).ok()
}
fn mesh_planes(mesh:&CaptureMesh,slots:&[Option<usize>],aliases:&HashMap<usize,String>)
    ->Option<Vec<ModelPlane>>{
    if mesh.indices.len()%6!=0 || slots.len()!=mesh.indices.len()/3 {return None}
    let mut planes=Vec::with_capacity(mesh.indices.len()/6);
    for (quad,idx) in mesh.indices.chunks_exact(6).enumerate(){
        if idx[0]!=idx[3] || idx[2]!=idx[4] {return None}
        let (Some(a),Some(b),Some(c),Some(d)) =
            (mesh.vertices.get(idx[0]),mesh.vertices.get(idx[1]),
             mesh.vertices.get(idx[2]),mesh.vertices.get(idx[5]))
            else {return None};
        let mut corners=[[0f32;3];4];
        let mut uv=[[0f32;2];4];
        for (i,row) in [a,b,c,d].into_iter().enumerate(){
            if row.len()!=9{return None}
            for axis in 0..3{corners[i][axis]=row[axis].as_f64()? as f32;}
            for axis in 0..2{uv[i][axis]=row[axis+6].as_f64()? as f32;}
            if !corners[i].iter().chain(uv[i].iter()).all(|v|v.is_finite()){return None}
        }
        let nx=a[3].as_f64()?.round() as i32;
        let ny=a[4].as_f64()?.round() as i32;
        let nz=a[5].as_f64()?.round() as i32;
        let first=slots[quad*2];let second=slots[quad*2+1];
        if first!=second {return None}
        let mut plane=ModelPlane::new(corners,[nx,ny,nz]).with_uv(uv).front_only();
        if let Some(alias)=first.and_then(|id|aliases.get(&id)){
            plane=plane.with_material_slot(capture_texture_slot(alias));
        }
        planes.push(plane);
    }
    Some(planes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn texture_alias_roundtrip_without_version_index(){
        for alias in ["stone","minecraft:grass_side","textures/blocks/oak_log_top","红色"] {
            assert_eq!(decode_capture_texture_slot(&capture_texture_slot(alias)).as_deref(),Some(alias));
        }
        assert!(decode_capture_texture_slot("bsc_xyz").is_none());
    }
    #[test]
    fn face_uv_and_slot_identity_survive_mesh_decode(){
        let v=|x:f32,y:f32|vec![serde_json::json!(x),serde_json::json!(y),serde_json::json!(0),
            serde_json::json!(0),serde_json::json!(0),serde_json::json!(-1),
            serde_json::json!(x),serde_json::json!(y),serde_json::json!(4294967295u32)];
        let mesh=CaptureMesh{topology:"triangles".into(),geometry_source:"registry_visual_bounds".into(),
            uv_space:"block_face_local".into(),vertices:vec![v(0.,0.),v(0.,1.),v(1.,1.),v(1.,0.)],
            indices:vec![0,1,2,0,2,3],face_index_per_triangle:vec![2,2]};
        let aliases=HashMap::from([(2,"grass_side".into())]);
        let planes=mesh_planes(&mesh,&[Some(2),Some(2)],&aliases).expect("quad");
        assert_eq!(planes.len(),1);
        assert_eq!(planes[0].material_slot.as_deref(),
            Some(capture_texture_slot("grass_side").as_str()));
        assert_eq!(planes[0].normal,[0,0,-1]);
        assert_eq!(planes[0].uv.unwrap()[3],[1.,0.]);
        assert!(mesh_planes(&mesh,&[Some(2),None],&aliases).is_none());
    }
    #[test]
    fn catalog_matches_exact_nbt_states_and_keeps_face_aliases(){
        use bedrock_world::NbtWriter;
        let dir=tempfile::tempdir().expect("temp catalog");
        std::fs::create_dir(dir.path().join("meshes")).expect("mesh folder");
        let NbtTag::Compound(mut root)=
            NbtReader::new(&NbtWriter::write_root(&NbtTag::Compound(Default::default()))
                .expect("root")).parse_root().expect("NBT") else {panic!("compound")};
        let mut states=std::collections::BTreeMap::new();
        states.insert("weirdo_direction".into(),NbtTag::Int(2));
        root.insert("name".into(),NbtTag::String("minecraft:oak_stairs".into()));
        root.insert("states".into(),NbtTag::Compound(states.into_iter().collect()));
        let bytes=NbtWriter::write_root(&NbtTag::Compound(root)).expect("write block state");
        let hex=bytes.iter().map(|byte|format!("{byte:02x}")).collect::<String>();
        let v=|x:f32,y:f32|serde_json::json!([x,y,0.,0.,0.,-1.,x,y,4294967295_u32]);
        let mesh=serde_json::json!({"topology":"triangles","uv_space":"block_face_local",
            "geometry_source":"registry_visual_bounds",
            "vertices":[v(0.,0.),v(0.,1.),v(1.,1.),v(1.,0.)],
            "indices":[0,1,2,0,2,3],"face_index_per_triangle":[2,2]});
        let digest="b".repeat(64);
        std::fs::write(dir.path().join("meshes").join(format!("{digest}.json")),
            serde_json::to_vec(&mesh).expect("mesh json")).expect("mesh");
        std::fs::write(dir.path().join("manifest.json"),
            r#"{"registry_only":true,"target_game_version":"1.26.51.1"}"#).expect("manifest");
        let blocks=serde_json::json!({"blocks":[{"name":"minecraft:oak_stairs",
            "state_nbt_hex":hex,"status":"partial","geometry_context":"none",
            "texture_bindings":[{"slot":2,"resource_alias":"grass_side"}],
            "variants":[{"neighborhood":"registry_visual_bounds",
                "mesh_refs":[digest],"mesh_triangle_texture_slots":[[2,2]]}]}]});
        std::fs::write(dir.path().join("blocks.json"),
            serde_json::to_vec(&blocks).expect("block json")).expect("blocks");
        let catalog=RuntimeCaptureCatalog::open(dir.path()).expect("catalog");
        assert_eq!(catalog.state_count(),1);
        let state=BlockStateQuery::new("oak_stairs").with_state("weirdo_direction",2);
        let shape=catalog.shape_for_state(&state).expect("exact state");
        assert_eq!(shape.planes.len(),1);
        assert_eq!(shape.planes[0].normal,[0,0,-1]);
        assert_eq!(shape.planes[0].material_slot.as_deref(),
            Some(capture_texture_slot("grass_side").as_str()));
        assert!(catalog.shape_for_state(
            &BlockStateQuery::new("oak_stairs").with_state("weirdo_direction",1)).is_none());
    }
    #[test]
    fn key_normalizes_namespace_without_erasing_state(){
        let a=BlockStateQuery::new("minecraft:cobblestone_wall")
            .with_state("minecraft:wall_connection_type_north","tall");
        let b=BlockStateQuery::new("cobblestone_wall")
            .with_state("wall_connection_type_north","tall");
        let c=BlockStateQuery::new("cobblestone_wall")
            .with_state("wall_connection_type_north","low");
        assert_eq!(query_key(&a),query_key(&b));
        assert_ne!(query_key(&a),query_key(&c));
    }
}
